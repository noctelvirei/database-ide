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
//! for an outside script (`.claude/skills/verify/ui-run.ps1`) to capture the window and delete the file.
//! Shots are taken only with `--ui-shots`; otherwise they are skipped.
//!
//! A `mark` input does the same with `<state file>.mark`, always, so the test running the
//! script can sample the app (its memory, say) at that point; the test deletes the file to
//! let the script go on.
//!
//! A `path` input queues a path as the answer to the next file dialog (`file_prompt.rs`):
//! `{ "path": "C:\\plans\\join.sqlplan" }` before the key that opens or saves. A dialog with
//! no path queued reads as cancelled; no real dialog opens under a script.
//!
//! Three inputs wait, each up to `WAIT_LIMIT`, so a script can check an outcome without
//! guessing how long it takes. `"idle"` waits until nothing is connecting, testing, running,
//! switching or listing. `"running"` waits until the active tab's run has started, so a
//! script can cancel a run that is under way rather than one still being set up.
//! `{ "until": "/tab_dialog/statements/0/rows/about" }` waits until the state line would
//! hold a value at that JSON pointer (RFC 6901), for a moment the other two can't name,
//! such as one estimate in while another is still pending.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    AsyncWindowContext, KeyUpEvent, Keystroke, Modifiers, OperatingSystem, PlatformInput,
    WeakEntity,
};
use serde::Deserialize;

use crate::file_prompt::ScriptedPaths;
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
    /// A pause for the test running the script to sample the app, under this name.
    Mark(String),
    /// The answer to the next file dialog.
    Path(PathBuf),
    /// Waits until no connect, test, run, switch or list is under way. Written as the string
    /// `"idle"`.
    Idle,
    /// Waits until the active tab's run has started. Written as the string `"running"`.
    Running,
    /// Waits until the visible state has a non-null value at this JSON pointer.
    Until(String),
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
        for suffix in ["done", "shot", "shot.partial", "mark", "mark.partial"] {
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
        let _ = cx.update(|_, cx| cx.set_global(ScriptedPaths::default()));
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
                        let marker = self.hand_over("shot", name);
                        for _ in 0..200 {
                            if !marker.exists() {
                                break;
                            }
                            pause(50).await;
                        }
                    }
                    Input::Shot(_) => {}
                    Input::Mark(name) => {
                        let marker = self.hand_over("mark", name);
                        for _ in 0..600 {
                            if !marker.exists() {
                                break;
                            }
                            pause(50).await;
                        }
                    }
                    Input::Path(path) => {
                        let path = path.clone();
                        let _ = cx.update(|_, cx| cx.global_mut::<ScriptedPaths>().push(path));
                    }
                    Input::Idle => {
                        wait_until(&mut cx, &workbench, |wb, _, cx| !wb.is_busy(cx)).await;
                    }
                    Input::Running => {
                        wait_until(&mut cx, &workbench, |wb, _, cx| wb.is_running(cx)).await;
                    }
                    Input::Until(pointer) => {
                        wait_until(&mut cx, &workbench, |wb, window, cx| {
                            serde_json::to_value(wb.visible_state(window, cx))
                                .ok()
                                .and_then(|state| state.pointer(pointer).cloned())
                                .is_some_and(|value| !value.is_null())
                        })
                        .await;
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

    /// Writes `<state file>.<suffix>` holding `name` for the outside runner, and returns
    /// its path; the runner deletes it when done. Renamed into place, so the runner never
    /// reads a half-written marker.
    fn hand_over(&self, suffix: &str, name: &str) -> PathBuf {
        let marker = with_suffix(&self.state_file, suffix);
        let partial = with_suffix(&self.state_file, &format!("{suffix}.partial"));
        let _ = std::fs::write(&partial, name).and_then(|()| std::fs::rename(&partial, &marker));
        marker
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

/// How long one `idle`, `running` or `until` input waits. Two million-row saves and a million-row run
/// take about 30 s on a loaded machine, so this leaves room for slower ones; the whole
/// script still has the test harness's own deadline.
const WAIT_LIMIT: Duration = Duration::from_secs(100);

/// Polls `done` every 100 ms until it holds or the window has gone. Panics, ending the
/// script with a failure, if `done` still doesn't hold after `WAIT_LIMIT`: a step that
/// carried on would take its state too early and fail later for no visible reason.
async fn wait_until(
    cx: &mut AsyncWindowContext,
    workbench: &WeakEntity<Workbench>,
    done: impl Fn(&Workbench, &gpui::Window, &gpui::App) -> bool,
) {
    let started = Instant::now();
    loop {
        let finished = cx
            .update(|window, cx| workbench.read_with(cx, |wb, cx| done(wb, window, cx)))
            .ok()
            .and_then(Result::ok)
            .unwrap_or(true);
        if finished {
            return;
        }
        assert!(
            started.elapsed() < WAIT_LIMIT,
            "the awaited state didn't come within {} s",
            WAIT_LIMIT.as_secs()
        );
        let executor = cx.background_executor().clone();
        executor.timer(Duration::from_millis(100)).await;
    }
}
