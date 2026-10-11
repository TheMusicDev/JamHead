//! Build a new versioned data directory from MusicBrainz JSON dumps.
//!
//! Streaming: a reader thread decompresses the dump and hands over batches of lines,
//! rayon parses a batch in parallel, and the batch is written to LMDB. Memory use is a
//! few batches plus the small name table used to build the search index.

use crate::compress::Blob;
#[cfg(test)]
use crate::compress::Encoding;
use crate::datadir::{self, Meta, SCHEMA_VERSION};
use crate::mbid::{self, Mbid};
use crate::model::{credit_text, Artist, Release, ReleaseGroup};
use crate::norm::normalize;
use crate::search::{self, Doc};
use crate::store::{Kind, LmdbWriter, Rel, StoreWriter};
use crate::text;
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
    blob: Blob,
    doc: Doc,
    /// Ids of the parents this record belongs to (artists of a release group, group of a release).
    parents: Vec<Mbid>,
    /// Position in the parents' child lists, see `date_key`.
    sort: u32,
}

fn parse(kind: Kind, line: &[u8], ctx: &Ctx) -> Result<Record> {
    match kind {
        Kind::Artist => parse_artist(line, ctx),
        Kind::ReleaseGroup => parse_release_group(line, ctx),
        Kind::Release => parse_release(line, ctx),
    }
}

/// The list a kind's records are filed under in their parents.
fn parent_rel(kind: Kind) -> Option<Rel> {
    match kind {
        Kind::Artist => None,
        Kind::ReleaseGroup => Some(Rel::ArtistReleaseGroups),
        Kind::Release => Some(Rel::ReleaseGroupReleases),
    }
}

/// "1994-09-27" -> 19940927, "1994" -> 19940000. No date sorts last.
fn date_key(date: Option<&str>) -> u32 {
    let Some(date) = date else { return u32::MAX };
    let mut parts = date.split('-').map(|p| p.parse::<u32>().unwrap_or(0));
    let (y, m, d) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    if y == 0 {
        u32::MAX
    } else {
        y * 10_000 + m * 100 + d
    }
}

fn unique_keys<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut keys: Vec<String> = names.map(normalize).filter(|k| !k.is_empty()).collect();
    keys.sort();
    keys.dedup();
    keys
}

fn parse_artist(line: &[u8], ctx: &Ctx) -> Result<Record> {
    let mut a: Artist = serde_json::from_slice(line)?;
    let id = mbid::parse(&a.id)?;
    let count = ctx.child_counts.get(&id).copied();
    a.release_group_count = count;
    a.tidy(ctx.supplementary);
    let keys = unique_keys(
        std::iter::once(a.name.as_str())
            .chain(a.sort_name.as_deref())
            .chain(a.aliases.iter().map(|x| x.name.as_str())),
    );
    let doc = Doc {
        id,
        name: a.name.clone(),
        detail: a.disambiguation.clone().unwrap_or_default(),
        score: count.unwrap_or(0),
        keys,
    };
    Ok(Record {
        id,
        blob: Blob::new(serde_json::to_vec(&a)?)?,
        doc,
        parents: vec![],
        sort: 0,
    })
}

fn parse_release_group(line: &[u8], ctx: &Ctx) -> Result<Record> {
    let mut g: ReleaseGroup = serde_json::from_slice(line)?;
    let id = mbid::parse(&g.id)?;
    let count = ctx.child_counts.get(&id).copied();
    g.release_count = count;
    g.tidy(ctx.supplementary);
    let mut parents: Vec<Mbid> = g
        .artist_credit
        .iter()
        .filter_map(|c| mbid::parse(&c.artist.id).ok())
        .collect();
    parents.sort();
    parents.dedup();
    let keys = unique_keys(
        std::iter::once(g.title.as_str()).chain(g.aliases.iter().map(|x| x.name.as_str())),
    );
    let doc = Doc {
        id,
        name: g.title.clone(),
        detail: credit_text(&g.artist_credit),
        score: count.unwrap_or(0),
        keys,
    };
    let sort = date_key(g.first_release_date.as_deref());
    Ok(Record {
        id,
        blob: Blob::new(serde_json::to_vec(&g)?)?,
        doc,
        parents,
        sort,
    })
}

fn parse_release(line: &[u8], ctx: &Ctx) -> Result<Record> {
    let mut r: Release = serde_json::from_slice(line)?;
    let id = mbid::parse(&r.id)?;
    r.tidy(ctx.supplementary);
    let parents: Vec<Mbid> = r
        .release_group
        .iter()
        .filter_map(|g| mbid::parse(&g.id).ok())
        .collect();
    let doc = Doc {
        id,
        name: r.title.clone(),
        detail: credit_text(&r.artist_credit),
        score: 0,
        keys: unique_keys(std::iter::once(r.title.as_str())),
    };
    let sort = date_key(r.date.as_deref());
    Ok(Record {
        id,
        blob: Blob::new(serde_json::to_vec(&r)?)?,
        doc,
        parents,
        sort,
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
    /// (parent, sort key, child). Sorting these gives each parent's child list in order.
    links: Vec<(Mbid, u32, Mbid)>,
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
            let hint =
                "the file may be cut off (an unfinished download?); use --limit to stop earlier";
            let _ = tx.send(Err(e.context(format!(
                "reading the {name} dump {}: {hint}",
                reader_path.display()
            ))));
        }
    });

    let started = Instant::now();
    let (mut docs, mut links, mut bad) = (Vec::new(), Vec::new(), 0u64);
    for batch in rx {
        let batch = batch?;
        let parsed: Vec<Result<Record>> = batch.par_iter().map(|l| parse(kind, l, ctx)).collect();
        let mut items = Vec::with_capacity(parsed.len());
        for r in parsed {
            match r {
                Ok(rec) => {
                    links.extend(rec.parents.iter().map(|p| (*p, rec.sort, rec.id)));
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
        links,
    })
}

/// Turn (parent, sort, child) links into stored child lists and remember how many each parent has.
fn store_children(
    rel: Rel,
    mut links: Vec<(Mbid, u32, Mbid)>,
    writer: &mut dyn StoreWriter,
    counts: &mut HashMap<Mbid, u32>,
) -> Result<()> {
    links.sort_unstable();
    links.dedup();
    let mut batch: Vec<(Mbid, Vec<u8>)> = Vec::new();
    for group in links.chunk_by(|a, b| a.0 == b.0) {
        counts.insert(group[0].0, group.len() as u32);
        batch.push((group[0].0, group.iter().flat_map(|l| l.2).collect()));
        if batch.len() >= 10_000 {
            writer.put_children(rel, &batch)?;
            batch.clear();
        }
    }
    writer.put_children(rel, &batch)
}

/// Order matters: a release group is counted by its releases and an artist by its release groups,
/// so releases go first, then release groups, then artists.
const ORDER: [Kind; 3] = [Kind::Release, Kind::ReleaseGroup, Kind::Artist];

pub fn run(opts: &Options) -> Result<PathBuf> {
    fs::create_dir_all(&opts.out)?;
    // Versions are named by the second; if one already exists (two quick runs), wait for the next.
    let (now, version) = loop {
        let now = chrono::Utc::now();
        let version = format!("v{}", now.format("%Y-%m-%dT%H%M%S"));
        if !opts.out.join(&version).exists() {
            break (now, version);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    };
    let build = opts.out.join(format!(".building-{version}"));
    let _ = fs::remove_dir_all(&build);
    // A failed build must not leave a half-written directory behind.
    if let Err(e) = build_into(opts, &build, &version, now) {
        let _ = fs::remove_dir_all(&build);
        return Err(e);
    }

    let live = opts.out.join(&version);
    fs::rename(&build, &live)?;
    datadir::flip_current(&opts.out, &version)?;
    for old in datadir::prune(&opts.out, 2)? {
        eprintln!("pruned old version {old}");
    }
    eprintln!("ingest done: {version}");
    Ok(live)
}

/// Read every dump, write the store and indexes, and finish with meta.json. `build` is a new directory.
fn build_into(
    opts: &Options,
    build: &Path,
    version: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    fs::create_dir_all(build.join("fst"))?;

    let mut writer: Box<dyn StoreWriter> = Box::new(LmdbWriter::create(&build.join("store"))?);
    let mut ctx = Ctx {
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
        text::build(&build.join("tantivy"), kind.table(), &res.docs)
            .with_context(|| format!("building {} text index", kind.api_name()))?;
        if let Some(rel) = parent_rel(kind) {
            store_children(rel, res.links, writer.as_mut(), &mut ctx.child_counts)?;
        }
        counts.insert(kind.api_name().to_string(), res.count);
    }
    if counts.is_empty() {
        bail!("no dumps found in {}", opts.input.display());
    }
    writer.finish()?;

    let meta = Meta {
        schema_version: SCHEMA_VERSION,
        data_version: version.to_string(),
        dump_date: fs::read_to_string(opts.input.join("DUMP_DATE"))
            .ok()
            .map(|s| s.trim().to_string()),
        created_at: now.to_rfc3339(),
        supplementary: opts.include_supplementary,
        counts,
        files: datadir::describe_files(build)?,
    };
    fs::write(build.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;

    Ok(())
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
        let blob = data
            .store
            .get(Kind::Artist, &id, Encoding::Identity)
            .unwrap()
            .unwrap();
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

    #[test]
    fn date_keys_sort_by_date_with_missing_last() {
        let mut keys = [
            date_key(Some("1994-09-27")),
            date_key(None),
            date_key(Some("1994")),
            date_key(Some("1993-12")),
        ];
        keys.sort();
        assert_eq!(keys, [19930000 + 1200, 19940000, 19940927, u32::MAX]);
        assert_eq!(date_key(Some("")), u32::MAX);
    }

    #[test]
    fn fixture_has_all_kinds_and_linked_lists() {
        let out = tempfile::tempdir().unwrap();
        run(&fixture_opts(out.path())).unwrap();
        let data = Data::open(&datadir::resolve(out.path()).unwrap()).unwrap();
        for (name, file) in [
            ("artist", "artist"),
            ("release-group", "release-group"),
            ("release", "release"),
        ] {
            let lines = fs::read_to_string(format!("tests/fixtures/dump/mbdump/{file}"))
                .unwrap()
                .lines()
                .count();
            assert_eq!(data.meta.counts[name], lines as u64, "{name}");
        }
        // Every release must be reachable from its release group, in sorted order.
        let first: serde_json::Value = serde_json::from_str(
            fs::read_to_string("tests/fixtures/dump/mbdump/release")
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let group = mbid::parse(first["release-group"]["id"].as_str().unwrap()).unwrap();
        let (total, ids) = data
            .store
            .children(Rel::ReleaseGroupReleases, &group, 0, 100)
            .unwrap();
        assert!(total >= 1 && ids.len() == total);
        assert!(ids.contains(&mbid::parse(first["id"].as_str().unwrap()).unwrap()));
    }
}
