//! Database IDE's desktop app: the gpui UI over the engine (docs/spec.md, "UI").
//!
//! Environment variables used by the measurement script (`.claude/skills/verify/measure-app.ps1`):
//! - `DBIDE_FIRST_FRAME`: a file the app writes the Unix time in milliseconds of its first
//!   frame to.
// Release builds open no console window; debug builds keep one for panics and logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod choice;
mod connection_dialog;
mod explorer;
mod file_prompt;
mod folder_dialog;
mod keymap;
mod messages;
mod palette;
mod query_tab;
mod results;
mod runtime;
mod ui_script;
mod workbench;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use dbide_engine::connections::{self, credentials::Credentials};
use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use gpui_component::Root;

use crate::explorer::Explorer;
use crate::runtime::Runtime;
use crate::ui_script::UiScript;
use crate::workbench::Workbench;

#[derive(Parser)]
#[command(
    version,
    about = "A lightweight SQL IDE for SQL Server, PostgreSQL and MySQL"
)]
struct Args {
    /// Test mode: play this UI script, then quit (CODE_STANDARDS.md, "UI E2E harness").
    #[arg(long, requires = "ui_state")]
    ui_script: Option<PathBuf>,
    /// The file the UI script's per-step state lines are written to.
    #[arg(long, requires = "ui_script")]
    ui_state: Option<PathBuf>,
    /// Pause at the script's screenshot steps for `.claude/skills/verify/ui-run.ps1` to capture the window.
    #[arg(long, requires = "ui_script")]
    ui_shots: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let script = match (args.ui_script, args.ui_state) {
        (Some(script), Some(state)) => match UiScript::load(&script, state, args.ui_shots) {
            Ok(script) => Some(script),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        },
        _ => None,
    };
    // UI scripts keep their passwords apart from the user's, under the tests' service name.
    let service = if script.is_some() {
        "database-ide-test"
    } else {
        "database-ide"
    };
    let setup = Credentials::new(service)
        .map_err(|e| format!("Can't open Windows Credential Manager: {e}"))
        .and_then(|credentials| {
            let runtime = Runtime::new().map_err(|e| format!("Can't start the engine: {e}"))?;
            let path = connections::default_path()
                .map_err(|e| format!("Can't find the connections file: {e}"))?;
            Ok((credentials, runtime, path))
        });
    let (credentials, runtime, path) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    // The component icons gpui-component draws (the find panel's buttons among them).
    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            keymap::bind(cx);
            let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Database IDE".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                let explorer =
                    cx.new(|cx| Explorer::new(path, credentials, runtime.clone(), window, cx));
                let workbench = cx.new(|cx| {
                    Workbench::new(
                        explorer,
                        runtime,
                        move |window, cx| {
                            // Next-frame callbacks run once this frame has been drawn and presented.
                            window.on_next_frame(|_, _| write_first_frame_time());
                            if let Some(script) = script {
                                cx.spawn_in(window, async move |this, cx| {
                                    script.play(this, cx.clone()).await
                                })
                                .detach();
                            }
                        },
                        window,
                        cx,
                    )
                });
                cx.new(|cx| Root::new(workbench, window, cx))
            });
            if opened.is_err() {
                cx.quit();
            }
            cx.activate(true);
        });
    ExitCode::SUCCESS
}

/// Writes the time the first frame was presented, for the startup measurement, when it asks.
fn write_first_frame_time() {
    let Ok(path) = std::env::var("DBIDE_FIRST_FRAME") else {
        return;
    };
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let _ = std::fs::write(path, ms.to_string());
}
