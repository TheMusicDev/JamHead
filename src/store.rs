//! Entity storage. The API and ingest only talk to the two traits below, so LMDB can be
//! replaced later (for example by a custom mmap'd file) without touching them.

use crate::compress::{Blob, Encoding};
use crate::mbid::Mbid;
use anyhow::{Context, Result};
use heed::types::Bytes;
use heed::{CompactionOption, Database, Env, EnvFlags, EnvOpenOptions};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Artist,
    ReleaseGroup,
    Release,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Artist, Kind::ReleaseGroup, Kind::Release];

    /// Name used in URLs and in the MusicBrainz dump file names ("release-group").
    pub fn api_name(self) -> &'static str {
        match self {
            Kind::Artist => "artist",
            Kind::ReleaseGroup => "release-group",
            Kind::Release => "release",
        }
    }

    /// Name used for tables and files on disk ("release_group").
    pub fn table(self) -> &'static str {
        match self {
            Kind::Artist => "artist",
            Kind::ReleaseGroup => "release_group",
            Kind::Release => "release",
        }
    }

    /// Table holding this kind's blobs in the given encoding.
    pub fn blob_table(self, enc: Encoding) -> &'static str {
        match (self, enc) {
            (Kind::Artist, Encoding::Identity) => "artist",
            (Kind::Artist, Encoding::Zstd) => "artist_zstd",
            (Kind::Artist, Encoding::Gzip) => "artist_gzip",
            (Kind::ReleaseGroup, Encoding::Identity) => "release_group",
            (Kind::ReleaseGroup, Encoding::Zstd) => "release_group_zstd",
            (Kind::ReleaseGroup, Encoding::Gzip) => "release_group_gzip",
            (Kind::Release, Encoding::Identity) => "release",
            (Kind::Release, Encoding::Zstd) => "release_zstd",
            (Kind::Release, Encoding::Gzip) => "release_gzip",
        }
    }

    pub fn from_api_name(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.api_name() == s)
    }
}

/// A parent -> children list that is precomputed at ingest, sorted by date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rel {
    ArtistReleaseGroups,
    ReleaseGroupReleases,
}

impl Rel {
    pub const ALL: [Rel; 2] = [Rel::ArtistReleaseGroups, Rel::ReleaseGroupReleases];

    pub fn table(self) -> &'static str {
        match self {
            Rel::ArtistReleaseGroups => "artist_rgs",
            Rel::ReleaseGroupReleases => "rg_releases",
        }
    }
}

/// Read side. Used by the HTTP server.
pub trait EntityStore: Send + Sync {
    /// The exact JSON blob for one entity, in the requested encoding. One memory-mapped read plus a
    /// copy of the bytes.
    fn get(&self, kind: Kind, id: &Mbid, enc: Encoding) -> Result<Option<Vec<u8>>>;
    fn contains(&self, kind: Kind, id: &Mbid) -> Result<bool>;
    fn count(&self, kind: Kind) -> Result<u64>;
    /// (table name, entries, bytes on disk) for every table, for `stats`.
    fn table_sizes(&self) -> Result<Vec<(String, u64, u64)>>;
    /// Up to `n` entities in key order. Keys are random UUIDs, so this is a fair sample.
    fn sample(&self, kind: Kind, n: usize) -> Result<Vec<(Mbid, Vec<u8>)>>;
    /// One page of a parent's children: (total children, ids in the page).
    fn children(
        &self,
        rel: Rel,
        parent: &Mbid,
        offset: usize,
        limit: usize,
    ) -> Result<(usize, Vec<Mbid>)>;
}

/// Write side. Used by ingest. A store is built once and never modified afterwards.
pub trait StoreWriter {
    fn put_batch(&mut self, kind: Kind, items: &[(Mbid, Blob)]) -> Result<()>;
    /// `items` are (parent, packed 16-byte child ids in the order they should be listed).
    fn put_children(&mut self, rel: Rel, items: &[(Mbid, Vec<u8>)]) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<()>;
}

// ponytail: 512 GiB of address space, not disk. LMDB only uses what is written.
// Raise it if the full release dump ever outgrows this.
const MAP_SIZE: usize = 512 << 30;
const MAX_DBS: u32 = 16;

pub struct LmdbStore {
    env: Env,
    dbs: HashMap<&'static str, Database<Bytes, Bytes>>,
}

impl LmdbStore {
    /// Open an existing store read-only.
    pub fn open(dir: &Path) -> Result<Self> {
        let mut opts = EnvOpenOptions::new();
        opts.max_dbs(MAX_DBS);
        // Read-only. (Not NO_LOCK: that allows only one reader at a time. LMDB keeps a small
        // lock file `lock.mdb` next to the data, so the data volume must be writable.)
        // SAFETY: the files are never modified after ingest finishes.
        let env = unsafe {
            opts.flags(EnvFlags::READ_ONLY);
            opts.open(dir)
        }
        .with_context(|| format!("opening LMDB store {}", dir.display()))?;
        let rtxn = env.read_txn()?;
        let mut dbs = HashMap::new();
        for name in table_names() {
            if let Some(db) = env.open_database::<Bytes, Bytes>(&rtxn, Some(name))? {
                dbs.insert(name, db);
            }
        }
        // Must commit (not drop): LMDB discards table handles opened in an aborted transaction.
        rtxn.commit()?;
        Ok(Self { env, dbs })
    }
}

fn table_names() -> Vec<&'static str> {
    let blobs = Kind::ALL
        .iter()
        .flat_map(|k| Encoding::ALL.map(|e| k.blob_table(e)));
    blobs.chain(Rel::ALL.iter().map(|r| r.table())).collect()
}

impl EntityStore for LmdbStore {
    fn get(&self, kind: Kind, id: &Mbid, enc: Encoding) -> Result<Option<Vec<u8>>> {
        let Some(db) = self.dbs.get(kind.blob_table(enc)) else {
            return Ok(None);
        };
        let rtxn = self.env.read_txn()?;
        Ok(db.get(&rtxn, id)?.map(|b| b.to_vec()))
    }

    fn contains(&self, kind: Kind, id: &Mbid) -> Result<bool> {
        let Some(db) = self.dbs.get(kind.table()) else {
            return Ok(false);
        };
        let rtxn = self.env.read_txn()?;
        Ok(db.get(&rtxn, id)?.is_some())
    }

    fn count(&self, kind: Kind) -> Result<u64> {
        let Some(db) = self.dbs.get(kind.table()) else {
            return Ok(0);
        };
        let rtxn = self.env.read_txn()?;
        Ok(db.len(&rtxn)?)
    }

    fn table_sizes(&self) -> Result<Vec<(String, u64, u64)>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for (name, db) in &self.dbs {
            let st = db.stat(&rtxn)?;
            let pages = (st.branch_pages + st.leaf_pages + st.overflow_pages) as u64;
            out.push((
                name.to_string(),
                st.entries as u64,
                pages * st.page_size as u64,
            ));
        }
        out.sort();
        Ok(out)
    }

    fn sample(&self, kind: Kind, n: usize) -> Result<Vec<(Mbid, Vec<u8>)>> {
        let Some(db) = self.dbs.get(kind.table()) else {
            return Ok(vec![]);
        };
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for entry in db.iter(&rtxn)?.take(n) {
            let (key, value) = entry?;
            out.push((key.try_into()?, value.to_vec()));
        }
        Ok(out)
    }

    fn children(
        &self,
        rel: Rel,
        parent: &Mbid,
        offset: usize,
        limit: usize,
    ) -> Result<(usize, Vec<Mbid>)> {
        let Some(db) = self.dbs.get(rel.table()) else {
            return Ok((0, vec![]));
        };
        let rtxn = self.env.read_txn()?;
        let packed = db.get(&rtxn, parent)?.unwrap_or(&[]);
        let total = packed.len() / 16;
        // The list is a run of 16-byte ids; as_chunks hands them out as [u8; 16].
        let (ids, _) = packed.as_chunks::<16>();
        Ok((
            total,
            ids.iter().skip(offset).take(limit).copied().collect(),
        ))
    }
}

/// Ingest does not write blobs into LMDB in dump order. The ids are random, so doing that puts every
/// insert at a random place in a file of tens of GB: fast while it fits in RAM, then a disk seek per
/// insert (on a NAS with spinning disks a full run slowed from 958 records/s to about 67/s and kept
/// falling). Instead `put_batch` appends each record to one of 256 spool files, chosen by the first
/// byte of its id, and `finish` writes the files into LMDB one at a time in id order. Ids are random,
/// so the files come out about the same size, each is small enough to sort in memory, and LMDB is
/// filled front to back.
const BUCKETS: usize = 256;
/// Each bucket collects records in memory and appends them to its file in chunks this big. The files
/// are opened and closed for each chunk: holding all of them open at once (256 per kind, three kinds)
/// hit the default open-files limit of 256 on macOS ("Too many open files").
const SPOOL_CHUNK: usize = 128 * 1024;

pub struct LmdbWriter {
    dir: PathBuf,
    env: Env,
    dbs: HashMap<&'static str, Database<Bytes, Bytes>>,
    spool: HashMap<Kind, Vec<Vec<u8>>>,
}

impl LmdbWriter {
    pub fn create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut opts = EnvOpenOptions::new();
        opts.max_dbs(MAX_DBS).map_size(MAP_SIZE);
        // The directory is brand new and not served yet, so a crash just means "rebuild".
        // SAFETY: we are the only process using this directory.
        let env = unsafe {
            opts.flags(EnvFlags::NO_SYNC | EnvFlags::NO_META_SYNC);
            opts.open(dir)
        }
        .with_context(|| format!("creating LMDB store {}", dir.display()))?;
        let mut wtxn = env.write_txn()?;
        let mut dbs = HashMap::new();
        for name in table_names() {
            dbs.insert(
                name,
                env.create_database::<Bytes, Bytes>(&mut wtxn, Some(name))?,
            );
        }
        wtxn.commit()?;
        Ok(Self {
            dir: dir.to_path_buf(),
            env,
            dbs,
            spool: HashMap::new(),
        })
    }

    fn spool_dir(&self, kind: Kind) -> PathBuf {
        self.dir.join("spool").join(kind.table())
    }

    /// Move one kind's spooled records into LMDB, bucket by bucket in id order.
    fn write_kind(&mut self, kind: Kind) -> Result<()> {
        let Some(mut buffers) = self.spool.remove(&kind) else {
            return Ok(());
        };
        let dir = self.spool_dir(kind);
        // Also creates the file of a bucket that never received a record.
        for (bucket, buf) in buffers.iter_mut().enumerate() {
            append_to_bucket(&dir, bucket, buf)?;
        }
        drop(buffers);
        let name = kind.api_name().replace('-', " ");
        crate::log!("Writing {name} records to the database, in order...");
        let started = Instant::now();
        for bucket in 0..BUCKETS {
            let path = dir.join(format!("{bucket:02x}"));
            let bytes = std::fs::read(&path)?;
            std::fs::remove_file(&path)?;
            let mut items = unspool(&bytes)?;
            // A stable sort: if an id ever appears twice, the later record is written last and wins.
            items.sort_by_key(|(id, _)| *id);
            // Compression happens here, in parallel, instead of while parsing.
            let blobs: Vec<(Mbid, Blob)> = items
                .par_iter()
                .map(|(id, plain)| Ok((*id, Blob::new(plain.to_vec())?)))
                .collect::<Result<_>>()?;
            let mut wtxn = self.env.write_txn()?;
            for enc in Encoding::ALL {
                let db = self.dbs[kind.blob_table(enc)];
                for (id, blob) in &blobs {
                    db.put(&mut wtxn, id, blob.get(enc))?;
                }
            }
            wtxn.commit()?;
            if (bucket + 1) % 32 == 0 {
                let done = (bucket + 1) as f64 / BUCKETS as f64;
                let left = crate::progress::time_left(done, started.elapsed().as_secs_f64())
                    .map(|t| format!(", about {t} left"))
                    .unwrap_or_default();
                crate::log!("  {name}: {:.0}% written{left}", done * 100.0);
            }
        }
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

/// Append what a bucket has collected to its file and empty the buffer.
fn append_to_bucket(dir: &Path, bucket: usize, buf: &mut Vec<u8>) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{bucket:02x}")))?;
    file.write_all(buf)?;
    buf.clear();
    Ok(())
}

/// Split a spool file into (id, plain JSON) records: 16 id bytes, a little-endian u32 length, the bytes.
fn unspool(mut bytes: &[u8]) -> Result<Vec<(Mbid, &[u8])>> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        anyhow::ensure!(bytes.len() >= 20, "spool file is cut off");
        let id: Mbid = bytes[..16].try_into()?;
        let len = u32::from_le_bytes(bytes[16..20].try_into()?) as usize;
        anyhow::ensure!(bytes.len() >= 20 + len, "spool file is cut off");
        out.push((id, &bytes[20..20 + len]));
        bytes = &bytes[20 + len..];
    }
    Ok(out)
}

impl StoreWriter for LmdbWriter {
    fn put_batch(&mut self, kind: Kind, items: &[(Mbid, Blob)]) -> Result<()> {
        if !self.spool.contains_key(&kind) {
            std::fs::create_dir_all(self.spool_dir(kind))?;
            self.spool.insert(kind, vec![Vec::new(); BUCKETS]);
        }
        let dir = self.spool_dir(kind);
        let buffers = self.spool.get_mut(&kind).expect("just inserted");
        for (id, blob) in items {
            let plain = blob.get(Encoding::Identity);
            let bucket = id[0] as usize;
            let buf = &mut buffers[bucket];
            buf.extend_from_slice(id);
            buf.extend_from_slice(&(plain.len() as u32).to_le_bytes());
            buf.extend_from_slice(plain);
            if buf.len() >= SPOOL_CHUNK {
                append_to_bucket(&dir, bucket, buf)?;
            }
        }
        Ok(())
    }

    fn put_children(&mut self, rel: Rel, items: &[(Mbid, Vec<u8>)]) -> Result<()> {
        let db = self.dbs[rel.table()];
        let mut wtxn = self.env.write_txn()?;
        for (parent, packed) in items {
            db.put(&mut wtxn, parent, packed)?;
        }
        wtxn.commit()?;
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        for kind in Kind::ALL {
            self.write_kind(kind)?;
        }
        let _ = std::fs::remove_dir(self.dir.join("spool")); // empty by now
        crate::log!("Compacting the database (one more copy, so it is smaller and laid out in order). This can be quiet for a while...");
        self.env.force_sync()?;
        // Writing in batches leaves unused pages behind. Copy to a compacted file and swap it in:
        // smaller on disk and laid out in order for the readers.
        let compact = self.dir.join("data.mdb.compact");
        self.env.copy_to_path(&compact, CompactionOption::Enabled)?;
        self.env.prepare_for_closing().wait();
        std::fs::rename(&compact, self.dir.join("data.mdb"))?;
        let _ = std::fs::remove_file(self.dir.join("lock.mdb"));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Not a correctness test: measures raw LMDB read speed with no HTTP in the way, to decide whether
    /// the store could ever be the bottleneck. Run with
    ///   JAMHEAD_BENCH_DATA=./data cargo test --release store_read_speed -- --ignored --nocapture
    #[test]
    #[ignore]
    fn store_read_speed() {
        let root = std::env::var("JAMHEAD_BENCH_DATA")
            .expect("set JAMHEAD_BENCH_DATA to a data directory");
        let dir = crate::datadir::resolve(Path::new(&root)).unwrap();
        let store = std::sync::Arc::new(LmdbStore::open(&dir.join("store")).unwrap());
        for kind in Kind::ALL {
            let ids: Vec<Mbid> = store
                .sample(kind, 100_000)
                .unwrap()
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            if ids.is_empty() {
                continue;
            }
            for threads in [1usize, 8] {
                let per_thread = 400_000 / threads;
                let started = Instant::now();
                let bytes: usize = std::thread::scope(|s| {
                    (0..threads)
                        .map(|t| {
                            let (store, ids) = (store.clone(), &ids);
                            s.spawn(move || {
                                let mut total = 0;
                                // A different stride per thread so they do not read the same keys in step.
                                for i in 0..per_thread {
                                    let id = &ids[(i * 7919 + t * 104_729) % ids.len()];
                                    total += store
                                        .get(kind, id, Encoding::Identity)
                                        .unwrap()
                                        .unwrap()
                                        .len();
                                }
                                total
                            })
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                        .map(|h| h.join().unwrap())
                        .sum()
                });
                let secs = started.elapsed().as_secs_f64();
                let n = per_thread * threads;
                println!(
                    "{:<14} {threads} thread(s): {:>9.0} gets/s, {:>6.0} ns per get per thread, avg blob {} B",
                    kind.api_name(),
                    n as f64 / secs,
                    secs * 1e9 * threads as f64 / n as f64,
                    bytes / n
                );
            }
        }
    }
}
