//! Name normalization shared by ingest (what goes into the search index) and
//! search (what the user typed), so the two always agree.
//!
//! "Björk" -> "bjork", "AC/DC" -> "ac dc", "  Beyoncé " -> "beyonce".

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    // NFKD splits "é" into "e" + an accent mark, which we then drop.
    for c in s.nfkd() {
        if is_combining_mark(c) {
            continue;
        }
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn folds_case_accents_and_punctuation() {
        assert_eq!(normalize("Björk"), "bjork");
        assert_eq!(normalize("AC/DC"), "ac dc");
        assert_eq!(normalize("  Beyoncé  "), "beyonce");
        assert_eq!(normalize("Sigur   Rós!"), "sigur ros");
        assert_eq!(normalize("!!!"), "");
    }
}
