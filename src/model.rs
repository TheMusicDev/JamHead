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
}
