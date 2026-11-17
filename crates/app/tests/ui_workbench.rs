//! Drives the real app window through `--ui-script` (CODE_STANDARDS.md, "UI E2E harness").
//! Each test opens a window on the desktop for a few seconds; skip them with `--skip ui_`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Runs a script from `tests/scripts` and returns its state lines.
fn run_script(name: &str) -> Vec<Value> {
    let dir = tempfile::tempdir().expect("temp dir");
    let state = dir.path().join("state.jsonl");
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/scripts")
        .join(name);
    let mut app = Command::new(env!("CARGO_BIN_EXE_database-ide"))
        .arg("--ui-script")
        .arg(&script)
        .arg("--ui-state")
        .arg(&state)
        .spawn()
        .expect("start the app");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = app.try_wait().expect("wait for the app") {
            assert!(status.success(), "the app exited with {status}");
            break;
        }
        if Instant::now() > deadline {
            let _ = app.kill();
            panic!("the app did not finish the script within 60 s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::fs::read_to_string(&state)
        .expect("state file")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON state line"))
        .collect()
}

/// The state recorded after the step called `step`.
fn state<'a>(lines: &'a [Value], step: &str) -> &'a Value {
    let line = lines
        .iter()
        .find(|l| l["step"] == step)
        .unwrap_or_else(|| panic!("no state line for step {step:?}: {lines:#?}"));
    assert!(line.get("error").is_none(), "step {step:?} failed: {line}");
    &line["state"]
}

#[test]
fn ui_f6_f8_and_ctrl_r_move_focus_through_the_visible_panes() {
    let lines = run_script("workbench-keys.json");
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
    let lines = run_script("workbench-keys.json");
    assert_eq!(state(&lines, "type in editor")["editor_text"], "select 1");
    let cases = [
        ("F5", "Run"),
        ("Ctrl+E", "Run"),
        ("Alt+Break", "CancelRun"),
        ("Ctrl+L", "EstimatedPlan"),
        ("Ctrl+M", "ToggleActualPlan"),
        ("F9", "TogglePlanView"),
        ("Ctrl+N", "NewQueryTab"),
        ("Ctrl+Tab", "NextQueryTab"),
        ("Ctrl+K Ctrl+C", "CommentLines"),
        ("Ctrl+K Ctrl+U", "UncommentLines"),
        ("Ctrl+K Ctrl+F", "Format"),
        ("Ctrl+Shift+R", "RefreshAutocomplete"),
        ("Ctrl+U", "PickDatabase"),
        ("Ctrl+O", "OpenFile"),
        ("Ctrl+S", "SaveFile"),
    ];
    for (step, action) in cases {
        let s = state(&lines, step);
        assert_eq!(s["last_action"], action, "action after {step:?}");
        assert_eq!(s["focused_pane"], "Editor", "focus after {step:?}");
        assert_eq!(
            s["editor_text"], "select 1",
            "{step:?} must not edit the text"
        );
    }
    let messages = &state(&lines, "Ctrl+S")["messages"];
    assert_eq!(
        messages[messages.as_array().unwrap().len() - 1],
        "SaveFile is not built yet."
    );
}
