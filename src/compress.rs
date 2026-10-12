//! Pre-compressed blobs. Every stored blob exists in three forms (plain, zstd, gzip), so the
//! server can send whichever one the client accepts without compressing anything per request.
//!
//! Both formats can be concatenated: several zstd frames (or gzip members) in a row decode to
//! the concatenation of their contents. The list endpoints use that to join stored blobs
//! without decompressing them.

use anyhow::Result;
use flate2::{write::GzEncoder, Compression};
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Identity,
    Zstd,
    Gzip,
}

impl Encoding {
    pub const ALL: [Encoding; 3] = [Encoding::Identity, Encoding::Zstd, Encoding::Gzip];

    /// Value of the `Content-Encoding` header, if any.
    pub fn header(self) -> Option<&'static str> {
        match self {
            Encoding::Identity => None,
            Encoding::Zstd => Some("zstd"),
            Encoding::Gzip => Some("gzip"),
        }
    }

    /// Different representations of a resource need different ETags.
    pub fn etag_suffix(self) -> &'static str {
        match self {
            Encoding::Identity => "",
            Encoding::Zstd => "-zstd",
            Encoding::Gzip => "-gzip",
        }
    }

    /// Pick the best encoding from an `Accept-Encoding` header value: zstd, then gzip, else plain.
    pub fn negotiate(accept: &str) -> Encoding {
        let accepts = |name: &str| {
            accept.split(',').any(|part| {
                let mut it = part.split(';');
                let token = it.next().unwrap_or("").trim();
                let refused = it.any(|p| matches!(p.trim().strip_prefix("q="), Some(q) if q.trim().parse::<f32>() == Ok(0.0)));
                token.eq_ignore_ascii_case(name) && !refused
            })
        };
        if accepts("zstd") {
            Encoding::Zstd
        } else if accepts("gzip") {
            Encoding::Gzip
        } else {
            Encoding::Identity
        }
    }
}

// ponytail: level 6 for both. Blobs are compressed once at ingest and served many times, so a higher
// level would buy a little space for a lot of ingest time; revisit if the data volume gets large.
const LEVEL: i32 = 6;

pub fn encode(enc: Encoding, bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(match enc {
        Encoding::Identity => bytes.to_vec(),
        Encoding::Zstd => zstd::bulk::compress(bytes, LEVEL)?,
        Encoding::Gzip => {
            let mut e = GzEncoder::new(Vec::new(), Compression::new(LEVEL as u32));
            e.write_all(bytes)?;
            e.finish()?
        }
    })
}

/// One entity's JSON in all three forms.
pub struct Blob {
    pub plain: Vec<u8>,
    pub zstd: Vec<u8>,
    pub gzip: Vec<u8>,
}

impl Blob {
    pub fn new(plain: Vec<u8>) -> Result<Blob> {
        Ok(Blob {
            zstd: encode(Encoding::Zstd, &plain)?,
            gzip: encode(Encoding::Gzip, &plain)?,
            plain,
        })
    }

    /// Only the uncompressed JSON. Ingest spools these and the writer compresses them later, when it
    /// puts them in the database (so the work is not done twice).
    pub fn plain_only(plain: Vec<u8>) -> Blob {
        Blob {
            plain,
            zstd: Vec::new(),
            gzip: Vec::new(),
        }
    }

    pub fn get(&self, enc: Encoding) -> &[u8] {
        match enc {
            Encoding::Identity => &self.plain,
            Encoding::Zstd => &self.zstd,
            Encoding::Gzip => &self.gzip,
        }
    }
}

pub fn decode(enc: Encoding, bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    Ok(match enc {
        Encoding::Identity => bytes.to_vec(),
        Encoding::Zstd => zstd::stream::decode_all(bytes)?,
        Encoding::Gzip => {
            let mut out = Vec::new();
            flate2::read::MultiGzDecoder::new(bytes).read_to_end(&mut out)?;
            out
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_prefers_zstd_then_gzip_and_honours_q0() {
        assert_eq!(
            Encoding::negotiate("gzip, deflate, br, zstd"),
            Encoding::Zstd
        );
        assert_eq!(Encoding::negotiate("gzip, deflate"), Encoding::Gzip);
        assert_eq!(Encoding::negotiate("br"), Encoding::Identity);
        assert_eq!(Encoding::negotiate(""), Encoding::Identity);
        assert_eq!(Encoding::negotiate("zstd;q=0, gzip;q=0.5"), Encoding::Gzip);
        assert_eq!(Encoding::negotiate("GZIP"), Encoding::Gzip);
    }

    #[test]
    fn round_trip_and_concatenation() {
        let (a, b) = (b"{\"a\":1}".to_vec(), b",{\"b\":2}".to_vec());
        for enc in [Encoding::Zstd, Encoding::Gzip] {
            let joined = [encode(enc, &a).unwrap(), encode(enc, &b).unwrap()].concat();
            assert_eq!(
                decode(enc, &joined).unwrap(),
                [a.clone(), b.clone()].concat()
            );
        }
        let blob = Blob::new(a.clone()).unwrap();
        for enc in Encoding::ALL {
            assert_eq!(decode(enc, blob.get(enc)).unwrap(), a);
        }
    }
}
