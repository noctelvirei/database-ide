//! Blocking MySQL test databases, shared while any database handle is alive.
//!
//! Requires mysqld, mysql and mysqladmin in DBIDE_MYSQL_BIN. The final handle
//! shuts the server down before removing its temporary files.

use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use tempfile::{Builder, TempDir};

static SERVER: Mutex<Weak<Server>> = Mutex::new(Weak::new());
const HOST: &str = "127.0.0.1";
const USER: &str = "dbide_test";

/// A harness failure, with command diagnostics and credentials redacted.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Set DBIDE_MYSQL_BIN to a folder containing all three MySQL programs.
    #[error("Set DBIDE_MYSQL_BIN to the MySQL binary folder")]
    MissingBin,
    /// A required program was absent from the configured folder.
    #[error("DBIDE_MYSQL_BIN is missing {0}")]
    MissingProgram(&'static str),
    /// A filesystem, process or port allocation operation failed.
    #[error("MySQL harness I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// An external program failed, retaining its output without credentials.
    #[error("{program} failed: {output}")]
    Command {
        /// Program whose exit status was unsuccessful.
        program: &'static str,
        /// Exit status and stdout/stderr diagnostics.
        output: String,
    },
    /// A panic interrupted server acquisition.
    #[error("MySQL test server lock was poisoned")]
    Poisoned,
}

struct Server {
    bin: PathBuf,
    folder: Option<TempDir>,
    port: u16,
    password: String,
    child: Child,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("folder", &self.folder)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// Owns one fresh database; independent handles share a server, never a database.
/// Operations block; async tests should call them from a blocking task.
#[derive(Debug)]
pub struct MySql {
    server: Arc<Server>,
    database: String,
}

impl MySql {
    /// Creates a database, starting a server if no handle remains alive.
    /// Fails when DBIDE_MYSQL_BIN is unset, a program fails or temporary I/O fails.
    pub fn start() -> Result<Self, Error> {
        let mut shared = SERVER.lock().map_err(|_| Error::Poisoned)?;
        let server = match shared.upgrade() {
            Some(server) => server,
            None => {
                let server = Arc::new(Server::start()?);
                *shared = Arc::downgrade(&server);
                server
            }
        };
        let name = Builder::new()
            .prefix("dbide_test_")
            .rand_bytes(24)
            .tempdir_in(server.folder())?;
        let database = name
            .path()
            .file_name()
            .expect("tempdir has a name")
            .to_string_lossy()
            // MySQL on Windows stores database names in lower case (lower_case_table_names=1).
            .to_lowercase();
        server.sql(None, &format!("CREATE DATABASE `{database}`"))?;
        Ok(Self { server, database })
    }

    /// Loopback address; the server does not listen on external interfaces.
    pub fn host(&self) -> &str {
        HOST
    }

    /// TCP port allocated for this server's lifetime.
    pub fn port(&self) -> u16 {
        self.server.port
    }

    /// Test user with privileges on this harness's databases only.
    pub fn user(&self) -> &str {
        USER
    }

    /// Random sign-in password; omitted from Debug output and command arguments.
    pub fn password(&self) -> &str {
        &self.server.password
    }

    /// Unique database name; valid only while this handle remains alive.
    pub fn database(&self) -> &str {
        &self.database
    }

    /// Server's temporary root, deleted after its final database handle drops.
    pub fn server_folder(&self) -> &Path {
        self.server.folder()
    }

    /// Runs SQL through mysql, returning headerless UTF8 tab-separated output.
    /// Any SQL error fails the call; queries may contain multiple statements.
    pub fn query(&self, sql: &str) -> Result<String, Error> {
        let output = self.server.sql(Some(&self.database), sql)?;
        Ok(String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"))
    }

    /// Loads a script through mysql's standard input, stopping on the first error.
    /// The path is opened directly, never interpolated into SQL or a shell.
    pub fn load_script(&self, script: impl AsRef<Path>) -> Result<(), Error> {
        let mut command = self.server.mysql_command(Some(&self.database), false);
        command.stdin(File::open(script)?);
        checked("mysql", command.output()?, &self.server.password)?;
        Ok(())
    }
}

impl Drop for MySql {
    fn drop(&mut self) {
        if let Err(error) = self
            .server
            .sql(None, &format!("DROP DATABASE `{}`", self.database))
        {
            eprintln!("Could not drop MySQL test database: {error}");
        }
    }
}

impl Server {
    fn folder(&self) -> &Path {
        self.folder
            .as_ref()
            .expect("server owns its temp folder until drop")
            .path()
    }

    fn start() -> Result<Self, Error> {
        let bin = std::env::var_os("DBIDE_MYSQL_BIN")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(Error::MissingBin)?;
        for program in ["mysqld", "mysql", "mysqladmin"] {
            if !binary(&bin, program).is_file() {
                return Err(Error::MissingProgram(program));
            }
        }
        let folder = Builder::new()
            .prefix("dbide_mysql_")
            .rand_bytes(24)
            .tempdir()?;
        let secret = Builder::new().rand_bytes(32).tempfile_in(folder.path())?;
        let password = secret
            .path()
            .file_name()
            .expect("tempfile has a name")
            .to_string_lossy()
            .into_owned();
        secret.close()?;
        let data = folder.path().join("data");
        checked(
            "mysqld",
            Command::new(binary(&bin, "mysqld"))
                .args(["--no-defaults", "--initialize-insecure"])
                .arg(format!("--datadir={}", data.display()))
                .output()?,
            &password,
        )?;
        let listener = TcpListener::bind((HOST, 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        // Captured pipes remain open for the daemon's lifetime; use its log instead.
        let child = Command::new(binary(&bin, "mysqld"))
            .arg("--no-defaults")
            .arg(format!("--datadir={}", data.display()))
            .arg(format!("--port={port}"))
            .arg(format!("--bind-address={HOST}"))
            .arg("--mysqlx=0")
            .arg("--skip-log-bin")
            .arg(format!(
                "--log-error={}",
                folder.path().join("server.log").display()
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut server = Self {
            bin,
            folder: Some(folder),
            port,
            password,
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(output) = server
                .mysql_command(None, true)
                .arg("--execute=SELECT 1")
                .output()
                && output.status.success()
            {
                break;
            }
            if server.child.try_wait()?.is_some() || Instant::now() >= deadline {
                return Err(Error::Command {
                    program: "mysqld",
                    output: fs::read_to_string(server.folder().join("server.log"))
                        .unwrap_or_else(|_| "Server did not become ready within 60 seconds".into()),
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // SQL is sent through stdin so the password never appears in process arguments.
        let setup = format!(
            "CREATE USER '{USER}'@'%' IDENTIFIED BY '{}'; GRANT ALL PRIVILEGES ON `dbide_test_%`.* TO '{USER}'@'%';",
            server.password.replace('\\', "\\\\").replace('\'', "''")
        );
        server.sql_as(None, &setup, true)?;
        Ok(server)
    }

    fn mysql_command(&self, database: Option<&str>, root: bool) -> Command {
        let mut command = self.client_command("mysql", root);
        command.args([
            "--batch",
            "--raw",
            "--skip-column-names",
            "--default-character-set=utf8mb4",
        ]);
        if let Some(database) = database {
            command.arg(format!("--database={database}"));
        }
        command
    }

    fn client_command(&self, program: &str, root: bool) -> Command {
        let mut command = Command::new(binary(&self.bin, program));
        command
            .args([
                "--no-defaults",
                "--no-login-paths",
                "--protocol=TCP",
                "--host=127.0.0.1",
                "--connect-timeout=5",
                "--ssl-mode=DISABLED",
                // caching_sha2_password over a plain connection needs the server's RSA key.
                "--get-server-public-key",
            ])
            .arg(format!("--port={}", self.port))
            .arg(format!("--user={}", if root { "root" } else { USER }))
            .env("MYSQL_PWD", if root { "" } else { &self.password })
            .env_remove("MYSQL_TEST_LOGIN_FILE");
        command
    }

    fn sql(&self, database: Option<&str>, sql: &str) -> Result<Output, Error> {
        self.sql_as(database, sql, false)
    }

    fn sql_as(&self, database: Option<&str>, sql: &str, root: bool) -> Result<Output, Error> {
        let mut child = self
            .mysql_command(database, root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let write = child
            .stdin
            .take()
            .expect("stdin was configured as piped")
            .write_all(sql.as_bytes());
        let output = child.wait_with_output()?;
        let output = checked("mysql", output, &self.password)?;
        write?;
        Ok(output)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        let shutdown = self
            .client_command("mysqladmin", true)
            .args(["--shutdown-timeout=10", "shutdown"])
            .output()
            .map_err(Error::from)
            .and_then(|output| checked("mysqladmin", output, &self.password));
        let deadline = Instant::now() + Duration::from_secs(15);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            // Keep a live server's data when shutdown fails, matching the PG harness.
            if let Some(folder) = self.folder.take() {
                eprintln!(
                    "MySQL stop failed; retained {:?}: {shutdown:?}",
                    folder.keep()
                );
            }
        }
    }
}

fn binary(bin: &Path, name: &str) -> PathBuf {
    bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn checked(program: &'static str, output: Output, password: &str) -> Result<Output, Error> {
    if !output.status.success() {
        return Err(Error::Command {
            program,
            output: format!(
                "{}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .replace(password, "[redacted]"),
        });
    }
    Ok(output)
}
