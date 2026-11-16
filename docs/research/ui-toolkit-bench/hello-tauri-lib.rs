//! Minimal Tauri 2 window for the UI-toolkit benchmark (database-ide #3).
//! The page calls `first_frame` after its first animation frame; it writes epoch ms to BENCH_FIRST_FRAME.
use std::time::{SystemTime, UNIX_EPOCH};

#[tauri::command]
fn first_frame() {
    if let Ok(path) = std::env::var("BENCH_FIRST_FRAME") {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        let _ = std::fs::write(path, ms.to_string());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![first_frame])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
