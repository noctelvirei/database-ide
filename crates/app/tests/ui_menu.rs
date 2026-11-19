//! The menu bar as a user drives it, against the real PostgreSQL server from
//! `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E harness"): Alt+letter opens each menu,
//! Left and Right move between them, Query runs and toggles, greyed items follow the run,
//! Help lists the keyboard shortcuts, and a grid keeps its keys after Edit.

mod lab;
mod passwords;
mod support;

use dbide_engine::danger::Engine;
use dbide_test_servers::postgres::Postgres;
use lab::Server;
use passwords::ForgetPasswords;
use serde_json::{Value, json};
use support::{run_script, state};

/// The open menu's title and its items as `label`, `label (greyed)` or `label [x]`/`[ ]`.
fn menu(state: &Value) -> (Value, Vec<String>) {
    let bar = &state["menu_bar"];
    let items = bar["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let label = item["label"].as_str().unwrap();
            match (item["enabled"].as_bool().unwrap(), &item["checked"]) {
                (false, _) => format!("{label} (greyed)"),
                (true, Value::Bool(true)) => format!("{label} [x]"),
                (true, Value::Bool(false)) => format!("{label} [ ]"),
                (true, _) => label.to_owned(),
            }
        })
        .collect();
    (bar["open"].clone(), items)
}

fn active_tab(state: &Value) -> &Value {
    &state["tabs"][state["active_tab"].as_u64().unwrap() as usize]
}

#[test]
fn ui_pg_menus_open_by_alt_letter_run_queries_and_list_the_shortcuts() {
    let db = Postgres::start().expect("PostgreSQL test server");
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    Server {
        engine: Engine::PostgreSql,
        host: db.host(),
        port: db.port(),
        user: db.user(),
        password: db.password(),
        database: db.database(),
    }
    .save_as_lab(&appdata);

    let steps = json!([
        // Lab is selected in the explorer, but focus is back on the unconnected first tab.
        {
            "name": "new query from file",
            "input": [
                "idle",
                { "key": "f8" },
                { "key": "shift-f6" },
                { "key": "alt-f" },
                { "key": "down" },
                { "key": "enter" },
                "idle",
            ],
        },
        { "name": "open a tab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle", { "text": "SELECT g FROM generate_series(1, 3) AS g" }] },
        { "name": "file", "input": [{ "key": "alt-f" }, { "shot": "file-menu" }] },
        { "name": "edit", "input": [{ "key": "escape" }, { "key": "alt-e" }] },
        { "name": "view", "input": [{ "key": "escape" }, { "key": "alt-v" }] },
        { "name": "query", "input": [{ "key": "escape" }, { "key": "alt-q" }] },
        { "name": "help", "input": [{ "key": "escape" }, { "key": "alt-h" }] },
        { "name": "right of help", "input": [{ "key": "right" }] },
        { "name": "left of file", "input": [{ "key": "left" }] },
        { "name": "closed", "input": [{ "key": "escape" }] },
        { "name": "f10", "input": [{ "key": "f10" }] },
        {
            "name": "query, run",
            "input": [{ "key": "escape" }, { "key": "alt-q" }, { "key": "down" }, { "key": "enter" }, "idle"],
        },
        { "name": "query after the run", "input": [{ "key": "alt-q" }] },
        {
            "name": "tick include actual plan",
            "input": [{ "key": "down" }, { "key": "down" }, { "key": "down" }, { "key": "enter" }],
        },
        { "name": "query with the plan on", "input": [{ "key": "alt-q" }] },
        {
            "name": "untick include actual plan",
            "input": [{ "key": "down" }, { "key": "down" }, { "key": "down" }, { "key": "enter" }],
        },
        {
            "name": "keyboard shortcuts",
            "input": [{ "key": "alt-h" }, { "key": "down" }, { "key": "enter" }, { "shot": "shortcuts" }],
        },
        {
            "name": "about over the shortcuts",
            "input": [{ "key": "alt-h" }, { "key": "down" }, { "key": "down" }, { "key": "enter" }],
        },
        { "name": "shortcuts closed", "input": [{ "key": "escape" }] },
        {
            "name": "grid keys after edit",
            "input": [
                { "key": "f6" },
                { "key": "f6" },
                { "key": "ctrl-end" },
                { "key": "alt-e" },
                { "key": "escape" },
                { "key": "up" },
            ],
        },
        {
            "name": "about, then another pane",
            "input": [
                { "key": "alt-h" },
                { "key": "down" },
                { "key": "down" },
                { "key": "enter" },
                { "key": "f6" },
                "idle",
            ],
        },
        {
            "name": "query after a run ends under file",
            "input": [
                { "key": "ctrl-n" },
                "idle",
                { "text": "SELECT pg_sleep(1)" },
                { "key": "f5" },
                "running",
                { "key": "alt-f" },
                "idle",
                { "key": "alt-q" },
            ],
        },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));
    // File, New query opens on the focused tab's connection (none), not the explorer's selection.
    let new_query = state(&lines, "new query from file");
    assert_eq!(new_query["active_tab"], 1);
    assert_eq!(
        new_query["tabs"][1]["connection"],
        Value::Null,
        "{new_query:#}"
    );
    let strings = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let query = |plan: &str| {
        vec![
            "Run".to_owned(),
            "Cancel (greyed)".to_owned(),
            "Estimated plan".to_owned(),
            format!("Include actual plan {plan}"),
            "Results to file [ ]".to_owned(),
            "Change database…".to_owned(),
        ]
    };
    let open = |step: &str| menu(state(&lines, step));

    let file = strings(&["New query", "New connection", "Open…", "Exit"]);
    assert_eq!(open("file"), (json!("File"), file));
    let edit = strings(&[
        "Undo",
        "Redo",
        "Cut",
        "Copy",
        "Paste",
        "Select all",
        "Find",
        "Replace",
        "Format",
    ]);
    assert_eq!(open("edit"), (json!("Edit"), edit));
    let view = strings(&[
        "Object explorer",
        "Results [x]",
        "Next pane",
        "Previous pane",
    ]);
    assert_eq!(open("view"), (json!("View"), view));
    assert_eq!(open("query"), (json!("Query"), query("[ ]")));
    let help = strings(&["Keyboard shortcuts", "About Database IDE"]);
    assert_eq!(open("help"), (json!("Help"), help));
    // Right from the last menu wraps to the first, and Left back again.
    assert_eq!(open("right of help").0, json!("File"));
    assert_eq!(open("left of file").0, json!("Help"));
    assert_eq!(open("closed"), (Value::Null, vec![]));
    assert_eq!(open("f10").0, json!("File"));

    // Query, Run runs the tab; afterwards Cancel is greyed again.
    let ran = state(&lines, "query, run");
    assert_eq!(ran["last_action"], "Run");
    assert_eq!(ran["results"].as_array().unwrap().len(), 1);
    assert_eq!(open("query after the run"), (json!("Query"), query("[ ]")));

    // Down skips the greyed Cancel, so three presses reach Include actual plan.
    let ticked = state(&lines, "tick include actual plan");
    assert_eq!(active_tab(ticked)["actual_plan"], true);
    assert_eq!(
        open("query with the plan on"),
        (json!("Query"), query("[x]"))
    );
    let unticked = state(&lines, "untick include actual plan");
    assert_eq!(active_tab(unticked)["actual_plan"], false);

    // Help, Keyboard shortcuts lists each menu item that has keys, with those keys.
    let shortcuts = state(&lines, "keyboard shortcuts");
    assert_eq!(shortcuts["menu_bar"]["panel"], "shortcuts");
    let rows = shortcuts["menu_bar"]["shortcuts"].as_array().unwrap();
    assert!(rows.contains(&json!({ "action": "Run", "keys": "Ctrl+E" })));
    assert!(rows.contains(&json!({ "action": "Cancel", "keys": "Alt+Pause" })));
    assert!(rows.contains(&json!({ "action": "Format", "keys": "Ctrl+K Ctrl+F" })));
    assert!(rows.contains(&json!({ "action": "Results to file", "keys": "Ctrl+Shift+F" })));
    assert!(!rows.iter().any(|row| row["action"] == "Exit"));
    let about = state(&lines, "about over the shortcuts");
    assert_eq!(about["menu_bar"]["panel"], "about");
    // Closing the second panel hands focus back to where it was before the first.
    let closed = state(&lines, "shortcuts closed");
    assert_eq!(closed["menu_bar"]["panel"], Value::Null);
    assert_eq!(closed["focused_pane"], "Editor", "{closed:#}");
    // A panel closes when focus leaves it, so Escape is never out of reach.
    let away = state(&lines, "about, then another pane");
    assert_eq!(away["menu_bar"]["panel"], Value::Null, "{away:#}");

    // Opening Edit over a grid and closing it leaves the grid's own keys working.
    let grid = state(&lines, "grid keys after edit");
    assert_eq!(
        grid["results"][0]["block"],
        json!({ "rows": { "start": 1, "end": 2 }, "columns": { "start": 0, "end": 1 } }),
        "{grid:#}"
    );

    // A menu opened after the run ended shows it as over, though File was open meanwhile.
    assert_eq!(
        open("query after a run ends under file"),
        (json!("Query"), query("[ ]"))
    );
}
