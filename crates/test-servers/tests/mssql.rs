use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dbide_test_servers::mssql::Mssql;
use mssql_tds::connection::client_context::{ClientContext, TdsAuthenticationMethod};
use mssql_tds::connection::tds_client::StatementResult;
use mssql_tds::connection_provider::tds_connection_provider::TdsConnectionProvider;
use mssql_tds::core::EncryptionSetting;

fn assert_removed(database: &Mssql, name: &str) {
    admin_query(
        database,
        format!(
            "IF DB_ID(N'{name}') IS NOT NULL OR EXISTS (SELECT 1 FROM sys.server_principals WHERE name = N'{name}') THROW 51000, 'Harness objects survived cleanup', 1;"
        ),
    );
}

fn admin_query(database: &Mssql, sql: String) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let address = format!("tcp:{},{}", database.host(), database.port());
        let mut context = ClientContext::with_data_source(&address);
        context.database = "master".into();
        context.tds_authentication_method = TdsAuthenticationMethod::SSPI;
        context.encryption_options.mode = EncryptionSetting::On;
        context.encryption_options.trust_server_certificate = true;
        let mut admin = TdsConnectionProvider::new()
            .create_client(context, &address, None)
            .await
            .unwrap();
        let mut result = admin.execute(sql, ()).await.unwrap();
        while !matches!(result, StatementResult::End) {
            result = admin.advance().await.unwrap();
        }
        admin.close_query().await.unwrap();
        admin.close_connection().await.unwrap();
    });
}

#[test]
fn mssql_server_errors_are_diagnosable_and_leave_the_connection_usable() {
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
    const CHILD_MODE: &str = "DBIDE_MSSQL_CLEANUP_CHILD";
    const CHILD_FILE: &str = "DBIDE_MSSQL_CLEANUP_FILE";
    if let Ok(mode) = std::env::var(CHILD_MODE) {
        let file = std::env::var_os(CHILD_FILE).unwrap();
        let database = Mssql::start().unwrap();
        if mode == "crash" {
            std::fs::write(file, database.database()).unwrap();
            // Skip Drop, as when a test process dies with its sessions still open.
            std::process::exit(0);
        }
        let name = std::fs::read_to_string(file).unwrap();
        assert_removed(&database, &name);
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("database.txt");
    let survivor = Mssql::start().unwrap();
    let crashed = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mssql_new_run_sweeps_crashed_run_without_waiting_for_global_lock",
        ])
        .env(CHILD_MODE, "crash")
        .env(CHILD_FILE, &file)
        .output()
        .unwrap();
    assert!(
        crashed.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&crashed.stdout),
        String::from_utf8_lossy(&crashed.stderr)
    );
    // A session left by the old harness must not serialize new test databases.
    survivor.query("USE master; DECLARE @rc int; EXEC @rc = sys.sp_getapplock @Resource = N'dbide_test_harness', @LockMode = 'Exclusive', @LockOwner = 'Session', @LockTimeout = 0; IF @rc < 0 THROW 51000, 'Could not set up old harness lock', 1;").unwrap();
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
    let deadline = Instant::now() + Duration::from_secs(120);
    while (0..4).any(|id| !folder.path().join(id.to_string()).exists()) {
        assert!(
            Instant::now() < deadline,
            "Concurrent children did not become ready"
        );
        std::thread::sleep(Duration::from_millis(20));
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
    let deadline = Instant::now() + Duration::from_secs(120);
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
        std::thread::sleep(Duration::from_millis(20));
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
