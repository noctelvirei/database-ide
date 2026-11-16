//! Minimal gpui window for the UI-toolkit benchmark (database-ide #3).
//! Writes epoch ms to BENCH_FIRST_FRAME on the first render() call (just before first paint).
#![windows_subsystem = "windows"]
use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, Window, WindowBounds,
    WindowOptions,
};
use std::time::{SystemTime, UNIX_EPOCH};

struct Hello {
    marked: bool,
}

impl Render for Hello {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.marked {
            self.marked = true;
            if let Ok(path) = std::env::var("BENCH_FIRST_FRAME") {
                let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
                let _ = std::fs::write(path, ms.to_string());
            }
        }
        div().size_full().bg(gpui::white()).child("Hello, Database IDE")
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Hello { marked: false }),
        )
        .unwrap();
        cx.activate(true);
    });
}
