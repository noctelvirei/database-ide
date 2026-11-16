use std::time::Duration;

use dbide_engine::connections::{Connection, SignInMethod, TlsMode};
use dbide_engine::danger::{self, Engine};
use dbide_engine::postgres::{self, Postgres};
use dbide_engine::session::temporal::{Date, Time, TimeWithZone, Timestamp, TimestampWithZone};
use dbide_engine::session::{
    Column, Event, PlanFormat, PlanMode, Session, Severity, TypeCategory, Value,
};
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

async fn open(database: &Database) -> Postgres {
    postgres::connect(&details(database), Some(database.password()))
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

fn artifact(database: &Database, name: &str, contents: &str) {
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let folder = root.join("test-artifacts").join(database.database());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), contents).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_select_streams_typed_columns_rows_count_then_done() {
    let database = database().await;
    let mut session = open(&database).await;
    assert_eq!(session.current_database(), database.database());
    let events: Vec<_> = session
        .run("SELECT 7::int AS answer, 'elm'::text AS name")
        .collect()
        .await;
    assert_eq!(
        events[0],
        Event::ResultSetStarted {
            columns: vec![
                Column {
                    name: "answer".into(),
                    type_name: "int4".into(),
                    category: TypeCategory::Integer,
                    nullable: None,
                    base_table: None,
                    base_column: None
                },
                Column {
                    name: "name".into(),
                    type_name: "text".into(),
                    category: TypeCategory::Text,
                    nullable: None,
                    base_table: None,
                    base_column: None
                },
            ]
        }
    );
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![Value::Integer(7), Value::Text("elm".into())]])
    );
    assert_eq!(events[2], Event::RowCount(1));
    assert!(matches!(events[3], Event::Done { .. }));
    assert_eq!(events.len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_text_protocol_preserves_null_binary_decimal_uuid_and_json() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT NULL::text, decode('00ff5c', 'hex'), 12345678901234567890.125::numeric, '00112233-4455-6677-8899-aabbccddeeff'::uuid, '{\"key\": 2}'::jsonb").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Null,
            Value::Bytes(vec![0, 255, 92]),
            Value::Decimal("12345678901234567890.125".into()),
            Value::Uuid([
                0, 17, 34, 51, 68, 85, 102, 119, 136, 153, 170, 187, 204, 221, 238, 255
            ]),
            Value::Json("{\"key\": 2}".into())
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_temporal_text_converts_to_validated_iso_values() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "SET TIME ZONE 'UTC'").await;
    let events: Vec<_> = session.run("SELECT DATE '2024-02-29', TIME '12:34:56.123456', TIMETZ '12:34:56+05:30', TIMESTAMP '2024-02-29 12:34:56', TIMESTAMPTZ '2024-02-29 12:34:56+00'").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Date(Date::parse("2024-02-29").unwrap()),
            Value::Time(Time::parse("12:34:56.123456").unwrap()),
            Value::TimeWithZone(TimeWithZone::parse("12:34:56+05:30").unwrap()),
            Value::Timestamp(Timestamp::parse("2024-02-29T12:34:56").unwrap()),
            Value::TimestampWithZone(
                TimestampWithZone::parse("2024-02-29T12:34:56+00:00").unwrap()
            )
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_bool_integer_and_float_convert_without_precision_loss() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT true, false, 9223372036854775807::bigint, -2::smallint, 1.25::float8, 'Infinity'::float8").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Integer(i64::MAX),
            Value::Integer(-2),
            Value::Float(1.25),
            Value::Float(f64::INFINITY)
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_unsupported_and_infinite_values_retain_server_text() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("SELECT ARRAY[1,2], 'infinity'::date, 'infinity'::timestamp, '1 day'::interval")
        .collect()
        .await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Other("{1,2}".into()),
            Value::Other("infinity".into()),
            Value::Other("infinity".into()),
            Value::Other("1 day".into())
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_two_selects_keep_separate_result_sets() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("SELECT 1 AS first; SELECT 2 AS second")
        .collect()
        .await;
    assert!(
        matches!(&events[0], Event::ResultSetStarted { columns } if columns[0].name == "first")
    );
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(1)]]));
    assert_eq!(events[2], Event::RowCount(1));
    assert!(
        matches!(&events[3], Event::ResultSetStarted { columns } if columns[0].name == "second")
    );
    assert_eq!(events[4], Event::Rows(vec![vec![Value::Integer(2)]]));
    assert_eq!(events[5], Event::RowCount(1));
    assert!(matches!(events[6], Event::Done { .. }));
    assert_eq!(events.len(), 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_notice_keeps_its_position_between_results() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("SELECT 1; DO $$ BEGIN RAISE NOTICE 'birch'; END $$; SELECT 2")
        .collect()
        .await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(1)]]));
    assert_eq!(events[2], Event::RowCount(1));
    assert_eq!(
        events[3],
        Event::Message {
            text: "birch".into(),
            severity: Severity::Info,
            line: None
        }
    );
    assert!(matches!(events[4], Event::ResultSetStarted { .. }));
    assert_eq!(events[5], Event::Rows(vec![vec![Value::Integer(2)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_warning_uses_warning_severity() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("DO $$ BEGIN RAISE WARNING 'willow'; END $$")
        .collect()
        .await;
    assert_eq!(
        events[0],
        Event::Message {
            text: "willow".into(),
            severity: Severity::Warning,
            line: None
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_syntax_error_reports_sqlstate_and_batch_line_three() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT 'é';\n\nSELEC 2").collect().await;
    assert!(
        matches!(&events[0], Event::Error { text, line, code } if text.contains("syntax error") && *line == Some(3.try_into().unwrap()) && code.as_deref() == Some("42601"))
    );
    assert!(matches!(events[1], Event::Done { .. }));
    assert_eq!(events.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_session_remains_usable_after_server_error() {
    let database = database().await;
    let mut session = open(&database).await;
    let failed: Vec<_> = session.run("SELECT * FROM absent_table").collect().await;
    assert!(matches!(&failed[0], Event::Error { code, .. } if code.as_deref() == Some("42P01")));
    let events: Vec<_> = session.run("SELECT 9").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(9)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_cancel_sleep_finishes_promptly_and_session_recovers() {
    let database = database().await;
    let mut session = open(&database).await;
    let run = session.run("SELECT pg_sleep(30)");
    let handle = run.cancel_handle();
    let cancellation = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.cancel();
    });
    let events: Vec<_> = tokio::time::timeout(Duration::from_secs(3), run.collect())
        .await
        .unwrap();
    cancellation.await.unwrap();
    assert!(
        matches!(&events[events.len()-2], Event::Error { text, code: None, line: None } if text == "Run was cancelled")
    );
    assert!(matches!(events.last(), Some(Event::Done { .. })));
    let recovered: Vec<_> =
        tokio::time::timeout(Duration::from_secs(3), session.run("SELECT 4").collect())
            .await
            .unwrap();
    assert_eq!(recovered[1], Event::Rows(vec![vec![Value::Integer(4)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_cancel_mid_stream_stops_large_table_rows() {
    let database = database().await;
    let database = tokio::task::spawn_blocking(move || {
        database
            .load_script("../../testdata/postgres-large.sql")
            .unwrap();
        database
    })
    .await
    .unwrap();
    let mut session = open(&database).await;
    let mut run = session.run("SELECT id, payload FROM shop.big_rows");
    assert!(matches!(
        run.next().await,
        Some(Event::ResultSetStarted { .. })
    ));
    assert!(matches!(run.next().await, Some(Event::Rows(rows)) if rows.len() == 256));
    run.cancel_handle().cancel();
    assert!(
        matches!(run.next().await, Some(Event::Error { text, .. }) if text == "Run was cancelled")
    );
    assert!(matches!(run.next().await, Some(Event::Done { .. })));
    assert_eq!(run.next().await, None);
    drop(run);
    let recovered: Vec<_> =
        tokio::time::timeout(Duration::from_secs(3), session.run("SELECT 5").collect())
            .await
            .unwrap();
    assert_eq!(recovered[1], Event::Rows(vec![vec![Value::Integer(5)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_dropping_a_pending_run_cancels_before_the_next_batch() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut run = session.run("SELECT pg_sleep(30)");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), run.next())
            .await
            .is_err()
    );
    drop(run);
    let recovered: Vec<_> =
        tokio::time::timeout(Duration::from_secs(3), session.run("SELECT 6").collect())
            .await
            .unwrap();
    assert_eq!(recovered[1], Event::Rows(vec![vec![Value::Integer(6)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_actual_delete_plan_rolls_back_rows() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t(id int); INSERT INTO t VALUES (1), (2), (3)",
    )
    .await;
    let events: Vec<_> = session
        .plan("DELETE FROM t", PlanMode::Actual)
        .collect()
        .await;
    assert!(
        matches!(&events[0], Event::Plan { payload, format: PlanFormat::ExplainJson } if payload.contains("Actual Rows"))
    );
    assert!(matches!(events[1], Event::Done { .. }));
    let rows: Vec<_> = session.run("SELECT count(*) FROM t").collect().await;
    assert_eq!(rows[1], Event::Rows(vec![vec![Value::Integer(3)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_actual_cte_data_changes_roll_back() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t(id int); INSERT INTO t VALUES (1), (2)",
    )
    .await;
    let events: Vec<_> = session
        .plan(
            "WITH removed AS (DELETE FROM t RETURNING *) SELECT * FROM removed",
            PlanMode::Actual,
        )
        .collect()
        .await;
    assert!(matches!(events[0], Event::Plan { .. }));
    let rows: Vec<_> = session.run("SELECT count(*) FROM t").collect().await;
    assert_eq!(rows[1], Event::Rows(vec![vec![Value::Integer(2)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_actual_plan_preserves_a_callers_open_transaction() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t(id int); BEGIN; INSERT INTO t VALUES (7)",
    )
    .await;
    let events: Vec<_> = session
        .plan("DELETE FROM t", PlanMode::Actual)
        .collect()
        .await;
    assert!(
        matches!(&events[0], Event::Error { text, .. } if text.contains("outside a transaction"))
    );
    setup(&mut session, "COMMIT").await;
    let rows: Vec<_> = session.run("SELECT id FROM t").collect().await;
    assert_eq!(rows[1], Event::Rows(vec![vec![Value::Integer(7)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_estimated_plan_parses_and_does_not_execute() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .plan("SELECT 1; SELECT 2", PlanMode::Estimated)
        .collect()
        .await;
    let Event::Plan { payload, format } = &events[0] else {
        panic!("{events:?}")
    };
    let parsed = dbide_engine::plan::parse(payload, *format).unwrap();
    artifact(&database, "estimated-plan.json", payload);
    assert_eq!(parsed.statements[0].root.name, "Result");
    assert_eq!(parsed.statements[0].root.estimated_rows, 1.0);
    assert_eq!(parsed.statements[0].root.actual_rows, None);
    assert!(matches!(events[1], Event::Plan { .. }));
    assert!(matches!(events[2], Event::Done { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_unparseable_plan_request_reports_a_parse_error() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.plan("SELEC 1", PlanMode::Estimated).collect().await;
    assert!(
        matches!(&events[0], Event::Error { text, .. } if text.contains("Cannot parse PostgreSQL batch"))
    );
    assert!(matches!(events[1], Event::Done { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_danger_estimate_matches_analyzed_table_cardinality() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t(id int); INSERT INTO t SELECT generate_series(1,1000); ANALYZE t",
    )
    .await;
    let findings = danger::check("DELETE FROM t", Engine::PostgreSql);
    let estimates = postgres::estimate_rows(&mut session, "DELETE FROM t", &findings.statements)
        .await
        .unwrap();
    assert_eq!(estimates, vec![Some(1000.0)]);
    let rows: Vec<_> = session.run("SELECT count(*) FROM t").collect().await;
    assert_eq!(rows[1], Event::Rows(vec![vec![Value::Integer(1000)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_wrong_password_is_clear_and_redacted() {
    let database = database().await;
    let error = postgres::connect(&details(&database), Some("invented-wrong-password"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("password authentication failed"));
    assert!(!format!("{error:?}").contains("invented-wrong-password"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_windows_sign_in_is_explicitly_unsupported() {
    let database = database().await;
    let mut connection = details(&database);
    connection.sign_in = SignInMethod::Windows;
    let error = postgres::connect(&connection, None).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "PostgreSQL Windows sign-in is not supported yet"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_plan_errors_refer_to_the_original_batch_line() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .plan("SELECT 'é;elm';\r\n\r\nSELECT absent", PlanMode::Estimated)
        .collect()
        .await;
    assert!(matches!(events[0], Event::Plan { .. }));
    assert!(
        matches!(&events[1], Event::Error { line, code, .. } if *line == Some(3.try_into().unwrap()) && code.as_deref() == Some("42703"))
    );
    assert!(matches!(events[2], Event::Done { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_actual_plan_error_rolls_back_before_the_next_batch() {
    let database = database().await;
    let mut session = open(&database).await;
    let failed: Vec<_> = session
        .plan("SELECT 1 / 0", PlanMode::Actual)
        .collect()
        .await;
    assert!(matches!(&failed[0], Event::Error { code, .. } if code.as_deref() == Some("22012")));
    let recovered: Vec<_> = session.run("SELECT 8").collect().await;
    assert_eq!(recovered[1], Event::Rows(vec![vec![Value::Integer(8)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_cancelled_actual_plan_rolls_back_before_the_next_batch() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t(id int); INSERT INTO t VALUES (1)",
    )
    .await;
    let run = session.plan(
        "DELETE FROM t WHERE (SELECT count(*) FROM pg_sleep(30)) = 1",
        PlanMode::Actual,
    );
    let handle = run.cancel_handle();
    let cancellation = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.cancel();
    });
    let events: Vec<_> = tokio::time::timeout(Duration::from_secs(3), run.collect())
        .await
        .unwrap();
    cancellation.await.unwrap();
    assert!(
        matches!(&events[events.len()-2], Event::Error { text, .. } if text == "Run was cancelled")
    );
    let recovered: Vec<_> = tokio::time::timeout(
        Duration::from_secs(4),
        session.run("SELECT id FROM t").collect(),
    )
    .await
    .unwrap();
    assert_eq!(recovered[1], Event::Rows(vec![vec![Value::Integer(1)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_copy_protocol_failure_discards_the_connection_instead_of_hanging() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE t(id int)").await;
    let failed: Vec<_> = tokio::time::timeout(
        Duration::from_secs(3),
        session.run("COPY t FROM STDIN").collect(),
    )
    .await
    .unwrap();
    assert!(matches!(failed[0], Event::Error { .. }));
    assert!(
        matches!(&failed[1], Event::Error { text, .. } if text == "PostgreSQL connection is closed")
    );
    assert!(matches!(failed[2], Event::Done { .. }));
    let next: Vec<_> =
        tokio::time::timeout(Duration::from_secs(3), session.run("SELECT 1").collect())
            .await
            .unwrap();
    assert!(
        matches!(&next[0], Event::Error { text, .. } if text == "PostgreSQL connection is closed")
    );
    assert!(matches!(next[1], Event::Done { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_mandatory_tls_rejects_a_server_without_tls() {
    let database = database().await;
    let mut connection = details(&database);
    connection.tls = TlsMode::Mandatory;
    let error = postgres::connect(&connection, Some(database.password()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("TLS"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_optional_tls_allows_a_server_without_tls() {
    let database = database().await;
    let mut connection = details(&database);
    connection.tls = TlsMode::Optional;
    let mut session = postgres::connect(&connection, Some(database.password()))
        .await
        .unwrap();
    let events: Vec<_> = session.run("SELECT 3").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(3)]]));
}

async fn cancel_near_completion(session: &mut Postgres) -> Vec<Event> {
    let run = session.run("SELECT pg_sleep(0.02)");
    let handle = run.cancel_handle();
    let cancellation = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        handle.cancel();
    });
    let _: Vec<Event> = tokio::time::timeout(Duration::from_secs(3), run.collect())
        .await
        .unwrap();
    cancellation.await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(4),
        session.run("SELECT 19 FROM pg_sleep(0.05)").collect(),
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_cancellation_racing_completion_does_not_cancel_the_next_query() {
    let database = database().await;
    let mut session = open(&database).await;
    let first = cancel_near_completion(&mut session).await;
    assert_eq!(first[1], Event::Rows(vec![vec![Value::Integer(19)]]));
    let second = cancel_near_completion(&mut session).await;
    assert_eq!(second[1], Event::Rows(vec![vec![Value::Integer(19)]]));
    let third = cancel_near_completion(&mut session).await;
    assert_eq!(third[1], Event::Rows(vec![vec![Value::Integer(19)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_batch_on_a_terminated_session_reports_an_error() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT pg_backend_pid()").collect().await;
    let Event::Rows(rows) = &events[1] else {
        panic!("{events:?}")
    };
    let Value::Integer(pid) = rows[0][0] else {
        panic!("{rows:?}")
    };
    let mut killer = open(&database).await;
    setup(&mut killer, &format!("SELECT pg_terminate_backend({pid})")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let first: Vec<_> = session.run("SELECT 1").collect().await;
    let second: Vec<_> = session.run("SELECT 1").collect().await;
    assert!(
        first
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{first:?}"
    );
    assert!(
        second
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{second:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_escape_format_bytea_ff_converts_without_panicking() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("SET bytea_output = 'escape'; SELECT decode('ff', 'hex')")
        .collect()
        .await;
    assert!(
        events.contains(&Event::Rows(vec![vec![Value::Bytes(vec![0xff])]])),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_danger_estimate_of_a_delete_inside_a_cte_is_unknown() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE t (id int)").await;
    let batch = "WITH removed AS (DELETE FROM t RETURNING *) SELECT * FROM removed";
    let check = danger::check(batch, Engine::PostgreSql);
    let estimates = postgres::estimate_rows(&mut session, batch, &check.statements)
        .await
        .unwrap();
    assert_eq!(estimates, vec![None]);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_table_columns_report_base_table_and_column_before_rows() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE SCHEMA \"Shop\"; CREATE TABLE \"Shop\".\"Order\" (id int NOT NULL, note text); INSERT INTO \"Shop\".\"Order\" VALUES (1, NULL)").await;
    let events: Vec<_> = session
        .run("SELECT id AS alias, note FROM \"Shop\".\"Order\"")
        .collect()
        .await;
    assert_eq!(
        events[0],
        Event::ResultSetStarted {
            columns: vec![
                Column {
                    name: "alias".into(),
                    type_name: "int4".into(),
                    category: TypeCategory::Integer,
                    nullable: None,
                    base_table: Some("\"Shop\".\"Order\"".into()),
                    base_column: Some("id".into())
                },
                Column {
                    name: "note".into(),
                    type_name: "text".into(),
                    category: TypeCategory::Text,
                    nullable: None,
                    base_table: Some("\"Shop\".\"Order\"".into()),
                    base_column: Some("note".into())
                },
            ]
        }
    );
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![Value::Integer(1), Value::Null]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_expression_column_reports_no_base_table_column_or_nullability() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t (id int NOT NULL); INSERT INTO t VALUES (1)",
    )
    .await;
    let events: Vec<_> = session.run("SELECT id + 1 AS total FROM t").collect().await;
    assert_eq!(
        events[0],
        Event::ResultSetStarted {
            columns: vec![Column {
                name: "total".into(),
                type_name: "int4".into(),
                category: TypeCategory::Integer,
                nullable: None,
                base_table: None,
                base_column: None
            },]
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_join_columns_report_their_own_base_tables() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE a (id int NOT NULL); CREATE TABLE b (id int)",
    )
    .await;
    let events: Vec<_> = session
        .run("SELECT a.id, b.id FROM a JOIN b ON a.id = b.id")
        .collect()
        .await;
    assert_eq!(
        events[0],
        Event::ResultSetStarted {
            columns: vec![
                Column {
                    name: "id".into(),
                    type_name: "int4".into(),
                    category: TypeCategory::Integer,
                    nullable: None,
                    base_table: Some("public.a".into()),
                    base_column: Some("id".into())
                },
                Column {
                    name: "id".into(),
                    type_name: "int4".into(),
                    category: TypeCategory::Integer,
                    nullable: None,
                    base_table: Some("public.b".into()),
                    base_column: Some("id".into())
                },
            ]
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_temporary_table_columns_report_their_base_table() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TEMP TABLE local_rows (id int NOT NULL)",
    )
    .await;
    let events: Vec<_> = session.run("SELECT id FROM local_rows").collect().await;
    let Event::ResultSetStarted { columns } = &events[0] else {
        panic!("{events:?}")
    };
    let (schema, table) = columns[0]
        .base_table
        .as_deref()
        .unwrap()
        .split_once('.')
        .unwrap();
    // PostgreSQL assigns the temporary namespace's numeric suffix at connection time.
    assert!(schema.starts_with("pg_temp_"));
    assert_eq!(table, "local_rows");
    assert_eq!(columns[0].base_column.as_deref(), Some("id"));
    assert_eq!(columns[0].nullable, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_create_table_emits_no_row_count() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("CREATE TABLE t (id int)").collect().await;
    assert!(matches!(events[0], Event::Done { .. }));
    assert_eq!(events.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_update_of_three_rows_emits_row_count_three() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE t (id int); INSERT INTO t VALUES (1), (2), (3)",
    )
    .await;
    let events: Vec<_> = session.run("UPDATE t SET id = id + 1").collect().await;
    assert_eq!(events[0], Event::RowCount(3));
    assert!(matches!(events[1], Event::Done { .. }));
    assert_eq!(events.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_select_of_zero_rows_emits_row_count_zero() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT 1 WHERE false").collect().await;
    assert!(matches!(events[0], Event::ResultSetStarted { .. }));
    assert_eq!(events[1], Event::RowCount(0));
    assert!(matches!(events[2], Event::Done { .. }));
    assert_eq!(events.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_renamed_column_reports_its_new_name() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE t (id int)").await;
    let _: Vec<_> = session.run("SELECT id FROM t").collect().await;
    setup(&mut session, "ALTER TABLE t RENAME COLUMN id TO renamed").await;
    let events: Vec<_> = session.run("SELECT renamed FROM t").collect().await;
    let Event::ResultSetStarted { columns } = &events[0] else {
        panic!("{events:?}")
    };
    assert_eq!(columns[0].base_column.as_deref(), Some("renamed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_not_null_column_through_a_left_join_has_unknown_nullability() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE a (id int); CREATE TABLE b (id int NOT NULL); INSERT INTO a VALUES (1)",
    )
    .await;
    let events: Vec<_> = session
        .run("SELECT b.id FROM a LEFT JOIN b ON a.id = b.id")
        .collect()
        .await;
    let Event::ResultSetStarted { columns } = &events[0] else {
        panic!("{events:?}")
    };
    assert_eq!(columns[0].nullable, None);
    assert!(
        events.contains(&Event::Rows(vec![vec![Value::Null]])),
        "{events:?}"
    );
}
