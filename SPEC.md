# JamHead: tiny music brains

A read-optimized, MusicBrainz-style metadata API for artists, release groups and releases. Written in Rust, shipped as one small static binary and a tiny Docker image. Data comes from MusicBrainz JSON dumps, ingested offline by a CLI into a read-only store.

Repo: `TheMusicDev/JamHead` (public)
Binary / CLI: `jamhead` (alias `jh`)
Owner: Sir Angel. He does not know Rust, so keep the code simple, commented where non-obvious, and verified by tests he can read in TypeScript (see Testing).

## Goals

1. Fastest practical read path: ID lookups are one memory-mapped read plus a byte copy. No query planner, no joins, no per-request JSON marshalling.
2. Fast name search: prefix/autocomplete, typo-tolerant matching and ranked full-text.
3. Portable: one binary, one Dockerfile, an image of 10-30 MB, data in a mounted volume.
4. Simple updates: a CLI downloads the latest dump, builds a fresh data directory, and the API swaps to it atomically with no downtime.
5. Measured, not assumed: benchmarks live in the repo and are rerunnable.

## Non-goals (v1)

- No writes, editing or user accounts. The data is read-only.
- No ad-hoc SQL. New access patterns mean new precomputed indexes at ingest.
- No incremental/replication-feed updates. Full rebuild from each dump.
- No entities beyond artist, release-group and release (recordings, works, labels etc. come later).

## Stack

| Concern | Choice |
|---|---|
| Language | Rust (stable) |
| HTTP | `axum` (tokio) |
| Entity store | LMDB via `heed`, read-only env, zero-copy reads |
| Entity values | Pre-rendered JSON blobs (optionally zstd-compressed variants) keyed by ID |
| Prefix/fuzzy name search | `fst` crate (prefix, Levenshtein automata) |
| Full-text ranked search | `tantivy`, in-process, one index per entity type |
| JSON parsing (ingest) | `serde_json` streaming, `simd-json` if profiling justifies it |
| CLI | `clap` |
| Parallelism (ingest) | `rayon` / tokio worker pool |
| Container | Multi-stage Docker build, `musl` static target, `scratch` or `distroless` final image |

Put storage behind a trait (`EntityStore`) so LMDB can be swapped for a custom mmap'd file format later without touching the API or ingest logic.

## Entities (v1)

Artist, release group, release. Store only what the API serves, plus IDs for future joins.

- Keys: MusicBrainz MBIDs (UUIDs) as 16-byte binary keys internally, rendered as hyphenated strings in the API.
- Each entity blob is the exact JSON the API returns for `GET /v1/<entity>/{mbid}`.
- Relationships are precomputed at ingest and embedded as ID lists, e.g. artist -> release-group IDs, release-group -> release IDs, release -> artist-credit IDs. No joins at read time.
- Keep foreign IDs for entities not ingested yet (recordings, labels) in the blob so they can be joined later without re-ingesting.
- Precompute counts and sort orders (e.g. release counts, first-release year).

Decide the exact blob fields in the first milestone by sampling the dump, then write them down in `docs/schema.md`.

## Data source

MusicBrainz JSON dumps (line-delimited JSON, one entity per line, compressed archives), published under `data.metabrainz.org`. Verify the current URL layout and file names before coding. Use the `artist`, `release-group` and `release` dumps. The `release` dump is by far the largest (tens of GB compressed), so ingest must stream: never load a whole file into memory.

Licensing: core MusicBrainz data is CC0. Supplementary data (tags, ratings, annotations) is CC BY-NC-SA. Include only core fields by default and put anything supplementary behind a clearly named flag, with the licensing noted in the README.

## API (v1)

All responses are JSON, served from precomputed blobs where possible.

- `GET /v1/artist/{mbid}`, `/v1/release-group/{mbid}`, `/v1/release/{mbid}`
- `GET /v1/artist/{mbid}/release-groups` and `/v1/release-group/{mbid}/releases` (paged ID lists resolved to blobs)
- `GET /v1/search/{entity}?q=...&limit=...` with `mode=prefix|fuzzy|text` (autocomplete uses `prefix`)
- `GET /healthz` and `GET /v1/meta` (dump date, data version, entity counts)
- Honor `Accept-Encoding`: serve pre-compressed blobs (zstd/br/gzip) directly when the client accepts them.
- Cache headers keyed to the data version (ETag = data version + MBID).

## Data directory layout

Each ingest writes a fresh versioned directory, never modifying a live one:

```
data/
  current -> v2026-10-11T0300/      # symlink, flipped atomically
  v2026-10-11T0300/
    meta.json                        # dump date, counts, schema version, checksums
    store/                           # LMDB env (data.mdb)
    fst/                             # artist.fst, release_group.fst, release.fst
    tantivy/                         # artist/, release_group/, release/
```

The server watches `current` (or receives `SIGHUP` / `POST /admin/reload`, bound to localhost or token-protected), opens the new directory, swaps it in behind an `ArcSwap`, and lets in-flight requests finish on the old one before it is dropped. Keep the previous version for rollback and prune older ones.

## CLI

One binary, subcommands:

- `jh serve --data ./data --bind 0.0.0.0:8080`
- `jh download [--dump latest|<date>] [--entities artist,release-group,release]` fetches dumps, verifies checksums, resumes interrupted downloads
- `jh ingest --input <dir> --out ./data [--limit N]` builds the full versioned data directory; `--limit` builds a small sample for local dev in seconds
- `jh update` runs `download` then `ingest` then the atomic flip
- `jh verify --data ./data` checks integrity and runs sanity queries
- `jh stats` prints counts and sizes

Ingest design: one parser + one builder per entity type behind a common trait, sharing streaming, batching, parallelism and the build/swap machinery. Adding an entity later should mean adding one parser/builder pair, not reworking the pipeline.

## Docker and deploy

- Multi-stage `Dockerfile`: Rust build stage (use `cargo-chef` or similar for layer caching), final stage `scratch`/`distroless` containing only the binary.
- Target both `linux/arm64` and `linux/amd64`. Document the `buildx` command. (No production host has been chosen yet.)
- Data is a mounted volume, never baked into the image.
- The same image runs `serve` (long-running) and `update` (scheduled job). Document how to run it with a volume and a scheduled `jh update`.
- Keep memory predictable: LMDB is mmap'd, FSTs are loaded or mmap'd, and the Tantivy readers are shared.

## Local development

Run the real Rust binary locally on a sample dataset, with no second implementation.

- `jh ingest --limit 50000` against a sample dump builds a dev data directory in seconds.
- Use `cargo watch` for rebuilds. `data/` is gitignored.
- Provide a tiny checked-in fixture dump (a few hundred hand-picked entities) for tests, so tests never need the full dump.

## Testing and benchmarks

The owner knows Bun/TypeScript, not Rust, so correctness is defined in TypeScript outside the Rust code:

- `tests/api/` holds black-box tests written for Bun (`bun test`) that run against a locally started `jamhead` over HTTP: lookups, relationships, search modes, 404s, compression, reload-while-serving.
- `bench/` holds a Bun load script (plus `oha` or `wrk` instructions) measuring p50/p99 latency and throughput for ID lookup, prefix search, fuzzy search and text search. Record results in `docs/benchmarks.md` for each milestone.
- Rust unit tests cover parsers and builders against the fixture dump.
- CI (GitHub Actions): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, the Bun API tests, and a Docker build.

## Milestones

1. **Repo and skeleton.** Create the repo (see below), add `CLAUDE.md`, this `SPEC.md`, license, CI, empty `serve`/`ingest` commands, Dockerfile. Sample the dumps and write `docs/schema.md`.
2. **Artists end to end.** Ingest artist dump -> LMDB + FST, serve `GET /v1/artist/{mbid}` and prefix search, Bun API tests, first benchmark.
3. **Release groups and releases.** Add both entity builders, the relationship ID lists, and the list endpoints.
4. **Search.** Add Tantivy full-text and fuzzy modes, unify the `/v1/search/{entity}` endpoint.
5. **Operations.** `download` and `update`, atomic swap and reload, `verify`, compression, ETags, deployment docs.
6. **Hardening.** Full-dump run on the real data, memory/latency profiling, decide whether LMDB should be replaced behind the `EntityStore` trait.

## Repo setup (first step for Claude Code)

Create the repo as **public** under the TheMusicDev org using the GitHub CLI:

```
gh repo create TheMusicDev/JamHead --public --description "Tiny music brains: a read-optimized MusicBrainz-style API in Rust" --clone
```

Then add `SPEC.md` and `CLAUDE.md` to the repo root, commit, and push to `main`. If `gh` is not authenticated or lacks permission on the org, stop and tell the owner instead of working around it. Use the MIT or Apache-2.0 license unless told otherwise, and credit MusicBrainz in the README with the data licensing notes above.

## Open questions to settle with the owner as they come up

- Exact blob fields per entity (decided after sampling the dump, in milestone 1).
- Whether to keep supplementary (CC BY-NC-SA) fields at all.
- Whether `/admin/reload` is needed or the symlink watch is enough.

## Status and deviations (kept current as the code changes)

All six milestones are built and tested, except that the full-dump run in milestone 6 was not possible on the
development machine (disk); a 300,000-per-kind run on the real archives was done instead and the rest is an
estimate. See `docs/benchmarks.md`. Where the code differs from this document:

- **Relationship lists are separate tables**, not ID lists embedded in the blobs; blobs carry only the counts
  (`release-group-count`, `release-count`). See `docs/schema.md` for why.
- **Compression**: every blob is stored plain, zstd and gzip (no brotli). Chosen with `Accept-Encoding`.
- **Reload**: all three ways are implemented: the server polls `current` (10 s default), `SIGHUP`, and
  `POST /admin/reload` with a bearer token (off unless `--admin-token` / `JAMHEAD_ADMIN_TOKEN` is set).
- **Supplementary data** (tags, genres, ratings, annotations) is off by default and kept only with
  `--include-supplementary`.
- **Final Docker image is Alpine, not scratch**, so there is a shell in the container for `docker exec` and rollbacks. The binary itself is fully static.
- The data volume must be writable (LMDB keeps `lock.mdb` next to the data file).
- `jh` is a symlink to `jamhead` in the Docker image, not a second Cargo binary.
- Dump PGP signatures (`.asc`) are not verified; SHA256SUMS are.
