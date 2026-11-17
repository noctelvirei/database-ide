use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::grid_edit::{
    self, Cell, Change, Conflict, Counts, Editability, EditableResult, Error,
};
use dbide_engine::postgres::{self, Postgres};
use dbide_engine::session::{Column, Event, Session, Value};
use dbide_test_servers::postgres::Postgres as Database;
use futures::StreamExt;

async fn database() -> Database {
    tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap()
}

async fn open(database: &Database) -> Postgres {
    let mut details = Connection::new(Engine::PostgreSql, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.tls = TlsMode::Off;
    postgres::connect(&details, Some(database.password()))
        .await
        .unwrap()
}

#[derive(Debug, Default)]
struct ResultSet {
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
}

async fn query(session: &mut Postgres, sql: &str) -> ResultSet {
    let mut run = session.run(sql);
    let mut result = ResultSet::default();
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

async fn editable(session: &mut Postgres, result: &ResultSet) -> EditableResult {
    match grid_edit::inspect(session, Engine::PostgreSql, &result.columns)
        .await
        .unwrap()
    {
        Editability::Editable(result) => result,
        Editability::ReadOnly(reason) => panic!("{reason}"),
    }
}

fn text_cell(column: usize, value: &str) -> Cell {
    Cell {
        column,
        value: Value::Text(value.into()),
    }
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
async fn pg_single_table_result_with_primary_key_is_editable() {
    let database = database().await;
    let mut session = open(&database).await;
    query(
        &mut session,
        "CREATE TABLE fruit (id integer PRIMARY KEY, name text)",
    )
    .await;
    let result = query(&mut session, "SELECT id AS number, name FROM fruit").await;
    let result = editable(&mut session, &result).await;
    assert_eq!(result.key_columns(), &[0]);
    assert!(!result.columns()[0].read_only);
    assert!(!result.columns()[1].read_only);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_result_without_a_key_column_says_no_key_column_in_result() {
    let database = database().await;
    let mut session = open(&database).await;
    query(
        &mut session,
        "CREATE TABLE fruit (id integer PRIMARY KEY, name text)",
    )
    .await;
    let result = query(&mut session, "SELECT name FROM fruit").await;
    let policy = grid_edit::inspect(&mut session, Engine::PostgreSql, &result.columns)
        .await
        .unwrap();
    assert!(matches!(policy, Editability::ReadOnly(reason) if reason == "no key column in result"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_join_with_columns_from_two_tables_says_columns_from_2_tables() {
    let database = database().await;
    let mut session = open(&database).await;
    query(
        &mut session,
        "CREATE TABLE fruit (id integer PRIMARY KEY); CREATE TABLE grove (id integer PRIMARY KEY)",
    )
    .await;
    let result = query(
        &mut session,
        "SELECT f.id, g.id FROM fruit f JOIN grove g ON f.id = g.id",
    )
    .await;
    let policy = grid_edit::inspect(&mut session, Engine::PostgreSql, &result.columns)
        .await
        .unwrap();
    assert!(matches!(policy, Editability::ReadOnly(reason) if reason == "columns from 2 tables"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_expression_columns_only_says_expression_columns_only() {
    let database = database().await;
    let mut session = open(&database).await;
    let result = query(&mut session, "SELECT 1 AS number").await;
    let policy = grid_edit::inspect(&mut session, Engine::PostgreSql, &result.columns)
        .await
        .unwrap();
    assert!(matches!(policy, Editability::ReadOnly(reason) if reason == "expression columns only"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_expression_alongside_key_stays_read_only_without_refusing_result() {
    let database = database().await;
    let mut session = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id integer PRIMARY KEY)").await;
    let result = query(&mut session, "SELECT id, id + 1 AS next_id FROM fruit").await;
    let result = editable(&mut session, &result).await;
    assert!(!result.columns()[0].read_only);
    assert!(result.columns()[1].read_only);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_save_changes_exactly_the_edited_added_and_deleted_rows() {
    let database = database().await;
    let mut session = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id integer PRIMARY KEY, name text DEFAULT 'pear', document json); INSERT INTO fruit VALUES (1, 'apple', '{}'), (2, 'plum', '{}'), (4, 'lime', '{}')").await;
    let result = query(
        &mut session,
        "SELECT id, name, document FROM fruit ORDER BY id",
    )
    .await;
    let policy = editable(&mut session, &result).await;
    assert!(!policy.columns()[2].comparable);
    let script = policy
        .script(&[
            Change::Update {
                original: result.rows[0].clone(),
                cells: vec![text_cell(1, "grape")],
            },
            Change::Insert {
                cells: vec![Cell {
                    column: 0,
                    value: Value::Integer(3),
                }],
            },
            Change::Delete {
                original: result.rows[1].clone(),
            },
        ])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE \"public\".\"fruit\" SET \"name\" = E'grape' WHERE \"id\" = 1 AND \"name\" = E'apple';\nINSERT INTO \"public\".\"fruit\" (\"id\") VALUES (3);\nDELETE FROM \"public\".\"fruit\" WHERE \"id\" = 2 AND \"name\" = E'plum';\n"
    );
    artifact(&database, "save.sql", script.sql());
    let report = session.save(script).await.unwrap();
    let after = query(&mut session, "SELECT id, name FROM fruit ORDER BY id")
        .await
        .rows;
    artifact(&database, "save.txt", &format!("{report:#?}\n{after:#?}\n"));
    assert_eq!(
        report.rows,
        Counts {
            updated: 1,
            inserted: 1,
            deleted: 1
        }
    );
    assert_eq!(report.conflict, None);
    assert!(!report.user_transaction_open);
    assert_eq!(
        after,
        vec![
            vec![Value::Integer(1), Value::Text("grape".into())],
            vec![Value::Integer(3), Value::Text("pear".into())],
            vec![Value::Integer(4), Value::Text("lime".into())]
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_conflict_rolls_back_every_pending_change() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut other = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id integer PRIMARY KEY, name text); INSERT INTO fruit VALUES (1, 'apple'), (2, 'plum')").await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let policy = editable(&mut session, &result).await;
    query(&mut other, "UPDATE fruit SET name = 'lime' WHERE id = 2").await;
    let script = policy
        .script(&[
            Change::Update {
                original: result.rows[0].clone(),
                cells: vec![text_cell(1, "grape")],
            },
            Change::Insert {
                cells: vec![Cell {
                    column: 0,
                    value: Value::Integer(3),
                }],
            },
            Change::Delete {
                original: result.rows[1].clone(),
            },
        ])
        .unwrap();
    artifact(&database, "conflict.sql", script.sql());
    let report = session.save(script).await.unwrap();
    let after = query(&mut session, "SELECT id, name FROM fruit ORDER BY id")
        .await
        .rows;
    artifact(
        &database,
        "conflict.txt",
        &format!("{report:#?}\n{after:#?}\n"),
    );
    assert_eq!(
        report.conflict,
        Some(Conflict {
            change: 2,
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
    assert!(!report.user_transaction_open);
    assert_eq!(
        after,
        vec![
            vec![Value::Integer(1), Value::Text("apple".into())],
            vec![Value::Integer(2), Value::Text("lime".into())]
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_save_in_user_transaction_stays_uncommitted_and_rollback_removes_it() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut other = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id integer PRIMARY KEY, name text); INSERT INTO fruit VALUES (1, 'apple')").await;
    let result = query(&mut session, "SELECT id, name FROM fruit").await;
    let policy = editable(&mut session, &result).await;
    query(&mut session, "BEGIN").await;
    let script = policy
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![text_cell(1, "pear")],
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
    assert!(report.user_transaction_open);
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit").await.rows,
        vec![vec![Value::Text("pear".into())]]
    );
    assert_eq!(
        query(&mut other, "SELECT name FROM fruit").await.rows,
        vec![vec![Value::Text("apple".into())]]
    );
    query(&mut session, "ROLLBACK").await;
    assert_eq!(
        query(&mut session, "SELECT name FROM fruit").await.rows,
        vec![vec![Value::Text("apple".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_null_originals_match_during_update_and_delete() {
    let database = database().await;
    let mut session = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id integer PRIMARY KEY, name text); INSERT INTO fruit VALUES (1, NULL), (2, NULL)").await;
    let result = query(&mut session, "SELECT id, name FROM fruit ORDER BY id").await;
    let policy = editable(&mut session, &result).await;
    let script = policy
        .script(&[
            Change::Update {
                original: result.rows[0].clone(),
                cells: vec![text_cell(1, "pear")],
            },
            Change::Delete {
                original: result.rows[1].clone(),
            },
        ])
        .unwrap();
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.rows,
        Counts {
            updated: 1,
            inserted: 0,
            deleted: 1
        }
    );
    assert_eq!(
        query(&mut session, "SELECT id, name FROM fruit").await.rows,
        vec![vec![Value::Integer(1), Value::Text("pear".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_identity_column_is_read_only() {
    let database = database().await;
    let mut session = open(&database).await;
    query(
        &mut session,
        "CREATE TABLE fruit (id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY, name text)",
    )
    .await;
    let result = query(&mut session, "SELECT id, name FROM fruit").await;
    let policy = editable(&mut session, &result).await;
    assert!(policy.columns()[0].read_only);
    assert!(!policy.columns()[1].read_only);
    assert!(matches!(
        policy.script(&[Change::Insert {
            cells: vec![Cell {
                column: 0,
                value: Value::Integer(7)
            }]
        }]),
        Err(Error::InvalidChange(_))
    ));
    let report = session
        .save(
            policy
                .script(&[Change::Insert {
                    cells: vec![text_cell(1, "pear")],
                }])
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(report.rows.inserted, 1);
    assert_eq!(
        query(&mut session, "SELECT id, name FROM fruit").await.rows,
        vec![vec![Value::Integer(1), Value::Text("pear".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_complete_nonnull_unique_key_is_editable() {
    let database = database().await;
    let mut session = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (grove integer NOT NULL, number integer NOT NULL, name text, UNIQUE(grove, number))").await;
    let result = query(&mut session, "SELECT name, number, grove FROM fruit").await;
    assert_eq!(editable(&mut session, &result).await.key_columns(), &[2, 1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_nullable_unique_key_is_not_an_editable_result() {
    let database = database().await;
    let mut session = open(&database).await;
    query(
        &mut session,
        "CREATE TABLE fruit (id integer UNIQUE, name text)",
    )
    .await;
    let result = query(&mut session, "SELECT id, name FROM fruit").await;
    assert!(
        matches!(grid_edit::inspect(&mut session, Engine::PostgreSql, &result.columns).await.unwrap(),
        Editability::ReadOnly(reason) if reason == "no key column in result")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_generated_and_binary_columns_are_read_only() {
    let database = database().await;
    let mut session = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id integer PRIMARY KEY, doubled integer GENERATED ALWAYS AS (id * 2) STORED, picture bytea)").await;
    let result = query(&mut session, "SELECT id, doubled, picture FROM fruit").await;
    let policy = editable(&mut session, &result).await;
    assert!(policy.columns()[1].read_only);
    assert!(policy.columns()[2].read_only);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_conflict_in_user_transaction_preserves_earlier_user_changes() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut other = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id integer PRIMARY KEY, name text); INSERT INTO fruit VALUES (1, 'apple')").await;
    let result = query(&mut session, "SELECT id, name FROM fruit").await;
    let policy = editable(&mut session, &result).await;
    query(&mut other, "UPDATE fruit SET name = 'lime' WHERE id = 1").await;
    query(&mut session, "BEGIN; INSERT INTO fruit VALUES (2, 'plum')").await;
    let report = session
        .save(
            policy
                .script(&[
                    Change::Insert {
                        cells: vec![Cell {
                            column: 0,
                            value: Value::Integer(3),
                        }],
                    },
                    Change::Update {
                        original: result.rows[0].clone(),
                        cells: vec![text_cell(1, "pear")],
                    },
                ])
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        report.conflict,
        Some(Conflict {
            change: 1,
            affected: 0
        })
    );
    assert!(report.user_transaction_open);
    query(&mut session, "COMMIT").await;
    assert_eq!(
        query(&mut other, "SELECT id, name FROM fruit ORDER BY id")
            .await
            .rows,
        vec![
            vec![Value::Integer(1), Value::Text("lime".into())],
            vec![Value::Integer(2), Value::Text("plum".into())]
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_server_error_rolls_back_save_and_keeps_user_transaction_usable() {
    let database = database().await;
    let mut session = open(&database).await;
    query(
        &mut session,
        "CREATE TABLE fruit (id integer PRIMARY KEY); INSERT INTO fruit VALUES (1)",
    )
    .await;
    let result = query(&mut session, "SELECT id FROM fruit").await;
    let policy = editable(&mut session, &result).await;
    query(&mut session, "BEGIN; INSERT INTO fruit VALUES (2)").await;
    let error = session
        .save(
            policy
                .script(&[
                    Change::Insert {
                        cells: vec![Cell {
                            column: 0,
                            value: Value::Integer(3),
                        }],
                    },
                    Change::Insert {
                        cells: vec![Cell {
                            column: 0,
                            value: Value::Integer(1),
                        }],
                    },
                ])
                .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Server { code: Some(code), change: Some(1), .. } if code == "23505")
    );
    query(&mut session, "COMMIT").await;
    assert_eq!(
        query(&mut session, "SELECT id FROM fruit ORDER BY id")
            .await
            .rows,
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_varchar_original_changed_by_another_session_conflicts_on_delete() {
    let database = database().await;
    let mut session = open(&database).await;
    let mut other = open(&database).await;
    query(&mut session, "CREATE TABLE fruit (id integer PRIMARY KEY, name varchar(50)); INSERT INTO fruit VALUES (1, 'apple')").await;
    let result = query(&mut session, "SELECT id, name FROM fruit").await;
    let policy = editable(&mut session, &result).await;
    assert!(policy.columns()[1].comparable);
    query(&mut other, "UPDATE fruit SET name = 'pear' WHERE id = 1").await;
    let script = policy
        .script(&[Change::Delete {
            original: result.rows[0].clone(),
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "DELETE FROM \"public\".\"fruit\" WHERE \"id\" = 1 AND \"name\" = E'apple';\n"
    );
    let report = session.save(script).await.unwrap();
    assert_eq!(
        report.conflict,
        Some(Conflict {
            change: 0,
            affected: 0
        })
    );
    assert_eq!(
        query(&mut session, "SELECT id, name FROM fruit").await.rows,
        vec![vec![Value::Integer(1), Value::Text("pear".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_varchar_primary_key_is_editable() {
    let database = database().await;
    let mut session = open(&database).await;
    query(
        &mut session,
        "CREATE TABLE fruit (id varchar(50) PRIMARY KEY, name text)",
    )
    .await;
    let result = query(&mut session, "SELECT id, name FROM fruit").await;
    assert_eq!(editable(&mut session, &result).await.key_columns(), &[0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_domains_preserve_underlying_binary_and_comparison_policy() {
    let database = database().await;
    let mut session = open(&database).await;
    query(&mut session, "CREATE DOMAIN picture AS bytea; CREATE DOMAIN document AS json; CREATE TABLE fruit (id integer PRIMARY KEY, picture picture, document document, names varchar(50)[])").await;
    let result = query(
        &mut session,
        "SELECT id, picture, document, names FROM fruit",
    )
    .await;
    let policy = editable(&mut session, &result).await;
    assert!(policy.columns()[1].read_only);
    assert!(policy.columns()[1].comparable);
    assert!(!policy.columns()[2].comparable);
    assert!(policy.columns()[3].comparable);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_quoted_table_names_with_dots_are_saved_without_changing_the_target() {
    let database = database().await;
    let mut session = open(&database).await;
    query(&mut session, "CREATE TABLE public.\"fruit.box\" (\"key\" integer PRIMARY KEY, \"Fruit Name\" text); INSERT INTO public.\"fruit.box\" VALUES (1, 'apple')").await;
    let result = query(
        &mut session,
        "SELECT \"key\", \"Fruit Name\" FROM public.\"fruit.box\"",
    )
    .await;
    let policy = editable(&mut session, &result).await;
    let script = policy
        .script(&[Change::Update {
            original: result.rows[0].clone(),
            cells: vec![text_cell(1, "O'Brien")],
        }])
        .unwrap();
    assert_eq!(
        script.sql(),
        "UPDATE \"public\".\"fruit.box\" SET \"Fruit Name\" = E'O''Brien' WHERE \"key\" = 1 AND \"Fruit Name\" = E'apple';\n"
    );
    assert_eq!(session.save(script).await.unwrap().rows.updated, 1);
    assert_eq!(
        query(
            &mut session,
            "SELECT \"Fruit Name\" FROM public.\"fruit.box\""
        )
        .await
        .rows,
        vec![vec![Value::Text("O'Brien".into())]]
    );
}
