use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::{self, Engine};
use dbide_engine::session::{Event, Session, Value};
use futures::StreamExt;

async fn run(session: &mut dyn Session, sql: &str) -> Vec<Vec<Value>> {
    let mut rows = Vec::new();
    let mut run = session.run(sql);
    while let Some(event) = run.next().await {
        match event {
            Event::Rows(batch) => rows.extend(batch),
            Event::Error { text, .. } => panic!("{text}"),
            _ => {}
        }
    }
    rows
}

async fn estimates_preserve_rows(session: &mut dyn Session, engine: Engine, statistics: &str) {
    run(session, "CREATE TABLE t (n int NOT NULL); INSERT INTO t VALUES (1),(2),(3),(4),(5),(6),(7),(8),(9),(10)").await;
    run(session, statistics).await;
    let batch = "DELETE FROM missing_table; DELETE FROM t WHERE n = 1; DELETE FROM t; UPDATE t SET n = 99; UPDATE missing_table SET n = 0; DROP TABLE t; DELETE FROM t";
    let check = danger::check(batch, engine);
    assert!(check.fully_checked);
    let estimates = danger::estimates::estimate_rows(session, batch, &check, engine).await;
    assert_eq!(estimates.len(), 6);
    assert_eq!(estimates[0], None);
    assert!(
        estimates[1].is_some_and(|n| (8.0..=12.0).contains(&n)),
        "{estimates:?}"
    );
    assert!(
        estimates[2].is_some_and(|n| (8.0..=12.0).contains(&n)),
        "{estimates:?}"
    );
    assert_eq!(estimates[3], None);
    assert_eq!(estimates[4], None);
    assert!(
        estimates[5].is_some_and(|n| (8.0..=12.0).contains(&n)),
        "{estimates:?}"
    );
    assert_eq!(
        run(session, "SELECT COUNT(*) FROM t WHERE n BETWEEN 1 AND 10").await,
        vec![vec![Value::Integer(10)]]
    );
    let folder = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "../../target".into())
        .join("test-artifacts/danger-estimates");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join(format!("{engine:?}.json")),
        serde_json::to_vec(&estimates).unwrap(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_danger_estimates_isolate_failures_and_preserve_rows() {
    let db = tokio::task::spawn_blocking(dbide_test_servers::mssql::Mssql::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = Connection::new(Engine::SqlServer, db.host());
    details.port = Some(db.port());
    details.user = Some(db.user().into());
    details.database = Some(db.database().into());
    details.trust_server_certificate = true;
    let mut session = dbide_engine::mssql::connect(&details, Some(db.password()))
        .await
        .unwrap();
    estimates_preserve_rows(
        &mut session,
        Engine::SqlServer,
        "UPDATE STATISTICS t WITH FULLSCAN",
    )
    .await;
    let batch =
        "DELETE FROM t\r\nGO\r\nunsupported syntax\r\nGO\r\nUPDATE t SET n = 99\r\nDELETE FROM t";
    let check = danger::check(batch, Engine::SqlServer);
    assert!(!check.fully_checked);
    let estimates =
        danger::estimates::estimate_rows(&mut session, batch, &check, Engine::SqlServer).await;
    assert_eq!(estimates.len(), 3);
    assert!(
        estimates
            .iter()
            .all(|estimate| estimate.is_some_and(|n| (8.0..=12.0).contains(&n))),
        "{estimates:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_danger_estimates_isolate_failures_and_preserve_rows() {
    let db = tokio::task::spawn_blocking(dbide_test_servers::postgres::Postgres::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = Connection::new(Engine::PostgreSql, db.host());
    details.port = Some(db.port());
    details.user = Some(db.user().into());
    details.database = Some(db.database().into());
    details.tls = TlsMode::Off;
    let mut session = dbide_engine::postgres::connect(&details, Some(db.password()))
        .await
        .unwrap();
    estimates_preserve_rows(&mut session, Engine::PostgreSql, "ANALYZE t").await;
    let batch = "UPDATE t SET n = (SELECT max(n) FROM t)";
    let check = danger::check(batch, Engine::PostgreSql);
    let estimates =
        danger::estimates::estimate_rows(&mut session, batch, &check, Engine::PostgreSql).await;
    assert!(
        estimates[0].is_some_and(|n| (8.0..=12.0).contains(&n)),
        "{estimates:?}"
    );
    let batch = "WITH d AS (DELETE FROM t RETURNING n) DELETE FROM t WHERE n=1; DELETE FROM t";
    let check = danger::check(batch, Engine::PostgreSql);
    let estimates =
        danger::estimates::estimate_rows(&mut session, batch, &check, Engine::PostgreSql).await;
    assert_eq!(estimates.len(), 2);
    assert_eq!(estimates[0], None);
    assert!(
        estimates[1].is_some_and(|n| (8.0..=12.0).contains(&n)),
        "{estimates:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_danger_estimates_isolate_failures_and_preserve_rows() {
    let db = tokio::task::spawn_blocking(dbide_test_servers::mysql::MySql::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = Connection::new(Engine::MySql, db.host());
    details.port = Some(db.port());
    details.user = Some(db.user().into());
    details.database = Some(db.database().into());
    details.tls = TlsMode::Off;
    let mut session = dbide_engine::mysql::connect(&details, Some(db.password()))
        .await
        .unwrap();
    estimates_preserve_rows(&mut session, Engine::MySql, "ANALYZE TABLE t").await;
    assert_eq!(
        run(&mut session, "SELECT @@session.explain_json_format_version").await,
        vec![vec![Value::UnsignedInteger(1)]]
    );
}
