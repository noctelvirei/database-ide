use std::process::Command;

use dbide_test_servers::mssql::Mssql;

fn run(connection: &str, database: &Mssql, name: &str) {
    let artifacts = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "../../target".into())
        .join("test-artifacts")
        .join(database.database())
        .join(name);
    std::fs::create_dir_all(&artifacts).unwrap();
    let script = artifacts.join("query.sql");
    std::fs::write(
        &script,
        "\u{feff}SELECT 1 AS id, 'elm' AS name UNION ALL SELECT 2, 'oak'",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args(["run", "--connection", connection, "--file"])
        .arg(&script)
        .arg("--output")
        .arg(artifacts.join("rows.csv"))
        .env("DBIDE_PASSWORD", database.password())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(artifacts.join("rows-1.csv")).unwrap(),
        "\u{feff}id,name\r\n1,elm\r\n2,oak\r\n"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("2 rows"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(database.password()));
}

#[test]
fn mssql_run_accepts_adonet_and_writes_csv() {
    let database = Mssql::start().unwrap();
    run(
        &format!(
            "Server={},{};Database={};User ID={};Encrypt=true;TrustServerCertificate=true",
            database.host(),
            database.port(),
            database.database(),
            database.user()
        ),
        &database,
        "adonet",
    );
}

#[test]
fn mssql_run_accepts_jdbc_and_writes_csv() {
    let database = Mssql::start().unwrap();
    run(
        &format!(
            "jdbc:sqlserver://{}:{};databaseName={};user={};encrypt=true;trustServerCertificate=true",
            database.host(),
            database.port(),
            database.database(),
            database.user()
        ),
        &database,
        "jdbc",
    );
}
