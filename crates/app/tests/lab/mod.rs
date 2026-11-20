//! A test server saved as the app's one connection, the way a user would have saved it.

use std::path::Path;

use dbide_engine::connections::credentials::Credentials;
use dbide_engine::connections::{Connection, Connections, SignInMethod, TlsMode};
use dbide_engine::danger::Engine;

/// Where a test server listens and how the test login signs in.
pub struct Server<'a> {
    pub engine: Engine,
    pub host: &'a str,
    pub port: u16,
    pub user: &'a str,
    pub password: &'a str,
    pub database: &'a str,
}

impl Server<'_> {
    /// Saves a red connection called "Lab" to this server in `appdata`, with its password
    /// in Credential Manager under the tests' service. Pair it with `ForgetPasswords`.
    pub fn save_as_lab(&self, appdata: &Path) {
        let mut details = Connection::new(self.engine, self.host);
        details.name = "Lab".into();
        details.port = Some(self.port);
        details.sign_in = SignInMethod::Password;
        details.user = Some(self.user.into());
        details.database = Some(self.database.into());
        details.colour = Some("#d13438".into());
        details.tls = TlsMode::Off;
        details.trust_server_certificate = true;
        let mut saved = Connections::default();
        saved.add_connection(details, None).unwrap();
        saved
            .save(appdata.join("Database IDE").join("connections.toml"))
            .unwrap();
        let credentials = Credentials::new("database-ide-test").unwrap();
        credentials
            .store(&saved.connections()[0], self.password)
            .unwrap();
    }
}
