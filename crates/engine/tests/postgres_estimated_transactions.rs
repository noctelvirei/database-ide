use std::time::Duration;

use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::{self, Engine};
use dbide_engine::postgres::{self, Postgres};
use dbide_engine::session::{Event, PlanMode, Session, Value};
use dbide_test_servers::postgres::Postgres as Database;
use futures::StreamExt;

async fn database() -> Database {
    tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap()
}

async fn open(database: &Database) -> Postgres {
    let mut connection = Connection::new(Engine::PostgreSql, database.host());
    connection.port = Some(database.port());
    connection.user = Some(database.user().into());
    connection.database = Some(database.database().into());
    connection.tls = TlsMode::Off;
    postgres::connect(&connection, Some(database.password()))
        .await
        .unwrap()
}

async fn setup(session: &mut Postgres, sql: &str) {
    let events: Vec<_> = session.run(sql).collect().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{events:?}"
    );
}

async fn backend_pid(session: &mut Postgres) -> i64 {
    let events: Vec<_> = session.run("SELECT pg_backend_pid()").collect().await;
    let Event::Rows(rows) = &events[1] else {
        panic!("{events:?}")
    };
    let Value::Integer(pid) = rows[0][0] else {
        panic!("{rows:?}")
    };
    pid
}

async fn wait_for_lock(observer: &mut Postgres, pid: i64) {
    let sql = format!("SELECT EXISTS (SELECT 1 FROM pg_locks WHERE pid = {pid} AND NOT granted)");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let events: Vec<_> = observer.run(&sql).collect().await;
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::Error { .. })),
                "{events:?}"
            );
            if events.contains(&Event::Rows(vec![vec![Value::Bool(true)]])) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("estimated EXPLAIN must be waiting on the table lock");
}

async fn commit_preserves_insert(session: &mut Postgres, database: &Database) {
    tokio::time::timeout(Duration::from_secs(5), setup(session, "COMMIT"))
        .await
        .expect("plan cleanup must finish before COMMIT");
    let mut observer = open(database).await;
    let events: Vec<_> = observer.run("SELECT id FROM saved_rows").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(7)]]));
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let folder = root.join("test-artifacts").join(database.database());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("committed-rows.txt"), format!("{events:#?}")).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_failed_estimated_plan_preserves_uncommitted_changes() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE saved_rows(id int)").await;
    setup(&mut session, "BEGIN; INSERT INTO saved_rows VALUES (7)").await;
    let events: Vec<_> = session
        .plan("SELECT 1; SELECT * FROM absent_table", PlanMode::Estimated)
        .collect()
        .await;
    assert!(matches!(events[0], Event::Plan { .. }));
    assert!(
        matches!(&events[1], Event::Error { code, .. } if code.as_deref() == Some("42P01")),
        "{events:?}"
    );
    assert!(matches!(events.last(), Some(Event::Done { .. })));
    commit_preserves_insert(&mut session, &database).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_cancelled_estimated_plan_preserves_uncommitted_changes() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE saved_rows(id int); CREATE TABLE locked_rows(id int)",
    )
    .await;
    setup(&mut session, "BEGIN; INSERT INTO saved_rows VALUES (7)").await;
    let pid = backend_pid(&mut session).await;
    let mut locker = open(&database).await;
    setup(
        &mut locker,
        "BEGIN; LOCK TABLE locked_rows IN ACCESS EXCLUSIVE MODE",
    )
    .await;
    let mut run = session.plan("DELETE FROM locked_rows", PlanMode::Estimated);
    let handle = run.cancel_handle();
    tokio::select! {
        event = run.next() => panic!("plan must wait for the lock: {event:?}"),
        () = wait_for_lock(&mut locker, pid) => {}
    }
    handle.cancel();
    let events: Vec<_> = tokio::time::timeout(Duration::from_secs(5), run.collect())
        .await
        .unwrap();
    assert!(
        matches!(&events[0], Event::Error { text, .. } if text == "Run was cancelled"),
        "{events:?}"
    );
    assert!(matches!(events.last(), Some(Event::Done { .. })));
    commit_preserves_insert(&mut session, &database).await;
    setup(&mut locker, "ROLLBACK").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_dropped_estimated_plan_preserves_uncommitted_changes() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE saved_rows(id int); CREATE TABLE locked_rows(id int)",
    )
    .await;
    setup(&mut session, "BEGIN; INSERT INTO saved_rows VALUES (7)").await;
    let pid = backend_pid(&mut session).await;
    let mut locker = open(&database).await;
    setup(
        &mut locker,
        "BEGIN; LOCK TABLE locked_rows IN ACCESS EXCLUSIVE MODE",
    )
    .await;
    let mut run = session.plan("DELETE FROM locked_rows", PlanMode::Estimated);
    tokio::select! {
        event = run.next() => panic!("plan must wait for the lock: {event:?}"),
        () = wait_for_lock(&mut locker, pid) => {}
    }
    drop(run);
    commit_preserves_insert(&mut session, &database).await;
    setup(&mut locker, "ROLLBACK").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_failed_danger_estimate_preserves_uncommitted_changes() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE saved_rows(id int)").await;
    setup(&mut session, "BEGIN; INSERT INTO saved_rows VALUES (7)").await;
    let batch = "SELECT 1; DELETE FROM absent_table";
    let check = danger::check(batch, Engine::PostgreSql);
    assert_eq!(check.statements.len(), 1);
    let estimates =
        danger::estimates::estimate_rows(&mut session, batch, &check, Engine::PostgreSql).await;
    assert_eq!(estimates, vec![None]);
    commit_preserves_insert(&mut session, &database).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_danger_estimates_report_progress_before_cancellation_and_preserve_rows() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE saved_rows(id int); CREATE TABLE locked_rows(id int); \
         INSERT INTO locked_rows VALUES (11)",
    )
    .await;
    setup(
        &mut session,
        "BEGIN; INSERT INTO saved_rows VALUES (7); ANALYZE saved_rows",
    )
    .await;
    let pid = backend_pid(&mut session).await;
    let mut locker = open(&database).await;
    setup(
        &mut locker,
        "BEGIN; LOCK TABLE locked_rows IN ACCESS EXCLUSIVE MODE",
    )
    .await;
    let batch = "DELETE FROM saved_rows; DELETE FROM locked_rows";
    let check = danger::check(batch, Engine::PostgreSql);
    assert_eq!(check.statements.len(), 2);
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut on_estimate = |index, estimate| sender.send((index, estimate)).unwrap();
    let mut estimates = Box::pin(danger::estimates::estimate_each(
        &mut session,
        batch,
        &check,
        Engine::PostgreSql,
        &mut on_estimate,
    ));
    let first = tokio::select! {
        () = &mut estimates => panic!("second estimate must wait for the lock"),
        result = tokio::time::timeout(Duration::from_secs(10), receiver.recv()) => {
            result.expect("first estimate must arrive before the lock is released").unwrap()
        }
    };
    assert_eq!(first, (0, Some(1.0)));
    tokio::select! {
        () = &mut estimates => panic!("second estimate must wait for the lock"),
        () = wait_for_lock(&mut locker, pid) => {}
    }
    drop(estimates);
    assert_eq!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );
    commit_preserves_insert(&mut session, &database).await;
    setup(&mut locker, "ROLLBACK").await;
    let events: Vec<_> = session.run("SELECT id FROM locked_rows").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(11)]]));
}
