use std::time::Duration;

use dbide_engine::connections::{Connection, SignInMethod};
use dbide_engine::danger::{self, Engine};
use dbide_engine::mssql::{self, Mssql};
use dbide_engine::session::temporal::{Date, Time, Timestamp, TimestampWithZone};
use dbide_engine::session::{
    Column, Event, PlanFormat, PlanMode, Session, Severity, TypeCategory, Value,
};
use dbide_test_servers::mssql::Mssql as Database;
use futures::StreamExt;

fn details(database: &Database) -> Connection {
    let mut details = Connection::new(Engine::SqlServer, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.trust_server_certificate = true;
    details
}

async fn database() -> Database {
    tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap()
}

async fn open(database: &Database) -> Mssql {
    mssql::connect(&details(database), Some(database.password()))
        .await
        .unwrap()
}

async fn setup(session: &mut Mssql, sql: &str) {
    let events: Vec<_> = session.run(sql).collect().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{events:?}"
    );
}

async fn fixture(file: &'static str) -> Database {
    tokio::task::spawn_blocking(move || {
        let database = Database::start().unwrap();
        database
            .load_script(std::path::Path::new("../../testdata").join(file))
            .unwrap();
        database
    })
    .await
    .unwrap()
}

fn artifact(database: &Database, name: &str, contents: &str) {
    let folder = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "../../target".into())
        .join("test-artifacts")
        .join(database.database());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), contents).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_select_streams_typed_columns_rows_count_then_done() {
    let database = database().await;
    let mut session = open(&database).await;
    assert_eq!(session.current_database(), database.database());
    let events: Vec<_> = session
        .run("\u{feff}SELECT CAST(7 AS int) AS answer")
        .collect()
        .await;
    assert_eq!(
        events[0],
        Event::ResultSetStarted {
            columns: vec![Column {
                name: "answer".into(),
                type_name: "int".into(),
                category: TypeCategory::Integer,
                // SQL Server reports a CAST expression as nullable.
                nullable: Some(true),
                base_table: None,
                base_column: None,
            }]
        }
    );
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(7)]]));
    assert_eq!(events[2], Event::RowCount(1));
    assert!(matches!(events[3], Event::Done { .. }));
    assert_eq!(events.len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_values_preserve_null_exact_numbers_uuid_binary_and_xml() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT CAST(NULL AS int), CAST(12345678901234567890.125 AS decimal(23,3)), CAST(-922337203685477.5808 AS money), CAST(-214748.3648 AS smallmoney), CAST('00112233-4455-6677-8899-aabbccddeeff' AS uniqueidentifier), CAST(0x00ff5c AS varbinary(max)), CAST(N'<v>雪</v>' AS xml), CAST(1 AS bit), CAST(1.25 AS float), CAST(N'雪é' AS nvarchar(10))").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Null,
            Value::Decimal("12345678901234567890.125".into()),
            Value::Decimal("-922337203685477.5808".into()),
            Value::Decimal("-214748.3648".into()),
            Value::Uuid([
                0, 17, 34, 51, 68, 85, 102, 119, 136, 153, 170, 187, 204, 221, 238, 255
            ]),
            Value::Bytes(vec![0, 255, 92]),
            Value::Text("<v>雪</v>".into()),
            Value::Bool(true),
            Value::Float(1.25),
            Value::Text("雪é".into()),
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_temporal_values_preserve_local_clock_offset_and_precision() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT CAST('2024-02-29' AS date), CAST('12:34:56.1234567' AS time(7)), CAST('1753-01-01T00:00:00.003' AS datetime), CAST('2024-02-29T12:34:00' AS smalldatetime), CAST('9999-12-31T23:59:59.9999999' AS datetime2(7)), CAST('2024-02-29T00:15:00.1234567+05:30' AS datetimeoffset(7))").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Date(Date::parse("2024-02-29").unwrap()),
            Value::Time(Time::parse("12:34:56.1234567").unwrap()),
            Value::Timestamp(Timestamp::parse("1753-01-01T00:00:00.003").unwrap()),
            Value::Timestamp(Timestamp::parse("2024-02-29T12:34:00").unwrap()),
            Value::Timestamp(Timestamp::parse("9999-12-31T23:59:59.9999999").unwrap()),
            Value::TimestampWithZone(
                TimestampWithZone::parse("2024-02-29T00:15:00.1234567+05:30").unwrap()
            ),
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_go_repeats_plain_batches_in_the_same_session() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("CREATE TABLE #t (n int);\nGO\nINSERT INTO #t VALUES (1);\ngo 3\nSELECT COUNT(*) AS total FROM #t").collect().await;
    let rows: Vec<_> = events
        .into_iter()
        .filter_map(|event| match event {
            Event::Rows(rows) => Some(rows),
            _ => None,
        })
        .collect();
    assert_eq!(rows, vec![vec![vec![Value::Integer(3)]]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_go_inside_strings_and_comments_is_not_a_boundary() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("/* outer\n/* inner */\nGO\n*/\nSELECT 'a\nGO\nb' AS text;\n-- GO\n")
        .collect()
        .await;
    assert!(matches!(&events[0], Event::ResultSetStarted { .. }));
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![Value::Text("a\nGO\nb".into())]])
    );
    assert_eq!(events[2], Event::RowCount(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_print_keeps_its_position_between_results() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("SELECT 1;\nPRINT 'birch';\nSELECT 2;")
        .collect()
        .await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(1)]]));
    assert_eq!(events[2], Event::RowCount(1));
    assert_eq!(
        events[3],
        Event::Message {
            text: "birch".into(),
            severity: Severity::Server(0),
            line: std::num::NonZeroU64::new(2)
        }
    );
    assert!(matches!(events[4], Event::ResultSetStarted { .. }));
    assert_eq!(events[5], Event::Rows(vec![vec![Value::Integer(2)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_low_severity_raiserror_is_a_message() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("RAISERROR('willow', 9, 1)").collect().await;
    assert!(
        matches!(&events[0], Event::Message { text, severity: Severity::Server(9), .. } if text == "willow")
    );
    assert!(matches!(events[1], Event::Done { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_error_reports_whole_script_line_and_session_recovers() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT 1;\nGO\n-- first\n-- second\n-- third\nRAISERROR('broken branch', 16, 1);\nSELECT 2;").collect().await;
    assert!(events.contains(&Event::Error {
        text: "broken branch".into(),
        line: std::num::NonZeroU64::new(6),
        code: Some("50000".into())
    }));
    assert!(events.contains(&Event::Rows(vec![vec![Value::Integer(2)]])));
    setup(&mut session, "SELECT 3").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_done_counts_respect_nocount_and_include_zero() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("CREATE TABLE #t(n int); INSERT INTO #t VALUES (1),(2); UPDATE #t SET n = 3 WHERE n = 4; SET NOCOUNT ON; SELECT n FROM #t").collect().await;
    let counts: Vec<_> = events
        .into_iter()
        .filter_map(|event| match event {
            Event::RowCount(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(counts, vec![2, 0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_waitfor_cancel_finishes_promptly_and_session_recovers() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut run = session.run("WAITFOR DELAY '00:00:30'");
    let cancel = run.cancel_handle();
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    });
    let events: Vec<_> = tokio::time::timeout(Duration::from_secs(5), (&mut run).collect())
        .await
        .unwrap();
    task.await.unwrap();
    assert!(matches!(&events[0], Event::Error { text, .. } if text == "Run was cancelled"));
    assert!(matches!(events[1], Event::Done { .. }));
    drop(run);
    tokio::time::timeout(Duration::from_secs(5), setup(&mut session, "SELECT 42"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_midstream_cancel_stops_rows_and_session_recovers() {
    let database = fixture("mssql-large.sql").await;
    let mut session = open(&database).await;
    let mut run = session.run("SELECT id, payload FROM shop.big_rows ORDER BY id");
    assert!(matches!(
        run.next().await,
        Some(Event::ResultSetStarted { .. })
    ));
    assert!(matches!(run.next().await, Some(Event::Rows(rows)) if rows.len() == 256));
    run.cancel_handle().cancel();
    let events: Vec<_> = tokio::time::timeout(Duration::from_secs(5), (&mut run).collect())
        .await
        .unwrap();
    assert!(matches!(&events[0], Event::Error { text, .. } if text == "Run was cancelled"));
    assert!(matches!(events[1], Event::Done { .. }));
    assert_eq!(events.len(), 2);
    drop(run);
    tokio::time::timeout(Duration::from_secs(5), setup(&mut session, "SELECT 42"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_dropping_a_run_cancels_before_the_next_batch() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut run = session.run("SELECT 1; WAITFOR DELAY '00:00:30'");
    assert!(matches!(
        run.next().await,
        Some(Event::ResultSetStarted { .. })
    ));
    drop(run);
    tokio::time::timeout(Duration::from_secs(5), setup(&mut session, "SELECT 42"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_estimated_plans_parse_without_executing_changes() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t(n int); INSERT INTO t VALUES (1),(2),(3)",
    )
    .await;
    let events: Vec<_> = session
        .plan("DELETE FROM t", PlanMode::Estimated)
        .collect()
        .await;
    let payload = events
        .iter()
        .find_map(|event| match event {
            Event::Plan {
                payload,
                format: PlanFormat::ShowplanXml,
            } => Some(payload),
            _ => None,
        })
        .expect("estimated XML plan");
    let plan = dbide_engine::plan::parse(payload, PlanFormat::ShowplanXml).unwrap();
    assert_eq!(plan.statements.len(), 1);
    artifact(&database, "estimated.sqlplan", payload);
    let events: Vec<_> = session.run("SELECT COUNT(*) FROM t").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(3)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_actual_plans_parse_and_keep_their_data_changes() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t(n int); INSERT INTO t VALUES (1),(2),(3)",
    )
    .await;
    let events: Vec<_> = session
        .plan(
            "DELETE FROM t WHERE n = 1; SELECT n FROM t",
            PlanMode::Actual,
        )
        .collect()
        .await;
    let payloads: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::Plan { payload, .. } => Some(payload),
            _ => None,
        })
        .collect();
    assert!(!payloads.is_empty(), "{events:?}");
    let plan = dbide_engine::plan::parse(payloads[0], PlanFormat::ShowplanXml).unwrap();
    assert!(
        plan.statements[0].root.actual_rows.is_some()
            || plan.statements[0].root.children[0].actual_rows.is_some()
    );
    artifact(&database, "actual.sqlplan", payloads[0]);
    let events: Vec<_> = session.run("SELECT COUNT(*) FROM t").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(2)]]));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Plan { .. }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_plan_mode_is_disabled_after_a_sql_error() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .plan("SELECT * FROM missing_table", PlanMode::Estimated)
        .collect()
        .await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Error { .. }))
    );
    let events: Vec<_> = session.run("SELECT 42").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(42)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_danger_estimate_matches_rows_without_deleting() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE t(n int); INSERT INTO t SELECT value FROM GENERATE_SERIES(1,100); UPDATE STATISTICS t WITH FULLSCAN").await;
    let flagged = danger::check("DELETE FROM t", Engine::SqlServer);
    let estimate = mssql::estimate_rows(&mut session, "DELETE FROM t", &flagged.statements)
        .await
        .unwrap();
    assert!((estimate[0].unwrap() - 100.0).abs() < 1.0);
    let events: Vec<_> = session.run("SELECT COUNT(*) FROM t").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(100)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_danger_estimate_skips_a_safe_delete_on_the_same_line() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE t(n int PRIMARY KEY); INSERT INTO t SELECT value FROM GENERATE_SERIES(1,100); UPDATE STATISTICS t WITH FULLSCAN").await;
    let script = "DELETE FROM t WHERE n = 1; DELETE FROM t";
    let flagged = danger::check(script, Engine::SqlServer);
    let estimate = mssql::estimate_rows(&mut session, script, &flagged.statements)
        .await
        .unwrap();
    assert_eq!(estimate.len(), 1);
    assert!((estimate[0].unwrap() - 100.0).abs() < 1.0, "{estimate:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_wrong_password_reports_login_failure_without_the_password() {
    let database = database().await;
    let error = mssql::connect(&details(&database), Some("wrong-password-never-report"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Login failed"), "{error}");
    assert!(!format!("{error:?}").contains("wrong-password-never-report"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_windows_signin_uses_the_current_account() {
    let database = database().await;
    let mut details = details(&database);
    details.sign_in = SignInMethod::Windows;
    details.user = None;
    let mut session = mssql::connect(&details, None).await.unwrap();
    setup(&mut session, "SELECT 1").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_fixture_loads_nullable_columns_and_variant_values() {
    let database = fixture("mssql.sql").await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("SELECT bool_value, variant_value FROM shop.common_types ORDER BY id")
        .collect()
        .await;
    assert!(
        matches!(&events[0], Event::ResultSetStarted { columns } if columns[0].nullable == Some(true) && columns[1].category == TypeCategory::Other)
    );
    assert!(
        matches!(&events[1], Event::Rows(rows) if rows[0] == vec![Value::Null, Value::Null] && matches!(&rows[1][1], Value::Other(_)))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_use_updates_the_current_database_after_the_run() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "USE master").await;
    assert_eq!(session.current_database(), "master");
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_cancelled_actual_plan_turns_statistics_xml_off() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut run = session.plan("SELECT 1; WAITFOR DELAY '00:00:30'", PlanMode::Actual);
    assert!(matches!(
        run.next().await,
        Some(Event::ResultSetStarted { .. })
    ));
    run.cancel_handle().cancel();
    let events: Vec<_> = (&mut run).collect().await;
    assert!(matches!(&events[0], Event::Error { text, .. } if text == "Run was cancelled"));
    drop(run);
    let events: Vec<_> =
        tokio::time::timeout(Duration::from_secs(5), session.run("SELECT 42").collect())
            .await
            .unwrap();
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(42)]]));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Plan { .. }))
    );
    assert_eq!(events.len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_killed_connection_reports_error_before_done() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT @@SPID").collect().await;
    let Event::Rows(rows) = &events[1] else {
        panic!("session identifier row missing");
    };
    let Value::Integer(id) = rows[0][0] else {
        panic!("session identifier is not an integer");
    };
    let mut details = details(&database);
    details.sign_in = SignInMethod::Windows;
    details.user = None;
    let mut administrator = mssql::connect(&details, None).await.unwrap();
    setup(&mut administrator, &format!("KILL {id}")).await;
    let events: Vec<_> = session.run("SELECT 42").collect().await;
    assert!(matches!(&events[0], Event::Error { .. }));
    assert!(matches!(events.last(), Some(Event::Done { .. })));
    assert!(!events.iter().any(|event| matches!(event, Event::Rows(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_danger_estimate_matches_delete_after_a_multiline_cte() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE t(n int); INSERT INTO t SELECT value FROM GENERATE_SERIES(1,100); UPDATE STATISTICS t WITH FULLSCAN").await;
    let sql = "WITH targets AS (SELECT n FROM t)\nDELETE FROM targets";
    let flagged = danger::check(sql, Engine::SqlServer);
    assert_eq!(flagged.statements[0].line, 2);
    let estimate = mssql::estimate_rows(&mut session, sql, &flagged.statements)
        .await
        .unwrap();
    assert!((estimate[0].unwrap() - 100.0).abs() < 1.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_harness_drop_removes_its_database_and_login_even_after_use_master() {
    let database = database().await;
    let name = database.database().to_owned();
    let user = database.user().to_owned();
    let mut session = open(&database).await;
    setup(&mut session, "USE master").await;
    let mut details = details(&database);
    details.database = Some("master".into());
    details.sign_in = SignInMethod::Windows;
    let mut administrator = mssql::connect(&details, None).await.unwrap();
    drop(database);
    let sql = format!(
        "SELECT (SELECT COUNT(*) FROM sys.databases WHERE name = N'{name}'), (SELECT COUNT(*) FROM sys.server_principals WHERE name = N'{user}')"
    );
    let events: Vec<_> = administrator.run(&sql).collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![Value::Integer(0), Value::Integer(0)]])
    );
}
