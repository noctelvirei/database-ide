//! Formatting as a user drives it on a tab connected to the real SQL Server, PostgreSQL and
//! MySQL servers from `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E harness"): Ctrl+K
//! Ctrl+F lays out a messy query, Ctrl+Z puts it back in one step, and a query the formatter
//! refuses stays as typed with the reason in the messages strip.

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

const MESSY: &str = "select name,Type,STATUS,uSeR from things";
const TIDY: &str = "SELECT name, Type, STATUS, uSeR FROM things";
/// A string with no closing quote, which the formatter can't tokenize.
const UNCLOSED: &str = "select 'abc";
/// A line ending in a comment; selected with its line break, it must keep the break, or
/// the second statement would join the comment.
const COMMENTED: [&str; 2] = ["select 1 -- first", "select 2"];

fn formats_and_undoes(server: &Server) {
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    server.save_as_lab(&appdata);

    let steps = json!([
        { "name": "open a tab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle"] },
        {
            "name": "format",
            "input": [{ "text": MESSY }, { "key": "ctrl-k ctrl-f" }, "idle"],
        },
        { "name": "undo", "input": [{ "key": "ctrl-z" }] },
        {
            "name": "format an unclosed string",
            "input": [
                { "key": "ctrl-a" },
                { "text": UNCLOSED },
                { "key": "delete" },
                { "key": "ctrl-k ctrl-f" },
                "idle",
            ],
        },
        {
            "name": "format a selected line",
            "input": [
                { "key": "ctrl-a" },
                { "text": COMMENTED[0] },
                { "key": "enter" },
                { "text": COMMENTED[1] },
                { "key": "ctrl-home" },
                { "key": "shift-down" },
                { "key": "ctrl-k ctrl-f" },
                "idle",
            ],
        },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));

    let opened = state(&lines, "open a tab");
    assert_eq!(opened["tabs"][1]["status"], "ready", "{opened:#}");
    let formatted = state(&lines, "format");
    assert_eq!(formatted["last_action"], "Format");
    assert_eq!(formatted["editor_text"], TIDY, "{formatted:#}");
    assert_eq!(state(&lines, "undo")["editor_text"], MESSY);

    let refused = state(&lines, "format an unclosed string");
    assert_eq!(refused["editor_text"], UNCLOSED, "{refused:#}");
    let last = refused["messages"]
        .as_array()
        .unwrap()
        .last()
        .and_then(Value::as_str)
        .unwrap();
    assert!(
        last.starts_with("Couldn't format: cannot tokenize SQL"),
        "{refused:#}"
    );

    let line = state(&lines, "format a selected line");
    assert_eq!(
        line["editor_text"], "SELECT 1 -- first\nselect 2",
        "{line:#}"
    );
}

#[test]
fn ui_mssql_format_lays_out_a_query_and_undo_restores_it() {
    let db = Mssql::start().expect("SQL Server test database");
    formats_and_undoes(&Server {
        engine: Engine::SqlServer,
        host: db.host(),
        port: db.port(),
        user: db.user(),
        password: db.password(),
        database: db.database(),
    });
}

#[test]
fn ui_pg_format_lays_out_a_query_and_undo_restores_it() {
    let db = Postgres::start().expect("PostgreSQL test server");
    formats_and_undoes(&Server {
        engine: Engine::PostgreSql,
        host: db.host(),
        port: db.port(),
        user: db.user(),
        password: db.password(),
        database: db.database(),
    });
}

#[test]
fn ui_mysql_format_lays_out_a_query_and_undo_restores_it() {
    let db = MySql::start().expect("MySQL test server");
    formats_and_undoes(&Server {
        engine: Engine::MySql,
        host: db.host(),
        port: db.port(),
        user: db.user(),
        password: db.password(),
        database: db.database(),
    });
}
