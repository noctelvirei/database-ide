use dbide_engine::connections::Connection;
use dbide_engine::danger::Engine;
use dbide_engine::grid_edit::{self, Cell, Change, Conflict, Counts, Editability, EditableResult};
use dbide_engine::mssql::{self, Mssql};
use dbide_engine::session::{Column, Event, Session, Value};
use dbide_test_servers::mssql::Mssql as Database;
use futures::StreamExt;

async fn open(database: &Database) -> Mssql {
    let mut details = Connection::new(Engine::SqlServer, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.trust_server_certificate = true;
    mssql::connect(&details, Some(database.password()))
        .await
        .unwrap()
}

#[derive(Debug, Default)]
struct ResultSet {
    statement: String,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
}

async fn query(session: &mut Mssql, sql: &str) -> ResultSet {
    let mut result = ResultSet {
        statement: sql.into(),
        ..ResultSet::default()
    };
    let mut run = session.run(sql);
    while let Some(event) = run.next().await {
        match event {
            Event::ResultSetStarted { columns } => result.columns = columns,
            Event::Rows(rows) => result.rows.extend(rows),
            Event::Error { text, .. } => panic!("{text}"),
            _ => {}
        }
    }
    result
}

async fn inspect(session: &mut Mssql, result: &ResultSet) -> Editability {
    grid_edit::inspect(
        session,
        Engine::SqlServer,
        &result.statement,
        &result.columns,
    )
    .await
    .unwrap()
}

async fn editable(session: &mut Mssql, result: &ResultSet) -> EditableResult {
    match inspect(session, result).await {
        Editability::Editable(result) => result,
        Editability::ReadOnly(reason) => panic!("{reason}"),
    }
}

async fn fixture() -> (Database, Mssql) {
    let database = tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap();
    let mut session = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id int PRIMARY KEY, name nvarchar(40)); INSERT INTO fruit VALUES (1,N'apple'),(2,N'pear');").await;
    (database, session)
}

fn name(value: &str) -> Cell {
    Cell {
        column: 1,
        value: Value::Text(value.into()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_single_table_finds_base_columns_and_key_behind_aliases() {
    let (_database, mut session) = fixture().await;
    let result = query(
        &mut session,
        "SELECT f.id AS number, f.name AS label, 42 AS expression FROM fruit f ORDER BY id",
    )
    .await;
    let policy = editable(&mut session, &result).await;
    assert_eq!(policy.key_columns(), &[0]);
    assert_eq!(policy.columns()[0].name.as_deref(), Some("id"));
    assert_eq!(policy.columns()[1].name.as_deref(), Some("name"));
    assert!(!policy.columns()[1].read_only);
    assert!(policy.columns()[2].read_only);
    assert_eq!(policy.columns()[2].name, None);
    assert_eq!(
        query(&mut session, "SELECT COUNT(*) FROM fruit").await.rows,
        vec![vec![Value::Integer(2)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_edit_saves_unicode_on_the_tabs_session() {
    let (_database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("雪 O'Brien")],
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.rows,
        Counts {
            updated: 1,
            inserted: 0,
            deleted: 0
        }
    );
    assert_eq!(report.conflict, None);
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("雪 O'Brien".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_insert_saves_explicit_cells() {
    let (_database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Insert {
            cells: vec![
                Cell {
                    column: 0,
                    value: Value::Integer(3),
                },
                name("plum"),
            ],
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.rows,
        Counts {
            updated: 0,
            inserted: 1,
            deleted: 0
        }
    );
    assert_eq!(
        query(&mut session, "SELECT id, name FROM fruit WHERE id = 3")
            .await
            .rows,
        vec![vec![Value::Integer(3), Value::Text("plum".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_delete_saves_one_original_row() {
    let (_database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Delete {
            original: result.rows[0].clone(),
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.rows,
        Counts {
            updated: 0,
            inserted: 0,
            deleted: 1
        }
    );
    assert_eq!(
        query(&mut session, "SELECT id, name FROM fruit").await.rows,
        vec![vec![Value::Integer(2), Value::Text("pear".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_conflict_rolls_back_earlier_changes_and_preserves_other_session() {
    let (database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[
            Change::Update {
                original: result.rows[0].clone(),
                cells: vec![name("orange")],
            },
            Change::Update {
                original: result.rows[1].clone(),
                cells: vec![name("plum")],
            },
        ])
        .unwrap();
    let mut other = open(&database).await;
    query(&mut other, "UPDATE fruit SET name = N'banana' WHERE id = 2").await;
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.conflict,
        Some(Conflict {
            change: 1,
            affected: 0
        })
    );
    assert_eq!(report.rows, Counts::default());
    assert_eq!(
        query(&mut other, "SELECT id, name FROM fruit ORDER BY id")
            .await
            .rows,
        vec![
            vec![Value::Integer(1), Value::Text("apple".into())],
            vec![Value::Integer(2), Value::Text("banana".into())],
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_join_is_read_only_with_reason() {
    let (_database, mut session) = fixture().await;
    let result = query(
        &mut session,
        "SELECT a.id, b.name FROM fruit a JOIN fruit b ON a.id = b.id",
    )
    .await;
    assert!(
        matches!(inspect(&mut session, &result).await, Editability::ReadOnly(reason) if reason == "joins aren't editable")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_table_without_key_is_read_only_with_reason() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE TABLE heap (id int, name nvarchar(40))",
    )
    .await;
    let result = query(&mut session, "SELECT id, name FROM heap").await;
    assert!(
        matches!(inspect(&mut session, &result).await, Editability::ReadOnly(reason) if reason == "no key column in result")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_real_original_point_one_does_not_cause_false_conflict() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE measurement (id int PRIMARY KEY, reading real); INSERT INTO measurement VALUES (1,0.1)").await;
    let result = query(&mut session, "SELECT id, reading FROM measurement").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![Cell {
                column: 1,
                value: Value::Float(0.5),
            }],
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(report.conflict, None);
    assert_eq!(report.rows.updated, 1);
    assert_eq!(
        query(&mut session, "SELECT reading FROM measurement")
            .await
            .rows,
        vec![vec![Value::Float(0.5)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_missing_composite_key_part_is_not_supplied_by_hidden_metadata() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE TABLE pair (a int, b int, PRIMARY KEY(a,b))",
    )
    .await;
    let result = query(&mut session, "SELECT a FROM pair").await;
    assert!(
        matches!(inspect(&mut session, &result).await, Editability::ReadOnly(reason) if reason == "no key column in result")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_trigger_counts_and_nocount_do_not_change_save_counts() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE audit (id int)").await;
    query(&mut session, "CREATE TRIGGER fruit_audit ON fruit AFTER UPDATE AS BEGIN INSERT INTO audit VALUES(10),(20); SELECT id FROM audit; END").await;
    query(&mut session, "SET NOCOUNT ON").await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("orange")],
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(report.conflict, None);
    assert_eq!(report.rows.updated, 1);
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("orange".into())]]
    );
    assert_eq!(
        query(&mut session, "SELECT COUNT(*) FROM audit").await.rows,
        vec![vec![Value::Integer(2)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_save_leaves_user_transaction_uncommitted() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "BEGIN TRANSACTION").await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("orange")],
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert!(report.user_transaction_open);
    assert_eq!(
        query(&mut session, "SELECT @@TRANCOUNT").await.rows,
        vec![vec![Value::Integer(1)]]
    );
    query(&mut session, "ROLLBACK TRANSACTION").await;
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("apple".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_save_from_idle_commits_with_implicit_transactions_enabled() {
    let (_database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("orange")],
        }])
        .unwrap();
    query(&mut session, "SET IMPLICIT_TRANSACTIONS ON").await;
    let report = session.save(script).await.unwrap();
    assert!(!report.user_transaction_open);
    assert_eq!(report.rows.updated, 1);
    assert_eq!(
        query(&mut session, "SELECT @@TRANCOUNT").await.rows,
        vec![vec![Value::Integer(0)]]
    );
    query(&mut session, "SET IMPLICIT_TRANSACTIONS OFF").await;
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("orange".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_nonnull_unique_key_makes_a_table_editable() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE TABLE keyed (code int NOT NULL UNIQUE, name nvarchar(40))",
    )
    .await;
    let result = query(&mut session, "SELECT name, code FROM keyed").await;
    assert_eq!(editable(&mut session, &result).await.key_columns(), &[1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_varchar_original_uses_the_columns_utf8_code_page() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE national_text (id int PRIMARY KEY, name varchar(40) COLLATE Latin1_General_100_CI_AS_SC_UTF8); INSERT INTO national_text VALUES (1,N'雪')").await;
    let result = query(&mut session, "SELECT id, name FROM national_text").await;
    assert_eq!(
        result.rows,
        vec![vec![Value::Integer(1), Value::Text("雪".into())]]
    );
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("pear")],
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(report.conflict, None);
    assert_eq!(report.rows.updated, 1);
    assert_eq!(
        query(&mut session, "SELECT name FROM national_text")
            .await
            .rows,
        vec![vec![Value::Text("pear".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_view_is_read_only_even_when_browse_metadata_names_a_base_table() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE VIEW fruit_view AS SELECT id, name FROM dbo.fruit",
    )
    .await;
    let result = query(&mut session, "SELECT id, name FROM fruit_view").await;
    assert!(matches!(
        inspect(&mut session, &result).await,
        Editability::ReadOnly(_)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_delete_compares_native_values_and_omits_legacy_lob_and_xml() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE native_values (id uniqueidentifier PRIMARY KEY, single_value real, double_value float, clock datetime, cash money, bytes varbinary(8), legacy text, unicode_legacy ntext, picture image, document xml); INSERT INTO native_values VALUES ('00112233-4455-6677-8899-aabbccddeeff',0.1,0.1,'2024-02-29T12:34:56.003',12.3400,0x00ff,'old',N'old',0x00,N'<a/>')").await;
    let result = query(&mut session, "SELECT * FROM native_values").await;
    let policy = editable(&mut session, &result).await;
    assert!(!policy.columns()[6].comparable);
    assert!(!policy.columns()[7].comparable);
    assert!(!policy.columns()[8].comparable);
    assert!(!policy.columns()[9].comparable);
    let script = policy
        .script(&[Change::Delete {
            original: result.rows[0].clone(),
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(report.conflict, None);
    assert_eq!(report.rows.deleted, 1);
    assert_eq!(
        query(&mut session, "SELECT COUNT(*) FROM native_values")
            .await
            .rows,
        vec![vec![Value::Integer(0)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_default_insert_keeps_identity_computed_and_rowversion_read_only() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE defaults (id int IDENTITY PRIMARY KEY, name nvarchar(40) DEFAULT N'apple', doubled AS (id * 2), version rowversion)").await;
    let result = query(&mut session, "SELECT * FROM defaults").await;
    let policy = editable(&mut session, &result).await;
    assert!(policy.columns()[0].read_only);
    assert!(policy.columns()[2].read_only);
    assert!(policy.columns()[3].read_only);
    let report = session
        .save(policy.script(&[Change::Insert { cells: vec![] }]).unwrap())
        .await
        .unwrap();
    assert_eq!(report.rows.inserted, 1);
    assert_eq!(
        query(&mut session, "SELECT id, name, doubled FROM defaults")
            .await
            .rows,
        vec![vec![
            Value::Integer(1),
            Value::Text("apple".into()),
            Value::Integer(2)
        ]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_server_error_rolls_back_earlier_save_changes() {
    let (_database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[
            Change::Update {
                original: result.rows[0].clone(),
                cells: vec![name("orange")],
            },
            Change::Insert {
                cells: vec![
                    Cell {
                        column: 0,
                        value: Value::Integer(2),
                    },
                    name("plum"),
                ],
            },
        ])
        .unwrap();
    assert!(session.save(script).await.is_err());
    assert_eq!(
        query(&mut session, "SELECT id, name FROM fruit ORDER BY id")
            .await
            .rows,
        vec![
            vec![Value::Integer(1), Value::Text("apple".into())],
            vec![Value::Integer(2), Value::Text("pear".into())],
        ]
    );
    assert_eq!(
        query(&mut session, "SELECT @@TRANCOUNT").await.rows,
        vec![vec![Value::Integer(0)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_conflict_preserves_earlier_work_in_a_user_transaction() {
    let (_database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[
            Change::Update {
                original: result.rows[1].clone(),
                cells: vec![name("plum")],
            },
            Change::Update {
                original: result.rows[0].clone(),
                cells: vec![name("orange")],
            },
        ])
        .unwrap();
    query(
        &mut session,
        "BEGIN TRANSACTION; UPDATE fruit SET name = N'banana' WHERE id = 1",
    )
    .await;
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.conflict,
        Some(Conflict {
            change: 1,
            affected: 0
        })
    );
    assert!(report.user_transaction_open);
    assert_eq!(
        query(&mut session, "SELECT id, name FROM fruit ORDER BY id")
            .await
            .rows,
        vec![
            vec![Value::Integer(1), Value::Text("banana".into())],
            vec![Value::Integer(2), Value::Text("pear".into())],
        ]
    );
    assert_eq!(
        query(&mut session, "SELECT @@TRANCOUNT").await.rows,
        vec![vec![Value::Integer(1)]]
    );
    query(&mut session, "ROLLBACK TRANSACTION").await;
}
