//! "Loaded" Tauri 2 window for the UI-toolkit benchmark (database-ide #3):
//! CodeMirror 6 SQL editor above a virtualised grid. The ROWS x COLS strings live in Rust;
//! the page fetches only the visible slice through `rows`, so the data is not held twice.
//! The page calls `first_frame` after the editor and first grid page have painted.
use std::time::{SystemTime, UNIX_EPOCH};

const ROWS: usize = 100_000;
const COLS: usize = 10;

struct ResultSet(Vec<Vec<String>>);

#[tauri::command]
fn first_frame() {
    if let Ok(path) = std::env::var("BENCH_FIRST_FRAME") {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        let _ = std::fs::write(path, ms.to_string());
    }
}

/// Total row count of the result set.
#[tauri::command]
fn row_count(rs: tauri::State<ResultSet>) -> usize {
    rs.0.len()
}

/// Rows `start..start+count`, clamped to the result set.
#[tauri::command]
fn rows(start: usize, count: usize, rs: tauri::State<ResultSet>) -> Vec<Vec<String>> {
    let end = (start + count).min(rs.0.len());
    rs.0[start.min(end)..end].to_vec()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let data = (0..ROWS)
        .map(|r| (0..COLS).map(|c| format!("row {r:06} col {c}")).collect())
        .collect();
    tauri::Builder::default()
        .manage(ResultSet(data))
        .invoke_handler(tauri::generate_handler![first_frame, row_count, rows])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
