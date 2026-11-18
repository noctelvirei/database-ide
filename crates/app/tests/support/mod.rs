//! Runs the real app on a UI script and reads back what the user saw after each step
//! (CODE_STANDARDS.md, "UI E2E harness").

use std::io::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Plays `script` in the app and returns its state lines. With `appdata`, the app keeps its
/// connections file there instead of the user's. At each `mark` step the app's private
/// working set is sampled and added as a line `{"mark": name, "private_ws_mb": n}`.
///
/// With `DBIDE_UI_SHOTS` set to a folder, the script runs through `scripts/ui-run.ps1`
/// instead, which also saves the script's screenshots in a subfolder named after the test.
pub fn run_script(script: &Path, appdata: Option<&Path>) -> Vec<Value> {
    if let Some(shots) = std::env::var_os("DBIDE_UI_SHOTS") {
        return run_with_shots(script, appdata, Path::new(&shots));
    }
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
    let marker = dir.path().join("state.jsonl.mark");
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
        if let Ok(name) = std::fs::read_to_string(&marker) {
            let line = serde_json::json!({
                "mark": name,
                "private_ws_mb": private_working_set_mb(app.id()),
            });
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&state)
                .expect("state file");
            writeln!(file, "{line}").expect("write a mark line");
            std::fs::remove_file(&marker).expect("release the mark");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::fs::read_to_string(&state)
        .expect("state file")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON state line"))
        .collect()
}

/// Plays `script` through `scripts/ui-run.ps1`, saving its state lines and screenshots in
/// `shots/<test name>`.
fn run_with_shots(script: &Path, appdata: Option<&Path>, shots: &Path) -> Vec<Value> {
    let test = std::thread::current()
        .name()
        .unwrap_or("ui")
        .replace("::", "-");
    let out = shots.join(test);
    let runner = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui-run.ps1");
    let mut command = Command::new("pwsh");
    command
        .args(["-NoProfile", "-File"])
        .arg(runner)
        .arg("-Exe")
        .arg(env!("CARGO_BIN_EXE_database-ide"))
        .arg("-Script")
        .arg(script)
        .arg("-OutDir")
        .arg(&out);
    if let Some(appdata) = appdata {
        command.env("APPDATA", appdata);
    }
    let output = command.output().expect("run scripts/ui-run.ps1");
    assert!(
        output.status.success(),
        "ui-run.ps1 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(out.join("state.jsonl"))
        .expect("state file")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON state line"))
        .collect()
}

/// Memory only the process uses, as Task Manager's "Memory" column and
/// scripts/measure-app.ps1 report it.
fn private_working_set_mb(pid: u32) -> f64 {
    let output = Command::new("pwsh")
        .args(["-NoProfile", "-Command"])
        .arg(format!(
            "(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter 'IDProcess={pid}').WorkingSetPrivate"
        ))
        .output()
        .expect("run pwsh");
    let bytes: f64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("no working set for process {pid}: {output:?}"));
    bytes / (1024. * 1024.)
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
