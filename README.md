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

## Commands

```
jamhead serve    --data ./data --bind 0.0.0.0:8080     # the API; follows the `current` data version
jamhead ingest   --input <dumps> --out ./data [--limit N] [--include-supplementary]
jamhead download --out ./dumps                          # latest MusicBrainz dump, resumable, checksum-verified
jamhead update   --data ./data --dumps ./dumps          # download + ingest + go live (for a scheduled job)
jamhead verify   --data ./data [--deep]                 # integrity check and sanity queries
jamhead stats    --data ./data
```

`jh` is an alias for `jamhead` in the Docker image. Deployment (Docker, Dokploy, scheduled updates, rollback)
is in [docs/deploy.md](docs/deploy.md).

## API

| request | returns |
|---|---|
| `GET /v1/artist/{mbid}`, `/v1/release-group/{mbid}`, `/v1/release/{mbid}` | the stored JSON for one entity ([fields](docs/schema.md)) |
| `GET /v1/artist/{mbid}/release-groups?limit=&offset=` | an artist's release groups, oldest first |
| `GET /v1/release-group/{mbid}/releases?limit=&offset=` | a release group's releases, oldest first |
| `GET /v1/search/{artist\|release-group\|release}?q=&limit=&mode=prefix\|fuzzy\|text` | `{results: [{id, name, detail?}]}`, see [docs/search.md](docs/search.md) |
| `GET /v1/meta`, `GET /healthz` | data version, dump date, counts / liveness |

Responses honour `Accept-Encoding` (zstd or gzip, served pre-compressed), carry an `ETag` made of the data version
and the MBID, and answer `If-None-Match` with `304`.

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
