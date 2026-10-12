//! The JSON we keep for each entity (see docs/schema.md).
//!
//! One struct per entity is used both to read a dump line (unknown fields such as
//! `relations` are skipped) and to write the blob the API serves. Field names are the
//! MusicBrainz names. Empty strings, nulls and empty lists are dropped.

use serde::{Deserialize, Deserializer, Serialize};

/// Dumps use "" or null for "no value". We treat both as absent.
pub fn opt_str<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.filter(|s| !s.is_empty()))
}

/// null -> empty list.
pub fn list<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

fn not_true(b: &Option<bool>) -> bool {
    *b != Some(true)
}

#[derive(Deserialize, Serialize, Default)]
pub struct Area {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(
        rename = "sort-name",
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub sort_name: Option<String>,
    #[serde(
        rename = "iso-3166-1-codes",
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub iso_codes: Vec<String>,
}

#[derive(Deserialize, Serialize, Default)]
pub struct LifeSpan {
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub begin: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "not_true")]
    pub ended: Option<bool>,
}

#[derive(Deserialize, Serialize, Default)]
pub struct Alias {
    pub name: String,
    #[serde(
        rename = "sort-name",
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub sort_name: Option<String>,
    #[serde(
        default,
        rename = "type",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub kind: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "not_true")]
    pub primary: Option<bool>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub begin: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "not_true")]
    pub ended: Option<bool>,
}

// ---- Supplementary (CC BY-NC-SA) pieces. Only kept with --include-supplementary. ----

#[derive(Deserialize, Serialize)]
pub struct Tag {
    pub name: String,
    #[serde(default)]
    pub count: i64,
}

#[derive(Deserialize, Serialize)]
pub struct Genre {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub count: i64,
}

#[derive(Deserialize, Serialize)]
pub struct Rating {
    #[serde(rename = "votes-count", default)]
    pub votes: i64,
    #[serde(default)]
    pub value: Option<f64>,
}

/// Drop all supplementary fields unless the operator asked for them.
/// (Called from each entity's `tidy`. The four fields are repeated on every entity struct on purpose:
/// `#[serde(flatten)]` would make serde buffer the huge `relations` arrays and slow ingest down a lot.)
pub fn supplementary_policy(
    keep: bool,
    tags: &mut Vec<Tag>,
    genres: &mut Vec<Genre>,
    rating: &mut Option<Rating>,
    annotation: &mut Option<String>,
) {
    if !keep {
        tags.clear();
        genres.clear();
        *rating = None;
        *annotation = None;
    } else if rating
        .as_ref()
        .is_some_and(|r| r.votes == 0 && r.value.is_none())
    {
        *rating = None;
    }
}

// ---- Artist ----

#[derive(Deserialize, Serialize, Default)]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(
        rename = "sort-name",
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub sort_name: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub disambiguation: Option<String>,
    #[serde(
        default,
        rename = "type",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub kind: Option<String>,
    #[serde(
        default,
        rename = "type-id",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub kind_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub gender: Option<String>,
    #[serde(
        default,
        rename = "gender-id",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub gender_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<Area>,
    #[serde(
        default,
        rename = "begin-area",
        skip_serializing_if = "Option::is_none"
    )]
    pub begin_area: Option<Area>,
    #[serde(default, rename = "end-area", skip_serializing_if = "Option::is_none")]
    pub end_area: Option<Area>,
    #[serde(default, rename = "life-span", skip_serializing_if = "Option::is_none")]
    pub life_span: Option<LifeSpan>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub aliases: Vec<Alias>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub isnis: Vec<String>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub ipis: Vec<String>,
    /// Computed at ingest, not in the dump.
    #[serde(
        default,
        rename = "release-group-count",
        skip_serializing_if = "Option::is_none"
    )]
    pub release_group_count: Option<u32>,
    // Supplementary (CC BY-NC-SA), cleared by `tidy` unless --include-supplementary.
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tags: Vec<Tag>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub genres: Vec<Genre>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<Rating>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub annotation: Option<String>,
}

impl Artist {
    /// Remove empty containers so the blob only has real information.
    pub fn tidy(&mut self, keep_supplementary: bool) {
        supplementary_policy(
            keep_supplementary,
            &mut self.tags,
            &mut self.genres,
            &mut self.rating,
            &mut self.annotation,
        );
        if let Some(ls) = &self.life_span {
            if ls.begin.is_none() && ls.end.is_none() && ls.ended != Some(true) {
                self.life_span = None;
            }
        }
    }
}

// ---- Artist credits (shared by release groups, releases and tracks) ----

#[derive(Deserialize, Serialize)]
pub struct ArtistRef {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(
        rename = "sort-name",
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub sort_name: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub disambiguation: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct Credit {
    /// The name as credited on this release, which can differ from the artist's own name.
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub joinphrase: Option<String>,
    pub artist: ArtistRef,
}

/// "Artist A feat. Artist B" as one display string.
pub fn credit_text(credits: &[Credit]) -> String {
    credits
        .iter()
        .map(|c| {
            format!(
                "{}{}",
                c.name.as_deref().unwrap_or(&c.artist.name),
                c.joinphrase.as_deref().unwrap_or("")
            )
        })
        .collect()
}

fn same_artists(a: &[Credit], b: &[Credit]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.artist.id == y.artist.id)
}

// ---- Release group ----

#[derive(Deserialize, Serialize, Default)]
pub struct ReleaseGroup {
    pub id: String,
    pub title: String,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub disambiguation: Option<String>,
    #[serde(
        default,
        rename = "primary-type",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub primary_type: Option<String>,
    #[serde(
        default,
        rename = "primary-type-id",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub primary_type_id: Option<String>,
    #[serde(
        default,
        rename = "secondary-types",
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub secondary_types: Vec<String>,
    #[serde(
        default,
        rename = "secondary-type-ids",
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub secondary_type_ids: Vec<String>,
    #[serde(
        default,
        rename = "first-release-date",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub first_release_date: Option<String>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub aliases: Vec<Alias>,
    #[serde(
        default,
        rename = "artist-credit",
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub artist_credit: Vec<Credit>,
    /// Computed at ingest, not in the dump.
    #[serde(
        default,
        rename = "release-count",
        skip_serializing_if = "Option::is_none"
    )]
    pub release_count: Option<u32>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tags: Vec<Tag>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub genres: Vec<Genre>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<Rating>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub annotation: Option<String>,
}

impl ReleaseGroup {
    pub fn tidy(&mut self, keep_supplementary: bool) {
        supplementary_policy(
            keep_supplementary,
            &mut self.tags,
            &mut self.genres,
            &mut self.rating,
            &mut self.annotation,
        );
    }
}

// ---- Release ----

/// The slice of the release group that is useful on a release page.
#[derive(Deserialize, Serialize)]
pub struct ReleaseGroupRef {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(
        default,
        rename = "primary-type",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub primary_type: Option<String>,
    #[serde(
        default,
        rename = "secondary-types",
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub secondary_types: Vec<String>,
    #[serde(
        default,
        rename = "first-release-date",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub first_release_date: Option<String>,
}

#[derive(Deserialize, Serialize, Default)]
pub struct TextRepresentation {
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub language: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub script: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct ReleaseEvent {
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<Area>,
}

#[derive(Deserialize, Serialize)]
pub struct LabelRef {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Deserialize, Serialize)]
pub struct LabelInfo {
    #[serde(
        default,
        rename = "catalog-number",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub catalog_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<LabelRef>,
}

#[derive(Deserialize, Serialize)]
pub struct CoverArtArchive {
    #[serde(default)]
    pub artwork: bool,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub front: bool,
    #[serde(default)]
    pub back: bool,
    #[serde(default)]
    pub darkened: bool,
}

#[derive(Deserialize, Serialize)]
pub struct RecordingRef {
    pub id: String,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct Track {
    pub id: String,
    #[serde(default)]
    pub position: i64,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub number: Option<String>,
    #[serde(default)]
    pub title: String,
    /// Milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingRef>,
    /// Only kept when it differs from the release's artist credit.
    #[serde(
        default,
        rename = "artist-credit",
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub artist_credit: Vec<Credit>,
}

#[derive(Deserialize, Serialize)]
pub struct Medium {
    #[serde(default)]
    pub position: i64,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub format: Option<String>,
    #[serde(
        default,
        rename = "format-id",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub format_id: Option<String>,
    #[serde(default, rename = "track-count")]
    pub track_count: i64,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tracks: Vec<Track>,
}

#[derive(Deserialize, Serialize, Default)]
pub struct Release {
    pub id: String,
    pub title: String,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub disambiguation: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub status: Option<String>,
    #[serde(
        default,
        rename = "status-id",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub status_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub date: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub country: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub barcode: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub asin: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub packaging: Option<String>,
    #[serde(
        default,
        rename = "packaging-id",
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub packaging_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub quality: Option<String>,
    #[serde(
        default,
        rename = "text-representation",
        skip_serializing_if = "Option::is_none"
    )]
    pub text_representation: Option<TextRepresentation>,
    #[serde(
        default,
        rename = "artist-credit",
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub artist_credit: Vec<Credit>,
    #[serde(
        default,
        rename = "release-group",
        skip_serializing_if = "Option::is_none"
    )]
    pub release_group: Option<ReleaseGroupRef>,
    #[serde(
        default,
        rename = "release-events",
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub release_events: Vec<ReleaseEvent>,
    #[serde(
        default,
        rename = "label-info",
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub label_info: Vec<LabelInfo>,
    #[serde(
        default,
        rename = "cover-art-archive",
        skip_serializing_if = "Option::is_none"
    )]
    pub cover_art_archive: Option<CoverArtArchive>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub media: Vec<Medium>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tags: Vec<Tag>,
    #[serde(
        default,
        deserialize_with = "list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub genres: Vec<Genre>,
    #[serde(
        default,
        deserialize_with = "opt_str",
        skip_serializing_if = "Option::is_none"
    )]
    pub annotation: Option<String>,
}

impl Release {
    pub fn tidy(&mut self, keep_supplementary: bool) {
        let mut no_rating = None;
        supplementary_policy(
            keep_supplementary,
            &mut self.tags,
            &mut self.genres,
            &mut no_rating,
            &mut self.annotation,
        );
        if let Some(t) = &self.text_representation {
            if t.language.is_none() && t.script.is_none() {
                self.text_representation = None;
            }
        }
        let release_credit = &self.artist_credit;
        for track in self.media.iter_mut().flat_map(|m| m.tracks.iter_mut()) {
            if same_artists(&track.artist_credit, release_credit) {
                track.artist_credit.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artist_drops_relations_and_empty_values() {
        let line = r#"{"id":"ca264abf-3eb6-4d53-827b-6ab16988a4a3","name":"X","sort-name":"X",
            "disambiguation":"","relations":[{"a":1}],"tags":[{"name":"rock","count":2}],
            "life-span":{"begin":null,"end":null,"ended":false},"gender":null}"#;
        let mut a: Artist = serde_json::from_str(line).unwrap();
        a.tidy(false);
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(
            json,
            r#"{"id":"ca264abf-3eb6-4d53-827b-6ab16988a4a3","name":"X","sort-name":"X"}"#
        );
    }

    #[test]
    fn supplementary_only_when_asked() {
        let line = r#"{"id":"ca264abf-3eb6-4d53-827b-6ab16988a4a3","name":"X","tags":[{"name":"rock","count":2}]}"#;
        let mut a: Artist = serde_json::from_str(line).unwrap();
        a.tidy(true);
        assert!(serde_json::to_string(&a)
            .unwrap()
            .contains(r#""tags":[{"name":"rock","count":2}]"#));
    }

    #[test]
    fn credit_text_joins_names_and_join_phrases() {
        let g: ReleaseGroup = serde_json::from_str(
            r#"{"id":"a","title":"t","artist-credit":[
              {"name":"A","joinphrase":" feat. ","artist":{"id":"1","name":"A!"}},
              {"name":"","joinphrase":"","artist":{"id":"2","name":"B"}}]}"#,
        )
        .unwrap();
        assert_eq!(credit_text(&g.artist_credit), "A feat. B");
    }

    #[test]
    fn track_credit_dropped_when_same_as_release() {
        let line = r#"{"id":"r","title":"t","artist-credit":[{"artist":{"id":"1","name":"A"}}],
          "release-group":{"id":"g","title":"gt","extra":1},
          "media":[{"position":1,"track-count":2,"data-tracks":[],"tracks":[
            {"id":"t1","position":1,"title":"one","artist-credit":[{"artist":{"id":"1","name":"A"}}]},
            {"id":"t2","position":2,"title":"two","artist-credit":[{"artist":{"id":"9","name":"Z"}}]}]}]}"#;
        let mut r: Release = serde_json::from_str(line).unwrap();
        r.tidy(false);
        let tracks = &r.media[0].tracks;
        assert!(tracks[0].artist_credit.is_empty());
        assert_eq!(tracks[1].artist_credit.len(), 1);
    }
}
