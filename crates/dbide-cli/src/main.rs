use clap::{Parser, Subcommand};
use dbide_engine::connections::connection_string;
use dbide_engine::danger::Engine;
use dbide_engine::export;
use dbide_engine::format;
use dbide_engine::session::{self, Event};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print the package version.
    Version,
    /// Format SQL from a UTF-8 file or stdin and write it to stdout.
    Format {
        /// SQL syntax: sql-server, postgresql or mysql.
        #[arg(long, value_parser = parse_engine)]
        engine: Engine,
        /// Read this UTF-8 file instead of stdin.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Run a SQL Server, PostgreSQL or MySQL batch and export each result as a numbered CSV file.
    Run {
        /// ADO.NET, JDBC, libpq or database URI; password falls back to DBIDE_PASSWORD.
        #[arg(long)]
        connection: String,
        /// SQL batch, sent unchanged.
        #[arg(long, required_unless_present = "file", conflicts_with = "file")]
        batch: Option<String>,
        /// UTF-8 SQL file, sent unchanged.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Output stem: results.csv produces results-1.csv, results-2.csv, etc.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Connection(#[from] connection_string::Error),
    #[error(transparent)]
    Connect(#[from] session::ConnectError),
    #[error(transparent)]
    Export(#[from] export::Error),
    #[error(transparent)]
    Format(#[from] format::Error),
    #[error("SQL input/output failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Batch failed; see reported server errors")]
    Batch,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Error> {
    match Cli::parse().command {
        Commands::Version => println!("{}", env!("CARGO_PKG_VERSION")),
        Commands::Format { engine, file } => {
            let text = match file {
                Some(file) => std::fs::read_to_string(file)?,
                None => {
                    let mut text = String::new();
                    std::io::stdin().read_to_string(&mut text)?;
                    text
                }
            };
            let output = format::format(&text, engine)?;
            std::io::stdout().lock().write_all(output.as_bytes())?;
        }
        Commands::Run {
            connection,
            batch,
            file,
            output,
        } => {
            let parsed = connection_string::parse(&connection)?;
            let environment = std::env::var("DBIDE_PASSWORD").ok();
            let password = parsed
                .password
                .as_ref()
                .map(|secret| secret.expose())
                .or(environment.as_deref());
            let batch = match file {
                Some(file) => std::fs::read_to_string(file)?,
                None => batch.expect("clap requires batch or file"),
            };
            let mut session = session::connect(&parsed.connection, password).await?;
            let report = export::run_to_file(
                session.run(&batch),
                &output,
                &export::Format::default(),
                &AtomicU64::new(0),
            )
            .await?;
            let mut failed = false;
            for event in report.events {
                match event {
                    Event::Message { text, severity, .. } => println!("{severity:?}: {text}"),
                    Event::RowCount(count) => println!("{count} rows"),
                    Event::Error { text, line, code } => {
                        eprintln!(
                            "Error {} line {}: {text}",
                            code.as_deref().unwrap_or("unknown"),
                            line.map_or_else(|| "unknown".into(), |line| line.to_string())
                        );
                        failed = true;
                    }
                    _ => {}
                }
            }
            for file in report.files {
                println!("{}: {} rows", file.path.display(), file.rows);
            }
            if failed {
                return Err(Error::Batch);
            }
        }
    }
    Ok(())
}

fn parse_engine(value: &str) -> Result<Engine, String> {
    match value {
        "sql-server" => Ok(Engine::SqlServer),
        "postgresql" => Ok(Engine::PostgreSql),
        "mysql" => Ok(Engine::MySql),
        _ => Err("expected sql-server, postgresql or mysql".into()),
    }
}
