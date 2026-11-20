//! Password-free saved connections, folder inheritance, atomic persistence and credential access.

use std::collections::HashSet;
use std::hash::{BuildHasher, RandomState};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::danger::Engine;

pub mod connection_string;
pub mod credentials;
pub mod regsrvr;

/// Failures reading, validating, editing or saving a connections tree.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The filesystem refused an operation; the original file is retained on failed saves.
    #[error("connections file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a supported, password-free TOML tree.
    #[error("invalid connections TOML")]
    Toml,
    /// An ID, parent, address or name violates the saved-tree contract.
    #[error("invalid connections tree: {0}")]
    Invalid(&'static str),
    /// The requested connection or folder does not exist.
    #[error("connection or folder not found")]
    NotFound,
    /// No app-data location can be determined from the environment.
    #[error("app-data environment variable is missing or empty")]
    AppData,
}

/// How to obtain credentials; Windows sign-in uses the current process account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInMethod {
    /// A database login whose password may be kept in Credential Manager.
    Password,
    /// The Windows account running the application.
    Windows,
    /// A database login that never stores its password.
    AskEachTime,
}

/// Encryption policy, independent of certificate validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    /// Require an encrypted connection.
    Mandatory,
    /// Negotiate encryption when the server supports it.
    Optional,
    /// Do not request encryption.
    Off,
}

/// Details for one server; passwords belong only in `credentials`, never in this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// Display name; cannot be blank.
    pub name: String,
    /// Database product, serialized as sql_server, postgre_sql or my_sql.
    #[serde(with = "engine_serde")]
    pub engine: Engine,
    /// Hostname or IP address, without a port or instance suffix.
    pub host: String,
    /// TCP port in 1..=65535; mutually exclusive with an instance.
    pub port: Option<u16>,
    /// SQL Server named instance; mutually exclusive with a port.
    pub instance: Option<String>,
    /// Where the caller obtains the login credentials.
    pub sign_in: SignInMethod,
    /// Database login or PostgreSQL Windows sign-in role; SQL Server Windows sign-in ignores it.
    pub user: Option<String>,
    /// Initial database, or the server/login default when absent.
    pub database: Option<String>,
    /// A UI colour string; absent means inherit from the nearest folder.
    pub colour: Option<String>,
    /// Whether to require TLS; certificate trust is configured separately.
    pub tls: TlsMode,
    /// Accept a certificate without validating its chain or hostname.
    pub trust_server_certificate: bool,
    /// Name reported to the server for this application.
    pub application_name: String,
    /// Seconds to establish a connection; zero means no timeout.
    pub connect_timeout: u32,
    /// Seconds allowed for a query; zero means no timeout.
    pub query_timeout: u32,
    /// Override the inherited setting; absent defaults to on if no folder sets it.
    pub danger_check: Option<bool>,
}

impl Connection {
    /// Start with password prompting, mandatory TLS, a 15-second connect timeout and no query limit.
    /// The host becomes the display name. Validation happens when added or saved.
    pub fn new(engine: Engine, host: impl Into<String>) -> Self {
        let host = host.into();
        Self {
            name: host.clone(),
            host,
            engine,
            port: None,
            instance: None,
            sign_in: SignInMethod::AskEachTime,
            user: None,
            database: None,
            colour: None,
            tls: TlsMode::Mandatory,
            trust_server_certificate: false,
            application_name: "Database IDE".into(),
            connect_timeout: 15,
            query_timeout: 0,
            danger_check: None,
        }
    }

    fn validate(&self) -> Result<(), Error> {
        nonblank(&self.name)?;
        nonblank(&self.host)?;
        if self.port == Some(0) {
            return Err(Error::Invalid("port must be nonzero"));
        }
        if let Some(instance) = &self.instance {
            nonblank(instance)?;
            if self.engine != Engine::SqlServer || self.port.is_some() {
                return Err(Error::Invalid("instance requires SQL Server and no port"));
            }
        }
        Ok(())
    }
}

/// A saved connection with a persistent identity independent of its display name and location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedConnection {
    /// Opaque ID used as the credential key; never changed by edits.
    pub id: String,
    /// Containing folder ID; absent at the tree root.
    pub parent: Option<String>,
    /// Password-free server details.
    pub connection: Connection,
}

/// A nestable folder with independently inherited colour and danger-check settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Folder {
    /// Opaque ID, independent of the folder name.
    pub id: String,
    /// Containing folder ID; absent at the tree root.
    pub parent: Option<String>,
    /// Display name; cannot be blank.
    pub name: String,
    /// UI colour string; absent means inherit.
    pub colour: Option<String>,
    /// Absent means inherit, with on as the root default.
    pub danger_check: Option<bool>,
}

/// Settings after resolving the connection and each ancestor independently.
#[derive(Debug, PartialEq, Eq)]
pub struct EffectiveSettings<'a> {
    /// Nearest explicit colour, or none if the whole ancestry is unset.
    pub colour: Option<&'a str>,
    /// Nearest explicit danger check, or true if the whole ancestry is unset.
    pub danger_check: bool,
}

/// A validated tree represented by ID-linked folders and connections, in insertion order.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Connections {
    folders: Vec<Folder>,
    connections: Vec<SavedConnection>,
}

impl<'de> Deserialize<'de> for Connections {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            #[serde(default)]
            folders: Vec<Folder>,
            #[serde(default)]
            connections: Vec<SavedConnection>,
        }
        let file = File::deserialize(deserializer)?;
        let tree = Self {
            folders: file.folders,
            connections: file.connections,
        };
        tree.validate().map_err(serde::de::Error::custom)?;
        Ok(tree)
    }
}

impl Connections {
    /// Read an explicit file; missing files yield an empty tree, malformed trees return errors.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
        };
        toml::from_str(&text).map_err(|_| Error::Toml)
    }

    /// Atomically replace an explicit file using a flushed sibling temp file; create parent folders.
    /// Invalid trees and failed writes leave the previous file intact.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        self.validate()?;
        let text = toml::to_string_pretty(self).map_err(|_| Error::Toml)?;
        let path = path.as_ref();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(text.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    /// Folders in insertion order; parent IDs define nesting.
    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// Connections in insertion order; details can be replaced through `update_connection`.
    pub fn connections(&self) -> &[SavedConnection] {
        &self.connections
    }

    /// Add a folder at the root or below an existing folder, returning its persistent ID.
    pub fn add_folder(
        &mut self,
        name: impl Into<String>,
        parent: Option<&str>,
    ) -> Result<String, Error> {
        let name = name.into();
        nonblank(&name)?;
        self.check_parent(parent)?;
        let id = new_id();
        self.folders.push(Folder {
            id: id.clone(),
            parent: parent.map(str::to_owned),
            name,
            colour: None,
            danger_check: None,
        });
        Ok(id)
    }

    /// Add validated server details at the root or below an existing folder; no password is stored.
    pub fn add_connection(
        &mut self,
        connection: Connection,
        parent: Option<&str>,
    ) -> Result<String, Error> {
        connection.validate()?;
        self.check_parent(parent)?;
        let id = new_id();
        self.connections.push(SavedConnection {
            id: id.clone(),
            parent: parent.map(str::to_owned),
            connection,
        });
        Ok(id)
    }

    /// Replace details while retaining identity and location, without accessing Credential Manager.
    /// After `save` succeeds, pass each returned ID to `Credentials::delete` and report any errors.
    pub fn update_connection(
        &mut self,
        id: &str,
        connection: Connection,
    ) -> Result<Vec<String>, Error> {
        connection.validate()?;
        let saved = self
            .connections
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or(Error::NotFound)?;
        let deletions = if connection.sign_in != SignInMethod::Password {
            vec![id.to_owned()]
        } else {
            Vec::new()
        };
        saved.connection = connection;
        Ok(deletions)
    }

    /// Set or clear a folder's colour and danger-check overrides.
    pub fn set_folder_settings(
        &mut self,
        id: &str,
        colour: Option<String>,
        danger_check: Option<bool>,
    ) -> Result<(), Error> {
        let folder = self
            .folders
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or(Error::NotFound)?;
        folder.colour = colour;
        folder.danger_check = danger_check;
        Ok(())
    }

    /// Rename a connection or folder without changing its ID or password key.
    pub fn rename(&mut self, id: &str, name: impl Into<String>) -> Result<(), Error> {
        let name = name.into();
        nonblank(&name)?;
        if let Some(folder) = self.folders.iter_mut().find(|f| f.id == id) {
            folder.name = name;
        } else {
            self.connections
                .iter_mut()
                .find(|c| c.id == id)
                .ok_or(Error::NotFound)?
                .connection
                .name = name;
        }
        Ok(())
    }

    /// Move a connection or folder; reject missing destinations and cycles without editing the tree.
    pub fn move_to(&mut self, id: &str, parent: Option<&str>) -> Result<(), Error> {
        self.check_parent(parent)?;
        let mut ancestor = parent;
        while let Some(current) = ancestor {
            if current == id {
                return Err(Error::Invalid("folder cycle"));
            }
            ancestor = self
                .folders
                .iter()
                .find(|f| f.id == current)
                .and_then(|f| f.parent.as_deref());
        }
        let parent = parent.map(str::to_owned);
        if let Some(folder) = self.folders.iter_mut().find(|f| f.id == id) {
            folder.parent = parent;
        } else {
            self.connections
                .iter_mut()
                .find(|c| c.id == id)
                .ok_or(Error::NotFound)?
                .parent = parent;
        }
        Ok(())
    }

    /// Delete a connection or folder subtree without accessing Credential Manager. Missing IDs
    /// are a no-op. After `save` succeeds, delete credentials for each returned ID and report errors.
    pub fn delete(&mut self, id: &str) -> Result<Vec<String>, Error> {
        let mut removed = HashSet::from([id.to_owned()]);
        loop {
            let before = removed.len();
            for folder in &self.folders {
                if folder.parent.as_ref().is_some_and(|p| removed.contains(p)) {
                    removed.insert(folder.id.clone());
                }
            }
            if before == removed.len() {
                break;
            }
        }
        let mut deletions = Vec::new();
        for saved in &self.connections {
            if removed.contains(&saved.id)
                || saved.parent.as_ref().is_some_and(|p| removed.contains(p))
            {
                deletions.push(saved.id.clone());
                removed.insert(saved.id.clone());
            }
        }
        self.connections.retain(|c| !removed.contains(&c.id));
        self.folders.retain(|f| !removed.contains(&f.id));
        Ok(deletions)
    }

    /// Resolve both settings in one call. Each setting uses its nearest explicit ancestor independently.
    pub fn effective_settings(&self, id: &str) -> Result<EffectiveSettings<'_>, Error> {
        let saved = self
            .connections
            .iter()
            .find(|c| c.id == id)
            .ok_or(Error::NotFound)?;
        let mut colour = saved.connection.colour.as_deref();
        let mut danger = saved.connection.danger_check;
        let mut parent = saved.parent.as_deref();
        while let Some(id) = parent {
            let folder = self
                .folders
                .iter()
                .find(|f| f.id == id)
                .ok_or(Error::NotFound)?;
            colour = colour.or(folder.colour.as_deref());
            danger = danger.or(folder.danger_check);
            parent = folder.parent.as_deref();
        }
        Ok(EffectiveSettings {
            colour,
            danger_check: danger.unwrap_or(true),
        })
    }

    fn check_parent(&self, parent: Option<&str>) -> Result<(), Error> {
        if parent.is_some_and(|id| !self.folders.iter().any(|f| f.id == id)) {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        let mut ids = HashSet::new();
        for id in self
            .folders
            .iter()
            .map(|f| &f.id)
            .chain(self.connections.iter().map(|c| &c.id))
        {
            nonblank(id)?;
            if !ids.insert(id) {
                return Err(Error::Invalid("duplicate ID"));
            }
        }
        for folder in &self.folders {
            nonblank(&folder.name)?;
            let mut ancestors = HashSet::from([folder.id.as_str()]);
            let mut parent = folder.parent.as_deref();
            while let Some(id) = parent {
                if !ancestors.insert(id) {
                    return Err(Error::Invalid("folder cycle"));
                }
                parent = self
                    .folders
                    .iter()
                    .find(|f| f.id == id)
                    .ok_or(Error::Invalid("missing parent"))?
                    .parent
                    .as_deref();
            }
        }
        for saved in &self.connections {
            saved.connection.validate()?;
            self.check_parent(saved.parent.as_deref())?;
        }
        Ok(())
    }
}

/// Resolve the default filename without reading or creating it. Windows uses APPDATA.
pub fn default_path() -> Result<PathBuf, Error> {
    fn variable(name: &str) -> Option<PathBuf> {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(windows)]
    let root = variable("APPDATA");
    #[cfg(not(windows))]
    let root = variable("XDG_CONFIG_HOME").or_else(|| variable("HOME").map(|p| p.join(".config")));
    Ok(root
        .ok_or(Error::AppData)?
        .join("Database IDE")
        .join("connections.toml"))
}

fn nonblank(value: &str) -> Result<(), Error> {
    if value.trim().is_empty() {
        Err(Error::Invalid("name, host and ID must not be blank"))
    } else {
        Ok(())
    }
}

fn new_id() -> String {
    format!(
        "{:016x}{:016x}",
        RandomState::new().hash_one(0),
        RandomState::new().hash_one(1)
    )
}

mod engine_serde {
    use super::*;
    pub(super) fn serialize<S: serde::Serializer>(
        engine: &Engine,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match engine {
            Engine::SqlServer => "sql_server",
            Engine::PostgreSql => "postgre_sql",
            Engine::MySql => "my_sql",
        })
    }
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Engine, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "sql_server" => Ok(Engine::SqlServer),
            "postgre_sql" => Ok(Engine::PostgreSql),
            "my_sql" => Ok(Engine::MySql),
            _ => Err(serde::de::Error::custom("unknown database engine")),
        }
    }
}
