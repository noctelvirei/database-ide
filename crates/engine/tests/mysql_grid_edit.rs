use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::grid_edit::{self, Cell, Change, Conflict, Counts, Editability, EditableResult};
use dbide_engine::mysql::{self, MySql};
use dbide_engine::session::{Column, Event, Session, Value};
use dbide_test_servers::mysql::MySql as Database;
use futures::StreamExt;
use std::time::Duration;

async fn open(database: &Database) -> MySql {
    let mut details = Connection::new(Engine::MySql, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.tls = TlsMode::Off;
    mysql::connect(&details, Some(database.password()))
        .await
        .unwrap()
}

#[derive(Debug, Default)]
struct ResultSet {
    statement: String,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
}

async fn query(session: &mut MySql, sql: &str) -> ResultSet {
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

async fn inspect(session: &mut MySql, result: &ResultSet) -> Editability {
    grid_edit::inspect(session, Engine::MySql, &result.statement, &result.columns)
        .await
        .unwrap()
}

async fn editable(session: &mut MySql, result: &ResultSet) -> EditableResult {
    match inspect(session, result).await {
        Editability::Editable(result) => result,
        Editability::ReadOnly(reason) => panic!("{reason}"),
    }
}

async fn fixture() -> (Database, MySql) {
    let database = tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap();
    let mut session = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id int PRIMARY KEY, name varchar(40) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci) ENGINE=InnoDB; INSERT INTO fruit VALUES (1,'apple'),(2,'pear');").await;
    (database, session)
}

fn name(value: &str) -> Cell {
    Cell {
        column: 1,
        value: Value::Text(value.into()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_saving_the_current_value_matches_one_row() {
    let (_database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit WHERE id = 1").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("apple")],
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(report.conflict, None);
    assert_eq!(report.rows.updated, 1);
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("apple".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_single_table_finds_base_columns_and_key_behind_aliases() {
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
async fn mysql_edit_saves_unicode_on_the_tabs_session() {
    let (database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("雪 O'Brien")],
        }])
        .unwrap();
    let folder = std::path::PathBuf::from(
        std::env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "../../target".into()),
    )
    .join("test-artifacts")
    .join(database.database());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("grid-save.sql"), script.sql()).unwrap();
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
async fn mysql_insert_saves_explicit_cells() {
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
async fn mysql_delete_saves_one_original_row() {
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
async fn mysql_conflict_rolls_back_earlier_changes_and_preserves_other_session() {
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
    query(&mut other, "UPDATE fruit SET name = 'banana' WHERE id = 2").await;
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
async fn mysql_case_only_change_conflicts_with_save() {
    let (database, mut session) = fixture().await;
    query(
        &mut session,
        "ALTER TABLE fruit MODIFY name varchar(40) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
    )
    .await;
    let result = query(&mut session, "SELECT id, name FROM fruit WHERE id = 1").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("orange")],
        }])
        .unwrap();
    let mut other = open(&database).await;
    query(&mut other, "UPDATE fruit SET name = 'APPLE' WHERE id = 1").await;
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.conflict,
        Some(Conflict {
            change: 0,
            affected: 0
        })
    );
    assert_eq!(
        report.rows,
        Counts {
            updated: 0,
            inserted: 0,
            deleted: 0
        }
    );
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("APPLE".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_trailing_space_change_conflicts_with_delete() {
    let (database, mut session) = fixture().await;
    query(&mut session, "ALTER TABLE fruit MODIFY name varchar(40) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci; UPDATE fruit SET name = 'a' WHERE id = 1").await;
    let result = query(&mut session, "SELECT id, name FROM fruit WHERE id = 1").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Delete {
            original: result.rows[0].clone(),
        }])
        .unwrap();
    let mut other = open(&database).await;
    query(&mut other, "UPDATE fruit SET name = 'a ' WHERE id = 1").await;
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.conflict,
        Some(Conflict {
            change: 0,
            affected: 0
        })
    );
    assert_eq!(
        report.rows,
        Counts {
            updated: 0,
            inserted: 0,
            deleted: 0
        }
    );
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("a ".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_dropping_blocked_save_restores_session_usability() {
    let (database, mut session) = fixture().await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let policy = editable(&mut session, &result).await;
    let script = policy
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
    let mut other = open(&database).await;
    query(
        &mut other,
        "START TRANSACTION; UPDATE fruit SET name = 'banana' WHERE id = 1",
    )
    .await;

    // timeout drops the polled Save future while the other session still holds its lock.
    assert!(
        tokio::time::timeout(Duration::from_secs(1), session.save(script))
            .await
            .is_err()
    );
    let reused =
        tokio::time::timeout(Duration::from_secs(5), query(&mut session, "SELECT 1")).await;
    // Release the lock even when reuse timed out, so a regression cannot hang teardown.
    query(&mut other, "ROLLBACK").await;
    assert_eq!(
        reused
            .expect("Save cancellation left the session blocked")
            .rows,
        vec![vec![Value::Integer(1)]]
    );
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit ORDER BY id")
            .await
            .rows,
        vec![
            vec![Value::Text("apple".into())],
            vec![Value::Text("pear".into())]
        ]
    );
    assert!(
        !session
            .save(policy.script(&[]).unwrap())
            .await
            .unwrap()
            .user_transaction_open
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_join_is_read_only_with_reason() {
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
async fn mysql_table_without_key_is_read_only_with_reason() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE heap (id int, name varchar(40))").await;
    let result = query(&mut session, "SELECT id, name FROM heap").await;
    assert!(
        matches!(inspect(&mut session, &result).await, Editability::ReadOnly(reason) if reason == "no key column in result")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_float_edits_are_refused_without_an_exact_original() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE measurement (id int PRIMARY KEY, reading float); INSERT INTO measurement VALUES (1,0.1)").await;
    let result = query(&mut session, "SELECT id, reading FROM measurement").await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![Cell {
                column: 1,
                value: Value::Float(0.5),
            }],
        }]);
    assert!(
        matches!(script, Err(grid_edit::Error::InvalidChange(reason))
        if reason == "edited column has no equality comparison")
    );
    assert_eq!(
        query(
            &mut session,
            "SELECT reading = CAST(0.1 AS FLOAT) FROM measurement"
        )
        .await
        .rows,
        vec![vec![Value::Integer(1)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_missing_composite_key_part_is_not_supplied_by_hidden_metadata() {
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
async fn mysql_save_leaves_user_transaction_uncommitted() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "START TRANSACTION").await;
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
    query(&mut session, "ROLLBACK").await;
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("apple".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_nonnull_unique_key_makes_a_table_editable() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE TABLE keyed (code int NOT NULL UNIQUE, name varchar(40))",
    )
    .await;
    let result = query(&mut session, "SELECT name, code FROM keyed").await;
    assert_eq!(editable(&mut session, &result).await.key_columns(), &[1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_nullable_unique_key_is_read_only() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE TABLE nullable_key (code int UNIQUE, name varchar(40))",
    )
    .await;
    let result = query(&mut session, "SELECT code, name FROM nullable_key").await;
    assert!(
        matches!(inspect(&mut session, &result).await, Editability::ReadOnly(reason) if reason == "no key column in result")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_view_is_read_only_even_when_it_reports_base_columns() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE VIEW fruit_view AS SELECT id, name FROM fruit",
    )
    .await;
    let result = query(&mut session, "SELECT id, name FROM fruit_view").await;
    assert!(matches!(
        inspect(&mut session, &result).await,
        Editability::ReadOnly(_)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_nontransactional_table_is_read_only() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE TABLE nontransactional (id int PRIMARY KEY) ENGINE=MyISAM",
    )
    .await;
    let result = query(&mut session, "SELECT id FROM nontransactional").await;
    assert!(
        matches!(inspect(&mut session, &result).await, Editability::ReadOnly(reason) if reason == "table does not support transactional Save")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_original_text_uses_the_columns_latin1_charset() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "ALTER TABLE fruit MODIFY name varchar(40) CHARACTER SET latin1; UPDATE fruit SET name = CONVERT(X'e9' USING latin1) WHERE id = 1").await;
    let result = query(&mut session, "SELECT id, name FROM fruit WHERE id = 1").await;
    assert_eq!(
        result.rows,
        vec![vec![Value::Integer(1), Value::Text("é".into())]]
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
        query(&mut session, "SELECT name FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Text("pear".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_delete_compares_long_text_and_blob_but_excludes_json() {
    let (_database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE documents (id int PRIMARY KEY AUTO_INCREMENT, body longtext, payload longblob, document json)").await;
    query(
        &mut session,
        "INSERT INTO documents (body, payload, document) VALUES ('apple', X'00ff', '{}')",
    )
    .await;
    let result = query(
        &mut session,
        "SELECT id, body, payload, document FROM documents",
    )
    .await;
    let policy = editable(&mut session, &result).await;
    assert!(policy.columns()[0].read_only);
    assert!(!policy.columns()[1].read_only);
    assert!(policy.columns()[1].comparable);
    assert!(policy.columns()[2].read_only);
    assert!(policy.columns()[2].comparable);
    assert!(!policy.columns()[3].comparable);
    query(
        &mut session,
        "UPDATE documents SET document = '[]' WHERE id = 1",
    )
    .await;
    let report = session
        .save(
            policy
                .script(&[Change::Delete {
                    original: result.rows[0].clone(),
                }])
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(report.conflict, None);
    assert_eq!(report.rows.deleted, 1);
    assert_eq!(
        query(&mut session, "SELECT id FROM documents").await.rows,
        Vec::<Vec<Value>>::new()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_conflict_preserves_earlier_work_in_a_user_transaction() {
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
        "START TRANSACTION; UPDATE fruit SET name = 'banana' WHERE id = 1",
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
    query(&mut session, "ROLLBACK").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_rounded_float_original_does_not_conflict_when_updating_another_column() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "ALTER TABLE fruit ADD reading FLOAT; UPDATE fruit SET reading = 1.23456789 WHERE id = 1",
    )
    .await;
    let result = query(
        &mut session,
        "SELECT id, name, reading FROM fruit WHERE id = 1",
    )
    .await;
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
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_rounded_float_original_does_not_conflict_with_delete() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "ALTER TABLE fruit ADD reading FLOAT; UPDATE fruit SET reading = 1.23456789 WHERE id = 1",
    )
    .await;
    let result = query(
        &mut session,
        "SELECT id, name, reading FROM fruit WHERE id = 1",
    )
    .await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Delete {
            original: result.rows[0].clone(),
        }])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(report.conflict, None);
    assert_eq!(report.rows.deleted, 1);
    assert_eq!(
        query(&mut session, "SELECT COUNT(*) FROM fruit WHERE id = 1")
            .await
            .rows,
        vec![vec![Value::Integer(0)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_float_only_key_is_read_only() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE TABLE approximate_key (id FLOAT PRIMARY KEY, name varchar(40))",
    )
    .await;
    let result = query(&mut session, "SELECT id, name FROM approximate_key").await;
    assert!(matches!(inspect(&mut session, &result).await,
        Editability::ReadOnly(reason) if reason == "no key column in result"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_exact_unique_key_is_used_instead_of_float_primary_key() {
    let (_database, mut session) = fixture().await;
    query(
        &mut session,
        "CREATE TABLE alternate_key (id FLOAT PRIMARY KEY, code int NOT NULL UNIQUE)",
    )
    .await;
    let result = query(&mut session, "SELECT id, code FROM alternate_key").await;
    assert_eq!(editable(&mut session, &result).await.key_columns(), &[1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_text_key_save_does_not_wait_for_an_unrelated_row_lock() {
    let (database, mut session) = fixture().await;
    query(&mut session, "CREATE TABLE text_key (code varchar(40) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci PRIMARY KEY, name varchar(40)) ENGINE=InnoDB; INSERT INTO text_key VALUES ('a', 'apple'), ('z', 'pear')").await;
    let result = query(
        &mut session,
        "SELECT code, name FROM text_key WHERE code = 'z'",
    )
    .await;
    let script = editable(&mut session, &result)
        .await
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![name("orange")],
        }])
        .unwrap();
    let mut other = open(&database).await;
    // Lock the first index entry: a scan for 'z' would block before reaching its row.
    query(
        &mut other,
        "START TRANSACTION; SELECT code FROM text_key WHERE code = 'a' FOR UPDATE",
    )
    .await;
    let saved = tokio::time::timeout(Duration::from_secs(3), session.save(script)).await;
    // Release before asserting so a regression cannot leave a blocked worker in teardown.
    query(&mut other, "ROLLBACK").await;
    let report = saved
        .expect("Save scanned an unrelated locked row")
        .unwrap();
    assert_eq!(report.conflict, None);
    assert_eq!(report.rows.updated, 1);
    assert_eq!(
        query(
            &mut session,
            "SELECT code, name FROM text_key ORDER BY code"
        )
        .await
        .rows,
        vec![
            vec![Value::Text("a".into()), Value::Text("apple".into())],
            vec![Value::Text("z".into()), Value::Text("orange".into())]
        ]
    );
}
