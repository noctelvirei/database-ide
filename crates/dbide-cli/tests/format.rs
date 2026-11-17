use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn format_reads_stdin_and_writes_sql_to_stdout() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args(["format", "--engine", "mysql"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"select `a b`,name from t")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"SELECT `a b`, name FROM t");
    assert!(output.stderr.is_empty());
}

#[test]
fn format_reads_a_file_and_leaves_an_output_artifact() {
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let artifacts = root.join("test-artifacts").join("format");
    std::fs::create_dir_all(&artifacts).unwrap();
    let input = artifacts.join("input.sql");
    std::fs::write(&input, "select [a b],name from t").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args(["format", "--engine", "sql-server", "--file"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"SELECT [a b], name FROM t");
    assert!(output.stderr.is_empty());
    std::fs::write(artifacts.join("formatted.sql"), output.stdout).unwrap();
    assert_eq!(
        std::fs::read_to_string(input).unwrap(),
        "select [a b],name from t"
    );
}

#[test]
fn format_refuses_unclosed_strings_without_partial_stdout() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .args(["format", "--engine", "postgresql"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"select 'unclosed")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unterminated string literal"));
}
