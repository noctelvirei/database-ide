use dbide_engine::connections::{Connection, SignInMethod};
use dbide_engine::danger::Engine;
use dbide_engine::mssql;
use dbide_engine::session::{Event, Session, Value};
use dbide_test_servers::mssql::Mssql as Database;
use futures::StreamExt;

async fn open() -> (Database, Box<dyn Session>) {
    let database = tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = Connection::new(Engine::SqlServer, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.trust_server_certificate = true;
    details.sign_in = SignInMethod::Windows;
    let mut session = mssql::connect(&details, Some(database.password()))
        .await
        .unwrap();
    rows(&mut session, "USE master").await;
    lock(&mut session, "dbide_switch_tests").await;
    session
        .switch_database(database.database(), None)
        .await
        .unwrap();
    (database, Box::new(session))
}

async fn lock(session: &mut dyn Session, resource: &str) {
    rows(
        session,
        &format!(
            "DECLARE @result int; EXEC @result = sys.sp_getapplock @Resource = N'{resource}', \
         @LockMode = 'Exclusive', @LockOwner = 'Session', @LockTimeout = 120000; \
         IF @result < 0 THROW 50000, 'Could not lock switch test databases', 1;"
        ),
    )
    .await;
}

async fn create_database(session: &mut dyn Session, name: &str) {
    rows(session, "USE master").await;
    lock(session, "dbide_test_harness_ddl").await;
    let name = name.replace(']', "]]");
    rows(session, &format!("DROP DATABASE IF EXISTS [{name}]")).await;
    rows(session, &format!("CREATE DATABASE [{name}]")).await;
    rows(
        session,
        "EXEC sys.sp_releaseapplock @Resource = N'dbide_test_harness_ddl', @LockOwner = 'Session'",
    )
    .await;
}

async fn drop_database(session: &mut dyn Session, name: &str) {
    rows(session, "USE master").await;
    lock(session, "dbide_test_harness_ddl").await;
    rows(
        session,
        &format!("DROP DATABASE [{}]", name.replace(']', "]]")),
    )
    .await;
    rows(
        session,
        "EXEC sys.sp_releaseapplock @Resource = N'dbide_test_harness_ddl', @LockOwner = 'Session'",
    )
    .await;
}

async fn rows(session: &mut dyn Session, sql: &str) -> Vec<Vec<Value>> {
    let mut run = session.run(sql);
    let mut rows = Vec::new();
    while let Some(event) = run.next().await {
        match event {
            Event::Rows(batch) => rows.extend(batch),
            Event::Error { text, .. } => panic!("{text}"),
            _ => {}
        }
    }
    rows
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_switch_accepts_spaces_and_escaped_identifier_quotes() {
    let (database, mut session) = open().await;
    create_database(&mut *session, "dbide_switch quoted ] db").await;
    session
        .switch_database("dbide_switch quoted ] db", None)
        .await
        .unwrap();
    assert_eq!(session.current_database(), "dbide_switch quoted ] db");
    session
        .switch_database(database.database(), Some(database.password()))
        .await
        .unwrap();
    drop_database(&mut *session, "dbide_switch quoted ] db").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_switch_updates_current_database_and_resolves_its_objects() {
    let (database, mut session) = open().await;
    create_database(&mut *session, "dbide_switch_objects").await;
    session
        .switch_database("dbide_switch_objects", None)
        .await
        .unwrap();
    rows(
        &mut *session,
        "CREATE TABLE switch_marker (id int); INSERT INTO switch_marker VALUES (37)",
    )
    .await;
    session
        .switch_database(database.database(), Some(database.password()))
        .await
        .unwrap();
    rows(
        &mut *session,
        "CREATE TABLE old_marker (id int); INSERT INTO old_marker VALUES (11)",
    )
    .await;
    session
        .switch_database("dbide_switch_objects", None)
        .await
        .unwrap();
    assert_eq!(session.current_database(), "dbide_switch_objects");
    assert_eq!(
        rows(&mut *session, "SELECT id FROM switch_marker").await,
        vec![vec![Value::Integer(37)]]
    );
    session
        .switch_database(database.database(), Some(database.password()))
        .await
        .unwrap();
    drop_database(&mut *session, "dbide_switch_objects").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_missing_database_names_the_failure_and_preserves_the_old_session() {
    let (database, mut session) = open().await;
    rows(
        &mut *session,
        "CREATE TABLE old_marker (id int); INSERT INTO old_marker VALUES (19)",
    )
    .await;
    let error = session
        .switch_database("dbide_test_missing_switch", Some(database.password()))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("dbide_test_missing_switch"),
        "{error}"
    );
    assert_eq!(session.current_database(), database.database());
    assert_eq!(
        rows(&mut *session, "SELECT id FROM old_marker").await,
        vec![vec![Value::Integer(19)]]
    );
}
