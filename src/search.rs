//! Name search over an FST (finite state transducer): prefix and typo-tolerant matching.
//!
//! Per entity kind two files are written:
//!   `<kind>.fst`  maps a normalized name to a value `(score << 40) | offset`
//!   `<kind>.ids`  postings. At `offset`: u32 count, then per entry: 16-byte id,
//!                 u16 length + name, u16 length + detail.
//! Several entities can share a name, hence the postings file.

use crate::mbid::Mbid;
use crate::norm::normalize;
use anyhow::{Context, Result};
use fst::automaton::{Automaton, Str};
use fst::{IntoStreamer, Map, MapBuilder, Streamer};
use levenshtein_automata::{Distance, LevenshteinAutomatonBuilder, DFA, SINK_STATE};
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::OnceLock;

const OFFSET_BITS: u32 = 40;
const SCORE_MAX: u64 = (1 << 24) - 1;
// ponytail: look at most this many matching keys per query. Very short prefixes ("a") only see the
// alphabetically first keys. Raise it, or precompute top-N per short prefix, if that ever matters.
const SCAN_CAP: usize = 20_000;

/// What the API shows for a search hit.
pub struct Hit {
    pub id: Mbid,
    pub name: String,
    pub detail: String,
}

/// One entity to index: its display strings and the normalized names that should find it.
pub struct Doc {
    pub id: Mbid,
    pub name: String,
    pub detail: String,
    pub score: u32,
    pub keys: Vec<String>,
}

fn clip(s: &str) -> &str {
    match s.char_indices().nth(200) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn build(dir: &Path, table: &str, docs: &[Doc]) -> Result<()> {
    // (key, score, doc index), sorted so equal keys are adjacent and the best score comes first.
    let mut entries: Vec<(&str, u32, u32)> = Vec::new();
    for (i, d) in docs.iter().enumerate() {
        for k in &d.keys {
            if !k.is_empty() {
                entries.push((k, d.score, i as u32));
            }
        }
    }
    entries.sort_unstable_by(|a, b| a.0.cmp(b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    entries.dedup_by(|b, a| a.0 == b.0 && a.2 == b.2);

    let mut ids = BufWriter::new(File::create(dir.join(format!("{table}.ids")))?);
    let mut fst = MapBuilder::new(BufWriter::new(File::create(
        dir.join(format!("{table}.fst")),
    )?))?;
    let mut offset: u64 = 0;
    let mut i = 0;
    while i < entries.len() {
        let key = entries[i].0;
        let mut j = i;
        while j < entries.len() && entries[j].0 == key {
            j += 1;
        }
        let group = &entries[i..j];
        let value = ((group[0].1 as u64).min(SCORE_MAX) << OFFSET_BITS) | offset;
        fst.insert(key, value)?;
        let mut buf = Vec::new();
        buf.extend_from_slice(&(group.len() as u32).to_le_bytes());
        for &(_, _, di) in group {
            let d = &docs[di as usize];
            buf.extend_from_slice(&d.id);
            for s in [clip(&d.name), clip(&d.detail)] {
                buf.extend_from_slice(&(s.len() as u16).to_le_bytes());
                buf.extend_from_slice(s.as_bytes());
            }
        }
        ids.write_all(&buf)?;
        offset += buf.len() as u64;
        i = j;
    }
    ids.flush()?;
    fst.into_inner()?.flush()?;
    Ok(())
}

pub struct NameIndex {
    fst: Map<Mmap>,
    ids: Mmap,
}

impl NameIndex {
    pub fn open(dir: &Path, table: &str) -> Result<Option<Self>> {
        let fst_path = dir.join(format!("{table}.fst"));
        if !fst_path.exists() {
            return Ok(None);
        }
        // SAFETY: the files are never modified after ingest finishes.
        let fst = Map::new(unsafe { Mmap::map(&File::open(&fst_path)?)? })
            .with_context(|| format!("reading {}", fst_path.display()))?;
        let ids = unsafe { Mmap::map(&File::open(dir.join(format!("{table}.ids")))?)? };
        Ok(Some(Self { fst, ids }))
    }

    /// Names that start with `q`, best-scoring first.
    pub fn prefix(&self, q: &str, limit: usize) -> Vec<Hit> {
        let nq = normalize(q);
        if nq.is_empty() {
            return vec![];
        }
        let mut s = self.fst.search(Str::new(&nq).starts_with()).into_stream();
        let mut found = Vec::new();
        while let Some((key, val)) = s.next() {
            found.push((
                val >> OFFSET_BITS,
                key.len(),
                val & ((1 << OFFSET_BITS) - 1),
            ));
            if found.len() >= SCAN_CAP {
                break;
            }
        }
        // Higher score first, then shorter (closer to what was typed).
        found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.expand(found.into_iter().map(|f| f.2), limit)
    }

    /// Every entity indexed under exactly this (already normalized) name. Used by `verify`.
    pub fn lookup_exact(&self, normalized: &str) -> Vec<Mbid> {
        self.fst
            .get(normalized)
            .map(|val| {
                self.expand(std::iter::once(val & ((1 << OFFSET_BITS) - 1)), usize::MAX)
                    .into_iter()
                    .map(|h| h.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Names within a small edit distance of `q` (a swapped pair of letters counts as one edit),
    /// closest first.
    pub fn fuzzy(&self, q: &str, limit: usize) -> Vec<Hit> {
        let nq = normalize(q);
        let builder = match nq.chars().count() {
            0 => return vec![],
            1..=2 => None,
            3..=5 => Some(&builders()[0]),
            _ => Some(&builders()[1]),
        };
        let mut found = Vec::new();
        let mut collect = |dist: u8, val: u64| {
            found.push((dist, val >> OFFSET_BITS, val & ((1 << OFFSET_BITS) - 1)));
            found.len() < SCAN_CAP
        };
        match builder {
            // Too short to guess typos: exact name only.
            None => {
                if let Some(val) = self.fst.get(&nq) {
                    collect(0, val);
                }
            }
            Some(b) => {
                let dfa = b.build_dfa(&nq);
                let matcher = Dfa(&dfa);
                let mut s = self.fst.search(&matcher).into_stream();
                while let Some((key, val)) = s.next() {
                    let dist = match dfa.eval(key) {
                        Distance::Exact(d) => d,
                        Distance::AtLeast(_) => continue,
                    };
                    if !collect(dist, val) {
                        break;
                    }
                }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        self.expand(found.into_iter().map(|f| f.2), limit)
    }

    /// `n` bytes of the postings file at `p`, or None if the file is damaged or too short.
    fn bytes(&self, p: usize, n: usize) -> Option<&[u8]> {
        self.ids.get(p..p.checked_add(n)?)
    }

    /// Read postings in order until `limit` distinct entities are collected.
    /// A damaged postings file ends the walk early instead of crashing the server.
    fn expand(&self, offsets: impl Iterator<Item = u64>, limit: usize) -> Vec<Hit> {
        let mut hits: Vec<Hit> = Vec::new();
        for off in offsets {
            if self.read_group(off as usize, limit, &mut hits).is_none() {
                break;
            }
            if hits.len() >= limit {
                break;
            }
        }
        hits
    }

    fn read_group(&self, mut p: usize, limit: usize, hits: &mut Vec<Hit>) -> Option<()> {
        let n = u32::from_le_bytes(self.bytes(p, 4)?.try_into().ok()?);
        p += 4;
        for _ in 0..n {
            let id: Mbid = self.bytes(p, 16)?.try_into().ok()?;
            p += 16;
            let mut parts = [String::new(), String::new()];
            for part in &mut parts {
                let len = u16::from_le_bytes(self.bytes(p, 2)?.try_into().ok()?) as usize;
                p += 2;
                *part = String::from_utf8_lossy(self.bytes(p, len)?).into_owned();
                p += len;
            }
            if !hits.iter().any(|h| h.id == id) {
                let [name, detail] = parts;
                hits.push(Hit { id, name, detail });
                if hits.len() >= limit {
                    return Some(());
                }
            }
        }
        Some(())
    }
}

/// Edit-distance automata for distance 1 and 2. Building one is the expensive part
/// (about 5 ms), so it is done once; building the matcher for a query is then cheap.
fn builders() -> &'static [LevenshteinAutomatonBuilder; 2] {
    static B: OnceLock<[LevenshteinAutomatonBuilder; 2]> = OnceLock::new();
    B.get_or_init(|| {
        [
            LevenshteinAutomatonBuilder::new(1, true),
            LevenshteinAutomatonBuilder::new(2, true),
        ]
    })
}

/// Lets the FST walk only the keys the edit-distance automaton accepts.
struct Dfa<'a>(&'a DFA);

impl Automaton for Dfa<'_> {
    type State = u32;
    fn start(&self) -> u32 {
        self.0.initial_state()
    }
    fn is_match(&self, state: &u32) -> bool {
        matches!(self.0.distance(*state), Distance::Exact(_))
    }
    fn can_match(&self, state: &u32) -> bool {
        *state != SINK_STATE
    }
    fn accept(&self, state: &u32, byte: u8) -> u32 {
        self.0.transition(*state, byte)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(n: u8, name: &str, score: u32, keys: &[&str]) -> Doc {
        Doc {
            id: [n; 16],
            name: name.into(),
            detail: format!("detail {n}"),
            score,
            keys: keys.iter().map(|k| normalize(k)).collect(),
        }
    }

    fn index() -> (tempfile::TempDir, NameIndex) {
        let dir = tempfile::tempdir().unwrap();
        let docs = vec![
            doc(1, "Radiohead", 50, &["Radiohead", "Radio Head"]),
            doc(2, "Radio Dept.", 5, &["Radio Dept."]),
            doc(3, "Björk", 30, &["Björk"]),
            doc(4, "Radiohead", 1, &["Radiohead"]), // same name, lower score
        ];
        build(dir.path(), "artist", &docs).unwrap();
        let idx = NameIndex::open(dir.path(), "artist").unwrap().unwrap();
        (dir, idx)
    }

    #[test]
    fn prefix_ranks_by_score_and_dedupes() {
        let (_d, idx) = index();
        let hits = idx.prefix("radio", 10);
        let ids: Vec<u8> = hits.iter().map(|h| h.id[0]).collect();
        // Ranked per matching name: both "Radiohead" entries (best score 50) come before "Radio Dept." (5).
        assert_eq!(ids, vec![1, 4, 2]);
        assert_eq!(hits[0].detail, "detail 1");
    }

    #[test]
    fn prefix_ignores_accents_and_limits() {
        let (_d, idx) = index();
        assert_eq!(idx.prefix("BJO", 10)[0].name, "Björk");
        assert_eq!(idx.prefix("radio", 1).len(), 1);
        assert!(idx.prefix("zzz", 10).is_empty());
    }

    #[test]
    fn fuzzy_tolerates_typos() {
        let (_d, idx) = index();
        assert_eq!(idx.fuzzy("radiohed", 10)[0].id[0], 1);
        assert_eq!(idx.fuzzy("bjork", 10)[0].id[0], 3);
        assert!(idx.fuzzy("xxxxxxxx", 10).is_empty());
    }
}
