use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dbide_test_servers::mssql::{CONNECT_TIMEOUT_SECS, Mssql, wait::QUERY_TIMEOUT};
use mssql_tds::connection::client_context::{ClientContext, TdsAuthenticationMethod};
use mssql_tds::connection::tds_client::{ResultSet, StatementResult, TdsClient};
use mssql_tds::connection_provider::tds_connection_provider::TdsConnectionProvider;
use mssql_tds::core::EncryptionSetting;

// The stress case creates 40 databases; allow five minutes for its full lifecycle under load.
const TEST_TIMEOUT: Duration = Duration::from_secs(300);
// A ready child may first need the harness's full DDL budget to create its database.
const CHILD_READY_TIMEOUT: Duration = Duration::from_secs(120);
// Check file handshakes promptly without spinning while another process performs DDL.
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(20);

fn assert_removed(database: &Mssql, name: &str) {
    admin_query(
        database,
        format!(
            "IF DB_ID(N'{name}') IS NOT NULL OR EXISTS (SELECT 1 FROM sys.server_principals WHERE name = N'{name}') THROW 51000, 'Harness objects survived cleanup', 1;"
        ),
    );
}

struct Deadline(std::sync::mpsc::Sender<()>);

impl Deadline {
    fn new(test: &'static str) -> Self {
        let (done, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if matches!(
                receiver.recv_timeout(TEST_TIMEOUT),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                let message = format!(
                    "{test}: exceeded {} seconds waiting for test/child completion; {}",
                    TEST_TIMEOUT.as_secs(),
                    runtime.block_on(dbide_test_servers::mssql::wait::diagnostics())
                );
                // Bypass libtest capture because exit cannot flush its per-test buffer.
                let _ = writeln!(std::io::stderr(), "{message}");
                // A panic would leave blocked test threads and child waits running.
                std::process::exit(1);
            }
        });
        Self(done)
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn admin_connection(database: &Mssql) -> (tokio::runtime::Runtime, TdsClient) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = runtime.block_on(async {
        let address = format!("tcp:{},{}", database.host(), database.port());
        let mut context = ClientContext::with_data_source(&address);
        context.database = "master".into();
        context.application_name = "Database IDE test harness".into();
        context.connect_timeout = CONNECT_TIMEOUT_SECS;
        context.login_timeout = Some(CONNECT_TIMEOUT_SECS);
        context.tds_authentication_method = TdsAuthenticationMethod::SSPI;
        context.encryption_options.mode = EncryptionSetting::On;
        context.encryption_options.trust_server_certificate = true;
        TdsConnectionProvider::new()
            .create_client(context, &address, None)
            .await
            .unwrap()
    });
    (runtime, client)
}

fn admin_query(database: &Mssql, sql: String) {
    let (runtime, mut admin) = admin_connection(database);
    execute_admin(&runtime, &mut admin, sql);
    runtime.block_on(admin.close_connection()).unwrap();
}

fn execute_admin(runtime: &tokio::runtime::Runtime, admin: &mut TdsClient, sql: String) {
    let operation = sql.clone();
    runtime
        .block_on(async {
            tokio::time::timeout(QUERY_TIMEOUT, async {
                let mut result = admin.execute(sql, ()).await.unwrap();
                while !matches!(result, StatementResult::End) {
                    result = admin.advance().await.unwrap();
                }
                admin.close_query().await.unwrap();
            })
            .await
        })
        .unwrap_or_else(|_| panic!("Timed out waiting for admin query: {operation}"));
}

#[test]
fn mssql_blocked_query_names_the_database_and_blocking_session() {
    let _deadline = Deadline::new("mssql_blocked_query_names_the_database_and_blocking_session");
    let database = Mssql::start().unwrap();
    database.query("CREATE TABLE marker (value int);").unwrap();
    let (runtime, mut blocker) = admin_connection(&database);
    let session = runtime.block_on(async {
        blocker.execute("SELECT @@SPID".into(), ()).await.unwrap();
        let row = blocker.next_row().await.unwrap().unwrap();
        let mssql_tds::datatypes::column_values::ColumnValues::SmallInt(session) = row[0] else {
            panic!("@@SPID must be a smallint");
        };
        blocker.close_query().await.unwrap();
        session
    });
    execute_admin(
        &runtime,
        &mut blocker,
        format!(
            "BEGIN TRAN; SELECT * FROM [{}].dbo.marker WITH (TABLOCKX, HOLDLOCK);",
            database.database()
        ),
    );
    let error = database
        .query("SET LOCK_TIMEOUT 1000; INSERT INTO marker VALUES (1)")
        .unwrap_err();
    execute_admin(&runtime, &mut blocker, "ROLLBACK;".into());
    runtime.block_on(blocker.close_connection()).unwrap();
    let message = error.to_string();
    assert!(message.contains(database.database()), "{message}");
    assert!(message.contains("1222"), "{message}");
    assert!(
        message.contains(&format!(
            "lock session={session} type=DATABASE database={}",
            database.database()
        )),
        "{message}"
    );
    assert_eq!(database.query("SELECT 42").unwrap(), "42");
}

#[test]
fn mssql_server_errors_are_diagnosable_and_leave_the_connection_usable() {
    let _deadline =
        Deadline::new("mssql_server_errors_are_diagnosable_and_leave_the_connection_usable");
    let database = Mssql::start().unwrap();
    let error = database
        .query("THROW 51042, 'Harness diagnostic test', 1;")
        .unwrap_err();
    assert!(error.to_string().contains("51042"));
    assert!(error.to_string().contains("Harness diagnostic test"));
    assert!(format!("{error:?}").contains("51042"));
    assert_eq!(database.query("SELECT 42").unwrap(), "42");
}

#[test]
fn mssql_parallel_handles_remain_usable_after_another_handle_panics() {
    let _deadline =
        Deadline::new("mssql_parallel_handles_remain_usable_after_another_handle_panics");
    let survivor = Mssql::start().unwrap();
    let threads: Vec<_> = (0..4).map(|_| std::thread::spawn(Mssql::start)).collect();
    let databases: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect();
    for database in &databases {
        assert_eq!(database.query("SELECT 42").unwrap(), "42");
    }
    let failed = Mssql::start().unwrap();
    let name = failed.database().to_owned();
    let panic = std::thread::spawn(move || {
        let _database = failed;
        panic!("Exercise SQL Server cleanup during unwinding");
    })
    .join();
    assert!(panic.is_err());
    assert_removed(&survivor, &name);
    let later = Mssql::start().unwrap();
    assert_eq!(later.query("SELECT 43").unwrap(), "43");
    assert_eq!(survivor.query("SELECT 44").unwrap(), "44");
}

#[test]
fn mssql_new_run_sweeps_crashed_run_without_waiting_for_global_lock() {
    let _deadline =
        Deadline::new("mssql_new_run_sweeps_crashed_run_without_waiting_for_global_lock");
    const CHILD_MODE: &str = "DBIDE_MSSQL_CLEANUP_CHILD";
    const CHILD_FILE: &str = "DBIDE_MSSQL_CLEANUP_FILE";
    if let Ok(mode) = std::env::var(CHILD_MODE) {
        let file = std::path::PathBuf::from(std::env::var_os(CHILD_FILE).unwrap());
        let database = Mssql::start().unwrap();
        if mode == "crash" {
            database
                .query("CREATE TABLE marker (value int); INSERT INTO marker VALUES (1);")
                .unwrap();
            std::fs::write(file.with_extension("tmp"), database.database()).unwrap();
            std::fs::rename(file.with_extension("tmp"), file).unwrap();
            let mut release = String::new();
            std::io::stdin().read_line(&mut release).unwrap();
            assert_eq!(release, "go\n");
            // Skip Drop, as when a test process dies with its sessions still open.
            std::process::exit(0);
        }
        let name = std::fs::read_to_string(file).unwrap();
        if mode == "blocked" {
            admin_query(
                &database,
                format!(
                    "IF DB_ID(N'{name}') IS NULL THROW 51000, 'Database with external locks was swept', 1;"
                ),
            );
            return;
        }
        assert_removed(&database, &name);
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("database.txt");
    let survivor = Mssql::start().unwrap();
    let mut crashed = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mssql_new_run_sweeps_crashed_run_without_waiting_for_global_lock",
        ])
        .env(CHILD_MODE, "crash")
        .env(CHILD_FILE, &file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Acquire the external lock before the owner exits, so another concurrent sweep
    // cannot delete the fixture between its creation and the regression check.
    while !file.exists() {
        assert!(
            crashed.try_wait().unwrap().is_none(),
            "Crash child exited before creating its database"
        );
        std::thread::sleep(CHILD_POLL_INTERVAL);
    }
    let name = std::fs::read_to_string(&file).unwrap();
    let (runtime, mut blocker) = admin_connection(&survivor);
    execute_admin(
        &runtime,
        &mut blocker,
        format!("BEGIN TRAN; SELECT * FROM [{name}].dbo.marker WITH (TABLOCKX, HOLDLOCK);"),
    );
    crashed.stdin.take().unwrap().write_all(b"go\n").unwrap();
    let crashed = crashed.wait_with_output().unwrap();
    assert!(
        crashed.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&crashed.stdout),
        String::from_utf8_lossy(&crashed.stderr)
    );
    // A session left by the old harness must not serialize new test databases.
    survivor.query("USE master; DECLARE @rc int; EXEC @rc = sys.sp_getapplock @Resource = N'dbide_test_harness', @LockMode = 'Exclusive', @LockOwner = 'Session', @LockTimeout = 0; IF @rc < 0 THROW 51000, 'Could not set up old harness lock', 1;").unwrap();
    let blocked = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mssql_new_run_sweeps_crashed_run_without_waiting_for_global_lock",
        ])
        .env(CHILD_MODE, "blocked")
        .env(CHILD_FILE, &file)
        .output()
        .unwrap();
    execute_admin(&runtime, &mut blocker, "ROLLBACK;".into());
    runtime.block_on(blocker.close_connection()).unwrap();
    assert!(
        blocked.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&blocked.stdout),
        String::from_utf8_lossy(&blocked.stderr)
    );
    let verified = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mssql_new_run_sweeps_crashed_run_without_waiting_for_global_lock",
        ])
        .env(CHILD_MODE, "verify")
        .env(CHILD_FILE, &file)
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    assert_eq!(
        survivor
            .query(&format!("USE [{}]; SELECT 45", survivor.database()))
            .unwrap(),
        "45"
    );
}

#[test]
fn mssql_processes_create_and_drop_databases_concurrently() {
    let _deadline = Deadline::new("mssql_processes_create_and_drop_databases_concurrently");
    const CHILD: &str = "DBIDE_MSSQL_CONCURRENT_CHILD";
    const FOLDER: &str = "DBIDE_MSSQL_CONCURRENT_FOLDER";
    if let Ok(id) = std::env::var(CHILD) {
        let folder = std::path::PathBuf::from(std::env::var_os(FOLDER).unwrap());
        std::fs::write(folder.join(id), "ready").unwrap();
        let mut release = String::new();
        std::io::stdin().read_line(&mut release).unwrap();
        assert_eq!(release, "go\n");
        let observer = Mssql::start().unwrap();
        let threads: Vec<_> = (0..3)
            .map(|_| {
                std::thread::spawn(|| {
                    let mut names = Vec::new();
                    for _ in 0..3 {
                        let database = Mssql::start().unwrap();
                        assert_eq!(database.query("SELECT 42").unwrap(), "42");
                        let name = database.database().to_owned();
                        drop(database);
                        names.push(name);
                    }
                    names
                })
            })
            .collect();
        for thread in threads {
            for name in thread.join().unwrap() {
                assert_removed(&observer, &name);
            }
        }
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let mut children: Vec<_> = (0..4)
        .map(|id| {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "mssql_processes_create_and_drop_databases_concurrently",
                ])
                .env(CHILD, id.to_string())
                .env(FOLDER, folder.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let deadline = Instant::now() + CHILD_READY_TIMEOUT;
    while (0..4).any(|id| !folder.path().join(id.to_string()).exists()) {
        assert!(
            Instant::now() < deadline,
            "Concurrent children did not become ready"
        );
        std::thread::sleep(CHILD_POLL_INTERVAL);
    }
    for child in &mut children {
        child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn mssql_sweep_preserves_another_live_process_database() {
    let _deadline = Deadline::new("mssql_sweep_preserves_another_live_process_database");
    const CHILD: &str = "DBIDE_MSSQL_LIVE_CHILD";
    const FILE: &str = "DBIDE_MSSQL_LIVE_FILE";
    if let Ok(mode) = std::env::var(CHILD) {
        let file = std::path::PathBuf::from(std::env::var_os(FILE).unwrap());
        let database = Mssql::start().unwrap();
        if mode == "hold" {
            database
                .query(
                    "CREATE TABLE marker (value int); INSERT INTO marker VALUES (47); USE master;",
                )
                .unwrap();
            std::fs::write(file.with_extension("tmp"), database.database()).unwrap();
            std::fs::rename(file.with_extension("tmp"), &file).unwrap();
            let mut release = String::new();
            std::io::stdin().read_line(&mut release).unwrap();
            assert_eq!(release, "go\n");
            assert_eq!(
                database
                    .query(&format!(
                        "SELECT value FROM [{}].dbo.marker",
                        database.database()
                    ))
                    .unwrap(),
                "47"
            );
        } else {
            let name = std::fs::read_to_string(file).unwrap();
            admin_query(
                &database,
                format!(
                    "IF DB_ID(N'{name}') IS NULL THROW 51000, 'Live database was swept', 1; IF NOT EXISTS (SELECT 1 FROM [{name}].dbo.marker WHERE value = 47) THROW 51000, 'Live contents were swept', 1;"
                ),
            );
        }
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("live.txt");
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mssql_sweep_preserves_another_live_process_database",
        ])
        .env(CHILD, "hold")
        .env(FILE, &file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + CHILD_READY_TIMEOUT;
    while !file.exists() {
        assert!(
            Instant::now() < deadline,
            "Live child did not create its database"
        );
        if holder.try_wait().unwrap().is_some() {
            let output = holder.wait_with_output().unwrap();
            panic!(
                "Live child exited: stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(CHILD_POLL_INTERVAL);
    }
    let swept = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mssql_sweep_preserves_another_live_process_database",
        ])
        .env(CHILD, "sweep")
        .env(FILE, &file)
        .output()
        .unwrap();
    holder.stdin.take().unwrap().write_all(b"go\n").unwrap();
    let held = holder.wait_with_output().unwrap();
    assert!(
        swept.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&swept.stdout),
        String::from_utf8_lossy(&swept.stderr)
    );
    assert!(
        held.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&held.stdout),
        String::from_utf8_lossy(&held.stderr)
    );
}
