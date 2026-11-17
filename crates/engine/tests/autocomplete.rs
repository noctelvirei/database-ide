use std::time::{Duration, Instant};

use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::objects::{AutocompleteCatalog, Kind};
use dbide_engine::session::{Event, PlanMode, Run, Session};
use dbide_test_servers::mssql::Mssql as MsDatabase;
use dbide_test_servers::mysql::MySql as MyDatabase;
use dbide_test_servers::postgres::Postgres as PgDatabase;
use futures::StreamExt;

// Counts real server batches so the fixed-query requirement cannot regress into N+1 calls.
#[derive(Debug)]
struct CountedSession<'a> {
    inner: &'a mut dyn Session,
    queries: usize,
}

impl Session for CountedSession<'_> {
    fn switch_database<'a>(
        &'a mut self,
        database: &'a str,
        password: Option<&'a str>,
    ) -> futures::future::BoxFuture<'a, Result<(), dbide_engine::session::Error>> {
        self.inner.switch_database(database, password)
    }

    fn current_database(&self) -> &str {
        self.inner.current_database()
    }

    fn run<'a>(&'a mut self, batch: &'a str) -> Run<'a> {
        self.queries += 1;
        self.inner.run(batch)
    }

    fn plan<'a>(&'a mut self, batch: &'a str, mode: PlanMode) -> Run<'a> {
        self.inner.plan(batch, mode)
    }
}

async fn run(session: &mut dyn Session, sql: &str) {
    let mut run = session.run(sql);
    while let Some(event) = run.next().await {
        if let Event::Error { text, .. } = event {
            panic!("{text}");
        }
    }
}

fn assert_fixture(catalog: &AutocompleteCatalog, schema: &str, customer_orders: Kind) {
    assert!(catalog.schemas.iter().any(|s| s.name == schema));
    assert_eq!(
        catalog
            .objects
            .iter()
            .filter(|o| o.object.schema.as_deref() == Some(schema))
            .map(|o| (o.object.name.as_str(), o.object.kind))
            .collect::<Vec<_>>(),
        vec![
            ("audit_log", Kind::Table),
            ("common_types", Kind::Table),
            ("customer_orders", customer_orders),
            ("customers", Kind::Table),
            ("line_total", Kind::Function),
            ("order_lines", Kind::Table),
            ("order_summary", Kind::View),
            ("orders", Kind::Table),
        ]
    );
    assert_columns(
        catalog,
        schema,
        "customers",
        &["id", "display_name", "email", "notes"],
    );
    assert_columns(catalog, schema, "audit_log", &["occurred_at", "message"]);
    assert_columns(
        catalog,
        schema,
        "orders",
        &["id", "customer_id", "ordered_at", "total"],
    );
    assert_columns(
        catalog,
        schema,
        "order_lines",
        &[
            "order_id",
            "line_number",
            "item_name",
            "quantity",
            "unit_price",
        ],
    );
    assert_columns(
        catalog,
        schema,
        "order_summary",
        &["id", "display_name", "ordered_at", "total"],
    );
    assert_columns(catalog, schema, "customer_orders", &[]);
    assert_columns(catalog, schema, "line_total", &[]);
}

fn assert_columns(catalog: &AutocompleteCatalog, schema: &str, name: &str, columns: &[&str]) {
    let object = catalog
        .objects
        .iter()
        .find(|o| o.object.schema.as_deref() == Some(schema) && o.object.name == name)
        .unwrap();
    assert_eq!(
        object
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        columns
    );
}

fn artifact(database: &str, catalog: &AutocompleteCatalog) {
    let folder = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "../../target".into())
        .join("test-artifacts")
        .join(database);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("autocomplete.txt"), format!("{catalog:#?}")).unwrap();
}

fn assert_large_catalog(catalog: &AutocompleteCatalog, schema: &str, elapsed: Duration) {
    let tables = catalog
        .objects
        .iter()
        .filter(|o| o.object.schema.as_deref() == Some(schema) && o.object.kind == Kind::Table)
        .collect::<Vec<_>>();
    assert_eq!(tables.len(), 500);
    assert!(tables.iter().all(|o| {
        o.columns
            .iter()
            .map(|c| c.name.as_str())
            .eq(["zebra", "apple"])
    }));
    assert_eq!(
        tables
            .iter()
            .map(|o| o.object.name.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        500
    );
    assert!(tables.iter().all(|o| {
        o.object
            .name
            .strip_prefix("bulk_")
            .and_then(|n| n.parse::<usize>().ok())
            .is_some_and(|n| n < 500)
    }));
    // Two local catalog scans over 1000 new columns should fit in five seconds even in debug
    // builds under concurrent harness load; setup and server startup are outside this budget.
    assert!(elapsed < Duration::from_secs(5), "catalog took {elapsed:?}");
}

async fn pg_open(fixture: bool) -> (PgDatabase, dbide_engine::postgres::Postgres) {
    let database = tokio::task::spawn_blocking(move || {
        let database = PgDatabase::start().unwrap();
        if fixture {
            database.load_script("../../testdata/postgres.sql").unwrap();
        }
        database
    })
    .await
    .unwrap();
    let mut details = Connection::new(Engine::PostgreSql, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.tls = TlsMode::Off;
    let session = dbide_engine::postgres::connect(&details, Some(database.password()))
        .await
        .unwrap();
    (database, session)
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_autocomplete_contains_fixture_objects_and_columns_in_declaration_order() {
    let (database, mut session) = pg_open(true).await;
    let catalog = dbide_engine::objects::postgres::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    assert_fixture(&catalog, "shop", Kind::Function);
    assert_columns(
        &catalog,
        "shop",
        "common_types",
        &[
            "id",
            "bool_value",
            "small_value",
            "int_value",
            "big_value",
            "decimal_value",
            "real_value",
            "double_value",
            "fixed_value",
            "varying_value",
            "text_value",
            "binary_value",
            "date_value",
            "time_value",
            "zoned_time_value",
            "timestamp_value",
            "zoned_timestamp_value",
            "interval_value",
            "uuid_value",
            "json_value",
            "jsonb_value",
            "xml_value",
            "bits_value",
            "array_value",
        ],
    );
    assert!(
        catalog
            .objects
            .iter()
            .all(|o| o.object.id.database == database.database())
    );
    artifact(database.database(), &catalog);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_autocomplete_returns_all_500_tables_and_columns_within_five_seconds() {
    let (database, mut session) = pg_open(false).await;
    run(&mut session, "CREATE SCHEMA bulk").await;
    let sql = (0..500)
        .map(|n| format!("CREATE TABLE bulk.bulk_{n} (zebra integer, apple integer);"))
        .collect::<String>();
    run(&mut session, &sql).await;
    let mut session = CountedSession {
        inner: &mut session,
        queries: 0,
    };
    let start = Instant::now();
    let catalog = dbide_engine::objects::postgres::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(session.queries, 2);
    assert_large_catalog(&catalog, "bulk", elapsed);
    artifact(database.database(), &catalog);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_autocomplete_returns_quoted_names_without_delimiters() {
    let (database, mut session) = pg_open(false).await;
    run(&mut session, r#"CREATE SCHEMA "odd schema"; CREATE SCHEMA empty;
        CREATE TABLE "odd schema"."select" ("zebra snow" integer, "a""b" text);
        CREATE TABLE public."select" (other integer);
        CREATE VIEW "odd schema"."odd view" AS SELECT * FROM "odd schema"."select";
        CREATE FUNCTION "odd schema"."odd function"(integer) RETURNS integer LANGUAGE sql AS 'SELECT $1';
        CREATE FUNCTION "odd schema"."odd function"(text) RETURNS text LANGUAGE sql AS 'SELECT $1';
        CREATE PROCEDURE "odd schema"."odd procedure"() LANGUAGE sql AS 'SELECT 1';"#).await;
    let catalog = dbide_engine::objects::postgres::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    assert_columns(&catalog, "odd schema", "select", &["zebra snow", "a\"b"]);
    assert_columns(&catalog, "odd schema", "odd view", &["zebra snow", "a\"b"]);
    assert_columns(&catalog, "odd schema", "odd function", &[]);
    assert_columns(&catalog, "odd schema", "odd procedure", &[]);
    assert!(
        catalog
            .objects
            .iter()
            .any(|o| o.object.name == "odd procedure" && o.object.kind == Kind::Procedure)
    );
    assert!(catalog.schemas.iter().any(|s| s.name == "empty"));
    assert_columns(&catalog, "public", "select", &["other"]);
    assert_eq!(
        catalog
            .objects
            .iter()
            .filter(|o| o.object.name == "odd function")
            .count(),
        2
    );
    artifact(database.database(), &catalog);
}

async fn mssql_open(fixture: bool) -> (MsDatabase, dbide_engine::mssql::Mssql) {
    let database = tokio::task::spawn_blocking(move || {
        let database = MsDatabase::start().unwrap();
        if fixture {
            database.load_script("../../testdata/mssql.sql").unwrap();
        }
        database
    })
    .await
    .unwrap();
    let mut details = Connection::new(Engine::SqlServer, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.trust_server_certificate = true;
    let session = dbide_engine::mssql::connect(&details, Some(database.password()))
        .await
        .unwrap();
    (database, session)
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_autocomplete_contains_fixture_objects_and_columns_in_declaration_order() {
    let (database, mut session) = mssql_open(true).await;
    let catalog = dbide_engine::objects::mssql::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    assert_fixture(&catalog, "shop", Kind::Procedure);
    assert!(catalog.schemas.iter().any(|s| s.name == "explorer"));
    assert_eq!(
        catalog
            .objects
            .iter()
            .filter(|o| o.object.schema.as_deref() == Some("explorer"))
            .map(|o| (o.object.name.as_str(), o.object.kind))
            .collect::<Vec<_>>(),
        vec![
            ("catalog_items", Kind::Table),
            ("odd]果実", Kind::Table),
            ("parents", Kind::Table)
        ]
    );
    assert_columns(&catalog, "explorer", "parents", &["id"]);
    assert_columns(
        &catalog,
        "explorer",
        "catalog_items",
        &["id", "code", "parent_id", "quantity", "doubled"],
    );
    assert_columns(&catalog, "explorer", "odd]果実", &["key]雪", "value]text"]);

    assert_columns(
        &catalog,
        "shop",
        "common_types",
        &[
            "id",
            "bool_value",
            "tiny_value",
            "small_value",
            "int_value",
            "big_value",
            "decimal_value",
            "money_value",
            "smallmoney_value",
            "real_value",
            "double_value",
            "fixed_value",
            "varying_value",
            "unicode_fixed_value",
            "unicode_varying_value",
            "text_value",
            "fixed_binary_value",
            "varying_binary_value",
            "binary_value",
            "date_value",
            "time_value",
            "legacy_timestamp_value",
            "small_timestamp_value",
            "timestamp_value",
            "zoned_timestamp_value",
            "uuid_value",
            "json_value",
            "xml_value",
            "variant_value",
            "row_version",
        ],
    );
    assert!(
        catalog
            .objects
            .iter()
            .all(|o| o.object.id.database == database.database())
    );
    artifact(database.database(), &catalog);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_autocomplete_returns_all_500_tables_and_columns_within_five_seconds() {
    let (database, mut session) = mssql_open(false).await;
    // `bulk` is reserved in T-SQL (BULK INSERT), so this schema has another name.
    run(&mut session, "CREATE SCHEMA many").await;
    let sql = (0..500)
        .map(|n| format!("CREATE TABLE many.bulk_{n} (zebra integer, apple integer);"))
        .collect::<String>();
    run(&mut session, &sql).await;
    let mut session = CountedSession {
        inner: &mut session,
        queries: 0,
    };
    let start = Instant::now();
    let catalog = dbide_engine::objects::mssql::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(session.queries, 2);
    assert_large_catalog(&catalog, "many", elapsed);
    artifact(database.database(), &catalog);
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_autocomplete_returns_quoted_names_without_delimiters() {
    let (database, mut session) = mssql_open(false).await;
    run(&mut session, r#"CREATE SCHEMA [odd schema];
        GO
        CREATE SCHEMA empty;
        GO
        CREATE TABLE [odd schema].[select] ([zebra snow] integer, [a]]b] nvarchar(20));
        CREATE TABLE dbo.[select] (other integer);
        GO
        CREATE VIEW [odd schema].[odd view] AS SELECT * FROM [odd schema].[select];
        GO
        CREATE FUNCTION [odd schema].[odd function](@x integer) RETURNS integer AS BEGIN RETURN @x; END;
        GO
        CREATE PROCEDURE [odd schema].[odd procedure] AS SELECT 1;"#).await;
    let catalog = dbide_engine::objects::mssql::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    assert_columns(&catalog, "odd schema", "select", &["zebra snow", "a]b"]);
    assert_columns(&catalog, "odd schema", "odd view", &["zebra snow", "a]b"]);
    assert_columns(&catalog, "odd schema", "odd function", &[]);
    assert_columns(&catalog, "odd schema", "odd procedure", &[]);
    assert!(
        catalog
            .objects
            .iter()
            .any(|o| o.object.name == "odd procedure" && o.object.kind == Kind::Procedure)
    );
    assert!(catalog.schemas.iter().any(|s| s.name == "empty"));
    assert_columns(&catalog, "dbo", "select", &["other"]);
    artifact(database.database(), &catalog);
}

async fn mysql_open(fixture: bool) -> (MyDatabase, dbide_engine::mysql::MySql) {
    let database = tokio::task::spawn_blocking(move || {
        let database = MyDatabase::start().unwrap();
        if fixture {
            database.load_script("../../testdata/mysql.sql").unwrap();
        }
        database
    })
    .await
    .unwrap();
    let mut details = Connection::new(Engine::MySql, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.tls = TlsMode::Off;
    let session = dbide_engine::mysql::connect(&details, Some(database.password()))
        .await
        .unwrap();
    (database, session)
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_autocomplete_contains_fixture_objects_and_columns_in_declaration_order() {
    let (database, mut session) = mysql_open(true).await;
    let catalog = dbide_engine::objects::mysql::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    assert_fixture(&catalog, database.database(), Kind::Procedure);
    assert_columns(
        &catalog,
        database.database(),
        "common_types",
        &[
            "id",
            "bool_value",
            "tiny_value",
            "small_value",
            "medium_value",
            "int_value",
            "big_value",
            "unsigned_value",
            "decimal_value",
            "real_value",
            "double_value",
            "fixed_value",
            "varying_value",
            "text_value",
            "fixed_binary_value",
            "varying_binary_value",
            "binary_value",
            "date_value",
            "time_value",
            "timestamp_value",
            "utc_timestamp_value",
            "year_value",
            "uuid_value",
            "json_value",
            "bits_value",
            "enum_value",
            "set_value",
        ],
    );
    assert!(
        catalog
            .objects
            .iter()
            .all(|o| o.object.id.database == database.database())
    );
    artifact(database.database(), &catalog);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_autocomplete_returns_all_500_tables_and_columns_within_five_seconds() {
    let (database, mut session) = mysql_open(false).await;
    let sql = (0..500)
        .map(|n| format!("CREATE TABLE bulk_{n} (zebra integer, apple integer);"))
        .collect::<String>();
    run(&mut session, &sql).await;
    let mut session = CountedSession {
        inner: &mut session,
        queries: 0,
    };
    let start = Instant::now();
    let catalog = dbide_engine::objects::mysql::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(session.queries, 2);
    assert_large_catalog(&catalog, database.database(), elapsed);
    artifact(database.database(), &catalog);
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_autocomplete_returns_quoted_names_without_delimiters() {
    let (database, mut session) = mysql_open(false).await;
    run(
        &mut session,
        r#"CREATE TABLE `select` (`zebra snow` integer, `a``b` text);
        CREATE VIEW `odd view` AS SELECT * FROM `select`;
        CREATE FUNCTION `odd function`(x integer) RETURNS integer DETERMINISTIC NO SQL RETURN x;
        CREATE PROCEDURE `odd procedure`() SELECT 1;"#,
    )
    .await;
    let catalog = dbide_engine::objects::mysql::autocomplete_catalog(&mut session)
        .await
        .unwrap();
    assert_columns(
        &catalog,
        database.database(),
        "select",
        &["zebra snow", "a`b"],
    );
    assert_columns(
        &catalog,
        database.database(),
        "odd view",
        &["zebra snow", "a`b"],
    );
    assert_columns(&catalog, database.database(), "odd function", &[]);
    assert_columns(&catalog, database.database(), "odd procedure", &[]);
    assert!(
        catalog
            .objects
            .iter()
            .any(|o| o.object.name == "odd procedure" && o.object.kind == Kind::Procedure)
    );
    artifact(database.database(), &catalog);
}
