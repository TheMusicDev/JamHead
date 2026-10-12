//! Operator commands: `verify`, `stats`, `update`, `healthcheck`.

use crate::compress::{decode, Encoding};
use crate::datadir::{self, Data, Meta};
use crate::download;
use crate::ingest;
use crate::mbid;
use crate::norm::normalize;
use crate::store::{Kind, Rel};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const SAMPLE: usize = 200;

fn read_meta(dir: &Path) -> Result<Meta> {
    Ok(serde_json::from_slice(
        &fs::read(dir.join("meta.json"))
            .with_context(|| format!("no meta.json in {}", dir.display()))?,
    )?)
}

struct Report {
    checks: u32,
    failures: u32,
}

impl Report {
    fn check(&mut self, ok: bool, what: impl std::fmt::Display) {
        self.checks += 1;
        if !ok {
            self.failures += 1;
            println!("FAIL  {what}");
        }
    }
}

/// Check a data directory: files match meta.json, counts add up, and a sample of entities
/// decodes, is searchable and has the child counts it claims. Returns true if all is well.
pub fn verify(root: &Path, deep: bool) -> Result<bool> {
    let dir = datadir::resolve(root)?;
    let meta = read_meta(&dir)?;
    println!("verifying {} ({})", dir.display(), meta.data_version);
    let mut r = Report {
        checks: 0,
        failures: 0,
    };

    for (rel, info) in &meta.files {
        let path = dir.join(rel);
        let size = fs::metadata(&path).map(|m| m.len());
        r.check(
            size.as_ref().ok() == Some(&info.size),
            format!("{rel}: size is {size:?}, meta.json says {}", info.size),
        );
        if let (true, Some(want), Ok(_)) = (deep, &info.sha256, &size) {
            let got = datadir::sha256_file(&path)?;
            r.check(
                &got == want,
                format!("{rel}: checksum differs from meta.json"),
            );
        }
    }
    println!(
        "{} files checked{}",
        meta.files.len(),
        if deep {
            " (with checksums)"
        } else {
            " (sizes only; use --deep for checksums)"
        }
    );

    let data = match Data::open(&dir) {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL  cannot open data: {e:#}");
            return Ok(false);
        }
    };
    for kind in Kind::ALL {
        let name = kind.api_name();
        let Some(&want) = meta.counts.get(name) else {
            continue;
        };
        let got = data.store.count(kind)?;
        r.check(
            got == want,
            format!("{name}: store holds {got} entities, meta.json says {want}"),
        );

        let sample = data.store.sample(kind, SAMPLE)?;
        let mut bad = (0, 0, 0, 0); // json, encodings, search, children
        for (id, plain) in &sample {
            let Ok(json) = serde_json::from_slice::<serde_json::Value>(plain) else {
                bad.0 += 1;
                continue;
            };
            if json["id"]
                .as_str()
                .map(mbid::parse)
                .transpose()
                .ok()
                .flatten()
                != Some(*id)
            {
                bad.0 += 1;
            }
            for enc in [Encoding::Zstd, Encoding::Gzip] {
                let ok = data
                    .store
                    .get(kind, id, enc)?
                    .is_some_and(|b| decode(enc, &b).is_ok_and(|d| &d == plain));
                if !ok {
                    bad.1 += 1;
                }
            }
            let title = json["name"]
                .as_str()
                .or(json["title"].as_str())
                .unwrap_or("");
            if !normalize(title).is_empty() {
                let in_fst = data
                    .names
                    .get(&kind)
                    .is_some_and(|n| n.lookup_exact(&normalize(title)).contains(id));
                let in_text = data
                    .text
                    .get(&kind)
                    .is_some_and(|t| t.search(title, 1).is_ok_and(|h| !h.is_empty()));
                if !(in_fst && in_text) {
                    bad.2 += 1;
                }
            }
            let (rel, field) = match kind {
                Kind::Artist => (Some(Rel::ArtistReleaseGroups), "release-group-count"),
                Kind::ReleaseGroup => (Some(Rel::ReleaseGroupReleases), "release-count"),
                Kind::Release => (None, ""),
            };
            if let Some(rel) = rel {
                let (total, _) = data.store.children(rel, id, 0, 1)?;
                if json[field].as_u64().unwrap_or(0) as usize != total {
                    bad.3 += 1;
                }
            }
        }
        r.check(
            bad.0 == 0,
            format!(
                "{name}: {} sampled blobs are not valid JSON with a matching id",
                bad.0
            ),
        );
        r.check(
            bad.1 == 0,
            format!(
                "{name}: {} zstd/gzip copies differ from the plain blob",
                bad.1
            ),
        );
        r.check(
            bad.2 == 0,
            format!(
                "{name}: {} sampled entities cannot be found by their own name",
                bad.2
            ),
        );
        r.check(
            bad.3 == 0,
            format!(
                "{name}: {} sampled entities disagree with their child list",
                bad.3
            ),
        );
        println!("{name}: {want} entities, {} sampled", sample.len());
    }
    println!("{} checks, {} failed", r.checks, r.failures);
    Ok(r.failures == 0)
}

fn human(bytes: u64) -> String {
    let mut v = bytes as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if v < 1024.0 || unit == "TB" {
            return format!("{v:.1} {unit}");
        }
        v /= 1024.0;
    }
    unreachable!()
}

/// Print what is in a data directory.
pub fn stats(root: &Path) -> Result<()> {
    let dir = datadir::resolve(root)?;
    let meta = read_meta(&dir)?;
    println!("data version  {}", meta.data_version);
    println!("directory     {}", dir.display());
    println!(
        "dump date     {}",
        meta.dump_date.as_deref().unwrap_or("unknown")
    );
    println!("created       {}", meta.created_at);
    println!("schema        {}", meta.schema_version);
    println!(
        "supplementary {}",
        if meta.supplementary {
            "yes (CC BY-NC-SA data included)"
        } else {
            "no (core CC0 data only)"
        }
    );
    for (name, n) in &meta.counts {
        println!("{name:<13} {n}");
    }
    let mut by_part = std::collections::BTreeMap::new();
    for (rel, info) in &meta.files {
        *by_part
            .entry(rel.split('/').next().unwrap_or(""))
            .or_insert(0u64) += info.size;
    }
    for (part, size) in &by_part {
        println!("size {part:<8} {}", human(*size));
    }
    println!("size total    {}", human(by_part.values().sum()));
    println!("\nstore tables (entries, bytes on disk, bytes per entry):");
    for (name, entries, bytes) in Data::open(&dir)?.store.table_sizes()? {
        let per = bytes.checked_div(entries).unwrap_or(0);
        println!("  {name:<20} {entries:>9} {:>10} {per:>8}", human(bytes));
    }
    let versions = fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.path().join("meta.json").exists())
        .count();
    println!("versions kept {versions}");
    Ok(())
}

pub struct UpdateOptions {
    pub base_url: String,
    pub dump: String,
    pub kinds: Vec<Kind>,
    pub data: PathBuf,
    pub dumps: PathBuf,
    pub limit: Option<u64>,
    pub include_supplementary: bool,
    pub keep_versions: usize,
    pub keep_dumps: bool,
    pub force: bool,
}

/// download -> ingest -> flip `current` (ingest does the flip and prunes old versions).
pub fn update(o: &UpdateOptions) -> Result<()> {
    let date = if o.dump == "latest" {
        download::latest(o.base_url.trim_end_matches('/'))?
    } else {
        o.dump.clone()
    };
    if !o.force {
        let live = datadir::resolve(&o.data)
            .ok()
            .and_then(|d| read_meta(&d).ok());
        if live.as_ref().and_then(|m| m.dump_date.as_deref()) == Some(date.as_str()) {
            println!("already up to date (dump {date}); use --force to rebuild anyway");
            return Ok(());
        }
    }
    println!("updating to dump {date}");
    let dump_dir = download::run(&download::Options {
        base_url: o.base_url.clone(),
        dump: date,
        kinds: o.kinds.clone(),
        out: o.dumps.clone(),
    })?;
    let version = ingest::run(&ingest::Options {
        input: dump_dir.clone(),
        out: o.data.clone(),
        limit: o.limit,
        include_supplementary: o.include_supplementary,
        keep_versions: o.keep_versions,
    })?;
    if !o.keep_dumps {
        fs::remove_dir_all(&dump_dir)?;
    }
    println!("now serving-ready: {}", version.display());
    Ok(())
}

/// For container health checks: exit 0 only if the URL answers 200.
pub fn healthcheck(url: &str) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(3)))
        .http_status_as_error(false)
        .build()
        .into();
    let res = agent.get(url).call()?;
    if res.status().as_u16() != 200 {
        bail!("{url}: HTTP {}", res.status());
    }
    Ok(())
}
