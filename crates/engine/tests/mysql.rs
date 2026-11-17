use std::time::Duration;

use dbide_engine::connections::{Connection, SignInMethod, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::mysql::{self, MySql};
use dbide_engine::session::temporal::{Date, Time, Timestamp};
use dbide_engine::session::{Column, Event, PlanMode, Session, Severity, TypeCategory, Value};
use dbide_test_servers::mysql::MySql as Database;
use futures::StreamExt;

fn details(database: &Database) -> Connection {
    let mut connection = Connection::new(Engine::MySql, database.host());
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

async fn open(database: &Database) -> MySql {
    mysql::connect(&details(database), Some(database.password()))
        .await
        .unwrap()
}

async fn setup(session: &mut MySql, sql: &str) {
    let events: Vec<_> = session.run(sql).collect().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_select_streams_typed_base_columns_rows_count_then_done() {
    let database = database().await;
    let mut session = open(&database).await;
    assert_eq!(session.current_database(), database.database());
    setup(&mut session, "CREATE TABLE trees (id int NOT NULL, name varchar(20)); INSERT INTO trees VALUES (7, 'elm')").await;
    let events: Vec<_> = session
        .run("SELECT id AS answer, name FROM trees")
        .collect()
        .await;
    assert_eq!(
        events[0],
        Event::ResultSetStarted {
            columns: vec![
                Column {
                    name: "answer".into(),
                    type_name: "int".into(),
                    category: TypeCategory::Integer,
                    nullable: Some(false),
                    base_table: Some(format!("{}.trees", database.database())),
                    base_column: Some("id".into()),
                },
                Column {
                    name: "name".into(),
                    type_name: "varchar".into(),
                    category: TypeCategory::Text,
                    nullable: Some(true),
                    base_table: Some(format!("{}.trees", database.database())),
                    base_column: Some("name".into()),
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
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let folder = root.join("test-artifacts").join(database.database());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("mysql-events.txt"), format!("{events:#?}")).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_expression_column_has_no_base_table_or_column() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT 2 + 3 AS total").collect().await;
    let Event::ResultSetStarted { columns } = &events[0] else {
        panic!("{events:?}")
    };
    assert_eq!(columns[0].name, "total");
    assert_eq!(columns[0].base_table, None);
    assert_eq!(columns[0].base_column, None);
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(5)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_fixture_loads_into_the_fresh_database() {
    let database = database().await;
    let database = tokio::task::spawn_blocking(move || {
        database.load_script("../../testdata/mysql.sql").unwrap();
        database
    })
    .await
    .unwrap();
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .run("SELECT COUNT(*) FROM customers")
        .collect()
        .await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(40)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_null_decimal_unsigned_blob_and_json_keep_their_values() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE cells (missing text, exact decimal(30,3), whole bigint unsigned, payload blob, document json); INSERT INTO cells VALUES (NULL, 12345678901234567890.125, 18446744073709551615, X'00ff5c', '{\"key\": 2}')").await;
    let events: Vec<_> = session.run("SELECT * FROM cells").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Null,
            Value::Decimal("12345678901234567890.125".into()),
            Value::UnsignedInteger(u64::MAX),
            Value::Bytes(vec![0, 255, 92]),
            Value::Json("{\"key\": 2}".into()),
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_temporal_columns_convert_to_iso_values() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "SET time_zone = '+00:00'; CREATE TABLE moments (day date, clock time(6), local_time datetime(6), instant timestamp(6)); INSERT INTO moments VALUES ('2024-02-29', '12:34:56.123456', '2024-02-29 12:34:56.123456', '2024-02-29 12:34:56.123456')").await;
    let events: Vec<_> = session.run("SELECT * FROM moments").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Date(Date::parse("2024-02-29").unwrap()),
            Value::Time(Time::parse("12:34:56.123456").unwrap()),
            Value::Timestamp(Timestamp::parse("2024-02-29T12:34:56.123456").unwrap()),
            Value::Timestamp(Timestamp::parse("2024-02-29T12:34:56.123456").unwrap()),
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_signed_integer_float_text_and_bit_keep_their_values() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(&mut session, "CREATE TABLE cells (whole bigint, fraction double, label varchar(20), bits bit(8)); INSERT INTO cells VALUES (-9223372036854775808, 1.25, '雪é', b'10100101')").await;
    let events: Vec<_> = session.run("SELECT * FROM cells").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![
            Value::Integer(i64::MIN),
            Value::Float(1.25),
            Value::Text("雪é".into()),
            Value::Bytes(vec![165]),
        ]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_two_selects_keep_separate_result_sets() {
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
async fn mysql_update_reports_three_affected_rows() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE trees (id int); INSERT INTO trees VALUES (1),(2),(3)",
    )
    .await;
    let events: Vec<_> = session.run("UPDATE trees SET id = id + 1").collect().await;
    assert_eq!(events[0], Event::RowCount(3));
    assert!(matches!(events[1], Event::Done { .. }));
    assert_eq!(events.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_zero_match_cte_update_reports_zero_rows() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE trees (id int); INSERT INTO trees VALUES (1)",
    )
    .await;
    let events: Vec<_> = session.run("WITH ids AS (SELECT 1 AS id) UPDATE trees JOIN ids USING (id) SET trees.id = 2 WHERE false").collect().await;
    assert_eq!(events[0], Event::RowCount(0));
    assert!(matches!(events[1], Event::Done { .. }));
    assert_eq!(events.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_next_batch_can_read_the_previous_affected_row_count() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "CREATE TABLE trees (id int); INSERT INTO trees VALUES (1),(2),(3)",
    )
    .await;
    setup(&mut session, "UPDATE trees SET id = id + 1").await;
    let events: Vec<_> = session.run("SELECT ROW_COUNT()").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(3)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_empty_select_reports_zero_rows() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT 1 WHERE false").collect().await;
    assert!(matches!(events[0], Event::ResultSetStarted { .. }));
    assert_eq!(events[1], Event::RowCount(0));
    assert!(matches!(events[2], Event::Done { .. }));
    assert_eq!(events.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_truncation_warning_arrives_with_warning_severity() {
    let database = database().await;
    let mut session = open(&database).await;
    setup(
        &mut session,
        "SET SESSION sql_mode = ''; CREATE TABLE trees (name varchar(3))",
    )
    .await;
    let events: Vec<_> = session
        .run("INSERT INTO trees VALUES ('willow')")
        .collect()
        .await;
    assert!(events.iter().any(|event| matches!(event, Event::Message { text, severity: Severity::Warning, .. } if text.contains("Data truncated for column 'name' at row 1"))), "{events:?}");
    assert!(events.contains(&Event::RowCount(1)), "{events:?}");
    let rows: Vec<_> = session.run("SELECT name FROM trees").collect().await;
    assert_eq!(rows[1], Event::Rows(vec![vec![Value::Text("wil".into())]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_syntax_error_reports_native_code_and_batch_line_three() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT 'é;elm';\n\nSELEC 2").collect().await;
    assert!(events.iter().any(|event| matches!(event, Event::Error { text, line, code } if text.contains("syntax") && *line == Some(3.try_into().unwrap()) && code.as_deref() == Some("1064"))), "{events:?}");
    assert!(matches!(events.last(), Some(Event::Done { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_session_remains_usable_after_server_error() {
    let database = database().await;
    let mut session = open(&database).await;
    let failed: Vec<_> = session.run("SELECT * FROM absent_table").collect().await;
    assert!(matches!(&failed[0], Event::Error { code, .. } if code.as_deref() == Some("1146")));
    let events: Vec<_> = session.run("SELECT 9").collect().await;
    assert_eq!(events[1], Event::Rows(vec![vec![Value::Integer(9)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_cancel_sleep_finishes_promptly_and_session_recovers() {
    let database = database().await;
    let mut session = open(&database).await;
    let run = session.run("SELECT SLEEP(30)");
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
async fn mysql_cancel_mid_stream_stops_large_table_rows() {
    let database = database().await;
    let database = tokio::task::spawn_blocking(move || {
        database
            .load_script("../../testdata/mysql-large.sql")
            .unwrap();
        database
    })
    .await
    .unwrap();
    let mut session = open(&database).await;
    let mut run = session.run("SELECT id, payload FROM big_rows");
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
async fn mysql_dropping_pending_run_cancels_before_the_next_batch() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut run = session.run("SELECT SLEEP(30)");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), run.by_ref().collect::<Vec<_>>())
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
async fn mysql_query_timeout_stops_sleep_and_session_recovers() {
    let database = database().await;
    let mut connection = details(&database);
    connection.query_timeout = 1;
    let mut session = mysql::connect(&connection, Some(database.password()))
        .await
        .unwrap();
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(4),
        session.run("SELECT SLEEP(30)").collect(),
    )
    .await
    .unwrap();
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Error { text, .. } if text == "MySQL query timed out")
        ),
        "{events:?}"
    );
    assert!(matches!(events.last(), Some(Event::Done { .. })));
    let recovered: Vec<_> =
        tokio::time::timeout(Duration::from_secs(3), session.run("SELECT 11").collect())
            .await
            .unwrap();
    assert_eq!(recovered[1], Event::Rows(vec![vec![Value::Integer(11)]]));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_use_updates_the_current_database() {
    let original = database().await;
    let destination = database().await;
    assert_eq!(original.port(), destination.port());
    let mut session = open(&original).await;
    setup(&mut session, &format!("USE `{}`", destination.database())).await;
    assert_eq!(session.current_database(), destination.database());
    let events: Vec<_> = session.run("SELECT DATABASE()").collect().await;
    assert_eq!(
        events[1],
        Event::Rows(vec![vec![Value::Text(destination.database().into())]])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_plan_reports_unsupported_without_running_the_batch() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session
        .plan("CREATE TABLE should_not_exist (id int)", PlanMode::Actual)
        .collect()
        .await;
    assert!(
        matches!(&events[0], Event::Error { text, .. } if text.contains("not supported for MySQL"))
    );
    assert!(matches!(events[1], Event::Done { .. }));
    assert_eq!(events.len(), 2);
    let rows: Vec<_> = session
        .run("SHOW TABLES LIKE 'should_not_exist'")
        .collect()
        .await;
    assert_eq!(rows[1], Event::RowCount(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_wrong_password_is_clear_and_redacted() {
    let database = database().await;
    let error = mysql::connect(&details(&database), Some("invented-wrong-password"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Access denied"), "{error}");
    assert!(!format!("{error:?}").contains("invented-wrong-password"));
    assert!(!error.to_string().contains("invented-wrong-password"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_windows_sign_in_is_explicitly_unsupported() {
    let database = database().await;
    let mut connection = details(&database);
    connection.sign_in = SignInMethod::Windows;
    let error = mysql::connect(&connection, None).await.unwrap_err();
    assert!(error.to_string().contains("not supported"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_runs_after_a_killed_connection_report_errors() {
    let database = database().await;
    let mut session = open(&database).await;
    let events: Vec<_> = session.run("SELECT CONNECTION_ID()").collect().await;
    let Event::Rows(rows) = &events[1] else {
        panic!("{events:?}")
    };
    let id = match rows[0][0] {
        Value::UnsignedInteger(id) => id,
        Value::Integer(id) => id.try_into().unwrap(),
        ref value => panic!("{value:?}"),
    };
    let mut killer = open(&database).await;
    setup(&mut killer, &format!("KILL CONNECTION {id}")).await;
    let first: Vec<_> =
        tokio::time::timeout(Duration::from_secs(3), session.run("SELECT 1").collect())
            .await
            .unwrap();
    let second: Vec<_> =
        tokio::time::timeout(Duration::from_secs(3), session.run("SELECT 1").collect())
            .await
            .unwrap();
    assert!(
        first
            .iter()
            .any(|event| matches!(event, Event::Error { .. })),
        "{first:?}"
    );
    assert!(second.iter().any(|event| matches!(event, Event::Error { text, .. } if text.contains("connection is closed"))), "{second:?}");
    assert!(matches!(second.last(), Some(Event::Done { .. })));
}
