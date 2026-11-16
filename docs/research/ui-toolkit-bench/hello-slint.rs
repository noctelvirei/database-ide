//! Minimal Slint window for the UI-toolkit benchmark (database-ide #3).
//! Writes epoch ms to BENCH_FIRST_FRAME after the first frame is rendered.
#![windows_subsystem = "windows"]
use slint::ComponentHandle;
use std::time::{SystemTime, UNIX_EPOCH};

slint::slint! {
    export component Hello inherits Window {
        title: "hello-slint";
        width: 800px; height: 600px;
        Text { text: "Hello, Database IDE"; }
    }
}

fn main() {
    let app = Hello::new().unwrap();
    let mut marked = false;
    let _ = app.window().set_rendering_notifier(move |state, _| {
        if !marked && matches!(state, slint::RenderingState::AfterRendering) {
            marked = true;
            if let Ok(path) = std::env::var("BENCH_FIRST_FRAME") {
                let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
                let _ = std::fs::write(path, ms.to_string());
            }
        }
    });
    app.run().unwrap();
}
