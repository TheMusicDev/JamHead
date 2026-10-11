//! Ranked full-text search with Tantivy. One small index per entity kind.
//!
//! What is searched: `title` (the names, aliases and sort name; weighted 3x) and `extra` (the
//! disambiguation for artists, the credited artist for releases and release groups). Words are
//! lowercased and accent-folded, like the FST name search. Results are ranked by BM25 and then
//! lifted a little for entities with many children (a popular artist beats a namesake).

use crate::mbid::Mbid;
use crate::norm::normalize;
use crate::search::{Doc, Hit};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::fs;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::columnar::ColumnValues;
use tantivy::merge_policy::NoMergePolicy;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, FAST, STORED,
};
use tantivy::tokenizer::{
    AsciiFoldingFilter, LowerCaser, RemoveLongFilter, SimpleTokenizer, TextAnalyzer,
};
use tantivy::{DocId, Index, IndexReader, ReloadPolicy, Score, SegmentReader, TantivyDocument};

const TOKENIZER: &str = "jh";
// ponytail: 512 MB of indexing memory is plenty for one kind on a laptop; the full release dump
// may want more (or fewer, smaller segments) if indexing turns out slow.
const WRITER_MEMORY: usize = 512 << 20;

struct Fields {
    id: Field,
    name: Field,
    detail: Field,
    title: Field,
    extra: Field,
}

fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let indexed = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TOKENIZER)
            .set_index_option(IndexRecordOption::WithFreqs),
    );
    let f = Fields {
        id: b.add_bytes_field("id", STORED),
        name: b.add_text_field("name", STORED),
        detail: b.add_text_field("detail", STORED),
        title: b.add_text_field("title", indexed.clone()),
        extra: b.add_text_field("extra", indexed),
    };
    b.add_u64_field("rank", FAST);
    (b.build(), f)
}

fn register_tokenizer(index: &Index) {
    let analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(40))
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build();
    index.tokenizers().register(TOKENIZER, analyzer);
}

pub fn build(dir: &Path, table: &str, docs: &[Doc]) -> Result<()> {
    let path = dir.join(table);
    fs::create_dir_all(&path)?;
    let (schema, f) = schema();
    let rank = schema.get_field("rank")?;
    let index = Index::create_in_dir(&path, schema)?;
    register_tokenizer(&index);
    let writer = index.writer(WRITER_MEMORY)?;
    // No background merging while adding; everything is merged once at the end.
    writer.set_merge_policy(Box::new(NoMergePolicy));
    docs.par_iter().try_for_each(|d| -> Result<()> {
        let mut t = TantivyDocument::default();
        t.add_bytes(f.id, &d.id);
        t.add_text(f.name, &d.name);
        t.add_text(f.detail, &d.detail);
        for k in &d.keys {
            t.add_text(f.title, k);
        }
        t.add_text(f.extra, &d.detail);
        t.add_u64(rank, d.score as u64);
        writer.add_document(t)?;
        Ok(())
    })?;
    let mut writer = writer;
    writer.commit()?;
    // One segment is the fastest to search. The index is never written again.
    let segments = index.searchable_segment_ids()?;
    if segments.len() > 1 {
        writer.merge(&segments).wait()?;
    }
    writer.wait_merging_threads()?;
    Ok(())
}

pub struct TextIndex {
    reader: IndexReader,
    all_words: QueryParser,
    any_word: QueryParser,
    f: Fields,
}

impl TextIndex {
    pub fn open(dir: &Path, table: &str) -> Result<Option<Self>> {
        let path = dir.join(table);
        if !path.exists() {
            return Ok(None);
        }
        let index =
            Index::open_in_dir(&path).with_context(|| format!("opening {}", path.display()))?;
        register_tokenizer(&index);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        let (_, f) = schema();
        let parser = |all: bool| {
            let mut p = QueryParser::for_index(&index, vec![f.title, f.extra]);
            p.set_field_boost(f.title, 3.0);
            if all {
                p.set_conjunction_by_default();
            }
            p
        };
        Ok(Some(Self {
            reader,
            all_words: parser(true),
            any_word: parser(false),
            f,
        }))
    }

    /// Entities matching every word of `q`; if there are none, entities matching any word.
    pub fn search(&self, q: &str, limit: usize) -> Result<Vec<Hit>> {
        // Normalizing first leaves only plain words, so users cannot trip over query syntax.
        let q = normalize(q);
        if q.is_empty() {
            return Ok(vec![]);
        }
        let hits = self.run(&self.all_words, &q, limit)?;
        if hits.is_empty() {
            return self.run(&self.any_word, &q, limit);
        }
        Ok(hits)
    }

    fn run(&self, parser: &QueryParser, q: &str, limit: usize) -> Result<Vec<Hit>> {
        let query = parser.parse_query(q)?;
        let searcher = self.reader.searcher();
        // Lift popular entities: score * (1 + 0.2 * ln(1 + children)).
        let collector = TopDocs::with_limit(limit).tweak_score(move |segment: &SegmentReader| {
            let rank = segment
                .fast_fields()
                .u64("rank")
                .unwrap()
                .first_or_default_col(0);
            move |doc: DocId, score: Score| score * (1.0 + 0.2 * (rank.get_val(doc) as f32).ln_1p())
        });
        let mut hits = Vec::new();
        for (_, addr) in searcher.search(&query, &collector)? {
            let d: TantivyDocument = searcher.doc(addr)?;
            let text = |f: Field| {
                d.get_first(f)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let id: Mbid = d
                .get_first(self.f.id)
                .and_then(|v| v.as_bytes())
                .and_then(|b| b.try_into().ok())
                .unwrap_or([0; 16]);
            hits.push(Hit {
                id,
                name: text(self.f.name),
                detail: text(self.f.detail),
            });
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(n: u8, name: &str, detail: &str, score: u32) -> Doc {
        Doc {
            id: [n; 16],
            name: name.into(),
            detail: detail.into(),
            score,
            keys: vec![normalize(name)],
        }
    }

    fn index() -> (tempfile::TempDir, TextIndex) {
        let dir = tempfile::tempdir().unwrap();
        let docs = vec![
            doc(1, "OK Computer", "Radiohead", 20),
            doc(2, "Computer World", "Kraftwerk", 5),
            doc(3, "Kid A", "Radiohead", 20),
            doc(4, "Computer Love", "Some Cover Band", 0),
            doc(5, "Björk Live", "Björk", 1),
        ];
        build(dir.path(), "release_group", &docs).unwrap();
        let idx = TextIndex::open(dir.path(), "release_group")
            .unwrap()
            .unwrap();
        (dir, idx)
    }

    #[test]
    fn all_words_must_match_and_artist_text_counts() {
        let (_d, idx) = index();
        let ids = |q: &str| {
            idx.search(q, 10)
                .unwrap()
                .iter()
                .map(|h| h.id[0])
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("radiohead computer"), vec![1]);
        assert_eq!(ids("kraftwerk"), vec![2]);
        assert_eq!(ids("OK COMPUTER!"), vec![1]);
    }

    #[test]
    fn falls_back_to_any_word_and_folds_accents() {
        let (_d, idx) = index();
        let ids: Vec<u8> = idx
            .search("kid computer", 10)
            .unwrap()
            .iter()
            .map(|h| h.id[0])
            .collect();
        assert!(ids.contains(&3) && ids.contains(&1)); // no title has both words, so any word counts
        assert_eq!(idx.search("bjork", 10).unwrap()[0].id[0], 5);
    }

    #[test]
    fn popular_entities_rank_first_and_hits_carry_display_fields() {
        let (_d, idx) = index();
        let hits = idx.search("computer", 10).unwrap();
        assert_eq!(hits[0].id[0], 1);
        assert_eq!(
            (hits[0].name.as_str(), hits[0].detail.as_str()),
            ("OK Computer", "Radiohead")
        );
        assert!(idx.search("   ", 10).unwrap().is_empty());
        assert!(idx.search("zzzzqqq", 10).unwrap().is_empty());
    }
}
