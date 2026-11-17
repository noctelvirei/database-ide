use std::time::Duration;

use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::postgres;
use dbide_engine::session::{Event, Session, Value};
use dbide_test_servers::postgres::Postgres as Database;
use futures::StreamExt;

fn details(database: &Database) -> Connection {
    let mut connection = Connection::new(Engine::PostgreSql, database.host());
    connection.port = Some(database.port());
    connection.user = Some(database.user().into());
    connection.database = Some(database.database().into());
    connection.tls = TlsMode::Off;
    connection
}

async fn database() -> Database {
    tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap()
}

fn artifact(database: &Database, name: &str, contents: &str) {
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let folder = root.join("test-artifacts").join(database.database());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), contents).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_query_timeout_reports_a_readable_error() {
    let database = database().await;
    let mut connection = details(&database);
    connection.query_timeout = 1;
    let mut session = postgres::connect(&connection, Some(database.password()))
        .await
        .unwrap();
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        session.run("SELECT pg_sleep(30); SELECT 99").collect(),
    )
    .await
    .expect("the one-second query timeout must end the run");
    artifact(&database, "query-timeout.txt", &format!("{events:#?}"));
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Error { text, .. } if text == "canceling statement due to statement timeout")
        ),
        "{events:?}"
    );
    assert!(!events.contains(&Event::Rows(vec![vec![Value::Integer(99)]])));
    assert!(matches!(events.last(), Some(Event::Done { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_session_returns_rows_after_query_timeout() {
    let database = database().await;
    let mut connection = details(&database);
    connection.query_timeout = 1;
    let mut session = postgres::connect(&connection, Some(database.password()))
        .await
        .unwrap();
    let timed_out: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        session.run("SELECT pg_sleep(30); SELECT 99").collect(),
    )
    .await
    .expect("the one-second query timeout must end the run");
    assert!(
        timed_out.iter().any(
            |event| matches!(event, Event::Error { text, .. } if text == "canceling statement due to statement timeout")
        ),
        "{timed_out:?}"
    );
    let recovered: Vec<_> =
        tokio::time::timeout(Duration::from_secs(5), session.run("SELECT 11").collect())
            .await
            .expect("the session must remain usable after a timeout");
    artifact(
        &database,
        "query-timeout-recovery.txt",
        &format!("{recovered:#?}"),
    );
    assert!(
        !recovered
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{recovered:?}"
    );
    assert!(recovered.contains(&Event::Rows(vec![vec![Value::Integer(11)]])));
    assert!(matches!(recovered.last(), Some(Event::Done { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_zero_query_timeout_allows_a_longer_run_to_finish() {
    let database = database().await;
    let mut connection = details(&database);
    connection.query_timeout = 0;
    let mut session = postgres::connect(&connection, Some(database.password()))
        .await
        .unwrap();
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        session.run("SELECT pg_sleep(2); SELECT 11").collect(),
    )
    .await
    .expect("a two-second run must finish with no query timeout");
    artifact(&database, "query-timeout-zero.txt", &format!("{events:#?}"));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{events:?}"
    );
    assert!(events.contains(&Event::Rows(vec![vec![Value::Integer(11)]])));
    assert!(matches!(events.last(), Some(Event::Done { .. })));
}
