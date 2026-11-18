//! The `--ui-script` test mode: plays a script of steps inside the running app and records
//! what a user would see after each one (CODE_STANDARDS.md, "UI E2E harness").
//!
//! Keys go through gpui's `Window::dispatch_keystroke`, the path a real key takes once
//! Windows has turned it into a keystroke: the keymap, the focused element's actions and its
//! text input. Each key is then released with a key-up event, because gpui clicks a focused
//! button or checkbox on the release of Enter or Space, not on the press. Synthetic OS input is not used because it lands in whatever window is in
//! front. What this skips is Windows' own key translation, so a key gpui does not
//! translate (Pause/Break) passes here but not at a real keyboard.
//!
//! The script is a JSON array of steps:
//!
//! ```json
//! [{ "name": "F6 moves to the explorer", "input": [{ "key": "f6" }, { "text": "select" }, { "shot": "panes" }] }]
//! ```
//!
//! After each step one JSON line `{"step": name, "state": VisibleState}` is appended to the
//! state file. A key that does not parse writes `{"step": name, "error": text}` and ends the
//! script. When the script ends the app writes `<state file>.done` and quits.
//!
//! A `shot` input writes `<state file>.shot` holding the shot's name and waits, up to 10 s,
//! for an outside script (`scripts/ui-run.ps1`) to capture the window and delete the file.
//! Shots are taken only with `--ui-shots`; otherwise they are skipped.
//!
//! An `"idle"` input waits, up to 30 s, until nothing is connecting, testing, running,
//! switching or listing, so a script can check an outcome without guessing how long it takes.
//! A `"running"` input waits, up to 30 s, until the active tab's run has started, so a script
//! can cancel a run that is under way rather than one still being set up.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    AsyncWindowContext, KeyUpEvent, Keystroke, Modifiers, OperatingSystem, PlatformInput,
    WeakEntity,
};
use serde::Deserialize;

use crate::workbench::Workbench;

/// One step: inputs played in order, then the state is recorded under `name`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    name: String,
    #[serde(default)]
    input: Vec<Input>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
enum Input {
    /// A chord or chord sequence in gpui keymap syntax, such as `ctrl-k ctrl-f`.
    Key(String),
    /// Text typed one character at a time.
    Text(String),
    /// A window screenshot saved under this name.
    Shot(String),
    /// Waits until no connect, test, run, switch or list is under way. Written as the string
    /// `"idle"`.
    Idle,
    /// Waits until the active tab's run has started. Written as the string `"running"`.
    Running,
}

/// A script read from disk, ready to play.
#[derive(Debug)]
pub struct UiScript {
    steps: Vec<Step>,
    state_file: PathBuf,
    take_shots: bool,
}

impl UiScript {
    /// Reads and checks the script, and empties the state file, before any window opens.
    /// Errors name the file and say what is wrong with it.
    pub fn load(script: &Path, state_file: PathBuf, take_shots: bool) -> Result<Self, String> {
        let text = std::fs::read_to_string(script)
            .map_err(|e| format!("can't read {}: {e}", script.display()))?;
        let steps: Vec<Step> = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a valid UI script: {e}", script.display()))?;
        std::fs::write(&state_file, "")
            .map_err(|e| format!("can't write {}: {e}", state_file.display()))?;
        for suffix in ["done", "shot", "shot.partial"] {
            // Leftovers from an earlier run would end a waiting runner early.
            let _ = std::fs::remove_file(with_suffix(&state_file, suffix));
        }
        Ok(Self {
            steps,
            state_file,
            take_shots,
        })
    }

    /// Plays every step, then quits the app.
    pub async fn play(self, workbench: WeakEntity<Workbench>, mut cx: AsyncWindowContext) {
        let executor = cx.background_executor().clone();
        let pause = |ms| executor.timer(Duration::from_millis(ms));
        // Let the first layout and focus settle.
        pause(500).await;
        'steps: for step in &self.steps {
            for input in &step.input {
                match input {
                    Input::Key(chord) => {
                        for part in chord.split_whitespace() {
                            // `parse` wants a `'static` key; the OS only resolves `secondary-`,
                            // which scripts don't use.
                            let parsed = Keystroke::parse_with_os(part, OperatingSystem::Windows);
                            let Ok(keystroke) = parsed else {
                                let line = serde_json::json!({
                                    "step": step.name,
                                    "error": format!("{part:?} is not a key"),
                                });
                                self.append(&line);
                                break 'steps;
                            };
                            press(&mut cx, keystroke);
                            pause(30).await;
                        }
                    }
                    Input::Text(text) => {
                        for c in text.chars() {
                            press(&mut cx, typed(c));
                            pause(10).await;
                        }
                    }
                    Input::Shot(name) if self.take_shots => {
                        pause(300).await;
                        let marker = with_suffix(&self.state_file, "shot");
                        // Renamed into place, so the runner never reads a half-written marker.
                        let partial = with_suffix(&self.state_file, "shot.partial");
                        let _ = std::fs::write(&partial, name)
                            .and_then(|()| std::fs::rename(&partial, &marker));
                        for _ in 0..200 {
                            if !marker.exists() {
                                break;
                            }
                            pause(50).await;
                        }
                    }
                    Input::Shot(_) => {}
                    Input::Idle => wait_until(&mut cx, &workbench, |wb, cx| !wb.is_busy(cx)).await,
                    Input::Running => {
                        wait_until(&mut cx, &workbench, Workbench::is_running).await;
                    }
                }
                pause(50).await;
            }
            pause(100).await;
            let state = cx
                .update(|window, cx| {
                    workbench
                        .read_with(cx, |wb, cx| wb.visible_state(window, cx))
                        .ok()
                })
                .ok()
                .flatten();
            self.append(&serde_json::json!({ "step": step.name, "state": state }));
        }
        let _ = std::fs::write(with_suffix(&self.state_file, "done"), "done");
        let _ = cx.update(|_, cx| cx.quit());
    }

    fn append(&self, line: &serde_json::Value) {
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.state_file)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// Presses and releases one key, as a real keyboard does.
fn press(cx: &mut AsyncWindowContext, keystroke: Keystroke) {
    let _ = cx.update(|window, cx| {
        window.dispatch_keystroke(keystroke.clone(), cx);
        window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
    });
}

/// The keystroke a character produces with no modifiers held.
fn typed(c: char) -> Keystroke {
    Keystroke {
        modifiers: Modifiers::default(),
        key: c.to_lowercase().to_string(),
        key_char: Some(c.to_string()),
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

/// Polls `done` every 100 ms for up to 30 s, or until the window has gone.
async fn wait_until(
    cx: &mut AsyncWindowContext,
    workbench: &WeakEntity<Workbench>,
    done: impl Fn(&Workbench, &gpui::App) -> bool,
) {
    for _ in 0..300 {
        let finished = cx
            .update(|_, cx| workbench.read_with(cx, |wb, cx| done(wb, cx)))
            .ok()
            .and_then(Result::ok)
            .unwrap_or(true);
        if finished {
            return;
        }
        let executor = cx.background_executor().clone();
        executor.timer(Duration::from_millis(100)).await;
    }
}
