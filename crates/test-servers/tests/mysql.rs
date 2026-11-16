use dbide_test_servers::mysql::MySql;

#[test]
fn mysql_handles_share_a_server_and_isolate_databases() {
    let first = MySql::start().unwrap();
    let second = MySql::start().unwrap();
    assert_eq!(first.host(), "127.0.0.1");
    assert_eq!(first.port(), second.port());
    assert_eq!(first.server_folder(), second.server_folder());
    assert_ne!(first.database(), second.database());
    assert!(first.database().starts_with("dbide_test_"));
    assert_eq!(first.user(), "dbide_test");
    assert!(!first.password().is_empty());
    assert!(!format!("{first:?}").contains(first.password()));
    first.query("CREATE TABLE isolated (id int)").unwrap();
    assert!(second.query("SELECT * FROM isolated").is_err());
    assert_eq!(second.query("SELECT 2").unwrap(), "2\n");
    let name = first.database().to_owned();
    drop(first);
    assert_eq!(
        second
            .query(&format!(
                "SELECT count(*) FROM information_schema.schemata WHERE schema_name = '{name}'"
            ))
            .unwrap(),
        "0\n"
    );
}

#[test]
fn mysql_script_errors_stop_before_later_statements() {
    let database = MySql::start().unwrap();
    let script = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        script.path(),
        "CREATE TABLE invented (label varchar(40)); INSERT INTO invented VALUES ('雪é');",
    )
    .unwrap();
    database.load_script(script.path()).unwrap();
    assert_eq!(
        database.query("SELECT label FROM invented").unwrap(),
        "雪é\n"
    );
    std::fs::write(
        script.path(),
        "SELECT missing_column; CREATE TABLE must_not_exist (id int);",
    )
    .unwrap();
    assert!(database.load_script(script.path()).is_err());
    assert_eq!(
        database
            .query("SELECT count(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'must_not_exist'")
            .unwrap(),
        "0\n"
    );
}
