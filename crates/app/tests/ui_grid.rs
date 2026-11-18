//! The results grid as a user drives it against the real SQL Server, PostgreSQL and MySQL
//! servers from `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E harness"): a run returning a
//! million rows and a second result set, the keyboard to the last row, a block copied with
//! headers, and the app's memory sampled while rows arrive and after the copy.

mod lab;
mod passwords;
mod support;

use std::sync::Mutex;

use dbide_engine::danger::Engine;
use dbide_test_servers::mssql::Mssql;
use dbide_test_servers::mysql::MySql;
use dbide_test_servers::postgres::Postgres;
use lab::Server;
use passwords::ForgetPasswords;
use serde_json::{Value, json};
use support::{run_script, state};

/// The idle app's memory target (docs/spec.md, "Targets"), in MB; spilling rows to disk
/// keeps a million-row run under it too.
const MEMORY_TARGET_MB: f64 = 100.;

/// The system clipboard is one per machine, so the tests that copy take turns.
static CLIPBOARD: Mutex<()> = Mutex::new(());

/// The private working set, in MB, sampled at the mark called `name`.
fn mark_mb(lines: &[Value], name: &str) -> f64 {
    lines
        .iter()
        .find(|l| l["mark"] == name)
        .and_then(|l| l["private_ws_mb"].as_f64())
        .unwrap_or_else(|| panic!("no sample for mark {name:?}: {lines:#?}"))
}

/// Runs `query` on `server`, which returns the million rows `n, label, odd` (n from 1, label
/// "row n", odd NULL for even n) and then one row `name, n`, and checks what the user sees.
fn drive_a_million_rows(server: &Server, query: &str) {
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    server.save_as_lab(&appdata);

    let steps = json!([
        { "name": "open a tab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle"] },
        {
            "name": "while rows arrive",
            "input": [{ "text": query }, { "key": "f5" }, "running", { "mark": "while rows arrive" }],
        },
        { "name": "run", "input": ["idle"] },
        { "name": "to the results", "input": [{ "key": "f6" }, { "key": "f6" }] },
        { "name": "to the last cell", "input": [{ "key": "ctrl-end" }] },
        {
            "name": "grow a block",
            "input": [{ "key": "shift-up" }, { "key": "shift-up" }, { "key": "shift-left" }],
        },
        {
            "name": "copy with headers",
            "input": [
                { "key": "ctrl-shift-c" },
                "idle",
                { "mark": "after the copy" },
                { "shot": "block-at-the-end" },
            ],
        },
        {
            "name": "to the second result",
            "input": [{ "key": "ctrl-down" }, { "shot": "second-result" }],
        },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = {
        let _clipboard = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
        run_script(&script, Some(&appdata))
    };

    // How far the run had got when memory was sampled; a fast run may already be done.
    let arriving = &state(&lines, "while rows arrive")["results"];
    eprintln!(
        "rows when sampled: {}, complete: {}",
        arriving[0]["rows"], arriving[0]["complete"]
    );
    let ran = state(&lines, "run");
    let grids = ran["results"].as_array().unwrap();
    assert_eq!(grids.len(), 2, "{ran:#}");
    assert_eq!(grids[0]["columns"], json!(["n", "label", "odd"]));
    assert_eq!(grids[0]["rows"], 1_000_000);
    assert_eq!(grids[0]["complete"], true);
    assert_eq!(grids[1]["columns"], json!(["name", "n"]));
    assert_eq!(grids[1]["rows"], 1);
    assert_eq!(grids[1]["complete"], true);

    let entered = state(&lines, "to the results");
    assert_eq!(entered["focused_pane"], "Results", "{entered:#}");
    assert_eq!(entered["inspector"]["column"], "n");
    assert_eq!(entered["inspector"]["text"], "1");

    // The last row is n = 1,000,000, even, so its odd column is NULL.
    let last = state(&lines, "to the last cell");
    let grid = &last["results"][0];
    assert_eq!(
        grid["block"],
        json!({ "rows": { "start": 999_999, "end": 1_000_000 }, "columns": { "start": 2, "end": 3 } })
    );
    assert_eq!(grid["visible_rows"]["end"], 1_000_000, "{grid:#}");
    assert_eq!(last["inspector"]["column"], "odd");
    assert_eq!(last["inspector"]["text"], Value::Null, "{last:#}");

    let grown = state(&lines, "grow a block");
    assert_eq!(
        grown["results"][0]["block"],
        json!({ "rows": { "start": 999_997, "end": 1_000_000 }, "columns": { "start": 1, "end": 3 } })
    );

    let copied = state(&lines, "copy with headers");
    assert_eq!(
        copied["clipboard"],
        "label\todd\r\nrow 999998\t\r\nrow 999999\t999999\r\nrow 1000000\t\r\n",
        "{copied:#}"
    );
    assert_eq!(
        copied["messages"].as_array().unwrap().last().unwrap(),
        "Copied 3 rows."
    );

    let second = state(&lines, "to the second result");
    assert_eq!(second["results"][1]["focused"], true, "{second:#}");
    assert_eq!(second["inspector"]["column"], "name");
    assert_eq!(second["inspector"]["text"], "second");

    for mark in ["while rows arrive", "after the copy"] {
        let used = mark_mb(&lines, mark);
        eprintln!("{used:.1} MB private working set {mark}");
        assert!(
            used < MEMORY_TARGET_MB,
            "{used:.1} MB private working set {mark}, over the {MEMORY_TARGET_MB} MB target"
        );
    }
}

#[test]
fn ui_mssql_a_million_rows_scroll_copy_and_stay_under_the_memory_target() {
    let mssql = Mssql::start().expect("SQL Server test database");
    drive_a_million_rows(
        &Server {
            engine: Engine::SqlServer,
            host: mssql.host(),
            port: mssql.port(),
            user: mssql.user(),
            password: mssql.password(),
            database: mssql.database(),
        },
        "SELECT value AS n, CONCAT('row ', value) AS label, \
         CASE WHEN value % 2 = 0 THEN NULL ELSE value END AS odd \
         FROM GENERATE_SERIES(1, 1000000) ORDER BY value; \
         SELECT 'second' AS name, 2 AS n",
    );
}

#[test]
fn ui_pg_a_million_rows_scroll_copy_and_stay_under_the_memory_target() {
    let postgres = Postgres::start().expect("PostgreSQL test server");
    drive_a_million_rows(
        &Server {
            engine: Engine::PostgreSql,
            host: postgres.host(),
            port: postgres.port(),
            user: postgres.user(),
            password: postgres.password(),
            database: postgres.database(),
        },
        "SELECT n, 'row ' || n AS label, CASE WHEN n % 2 = 0 THEN NULL ELSE n END AS odd \
         FROM generate_series(1, 1000000) AS n; \
         SELECT 'second' AS name, 2 AS n",
    );
}

#[test]
fn ui_mysql_a_million_rows_scroll_copy_and_stay_under_the_memory_target() {
    let mysql = MySql::start().expect("MySQL test server");
    drive_a_million_rows(
        &Server {
            engine: Engine::MySql,
            host: mysql.host(),
            port: mysql.port(),
            user: mysql.user(),
            password: mysql.password(),
            database: mysql.database(),
        },
        // MySQL has no series function and caps recursion at 1000, so a million rows come
        // from six copies of the digits 0 to 9.
        "WITH d (v) AS (SELECT 0 UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 \
         UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 \
         UNION ALL SELECT 8 UNION ALL SELECT 9), \
         s (n) AS (SELECT a.v + 10 * b.v + 100 * c.v + 1000 * e.v + 10000 * f.v \
         + 100000 * g.v + 1 FROM d a, d b, d c, d e, d f, d g) \
         SELECT n, CONCAT('row ', n) AS label, CASE WHEN n % 2 = 0 THEN NULL ELSE n END AS odd \
         FROM s ORDER BY n; \
         SELECT 'second' AS name, 2 AS n",
    );
}

/// Six result sets don't fit the results pane, so it scrolls the one the keyboard moves to
/// into view; the last holds a 200-line value, which the inspector scrolls from the keyboard.
#[test]
fn ui_pg_results_scroll_to_the_focused_grid_and_the_inspector_scrolls_a_long_value() {
    let postgres = Postgres::start().expect("PostgreSQL test server");
    let server = Server {
        engine: Engine::PostgreSql,
        host: postgres.host(),
        port: postgres.port(),
        user: postgres.user(),
        password: postgres.password(),
        database: postgres.database(),
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    server.save_as_lab(&appdata);

    let query = "SELECT 1 AS a; SELECT 2 AS a; SELECT 3 AS a; SELECT 4 AS a; SELECT 5 AS a; \
                 SELECT string_agg('line ' || g, chr(10)) AS long FROM generate_series(1, 200) AS g";
    let down = json!({ "key": "ctrl-down" });
    let steps = json!([
        { "name": "open a tab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle"] },
        { "name": "run", "input": [{ "text": query }, { "key": "f5" }, "idle"] },
        { "name": "to the results", "input": [{ "key": "f6" }, { "key": "f6" }] },
        {
            "name": "to the last result",
            "input": [down, down, down, down, down, { "shot": "last-result" }],
        },
        { "name": "to the inspector", "input": [{ "key": "f6" }] },
        { "name": "to the value's end", "input": [{ "key": "end" }, { "shot": "value-end" }] },
        { "name": "back to its start", "input": [{ "key": "home" }] },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));

    let first = state(&lines, "to the results");
    assert_eq!(first["results"].as_array().unwrap().len(), 6, "{first:#}");
    assert_eq!(first["results"][0]["on_screen"], true);
    assert_eq!(first["results"][5]["on_screen"], false, "{first:#}");

    let last = state(&lines, "to the last result");
    assert_eq!(last["results"][5]["focused"], true);
    assert_eq!(last["results"][5]["on_screen"], true, "{last:#}");
    assert_eq!(last["results"][0]["on_screen"], false, "{last:#}");
    assert_eq!(last["inspector"]["column"], "long");

    let inspecting = state(&lines, "to the inspector");
    assert_eq!(inspecting["focused_pane"], "Inspector");
    assert_eq!(inspecting["inspector_scrolled"], 0.0);
    let scrolled = state(&lines, "to the value's end")["inspector_scrolled"]
        .as_f64()
        .unwrap();
    assert!(
        scrolled > 1000.0,
        "200 lines scroll further than {scrolled} px"
    );
    assert_eq!(
        state(&lines, "back to its start")["inspector_scrolled"],
        0.0
    );
}
