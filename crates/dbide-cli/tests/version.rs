use std::process::Command;

#[test]
fn version_prints_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_dbide"))
        .arg("version")
        .output()
        .expect("dbide should start");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"0.1.0\n");
    assert!(output.stderr.is_empty());
}
