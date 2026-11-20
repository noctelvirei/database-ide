//! Drives the real app window through `--ui-script` (CODE_STANDARDS.md, "UI E2E harness").
//! Each test opens a window on the desktop for a few seconds; skip them with `--skip ui_`.

mod support;

use std::path::Path;

use support::{run_script, state};

#[test]
fn ui_f6_f8_and_ctrl_r_move_focus_through_the_visible_panes() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/workbench-keys.json");
    let lines = run_script(&script, None);
    let cases = [
        ("start", "Editor", true),
        ("F6 to messages", "Messages", true),
        ("F6 to results", "Results", true),
        ("F6 to inspector", "Inspector", true),
        ("F6 wraps to query tabs", "QueryTabs", true),
        ("Shift+F6 wraps to inspector", "Inspector", true),
        ("F8 to explorer", "Explorer", true),
        ("Ctrl+R hides results", "Explorer", false),
        ("F6 to editor", "Editor", false),
        ("F6 to messages again", "Messages", false),
        ("F6 skips hidden results", "Inspector", false),
        ("Ctrl+R shows results", "Inspector", true),
        ("back to editor", "Editor", true),
    ];
    for (step, pane, results_visible) in cases {
        let s = state(&lines, step);
        assert_eq!(s["focused_pane"], pane, "focused pane after {step:?}");
        assert_eq!(
            s["results_visible"], results_visible,
            "results after {step:?}"
        );
    }
}

#[test]
fn ui_every_ssms_shortcut_reaches_its_action_from_the_editor() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/workbench-keys.json");
    let lines = run_script(&script, None);
    assert_eq!(state(&lines, "type in editor")["editor_text"], "select 1");
    let cases = [
        ("F5", "Run", "select 1"),
        ("Ctrl+E", "Run", "select 1"),
        ("Alt+Break", "CancelRun", "select 1"),
        ("Ctrl+L", "EstimatedPlan", "select 1"),
        ("Ctrl+M", "ToggleActualPlan", "select 1"),
        ("F9", "TogglePlanView", "select 1"),
        ("Ctrl+N", "NewQueryTab", ""),
        ("Ctrl+Tab", "NextQueryTab", "select 1"),
        ("Ctrl+K Ctrl+C", "CommentLines", "select 1"),
        ("Ctrl+K Ctrl+U", "UncommentLines", "select 1"),
        ("Ctrl+K Ctrl+F", "Format", "select 1"),
        ("Ctrl+Shift+R", "RefreshAutocomplete", "select 1"),
        ("Ctrl+U", "PickDatabase", "select 1"),
        ("Ctrl+O", "OpenFile", "select 1"),
        ("Ctrl+S", "SaveFile", "select 1"),
    ];
    for (step, action, text) in cases {
        let s = state(&lines, step);
        assert_eq!(s["last_action"], action, "action after {step:?}");
        assert_eq!(s["focused_pane"], "Editor", "focus after {step:?}");
        assert_eq!(s["editor_text"], text, "editor text after {step:?}");
    }
    let unconnected = &state(&lines, "F5")["messages"];
    assert_eq!(
        unconnected[unconnected.as_array().unwrap().len() - 1],
        "This tab has no connection. Select a connection in the explorer (F8) and press Ctrl+N."
    );
}

#[test]
fn ui_ctrl_f_finds_in_the_editor_and_ctrl_h_adds_replace() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/editor-search.json");
    let lines = run_script(&script, None);
    let search = |step| state(&lines, step)["tabs"][0]["search"].clone();
    assert_eq!(search("type"), serde_json::Value::Null);
    let found = search("find");
    assert_eq!(found["replace"], false, "{found:#}");
    assert_eq!(found["query"], "select");
    assert_eq!(found["matches"], "1/2");
    assert_eq!(search("find and replace")["replace"], true);
    assert_eq!(search("close"), serde_json::Value::Null);
    assert_eq!(
        state(&lines, "close")["editor_text"],
        "select 1 union select 2"
    );
}
