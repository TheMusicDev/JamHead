use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// JamHead: tiny music brains. A read-optimized MusicBrainz-style API.
#[derive(Parser)]
#[command(name = "jamhead", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Answer API requests from a built data directory.
    Serve {
        #[arg(long, default_value = "./data")]
        data: PathBuf,
        #[arg(long, default_value = "0.0.0.0:8080")]
        bind: String,
    },
    /// Build a new versioned data directory from MusicBrainz JSON dumps.
    Ingest {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "./data")]
        out: PathBuf,
        /// Only read the first N records of each dump (small dev sample).
        #[arg(long)]
        limit: Option<u64>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve { .. } => anyhow::bail!("serve: not implemented yet"),
        Command::Ingest { .. } => anyhow::bail!("ingest: not implemented yet"),
    }
}
