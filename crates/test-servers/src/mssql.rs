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
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

pub mod wait;

const PREFIX: &str = "dbide_test_";
const ADMIN_DATABASE: &str = "master";
static SWEPT: Mutex<bool> = Mutex::new(false);
const DDL_LOCK: &str = "dbide_test_harness_ddl";
/// Match the DDL budget so contention with a slow creator is not mistaken for a hang.
const DDL_LOCK_TIMEOUT_MS: u32 = 120_000;
const LOCK_RESPONSE_GRACE: Duration = Duration::from_secs(5);
/// Model locks and killed-session rollback can outlast one attempt on a busy server.
const RETRY_TIMEOUT: Duration = Duration::from_secs(120);
const RETRY_INTERVAL: Duration = Duration::from_millis(500);
/// Allow TCP, TLS and SSPI/login to finish under load, without an unlimited connection attempt.
pub const CONNECT_TIMEOUT_SECS: u32 = 30;

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
    #[error("SQL Server harness failed: {}", tds_message(.0))]
    Tds(#[from] TdsError),
    /// Reading a SQL script failed.
    #[error("Could not read SQL script: {0}")]
    Io(#[from] std::io::Error),
    /// The server refused an application lock, so cleanup cannot safely proceed.
    #[error("Could not acquire SQL Server test harness lock: {0}")]
    Lock(String),
    /// A harness operation exceeded its deadline, with available server wait diagnostics.
    #[error("SQL Server harness wait: {0}")]
    Wait(String),
    /// A SQL client lock was poisoned by a panic.
    #[error("SQL Server harness client lock was poisoned")]
    Poisoned,
}

/// Owns one throwaway database and SQL login on the configured server.
///
/// Calls block while running TDS operations. Dropping the handle drops its
/// database and login; SQL scripts are executed as separate `GO` batches.
/// Cleanup failures panic unless the thread is already unwinding.
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
                let mut sweeper = runtime.block_on(connect(&host, port, ADMIN_DATABASE, None))?;
                let result = (|| {
                    runtime.block_on(acquire_lock(&mut sweeper, DDL_LOCK, DDL_LOCK_TIMEOUT_MS))?;
                    sweep(&runtime, &mut sweeper)
                })();
                let closed = runtime.block_on(wait::close(&mut sweeper));
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
        runtime.block_on(acquire_lock(&mut admin, DDL_LOCK, DDL_LOCK_TIMEOUT_MS))?;
        if runtime.block_on(acquire_lock(&mut admin, &database, 0))? == -1 {
            return Err(Error::Lock(format!("{database}: name is already owned")));
        }
        let result = (|| {
            run_admin(
                &runtime,
                &mut admin,
                &format!(
                    "CREATE LOGIN [{user}] WITH PASSWORD = N'{password}', CHECK_POLICY = OFF;"
                ),
            )?;
            create_database(&runtime, &mut admin, &database)?;
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
        let released = runtime.block_on(release_lock(&mut admin, DDL_LOCK));
        if let Err(error) = result.and(released) {
            let _ = cleanup(&runtime, &mut admin, &database, &user);
            let _ = runtime.block_on(wait::close(&mut admin));
            return Err(error);
        }
        let client =
            match runtime.block_on(connect(&host, port, &database, Some((&user, &password)))) {
                Ok(client) => client,
                Err(error) => {
                    let _ = cleanup(&runtime, &mut admin, &database, &user);
                    let _ = runtime.block_on(wait::close(&mut admin));
                    return Err(error);
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
    /// Statements with no result rows contribute no output. A query exceeding
    /// 120 seconds fails with server wait diagnostics and retires its connection.
    /// Server lock-timeout errors include lock owners and leave the connection usable.
    pub fn query(&self, sql: &str) -> Result<String, Error> {
        let mut client = self.client.lock().map_err(|_| Error::Poisoned)?;
        let client = client.as_mut().expect("live handle owns a SQL client");
        self.runtime
            .as_ref()
            .expect("live handle owns a runtime")
            .block_on(wait::query(
                client,
                sql,
                &format!("query on {}", self.database),
            ))
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
        let client = client.take();
        if let (Some(runtime), Some(mut admin)) = (self.runtime.take(), self.admin.take()) {
            let database = self.database.clone();
            let user = self.user.clone();
            let result = std::thread::spawn(move || {
                let closed = match client {
                    Some(mut client) => runtime.block_on(wait::close(&mut client)),
                    None => Ok(()),
                };
                let removed = cleanup(&runtime, &mut admin, &database, &user);
                let admin_closed = runtime.block_on(wait::close(&mut admin));
                drop(admin);
                drop(runtime);
                closed.and(removed).and(admin_closed)
            })
            .join();
            let failure = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(_) => Some("cleanup thread panicked".into()),
            };
            if let Some(error) = failure {
                if std::thread::panicking() {
                    eprintln!(
                        "SQL Server harness cleanup failed for {}: {error}",
                        self.database
                    );
                } else {
                    panic!(
                        "SQL Server harness cleanup failed for {}: {error}",
                        self.database
                    );
                }
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
) -> Result<TdsClient, Error> {
    let datasource = format!("tcp:{host},{port}");
    let mut context = ClientContext::with_data_source(&datasource);
    context.database = database.to_owned();
    context.application_name = "Database IDE test harness".to_owned();
    context.connect_timeout = CONNECT_TIMEOUT_SECS;
    context.login_timeout = Some(CONNECT_TIMEOUT_SECS);
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
    tokio::time::timeout(
        Duration::from_secs(u64::from(CONNECT_TIMEOUT_SECS)),
        TdsConnectionProvider::new().create_client(context, &datasource, None),
    )
    .await
    .map_err(|_| {
        Error::Wait(format!(
            "connecting to database {database} exceeded {CONNECT_TIMEOUT_SECS} seconds"
        ))
    })?
    .map_err(Error::from)
}

/// The DDL lock serializes harness creators; other clients may still lock model.
fn create_database(runtime: &Runtime, client: &mut TdsClient, database: &str) -> Result<(), Error> {
    run_retry(
        runtime,
        client,
        &format!("CREATE DATABASE [{database}];"),
        &[1807, 1802],
    )
}

/// Retries only known transient SQL failures and returns the final server diagnostics.
fn run_retry(
    runtime: &Runtime,
    client: &mut TdsClient,
    sql: &str,
    numbers: &[u32],
) -> Result<(), Error> {
    let deadline = Instant::now() + RETRY_TIMEOUT;
    let mut reported = false;
    loop {
        match runtime.block_on(wait::query(client, sql, sql)) {
            Ok(_) => return Ok(()),
            Err(Error::Tds(TdsError::SqlServerError { diagnostics }))
                if diagnostics
                    .errors
                    .iter()
                    .any(|error| error.number == numbers[0])
                    && diagnostics
                        .errors
                        .iter()
                        .all(|error| numbers.contains(&error.number)) =>
            {
                if Instant::now() >= deadline {
                    return Err(Error::Wait(format!(
                        "retrying {sql} exceeded {} seconds: {}; {}",
                        RETRY_TIMEOUT.as_secs(),
                        tds_message(&TdsError::SqlServerError { diagnostics }),
                        runtime.block_on(wait::diagnostics())
                    )));
                }
                if !reported {
                    eprintln!(
                        "SQL Server harness retrying {sql}: {} ({} seconds remain)",
                        diagnostics
                            .errors
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; "),
                        deadline.saturating_duration_since(Instant::now()).as_secs()
                    );
                    reported = true;
                }
                std::thread::sleep(
                    RETRY_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(error) => return Err(error),
        }
    }
}

fn tds_message(error: &TdsError) -> String {
    match error {
        TdsError::SqlServerError { diagnostics } => diagnostics
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "),
        error => error.to_string(),
    }
}

fn run_admin(runtime: &Runtime, client: &mut TdsClient, sql: &str) -> Result<(), Error> {
    runtime
        .block_on(wait::query(
            client,
            sql,
            sql.split(" WITH PASSWORD").next().unwrap_or(sql),
        ))
        .map(|_| ())
}

async fn query_rows(client: &mut TdsClient, sql: &str) -> Result<String, TdsError> {
    let result: Result<String, TdsError> = async {
        let mut output = String::new();
        let mut result = client.execute(sql.to_owned(), ()).await?;
        loop {
            match result {
                StatementResult::Rows => {
                    while let Some(row) = client.next_row().await? {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output
                            .push_str(&row.iter().map(format_value).collect::<Vec<_>>().join("\t"));
                    }
                }
                StatementResult::NoRows { .. } => {}
                StatementResult::End => break,
            }
            result = client.advance().await?;
        }
        Ok(output)
    }
    .await;
    let closed = client.close_query().await;
    let output = result?;
    closed?;
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

async fn acquire_lock(
    client: &mut TdsClient,
    resource: &str,
    timeout_ms: u32,
) -> Result<i32, Error> {
    let sql = format!(
        "USE [master]; DECLARE @lock_result int; EXEC @lock_result = sys.sp_getapplock @Resource = N'{resource}', @LockMode = 'Exclusive', @LockOwner = 'Session', @LockTimeout = {timeout_ms}; SELECT @lock_result;"
    );
    let result = wait::query_with_timeout(
        client,
        &sql,
        &format!("application lock {resource}"),
        Duration::from_millis(u64::from(timeout_ms)) + LOCK_RESPONSE_GRACE,
    )
    .await?;
    let code = result
        .parse::<i32>()
        .map_err(|_| Error::Lock(format!("{resource}: invalid result {result:?}")))?;
    if code < 0 && !(code == -1 && timeout_ms == 0) {
        return Err(Error::Lock(format!(
            "{resource}: sp_getapplock returned {code} after waiting up to {timeout_ms} ms; {}",
            wait::diagnostics().await
        )));
    }
    Ok(code)
}

async fn release_lock(client: &mut TdsClient, resource: &str) -> Result<(), Error> {
    wait::query(
        client,
        &format!("USE [master]; DECLARE @rc int; EXEC @rc = sys.sp_releaseapplock @Resource = N'{resource}', @LockOwner = 'Session'; IF @rc < 0 THROW 51000, 'Could not release test harness application lock', 1;"),
        &format!("releasing application lock {resource}"),
    )
    .await
    .map(|_| ())
}

/// The caller holds the DDL lock, so candidate names cannot be mid-create or mid-drop.
fn sweep(runtime: &Runtime, admin: &mut TdsClient) -> Result<(), Error> {
    let candidates = runtime.block_on(wait::query(admin,
        "SELECT name FROM sys.databases WHERE name LIKE N'dbide[_]test[_]%' UNION SELECT name FROM sys.server_principals WHERE type = 'S' AND name LIKE N'dbide[_]test[_]%';", "enumerating abandoned databases and logins"
    ))?;
    for name in candidates.lines().filter(|name| safe_name(name)) {
        if runtime.block_on(acquire_lock(admin, name, 0))? == -1 {
            continue;
        }
        let active = runtime.block_on(wait::query(admin, &format!(
            "IF EXISTS (SELECT 1 FROM sys.dm_exec_sessions s WHERE s.original_login_name <> N'{name}' AND s.session_id <> @@SPID AND (s.database_id = DB_ID(N'{name}') OR EXISTS (SELECT 1 FROM sys.dm_tran_locks l WHERE l.request_session_id = s.session_id AND l.resource_database_id = DB_ID(N'{name}')))) SELECT N'active';"
        ), &format!("checking sessions using {name}")))?;
        if active.is_empty() {
            match remove_objects(runtime, admin, name, name, false) {
                Err(Error::Tds(TdsError::SqlServerError { diagnostics }))
                    if !diagnostics.errors.is_empty()
                        && diagnostics.errors.iter().all(|error| error.number == 3702) => {}
                result => result?,
            }
        }
        runtime.block_on(release_lock(admin, name))?;
    }
    Ok(())
}

fn cleanup(
    runtime: &Runtime,
    admin: &mut TdsClient,
    database: &str,
    user: &str,
) -> Result<(), Error> {
    let result = (|| {
        runtime.block_on(acquire_lock(admin, DDL_LOCK, DDL_LOCK_TIMEOUT_MS))?;
        let removed = remove_objects(runtime, admin, database, user, true);
        let released = runtime.block_on(release_lock(admin, DDL_LOCK));
        removed.and(released)
    })();
    if let Err(error) = &result {
        eprintln!("Could not clean up SQL Server harness objects {database}: {error}");
    }
    result
}

/// Caller owns the name lock and DDL lock. Only the owning handle may force disconnects.
fn remove_objects(
    runtime: &Runtime,
    admin: &mut TdsClient,
    database: &str,
    user: &str,
    force_disconnect: bool,
) -> Result<(), Error> {
    let sessions = runtime.block_on(wait::query(admin, &format!(
        "SELECT session_id FROM sys.dm_exec_sessions WHERE original_login_name = N'{user}' AND session_id <> @@SPID"
    ), &format!("finding sessions for {user}")))?;
    for id in sessions.lines().filter_map(|id| id.parse::<u16>().ok()) {
        run_admin(
            runtime,
            admin,
            &format!(
                "BEGIN TRY IF EXISTS (SELECT 1 FROM sys.dm_exec_sessions WHERE session_id = {id} AND original_login_name = N'{user}') KILL {id}; END TRY BEGIN CATCH IF ERROR_NUMBER() <> 6106 THROW; END CATCH"
            ),
        )?;
    }
    let disconnect = if force_disconnect {
        format!("ALTER DATABASE [{database}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE;")
    } else {
        String::new()
    };
    let sql = format!(
        "USE [master]; IF DB_ID(N'{database}') IS NOT NULL BEGIN {disconnect} DROP DATABASE [{database}]; END;"
    );
    if force_disconnect {
        run_retry(runtime, admin, &sql, &[3702])?;
    } else {
        run_admin(runtime, admin, &sql)?;
    }
    run_retry(
        runtime,
        admin,
        &format!(
            "IF EXISTS (SELECT 1 FROM sys.server_principals WHERE name = N'{user}') DROP LOGIN [{user}];"
        ),
        &[15434],
    )
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
    fn server_failures_display_every_error_number_and_message() {
        use mssql_tds::error::{SqlErrorInfo, SqlServerDiagnostics};
        let error = Error::from(TdsError::SqlServerError {
            diagnostics: SqlServerDiagnostics::new(
                vec![
                    SqlErrorInfo {
                        number: 1807,
                        message: "Could not obtain exclusive lock on model".into(),
                        state: 1,
                        class: 16,
                        server_name: None,
                        proc_name: None,
                        line_number: Some(1),
                    },
                    SqlErrorInfo {
                        number: 1802,
                        message: "CREATE DATABASE failed".into(),
                        state: 1,
                        class: 16,
                        server_name: None,
                        proc_name: None,
                        line_number: Some(1),
                    },
                ],
                vec![],
            ),
        });
        let message = error.to_string();
        assert!(message.contains("1807"));
        assert!(message.contains("Could not obtain exclusive lock on model"));
        assert!(message.contains("1802"));
        assert!(message.contains("CREATE DATABASE failed"));
        assert!(format!("{error:?}").contains("1807"));
    }

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
