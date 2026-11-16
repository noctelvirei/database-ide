//! Blocking PostgreSQL test databases, shared while any database handle is alive.
//!
//! Requires initdb, pg_ctl, postgres and psql in DBIDE_PG_BIN. Drop closes each
//! database and the final handle stops the server, including during unwinding.

use std::fmt;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, Weak};

use tempfile::{Builder, TempDir};

static SERVER: Mutex<Weak<Server>> = Mutex::new(Weak::new());
const HOST: &str = "127.0.0.1";
const USER: &str = "dbide_test";

/// A harness failure; command errors retain server diagnostics for the test report.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Set DBIDE_PG_BIN to a folder containing all four PostgreSQL programs.
    #[error("Set DBIDE_PG_BIN to the PostgreSQL binary folder")]
    MissingBin,
    /// A required program was absent from the configured folder.
    #[error("DBIDE_PG_BIN is missing {0}")]
    MissingProgram(&'static str),
    /// A filesystem, process or port allocation operation failed.
    #[error("PostgreSQL harness I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// An external program failed; includes its output but never its environment.
    #[error("{program} failed: {output}")]
    Command {
        /// Program whose exit status was unsuccessful.
        program: &'static str,
        /// Exit status and stdout/stderr diagnostics.
        output: String,
    },
    /// A panic interrupted server acquisition; subsequent tests must fail explicitly.
    #[error("PostgreSQL test server lock was poisoned")]
    Poisoned,
}

struct Server {
    // All programs come from the same configured installation.
    bin: PathBuf,
    // Taken only when shutdown fails, preserving a possibly live server's files.
    folder: Option<TempDir>,
    // Fixed for this server's lifetime.
    port: u16,
    // Shared by its databases; never included in Debug output.
    password: String,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("folder", &self.folder)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// Owns one fresh database. Independent handles share a server, never a database.
/// Operations block; async tests should call them from a blocking task.
#[derive(Debug)]
pub struct Postgres {
    // Keeps the server alive until this database has been dropped.
    server: Arc<Server>,
    // Created for this handle alone, so tests cannot share tables accidentally.
    database: String,
}

impl Postgres {
    /// Creates a database, starting a server if no handle remains alive.
    /// Fails when DBIDE_PG_BIN is unset, a program fails or temporary I/O fails.
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
            .into_owned();
        server.psql(
            "postgres",
            &["-c", &format!("CREATE DATABASE \"{database}\"")],
        )?;
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

    /// Superuser restricted to this throwaway server.
    pub fn user(&self) -> &str {
        USER
    }

    /// Random password for SCRAM sign-in; omitted from Debug output.
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

    /// Runs SQL through psql -tAc, returning headerless UTF8 output.
    /// Any SQL error fails the call; queries may contain multiple statements.
    pub fn query(&self, sql: &str) -> Result<String, Error> {
        let output = self.server.psql(&self.database, &["-tAc", sql])?;
        Ok(String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"))
    }

    /// Loads a script with psql -v ON_ERROR_STOP=1 -f; SQL errors fail the call.
    /// The path is passed as an argument, never interpolated into SQL or a shell.
    pub fn load_script(&self, script: impl AsRef<Path>) -> Result<(), Error> {
        let mut command = self.server.psql_command(&self.database);
        command.arg("-f").arg(script.as_ref());
        run("psql", &mut command)?;
        Ok(())
    }
}

impl Drop for Postgres {
    fn drop(&mut self) {
        if let Err(error) = self.server.psql(
            "postgres",
            &[
                "-c",
                &format!("DROP DATABASE \"{}\" WITH (FORCE)", self.database),
            ],
        ) {
            eprintln!("Could not drop PostgreSQL test database: {error}");
        }
    }
}

impl Server {
    // The folder remains owned until shutdown finishes or explicitly retains it.
    fn folder(&self) -> &Path {
        self.folder
            .as_ref()
            .expect("server owns its temp folder until drop")
            .path()
    }

    // Initializes SCRAM sign-in and waits for a loopback-only server to be ready.
    fn start() -> Result<Self, Error> {
        let bin = std::env::var_os("DBIDE_PG_BIN")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(Error::MissingBin)?;
        for program in ["initdb", "pg_ctl", "postgres", "psql"] {
            if !binary(&bin, program).is_file() {
                return Err(Error::MissingProgram(program));
            }
        }
        let folder = Builder::new()
            .prefix("dbide_pg_")
            .rand_bytes(24)
            .tempdir()?;
        let password_file = Builder::new().rand_bytes(32).tempfile_in(folder.path())?;
        let password = password_file
            .path()
            .file_name()
            .expect("tempdir has a name")
            .to_string_lossy()
            .into_owned();
        fs::write(password_file.path(), &password)?;
        let data = folder.path().join("data");
        run(
            "initdb",
            Command::new(binary(&bin, "initdb"))
                .arg("-D")
                .arg(&data)
                .arg("-U")
                .arg(USER)
                .args(["--auth=scram-sha-256", "--encoding=UTF8", "--no-locale"])
                .arg("--pwfile")
                .arg(password_file.path()),
        )?;
        password_file.close()?;
        let listener = TcpListener::bind((HOST, 0))?;
        let port = listener.local_addr()?.port();
        fs::write(
            data.join("postgresql.auto.conf"),
            format!("listen_addresses = '{HOST}'\nport = {port}\nunix_socket_directories = ''\n"),
        )?;
        let server = Self {
            bin,
            folder: Some(folder),
            port,
            password,
        };
        drop(listener);
        // The server inherits pg_ctl's standard handles and keeps them open, so capturing
        // pg_ctl's output would wait for the server to exit. The log file carries diagnostics.
        let start = Command::new(binary(&server.bin, "pg_ctl"))
            .arg("start")
            .arg("-D")
            .arg(&data)
            .arg("-p")
            .arg(binary(&server.bin, "postgres"))
            .arg("-l")
            .arg(server.folder().join("server.log"))
            .args(["-w", "-t", "60"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(Error::from)
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(Error::Command {
                        program: "pg_ctl",
                        output: status.to_string(),
                    })
                }
            });
        if let Err(error) = start {
            eprintln!(
                "PostgreSQL startup log: {}",
                fs::read_to_string(server.folder().join("server.log")).unwrap_or_default()
            );
            return Err(error);
        }
        Ok(server)
    }

    // Ignores psql startup files and inherited connection overrides.
    fn psql_command(&self, database: &str) -> Command {
        let mut command = Command::new(binary(&self.bin, "psql"));
        command
            .args([
                "-X",
                "-w",
                "-h",
                HOST,
                "-p",
                &self.port.to_string(),
                "-U",
                USER,
                "-d",
                database,
                "-v",
                "ON_ERROR_STOP=1",
            ])
            .env("PGPASSWORD", &self.password)
            .env("PGCLIENTENCODING", "UTF8")
            .env("PGCONNECT_TIMEOUT", "5")
            .env("PGSSLMODE", "disable")
            .env("PGGSSENCMODE", "disable")
            .env_remove("PGHOSTADDR")
            .env_remove("PGSERVICE")
            .env_remove("PGOPTIONS");
        command
    }

    // All SQL commands fail on the first error and carry the same credentials.
    fn psql(&self, database: &str, args: &[&str]) -> Result<Output, Error> {
        run("psql", self.psql_command(database).args(args))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let data = self.folder().join("data");
        if data.join("postmaster.pid").exists()
            && let Err(error) = run(
                "pg_ctl",
                Command::new(binary(&self.bin, "pg_ctl"))
                    .arg("stop")
                    .arg("-D")
                    .arg(data)
                    .args(["-m", "fast", "-w", "-t", "60"]),
            )
        {
            // Preserve live server files rather than deleting data after a failed stop.
            if let Some(folder) = self.folder.take() {
                eprintln!(
                    "PostgreSQL stop failed; retained {:?}: {error}",
                    folder.keep()
                );
            }
        }
    }
}

// The installation directory is read-only; platform suffixes are resolved here.
fn binary(bin: &Path, name: &str) -> PathBuf {
    bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

// Captures diagnostics without formatting command arguments or credentials.
fn run(program: &'static str, command: &mut Command) -> Result<Output, Error> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(Error::Command {
            program,
            output: format!(
                "{}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }
    Ok(output)
}
