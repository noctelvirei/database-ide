//! Runs the real app on a UI script and reads back what the user saw after each step
//! (CODE_STANDARDS.md, "UI E2E harness").

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Plays `script` in the app and returns its state lines. With `appdata`, the app keeps its
/// connections file there instead of the user's.
pub fn run_script(script: &Path, appdata: Option<&Path>) -> Vec<Value> {
    let dir = tempfile::tempdir().expect("temp dir");
    let state = dir.path().join("state.jsonl");
    let mut command = Command::new(env!("CARGO_BIN_EXE_database-ide"));
    command
        .arg("--ui-script")
        .arg(script)
        .arg("--ui-state")
        .arg(&state);
    if let Some(appdata) = appdata {
        command.env("APPDATA", appdata);
    }
    let mut app = command.spawn().expect("start the app");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = app.try_wait().expect("wait for the app") {
            assert!(status.success(), "the app exited with {status}");
            break;
        }
        if Instant::now() > deadline {
            let _ = app.kill();
            panic!("the app did not finish the script within 120 s");
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
pub fn state<'a>(lines: &'a [Value], step: &str) -> &'a Value {
    let line = lines
        .iter()
        .find(|l| l["step"] == step)
        .unwrap_or_else(|| panic!("no state line for step {step:?}: {lines:#?}"));
    assert!(line.get("error").is_none(), "step {step:?} failed: {line}");
    &line["state"]
}
