use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::postgres;
use dbide_engine::session::{Event, Session, Value};
use dbide_test_servers::postgres::Postgres as Database;
use futures::StreamExt;

async fn open() -> (Database, Box<dyn Session>) {
    let database = tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = Connection::new(Engine::PostgreSql, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.tls = TlsMode::Off;
    let session = postgres::connect(&details, Some(database.password()))
        .await
        .unwrap();
    (database, Box::new(session))
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
async fn pg_switch_accepts_spaces_and_escaped_identifier_quotes() {
    let (database, mut session) = open().await;
    rows(
        &mut *session,
        "CREATE DATABASE \"dbide_test_switch quoted \"\" db\"",
    )
    .await;
    session
        .switch_database("dbide_test_switch quoted \" db", Some(database.password()))
        .await
        .unwrap();
    assert_eq!(session.current_database(), "dbide_test_switch quoted \" db");
    session
        .switch_database(database.database(), Some(database.password()))
        .await
        .unwrap();
    rows(
        &mut *session,
        "DROP DATABASE \"dbide_test_switch quoted \"\" db\" WITH (FORCE)",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_switch_updates_current_database_and_resolves_its_objects() {
    let (database, mut session) = open().await;
    rows(&mut *session, "CREATE DATABASE dbide_test_switch_objects").await;
    session
        .switch_database("dbide_test_switch_objects", Some(database.password()))
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
        .switch_database("dbide_test_switch_objects", Some(database.password()))
        .await
        .unwrap();
    assert_eq!(session.current_database(), "dbide_test_switch_objects");
    assert_eq!(
        rows(&mut *session, "SELECT id FROM switch_marker").await,
        vec![vec![Value::Integer(37)]]
    );
    session
        .switch_database(database.database(), Some(database.password()))
        .await
        .unwrap();
    rows(
        &mut *session,
        "DROP DATABASE dbide_test_switch_objects WITH (FORCE)",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_switch_without_a_required_password_preserves_the_old_session() {
    let (_database, mut session) = open().await;
    rows(
        &mut *session,
        "CREATE TEMP TABLE old_marker (id int); INSERT INTO old_marker VALUES (19)",
    )
    .await;
    let error = session.switch_database("postgres", None).await.unwrap_err();
    assert!(error.to_string().contains("postgres"), "{error}");
    assert_eq!(
        rows(&mut *session, "SELECT id FROM old_marker").await,
        vec![vec![Value::Integer(19)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_missing_database_names_the_failure_and_preserves_the_old_session() {
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
