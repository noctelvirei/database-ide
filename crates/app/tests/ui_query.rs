//! Query tabs as a user drives them against the real SQL Server, PostgreSQL and MySQL servers
//! from `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E harness"): open a tab on a saved
//! connection, run a batch with an error and jump to its line, cancel a long query, decline a
//! DELETE with no WHERE, and switch database.
//!
//! The connection and its password are saved before the app starts, in a temp APPDATA and
//! under the tests' Credential Manager service, which the test empties afterwards. Scripts are
//! written at run time, in a temp folder, because they hold the servers' generated details.

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

/// One server, and what the script types on it.
struct Lab<'a> {
    server: Server<'a>,
    /// The line between `SELECT 1;` and the bad statement: `GO` on SQL Server, so the error
    /// is in the second batch and its line still counts from the top of the tab.
    separator: &'a str,
    /// A query that runs for ten minutes unless cancelled.
    long_query: &'a str,
    /// A database the test login can switch to.
    other_database: &'a str,
}

/// Keys that type `lines` into the editor, one Enter between each.
fn type_lines(lines: &[&str]) -> Vec<Value> {
    let mut keys = Vec::new();
    for (ix, line) in lines.iter().enumerate() {
        if ix > 0 {
            keys.push(json!({ "key": "enter" }));
        }
        if !line.is_empty() {
            keys.push(json!({ "text": line }));
        }
    }
    keys
}

/// The newest message starting with `prefix`, and its index in the strip.
fn newest_starting(state: &Value, prefix: &str) -> (usize, String) {
    let messages = state["messages"].as_array().unwrap();
    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(ix, m)| {
            let text = m.as_str().unwrap();
            text.starts_with(prefix).then(|| (ix, text.to_owned()))
        })
        .unwrap_or_else(|| panic!("no message starting {prefix:?}: {state:#}"))
}

fn last_message(state: &Value) -> &str {
    state["messages"]
        .as_array()
        .unwrap()
        .last()
        .and_then(Value::as_str)
        .unwrap()
}

/// The tab the editor shows.
fn active_tab(state: &Value) -> &Value {
    &state["tabs"][state["active_tab"].as_u64().unwrap() as usize]
}

/// Plays the whole query-tab script on `lab` and checks each step. Returns the state lines.
fn drive_a_query_tab(lab: &Lab) -> Vec<Value> {
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    lab.server.save_as_lab(&appdata);

    let mut error_batch = type_lines(&["SELECT 1;", lab.separator, "SELECT * FROM WHERE;"]);
    error_batch.extend([json!({ "key": "f5" }), json!("idle")]);
    let steps = json!([
        { "name": "open a tab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle"] },
        { "name": "run with an error", "input": error_batch },
        { "name": "caret to the top", "input": [{ "key": "ctrl-home" }] },
        { "name": "choose the error", "input": [{ "key": "f6" }, { "key": "up" }] },
        { "name": "jump to the error", "input": [{ "key": "enter" }] },
        {
            "name": "run the selection",
            "input": [{ "key": "ctrl-end" }, { "key": "shift-home" }, { "key": "f5" }, "idle"],
        },
        {
            "name": "start a long query",
            "input": [{ "key": "ctrl-a" }, { "text": lab.long_query }, { "key": "f5" }, "running"],
        },
        { "name": "cancel it", "input": [{ "key": "alt-pause" }, "idle"] },
        {
            "name": "run after the cancel",
            "input": [{ "key": "ctrl-a" }, { "text": "SELECT 1" }, { "key": "f5" }, "idle"],
        },
        {
            "name": "ask before a DELETE",
            "input": [{ "key": "ctrl-a" }, { "text": "DELETE FROM kept" }, { "key": "f5" }],
        },
        { "name": "decline the DELETE", "input": [{ "key": "escape" }, "idle"] },
        { "name": "list databases", "input": [{ "key": "ctrl-u" }, "idle"] },
        {
            "name": "switch database",
            "input": [{ "text": lab.other_database }, { "key": "enter" }, "idle"],
        },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));
    for line in &lines {
        assert!(
            !line.to_string().contains(lab.server.password),
            "a state line shows the password"
        );
    }

    let opened = state(&lines, "open a tab");
    assert_eq!(opened["tabs"].as_array().unwrap().len(), 2, "{opened:#}");
    assert_eq!(opened["active_tab"], 1);
    let tab = active_tab(opened);
    assert_eq!(tab["connection"], "Lab");
    assert_eq!(
        tab["colour"], "Red",
        "the tab wears its connection's colour"
    );
    assert_eq!(tab["status"], "ready", "{opened:#}");
    assert_eq!(tab["database"], lab.server.database);
    assert_eq!(opened["focused_pane"], "Editor");

    let errored = state(&lines, "run with an error");
    let (error_ix, _) = newest_starting(errored, "Line 3: ");
    // The bad statement stops the parser, so the run says the check was by keywords only.
    newest_starting(errored, "The danger check couldn't parse this batch");
    assert_eq!(active_tab(errored)["status"], "ready");

    assert_eq!(
        active_tab(state(&lines, "caret to the top"))["cursor_line"],
        1
    );
    let chosen = state(&lines, "choose the error");
    assert_eq!(chosen["focused_pane"], "Messages");
    assert_eq!(chosen["chosen_message"], error_ix);
    let jumped = state(&lines, "jump to the error");
    assert_eq!(jumped["focused_pane"], "Editor");
    assert_eq!(active_tab(jumped)["cursor_line"], 3);

    // The selection starts on line 3, so its error is reported on line 3, not line 1.
    let selected = state(&lines, "run the selection");
    let (selected_ix, _) = newest_starting(selected, "Line ");
    let (line_3_ix, _) = newest_starting(selected, "Line 3: ");
    assert!(selected_ix > error_ix, "{selected:#}");
    assert_eq!(selected_ix, line_3_ix, "{selected:#}");

    let started = state(&lines, "start a long query");
    assert_eq!(active_tab(started)["status"], "running", "{started:#}");
    let cancelled = state(&lines, "cancel it");
    assert_eq!(active_tab(cancelled)["status"], "ready", "{cancelled:#}");
    newest_starting(cancelled, "Cancelled after ");
    // The session came back from the cancel and runs the next batch.
    let after = state(&lines, "run after the cancel");
    assert!(last_message(after).starts_with("Finished in "), "{after:#}");

    let asking = state(&lines, "ask before a DELETE");
    assert_eq!(asking["tab_dialog"]["dialog"], "danger", "{asking:#}");
    assert_eq!(asking["tab_dialog"]["statements"][0]["line"], 1);
    let declined = state(&lines, "decline the DELETE");
    assert_eq!(declined["tab_dialog"], Value::Null);
    assert_eq!(declined["focused_pane"], "Editor");

    let picking = state(&lines, "list databases");
    assert_eq!(picking["tab_dialog"]["dialog"], "picker", "{picking:#}");
    let switched = state(&lines, "switch database");
    assert_eq!(
        active_tab(switched)["database"],
        lab.other_database,
        "{switched:#}"
    );
    assert_eq!(
        last_message(switched),
        format!("Switched to {}.", lab.other_database)
    );
    lines
}

const SEED: &str = "CREATE TABLE kept (id int); INSERT INTO kept VALUES (1);";
const COUNT: &str = "SELECT count(*) FROM kept;";

#[test]
fn ui_mssql_a_query_tab_runs_jumps_to_errors_cancels_asks_and_switches() {
    let mssql = Mssql::start().expect("SQL Server test database");
    mssql.query(SEED).unwrap();
    drive_a_query_tab(&Lab {
        server: Server {
            engine: Engine::SqlServer,
            host: mssql.host(),
            port: mssql.port(),
            user: mssql.user(),
            password: mssql.password(),
            database: mssql.database(),
        },
        separator: "GO",
        long_query: "WAITFOR DELAY '00:10:00'",
        other_database: "master",
    });
    assert_eq!(mssql.query(COUNT).unwrap().trim(), "1", "the DELETE ran");
}

#[test]
fn ui_pg_a_query_tab_runs_jumps_to_errors_cancels_asks_and_switches() {
    let postgres = Postgres::start().expect("PostgreSQL test server");
    postgres.query(SEED).unwrap();
    drive_a_query_tab(&Lab {
        server: Server {
            engine: Engine::PostgreSql,
            host: postgres.host(),
            port: postgres.port(),
            user: postgres.user(),
            password: postgres.password(),
            database: postgres.database(),
        },
        separator: "",
        long_query: "SELECT pg_sleep(600)",
        other_database: "postgres",
    });
    assert_eq!(postgres.query(COUNT).unwrap().trim(), "1", "the DELETE ran");
}

#[test]
fn ui_mysql_a_query_tab_runs_jumps_to_errors_cancels_asks_and_switches() {
    let mysql = MySql::start().expect("MySQL test server");
    mysql.query(SEED).unwrap();
    drive_a_query_tab(&Lab {
        server: Server {
            engine: Engine::MySql,
            host: mysql.host(),
            port: mysql.port(),
            user: mysql.user(),
            password: mysql.password(),
            database: mysql.database(),
        },
        separator: "",
        long_query: "SELECT SLEEP(600)",
        other_database: "information_schema",
    });
    assert_eq!(mysql.query(COUNT).unwrap().trim(), "1", "the DELETE ran");
}
