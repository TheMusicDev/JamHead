//! A data directory is `<root>/current -> <root>/v<timestamp>/`. Each version holds
//! `meta.json`, `store/` (LMDB) and `fst/`. A version is built once and never modified;
//! going live means flipping the `current` symlink.

use crate::search::NameIndex;
use crate::store::{EntityStore, Kind, LmdbStore};
use crate::text::TextIndex;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone)]
pub struct Meta {
    pub schema_version: u32,
    pub data_version: String,
    pub dump_date: Option<String>,
    pub created_at: String,
    pub supplementary: bool,
    /// Entities per kind, keyed by API name ("artist", "release-group", ...).
    pub counts: BTreeMap<String, u64>,
    /// Size (and checksum) of every file in the version directory except meta.json, for `verify`.
    pub files: BTreeMap<String, FileInfo>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FileInfo {
    pub size: u64,
    /// Left out for the LMDB data file, which can be hundreds of GB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Everything the server needs from one opened data version.
pub struct Data {
    pub dir: PathBuf,
    pub meta: Meta,
    pub store: Box<dyn EntityStore>,
    pub names: HashMap<Kind, NameIndex>,
    pub text: HashMap<Kind, TextIndex>,
}

impl Data {
    pub fn open(dir: &Path) -> Result<Data> {
        let meta: Meta = serde_json::from_slice(
            &fs::read(dir.join("meta.json"))
                .with_context(|| format!("no meta.json in {}", dir.display()))?,
        )?;
        if meta.schema_version != SCHEMA_VERSION {
            bail!(
                "{} has schema version {} but this binary reads {}. Re-run ingest.",
                dir.display(),
                meta.schema_version,
                SCHEMA_VERSION
            );
        }
        let store = Box::new(LmdbStore::open(&dir.join("store"))?);
        let (mut names, mut text) = (HashMap::new(), HashMap::new());
        for kind in Kind::ALL {
            if let Some(idx) = NameIndex::open(&dir.join("fst"), kind.table())? {
                names.insert(kind, idx);
            }
            if let Some(idx) = TextIndex::open(&dir.join("tantivy"), kind.table())? {
                text.insert(kind, idx);
            }
        }
        Ok(Data {
            dir: dir.to_path_buf(),
            meta,
            store,
            names,
            text,
        })
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Record every file of a finished version so `verify` can detect damage later.
pub fn describe_files(dir: &Path) -> Result<BTreeMap<String, FileInfo>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, FileInfo>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
                continue;
            }
            let rel = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            // Lock files are bookkeeping that readers may recreate.
            if rel == "meta.json" || rel.ends_with(".lock") || rel.ends_with("lock.mdb") {
                continue;
            }
            let sha256 = if rel == "store/data.mdb" {
                None
            } else {
                Some(sha256_file(&path)?)
            };
            out.insert(
                rel,
                FileInfo {
                    size: fs::metadata(&path)?.len(),
                    sha256,
                },
            );
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// `--data` may be the root (with a `current` link) or a version directory itself.
pub fn resolve(root: &Path) -> Result<PathBuf> {
    if root.join("meta.json").exists() {
        return Ok(root.to_path_buf());
    }
    let current = root.join("current");
    fs::canonicalize(&current).with_context(|| {
        format!(
            "no data at {} (run `jamhead ingest` first)",
            current.display()
        )
    })
}

/// Point `current` at `version`. Creates a temp link and renames it over `current`,
/// which is atomic: readers see either the old or the new target, never nothing.
pub fn flip_current(root: &Path, version: &str) -> Result<()> {
    let tmp = root.join(".current.tmp");
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(version, &tmp)?;
    fs::rename(&tmp, root.join("current"))?;
    Ok(())
}

/// Keep the newest `keep` versions (and always the live one). Only touches directories
/// that look like versions, so stray files in `--out` are safe.
pub fn prune(root: &Path, keep: usize) -> Result<Vec<String>> {
    let live = fs::canonicalize(root.join("current")).ok();
    let mut versions: Vec<String> = fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("meta.json").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with('v'))
        .collect();
    versions.sort();
    versions.reverse();
    let mut removed = vec![];
    for v in versions.into_iter().skip(keep) {
        let path = root.join(&v);
        if live.as_deref() == fs::canonicalize(&path).ok().as_deref() {
            continue;
        }
        fs::remove_dir_all(&path)?;
        removed.push(v);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_version(root: &Path, name: &str) {
        fs::create_dir_all(root.join(name)).unwrap();
        fs::write(root.join(name).join("meta.json"), "{}").unwrap();
    }

    #[test]
    fn flip_switches_current_and_prune_keeps_live_and_newest() {
        let root = tempfile::tempdir().unwrap();
        for v in [
            "v2026-01-01T000001",
            "v2026-01-01T000002",
            "v2026-01-01T000003",
        ] {
            make_version(root.path(), v);
        }
        flip_current(root.path(), "v2026-01-01T000001").unwrap();
        // The oldest is live, so it must survive pruning even though it is not among the newest 1.
        let removed = prune(root.path(), 1).unwrap();
        assert_eq!(removed, vec!["v2026-01-01T000002"]);
        assert!(root.path().join("v2026-01-01T000001").exists());
        assert!(root.path().join("v2026-01-01T000003").exists());
        flip_current(root.path(), "v2026-01-01T000003").unwrap();
        assert_eq!(
            resolve(root.path()).unwrap(),
            fs::canonicalize(root.path().join("v2026-01-01T000003")).unwrap()
        );
    }
}
