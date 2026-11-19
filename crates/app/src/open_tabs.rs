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

/// The tabs kept at `path`; none when there is no file. A file that can't be read or parsed
/// is moved aside to `tabs.json.unreadable` rather than overwritten by the next save, so no
/// tab text is lost to a bad file. Errors when it can't even be moved aside: the caller must
/// then not save over it.
pub fn load(path: &Path) -> std::io::Result<Vec<SavedTab>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return move_aside(path).map(|()| Vec::new()),
    };
    match serde_json::from_slice(&bytes) {
        Ok(tabs) => Ok(tabs),
        Err(_) => move_aside(path).map(|()| Vec::new()),
    }
}

/// Renames `path` to `<path>.unreadable`, replacing an older one.
fn move_aside(path: &Path) -> std::io::Result<()> {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".unreadable");
    std::fs::rename(path, aside)
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
    fn a_tabs_file_that_is_not_utf8_is_moved_aside_rather_than_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tabs.json");
        std::fs::write(&path, [b'[', 0xFF, b']']).unwrap();

        assert_eq!(load(&path).unwrap(), []);
        let aside = std::fs::read(dir.path().join("tabs.json.unreadable")).unwrap();
        assert_eq!(aside, [b'[', 0xFF, b']']);
    }

    #[test]
    fn an_unreadable_tabs_file_is_moved_aside_rather_than_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tabs.json");
        std::fs::write(&path, "[{\"text\": \"SELECT 1").unwrap();

        assert_eq!(load(&path).unwrap(), []);
        assert!(!path.exists());
        let aside = std::fs::read_to_string(dir.path().join("tabs.json.unreadable")).unwrap();
        assert_eq!(aside, "[{\"text\": \"SELECT 1");
    }
}
