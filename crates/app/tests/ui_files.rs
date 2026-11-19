//! Files, restored tabs and history as a user meets them (docs/spec.md, "Files, restored tabs
//! and history"): the app is quit and started again on the same app-data folder, and what
//! the second start shows is what the first one kept.

mod lab;
mod passwords;
mod support;

use std::path::Path;

use dbide_engine::danger::Engine;
use dbide_test_servers::postgres::Postgres;
use lab::Server;
use passwords::ForgetPasswords;
use serde_json::{Value, json};
use support::{run_script, state};

/// Plays `steps` in the app on `appdata`, then quits it.
fn play(dir: &Path, appdata: &Path, name: &str, steps: Value) -> Vec<Value> {
    let script = dir.join(format!("{name}.json"));
    std::fs::write(&script, steps.to_string()).unwrap();
    run_script(&script, Some(appdata))
}

/// `text` as UTF-16 little-endian with its byte-order mark, as SSMS saves a Unicode file.
fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

#[test]
fn ui_tabs_come_back_after_a_restart_with_their_text_and_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).unwrap();
    let saved = dir.path().join("saved.sql");
    let wide = dir.path().join("wide.sql");
    std::fs::write(&wide, utf16le("SELECT N'café'")).unwrap();

    let lines = play(
        dir.path(),
        &appdata,
        "first",
        json!([
            { "name": "type in the first tab", "input": [{ "text": "SELECT 1 -- never saved" }] },
            { "name": "a second tab", "input": [{ "key": "ctrl-n" }, { "text": "SELECT 2" }] },
            { "name": "save it", "input": [{ "path": saved }, { "key": "ctrl-s" }, "idle"] },
            { "name": "change it", "input": [{ "text": " -- changed" }] },
            { "name": "open a UTF-16 file", "input": [{ "path": wide }, { "key": "ctrl-o" }, "idle"] },
            {
                "name": "change and save it",
                "input": [{ "key": "ctrl-end" }, { "text": " -- edited" }, { "key": "ctrl-s" }, "idle"],
            },
        ]),
    );

    let after_save = state(&lines, "save it");
    assert_eq!(after_save["tabs"][1]["file"], saved.display().to_string());
    assert_eq!(after_save["tabs"][1]["modified"], false, "{after_save:#}");
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "SELECT 2");
    assert_eq!(state(&lines, "change it")["tabs"][1]["modified"], true);

    let opened = state(&lines, "open a UTF-16 file");
    assert_eq!(opened["active_tab"], 2, "a .sql file opens in a new tab");
    assert_eq!(opened["editor_text"], "SELECT N'café'");
    assert_eq!(
        std::fs::read(&wide).unwrap(),
        utf16le("SELECT N'café' -- edited"),
        "a file saves in the encoding it was opened in"
    );

    let lines = play(
        dir.path(),
        &appdata,
        "second",
        json!([{ "name": "restarted", "input": ["idle"] }]),
    );
    let restarted = state(&lines, "restarted");
    let tabs = restarted["tabs"].as_array().expect("tabs");
    let kept: Vec<(&Value, &Value, &Value, &Value)> = tabs
        .iter()
        .map(|t| (&t["text"], &t["file"], &t["modified"], &t["status"]))
        .collect();
    let (saved, wide) = (saved.display().to_string(), wide.display().to_string());
    assert_eq!(
        kept,
        [
            (
                &json!("SELECT 1 -- never saved"),
                &Value::Null,
                &json!(false),
                &json!("unconnected")
            ),
            (
                &json!("SELECT 2 -- changed"),
                &json!(saved),
                &json!(true),
                &json!("unconnected")
            ),
            (
                &json!("SELECT N'café' -- edited"),
                &json!(wide),
                &json!(false),
                &json!("unconnected")
            ),
        ],
        "{restarted:#}"
    );
    assert_eq!(restarted["active_tab"], 0);
}

#[test]
fn ui_pg_a_restored_tab_signs_in_on_f5_and_the_history_reopens_a_run() {
    let db = Postgres::start().expect("PostgreSQL test server");
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).unwrap();
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

    let lines = play(
        dir.path(),
        &appdata,
        "first",
        json!([
            { "name": "a tab on lab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle"] },
            { "name": "run", "input": [{ "text": "SELECT 42 AS answer" }, { "key": "f5" }, "idle"] },
            { "name": "another tab", "input": [{ "key": "ctrl-n" }, "idle"] },
            { "name": "run again", "input": [{ "text": "SELECT 7 AS seven" }, { "key": "f5" }, "idle"] },
        ]),
    );
    assert_eq!(state(&lines, "run again")["results"][0]["rows"], 1);

    let lines = play(
        dir.path(),
        &appdata,
        "second",
        json!([
            { "name": "restarted", "input": ["idle"] },
            { "name": "to the lab tab", "input": [{ "key": "ctrl-tab" }] },
            { "name": "F5", "input": [{ "key": "f5" }, "idle"] },
            { "name": "history", "input": [{ "key": "ctrl-shift-h" }, "idle"] },
            { "name": "search", "input": [{ "text": "seven" }] },
            { "name": "reopen", "input": [{ "key": "enter" }, "idle"] },
            {
                "name": "actual plan run",
                "input": [{ "key": "ctrl-m" }, { "key": "f5" }, "idle", { "key": "ctrl-shift-h" }, "idle"],
            },
            { "name": "Ctrl+N beside it", "input": [{ "key": "escape" }, { "key": "ctrl-n" }, "idle"] },
        ]),
    );

    let restarted = state(&lines, "restarted");
    let lab_tab = &restarted["tabs"][1];
    assert_eq!(lab_tab["connection"], "Lab", "{restarted:#}");
    assert_eq!(lab_tab["database"], db.database());
    assert_eq!(
        lab_tab["status"], "unconnected",
        "no sign-in before the first run"
    );

    let ran = state(&lines, "F5");
    assert_eq!(ran["tabs"][1]["status"], "ready", "{ran:#}");
    assert_eq!(ran["results"][0]["rows"], 1, "{ran:#}");

    let history = &state(&lines, "history")["history"]["matches"];
    let firsts: Vec<&str> = history
        .as_array()
        .expect("the history panel")
        .iter()
        .map(|m| m.as_str().unwrap().split(" — ").next().unwrap())
        .collect();
    assert_eq!(
        firsts,
        [
            "SELECT 42 AS answer",
            "SELECT 7 AS seven",
            "SELECT 42 AS answer"
        ],
        "every run, newest first"
    );
    let searched = &state(&lines, "search")["history"]["matches"];
    assert_eq!(searched.as_array().unwrap().len(), 1, "{searched:#}");

    let reopened = state(&lines, "reopen");
    assert_eq!(reopened["history"], Value::Null, "the panel closes");
    assert_eq!(reopened["active_tab"], 3, "{reopened:#}");
    assert_eq!(reopened["editor_text"], "SELECT 7 AS seven");
    assert_eq!(
        reopened["focused_pane"], "Editor",
        "the reopened tab takes the keys"
    );
    assert_eq!(reopened["tabs"][3]["connection"], "Lab");
    assert_eq!(reopened["tabs"][3]["status"], "unconnected");

    let planned = &state(&lines, "actual plan run")["history"]["matches"];
    assert_eq!(planned.as_array().unwrap().len(), 4, "{planned:#}");
    assert!(
        planned[0]
            .as_str()
            .unwrap()
            .starts_with("SELECT 7 AS seven"),
        "a run with its actual plan is in the history: {planned:#}"
    );

    let beside = state(&lines, "Ctrl+N beside it");
    assert_eq!(beside["active_tab"], 4, "{beside:#}");
    assert_eq!(beside["tabs"][4]["connection"], "Lab");
}
