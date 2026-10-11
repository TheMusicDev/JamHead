mod datadir;
mod ingest;
mod mbid;
mod model;
mod norm;
mod search;
mod server;
mod store;
mod text;

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
        /// Also keep tags, genres, ratings and annotations. These are CC BY-NC-SA
        /// (non-commercial, share-alike); the default is core CC0 data only.
        #[arg(long)]
        include_supplementary: bool,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve { data, bind } => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(server::serve(data, bind)),
        Command::Ingest {
            input,
            out,
            limit,
            include_supplementary,
        } => {
            ingest::run(&ingest::Options {
                input,
                out,
                limit,
                include_supplementary,
            })?;
            Ok(())
        }
    }
}
