//! PROTOTYPE. Runs the spike's keyboard script inside the app.
//!
//! Each key goes through `Window::dispatch_keystroke`, which is the path a real key takes
//! after gpui's Windows layer has turned it into a keystroke: the keymap, the focused
//! element's actions, and the text input handler. It skips only the Win32 message
//! translation. This is used instead of SendInput because synthetic OS input can land in
//! whatever window the person at the PC is using.
//!
//! After each step the app's state is appended as one JSON line to the output file. A step
//! that names a screenshot writes `<out>.shot` holding the name and waits until an outside
//! script has captured the window and deleted the file.

use std::time::Duration;

use gpui::{
    AsyncWindowContext, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    PlatformInput, WeakEntity,
};

use crate::Workbench;

#[derive(Clone)]
enum Input {
    /// A key chord in gpui keymap syntax, such as `ctrl-alt-down`.
    Key(&'static str),
    /// Text typed one character at a time.
    Text(&'static str),
    /// A left click on the centre of plan operator `n`.
    ClickPlanNode(usize),
    /// A left click at a window position in pixels.
    ClickAt(f32, f32),
    /// Pause for an outside screenshot named by the string.
    Shot(&'static str),
}

use Input::*;

fn script() -> Vec<(&'static str, Vec<Input>)> {
    vec![
        ("01 start", vec![]),
        ("02 F6 -> grid", vec![Key("f6")]),
        ("03 F6 -> plan", vec![Key("f6")]),
        ("04 F6 -> details", vec![Key("f6")]),
        ("05 F6 -> editor (wraps)", vec![Key("f6")]),
        ("06 Shift+F6 -> details", vec![Key("shift-f6")]),
        ("07 Ctrl+2 -> grid", vec![Key("ctrl-2")]),
        ("08 Ctrl+3 -> plan", vec![Key("ctrl-3")]),
        ("09 Ctrl+4 -> details", vec![Key("ctrl-4")]),
        ("10 Ctrl+1 -> editor", vec![Key("ctrl-1")]),
        (
            "11 Ctrl+End, Enter, type a comment, Esc",
            vec![Key("ctrl-end"), Key("enter"), Text("-- typed by the script"), Key("escape")],
        ),
        ("12 Backspace x6, type 'keys'", vec![Key("backspace"); 6].into_iter().chain([Text("keys"), Key("escape")]).collect()),
        (
            "13 Ctrl+Home, Ctrl+Alt+Down x2 (two more cursors), type '#'",
            vec![Key("ctrl-home"), Key("ctrl-alt-down"), Key("ctrl-alt-down"), Text("#"), Shot("multi-cursor")],
        ),
        ("14 Esc (one cursor), Ctrl+Z", vec![Key("escape"), Key("ctrl-z")]),
        ("15 Ctrl+F, type 'created'", vec![Key("ctrl-f"), Text("created"), Shot("find")]),
        ("16 Enter (next match), Enter", vec![Key("enter"), Key("enter")]),
        ("17 Ctrl+H (replace field), type 'made'", vec![Key("ctrl-h"), Text("made"), Shot("replace")]),
        ("17b mouse: click Replace All", vec![ClickAt(1390., 82.), Shot("replaced")]),
        ("18 Esc, Ctrl+1", vec![Key("escape"), Key("ctrl-1")]),
        (
            "19 Ctrl+End, Enter, type 'SEL' (menu opens while typing)",
            vec![Key("ctrl-end"), Key("enter"), Text("SEL"), Shot("completion")],
        ),
        ("20 Enter accepts the first item", vec![Key("enter")]),
        ("21 type ' cust', Esc, Ctrl+Space reopens", vec![Text(" cust"), Key("escape"), Key("ctrl-space")]),
        ("22 Down, Enter picks the second item", vec![Key("down"), Key("enter")]),
        ("23 Ctrl+2, Down x3, Right x2", vec![Key("ctrl-2"), Key("down"), Key("down"), Key("down"), Key("right"), Key("right")]),
        ("24 PageDown x3", vec![Key("pagedown"), Key("pagedown"), Key("pagedown")]),
        ("25 Ctrl+End", vec![Key("ctrl-end"), Shot("grid-end")]),
        ("26 Up x2, PageUp", vec![Key("up"), Key("up"), Key("pageup")]),
        ("27 Ctrl+Home", vec![Key("ctrl-home")]),
        ("28 Ctrl+3, Down (first operator)", vec![Key("ctrl-3"), Key("down")]),
        ("29 Down x3, Right (first child)", vec![Key("down"), Key("down"), Key("down"), Key("right")]),
        ("30 Left (parent)", vec![Key("left"), Shot("plan-keyboard")]),
        ("31 Ctrl+4 (details pane)", vec![Key("ctrl-4")]),
        ("32 mouse: click operator 10", vec![ClickPlanNode(10), Shot("plan-click")]),
    ]
}

fn typed(c: char) -> Keystroke {
    Keystroke {
        modifiers: Modifiers::default(),
        key: c.to_lowercase().to_string(),
        key_char: Some(c.to_string()),
    }
}

pub fn start(out: String, this: WeakEntity<Workbench>, cx: &mut AsyncWindowContext) -> impl std::future::Future<Output = ()> {
    let cx = cx.clone();
    async move {
        let mut cx = cx;
        let _ = std::fs::write(&out, "");
        let executor = cx.background_executor().clone();
        let pause = |ms| executor.timer(Duration::from_millis(ms));
        pause(1500).await;
        for (name, inputs) in script() {
            for input in inputs {
                match input {
                    Key(chord) => {
                        let _ = cx.update(|window, cx| {
                            if let Ok(k) = Keystroke::parse(chord) {
                                window.dispatch_keystroke(k, cx);
                            }
                        });
                    }
                    Text(text) => {
                        for c in text.chars() {
                            let _ = cx.update(|window, cx| window.dispatch_keystroke(typed(c), cx));
                            pause(40).await;
                        }
                    }
                    ClickPlanNode(_) | ClickAt(..) => {
                        let _ = cx.update(|window, cx| {
                            let position = match input {
                                ClickAt(x, y) => Some(gpui::point(gpui::px(x), gpui::px(y))),
                                ClickPlanNode(n) => this
                                    .update(cx, |wb, cx| wb.plan.read(cx).node_center(n))
                                    .ok()
                                    .flatten(),
                                _ => None,
                            };
                            let Some(position) = position else { return };
                            let hover = gpui::MouseMoveEvent {
                                position,
                                pressed_button: None,
                                modifiers: Modifiers::default(),
                            };
                            window.dispatch_event(PlatformInput::MouseMove(hover), cx);
                            let down = MouseDownEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                                first_mouse: false,
                            };
                            window.dispatch_event(PlatformInput::MouseDown(down), cx);
                            let up = MouseUpEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                            };
                            window.dispatch_event(PlatformInput::MouseUp(up), cx);
                        });
                    }
                    Shot(shot) => {
                        pause(400).await;
                        let marker = format!("{out}.shot");
                        let _ = std::fs::write(&marker, shot);
                        for _ in 0..200 {
                            if !std::path::Path::new(&marker).exists() {
                                break;
                            }
                            pause(50).await;
                        }
                    }
                }
                pause(150).await;
            }
            pause(400).await;
            let line = cx
                .update(|window, cx| this.update(cx, |wb, cx| wb.state_json(window, cx)).ok())
                .ok()
                .flatten()
                .unwrap_or_default();
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&out) {
                let _ = writeln!(f, "{{\"step\":{name:?},\"state\":{}}}", line.trim_end());
            }
        }
        let _ = std::fs::write(format!("{out}.done"), "done");
    }
}
