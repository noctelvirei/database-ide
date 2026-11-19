//! The open tabs, kept in `tabs.json` in the app-data folder so they come back on start
//! (docs/spec.md, "Files, restored tabs and history"): each tab's text, saved or not, its
//! file, and its connection and database. Never a password.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sql_file::Encoding;

/// One tab as it is kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub text: String,
    /// The `.sql` file the tab is tied to, with its encoding.
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub encoding: Encoding,
    /// The saved connection's id; `None` for a tab with no connection.
    pub connection_id: Option<String>,
    pub database: Option<String>,
}

/// The file in the app-data folder, beside `connections.toml`.
pub fn path() -> Option<PathBuf> {
    let connections = dbide_engine::connections::default_path().ok()?;
    Some(connections.with_file_name("tabs.json"))
}

/// The tabs kept at `path`; none when there is no file. A file that can't be read is moved
/// aside to `tabs.json.unreadable` rather than overwritten by the next save, so no tab text
/// is lost to a bad file.
pub fn load(path: &Path) -> Vec<SavedTab> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|_| {
        let mut aside = path.as_os_str().to_owned();
        aside.push(".unreadable");
        let _ = std::fs::rename(path, aside);
        Vec::new()
    })
}

/// Keeps `tabs` at `path`, replacing the file only once all of it is written.
pub fn save(path: &Path, tabs: &[SavedTab]) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let json = serde_json::to_vec_pretty(tabs).map_err(std::io::Error::other)?;
    crate::replace_file::write(path, &json)
}

#[cfg(test)]
mod tests {
    use super::load;

    #[test]
    fn an_unreadable_tabs_file_is_moved_aside_rather_than_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tabs.json");
        std::fs::write(&path, "[{\"text\": \"SELECT 1").unwrap();

        assert_eq!(load(&path), []);
        assert!(!path.exists());
        let aside = std::fs::read_to_string(dir.path().join("tabs.json.unreadable")).unwrap();
        assert_eq!(aside, "[{\"text\": \"SELECT 1");
    }
}
