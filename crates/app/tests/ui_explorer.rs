//! The object explorer as a user drives it against the real SQL Server, PostgreSQL and MySQL
//! servers from `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E harness"): open a connection
//! and a database, filter to a table, script it as CREATE into a new query tab, and run that
//! script in a second, scratch database.

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

/// The table the source database holds and the test scripts.
const TABLE: &str = "CREATE TABLE pears (id INT PRIMARY KEY, name VARCHAR(40) NOT NULL)";

/// The selected explorer row.
fn selected_row(state: &Value) -> &Value {
    state["explorer"]["rows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["selected"] == true))
        .unwrap_or_else(|| panic!("no row selected: {state:#}"))
}

/// Scripts `pears` from `server`'s database as CREATE and runs the script in `scratch`.
/// Returns the state lines; the caller checks the table now exists in `scratch`.
fn script_a_table_into(server: &Server, scratch: &str) -> Vec<Value> {
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    server.save_as_lab(&appdata);

    let steps = json!([
        { "name": "connect", "input": [{ "key": "f8" }, { "key": "enter" }, "idle"] },
        { "name": "open the connection", "input": [{ "key": "right" }, "idle"] },
        {
            "name": "filter to the database",
            "input": [{ "key": "ctrl-f" }, { "text": server.database }, { "key": "enter" }],
        },
        { "name": "open the database", "input": [{ "key": "right" }, "idle"] },
        {
            "name": "filter to the table",
            "input": [
                { "key": "ctrl-f" },
                { "key": "ctrl-a" },
                { "text": "pears" },
                { "key": "enter" },
            ],
        },
        { "name": "open the table", "input": [{ "key": "right" }, "idle"] },
        {
            "name": "database menu",
            "input": [{ "key": "up" }, { "key": "up" }, { "key": "menu" }],
        },
        {
            "name": "refresh the database",
            "input": [{ "key": "down" }, { "key": "enter" }, "idle"],
        },
        { "name": "back to the table", "input": [{ "key": "down" }, { "key": "down" }] },
        { "name": "item menu", "input": [{ "key": "menu" }] },
        {
            "name": "script as create",
            "input": [{ "key": "down" }, { "key": "enter" }, "idle"],
        },
        { "name": "list databases", "input": [{ "key": "ctrl-u" }, "idle"] },
        {
            "name": "switch to the scratch database",
            "input": [{ "text": scratch }, { "key": "enter" }, "idle"],
        },
        { "name": "run the script", "input": [{ "key": "f5" }, "idle"] },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));

    let opened = state(&lines, "open the connection");
    let database = opened["explorer"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == server.database);
    assert_eq!(
        database.map(|r| &r["kind"]),
        Some(&json!("database")),
        "{opened:#}"
    );

    let filtered = state(&lines, "filter to the database");
    assert_eq!(selected_row(filtered)["name"], server.database);
    assert_eq!(filtered["focused_pane"], "Explorer", "{filtered:#}");

    let found = state(&lines, "filter to the table");
    let table = selected_row(found);
    assert_eq!(table["kind"], "table", "{found:#}");
    assert!(
        table["name"].as_str().unwrap().ends_with("pears"),
        "{found:#}"
    );

    // The table lists its primary key's index, and lists it again after a refresh of its
    // database, which reloads every open row.
    let has_index = |state: &Value| {
        state["explorer"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "index")
    };
    let table_open = state(&lines, "open the table");
    assert!(has_index(table_open), "{table_open:#}");
    let database_menu = state(&lines, "database menu");
    assert_eq!(
        database_menu["explorer"]["menu"],
        json!(["Refresh"]),
        "{database_menu:#}"
    );
    let refreshed = state(&lines, "refresh the database");
    assert_eq!(selected_row(refreshed)["kind"], "database", "{refreshed:#}");
    assert!(has_index(refreshed), "{refreshed:#}");
    assert_eq!(
        selected_row(state(&lines, "back to the table"))["kind"],
        "table"
    );

    let menu = state(&lines, "item menu");
    assert_eq!(menu["explorer"]["menu"][0], "Script as CREATE", "{menu:#}");

    let scripted = state(&lines, "script as create");
    assert_eq!(
        scripted["tabs"].as_array().unwrap().len(),
        2,
        "{scripted:#}"
    );
    assert_eq!(scripted["active_tab"], 1);
    assert_eq!(scripted["explorer"]["menu"], Value::Null);
    let text = scripted["editor_text"].as_str().unwrap();
    assert!(text.contains("CREATE TABLE"), "{text}");

    let ran = state(&lines, "run the script");
    let messages = ran["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m == &json!(format!("Switched to {scratch}."))),
        "{ran:#}"
    );
    assert!(
        messages
            .last()
            .and_then(Value::as_str)
            .is_some_and(|m| m.starts_with("Finished")),
        "{ran:#}"
    );
    lines
}

#[test]
fn ui_mssql_a_table_scripted_as_create_runs_in_another_database() {
    let source = Mssql::start().expect("SQL Server test database");
    let scratch = Mssql::start().expect("SQL Server scratch database");
    source.query(TABLE).unwrap();
    // The source's login, which the app signs in with, needs a user in the scratch database.
    let login = source.user();
    scratch
        .query(&format!(
            "CREATE USER [{login}] FOR LOGIN [{login}]; ALTER ROLE db_owner ADD MEMBER [{login}];"
        ))
        .unwrap();
    script_a_table_into(
        &Server {
            engine: Engine::SqlServer,
            host: source.host(),
            port: source.port(),
            user: source.user(),
            password: source.password(),
            database: source.database(),
        },
        scratch.database(),
    );
    assert_eq!(
        scratch
            .query("SELECT COUNT(*) FROM sys.tables WHERE name = 'pears'")
            .unwrap()
            .trim(),
        "1"
    );
}

#[test]
fn ui_pg_a_table_scripted_as_create_runs_in_another_database() {
    let source = Postgres::start().expect("PostgreSQL test server");
    let scratch = Postgres::start().expect("PostgreSQL scratch database");
    source.query(TABLE).unwrap();
    script_a_table_into(
        &Server {
            engine: Engine::PostgreSql,
            host: source.host(),
            port: source.port(),
            user: source.user(),
            password: source.password(),
            database: source.database(),
        },
        scratch.database(),
    );
    assert_eq!(
        scratch
            .query("SELECT count(*) FROM pg_tables WHERE tablename = 'pears'")
            .unwrap()
            .trim(),
        "1"
    );
}

#[test]
fn ui_mysql_a_table_scripted_as_create_runs_in_another_database() {
    let source = MySql::start().expect("MySQL test server");
    let scratch = MySql::start().expect("MySQL scratch database");
    source.query(TABLE).unwrap();
    script_a_table_into(
        &Server {
            engine: Engine::MySql,
            host: source.host(),
            port: source.port(),
            user: source.user(),
            password: source.password(),
            database: source.database(),
        },
        scratch.database(),
    );
    assert_eq!(
        scratch
            .query(
                "SELECT COUNT(*) FROM information_schema.tables \
                 WHERE table_schema = DATABASE() AND table_name = 'pears'"
            )
            .unwrap()
            .trim(),
        "1"
    );
}
