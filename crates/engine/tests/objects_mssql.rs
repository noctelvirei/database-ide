use dbide_engine::connections::Connection;
use dbide_engine::danger::Engine;
use dbide_engine::mssql::{self, Mssql};
use dbide_engine::objects::mssql as objects;
use dbide_engine::objects::{ConstraintKind, Generated, Identity, Kind, Object, Script, Search};
use dbide_engine::session::{Event, Session, Value};
use dbide_test_servers::mssql::Mssql as Database;
use futures::StreamExt;

async fn database(fixture: bool) -> Database {
    tokio::task::spawn_blocking(move || {
        let database = Database::start().unwrap();
        if fixture {
            database.load_script("../../testdata/mssql.sql").unwrap();
        }
        database
    })
    .await
    .unwrap()
}

fn details(database: &Database) -> Connection {
    let mut details = Connection::new(Engine::SqlServer, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.trust_server_certificate = true;
    details
}

async fn open(database: &Database) -> Mssql {
    mssql::connect(&details(database), Some(database.password()))
        .await
        .unwrap()
}

async fn run(session: &mut dyn Session, sql: &str) -> Vec<Vec<Value>> {
    let mut run = session.run(sql);
    let mut rows = Vec::new();
    while let Some(event) = run.next().await {
        match event {
            Event::Rows(batch) => rows.extend(batch),
            Event::Error { text, .. } => panic!("{text}"),
            _ => {}
        }
    }
    rows
}

async fn object(session: &mut dyn Session, name: &str, kind: Kind) -> Object {
    objects::search(
        session,
        &Search {
            name: name.into(),
            kinds: vec![kind],
            limit: 100,
        },
    )
    .await
    .unwrap()
    .into_iter()
    .find(|o| o.name == name)
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
async fn mssql_database_listing_excludes_system_databases() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let databases = objects::databases(&mut session).await.unwrap();
    assert!(
        databases
            .iter()
            .any(|o| o.name == database.database() && o.kind == Kind::Database)
    );
    assert!(
        !databases
            .iter()
            .any(|o| ["master", "tempdb", "model", "msdb"].contains(&o.name.as_str()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_schema_listing_flags_system_namespaces() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let schemas = objects::schemas(&mut session).await.unwrap();
    assert!(schemas.iter().any(|o| o.name == "sys" && o.system));
    assert!(
        schemas
            .iter()
            .any(|o| o.name == "INFORMATION_SCHEMA" && o.system)
    );
    assert!(schemas.iter().any(|o| o.name == "shop" && !o.system));
    assert!(
        objects::objects(&mut session, "absent_schema")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_object_listing_returns_only_the_requested_schemas_objects() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let children = objects::objects(&mut session, "shop").await.unwrap();
    assert_eq!(
        children
            .iter()
            .map(|o| (o.name.as_str(), o.kind))
            .collect::<Vec<_>>(),
        vec![
            ("audit_log", Kind::Table),
            ("common_types", Kind::Table),
            ("customer_orders", Kind::Procedure),
            ("customers", Kind::Table),
            ("line_total", Kind::Function),
            ("order_lines", Kind::Table),
            ("order_summary", Kind::View),
            ("orders", Kind::Table),
        ]
    );
    artifact(&database, "explorer.txt", &format!("{children:#?}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_columns_preserve_identity_defaults_types_and_computation() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let table = object(&mut session, "catalog_items", Kind::Table).await;
    let columns = objects::column_details(&mut session, &table).await.unwrap();
    assert_eq!(
        columns
            .iter()
            .map(|c| (
                c.column.name.as_str(),
                c.column.type_name.as_str(),
                c.column.nullable
            ))
            .collect::<Vec<_>>(),
        vec![
            ("id", "bigint", false),
            ("code", "nvarchar(40)", false),
            ("parent_id", "int", true),
            ("quantity", "int", false),
            ("doubled", "int", true)
        ]
    );
    assert_eq!(columns[0].column.identity, Some(Identity::Always));
    assert_eq!(columns[0].identity_seed.as_deref(), Some("17"));
    assert_eq!(columns[0].identity_increment.as_deref(), Some("3"));
    assert_eq!(columns[1].column.default.as_deref(), Some("(N'雪]''s')"));
    assert_eq!(
        columns[1].default_constraint.as_deref(),
        Some("catalog_items_code_default")
    );
    assert_eq!(columns[3].column.default.as_deref(), Some("((2))"));
    assert_eq!(
        columns[4].column.generated,
        Some(Generated {
            expression: "([quantity]*(2))".into(),
            stored: true
        })
    );
    assert_eq!(
        objects::columns(&mut session, &table).await.unwrap()[0],
        columns[0].column
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_constraints_preserve_key_clustering_checks_and_foreign_key_actions() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let table = object(&mut session, "catalog_items", Kind::Table).await;
    let rules = objects::constraints(&mut session, &table).await.unwrap();
    assert_eq!(
        rules
            .iter()
            .map(|c| (c.name.as_str(), c.kind, c.definition.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (
                "catalog_items_code_key",
                ConstraintKind::Unique,
                "UNIQUE CLUSTERED ([code] ASC)"
            ),
            (
                "catalog_items_parent_fk",
                ConstraintKind::ForeignKey,
                "FOREIGN KEY ([parent_id]) REFERENCES [explorer].[parents] ([id]) ON DELETE SET NULL ON UPDATE CASCADE"
            ),
            (
                "catalog_items_pk",
                ConstraintKind::PrimaryKey,
                "PRIMARY KEY NONCLUSTERED ([id] DESC)"
            ),
            (
                "catalog_items_quantity_check",
                ConstraintKind::Check,
                "CHECK ([quantity]>(0))"
            ),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_indexes_preserve_included_columns_filters_and_clustering() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let table = object(&mut session, "catalog_items", Kind::Table).await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    assert_eq!(
        indexes
            .iter()
            .map(|i| (
                i.object.name.as_str(),
                i.method.as_str(),
                i.unique,
                i.constraint_backed
            ))
            .collect::<Vec<_>>(),
        vec![
            ("catalog_items_code_key", "CLUSTERED", true, true),
            ("catalog_items_pk", "NONCLUSTERED", true, true),
            ("catalog_items_quantity", "NONCLUSTERED", false, false)
        ]
    );
    assert_eq!(indexes[2].columns, vec!["quantity", "parent_id"]);
    assert_eq!(
        indexes[2].definition,
        "CREATE NONCLUSTERED INDEX [catalog_items_quantity] ON [explorer].[catalog_items] ([quantity] DESC) INCLUDE ([parent_id]) WHERE ([quantity]>(1));"
    );
    assert_eq!(
        objects::script(&mut session, &indexes[2].object, Script::Create)
            .await
            .unwrap()
            .matches("CREATE NONCLUSTERED INDEX")
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_search_treats_wildcards_as_literal_characters() {
    let database = database(false).await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TABLE dbo.[literal_%[] (id int); CREATE TABLE dbo.literal_other (id int);",
    )
    .await;
    let hits = objects::search(
        &mut session,
        &Search {
            name: "_%[".into(),
            kinds: vec![Kind::Table],
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        hits.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
        vec!["literal_%["]
    );
    let hits = objects::search(
        &mut session,
        &Search {
            name: "LITERAL".into(),
            kinds: vec![Kind::Table],
            limit: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(hits.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_search_across_databases_reports_refused_connections_and_preserves_current_database()
{
    let first = database(false).await;
    let second = database(false).await;
    let refused = database(false).await;
    let mut session = open(&first).await;
    let mut other = open(&second).await;
    run(&mut session, "CREATE TABLE dbo.cross_database_hit (id int)").await;
    run(&mut other, "CREATE TABLE dbo.cross_database_hit (id int)").await;
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "cross_database_hit".into(),
            kinds: vec![Kind::Table],
            limit: 100,
        },
        |name| {
            let source = if name == second.database() {
                &second
            } else {
                &first
            };
            let mut connection = details(source);
            connection.database = Some(name);
            let password = source.password().to_owned();
            async move { mssql::connect(&connection, Some(&password)).await }
        },
    )
    .await
    .unwrap();
    assert_eq!(report.objects.len(), 2);
    assert!(
        report
            .objects
            .iter()
            .any(|o| o.id.database == first.database())
    );
    assert!(
        report
            .objects
            .iter()
            .any(|o| o.id.database == second.database())
    );
    assert_eq!(session.current_database(), first.database());
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.database == refused.database() && !s.reason.is_empty())
    );
    artifact(&first, "cross-database.txt", &format!("{report:#?}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_search_across_databases_records_each_connector_failure() {
    let database = database(false).await;
    let mut session = open(&database).await;
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "".into(),
            kinds: vec![],
            limit: 100,
        },
        |_| async { Err::<Mssql, _>("deliberately refused") },
    )
    .await
    .unwrap();
    assert!(report.objects.is_empty());
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.database == database.database() && s.reason == "deliberately refused")
    );
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "".into(),
            kinds: vec![],
            limit: 0,
        },
        |_| std::future::ready(Err::<Mssql, _>("zero limit must not connect")),
    )
    .await
    .unwrap();
    assert!(report.objects.is_empty());
    assert!(report.skipped.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_scripted_table_recreates_columns_identity_defaults_constraints_and_indexes() {
    let source = database(true).await;
    let destination = database(false).await;
    let mut session = open(&source).await;
    let table = object(&mut session, "catalog_items", Kind::Table).await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    artifact(&source, "catalog_items.sql", &sql);
    let mut target = open(&destination).await;
    run(&mut target, "CREATE SCHEMA explorer;\nGO\nCREATE TABLE explorer.parents (id int CONSTRAINT parents_pk PRIMARY KEY);").await;
    run(&mut target, &sql).await;
    let recreated = object(&mut target, "catalog_items", Kind::Table).await;
    assert_eq!(
        objects::column_details(&mut session, &table).await.unwrap(),
        objects::column_details(&mut target, &recreated)
            .await
            .unwrap()
    );
    assert_eq!(
        objects::constraints(&mut session, &table).await.unwrap(),
        objects::constraints(&mut target, &recreated).await.unwrap()
    );
    let indexes = objects::indexes(&mut target, &recreated).await.unwrap();
    assert_eq!(
        indexes
            .iter()
            .map(|i| i.definition.as_str())
            .collect::<Vec<_>>(),
        vec![
            "CREATE UNIQUE CLUSTERED INDEX [catalog_items_code_key] ON [explorer].[catalog_items] ([code] ASC);",
            "CREATE UNIQUE NONCLUSTERED INDEX [catalog_items_pk] ON [explorer].[catalog_items] ([id] DESC);",
            "CREATE NONCLUSTERED INDEX [catalog_items_quantity] ON [explorer].[catalog_items] ([quantity] DESC) INCLUDE ([parent_id]) WHERE ([quantity]>(1));",
        ]
    );
    assert_eq!(run(&mut target, "INSERT explorer.catalog_items DEFAULT VALUES; SELECT id, code, quantity, doubled FROM explorer.catalog_items;").await,
        vec![vec![Value::Integer(17), Value::Text("雪]'s".into()), Value::Integer(2), Value::Integer(4)]]);
    assert_eq!(run(&mut target, "INSERT explorer.catalog_items(code) VALUES (N'next'); SELECT id FROM explorer.catalog_items WHERE code = N'next';").await, vec![vec![Value::Integer(20)]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_scripted_table_preserves_its_trigger() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let table = object(&mut session, "catalog_items", Kind::Table).await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP TABLE explorer.catalog_items").await;
    run(&mut session, &sql).await;
    let events: Vec<_> = session
        .run("INSERT explorer.catalog_items(code,quantity) VALUES (N'blocked',13)")
        .collect()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Error { code: Some(code), .. } if code == "51001"))
    );
    assert_eq!(
        run(&mut session, "SELECT COUNT(*) FROM explorer.catalog_items").await,
        vec![vec![Value::Integer(0)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_scripted_trigger_fires_on_its_renamed_parent_table() {
    let source = database(false).await;
    let destination = database(false).await;
    let mut session = open(&source).await;
    run(&mut session, "CREATE TABLE dbo.trigger_log (message nvarchar(100)); CREATE TABLE dbo.[old]]雪] (id int);\nGO\n/* ON dbo.decoy */ CREATE TRIGGER dbo.[audit]]雪] ON /* parent */ dbo.[old]]雪]
        AFTER INSERT AS BEGIN SET NOCOUNT ON; INSERT dbo.trigger_log VALUES (N'ON dbo.[old]]雪]'); END;\nGO\nEXEC sys.sp_rename N'dbo.[old]]雪]', N'new]果';").await;
    let table = object(&mut session, "new]果", Kind::Table).await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    assert!(sql.contains("ON /* parent */ [dbo].[new]]果]"));
    assert!(sql.contains("AFTER INSERT AS BEGIN SET NOCOUNT ON; INSERT dbo.trigger_log VALUES (N'ON dbo.[old]]雪]'); END;"));
    artifact(&source, "renamed-trigger.sql", &sql);
    let mut target = open(&destination).await;
    run(&mut target, "CREATE TABLE dbo.trigger_log (message nvarchar(100)); CREATE TABLE dbo.[old]]雪] (id int);").await;
    run(&mut target, &sql).await;
    assert_eq!(
        run(
            &mut target,
            "INSERT dbo.[new]]果] VALUES (1); SELECT message FROM dbo.trigger_log;"
        )
        .await,
        vec![vec![Value::Text("ON dbo.[old]]雪]".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_scripted_unique_index_ignores_duplicate_inserts() {
    let source = database(false).await;
    let destination = database(false).await;
    let mut session = open(&source).await;
    run(&mut session, "CREATE TABLE dbo.unique_items (id int NOT NULL); CREATE UNIQUE INDEX unique_items_id ON dbo.unique_items(id) WITH (IGNORE_DUP_KEY = ON);").await;
    let table = object(&mut session, "unique_items", Kind::Table).await;
    let index = object(&mut session, "unique_items_id", Kind::Index).await;
    let index_sql = objects::script(&mut session, &index, Script::Create)
        .await
        .unwrap();
    assert!(index_sql.contains("CREATE UNIQUE NONCLUSTERED INDEX [unique_items_id] ON [dbo].[unique_items] ([id] ASC) WITH (IGNORE_DUP_KEY = ON);"));
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    artifact(&source, "ignore-duplicate-index.sql", &sql);
    let mut target = open(&destination).await;
    run(&mut target, &sql).await;
    run(&mut target, "INSERT dbo.unique_items VALUES (1);").await;
    let events: Vec<_> = target
        .run("INSERT dbo.unique_items VALUES (1),(2);")
        .collect()
        .await;
    assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })));
    assert!(
        events.iter().any(
            |e| matches!(e, Event::Message { text, .. } if text == "Duplicate key was ignored.")
        )
    );
    assert_eq!(
        run(&mut target, "SELECT id FROM dbo.unique_items ORDER BY id;").await,
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_scripted_unique_constraint_ignores_duplicate_inserts() {
    let source = database(false).await;
    let destination = database(false).await;
    let mut session = open(&source).await;
    run(&mut session, "CREATE TABLE dbo.unique_items (id int NOT NULL CONSTRAINT unique_items_id UNIQUE NONCLUSTERED WITH (IGNORE_DUP_KEY = ON));").await;
    let table = object(&mut session, "unique_items", Kind::Table).await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    assert!(sql.contains("ADD CONSTRAINT [unique_items_id] UNIQUE NONCLUSTERED ([id] ASC) WITH (IGNORE_DUP_KEY = ON);"));
    artifact(&source, "ignore-duplicate-constraint.sql", &sql);
    let mut target = open(&destination).await;
    run(&mut target, &sql).await;
    run(&mut target, "INSERT dbo.unique_items VALUES (1);").await;
    let events: Vec<_> = target
        .run("INSERT dbo.unique_items VALUES (1),(2);")
        .collect()
        .await;
    assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })));
    assert!(
        events.iter().any(
            |e| matches!(e, Event::Message { text, .. } if text == "Duplicate key was ignored.")
        )
    );
    assert_eq!(
        run(&mut target, "SELECT id FROM dbo.unique_items ORDER BY id;").await,
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_scripted_procedure_recreates_and_runs() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let procedure = object(&mut session, "customer_orders", Kind::Procedure).await;
    let sql = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    artifact(&database, "customer_orders.sql", &sql);
    assert!(sql.starts_with("SET ANSI_NULLS ON;\nGO\nSET QUOTED_IDENTIFIER"));
    run(&mut session, "DROP PROCEDURE shop.customer_orders").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(&mut session, "EXEC shop.customer_orders @customer = 1")
            .await
            .len(),
        2
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_scripted_view_recreates_and_runs() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let view = object(&mut session, "order_summary", Kind::View).await;
    let sql = objects::script(&mut session, &view, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP VIEW shop.order_summary").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(&mut session, "SELECT COUNT(*) FROM shop.order_summary").await,
        vec![vec![Value::Integer(80)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_scripted_function_recreates_and_runs() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let function = object(&mut session, "line_total", Kind::Function).await;
    let sql = objects::script(&mut session, &function, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP FUNCTION shop.line_total").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(&mut session, "SELECT shop.line_total(3,2.50)").await,
        vec![vec![Value::Decimal("7.50".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_alter_rewrites_only_the_module_header_and_keeps_quoted_identifier_setting() {
    let database = database(false).await;
    let mut session = open(&database).await;
    run(&mut session, "SET QUOTED_IDENTIFIER OFF;\nGO\n/* CREATE in a comment */ CREATE PROCEDURE dbo.old_name AS SELECT 'CREATE PROCEDURE dbo.old_name' AS message;\nGO\nEXEC sys.sp_rename N'dbo.old_name', N'new_name';").await;
    let procedure = object(&mut session, "new_name", Kind::Procedure).await;
    let sql = objects::script(&mut session, &procedure, Script::Alter)
        .await
        .unwrap();
    assert!(sql.contains("SET QUOTED_IDENTIFIER OFF;\nGO\n"));
    assert!(sql.contains("CREATE OR ALTER PROCEDURE [dbo].[new_name]"));
    assert!(sql.contains("SELECT 'CREATE PROCEDURE dbo.old_name'"));
    run(&mut session, &sql).await;
    assert_eq!(
        run(&mut session, "EXEC dbo.new_name").await,
        vec![vec![Value::Text("CREATE PROCEDURE dbo.old_name".into())]]
    );
    let create = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP PROCEDURE dbo.new_name").await;
    run(&mut session, &create).await;
    assert_eq!(
        run(&mut session, "EXEC dbo.new_name").await,
        vec![vec![Value::Text("CREATE PROCEDURE dbo.old_name".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_bracket_identifiers_round_trip_through_table_and_select_scripts() {
    let database = database(true).await;
    let mut session = open(&database).await;
    let table = object(&mut session, "odd]果実", Kind::Table).await;
    let create = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    let select = objects::script(&mut session, &table, Script::Select)
        .await
        .unwrap();
    assert_eq!(
        select,
        "SELECT TOP (1000) [key]]雪], [value]]text]\nFROM [explorer].[odd]]果実];\n"
    );
    assert_eq!(
        objects::script(&mut session, &table, Script::Alter)
            .await
            .unwrap(),
        "-- ALTER TABLE [explorer].[odd]]果実]\n--     ADD [column_name] data_type;\n"
    );
    run(&mut session, "DROP TABLE explorer.[odd]]果実]").await;
    run(&mut session, &create).await;
    run(
        &mut session,
        "INSERT explorer.[odd]]果実] ([key]]雪]) VALUES (7)",
    )
    .await;
    assert_eq!(
        run(&mut session, &select).await,
        vec![vec![Value::Integer(7), Value::Text("雪's]".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_stale_or_foreign_object_identities_are_rejected() {
    let database = database(false).await;
    let mut session = open(&database).await;
    run(&mut session, "CREATE TABLE dbo.items(id int)").await;
    let mut table = object(&mut session, "items", Kind::Table).await;
    table.id.database = "another_database".into();
    assert!(matches!(
        objects::columns(&mut session, &table).await,
        Err(objects::Error::Identity)
    ));
    table.id.database = database.database().into();
    run(&mut session, "DROP TABLE dbo.items").await;
    assert!(matches!(
        objects::columns(&mut session, &table).await,
        Err(objects::Error::NotFound)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_table_script_preserves_inferred_nonnull_computed_columns() {
    let database = database(false).await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TABLE dbo.computed_items (a int NULL, b AS ISNULL(a,0))",
    )
    .await;
    let table = object(&mut session, "computed_items", Kind::Table).await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP TABLE dbo.computed_items").await;
    run(&mut session, &sql).await;
    let recreated = object(&mut session, "computed_items", Kind::Table).await;
    let columns = objects::columns(&mut session, &recreated).await.unwrap();
    assert!(!columns[1].nullable);
    assert_eq!(
        columns[1].generated,
        Some(Generated {
            expression: "(isnull([a],(0)))".into(),
            stored: false
        })
    );
    assert_eq!(
        run(
            &mut session,
            "INSERT dbo.computed_items DEFAULT VALUES; SELECT b FROM dbo.computed_items"
        )
        .await,
        vec![vec![Value::Integer(0)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_table_script_creates_unique_indexes_before_self_referencing_foreign_keys() {
    let database = database(false).await;
    let mut session = open(&database).await;
    run(&mut session, "CREATE TABLE dbo.linked_items (code int NOT NULL, parent_code int NULL);
        CREATE UNIQUE INDEX linked_items_code ON dbo.linked_items(code);
        ALTER TABLE dbo.linked_items ADD CONSTRAINT linked_items_fk FOREIGN KEY (parent_code) REFERENCES dbo.linked_items(code);").await;
    let table = object(&mut session, "linked_items", Kind::Table).await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP TABLE dbo.linked_items").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(
            &mut session,
            "INSERT dbo.linked_items VALUES (1,1); SELECT code,parent_code FROM dbo.linked_items"
        )
        .await,
        vec![vec![Value::Integer(1), Value::Integer(1)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_table_script_preserves_disabled_constraint_backed_indexes() {
    let database = database(false).await;
    let mut session = open(&database).await;
    run(&mut session, "CREATE TABLE dbo.disabled_items (id int CONSTRAINT disabled_items_pk PRIMARY KEY NONCLUSTERED);
        ALTER INDEX disabled_items_pk ON dbo.disabled_items DISABLE;").await;
    let table = object(&mut session, "disabled_items", Kind::Table).await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP TABLE dbo.disabled_items").await;
    run(&mut session, &sql).await;
    assert_eq!(run(&mut session, "SELECT is_disabled FROM sys.indexes WHERE object_id = OBJECT_ID(N'dbo.disabled_items') AND name = N'disabled_items_pk'").await, vec![vec![Value::Bool(true)]]);
}
