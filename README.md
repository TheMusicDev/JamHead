# JamHead

**Tiny music brains.** A read-optimized, MusicBrainz-style metadata API for artists, release groups and
releases, written in Rust. One small static binary, one small Docker image, data in a mounted volume.

An ingest command builds a read-only data directory from the MusicBrainz JSON dumps. The `serve` command
answers requests from it. Lookups by ID are one memory-mapped read; names are searched through an FST
(prefix, typo-tolerant) and Tantivy (ranked full text).

See [SPEC.md](SPEC.md) for scope and milestones and [docs/schema.md](docs/schema.md) for the data model.

## Quick start

```
cargo build --release
./target/release/jamhead ingest --input tests/fixtures/dump --out ./data
./target/release/jamhead serve --data ./data
curl localhost:8080/v1/meta
```

## Data and licensing

All data comes from [MusicBrainz](https://musicbrainz.org). Thank you to the MusicBrainz community.

- **Core data is CC0** (public domain). That is everything JamHead serves by default.
- **Supplementary data is CC BY-NC-SA 3.0**: tags, genres, ratings and annotations. JamHead leaves it out unless
  you ingest with `--include-supplementary`. If you do, the non-commercial and share-alike terms apply to
  that data and to anything you serve from it.

MusicBrainz is a trademark of the MetaBrainz Foundation. JamHead is not affiliated with or endorsed by them.
See <https://musicbrainz.org/doc/MusicBrainz_Database/Schema> and
<https://musicbrainz.org/doc/About/Data_License>.

## License

Code: MIT, see [LICENSE](LICENSE).
