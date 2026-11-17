use dbide_engine::connections::{Connection, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::mysql::{self, MySql};
use dbide_engine::objects::mysql as objects;
use dbide_engine::objects::{Object, Search};
use dbide_engine::session::{Event, Session, Value};
use dbide_test_servers::mysql::MySql as Database;
use futures::StreamExt;

pub async fn database() -> Database {
    tokio::task::spawn_blocking(Database::start)
        .await
        .unwrap()
        .unwrap()
}

pub async fn fixture() -> Database {
    let database = database().await;
    tokio::task::spawn_blocking(move || {
        database.load_script("../../testdata/mysql.sql").unwrap();
        database
    })
    .await
    .unwrap()
}

pub fn details(database: &Database) -> Connection {
    let mut details = Connection::new(Engine::MySql, database.host());
    details.port = Some(database.port());
    details.user = Some(database.user().into());
    details.database = Some(database.database().into());
    details.tls = TlsMode::Off;
    details
}

pub async fn open(database: &Database) -> MySql {
    mysql::connect(&details(database), Some(database.password()))
        .await
        .unwrap()
}

pub async fn run(session: &mut dyn Session, sql: &str) -> Vec<Vec<Value>> {
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

pub async fn object(session: &mut dyn Session, name: &str) -> Object {
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

pub fn artifact(database: &Database, name: &str, contents: &str) {
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("../../target"));
    let folder = root.join("test-artifacts").join(database.database());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), contents).unwrap();
}
