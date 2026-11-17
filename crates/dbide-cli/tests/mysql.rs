use std::process::Command;

use dbide_test_servers::mysql::MySql;

fn run_csv(database: &MySql, connection: &str) {
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let artifacts = root.join("test-artifacts").join(database.database());
    std::fs::create_dir_all(&artifacts).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args([
            "run",
            "--connection",
            connection,
            "--batch",
            "SELECT 1 AS id, 'elm' AS name UNION ALL SELECT 2, 'oak'",
            "--output",
        ])
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
fn mysql_uri_run_command_writes_csv_header_rows_and_count() {
    let database = MySql::start().unwrap();
    run_csv(
        &database,
        &format!(
            "mysql://{}@{}:{}/{}?sslmode=disabled",
            database.user(),
            database.host(),
            database.port(),
            database.database()
        ),
    );
}

#[test]
fn mysql_jdbc_run_command_writes_csv_header_rows_and_count() {
    let database = MySql::start().unwrap();
    run_csv(
        &database,
        &format!(
            "jdbc:mysql://{}:{}/{}?user={}&sslMode=DISABLED",
            database.host(),
            database.port(),
            database.database(),
            database.user()
        ),
    );
}

#[test]
fn mysql_ado_run_command_writes_csv_header_rows_and_count() {
    let database = MySql::start().unwrap();
    run_csv(
        &database,
        &format!(
            "Provider=MySQL;Server={};Port={};Database={};User ID={};SslMode=Disabled",
            database.host(),
            database.port(),
            database.database(),
            database.user()
        ),
    );
}
