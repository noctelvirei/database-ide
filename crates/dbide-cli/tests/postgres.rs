use std::process::Command;

use dbide_test_servers::postgres::Postgres;

#[test]
fn pg_run_command_writes_csv_header_rows_and_reports_count() {
    let database = Postgres::start().unwrap();
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let artifacts = root.join("test-artifacts").join(database.database());
    std::fs::create_dir_all(&artifacts).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args([
            "run",
            "--connection",
            &format!(
                "host={} port={} user={} dbname={} sslmode=disable",
                database.host(),
                database.port(),
                database.user(),
                database.database()
            ),
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
fn pg_run_command_reads_a_file_and_an_embedded_uri_password() {
    let database = Postgres::start().unwrap();
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let artifacts = root.join("test-artifacts").join(database.database());
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(artifacts.join("batch.sql"), "SELECT 42 AS answer").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args([
            "run",
            "--connection",
            &format!(
                "postgresql://{}:{}@{}:{}/{}?sslmode=disable",
                database.user(),
                database.password(),
                database.host(),
                database.port(),
                database.database()
            ),
            "--file",
        ])
        .arg(artifacts.join("batch.sql"))
        .arg("--output")
        .arg(artifacts.join("rows.csv"))
        .env_remove("DBIDE_PASSWORD")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(artifacts.join("rows-1.csv")).unwrap(),
        "\u{feff}answer\r\n42\r\n"
    );
}
