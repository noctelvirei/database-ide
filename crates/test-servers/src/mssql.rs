//! Blocking SQL Server test databases on an existing server.
//!
//! `DBIDE_MSSQL` must contain `host,port`. Each handle owns a fresh database
//! and SQL login; app locks protect live databases from another process's sweep.

use mssql_tds::connection::client_context::{ClientContext, TdsAuthenticationMethod};
use mssql_tds::connection::tds_client::{ResultSet, StatementResult, TdsClient};
use mssql_tds::connection_provider::tds_connection_provider::TdsConnectionProvider;
use mssql_tds::core::EncryptionSetting;
use mssql_tds::datatypes::column_values::ColumnValues;
use mssql_tds::error::Error as TdsError;
use rand::{Rng, rng};
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tokio::runtime::Runtime;

const PREFIX: &str = "dbide_test_";
const ADMIN_DATABASE: &str = "master";
static SWEPT: Mutex<bool> = Mutex::new(false);

/// A harness failure; SQL Server errors retain driver diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// DBIDE_MSSQL must be set to a host and TCP port separated by a comma.
    #[error("Set DBIDE_MSSQL to the SQL Server host,port (for example localhost,1433)")]
    MissingServer,
    /// DBIDE_MSSQL did not contain a valid host and TCP port.
    #[error("DBIDE_MSSQL must contain a host and TCP port, for example localhost,1433")]
    InvalidServer,
    /// SQL Server rejected a command or the TDS connection failed.
    #[error("SQL Server harness failed: {0}")]
    Tds(#[from] TdsError),
    /// Reading a SQL script failed.
    #[error("Could not read SQL script: {0}")]
    Io(#[from] std::io::Error),
    /// The server refused an application lock, so cleanup cannot safely proceed.
    #[error("Could not acquire SQL Server test harness lock: {0}")]
    Lock(String),
    /// A SQL client lock was poisoned by a panic.
    #[error("SQL Server harness client lock was poisoned")]
    Poisoned,
}

/// Owns one throwaway database and SQL login on the configured server.
///
/// Calls block while running TDS operations. Dropping the handle drops its
/// database and login; SQL scripts are executed as separate `GO` batches.
pub struct Mssql {
    runtime: Option<Runtime>,
    admin: Option<TdsClient>,
    client: Mutex<Option<TdsClient>>,
    host: String,
    port: u16,
    database: String,
    user: String,
    password: String,
}

impl fmt::Debug for Mssql {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mssql")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl Mssql {
    /// Connects with Windows SSPI, sweeps abandoned harness objects, and creates
    /// a database with a dedicated SQL login. Requires DBIDE_MSSQL=`host,port`.
    pub fn start() -> Result<Self, Error> {
        let (host, port) = server_address()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        {
            let mut swept = SWEPT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !*swept {
                // Sweep locks belong to a short-lived connection, never to a test handle.
                let mut sweeper = runtime.block_on(connect(&host, port, ADMIN_DATABASE, None))?;
                let result = sweep(&runtime, &mut sweeper);
                let closed = runtime.block_on(sweeper.close_connection());
                result?;
                closed?;
                *swept = true;
            }
        }
        let mut admin = runtime.block_on(connect(&host, port, ADMIN_DATABASE, None))?;

        let suffix = random_lowercase(20);
        let database = format!("{PREFIX}{suffix}");
        let user = database.clone();
        let password = random_lowercase(40);
        // Protect both names before either object becomes visible to other processes' sweeps.
        runtime.block_on(acquire_lock(&mut admin, &database))?;
        let result = (|| {
            run_admin(
                &runtime,
                &mut admin,
                &format!(
                    "CREATE LOGIN [{}] WITH PASSWORD = N'{}', CHECK_POLICY = OFF; CREATE DATABASE [{}];",
                    user, password, database
                ),
            )?;
            run_admin(
                &runtime,
                &mut admin,
                &format!("ALTER DATABASE [{database}] SET TRUSTWORTHY OFF;"),
            )?;
            run_admin(
                &runtime,
                &mut admin,
                &format!(
                    "USE [{database}]; CREATE USER [{user}] FOR LOGIN [{user}]; ALTER ROLE db_owner ADD MEMBER [{user}]; USE [master];"
                ),
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            cleanup(&runtime, &mut admin, &database, &user);
            let _ = runtime.block_on(admin.close_connection());
            return Err(error);
        }
        let client =
            match runtime.block_on(connect(&host, port, &database, Some((&user, &password)))) {
                Ok(client) => client,
                Err(error) => {
                    cleanup(&runtime, &mut admin, &database, &user);
                    let _ = runtime.block_on(admin.close_connection());
                    return Err(error.into());
                }
            };
        Ok(Self {
            runtime: Some(runtime),
            admin: Some(admin),
            client: Mutex::new(Some(client)),
            host,
            port,
            database,
            user,
            password,
        })
    }

    /// Configured SQL Server host.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Configured SQL Server TCP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// SQL login created for this database only.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// Random SQL login password; omitted from Debug output.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Database name, valid while this handle remains alive.
    pub fn database(&self) -> &str {
        &self.database
    }

    /// Executes SQL and returns headerless, tab-separated rows; NULL is empty text.
    /// Statements with no result rows contribute no output.
    pub fn query(&self, sql: &str) -> Result<String, Error> {
        let mut client = self.client.lock().map_err(|_| Error::Poisoned)?;
        let client = client.as_mut().expect("live handle owns a SQL client");
        Ok(self
            .runtime
            .as_ref()
            .expect("live handle owns a runtime")
            .block_on(query(client, sql))?)
    }

    /// Reads and runs a SQL script, splitting standalone `GO [repeat]` lines.
    pub fn load_script(&self, script: impl AsRef<Path>) -> Result<(), Error> {
        let source = fs::read_to_string(script)?;
        for (batch, repeat) in split_batches(&source) {
            for _ in 0..repeat {
                self.query(&batch)?;
            }
        }
        Ok(())
    }
}

impl Drop for Mssql {
    fn drop(&mut self) {
        let client = self
            .client
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(client.take());
        if let (Some(runtime), Some(mut admin)) = (self.runtime.take(), self.admin.take()) {
            let database = self.database.clone();
            let user = self.user.clone();
            if std::thread::spawn(move || {
                cleanup(&runtime, &mut admin, &database, &user);
                if let Err(error) = runtime.block_on(admin.close_connection()) {
                    eprintln!("Could not close SQL Server harness admin connection: {error}");
                }
                drop(admin);
                drop(runtime);
            })
            .join()
            .is_err()
            {
                eprintln!("SQL Server harness cleanup thread panicked");
            }
        }
    }
}

fn server_address() -> Result<(String, u16), Error> {
    let value = std::env::var("DBIDE_MSSQL").map_err(|_| Error::MissingServer)?;
    let Some((host, port)) = value.rsplit_once(',') else {
        return Err(Error::InvalidServer);
    };
    let port = port
        .trim()
        .parse::<u16>()
        .map_err(|_| Error::InvalidServer)?;
    if host.trim().is_empty() || port == 0 {
        return Err(Error::InvalidServer);
    }
    Ok((host.trim().to_owned(), port))
}

fn random_lowercase(length: usize) -> String {
    let mut random = rng();
    (0..length)
        .map(|_| char::from(random.random_range(b'a'..=b'z')))
        .collect()
}

async fn connect(
    host: &str,
    port: u16,
    database: &str,
    credentials: Option<(&str, &str)>,
) -> Result<TdsClient, TdsError> {
    let datasource = format!("tcp:{host},{port}");
    let mut context = ClientContext::with_data_source(&datasource);
    context.database = database.to_owned();
    context.application_name = "Database IDE test harness".to_owned();
    context.connect_timeout = 5;
    context.connect_retry_count = 0;
    context.encryption_options.mode = EncryptionSetting::On;
    context.encryption_options.trust_server_certificate = true;
    if let Some((user, password)) = credentials {
        context.user_name = user.to_owned();
        context.password = password.to_owned();
        context.tds_authentication_method = TdsAuthenticationMethod::Password;
    } else {
        context.tds_authentication_method = TdsAuthenticationMethod::SSPI;
    }
    TdsConnectionProvider::new()
        .create_client(context, &datasource, None)
        .await
}

fn run_admin(runtime: &Runtime, client: &mut TdsClient, sql: &str) -> Result<(), Error> {
    runtime
        .block_on(query(client, sql))
        .map(|_| ())
        .map_err(Error::from)
}

async fn query(client: &mut TdsClient, sql: &str) -> Result<String, TdsError> {
    let mut output = String::new();
    let mut result = client.execute(sql.to_owned(), ()).await?;
    loop {
        match result {
            StatementResult::Rows => {
                while let Some(row) = client.next_row().await? {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&row.iter().map(format_value).collect::<Vec<_>>().join("\t"));
                }
            }
            StatementResult::NoRows { .. } => {}
            StatementResult::End => break,
        }
        result = client.advance().await?;
    }
    client.close_query().await?;
    Ok(output)
}

fn format_value(value: &ColumnValues) -> String {
    match value {
        ColumnValues::String(value) => value.to_string(),
        ColumnValues::Null => String::new(),
        ColumnValues::TinyInt(value) => value.to_string(),
        ColumnValues::SmallInt(value) => value.to_string(),
        ColumnValues::Int(value) => value.to_string(),
        ColumnValues::BigInt(value) => value.to_string(),
        ColumnValues::Real(value) => value.to_string(),
        ColumnValues::Float(value) => value.to_string(),
        ColumnValues::Bit(value) => value.to_string(),
        value => format!("{value:?}"),
    }
}

async fn acquire_lock(client: &mut TdsClient, resource: &str) -> Result<(), Error> {
    let sql = format!(
        "DECLARE @lock_result int; EXEC @lock_result = sys.sp_getapplock @Resource = N'{resource}', @LockMode = 'Exclusive', @LockOwner = 'Session', @LockTimeout = 0; IF @lock_result < 0 THROW 51000, 'Could not acquire test harness application lock', 1;"
    );
    query(client, &sql)
        .await
        .map(|_| ())
        .map_err(|error| Error::Lock(error.to_string()))
}

async fn release_lock(client: &mut TdsClient, resource: &str) -> Result<(), Error> {
    query(
        client,
        &format!("EXEC sys.sp_releaseapplock @Resource = N'{resource}', @LockOwner = 'Session';"),
    )
    .await
    .map(|_| ())
    .map_err(|error| Error::Lock(error.to_string()))
}

fn sweep(runtime: &Runtime, admin: &mut TdsClient) -> Result<(), Error> {
    let candidates = runtime
        .block_on(query(admin, "SELECT name FROM sys.databases WHERE name LIKE N'dbide[_]test[_]%' AND name NOT IN (N'master', N'model', N'msdb', N'tempdb');"))?;
    let mut live_logins = Vec::new();
    for database in candidates.lines().filter(|name| safe_name(name)) {
        let lock = runtime.block_on(acquire_lock(admin, database));
        if lock.is_err() {
            live_logins.push(database);
            continue;
        }
        let active = runtime.block_on(query(
            admin,
            &format!(
                "USE [master]; IF EXISTS (SELECT 1 FROM sys.sysprocesses WHERE dbid = DB_ID(N'{database}') AND spid <> @@SPID) SELECT N'active';"
            ),
        ))?;
        if !active.is_empty() {
            live_logins.push(database);
            runtime.block_on(release_lock(admin, database))?;
            continue;
        }
        let login_active = runtime.block_on(query(
            admin,
            &format!(
                "IF EXISTS (SELECT 1 FROM sys.sysprocesses WHERE loginame = N'{database}' AND spid <> @@SPID) SELECT N'active';"
            ),
        ))?;
        if !login_active.is_empty() {
            live_logins.push(database);
        }
        run_admin(
            runtime,
            admin,
            &format!(
                "IF DB_ID(N'{database}') IS NOT NULL DROP DATABASE [{database}]; IF NOT EXISTS (SELECT 1 FROM sys.sysprocesses WHERE loginame = N'{database}' AND spid <> @@SPID) AND EXISTS (SELECT 1 FROM sys.server_principals WHERE name = N'{database}') DROP LOGIN [{database}];"
            ),
        )?;
    }
    let logins = runtime.block_on(query(
        admin,
        "SELECT name FROM sys.server_principals WHERE type = 'S' AND name LIKE N'dbide[_]test[_]%';",
    ))?;
    for login in logins.lines().filter(|name| safe_name(name)) {
        if live_logins.contains(&login) || runtime.block_on(acquire_lock(admin, login)).is_err() {
            continue;
        }
        run_admin(
            runtime,
            admin,
            &format!(
                "IF NOT EXISTS (SELECT 1 FROM sys.sysprocesses WHERE loginame = N'{login}' AND spid <> @@SPID) AND EXISTS (SELECT 1 FROM sys.server_principals WHERE name = N'{login}') DROP LOGIN [{login}];"
            ),
        )?;
    }
    Ok(())
}

fn cleanup(runtime: &Runtime, admin: &mut TdsClient, database: &str, user: &str) {
    // This handle owns the generated database, including test connections still closing.
    // The leftover sweep never forces another user's active database closed.
    if let Err(error) = run_admin(
        runtime,
        admin,
        &format!(
            "USE [master]; IF DB_ID(N'{database}') IS NOT NULL BEGIN ALTER DATABASE [{database}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{database}]; END;"
        ),
    ) {
        eprintln!("Could not clean up SQL Server test database {database}: {error}");
    }
    // A test may have changed its current database. Only this handle's SQL login is retired.
    let remove_login = async {
        let sessions = query(admin, &format!("SELECT session_id FROM sys.dm_exec_sessions WHERE original_login_name = N'{user}' AND session_id <> @@SPID")).await?;
        for id in sessions.lines().filter_map(|id| id.parse::<u16>().ok()) {
            query(admin, &format!("BEGIN TRY IF EXISTS (SELECT 1 FROM sys.dm_exec_sessions WHERE session_id = {id} AND original_login_name = N'{user}') KILL {id}; END TRY BEGIN CATCH IF ERROR_NUMBER() <> 6106 THROW; END CATCH")).await?;
        }
        let sql = format!(
            "IF EXISTS (SELECT 1 FROM sys.server_principals WHERE name = N'{user}') DROP LOGIN [{user}]"
        );
        for attempt in 0..20 {
            match query(admin, &sql).await {
                Ok(_) => return Ok(()),
                Err(TdsError::SqlServerError { diagnostics })
                    if attempt < 19
                        && diagnostics.errors.iter().all(|error| error.number == 15434) =>
                {
                    // KILL can leave rollback finishing after its acknowledgement.
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the final attempt returns success or its error")
    };
    if let Err(error) = runtime.block_on(remove_login) {
        eprintln!("Could not remove SQL Server test login {user}: {error}");
    }
}

fn safe_name(name: &str) -> bool {
    name.starts_with(PREFIX)
        && name.len() > PREFIX.len()
        && name[PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LexState {
    Normal,
    SingleQuote,
    DoubleQuote,
    Bracket,
    LineComment,
    BlockComment(u32),
}

fn split_batches(source: &str) -> Vec<(String, usize)> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut batches = Vec::new();
    let mut current = String::new();
    let mut state = LexState::Normal;
    for line in source.split_inclusive('\n') {
        if state == LexState::Normal
            && let Some(repeat) = go_repeat(line.trim_end_matches(['\r', '\n']))
        {
            if !current.trim().is_empty() {
                batches.push((std::mem::take(&mut current), repeat));
            }
            continue;
        }
        current.push_str(line);
        scan_line(line, &mut state);
    }
    if !current.trim().is_empty() {
        batches.push((current, 1));
    }
    batches
}

fn go_repeat(line: &str) -> Option<usize> {
    let mut parts = line.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("GO") {
        return None;
    }
    let repeat = match parts.next() {
        Some(number) => number.parse::<usize>().ok().filter(|count| *count > 0)?,
        None => 1,
    };
    if parts.next().is_some_and(|tail| !tail.starts_with("--")) {
        return None;
    }
    Some(repeat)
}

fn scan_line(line: &str, state: &mut LexState) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let next = chars.get(i + 1).copied();
        match *state {
            LexState::Normal => match (chars[i], next) {
                ('-', Some('-')) => *state = LexState::LineComment,
                ('/', Some('*')) => {
                    *state = LexState::BlockComment(1);
                    i += 1;
                }
                ('\'', _) => *state = LexState::SingleQuote,
                ('"', _) => *state = LexState::DoubleQuote,
                ('[', _) => *state = LexState::Bracket,
                _ => {}
            },
            LexState::SingleQuote if chars[i] == '\'' => {
                if next == Some('\'') {
                    i += 1;
                } else {
                    *state = LexState::Normal;
                }
            }
            LexState::DoubleQuote if chars[i] == '"' => {
                if next == Some('"') {
                    i += 1;
                } else {
                    *state = LexState::Normal;
                }
            }
            LexState::Bracket if chars[i] == ']' => {
                if next == Some(']') {
                    i += 1;
                } else {
                    *state = LexState::Normal;
                }
            }
            LexState::LineComment if chars[i] == '\n' => *state = LexState::Normal,
            LexState::BlockComment(depth) => match (chars[i], next) {
                ('/', Some('*')) => {
                    *state = LexState::BlockComment(depth + 1);
                    i += 1;
                }
                ('*', Some('/')) => {
                    *state = if depth == 1 {
                        LexState::Normal
                    } else {
                        LexState::BlockComment(depth - 1)
                    };
                    i += 1;
                }
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
    if *state == LexState::LineComment {
        *state = LexState::Normal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_go_repeat_keeps_text_and_execution_count() {
        assert_eq!(
            split_batches("SELECT 1\r\ngo 3\r\nSELECT 2"),
            vec![("SELECT 1\r\n".into(), 3), ("SELECT 2".into(), 1)]
        );
    }

    #[test]
    fn script_go_inside_quotes_and_nested_comments_is_sql() {
        let sql = "SELECT 'it''s\nGO\ntext';\n/* outer /* inner */\nGO\n*/\nSELECT [x]]\nGO\ny]";
        assert_eq!(split_batches(sql), vec![(sql.into(), 1)]);
    }

    #[test]
    fn overlapping_comment_markers_do_not_close_a_comment() {
        let sql = "/*/\nGO\n*/\nSELECT 1";
        assert_eq!(split_batches(sql), vec![(sql.into(), 1)]);
    }

    #[test]
    fn cleanup_names_require_an_exact_prefix_and_nonempty_safe_suffix() {
        assert!(!safe_name("dbide_test_"));
        assert!(!safe_name("dbide_test_a]; DROP DATABASE x"));
        assert!(!safe_name("dbideXtest_a"));
        assert!(safe_name("dbide_test_abc"));
    }
    #[test]
    fn script_leading_bom_is_not_sql_and_embedded_bom_is_preserved() {
        assert_eq!(
            split_batches("\u{feff}SELECT '\u{feff}'\nGO\nSELECT 2"),
            vec![("SELECT '\u{feff}'\n".into(), 1), ("SELECT 2".into(), 1),]
        );
        assert!(split_batches("\u{feff}").is_empty());
    }
}
