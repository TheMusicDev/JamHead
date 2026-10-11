//! Entity storage. The API and ingest only talk to the two traits below, so LMDB can be
//! replaced later (for example by a custom mmap'd file) without touching them.

use crate::mbid::Mbid;
use anyhow::{Context, Result};
use heed::types::Bytes;
use heed::{Database, Env, EnvFlags, EnvOpenOptions};
use std::collections::HashMap;
use std::path::Path;

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
    /// The exact JSON blob for one entity. One memory-mapped read plus a copy of the bytes.
    fn get(&self, kind: Kind, id: &Mbid) -> Result<Option<Vec<u8>>>;
    fn contains(&self, kind: Kind, id: &Mbid) -> Result<bool>;
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
    fn put_batch(&mut self, kind: Kind, items: &[(Mbid, Vec<u8>)]) -> Result<()>;
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
    Kind::ALL
        .iter()
        .map(|k| k.table())
        .chain(Rel::ALL.iter().map(|r| r.table()))
        .collect()
}

impl EntityStore for LmdbStore {
    fn get(&self, kind: Kind, id: &Mbid) -> Result<Option<Vec<u8>>> {
        let Some(db) = self.dbs.get(kind.table()) else {
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
        let page = packed.chunks_exact(16).skip(offset).take(limit);
        Ok((total, page.map(|c| c.try_into().unwrap()).collect()))
    }
}

pub struct LmdbWriter {
    env: Env,
    dbs: HashMap<&'static str, Database<Bytes, Bytes>>,
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
        Ok(Self { env, dbs })
    }
}

impl StoreWriter for LmdbWriter {
    fn put_batch(&mut self, kind: Kind, items: &[(Mbid, Vec<u8>)]) -> Result<()> {
        let db = self.dbs[kind.table()];
        let mut wtxn = self.env.write_txn()?;
        for (id, blob) in items {
            db.put(&mut wtxn, id, blob)?;
        }
        wtxn.commit()?;
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

    fn finish(self: Box<Self>) -> Result<()> {
        self.env.force_sync()?;
        Ok(())
    }
}
