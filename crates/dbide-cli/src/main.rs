use clap::{Parser, Subcommand};
use dbide_engine::connections::connection_string;
use dbide_engine::export;
use dbide_engine::session::{Event, Session};
use std::path::PathBuf;

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
    /// Run a PostgreSQL batch and export each result set as a numbered CSV file.
    Run {
        /// libpq keyword string or PostgreSQL URI; password falls back to DBIDE_PASSWORD.
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
    Postgres(#[from] dbide_engine::postgres::Error),
    #[error(transparent)]
    Export(#[from] export::Error),
    #[error("Cannot read SQL file: {0}")]
    File(#[from] std::io::Error),
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
            let mut session = dbide_engine::postgres::connect(&parsed.connection, password).await?;
            let report =
                export::run_to_file(session.run(&batch), &output, &export::Format::default())
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
