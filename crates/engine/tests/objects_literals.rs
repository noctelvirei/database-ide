use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::objects::postgres as objects;
use dbide_engine::objects::{Object, Script, Search};
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

async fn table_script(session: &mut Postgres, strings: &str) -> String {
    run(session, strings).await;
    run(
        session,
        r"CREATE TABLE public.pears (name text DEFAULT E'pear\\tree',
            CONSTRAINT pear_name CHECK (name <> E'bad\\fruit'));
        CREATE INDEX pear_expression ON public.pears ((name || E'\\index'));
        CREATE POLICY pear_policy ON public.pears USING (name <> E'bad\\policy');
        COMMENT ON TABLE public.pears IS E'Pear''s\\table'",
    )
    .await;
    let table = object(session, "pears").await;
    let script = objects::script(session, &table, Script::Create)
        .await
        .unwrap();
    assert!(script.contains(r"DEFAULT E'pear\\tree'::text"));
    assert!(!script.contains("SET standard_conforming_strings"));
    script
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_create_table_batch_preserves_backslash_default_with_destination_strings_off() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    let script = table_script(&mut session, "SET standard_conforming_strings = on").await;
    artifact(&source, "pears-backslash.sql", &script);
    run(&mut destination, "SET standard_conforming_strings = off").await;
    run(&mut destination, &script).await;
    assert_eq!(
        run(
            &mut destination,
            "INSERT INTO public.pears DEFAULT VALUES RETURNING name"
        )
        .await,
        vec![vec![Value::Text(r"pear\tree".into())]]
    );
    assert_eq!(
        run(
            &mut destination,
            "SELECT obj_description('public.pears'::regclass)"
        )
        .await,
        vec![vec![Value::Text(r"Pear's\table".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_create_table_batch_preserves_backslash_default_with_destination_strings_on() {
    let source = database().await;
    let target = database().await;
    let mut session = open(&source).await;
    let mut destination = open(&target).await;
    let script = table_script(&mut session, "SET standard_conforming_strings = off").await;
    artifact(&source, "pears-backslash.sql", &script);
    run(&mut destination, "SET standard_conforming_strings = on").await;
    run(&mut destination, &script).await;
    assert_eq!(
        run(
            &mut destination,
            "INSERT INTO public.pears DEFAULT VALUES RETURNING name"
        )
        .await,
        vec![vec![Value::Text(r"pear\tree".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_scripting_preserves_the_users_session_string_setting() {
    let database = database().await;
    let mut session = open(&database).await;
    table_script(&mut session, "SET standard_conforming_strings = on").await;
    assert_eq!(
        run(&mut session, "SHOW standard_conforming_strings").await,
        vec![vec![Value::Text("on".into())]]
    );
    run(
        &mut session,
        "BEGIN; SET LOCAL standard_conforming_strings = off",
    )
    .await;
    let table = object(&mut session, "pears").await;
    objects::script(&mut session, &table, Script::Create)
        .await
        .unwrap();
    assert_eq!(
        run(&mut session, "SHOW standard_conforming_strings").await,
        vec![vec![Value::Text("off".into())]]
    );
    run(&mut session, "ROLLBACK").await;
    assert_eq!(
        run(&mut session, "SHOW standard_conforming_strings").await,
        vec![vec![Value::Text("on".into())]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_scripting_errors_preserve_the_users_session_string_setting() {
    let database = database().await;
    let mut session = open(&database).await;
    run(
        &mut session,
        "CREATE TABLE public.parent (id integer);
        CREATE TABLE public.child () INHERITS (public.parent)",
    )
    .await;
    run(&mut session, "SET standard_conforming_strings = on").await;
    let table = object(&mut session, "child").await;
    assert!(matches!(
        objects::script(&mut session, &table, Script::Create).await,
        Err(objects::Error::InheritedTable)
    ));
    assert_eq!(
        run(&mut session, "SHOW standard_conforming_strings").await,
        vec![vec![Value::Text("on".into())]]
    );
}
