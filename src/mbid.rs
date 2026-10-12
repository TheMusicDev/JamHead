//! MusicBrainz IDs. Stored as 16 raw bytes, shown as hyphenated lowercase UUID strings.

use anyhow::Result;
use uuid::Uuid;

pub type Mbid = [u8; 16];

pub fn parse(s: &str) -> Result<Mbid> {
    Ok(Uuid::parse_str(s)?.into_bytes())
}

pub fn format(id: &Mbid) -> String {
    Uuid::from_bytes(*id).hyphenated().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let s = "ca264abf-3eb6-4d53-827b-6ab16988a4a3";
        assert_eq!(super::format(&super::parse(s).unwrap()), s);
        assert!(super::parse("nope").is_err());
    }
}
