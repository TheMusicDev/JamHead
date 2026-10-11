//! Build a new versioned data directory from MusicBrainz JSON dumps.
//!
//! Streaming: a reader thread decompresses the dump and hands over batches of lines,
//! rayon parses a batch in parallel, and the batch is written to LMDB. Memory use is a
//! few batches plus the small name table used to build the search index.

use crate::datadir::{self, Meta, SCHEMA_VERSION};
use crate::mbid::{self, Mbid};
use crate::model::Artist;
use crate::norm::normalize;
use crate::search::{self, Doc};
use crate::store::{Kind, LmdbWriter, StoreWriter};
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::time::Instant;

pub struct Options {
    pub input: PathBuf,
    pub out: PathBuf,
    pub limit: Option<u64>,
    pub include_supplementary: bool,
}

/// Read-only facts shared by all parser threads.
pub struct Ctx {
    pub supplementary: bool,
    /// Number of children per parent id (for example release groups per artist),
    /// filled in by earlier passes.
    pub child_counts: HashMap<Mbid, u32>,
}

/// One parsed dump line, ready to store and index.
struct Record {
    id: Mbid,
    blob: Vec<u8>,
    doc: Doc,
}

fn parse(kind: Kind, line: &[u8], ctx: &Ctx) -> Result<Record> {
    match kind {
        Kind::Artist => parse_artist(line, ctx),
        _ => bail!("{} ingest is not implemented yet", kind.api_name()),
    }
}

fn parse_artist(line: &[u8], ctx: &Ctx) -> Result<Record> {
    let mut a: Artist = serde_json::from_slice(line)?;
    let id = mbid::parse(&a.id)?;
    let count = ctx.child_counts.get(&id).copied();
    a.release_group_count = count;
    a.tidy(ctx.supplementary);
    let mut keys: Vec<String> = std::iter::once(a.name.as_str())
        .chain(a.sort_name.as_deref())
        .chain(a.aliases.iter().map(|x| x.name.as_str()))
        .map(normalize)
        .filter(|k| !k.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    let doc = Doc {
        id,
        name: a.name.clone(),
        detail: a.disambiguation.clone().unwrap_or_default(),
        score: count.unwrap_or(0),
        keys,
    };
    Ok(Record {
        id,
        blob: serde_json::to_vec(&a)?,
        doc,
    })
}

// ---- reading dumps ----

/// Find the dump for `name`: the original `.tar.xz`, or an already extracted file.
fn locate(input: &Path, name: &str) -> Option<PathBuf> {
    [
        input.join(format!("{name}.tar.xz")),
        input.join("mbdump").join(name),
        input.join(name),
        input.join(format!("{name}.jsonl")),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

type Batch = Result<Vec<Vec<u8>>>;

fn feed<R: BufRead>(mut r: R, limit: Option<u64>, tx: &SyncSender<Batch>) -> Result<()> {
    let (mut chunk, mut bytes, mut n) = (Vec::new(), 0usize, 0u64);
    let mut line = Vec::new();
    while limit.is_none_or(|l| n < l) {
        line.clear();
        if r.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        n += 1;
        bytes += line.len();
        chunk.push(std::mem::take(&mut line));
        if chunk.len() >= 4096 || bytes >= 32 << 20 {
            bytes = 0;
            if tx.send(Ok(std::mem::take(&mut chunk))).is_err() {
                return Ok(()); // consumer stopped
            }
        }
    }
    if !chunk.is_empty() {
        let _ = tx.send(Ok(chunk));
    }
    Ok(())
}

fn read_dump(path: &Path, name: &str, limit: Option<u64>, tx: &SyncSender<Batch>) -> Result<()> {
    let file = File::open(path)?;
    if path.to_string_lossy().ends_with(".tar.xz") {
        let xz = xz2::read::XzDecoder::new_multi_decoder(BufReader::with_capacity(1 << 20, file));
        let mut archive = tar::Archive::new(xz);
        for entry in archive.entries()? {
            let entry = entry?;
            let p = entry.path()?.into_owned();
            if p.file_name().is_some_and(|f| f == name) && entry.header().entry_type().is_file() {
                return feed(BufReader::with_capacity(1 << 20, entry), limit, tx);
            }
        }
        bail!("{} has no entry named {name}", path.display());
    }
    feed(BufReader::with_capacity(1 << 20, file), limit, tx)
}

// ---- one pass over one entity kind ----

struct KindResult {
    count: u64,
    docs: Vec<Doc>,
}

fn run_kind(
    kind: Kind,
    path: &Path,
    opts: &Options,
    ctx: &Ctx,
    writer: &mut dyn StoreWriter,
) -> Result<KindResult> {
    let (tx, rx) = sync_channel::<Batch>(2);
    let name = kind.api_name();
    let (limit, reader_path) = (opts.limit, path.to_path_buf());
    let reader = std::thread::spawn(move || {
        if let Err(e) = read_dump(&reader_path, name, limit, &tx) {
            let _ = tx.send(Err(e));
        }
    });

    let started = Instant::now();
    let (mut docs, mut bad) = (Vec::new(), 0u64);
    for batch in rx {
        let batch = batch?;
        let parsed: Vec<Result<Record>> = batch.par_iter().map(|l| parse(kind, l, ctx)).collect();
        let mut items = Vec::with_capacity(parsed.len());
        for r in parsed {
            match r {
                Ok(rec) => {
                    items.push((rec.id, rec.blob));
                    docs.push(rec.doc);
                }
                Err(e) => {
                    bad += 1;
                    if bad <= 10 {
                        eprintln!("{name}: skipping bad line: {e:#}");
                    }
                }
            }
        }
        writer.put_batch(kind, &items)?;
        if docs.len() % 100_000 < items.len() {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            eprintln!(
                "{name}: {} records ({:.0}/s)",
                docs.len(),
                docs.len() as f64 / secs
            );
        }
    }
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("reader thread panicked"))?;
    if bad > 0 {
        eprintln!("{name}: skipped {bad} unreadable lines");
    }
    Ok(KindResult {
        count: docs.len() as u64,
        docs,
    })
}

/// Order matters: later passes use counts produced by earlier ones.
const ORDER: [Kind; 1] = [Kind::Artist];

pub fn run(opts: &Options) -> Result<PathBuf> {
    fs::create_dir_all(&opts.out)?;
    let now = chrono::Utc::now();
    let version = format!("v{}", now.format("%Y-%m-%dT%H%M%S"));
    let build = opts.out.join(format!(".building-{version}"));
    let _ = fs::remove_dir_all(&build);
    fs::create_dir_all(build.join("fst"))?;

    let mut writer: Box<dyn StoreWriter> = Box::new(LmdbWriter::create(&build.join("store"))?);
    let ctx = Ctx {
        supplementary: opts.include_supplementary,
        child_counts: HashMap::new(),
    };
    let mut counts = BTreeMap::new();
    for kind in ORDER {
        let Some(path) = locate(&opts.input, kind.api_name()) else {
            eprintln!(
                "{}: no dump found in {}, skipping",
                kind.api_name(),
                opts.input.display()
            );
            continue;
        };
        let res = run_kind(kind, &path, opts, &ctx, writer.as_mut())?;
        search::build(&build.join("fst"), kind.table(), &res.docs)
            .with_context(|| format!("building {} name index", kind.api_name()))?;
        counts.insert(kind.api_name().to_string(), res.count);
    }
    if counts.is_empty() {
        bail!("no dumps found in {}", opts.input.display());
    }
    writer.finish()?;

    let meta = Meta {
        schema_version: SCHEMA_VERSION,
        data_version: version.clone(),
        dump_date: fs::read_to_string(opts.input.join("DUMP_DATE"))
            .ok()
            .map(|s| s.trim().to_string()),
        created_at: now.to_rfc3339(),
        supplementary: opts.include_supplementary,
        counts,
    };
    fs::write(build.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;

    let live = opts.out.join(&version);
    fs::rename(&build, &live)?;
    datadir::flip_current(&opts.out, &version)?;
    for old in datadir::prune(&opts.out, 2)? {
        eprintln!("pruned old version {old}");
    }
    eprintln!("ingest done: {version}");
    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datadir::Data;
    use crate::store::Kind;

    fn fixture_opts(out: &Path) -> Options {
        Options {
            input: PathBuf::from("tests/fixtures/dump"),
            out: out.to_path_buf(),
            limit: None,
            include_supplementary: false,
        }
    }

    #[test]
    fn ingest_fixture_then_read_back() {
        let out = tempfile::tempdir().unwrap();
        run(&fixture_opts(out.path())).unwrap();
        let data = Data::open(&datadir::resolve(out.path()).unwrap()).unwrap();
        let expected = fs::read_to_string("tests/fixtures/dump/mbdump/artist")
            .unwrap()
            .lines()
            .count() as u64;
        assert_eq!(data.meta.counts["artist"], expected);
        let first: serde_json::Value = serde_json::from_str(
            fs::read_to_string("tests/fixtures/dump/mbdump/artist")
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let id = mbid::parse(first["id"].as_str().unwrap()).unwrap();
        let blob = data.store.get(Kind::Artist, &id).unwrap().unwrap();
        let got: serde_json::Value = serde_json::from_slice(&blob).unwrap();
        assert_eq!(got["name"], first["name"]);
        assert!(got.get("relations").is_none());
    }

    #[test]
    fn limit_caps_records_per_dump() {
        let out = tempfile::tempdir().unwrap();
        let opts = Options {
            limit: Some(7),
            ..fixture_opts(out.path())
        };
        run(&opts).unwrap();
        let data = Data::open(&datadir::resolve(out.path()).unwrap()).unwrap();
        assert_eq!(data.meta.counts["artist"], 7);
    }

    #[test]
    fn a_bad_line_is_skipped_not_fatal() {
        let ctx = Ctx {
            supplementary: false,
            child_counts: HashMap::new(),
        };
        assert!(parse(Kind::Artist, b"{not json", &ctx).is_err());
        assert!(parse(Kind::Artist, br#"{"id":"nope","name":"x"}"#, &ctx).is_err());
    }
}
