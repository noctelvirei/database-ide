//! Windows Credential Manager access, keyed by stable saved-connection IDs.

use super::{SavedConnection, SignInMethod};

/// Credential errors never include the password.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Native Credential Manager rejected an operation.
    #[error("credential store: {0}")]
    Keyring(String),
    /// Native credentials are not supported on this platform yet.
    #[error("credential storage requires Windows")]
    Unsupported,
    /// An empty service or connection ID is not a valid credential key.
    #[error("credential service and connection ID must not be empty")]
    InvalidKey,
}

impl From<keyring::Error> for Error {
    fn from(error: keyring::Error) -> Self {
        // Discard raw errors: BadEncoding contains password bytes, including in its source chain.
        Self::Keyring(error.to_string())
    }
}

/// An isolated credential namespace; production uses database-ide, tests database-ide-test.
#[derive(Debug)]
pub struct Credentials {
    service: String,
}

impl Default for Credentials {
    fn default() -> Self {
        Self {
            service: "database-ide".into(),
        }
    }
}

impl Credentials {
    /// Select an explicit service name; tests must use database-ide-test.
    pub fn new(service: impl Into<String>) -> Result<Self, Error> {
        let service = service.into();
        if service.is_empty() {
            return Err(Error::InvalidKey);
        }
        Ok(Self { service })
    }

    /// Store only for password sign-in. Other methods delete any old entry and store nothing.
    pub fn store(&self, saved: &SavedConnection, password: &str) -> Result<(), Error> {
        if saved.connection.sign_in != SignInMethod::Password {
            return self.delete(&saved.id);
        }
        self.entry(&saved.id)?.set_password(password)?;
        Ok(())
    }

    /// Read a saved password; absent entries and non-password sign-in return None.
    /// The returned secret redacts Debug output.
    pub fn read(&self, saved: &SavedConnection) -> Result<Option<Secret>, Error> {
        if saved.connection.sign_in != SignInMethod::Password {
            return Ok(None);
        }
        match self.entry(&saved.id)?.get_password() {
            Ok(password) => Ok(Some(Secret(password))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Delete by stable connection ID; absent entries succeed.
    pub fn delete(&self, id: &str) -> Result<(), Error> {
        match self.entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn entry(&self, id: &str) -> Result<keyring::Entry, Error> {
        if id.is_empty() {
            return Err(Error::InvalidKey);
        }
        if !cfg!(windows) {
            return Err(Error::Unsupported);
        }
        Ok(keyring::Entry::new(&self.service, id)?)
    }
}

/// A password in memory, excluded from serialization and redacted in Debug.
pub struct Secret(pub(super) String);

impl Secret {
    /// Borrow the plaintext only when passing it to a driver or credential store.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret([redacted])")
    }
}
