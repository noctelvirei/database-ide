use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::mysql::{self, MySql};
use dbide_engine::objects::mysql as objects;
use dbide_engine::objects::{
    Column, Constraint, ConstraintKind, Generated, Identity, Kind, Object, Script, Search,
};
use dbide_engine::session::{Event, Session, Value};
use dbide_test_servers::mysql::MySql as Database;
use futures::StreamExt;

async fn database() -> Database {
    tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap()
}

async fn fixture() -> Database {
    let database = database().await;
    tokio::task::spawn_blocking(move || {
        database.load_script("../../testdata/mysql.sql").unwrap();
        database
    })
    .await
    .unwrap()
}

fn details(database: &Database) -> Connection {
    let mut details = Connection::new(Engine::MySql, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.tls = TlsMode::Off;
    details
}

async fn open(database: &Database) -> MySql {
    mysql::connect(&details(database), Some(database.password()))
        .await
        .unwrap()
}

async fn run(session: &mut dyn Session, sql: &str) -> Vec<Vec<Value>> {
    let mut run = session.run(sql);
    let mut rows = Vec::new();
    while let Some(event) = run.next().await {
        match event {
            Event::Error { text, .. } => panic!("{text}"),
            Event::Rows(batch) => rows.extend(batch),
            _ => {}
        }
    }
    rows
}

async fn object(session: &mut dyn Session, name: &str) -> Object {
    objects::search(
        session,
        &Search {
            name: name.into(),
            kinds: vec![],
            limit: 100,
        },
    )
    .await
    .unwrap()
    .into_iter()
    .find(|object| object.name == name)
    .unwrap()
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
async fn mysql_database_listing_excludes_system_databases() {
    let database = database().await;
    let mut session = open(&database).await;
    let databases = objects::databases(&mut session).await.unwrap();
    assert!(
        databases
            .iter()
            .any(|object| object.name == database.database())
    );
    assert!(!databases.iter().any(|object| {
        ["mysql", "sys", "information_schema", "performance_schema"].contains(&object.name.as_str())
    }));
    assert!(
        databases
            .iter()
            .all(|object| object.kind == Kind::Database && !object.system)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_schema_listing_maps_the_current_database_to_one_schema() {
    let database = database().await;
    let mut session = open(&database).await;
    let schemas = objects::schemas(&mut session).await.unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, database.database());
    assert_eq!(schemas[0].kind, Kind::Schema);
    assert_eq!(schemas[0].schema, None);
    assert!(
        objects::objects(&mut session, "absent")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_object_listing_returns_fixture_tables_views_and_routines() {
    let database = fixture().await;
    let mut session = open(&database).await;
    let children = objects::objects(&mut session, database.database())
        .await
        .unwrap();
    assert_eq!(
        children
            .iter()
            .map(|object| (object.name.as_str(), object.kind))
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
    assert!(
        children
            .iter()
            .all(|object| object.schema.as_deref() == Some(database.database()) && !object.system)
    );
    artifact(&database, "mysql-explorer.txt", &format!("{children:#?}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_columns_return_fixture_types_nullability_and_auto_increment() {
    let database = fixture().await;
    let mut session = open(&database).await;
    let table = object(&mut session, "customers").await;
    assert_eq!(
        objects::columns(&mut session, &table).await.unwrap(),
        vec![
            Column {
                name: "id".into(),
                type_name: "bigint".into(),
                nullable: false,
                default: None,
                identity: Some(Identity::ByDefault),
                generated: None
            },
            Column {
                name: "display_name".into(),
                type_name: "text".into(),
                nullable: false,
                default: None,
                identity: None,
                generated: None
            },
            Column {
                name: "email".into(),
                type_name: "text".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: None
            },
            Column {
                name: "notes".into(),
                type_name: "text".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: None
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_constraints_return_fixture_primary_and_foreign_keys() {
    let database = fixture().await;
    let mut session = open(&database).await;
    let table = object(&mut session, "order_lines").await;
    let constraints = objects::constraints(&mut session, &table).await.unwrap();
    assert_eq!(constraints, vec![
        Constraint { name: "PRIMARY".into(), kind: ConstraintKind::PrimaryKey, definition: "PRIMARY KEY (`order_id`, `line_number`)".into() },
        Constraint { name: "order_lines_ibfk_1".into(), kind: ConstraintKind::ForeignKey,
            definition: "FOREIGN KEY (`order_id`) REFERENCES `orders` (`id`) ON DELETE NO ACTION ON UPDATE NO ACTION".into() },
    ]);
    artifact(&database, "mysql-keys.txt", &format!("{constraints:#?}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_indexes_return_fixture_key_order_and_uniqueness() {
    let database = fixture().await;
    let mut session = open(&database).await;
    let table = object(&mut session, "orders").await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    assert_eq!(indexes.len(), 2);
    assert_eq!(indexes[0].object.name, "PRIMARY");
    assert_eq!(indexes[0].columns, vec!["`id`"]);
    assert!(indexes[0].unique);
    assert!(indexes[0].constraint_backed);
    assert_eq!(indexes[0].method, "btree");
    assert_eq!(indexes[1].object.name, "customer_id");
    assert_eq!(indexes[1].columns, vec!["`customer_id`"]);
    assert!(!indexes[1].unique);
    assert!(!indexes[1].constraint_backed);
    assert_eq!(
        objects::script(&mut session, &indexes[1].object, Script::Create)
            .await
            .unwrap(),
        format!(
            "CREATE INDEX `customer_id` ON `{}`.`orders` (`customer_id`) USING BTREE;\n",
            database.database()
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_search_matches_literal_substrings_and_filters_kinds() {
    let database = fixture().await;
    let mut session = open(&database).await;
    let hits = objects::search(
        &mut session,
        &Search {
            name: "ORDER".into(),
            kinds: vec![Kind::View, Kind::Procedure],
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        hits.iter()
            .map(|object| object.name.as_str())
            .collect::<Vec<_>>(),
        vec!["customer_orders", "order_summary"]
    );
    run(
        &mut session,
        "CREATE TABLE `fruit_%` (id int); CREATE TABLE fruit_pear (id int)",
    )
    .await;
    let hits = objects::search(
        &mut session,
        &Search {
            name: "_%".into(),
            kinds: vec![Kind::Table],
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        hits.iter()
            .map(|object| object.name.as_str())
            .collect::<Vec<_>>(),
        vec!["fruit_%"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_search_all_returns_hits_from_two_databases_without_switching_the_source() {
    let first = database().await;
    let second = database().await;
    let mut session = open(&first).await;
    let mut other = open(&second).await;
    run(&mut session, "CREATE TABLE cross_database_pear (id int)").await;
    run(&mut other, "CREATE TABLE cross_database_pear (id int)").await;
    let connection = details(&first);
    let password = first.password().to_owned();
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "cross_database_pear".into(),
            kinds: vec![Kind::Table],
            limit: 100,
        },
        |database| {
            let mut connection = connection.clone();
            let password = password.clone();
            connection.database = Some(database);
            async move { mysql::connect(&connection, Some(&password)).await }
        },
    )
    .await
    .unwrap();
    artifact(&first, "mysql-search.txt", &format!("{report:#?}"));
    assert_eq!(report.objects.len(), 2);
    assert!(report.objects.iter().any(
        |object| object.id.database == first.database() && object.name == "cross_database_pear"
    ));
    assert!(
        report
            .objects
            .iter()
            .any(|object| object.id.database == second.database()
                && object.name == "cross_database_pear")
    );
    // Other tests share this user/server and may drop a database after enumeration.
    assert!(
        report.skipped.iter().all(|skipped| {
            skipped.database != first.database()
                && skipped.database != second.database()
                && skipped.reason.contains("1049")
                && skipped.reason.contains("Unknown database")
        }),
        "{report:#?}"
    );
    assert_eq!(session.current_database(), first.database());
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_search_all_applies_one_global_limit() {
    let first = database().await;
    let second = database().await;
    let mut session = open(&first).await;
    let mut other = open(&second).await;
    run(&mut session, "CREATE TABLE limited_pear (id int)").await;
    run(&mut other, "CREATE TABLE limited_pear (id int)").await;
    let connection = details(&first);
    let password = first.password().to_owned();
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "limited_pear".into(),
            kinds: vec![Kind::Table],
            limit: 1,
        },
        |database| {
            let mut connection = connection.clone();
            let password = password.clone();
            connection.database = Some(database);
            async move { mysql::connect(&connection, Some(&password)).await }
        },
    )
    .await
    .unwrap();
    assert_eq!(report.objects.len(), 1);
    assert_eq!(report.objects[0].name, "limited_pear");
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_table_recreates_fixture_columns_and_keys_in_a_fresh_database() {
    let source = fixture().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    let table = object(&mut session, "customers").await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-customers.sql", &sql);
    run(&mut destination, &sql).await;
    let copy = object(&mut destination, "customers").await;
    assert_eq!(
        objects::columns(&mut destination, &copy).await.unwrap(),
        vec![
            Column {
                name: "id".into(),
                type_name: "bigint".into(),
                nullable: false,
                default: None,
                identity: Some(Identity::ByDefault),
                generated: None
            },
            Column {
                name: "display_name".into(),
                type_name: "text".into(),
                nullable: false,
                default: None,
                identity: None,
                generated: None
            },
            Column {
                name: "email".into(),
                type_name: "text".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: None
            },
            Column {
                name: "notes".into(),
                type_name: "text".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: None
            },
        ]
    );
    assert_eq!(
        objects::constraints(&mut destination, &copy).await.unwrap(),
        vec![Constraint {
            name: "PRIMARY".into(),
            kind: ConstraintKind::PrimaryKey,
            definition: "PRIMARY KEY (`id`)".into()
        },]
    );
    assert_eq!(
        run(&mut destination, "SELECT COUNT(*) FROM customers").await,
        vec![vec![Value::Integer(0)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_table_preserves_defaults_generation_and_checks_across_sql_modes() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(&mut session, r"SET sql_mode = 'NO_BACKSLASH_ESCAPES';
        CREATE TABLE pears (id int PRIMARY KEY AUTO_INCREMENT, name varchar(20) NOT NULL DEFAULT 'pear\tree',
        quantity int NOT NULL DEFAULT 7, doubled int GENERATED ALWAYS AS (quantity * 2) STORED,
        UNIQUE KEY pear_name (name), CONSTRAINT positive_quantity CHECK (quantity > 0))").await;
    let table = object(&mut session, "pears").await;
    let sql = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-pears-defaults.sql", &sql);
    run(&mut destination, "SET sql_mode = ''").await;
    run(&mut destination, &sql).await;
    let copy = object(&mut destination, "pears").await;
    let columns = objects::columns(&mut destination, &copy).await.unwrap();
    assert_eq!(
        columns[1].default.as_deref(),
        Some("_utf8mb4 X'706561725C74726565'")
    );
    assert_eq!(columns[2].default.as_deref(), Some("7"));
    assert_eq!(
        columns[3].generated,
        Some(Generated {
            expression: "(`quantity` * 2)".into(),
            stored: true
        })
    );
    assert_eq!(
        objects::constraints(&mut destination, &copy).await.unwrap(),
        vec![
            Constraint {
                name: "PRIMARY".into(),
                kind: ConstraintKind::PrimaryKey,
                definition: "PRIMARY KEY (`id`)".into()
            },
            Constraint {
                name: "pear_name".into(),
                kind: ConstraintKind::Unique,
                definition: "UNIQUE (`name`)".into()
            },
            Constraint {
                name: "positive_quantity".into(),
                kind: ConstraintKind::Check,
                definition: "CHECK ((`quantity` > 0))".into()
            },
        ]
    );
    run(&mut destination, "INSERT INTO pears () VALUES ()").await;
    assert_eq!(
        run(
            &mut destination,
            "SELECT id, name, quantity, doubled FROM pears"
        )
        .await,
        vec![vec![
            Value::Integer(1),
            Value::Text(r"pear\tree".into()),
            Value::Integer(7),
            Value::Integer(14)
        ]]
    );
    assert_eq!(
        run(&mut session, "SELECT @@sql_mode").await,
        vec![vec![Value::Text("NO_BACKSLASH_ESCAPES".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_procedure_recreates_the_fixture_body() {
    let source = fixture().await;
    let mut session = open(&source).await;
    let procedure = object(&mut session, "customer_orders").await;
    let sql = objects::script(&mut session, &procedure, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-procedure.sql", &sql);
    run(&mut session, "DROP PROCEDURE customer_orders").await;
    run(&mut session, &sql).await;
    let rows = run(&mut session, "CALL customer_orders(1)").await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], Value::Integer(1));
    assert_eq!(rows[1][0], Value::Integer(41));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_function_recreates_the_fixture_body() {
    let source = fixture().await;
    let mut session = open(&source).await;
    let function = object(&mut session, "line_total").await;
    let sql = objects::script(&mut session, &function, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP FUNCTION line_total").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(&mut session, "SELECT line_total(3, 2.50)").await,
        vec![vec![Value::Decimal("7.50".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_view_recreates_the_fixture_query() {
    let source = fixture().await;
    let mut session = open(&source).await;
    let view = object(&mut session, "order_summary").await;
    let sql = objects::script(&mut session, &view, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-view.sql", &sql);
    run(&mut session, "DROP VIEW order_summary").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(
            &mut session,
            "SELECT id, display_name, total FROM order_summary WHERE id = 1"
        )
        .await,
        vec![vec![
            Value::Integer(1),
            Value::Text("Zorb-1 雪é".into()),
            Value::Decimal("7.50".into())
        ]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_alter_view_preserves_the_fixture_query() {
    let source = fixture().await;
    let mut session = open(&source).await;
    let view = object(&mut session, "order_summary").await;
    let sql = objects::script(&mut session, &view, Script::Alter)
        .await
        .unwrap();
    assert!(sql.contains("\nALTER "));
    run(&mut session, &sql).await;
    assert_eq!(
        run(&mut session, "SELECT COUNT(*) FROM order_summary").await,
        vec![vec![Value::Integer(80)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_backtick_identifiers_round_trip_through_create_and_select() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(&mut session, "CREATE TABLE `pear``box` (`fruit``name` varchar(20) DEFAULT 'pear'); INSERT INTO `pear``box` () VALUES ()").await;
    let table = object(&mut session, "pear`box").await;
    let select = objects::script(&mut session, &table, Script::Select)
        .await
        .unwrap();
    assert_eq!(
        select,
        format!(
            "SELECT `fruit``name` FROM `{}`.`pear``box` LIMIT 1000;\n",
            source.database()
        )
    );
    assert_eq!(
        run(&mut session, &select).await,
        vec![vec![Value::Text("pear".into())]]
    );
    let create = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    run(&mut destination, &create).await;
    run(&mut destination, "INSERT INTO `pear``box` () VALUES ()").await;
    assert_eq!(
        run(&mut destination, "SELECT `fruit``name` FROM `pear``box`").await,
        vec![vec![Value::Text("pear".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_table_alter_is_only_a_commented_skeleton() {
    let source = database().await;
    let mut session = open(&source).await;
    run(&mut session, "CREATE TABLE pears (id int)").await;
    let table = object(&mut session, "pears").await;
    assert_eq!(
        objects::script(&mut session, &table, Script::Alter)
            .await
            .unwrap(),
        format!(
            "-- ALTER TABLE `{}`.`pears`\n--     ADD COLUMN column_name data_type;\n",
            source.database()
        )
    );
    assert!(!objects::supports(Kind::Procedure, Script::Alter));
    assert!(!objects::supports(Kind::Function, Script::Alter));
    assert!(!objects::supports(Kind::Index, Script::Select));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_metadata_rejects_foreign_database_identities() {
    let source = fixture().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    let table = object(&mut session, "customers").await;
    assert!(matches!(
        objects::columns(&mut destination, &table).await,
        Err(objects::Error::Identity)
    ));
    assert!(matches!(
        objects::script(&mut destination, &table, Script::Create).await,
        Err(objects::Error::Identity)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_script_rejects_dropped_objects() {
    let source = database().await;
    let mut session = open(&source).await;
    run(&mut session, "CREATE TABLE pears (id int)").await;
    let table = object(&mut session, "pears").await;
    run(&mut session, "DROP TABLE pears").await;
    assert!(matches!(
        objects::script(&mut session, &table, Script::Create).await,
        Err(objects::Error::NotFound)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_index_create_recreates_prefix_direction_and_visibility() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (name varchar(30), quantity int);
        CREATE INDEX pear_name ON pears (name(10), quantity DESC) INVISIBLE",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    assert_eq!(indexes[0].columns, vec!["`name`(10)", "`quantity` DESC"]);
    let sql = objects::script(&mut session, &indexes[0].object, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-index.sql", &sql);
    run(&mut session, "DROP INDEX pear_name ON pears").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(
            &mut session,
            "SELECT INDEX_NAME, COLUMN_NAME, CAST(SUB_PART AS CHAR), COLLATION, IS_VISIBLE
        FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'pears'
        ORDER BY SEQ_IN_INDEX"
        )
        .await,
        vec![
            vec![
                Value::Text("pear_name".into()),
                Value::Text("name".into()),
                Value::Text("10".into()),
                Value::Text("A".into()),
                Value::Text("NO".into())
            ],
            vec![
                Value::Text("pear_name".into()),
                Value::Text("quantity".into()),
                Value::Null,
                Value::Text("D".into()),
                Value::Text("NO".into())
            ],
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_view_create_preserves_unicode_from_a_latin1_creation_context() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "SET NAMES latin1; CREATE VIEW pears AS SELECT _latin1 X'E9' AS name,
            CAST(COERCIBILITY(_latin1 X'E9') AS SIGNED) AS coercibility,
            _latin1 X'615C62276300' COLLATE latin1_bin AS escaped_text,
            _utf8mb4 X'E99BAA' AS mixed_name; SET NAMES utf8mb4",
    )
    .await;
    let view = object(&mut session, "pears").await;
    let sql = objects::script(&mut session, &view, Script::Create)
        .await
        .unwrap();
    artifact(&source, "mysql-view-latin1.sql", &sql);
    run(&mut session, "DROP VIEW pears").await;
    run(&mut session, &sql).await;
    assert_eq!(
        run(
            &mut session,
            "SELECT name, coercibility, escaped_text, mixed_name FROM pears"
        )
        .await,
        vec![vec![
            Value::Text("é".into()),
            Value::Integer(4),
            Value::Text("a\\b'c\0".into()),
            Value::Text("雪".into())
        ]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_search_all_reports_a_refused_database_without_losing_other_hits() {
    let source = database().await;
    let refused = database().await;
    let mut session = open(&source).await;
    run(&mut session, "CREATE TABLE surviving_pear (id int)").await;
    let connection = details(&source);
    let password = source.password().to_owned();
    let refused_name = refused.database().to_owned();
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "surviving_pear".into(),
            kinds: vec![Kind::Table],
            limit: 100,
        },
        |database| {
            let mut connection = connection.clone();
            let password = if database == refused_name {
                "incorrect password".to_owned()
            } else {
                password.clone()
            };
            connection.database = Some(database);
            async move {
                mysql::connect(&connection, Some(&password))
                    .await
                    .map_err(|error| error.to_string())
            }
        },
    )
    .await
    .unwrap();
    artifact(&source, "mysql-search-refused.txt", &format!("{report:#?}"));
    assert_eq!(report.objects.len(), 1);
    assert_eq!(report.objects[0].name, "surviving_pear");
    let refusal = report
        .skipped
        .iter()
        .find(|skipped| skipped.database == refused.database())
        .unwrap_or_else(|| panic!("{report:#?}"));
    assert!(refusal.reason.contains("1045"), "{report:#?}");
    assert!(refusal.reason.contains("Access denied"), "{report:#?}");
    assert!(
        report.skipped.iter().all(|skipped| {
            skipped.database == refused.database()
                || (skipped.database != source.database()
                    && skipped.reason.contains("1049")
                    && skipped.reason.contains("Unknown database"))
        }),
        "{report:#?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_zero_search_limit_never_calls_the_connector() {
    let source = database().await;
    let mut session = open(&source).await;
    let mut called = false;
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "".into(),
            kinds: vec![],
            limit: 0,
        },
        |_| {
            called = true;
            async { Err::<MySql, &str>("must not connect") }
        },
    )
    .await
    .unwrap();
    assert!(report.objects.is_empty());
    assert!(report.skipped.is_empty());
    assert!(!called);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_select_scripts_omit_invisible_columns() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (id int, hidden int INVISIBLE)",
    )
    .await;
    let table = object(&mut session, "pears").await;
    assert_eq!(
        objects::columns(&mut session, &table).await.unwrap(),
        vec![Column {
            name: "id".into(),
            type_name: "int".into(),
            nullable: true,
            default: None,
            identity: None,
            generated: None
        },]
    );
    assert_eq!(
        objects::script(&mut session, &table, Script::Select)
            .await
            .unwrap(),
        format!(
            "SELECT `id` FROM `{}`.`pears` LIMIT 1000;\n",
            source.database()
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_refuses_unquoted_show_definitions_without_changing_settings() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE `pear``box` (id int); SET sql_quote_show_create = OFF",
    )
    .await;
    let table = object(&mut session, "pear`box").await;
    assert!(matches!(
        objects::script(&mut session, &table, Script::Create).await,
        Err(objects::Error::QuotingDisabled)
    ));
    assert_eq!(
        run(&mut session, "SELECT CAST(@@sql_quote_show_create AS CHAR)").await,
        vec![vec![Value::Text("0".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_spatial_index_scripts_recreate_the_spatial_access_method() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (location POINT NOT NULL SRID 4326);
        CREATE SPATIAL INDEX pear_location ON pears (location)",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    let sql = objects::script(&mut session, &indexes[0].object, Script::Create)
        .await
        .unwrap();
    assert!(sql.contains("CREATE SPATIAL INDEX `pear_location`"));
    run(&mut session, "DROP INDEX pear_location ON pears").await;
    run(&mut session, &sql).await;
    assert_eq!(
        objects::indexes(&mut session, &table).await.unwrap()[0].method,
        "spatial"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_functional_index_scripts_recreate_the_expression() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (quantity int);
        CREATE INDEX pear_quantity ON pears ((quantity * 2))",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    assert_eq!(indexes[0].columns, vec!["((`quantity` * 2))"]);
    let sql = objects::script(&mut session, &indexes[0].object, Script::Create)
        .await
        .unwrap();
    run(&mut session, "DROP INDEX pear_quantity ON pears").await;
    run(&mut session, &sql).await;
    assert_eq!(
        objects::indexes(&mut session, &table).await.unwrap()[0].columns,
        vec!["((`quantity` * 2))"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_create_refuses_ansi_identifier_quotes_without_changing_sql_mode() {
    let source = database().await;
    let mut session = open(&source).await;
    run(
        &mut session,
        "CREATE TABLE pears (id int); SET sql_mode = 'ANSI_QUOTES'",
    )
    .await;
    let table = object(&mut session, "pears").await;
    assert!(matches!(
        objects::script(&mut session, &table, Script::Create).await,
        Err(objects::Error::AnsiQuotes)
    ));
    assert_eq!(
        run(&mut session, "SELECT @@sql_mode").await,
        vec![vec![Value::Text("ANSI_QUOTES".into())]]
    );
}
