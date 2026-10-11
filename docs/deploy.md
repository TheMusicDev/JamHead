# Running JamHead

There is no production deploy target yet. This page covers running locally and what any host needs.

## Requirements: read this before you download anything

The full MusicBrainz catalog is big. Pick the size that fits your disk:

| what you run | download | disk for the data | memory | time (10-core laptop) | status |
|---|---:|---:|---:|---:|---|
| the test fixture (`tests/fixtures/dump`) | 0 (in the repo) | 1.5 MB | negligible | 1 s | measured |
| 10,000 artists + release groups + releases (partial download) | about 150 MB | 157 MB | small | 10 s | measured |
| 300,000 artists + release groups + releases | about 2.7 GB (partial download) | 4.4 GB | about 0.5 GB private | 3 min | measured |
| **the full catalog** | **about 25 GB** (artist 2 GB, release-group 1 GB, release 22 GB) | **about 66 GB per data version** | roughly 3 to 4 GB private during ingest, plus file cache | about 35 min plus the download | **estimate** (see [benchmarks.md](benchmarks.md)) |

For the full catalog, **plan for about 250 GB of free disk**:

- A new version is built next to the live one and the previous one is kept for rollback, so while an update runs
  there are **three** versions on disk (about 200 GB).
- The downloaded dumps (about 25 GB) are on disk until the update finishes (they are deleted afterwards unless you
  pass `--keep-dumps`).
- The old versions are only pruned after the new one is live.

The 66 GB and the 250 GB are extrapolations from the 300,000-per-kind run, not measurements. Nobody has run the
full catalog yet. After a real run, `jamhead stats` prints the true sizes; please correct this table.

Other things a host needs:

- The data volume must be **writable** (LMDB keeps a small `lock.mdb` next to the data file even when reading).
- Plenty of RAM helps because the databases are memory-mapped, but it is file cache the kernel can drop. Serving
  300,000 per kind used 300 to 590 MB resident.
- CPU: ingest uses every core; serving is light.
- Outbound HTTPS to `data.metabrainz.org` for `download` / `update`.
- Something in front of the API for TLS and request timeouts. The server speaks plain HTTP and has no auth.

To work with much less than the full catalog, see "Small data sets" below.

## The pieces

| command | what it does |
|---|---|
| `jamhead serve --data ./data` | the API. Follows the `current` link: a new data version goes live by itself within 10 s |
| `jamhead update --data ./data --dumps ./dumps` | download the newest MusicBrainz dump, build a new data version, flip `current`, delete the dump |
| `jamhead download` / `jamhead ingest` | the two halves of `update`, for doing them by hand |
| `jamhead verify --data ./data [--deep]` | integrity check plus sanity queries. Exit code 1 if anything is wrong |
| `jamhead stats --data ./data` | counts, sizes per table, dump date |
| `jamhead healthcheck` | exit 0 if the local server answers `/healthz` (for container health checks) |

`jh` is the same program: the Docker image has both names.

The data directory looks like this and is only ever added to by ingest, never edited:

```
data/current -> v2026-10-11T030000/     the live version (a symlink)
data/v2026-10-11T030000/                the live version
data/v2026-10-08T030000/                the previous one, kept for rollback
```

Ingest builds into `.building-<version>/`, renames it into place when it is finished and then flips
`current`. A failed ingest deletes its half-built directory. After a successful run all but the newest two
versions are removed (the live one is never removed).

## Small data sets

The full `download` fetches whole files, so it cannot be used for a small trial. Today's options, from smallest:

1. **The fixture**, in the repo, already linked together:
   `jamhead ingest --input tests/fixtures/dump --out ./data`
2. **`--limit N`** reads only the first N records of each dump, but only from files you already have.
3. **A partial download by hand.** The dumps are `.tar.xz` streams, so the first part of each file is usable.
   Fetch just the start with an HTTP range request, then ingest with a limit. **Measured**: 10,000 records of each
   kind is a 148 MB download, ingests in about 10 s and gives a 157 MB data directory:

   ```
   B=https://data.metabrainz.org/pub/musicbrainz/data/json-dumps/$(curl -s https://data.metabrainz.org/pub/musicbrainz/data/json-dumps/LATEST)
   mkdir -p dumps/sample && cd dumps/sample
   curl -r 0-60000000 -o release.tar.xz       $B/release.tar.xz
   curl -r 0-80000000 -o artist.tar.xz        $B/artist.tar.xz
   curl -r 0-10000000 -o release-group.tar.xz $B/release-group.tar.xz
   cd ../.. && jamhead ingest --input dumps/sample --out ./data --limit 10000
   ```

   Notes: the early artists in the dump are huge (they carry millions of relationship rows), which is why artists
   need more bytes than their count suggests; if a range is too short, ingest stops with "the file may be cut off"
   and you fetch more or lower `--limit`. The first records of the three files are mostly unrelated to each other,
   so artist to release-group to release lists are only partly filled. Lookups and all search modes work.

## Docker image

```
docker buildx build --platform linux/arm64,linux/amd64 -t <registry>/jamhead:latest --push .
docker buildx build --platform linux/arm64 -t jamhead:dev --load .     # one platform, into the local daemon
```

The binary is static (about 11 MB) and sits on a plain Alpine base (whole image about 27 MB) rather than
`scratch`, so there is a shell for `docker exec`, one-off commands and rollbacks. It builds on Alpine, which is
musl-native for both architectures, so there is no cross-compiling. The first build compiles every dependency
(several minutes); later builds reuse the cached dependency layer until `Cargo.toml` changes. The arm64 and
amd64 images both build; amd64 was only built (under emulation), not load-tested.

The data is never in the image. Mount a volume at `/data`.

Try it locally:

```
docker run --rm -v $PWD/tests/fixtures/dump:/in:ro -v jh-data:/data jamhead:dev ingest --input /in --out /data
docker run --rm -p 8080:8080 -v jh-data:/data jamhead:dev
curl localhost:8080/v1/meta
```

The image declares a health check (`jamhead healthcheck`, every 30 s). Until the first data version exists `serve`
exits with "no data ... run `jamhead ingest` first".

## Keeping data fresh

Run `jamhead update --data ./data --dumps ./dumps` from any scheduler (cron, a systemd timer, your platform's
scheduled jobs). MusicBrainz publishes new dumps a few times a week. When nothing is new the command prints
"already up to date" and exits in a second. The running server notices the new version by itself (it checks the
`current` link every 10 s), so no restart and no downtime; requests already in flight finish on the old version.

Optional settings for `serve`:

| flag / env | default | meaning |
|---|---|---|
| `--bind` | `0.0.0.0:8080` | listen address |
| `--watch-interval` | 10 | seconds between checks of the `current` link; 0 turns it off |
| `--admin-token` / `JAMHEAD_ADMIN_TOKEN` | none | enables `POST /admin/reload` (`Authorization: Bearer <token>`); without a token the route returns 404 |

A reload can also be triggered with `kill -HUP <pid>`. If the new version cannot be opened, the old one keeps
serving and the problem is logged.

## Rolling back

Point `current` at the previous version. The server follows within 10 s.

```
ln -sfn v2026-10-08T030000 data/current
```

## Supplementary data

By default only core (CC0) data is stored. Adding `--include-supplementary` to `ingest`/`update` also stores tags,
genres, ratings and annotations, which are CC BY-NC-SA: non-commercial and share-alike. Only turn it on if that
fits how you use the API.

## What is compressed, and how

Every blob is stored three times: plain, zstd and gzip. The server picks one from `Accept-Encoding` (zstd, then
gzip, then plain) and sends the stored bytes as they are. This adds about 70% to the store size (measured in
[benchmarks.md](benchmarks.md)) and cuts a typical response to about 40% of its size. Brotli is not offered.
