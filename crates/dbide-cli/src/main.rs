use clap::{Parser, Subcommand};
use dbide_engine as _;

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
}

fn main() {
    match Cli::parse().command {
        Commands::Version => println!("{}", env!("CARGO_PKG_VERSION")),
    }
}
