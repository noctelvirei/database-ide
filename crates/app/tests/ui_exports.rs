//! Exports as a user drives them against the real SQL Server, PostgreSQL and MySQL servers
//! from `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E harness"): Ctrl+S on a result grid
//! saves it as CSV, XLSX and INSERT statements, and with results to file on (Ctrl+Shift+F)
//! F5 streams the run straight into a file, leaving the grid empty.

mod lab;
mod passwords;
mod support;

use std::path::Path;

use dbide_engine::danger::Engine;
use dbide_test_servers::mssql::Mssql;
use dbide_test_servers::mysql::MySql;
use dbide_test_servers::postgres::Postgres;
use lab::Server;
use passwords::ForgetPasswords;
use serde_json::{Value, json};
use support::{run_script, state};

const TABLE: &str = "CREATE TABLE fruit (id INT PRIMARY KEY, name VARCHAR(40) NULL)";
/// A comma, both quote characters and a NULL: the values CSV and INSERT escaping must get right.
const ROWS: &str =
    "INSERT INTO fruit (id, name) VALUES (1, 'apple'), (2, 'it''s, \"ripe\"'), (3, NULL)";
const QUERY: &str = "SELECT id, name FROM fruit ORDER BY id";
/// The CSV of `QUERY`: a byte-order mark for Excel, CRLF records, NULL as an empty field.
const CSV: &str = "\u{feff}id,name\r\n1,apple\r\n2,\"it's, \"\"ripe\"\"\"\r\n3,\r\n";

/// The test database, as the checks after the script need it.
struct Database<'a> {
    /// Runs SQL and returns its rows as text, one per line, without headers.
    query: &'a dyn Fn(&str) -> String,
    /// Runs a SQL script file.
    load: &'a dyn Fn(&Path),
}

fn exports(server: &Server, db: Database) {
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    server.save_as_lab(&appdata);
    let csv = dir.path().join("fruit.csv");
    let xlsx = dir.path().join("fruit.xlsx");
    let sql = dir.path().join("fruit.sql");
    let streamed = dir.path().join("streamed.csv");
    // An existing file is replaced, as the save dialog's overwrite question promises.
    std::fs::write(&csv, "old").unwrap();

    let steps = json!([
        { "name": "open a tab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle"] },
        { "name": "run", "input": [{ "text": QUERY }, { "key": "f5" }, "idle"] },
        { "name": "to the results", "input": [{ "key": "f6" }, { "key": "f6" }] },
        { "name": "save as csv", "input": [{ "path": csv }, { "key": "ctrl-s" }, "idle"] },
        { "name": "save as xlsx", "input": [{ "path": xlsx }, { "key": "ctrl-s" }, "idle"] },
        { "name": "save as inserts", "input": [{ "path": sql }, { "key": "ctrl-s" }, "idle"] },
        { "name": "results to file", "input": [{ "key": "ctrl-shift-f" }] },
        {
            "name": "run to file",
            "input": [{ "path": streamed }, { "key": "f5" }, "idle"],
        },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));

    let ran = state(&lines, "run");
    assert_eq!(ran["results"][0]["rows"], 3, "{ran:#}");

    let saved = state(&lines, "save as csv");
    assert_eq!(std::fs::read_to_string(&csv).unwrap(), CSV);
    assert_eq!(
        last_message(saved),
        format!("Wrote 3 rows to {}.", csv.display()),
        "{saved:#}"
    );

    let workbook = std::fs::read(&xlsx).expect("the workbook");
    assert_eq!(&workbook[..2], b"PK", "an .xlsx file is a zip archive");
    assert_eq!(
        last_message(state(&lines, "save as xlsx")),
        format!("Wrote 3 rows to {}.", xlsx.display())
    );

    // The INSERT script puts back exactly the rows it was saved from.
    assert_eq!(
        last_message(state(&lines, "save as inserts")),
        format!("Wrote 3 rows to {}.", sql.display())
    );
    (db.query)("DELETE FROM fruit");
    (db.load)(&sql);
    assert_eq!((db.query)("SELECT COUNT(*) FROM fruit").trim(), "3");
    assert_eq!(
        (db.query)("SELECT COUNT(*) FROM fruit WHERE id = 2 AND name = 'it''s, \"ripe\"'").trim(),
        "1"
    );
    assert_eq!(
        (db.query)("SELECT COUNT(*) FROM fruit WHERE name IS NULL").trim(),
        "1"
    );

    let toggled = state(&lines, "results to file");
    let tab = &toggled["tabs"][toggled["active_tab"].as_u64().unwrap() as usize];
    assert_eq!(tab["results_to_file"], true, "{toggled:#}");

    // The run's one result set lands in `streamed-1.csv`, the same bytes as the saved grid.
    let streamed_run = state(&lines, "run to file");
    let first = dir.path().join("streamed-1.csv");
    assert_eq!(std::fs::read_to_string(&first).unwrap(), CSV);
    assert_eq!(
        streamed_run["results"].as_array().unwrap().len(),
        0,
        "{streamed_run:#}"
    );
    let messages = streamed_run["messages"].as_array().unwrap();
    assert!(
        messages.contains(&json!(format!("Wrote 3 rows to {}.", first.display()))),
        "{streamed_run:#}"
    );
}

fn last_message(state: &Value) -> &str {
    state["messages"]
        .as_array()
        .unwrap()
        .last()
        .and_then(Value::as_str)
        .unwrap()
}

#[test]
fn ui_mssql_results_save_as_csv_xlsx_and_inserts_and_run_to_file() {
    let db = Mssql::start().expect("SQL Server test database");
    db.query(TABLE).unwrap();
    db.query(ROWS).unwrap();
    exports(
        &Server {
            engine: Engine::SqlServer,
            host: db.host(),
            port: db.port(),
            user: db.user(),
            password: db.password(),
            database: db.database(),
        },
        Database {
            query: &|sql| db.query(sql).unwrap(),
            load: &|path| db.load_script(path).unwrap(),
        },
    );
}

#[test]
fn ui_pg_results_save_as_csv_xlsx_and_inserts_and_run_to_file() {
    let db = Postgres::start().expect("PostgreSQL test server");
    db.query(TABLE).unwrap();
    db.query(ROWS).unwrap();
    exports(
        &Server {
            engine: Engine::PostgreSql,
            host: db.host(),
            port: db.port(),
            user: db.user(),
            password: db.password(),
            database: db.database(),
        },
        Database {
            query: &|sql| db.query(sql).unwrap(),
            load: &|path| db.load_script(path).unwrap(),
        },
    );
}

#[test]
fn ui_mysql_results_save_as_csv_xlsx_and_inserts_and_run_to_file() {
    let db = MySql::start().expect("MySQL test server");
    db.query(TABLE).unwrap();
    db.query(ROWS).unwrap();
    exports(
        &Server {
            engine: Engine::MySql,
            host: db.host(),
            port: db.port(),
            user: db.user(),
            password: db.password(),
            database: db.database(),
        },
        Database {
            query: &|sql| db.query(sql).unwrap(),
            load: &|path| db.load_script(path).unwrap(),
        },
    );
}
