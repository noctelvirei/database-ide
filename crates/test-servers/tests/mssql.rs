use std::process::Command;

use dbide_test_servers::mssql::Mssql;
use mssql_tds::connection::client_context::{ClientContext, TdsAuthenticationMethod};
use mssql_tds::connection::tds_client::StatementResult;
use mssql_tds::connection_provider::tds_connection_provider::TdsConnectionProvider;
use mssql_tds::core::EncryptionSetting;

fn assert_removed(database: &Mssql, name: &str) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let address = format!("tcp:{},{}", database.host(), database.port());
        let mut context = ClientContext::with_data_source(&address);
        context.database = "master".into();
        context.tds_authentication_method = TdsAuthenticationMethod::SSPI;
        context.encryption_options.mode = EncryptionSetting::On;
        context.encryption_options.trust_server_certificate = true;
        let mut admin = TdsConnectionProvider::new()
            .create_client(context, &address, None).await.unwrap();
        let sql = format!(
            "IF DB_ID(N'{name}') IS NOT NULL OR EXISTS (SELECT 1 FROM sys.server_principals WHERE name = N'{name}') THROW 51000, 'Harness objects survived cleanup', 1;"
        );
        let mut result = admin.execute(sql, ()).await.unwrap();
        while !matches!(result, StatementResult::End) {
            result = admin.advance().await.unwrap();
        }
        admin.close_query().await.unwrap();
        admin.close_connection().await.unwrap();
    });
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
    assert!(crashed.status.success(), "{crashed:?}");
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
    assert!(verified.status.success(), "{verified:?}");
    assert_eq!(
        survivor
            .query(&format!("USE [{}]; SELECT 45", survivor.database()))
            .unwrap(),
        "45"
    );
}
