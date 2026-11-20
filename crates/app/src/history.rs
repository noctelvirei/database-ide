//! Query history (docs/spec.md, "Files, restored tabs and history"): each run's text,
//! connection, database, start time, duration and outcome, one JSON line per run appended to
//! `history.jsonl` in the app-data folder. Entries older than 90 days are dropped when the
//! history is read; a line that doesn't parse (a write cut short) is skipped.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How long an entry is kept.
const KEPT: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// Held for every append and every read-and-prune, so a prune never drops a run this app
/// appends meanwhile.
static FILE: Mutex<()> = Mutex::new(());

/// One run, as the history panel lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// When the run started, in milliseconds since the Unix epoch.
    pub started_ms: u64,
    /// How long the run took, from the engine's own count.
    pub duration_ms: u64,
    /// The saved connection's id, so the entry reopens on it; `None` for a deleted one.
    pub connection_id: Option<String>,
    /// The connection's name when the run started.
    pub connection: Option<String>,
    pub database: Option<String>,
    /// The batch as it was sent.
    pub text: String,
    pub outcome: Outcome,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Finished; `rows` totals the row counts the server reported.
    Rows {
        rows: u64,
    },
    /// The server or the app reported this error first.
    Error {
        text: String,
    },
    Cancelled,
}

/// The history file in the app-data folder, beside `connections.toml`.
pub fn path() -> Option<PathBuf> {
    let connections = dbide_engine::connections::default_path().ok()?;
    Some(connections.with_file_name("history.jsonl"))
}

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Appends `entry` to the history at `path`, creating the file and its folder as needed.
pub fn append(path: &Path, entry: &Entry) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut line = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
    line.push(b'\n');
    let _held = FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // One write per line, so a concurrent reader sees whole lines or none of this one.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&line)
}

/// The history at `path`, latest start first, without entries older than 90 days at `now_ms`.
/// Rewrites the file when it dropped any. A missing file is an empty history. Blocking.
pub fn load(path: &Path, now_ms: u64) -> Vec<Entry> {
    let _held = FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let oldest = now_ms.saturating_sub(KEPT.as_millis() as u64);
    let lines = text.lines().filter(|line| !line.trim().is_empty());
    let parsed: Vec<Entry> = lines
        .clone()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let kept: Vec<Entry> = parsed
        .iter()
        .filter(|entry| entry.started_ms >= oldest)
        .cloned()
        .collect();
    if kept.len() < lines.count() {
        let mut rewritten = Vec::new();
        for entry in &kept {
            rewritten.extend(serde_json::to_vec(entry).expect("entry serialises"));
            rewritten.push(b'\n');
        }
        let _ = crate::replace_file::write(path, &rewritten);
    }
    // Runs are appended as they finish, so a long run lands after a later short one.
    let mut newest_first = kept;
    newest_first.sort_by_key(|entry| std::cmp::Reverse(entry.started_ms));
    newest_first
}

/// The entries whose text, connection or database contains `typed`, ignoring case, in the
/// order given. Everything matches an empty `typed`.
pub fn search<'a>(entries: &'a [Entry], typed: &str) -> Vec<&'a Entry> {
    let typed = typed.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| {
            [
                Some(entry.text.as_str()),
                entry.connection.as_deref(),
                entry.database.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(&typed))
        })
        .collect()
}

impl Entry {
    /// The entry as the history panel lists it: the batch's first line, then where and when
    /// it ran and how it ended.
    pub fn summary(&self, now_ms: u64) -> String {
        let first_line = self
            .text
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let place = match (&self.connection, &self.database) {
            (Some(connection), Some(database)) => format!("{connection} · {database}"),
            (Some(connection), None) => connection.clone(),
            (None, _) => "no connection".to_owned(),
        };
        let outcome = match &self.outcome {
            Outcome::Rows { rows: 1 } => "1 row".to_owned(),
            Outcome::Rows { rows } => format!("{rows} rows"),
            Outcome::Error { .. } => "error".to_owned(),
            Outcome::Cancelled => "cancelled".to_owned(),
        };
        format!(
            "{} — {place}, {}, {outcome}",
            first_line.trim(),
            age(now_ms.saturating_sub(self.started_ms))
        )
    }
}

/// How long ago something happened, as the panel says it.
fn age(ms: u64) -> String {
    let minutes = ms / 60_000;
    match minutes {
        0 => "just now".to_owned(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        1440..2880 => "yesterday".to_owned(),
        _ => format!("{} days ago", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Outcome, append, load};

    const DAY_MS: u64 = 24 * 60 * 60 * 1000;
    const NOW_MS: u64 = 1_000 * DAY_MS;

    fn run(text: &str, started_ms: u64) -> Entry {
        Entry {
            started_ms,
            duration_ms: 5,
            connection_id: Some("lab".into()),
            connection: Some("Lab".into()),
            database: Some("orders".into()),
            text: text.into(),
            outcome: Outcome::Rows { rows: 1 },
        }
    }

    #[test]
    fn loading_drops_runs_older_than_90_days_and_lists_the_rest_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        append(&path, &run("SELECT 'old'", NOW_MS - 91 * DAY_MS)).unwrap();
        append(&path, &run("SELECT 'recent'", NOW_MS - 10 * DAY_MS)).unwrap();
        append(&path, &run("SELECT 'today'", NOW_MS)).unwrap();

        let texts = |entries: Vec<Entry>| entries.into_iter().map(|e| e.text).collect::<Vec<_>>();
        assert_eq!(
            texts(load(&path, NOW_MS)),
            ["SELECT 'today'", "SELECT 'recent'"]
        );
        let file = std::fs::read_to_string(&path).unwrap();
        assert_eq!(file.lines().count(), 2, "the dropped run leaves the file");
    }

    #[test]
    fn a_long_run_that_finished_last_lists_by_when_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        append(&path, &run("SELECT 'short, started later'", NOW_MS)).unwrap();
        append(&path, &run("SELECT 'long, started first'", NOW_MS - 60_000)).unwrap();

        assert_eq!(
            load(&path, NOW_MS),
            [
                run("SELECT 'short, started later'", NOW_MS),
                run("SELECT 'long, started first'", NOW_MS - 60_000),
            ]
        );
    }

    #[test]
    fn a_line_cut_short_is_skipped_not_the_whole_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        append(&path, &run("SELECT 1", NOW_MS)).unwrap();
        let mut file = std::fs::read_to_string(&path).unwrap();
        file.push_str("{\"started_ms\":12");
        std::fs::write(&path, file).unwrap();

        assert_eq!(load(&path, NOW_MS), [run("SELECT 1", NOW_MS)]);
    }
}
