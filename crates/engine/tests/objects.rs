use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::objects::postgres as objects;
use dbide_engine::objects::{
    Column, ConstraintKind, Generated, Identity, Kind, Object, Script, Search,
};
use dbide_engine::postgres::{self, Postgres};
use dbide_engine::session::{Event, Session, Value};
use dbide_test_servers::postgres::Postgres as Database;
use futures::StreamExt;

async fn database() -> Database {
    tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap()
}

fn details(database: &Database) -> Connection {
    let mut details = Connection::new(Engine::PostgreSql, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.tls = TlsMode::Off;
    details
}

async fn open(database: &Database) -> Postgres {
    postgres::connect(&details(database), Some(database.password()))
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
    .find(|o| o.name == name)
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
async fn pg_databases_include_current_database_and_exclude_templates() {
    let database = database().await;
    let mut session = open(&database).await;
    let databases = objects::databases(&mut session).await.unwrap();
    assert!(databases.iter().any(|o| o.name == database.database()));
    assert!(
        !databases
            .iter()
            .any(|o| o.name == "template0" || o.name == "template1")
    );
    assert!(databases.iter().all(|o| o.kind == Kind::Database));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_schema_lists_only_its_objects_and_flags_system_schemas() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE SCHEMA orchard; CREATE SCHEMA empty;
        CREATE TABLE orchard.apples (id integer);
        CREATE VIEW orchard.fruit AS SELECT id FROM orchard.apples;
        CREATE MATERIALIZED VIEW orchard.harvest AS SELECT id FROM orchard.apples;
        CREATE FUNCTION orchard.counts(integer) RETURNS integer LANGUAGE sql AS 'SELECT $1';
        CREATE FUNCTION orchard.counts(text) RETURNS text LANGUAGE sql AS 'SELECT $1';
        CREATE PROCEDURE orchard.pick() LANGUAGE sql AS 'SELECT 1';
        CREATE SEQUENCE orchard.numbers",
    )
    .await;
    let schemas = objects::schemas(&mut session).await.unwrap();
    assert!(schemas.iter().any(|o| o.name == "pg_catalog" && o.system));
    assert!(
        schemas
            .iter()
            .any(|o| o.name == "information_schema" && o.system)
    );
    assert!(schemas.iter().any(|o| o.name == "orchard" && !o.system));
    assert!(
        objects::objects(&mut session, "empty")
            .await
            .unwrap()
            .is_empty()
    );
    let children = objects::objects(&mut session, "orchard").await.unwrap();
    let names: Vec<_> = children.iter().map(|o| (o.name.as_str(), o.kind)).collect();
    assert_eq!(
        names,
        vec![
            ("apples", Kind::Table),
            ("counts", Kind::Function),
            ("counts", Kind::Function),
            ("fruit", Kind::View),
            ("harvest", Kind::MaterializedView),
            ("numbers", Kind::Sequence),
            ("pick", Kind::Procedure)
        ]
    );
    assert_ne!(children[1].id, children[2].id);
    assert_ne!(children[1].signature, children[2].signature);
    artifact(&database, "explorer.txt", &format!("{children:#?}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_columns_preserve_types_defaults_nullability_identity_and_generation() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TABLE public.fruit (
        id bigint GENERATED ALWAYS AS IDENTITY, name varchar(20) NOT NULL DEFAULT 'pear',
        price numeric(8,2), doubled numeric GENERATED ALWAYS AS (price * 2) STORED,
        next_id integer GENERATED BY DEFAULT AS IDENTITY,
        virtual_value integer GENERATED ALWAYS AS (1 + 2) VIRTUAL, dropped integer);
        ALTER TABLE public.fruit DROP COLUMN dropped",
    )
    .await;
    let table = object(&mut session, "fruit").await;
    assert_eq!(
        objects::columns(&mut session, &table).await.unwrap(),
        vec![
            Column {
                name: "id".into(),
                type_name: "bigint".into(),
                nullable: false,
                default: None,
                identity: Some(Identity::Always),
                generated: None
            },
            Column {
                name: "name".into(),
                type_name: "character varying(20)".into(),
                nullable: false,
                default: Some("'pear'::character varying".into()),
                identity: None,
                generated: None
            },
            Column {
                name: "price".into(),
                type_name: "numeric(8,2)".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: None
            },
            Column {
                name: "doubled".into(),
                type_name: "numeric".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: Some(Generated {
                    expression: "(price * (2)::numeric)".into(),
                    stored: true
                })
            },
            Column {
                name: "next_id".into(),
                type_name: "integer".into(),
                nullable: false,
                default: None,
                identity: Some(Identity::ByDefault),
                generated: None
            },
            Column {
                name: "virtual_value".into(),
                type_name: "integer".into(),
                nullable: true,
                default: None,
                identity: None,
                generated: Some(Generated {
                    expression: "(1 + 2)".into(),
                    stored: false
                })
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_keys_and_indexes_preserve_constraint_and_expression_metadata() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TABLE public.groves (id integer PRIMARY KEY);
        CREATE TABLE public.fruit (id integer PRIMARY KEY, name text UNIQUE,
        grove integer REFERENCES public.groves(id), CHECK (id > 0));
        CREATE INDEX fruit_lower ON public.fruit USING btree (lower(name)) INCLUDE (grove)",
    )
    .await;
    let table = object(&mut session, "fruit").await;
    let constraints = objects::constraints(&mut session, &table).await.unwrap();
    assert_eq!(
        constraints
            .iter()
            .map(|c| (c.name.as_str(), c.kind))
            .collect::<Vec<_>>(),
        vec![
            ("fruit_grove_fkey", ConstraintKind::ForeignKey),
            ("fruit_id_check", ConstraintKind::Check),
            ("fruit_name_key", ConstraintKind::Unique),
            ("fruit_pkey", ConstraintKind::PrimaryKey)
        ]
    );
    assert_eq!(
        constraints[0].definition,
        "FOREIGN KEY (grove) REFERENCES groves(id)"
    );
    let indexes = objects::indexes(&mut session, &table).await.unwrap();
    assert_eq!(indexes[0].object.name, "fruit_lower");
    assert_eq!(indexes[0].columns, vec!["lower(name)", "grove"]);
    assert_eq!(indexes[0].method, "btree");
    assert!(!indexes[0].unique);
    assert!(!indexes[0].constraint_backed);
    assert!(indexes[1].unique && indexes[1].constraint_backed);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_search_matches_literal_mixed_case_substrings_kind_and_limit() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TABLE public.\"Pear%_Box\" (id integer);
        CREATE VIEW public.\"Pear%_View\" AS SELECT 1 AS id;
        CREATE TABLE public.pear_other (id integer)",
    )
    .await;
    let search = Search {
        name: "pEaR%_".into(),
        kinds: vec![Kind::Table],
        limit: 1,
    };
    let hits = objects::search(&mut session, &search).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Pear%_Box");
    assert!(
        objects::search(&mut session, &Search { limit: 0, ..search })
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_cross_database_search_finds_second_database_and_reports_refusal() {
    let first = database().await;
    let second = database().await;
    let denied = database().await;
    let mut session = open(&first).await;
    let mut other = open(&second).await;
    run(&mut other, "CREATE TABLE public.remote_pears (id integer)").await;
    let mut connection = details(&first);
    // A real connection refusal, even though the harness user normally has superuser access.
    run(
        &mut session,
        &format!(
            "ALTER DATABASE \"{}\" ALLOW_CONNECTIONS false",
            denied.database()
        ),
    )
    .await;
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "remote_pears".into(),
            kinds: vec![Kind::Table],
            limit: 10,
        },
        |name| {
            connection.database = Some(name);
            let connection = connection.clone();
            let password = first.password().to_owned();
            async move { postgres::connect(&connection, Some(&password)).await }
        },
    )
    .await
    .unwrap();
    assert_eq!(report.objects.len(), 1);
    assert_eq!(report.objects[0].id.database, second.database());
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.database == denied.database() && !s.reason.is_empty())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_create_table_recreates_columns_constraints_indexes_and_comments() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "CREATE TABLE public.pears (id bigint GENERATED ALWAYS AS IDENTITY
        (START WITH 12 INCREMENT BY 3 CACHE 4) PRIMARY KEY,
        name text NOT NULL DEFAULT 'pear', price integer DEFAULT 1,
        doubled integer GENERATED ALWAYS AS (price * 2) STORED,
        CONSTRAINT pears_price_check CHECK (price > 0), UNIQUE (name));
        CREATE INDEX pears_price ON public.pears (price);
        COMMENT ON TABLE public.pears IS 'Pear''s table';
        COMMENT ON COLUMN public.pears.name IS 'Fruit name'",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let script = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    artifact(&source, "pears.sql", &script);
    run(&mut destination, &script).await;
    let copy = object(&mut destination, "pears").await;
    assert_eq!(
        objects::columns(&mut session, &table).await.unwrap(),
        objects::columns(&mut destination, &copy).await.unwrap()
    );
    assert_eq!(
        objects::constraints(&mut session, &table).await.unwrap(),
        objects::constraints(&mut destination, &copy).await.unwrap()
    );
    let indexes = objects::indexes(&mut destination, &copy).await.unwrap();
    assert_eq!(indexes.len(), 3);
    assert_eq!(indexes[2].object.name, "pears_price");
    assert_eq!(
        run(
            &mut destination,
            "SELECT obj_description('public.pears'::regclass),
        col_description('public.pears'::regclass, 2)"
        )
        .await,
        vec![vec![
            Value::Text("Pear's table".into()),
            Value::Text("Fruit name".into())
        ]]
    );
    assert_eq!(
        run(
            &mut destination,
            "INSERT INTO public.pears (name) VALUES ('apple') RETURNING id"
        )
        .await,
        vec![vec![Value::Integer(12)]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_create_view_and_routines_round_trip_and_offer_replace() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "CREATE VIEW public.fruit_view AS SELECT 7 AS count;
        CREATE FUNCTION public.fruit_count(integer) RETURNS integer LANGUAGE sql AS 'SELECT $1 + 2';
        CREATE PROCEDURE public.fruit_pick() LANGUAGE sql AS 'SELECT 1'",
    )
    .await;
    let view = object(&mut session, "fruit_view").await;
    let function = object(&mut session, "fruit_count").await;
    let procedure = object(&mut session, "fruit_pick").await;
    run(
        &mut destination,
        &objects::script(&mut session, &view, Script::Create)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        &objects::script(&mut session, &function, Script::Create)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        &objects::script(&mut session, &procedure, Script::Create)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        &objects::script(&mut session, &view, Script::Alter)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        &objects::script(&mut session, &function, Script::Alter)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        &objects::script(&mut session, &procedure, Script::Alter)
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        run(
            &mut destination,
            "SELECT count, public.fruit_count(count) FROM public.fruit_view"
        )
        .await,
        vec![vec![Value::Integer(7), Value::Integer(9)]]
    );
    let copy = object(&mut destination, "fruit_view").await;
    assert_eq!(
        objects::columns(&mut session, &view).await.unwrap(),
        objects::columns(&mut destination, &copy).await.unwrap()
    );
    assert!(
        run(&mut destination, "CALL public.fruit_pick()")
            .await
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_select_quotes_only_required_identifiers_and_returns_rows() {
    let database = database().await;
    let mut session = open(&database).await;
    run(&mut session, "CREATE SCHEMA \"Fruit Stand\";
        CREATE TABLE \"Fruit Stand\".\"PearBox\" (id integer, \"Fruit Name\" text, \"select\" integer);
        INSERT INTO \"Fruit Stand\".\"PearBox\" VALUES (1, 'pear', 2)").await;
    let table = object(&mut session, "PearBox").await;
    let sql = objects::script(&mut session, &table, Script::Select)
        .await
        .unwrap();
    assert_eq!(
        sql,
        "SELECT id, \"Fruit Name\", \"select\" FROM \"Fruit Stand\".\"PearBox\" LIMIT 1000;\n"
    );
    assert_eq!(
        run(&mut session, &sql).await,
        vec![vec![
            Value::Integer(1),
            Value::Text("pear".into()),
            Value::Integer(2)
        ]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_create_schema_sequence_materialized_view_and_index_round_trip() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "CREATE SCHEMA orchard;
        CREATE SEQUENCE orchard.harvest_numbers AS integer INCREMENT BY 3
            MINVALUE 2 MAXVALUE 100 START WITH 8 CACHE 4 CYCLE;
        CREATE MATERIALIZED VIEW orchard.harvest AS SELECT 7 AS count;
        CREATE UNIQUE INDEX harvest_count ON orchard.harvest (count)",
    )
    .await;
    let schema = object(&mut session, "orchard").await;
    let sequence = object(&mut session, "harvest_numbers").await;
    let view = object(&mut session, "harvest").await;
    let index = object(&mut session, "harvest_count").await;
    run(
        &mut destination,
        &objects::script(&mut session, &schema, Script::Create)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        &objects::script(&mut session, &sequence, Script::Create)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        &objects::script(&mut session, &view, Script::Create)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        &objects::script(&mut session, &index, Script::Create)
            .await
            .unwrap(),
    )
    .await;
    run(
        &mut destination,
        "REFRESH MATERIALIZED VIEW orchard.harvest",
    )
    .await;
    assert_eq!(
        run(
            &mut destination,
            "SELECT nextval('orchard.harvest_numbers'), count FROM orchard.harvest"
        )
        .await,
        vec![vec![Value::Integer(8), Value::Integer(7)]]
    );
    let copy = object(&mut destination, "harvest").await;
    assert_eq!(
        objects::columns(&mut session, &view).await.unwrap(),
        objects::columns(&mut destination, &copy).await.unwrap()
    );
    let indexes = objects::indexes(&mut destination, &copy).await.unwrap();
    assert_eq!(indexes.len(), 1);
    assert_eq!(indexes[0].columns, vec!["count"]);
    assert!(indexes[0].unique);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_table_alter_is_a_comment_and_select_respects_the_row_limit() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TABLE public.pears (id integer);
        INSERT INTO public.pears SELECT generate_series(1, 1001);
        CREATE VIEW public.pear_view AS SELECT id FROM public.pears",
    )
    .await;
    let table = object(&mut session, "pears").await;
    assert_eq!(
        objects::script(&mut session, &table, Script::Alter)
            .await
            .unwrap(),
        "-- ALTER TABLE public.pears\n--     ADD COLUMN column_name data_type;\n"
    );
    let view = object(&mut session, "pear_view").await;
    let select = objects::script(&mut session, &view, Script::Select)
        .await
        .unwrap();
    assert_eq!(select, "SELECT id FROM public.pear_view LIMIT 1000;\n");
    assert_eq!(run(&mut session, &select).await.len(), 1000);
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_names_with_quotes_and_backslashes_remain_data() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE SCHEMA \"Pear's\\shed\";
        CREATE TABLE \"Pear's\\shed\".\"box\"\"; DROP SCHEMA public; --\" (id integer)",
    )
    .await;
    run(&mut session, "SET standard_conforming_strings = off").await;
    let children = objects::objects(&mut session, "Pear's\\shed")
        .await
        .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name, "box\"; DROP SCHEMA public; --");
    let sql = objects::script(&mut session, &children[0], Script::Select)
        .await
        .unwrap();
    assert_eq!(
        sql,
        "SELECT id FROM \"Pear's\\shed\".\"box\"\"; DROP SCHEMA public; --\" LIMIT 1000;\n"
    );
    assert!(run(&mut session, &sql).await.is_empty());
    assert!(
        objects::schemas(&mut session)
            .await
            .unwrap()
            .iter()
            .any(|s| s.name == "public")
    );
    assert!(
        objects::search(
            &mut session,
            &Search {
                name: "'; DROP SCHEMA public; --".into(),
                kinds: vec![],
                limit: 10
            }
        )
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_script_rejects_missing_objects_and_identities_from_other_databases() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(&mut session, "CREATE TABLE public.pears (id integer)").await;
    let table = object(&mut session, "pears").await;
    assert!(matches!(
        objects::script(&mut destination, &table, Script::Create).await,
        Err(objects::Error::Identity)
    ));
    run(&mut session, "DROP TABLE public.pears").await;
    assert!(matches!(
        objects::script(&mut session, &table, Script::Create).await,
        Err(objects::Error::NotFound)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_create_table_preserves_owned_serial_sequence_and_foreign_key() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "CREATE TABLE public.pears (id serial PRIMARY KEY,
        parent integer REFERENCES public.pears(id) DEFERRABLE INITIALLY DEFERRED)",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let script = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    run(&mut destination, &script).await;
    let copy = object(&mut destination, "pears").await;
    assert_eq!(
        objects::columns(&mut session, &table).await.unwrap(),
        objects::columns(&mut destination, &copy).await.unwrap()
    );
    assert_eq!(
        objects::constraints(&mut session, &table).await.unwrap(),
        objects::constraints(&mut destination, &copy).await.unwrap()
    );
    assert_eq!(
        run(
            &mut destination,
            "INSERT INTO public.pears DEFAULT VALUES RETURNING id"
        )
        .await,
        vec![vec![Value::Integer(1)]]
    );
    assert_eq!(
        run(
            &mut destination,
            "SELECT pg_get_serial_sequence('public.pears', 'id')"
        )
        .await,
        vec![vec![Value::Text("public.pears_id_seq".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_create_scripts_preserve_source_name_resolution_in_another_database() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    run(
        &mut session,
        "CREATE SCHEMA orchard; CREATE SCHEMA \"Fruit Stand\";
        SET search_path = orchard, \"Fruit Stand\", public;
        CREATE TABLE groves (id integer PRIMARY KEY);
        CREATE TABLE pears (id serial PRIMARY KEY, grove integer REFERENCES groves(id));
        CREATE VIEW pear_view AS SELECT id, grove FROM pears",
    )
    .await;
    run(
        &mut destination,
        "CREATE SCHEMA orchard; CREATE SCHEMA \"Fruit Stand\";
        CREATE TABLE orchard.groves (id integer PRIMARY KEY)",
    )
    .await;
    let table = object(&mut session, "pears").await;
    let view = object(&mut session, "pear_view").await;
    let create_table = objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    let create_view = objects::script(&mut session, &view, Script::Create)
        .await
        .unwrap();
    assert!(
        create_table
            .starts_with("SET search_path = pg_catalog, orchard, \"Fruit Stand\", public;\n")
    );
    run(&mut destination, &create_table).await;
    run(&mut destination, "SET search_path = public").await;
    run(&mut destination, &create_view).await;
    run(
        &mut destination,
        "INSERT INTO orchard.groves VALUES (7);
        INSERT INTO orchard.pears (grove) VALUES (7)",
    )
    .await;
    assert_eq!(
        run(&mut destination, "SELECT id, grove FROM orchard.pear_view").await,
        vec![vec![Value::Integer(1), Value::Integer(7)]]
    );
    assert_eq!(
        run(&mut session, "SELECT current_setting('search_path')").await,
        vec![vec![Value::Text("orchard, \"Fruit Stand\", public".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_cross_database_search_applies_one_limit_to_all_databases() {
    let first = database().await;
    let second = database().await;
    let mut session = open(&first).await;
    let mut other = open(&second).await;
    run(
        &mut session,
        "CREATE TABLE public.bounded_pears (id integer)",
    )
    .await;
    run(&mut other, "CREATE TABLE public.bounded_pears (id integer)").await;
    let connection = details(&first);
    let password = first.password().to_owned();
    let report = objects::search_all(
        &mut session,
        &Search {
            name: "bounded_pears".into(),
            kinds: vec![Kind::Table],
            limit: 1,
        },
        |name| {
            let mut connection = connection.clone();
            connection.database = Some(name);
            let password = password.clone();
            async move { postgres::connect(&connection, Some(&password)).await }
        },
    )
    .await
    .unwrap();
    assert_eq!(report.objects.len(), 1);
    assert_eq!(report.objects[0].name, "bounded_pears");
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_temporary_objects_offer_select_but_reject_nonportable_ddl() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TEMP TABLE temporary_pears (id integer);
        INSERT INTO temporary_pears VALUES (7);
        CREATE TABLE public.permanent_pears (id integer)",
    )
    .await;
    let temporary = object(&mut session, "temporary_pears").await;
    assert!(matches!(
        objects::script(&mut session, &temporary, Script::Create).await,
        Err(objects::Error::TemporaryObject)
    ));
    let select = objects::script(&mut session, &temporary, Script::Select)
        .await
        .unwrap();
    assert_eq!(
        select,
        "SELECT id FROM pg_temp.temporary_pears LIMIT 1000;\n"
    );
    assert_eq!(
        run(&mut session, &select).await,
        vec![vec![Value::Integer(7)]]
    );
    let permanent = object(&mut session, "permanent_pears").await;
    let create = objects::script(&mut session, &permanent, Script::Create)
        .await
        .unwrap();
    assert!(create.starts_with("SET search_path = pg_temp, pg_catalog, public;\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_inherited_and_partition_child_tables_do_not_silently_lose_inheritance() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TABLE public.parent_pears (id integer);
        CREATE TABLE public.inherited_pears () INHERITS (public.parent_pears);
        CREATE TABLE public.partitioned_pears (id integer) PARTITION BY RANGE (id);
        CREATE TABLE public.partition_pears PARTITION OF public.partitioned_pears
            FOR VALUES FROM (1) TO (10)",
    )
    .await;
    let inherited = object(&mut session, "inherited_pears").await;
    let partition = object(&mut session, "partition_pears").await;
    assert!(matches!(
        objects::script(&mut session, &inherited, Script::Create).await,
        Err(objects::Error::InheritedTable)
    ));
    assert!(matches!(
        objects::script(&mut session, &partition, Script::Create).await,
        Err(objects::Error::InheritedTable)
    ));
    let parent = object(&mut session, "partitioned_pears").await;
    let create = objects::script(&mut session, &parent, Script::Create)
        .await
        .unwrap();
    assert!(create.contains("PARTITION BY RANGE (id)"));
}
