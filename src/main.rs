mod compress;
mod datadir;
mod download;
mod ingest;
mod mbid;
mod model;
mod norm;
mod ops;
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
        /// Seconds between checks for a newer `current` data version (0 = never).
        #[arg(long, default_value_t = 10)]
        watch_interval: u64,
        /// Enables POST /admin/reload (send it as `Authorization: Bearer <token>`).
        #[arg(long, env = "JAMHEAD_ADMIN_TOKEN", hide_env_values = true)]
        admin_token: Option<String>,
    },
    /// Download MusicBrainz JSON dumps (resumable, checksum-verified).
    Download {
        /// Where to put the dumps; they land in <out>/<dump date>/.
        #[arg(long, default_value = "./dumps")]
        out: PathBuf,
        /// "latest" or a dump directory name such as 20261010-001001.
        #[arg(long, default_value = "latest")]
        dump: String,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "artist,release-group,release"
        )]
        entities: Vec<String>,
        #[arg(long, default_value = download::DEFAULT_BASE_URL)]
        base_url: String,
    },
    /// Download the newest dump, build a new data version from it and make it current.
    Update {
        #[arg(long, default_value = "./data")]
        data: PathBuf,
        /// Scratch space for the downloaded dumps (about 25 GB for the full set).
        #[arg(long, default_value = "./dumps")]
        dumps: PathBuf,
        #[arg(long, default_value = "latest")]
        dump: String,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "artist,release-group,release"
        )]
        entities: Vec<String>,
        #[arg(long, default_value = download::DEFAULT_BASE_URL)]
        base_url: String,
        #[arg(long)]
        limit: Option<u64>,
        #[arg(long)]
        include_supplementary: bool,
        /// Keep the downloaded dump after a successful ingest (it is deleted by default).
        #[arg(long)]
        keep_dumps: bool,
        /// Rebuild even if the live data already comes from this dump.
        #[arg(long)]
        force: bool,
    },
    /// Check a data directory for damage and run sanity queries. Exit code 1 if anything is wrong.
    Verify {
        #[arg(long, default_value = "./data")]
        data: PathBuf,
        /// Also check the sha256 of every file (slow on big data).
        #[arg(long)]
        deep: bool,
    },
    /// Print counts, sizes and versions of a data directory.
    Stats {
        #[arg(long, default_value = "./data")]
        data: PathBuf,
    },
    /// Exit 0 if the server answers its health URL (for Docker health checks).
    Healthcheck {
        #[arg(long, default_value = "http://127.0.0.1:8080/healthz")]
        url: String,
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

fn kinds(names: &[String]) -> anyhow::Result<Vec<store::Kind>> {
    names
        .iter()
        .map(|n| {
            store::Kind::from_api_name(n).ok_or_else(|| {
                anyhow::anyhow!("unknown entity {n:?} (use artist, release-group, release)")
            })
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve {
            data,
            bind,
            watch_interval,
            admin_token,
        } => {
            let opts = server::ServeOptions {
                root: data,
                bind,
                watch_secs: watch_interval,
                admin_token,
            };
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(server::serve(opts))
        }
        Command::Download {
            out,
            dump,
            entities,
            base_url,
        } => {
            download::run(&download::Options {
                base_url,
                dump,
                kinds: kinds(&entities)?,
                out,
            })?;
            Ok(())
        }
        Command::Update {
            data,
            dumps,
            dump,
            entities,
            base_url,
            limit,
            include_supplementary,
            keep_dumps,
            force,
        } => ops::update(&ops::UpdateOptions {
            base_url,
            dump,
            kinds: kinds(&entities)?,
            data,
            dumps,
            limit,
            include_supplementary,
            keep_dumps,
            force,
        }),
        Command::Verify { data, deep } => {
            if ops::verify(&data, deep)? {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        Command::Stats { data } => ops::stats(&data),
        Command::Healthcheck { url } => ops::healthcheck(&url),
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
