//! Cleans up the passwords a UI test's app saved in Windows Credential Manager.

use std::path::Path;

use dbide_engine::connections::Connections;
use dbide_engine::connections::credentials::Credentials;

/// On drop, deletes the passwords saved under the tests' Credential Manager service for the
/// connections in `appdata`'s connections file.
pub struct ForgetPasswords<'a>(pub &'a Path);

impl Drop for ForgetPasswords<'_> {
    fn drop(&mut self) {
        let file = self.0.join("Database IDE").join("connections.toml");
        let (Ok(saved), Ok(credentials)) = (
            Connections::load(file),
            Credentials::new("database-ide-test"),
        ) else {
            return;
        };
        for connection in saved.connections() {
            let _ = credentials.delete(&connection.id);
        }
    }
}
