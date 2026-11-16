//! Baseline for the loaded benchmarks (database-ide #3): allocates the same 100,000 x 10 test strings
//! with no UI, marks "first frame" at once, then idles so measure.ps1 can read its memory.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn main() {
    let data: Vec<Vec<String>> = (0..100_000usize)
        .map(|r| (0..10).map(|c| format!("row {r:06} col {c}")).collect())
        .collect();
    if let Ok(path) = std::env::var("BENCH_FIRST_FRAME") {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        let _ = std::fs::write(path, ms.to_string());
    }
    std::thread::sleep(Duration::from_secs(600));
    drop(data);
}
