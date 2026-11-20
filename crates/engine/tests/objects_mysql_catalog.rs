use dbide_engine::mysql::{self, MySql};
use dbide_engine::objects::mysql as objects;
use dbide_engine::objects::{Column, Constraint, ConstraintKind, Identity, Kind, Script, Search};
use dbide_engine::session::Session;

mod mysql_support;
use mysql_support::*;

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
    assert!(
        report.skipped.iter().all(|skipped| {
            skipped.database != first.database()
                && skipped.database != second.database()
                && skipped.reason
                    == format!(
                        "Signed in, but database {} doesn't exist or this login can't open it.",
                        skipped.database
                    )
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
    assert_eq!(
        refusal.reason,
        "Sign-in failed: the user name or password is wrong."
    );
    assert!(
        report.skipped.iter().all(|skipped| {
            skipped.database == refused.database()
                || (skipped.database != source.database()
                    && skipped.reason
                        == format!(
                            "Signed in, but database {} doesn't exist or this login can't open it.",
                            skipped.database
                        ))
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
