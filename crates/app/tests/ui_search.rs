//! Searching every database from the explorer as a user drives it against the real SQL Server,
//! PostgreSQL and MySQL servers from `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E
//! harness"): Ctrl+Enter in the filter box finds a table in a database the user never opened,
//! names a database the search could not open, and the hit scripts as CREATE like any table.

mod lab;
mod passwords;
mod support;

use dbide_engine::danger::Engine;
use dbide_test_servers::mssql::Mssql;
use dbide_test_servers::mysql::MySql;
use dbide_test_servers::postgres::Postgres;
use lab::Server;
use passwords::ForgetPasswords;
use serde_json::{Value, json};
use support::{run_script, state};

/// The table only the far database holds.
const TABLE: &str = "CREATE TABLE far_away_quinces (id INT PRIMARY KEY)";

/// The selected explorer row.
fn selected_row(state: &Value) -> &Value {
    state["explorer"]["rows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["selected"] == true))
        .unwrap_or_else(|| panic!("no row selected: {state:#}"))
}

/// The messages strip's lines.
fn messages(state: &Value) -> Vec<&str> {
    state["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

/// Connects to `server`, searches every database for `far_away_quinces` and scripts the hit
/// in `far` as CREATE. Returns the state after the search.
fn search_and_script(server: &Server, far: &str) -> Value {
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    server.save_as_lab(&appdata);

    let steps = json!([
        { "name": "connect", "input": [{ "key": "f8" }, { "key": "enter" }, "idle"] },
        {
            "name": "search every database",
            "input": [
                { "key": "ctrl-f" },
                { "text": "far_away_quinces" },
                { "key": "ctrl-enter" },
                "idle",
            ],
        },
        { "name": "choose the hit", "input": [{ "key": "enter" }, { "key": "down" }] },
        { "name": "item menu", "input": [{ "key": "menu" }] },
        {
            "name": "script as create",
            "input": [{ "key": "down" }, { "key": "enter" }, "idle"],
        },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));

    let searched = state(&lines, "search every database").clone();
    let rows = searched["explorer"]["rows"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["name"] == "Search: far_away_quinces" && r["kind"] == "group"),
        "{searched:#}"
    );
    assert!(
        messages(&searched)
            .iter()
            .any(|m| m.starts_with("Found ") && m.ends_with(" named like \"far_away_quinces\".")),
        "{searched:#}"
    );

    let hit = selected_row(state(&lines, "choose the hit"));
    assert_eq!(hit["kind"], "table", "{hit:#}");
    let name = hit["name"].as_str().unwrap();
    assert!(
        name.starts_with(&format!("{far} · ")) && name.ends_with("far_away_quinces"),
        "{name}"
    );

    let menu = state(&lines, "item menu");
    assert_eq!(menu["explorer"]["menu"][0], "Script as CREATE", "{menu:#}");

    let scripted = state(&lines, "script as create");
    assert_eq!(scripted["active_tab"], 1, "{scripted:#}");
    let text = scripted["editor_text"].as_str().unwrap();
    assert!(text.contains("CREATE TABLE"), "{text}");
    assert!(text.contains("far_away_quinces"), "{text}");
    searched
}

/// Whether the messages name `database` as one the search couldn't open.
fn says_skipped(state: &Value, database: &str) -> bool {
    messages(state)
        .iter()
        .any(|m| m.starts_with(&format!("Couldn't search {database}: ")))
}

#[test]
fn ui_mssql_search_finds_a_table_in_an_unopened_database() {
    let source = Mssql::start().expect("SQL Server test database");
    let far = Mssql::start().expect("SQL Server far database");
    let refused = Mssql::start().expect("SQL Server refused database");
    far.query(TABLE).unwrap();
    let login = source.user();
    far.query(&format!(
        "CREATE USER [{login}] FOR LOGIN [{login}]; ALTER ROLE db_owner ADD MEMBER [{login}];"
    ))
    .unwrap();
    let searched = search_and_script(
        &Server {
            engine: Engine::SqlServer,
            host: source.host(),
            port: source.port(),
            user: source.user(),
            password: source.password(),
            database: source.database(),
        },
        far.database(),
    );
    assert!(says_skipped(&searched, refused.database()), "{searched:#}");
}

#[test]
fn ui_pg_search_finds_a_table_in_an_unopened_database() {
    let source = Postgres::start().expect("PostgreSQL test server");
    let far = Postgres::start().expect("PostgreSQL far database");
    let refused = Postgres::start().expect("PostgreSQL refused database");
    far.query(TABLE).unwrap();
    source
        .query(&format!(
            "ALTER DATABASE \"{}\" ALLOW_CONNECTIONS false",
            refused.database()
        ))
        .unwrap();
    let searched = search_and_script(
        &Server {
            engine: Engine::PostgreSql,
            host: source.host(),
            port: source.port(),
            user: source.user(),
            password: source.password(),
            database: source.database(),
        },
        far.database(),
    );
    assert!(says_skipped(&searched, refused.database()), "{searched:#}");
}

#[test]
fn ui_mysql_search_finds_a_table_in_an_unopened_database() {
    let source = MySql::start().expect("MySQL test server");
    let far = MySql::start().expect("MySQL far database");
    far.query(TABLE).unwrap();
    search_and_script(
        &Server {
            engine: Engine::MySql,
            host: source.host(),
            port: source.port(),
            user: source.user(),
            password: source.password(),
            database: source.database(),
        },
        far.database(),
    );
}
