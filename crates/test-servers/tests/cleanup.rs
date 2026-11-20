use dbide_test_servers::postgres::Postgres;

#[test]
fn pg_server_folder_is_deleted_after_drop() {
    let database = Postgres::start().unwrap();
    let folder = database.server_folder().to_owned();
    let second = Postgres::start().unwrap();
    drop(database);
    assert!(folder.exists());
    assert_eq!(second.query("SELECT 1").unwrap(), "1\n");
    drop(second);
    assert!(!folder.exists());

    let later = Postgres::start().unwrap();
    let later_folder = later.server_folder().to_owned();
    assert_ne!(folder, later_folder);
    let panic = std::panic::catch_unwind(move || {
        let _database = later;
        panic!("Exercise cleanup during test unwinding");
    });
    assert!(panic.is_err());
    assert!(!later_folder.exists());
}
