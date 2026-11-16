//! Minimal eframe window for the UI-toolkit benchmark (database-ide #3).
//! Writes epoch ms to the file named by BENCH_FIRST_FRAME on the first ui() call.
#![windows_subsystem = "windows"]
use std::time::{SystemTime, UNIX_EPOCH};

fn mark_first_frame() {
    if let Ok(path) = std::env::var("BENCH_FIRST_FRAME") {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        let _ = std::fs::write(path, ms.to_string());
    }
}

struct Hello {
    marked: bool,
}

impl eframe::App for Hello {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        ui.label("Hello, Database IDE");
        if !self.marked {
            self.marked = true;
            mark_first_frame();
        }
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "hello-egui",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(Hello { marked: false }))),
    )
}
