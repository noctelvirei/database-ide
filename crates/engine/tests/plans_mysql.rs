use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::mysql;
use dbide_engine::plan::{self, Operator};
use dbide_engine::session::{Event, PlanFormat, PlanMode, Run, Session, Value};
use dbide_test_servers::mysql::MySql as Database;
use futures::StreamExt;
use std::time::{Duration, Instant};

const JOIN: &str = "SELECT STRAIGHT_JOIN * FROM trees JOIN leaves FORCE INDEX (tree_idx) ON leaves.tree_id=trees.id";
const SETUP: &str = "CREATE TABLE trees (id int PRIMARY KEY, name varchar(20)) ENGINE=InnoDB; CREATE TABLE leaves (id int PRIMARY KEY, tree_id int, INDEX tree_idx(tree_id)) ENGINE=InnoDB; INSERT INTO trees VALUES (1,'elm'),(2,'oak'); INSERT INTO leaves VALUES (1,1),(2,1),(3,2)";

async fn open() -> (Database, mysql::MySql) {
    let db = tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = Connection::new(Engine::MySql, db.host());
    details.port = Some(db.port());
    details.database = Some(db.database().into());
    details.user = Some(db.user().into());
    details.tls = TlsMode::Off;
    let mut session = mysql::connect(&details, Some(db.password())).await.unwrap();
    let events: Vec<_> = session.run(SETUP).collect().await;
    no_errors(&events);
    (db, session)
}

fn no_errors(events: &[Event]) {
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{events:?}"
    );
    assert!(matches!(events.last(), Some(Event::Done { .. })));
}

fn parsed(events: &[Event]) -> plan::Plan {
    no_errors(events);
    let Event::Plan { payload, format } = &events[0] else {
        panic!("{events:?}")
    };
    assert_eq!(*format, PlanFormat::MySqlJson);
    plan::parse(payload, *format).unwrap()
}

fn join_tree(root: &Operator) {
    assert_eq!(root.name, "Nested loop inner join");
    assert_eq!(root.kind, "inner join");
    assert_eq!(root.children.len(), 2);
    assert_eq!(
        root.children[0].object.as_ref().unwrap().table.as_deref(),
        Some("trees")
    );
    let inner = &root.children[1];
    assert_eq!(
        inner.object.as_ref().unwrap().table.as_deref(),
        Some("leaves")
    );
    assert_eq!(
        inner.object.as_ref().unwrap().index.as_deref(),
        Some("tree_idx")
    );
}

async fn wait_for_sleep(db: Database, run: &mut Run<'_>) -> Database {
    let observed = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if db.query("SELECT COUNT(*) FROM information_schema.processlist WHERE DB=DATABASE() AND STATE='User sleep'").unwrap().trim() == "1" {
                return db;
            }
            assert!(Instant::now() < deadline, "actual plan did not reach SLEEP");
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    tokio::select! {
        db = observed => db.unwrap(),
        event = run.next() => panic!("actual plan finished before cancellation: {event:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_estimated_join_has_both_accesses_and_index_without_runtime_counters() {
    let (_db, mut session) = open().await;
    let events: Vec<_> = session.plan(JOIN, PlanMode::Estimated).collect().await;
    let plan = parsed(&events);
    join_tree(&plan.statements[0].root);
    assert_eq!(plan.statements[0].root.actual_rows, None);
    assert!(plan.statements[0].root.subtree_cost > 0.0);
    assert!(plan.statements[0].root.cost_percent > 0.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_actual_join_normalizes_rows_and_time_over_loops() {
    let (_db, mut session) = open().await;
    let events: Vec<_> = session.plan(JOIN, PlanMode::Actual).collect().await;
    let plan = parsed(&events);
    let root = &plan.statements[0].root;
    join_tree(root);
    assert_eq!(root.actual_rows, Some(3.0));
    assert_eq!(root.children[0].actual_rows, Some(2.0));
    let inner = &root.children[1];
    assert_eq!(inner.actual_rows, Some(3.0));
    assert!(inner.elapsed_ms.is_some_and(|ms| ms >= 0.0));
    assert!(
        inner
            .properties
            .iter()
            .any(|p| p.name == "actual_loops" && p.value == "2")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_actual_update_rolls_back_and_restores_explain_settings() {
    let (_db, mut session) = open().await;
    let events: Vec<_> = session.plan("UPDATE trees JOIN leaves ON leaves.tree_id=trees.id SET trees.name='ash' WHERE trees.id=1", PlanMode::Actual).collect().await;
    let plan = parsed(&events);
    assert!(plan.statements[0].root.name.starts_with("Update trees"));
    assert_eq!(plan.statements[0].root.children[0].actual_rows, Some(2.0));
    let events: Vec<_> = session
        .run("SELECT name, @@session.explain_json_format_version FROM trees WHERE id=1")
        .collect()
        .await;
    no_errors(&events);
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Text("elm".into()),
            Value::UnsignedInteger(1)
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_actual_plan_rolls_back_stored_function_writes() {
    let (_db, mut session) = open().await;
    let events: Vec<_> = session.run("CREATE FUNCTION plan_touch() RETURNS INT MODIFIES SQL DATA BEGIN SET @plan_called=1; UPDATE trees SET name='ash' WHERE id=1; RETURN 1; END").collect().await;
    no_errors(&events);
    let events: Vec<_> = session
        .plan("SELECT plan_touch()", PlanMode::Actual)
        .collect()
        .await;
    no_errors(&events);
    assert!(matches!(events[0], Event::Plan { .. }));
    let events: Vec<_> = session.run("SELECT name, CAST(@plan_called AS SIGNED), @@session.explain_json_format_version FROM trees WHERE id=1").collect().await;
    no_errors(&events);
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Text("elm".into()),
            Value::Integer(1),
            Value::UnsignedInteger(1)
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_plan_emits_one_payload_per_statement_and_preserves_user_transaction() {
    let (_db, mut session) = open().await;
    let events: Vec<_> = session
        .plan("SELECT 'é;elm'; SELECT 2", PlanMode::Estimated)
        .collect()
        .await;
    no_errors(&events);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Plan { .. }))
            .count(),
        2
    );
    let events: Vec<_> = session
        .run("START TRANSACTION; UPDATE trees SET name='fir' WHERE id=1")
        .collect()
        .await;
    no_errors(&events);
    let events: Vec<_> = session.plan(JOIN, PlanMode::Actual).collect().await;
    assert!(matches!(&events[0], Event::Error { text, .. } if text.contains("active transaction")));
    let events: Vec<_> = session
        .run("SELECT name FROM trees WHERE id=1; ROLLBACK")
        .collect()
        .await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![Value::Text("fir".into())]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_actual_plan_errors_roll_back_before_session_reuse() {
    let (_db, mut session) = open().await;
    let events: Vec<_> = session
        .plan("SELECT * FROM absent", PlanMode::Actual)
        .collect()
        .await;
    assert!(matches!(&events[0], Event::Error { .. }));
    assert!(matches!(events.last(), Some(Event::Done { .. })));
    let events: Vec<_> = session.plan(JOIN, PlanMode::Actual).collect().await;
    assert_eq!(parsed(&events).statements[0].root.actual_rows, Some(3.0));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_dropped_actual_plan_rolls_back_stored_function_writes_before_reuse() {
    let (db, mut session) = open().await;
    let events: Vec<_> = session.run("CREATE FUNCTION plan_pause() RETURNS INT MODIFIES SQL DATA BEGIN UPDATE trees SET name='ash' WHERE id=1; RETURN SLEEP(30); END").collect().await;
    no_errors(&events);
    let mut run = session.plan("SELECT plan_pause()", PlanMode::Actual);
    let _db = wait_for_sleep(db, &mut run).await;
    drop(run);
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(5),
        session
            .run("SELECT name, @@session.explain_json_format_version FROM trees ORDER BY id")
            .collect(),
    )
    .await
    .unwrap();
    no_errors(&events);
    assert_eq!(
        events[1],
        Event::Rows(vec![
            vec![Value::Text("elm".into()), Value::UnsignedInteger(1)],
            vec![Value::Text("oak".into()), Value::UnsignedInteger(1)],
        ])
    );
    let events: Vec<_> = session.plan(JOIN, PlanMode::Actual).collect().await;
    assert_eq!(parsed(&events).statements[0].root.actual_rows, Some(3.0));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_actual_plan_refuses_disabled_autocommit_without_changing_rows() {
    let (_db, mut session) = open().await;
    let events: Vec<_> = session.run("SET autocommit=0").collect().await;
    no_errors(&events);
    let events: Vec<_> = session
        .plan("UPDATE trees SET name='ash'", PlanMode::Actual)
        .collect()
        .await;
    assert!(matches!(&events[0], Event::Error { text, .. } if text.contains("autocommit enabled")));
    let events: Vec<_> = session
        .run("SELECT name FROM trees WHERE id=1; ROLLBACK; SET autocommit=1")
        .collect()
        .await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![Value::Text("elm".into())]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_plan_refuses_executable_comments_and_preserves_literal_spelling() {
    let (_db, mut session) = open().await;
    let events: Vec<_> = session
        .plan(
            "SELECT 1; /*! CREATE TABLE hidden_ddl (id int) */",
            PlanMode::Actual,
        )
        .collect()
        .await;
    assert!(
        matches!(&events[0], Event::Error { text, .. } if text.contains("DDL")),
        "{events:?}"
    );
    let events: Vec<_> = session.run("SHOW TABLES LIKE 'hidden_ddl'").collect().await;
    assert_eq!(events[1], Event::RowCount(0));
    let events: Vec<_> = session
        .plan(r"SELECT 'é;elm'; SELECT 'a\nb'", PlanMode::Estimated)
        .collect()
        .await;
    no_errors(&events);
    let Event::Plan { payload, .. } = &events[1] else {
        panic!("{events:?}")
    };
    let plan = plan::parse(payload, PlanFormat::MySqlJson).unwrap();
    assert!(
        plan.statements[0]
            .text
            .as_deref()
            .unwrap()
            .contains(r"a\nb"),
        "{payload}"
    );
}

#[test]
fn mysql_plans_cannot_overwrite_sqlplan_files() {
    let plan = plan::parse(
        include_str!("fixtures/plans/mysql-actual.json"),
        PlanFormat::MySqlJson,
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retained.sqlplan");
    std::fs::write(&path, "retained").unwrap();
    assert!(plan::showplan::save(&path, &plan).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "retained");
}

#[test]
fn mysql_json_captures_retain_raw_fields_and_normalize_inner_costs() {
    let estimated = plan::parse(
        include_str!("fixtures/plans/mysql-estimated.json"),
        PlanFormat::MySqlJson,
    )
    .unwrap();
    join_tree(&estimated.statements[0].root);
    let actual = plan::parse(
        include_str!("fixtures/plans/mysql-actual.json"),
        PlanFormat::MySqlJson,
    )
    .unwrap();
    let root = &actual.statements[0].root;
    join_tree(root);
    assert_eq!(root.actual_rows, Some(3.0));
    assert_eq!(root.children[1].actual_rows, Some(3.0));
    assert_eq!(root.children[1].elapsed_ms, Some(0.0131));
    assert!(root.children[1].subtree_cost > 0.65);
    assert!(root.children[1].subtree_cost < 0.66);
    assert!(
        root.properties
            .iter()
            .any(|p| p.name == "query_type" && p.value == "\"select\"")
    );
}

#[test]
fn mysql_json_unknown_properties_survive_and_invalid_counters_fail() {
    let plan = plan::parse(r#"{"operation":"Future scan","estimated_rows":1,"warnings":["server warning"],"future":{"inputs":42},"actual_loops":0,"actual_rows":0}"#, PlanFormat::MySqlJson).unwrap();
    let root = &plan.statements[0].root;
    assert_eq!(root.actual_rows, Some(0.0));
    assert_eq!(root.estimated_rows_total, 0.0);
    assert_eq!(root.warnings, vec!["[\"server warning\"]"]);
    assert!(
        root.properties
            .iter()
            .any(|p| p.name == "future/inputs" && p.value == "42")
    );
    assert!(
        plan::parse(
            r#"{"operation":"Scan","actual_rows":-1}"#,
            PlanFormat::MySqlJson
        )
        .is_err()
    );
    assert!(plan::parse(r#"{"query_block":{}}"#, PlanFormat::MySqlJson).is_err());
}
