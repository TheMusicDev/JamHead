# Running JamHead

## The pieces

| command | what it does |
|---|---|
| `jamhead serve --data /data` | the API. Follows the `current` link: a new data version goes live by itself within 10 s |
| `jamhead update --data /data --dumps /data/dumps` | download the newest MusicBrainz dump, build a new data version, flip `current`, delete the dump |
| `jamhead download` / `jamhead ingest` | the two halves of `update`, for doing them by hand |
| `jamhead verify --data /data [--deep]` | integrity check plus sanity queries. Exit code 1 if anything is wrong |
| `jamhead stats --data /data` | counts, sizes, dump date |
| `jamhead healthcheck` | exit 0 if the local server answers `/healthz` (for container health checks) |

`jh` is the same program: the Docker image has both names.

The data directory looks like this and is only ever added to by ingest, never edited:

```
/data/current -> v2026-10-11T030000/     the live version (a symlink)
/data/v2026-10-11T030000/                the live version
/data/v2026-10-08T030000/                the previous one, kept for rollback
```

Ingest builds into `.building-<version>/`, renames it into place when it is finished and then flips
`current`. A crashed ingest leaves only a `.building-*` directory, which is safe to delete.
After a successful run all but the newest two versions are removed (the live one is never removed).

## Docker image

```
docker buildx build --platform linux/arm64,linux/amd64 -t <registry>/jamhead:latest --push .
docker buildx build --platform linux/arm64 -t jamhead:dev --load .     # one platform, into the local daemon
```

The binary is static (about 11 MB) and sits on a plain Alpine base (whole image about 27 MB) rather than `scratch`, so
there is a shell for Dokploy's terminal and scheduled tasks. It builds on Alpine, which is musl-native for both
architectures, so there is no cross-compiling. The first build compiles every dependency
(several minutes); later builds reuse the cached dependency layer until `Cargo.toml` changes.

The data is never in the image. Mount a volume at `/data`. **The volume must be writable**: LMDB keeps a small
`lock.mdb` next to the data file when reading.

Try it locally:

```
docker run --rm -v $PWD/tests/fixtures/dump:/in:ro -v jh-data:/data jamhead:dev ingest --input /in --out /data
docker run --rm -p 8080:8080 -v jh-data:/data jamhead:dev
curl localhost:8080/v1/meta
```

## Dokploy (Oracle Cloud A1.Flex, arm64)

One application and one scheduled task, sharing a volume.

1. **Application**: source = this GitHub repo, build type = Dockerfile, container port 8080.
   Add your domain in the Domains tab. Build on the arm64 server itself so no cross-building is needed.
2. **Volume**: in the application's Volumes section, add a volume mount named `jamhead-data` at `/data`.
3. **First data**: open a terminal on the application container (or run the schedule from step 4 once by hand)
   and run `jh update --data /data --dumps /data/dumps`. The first full run downloads about 25 GB
   and takes a while; a quick try is `jh update --data /data --dumps /data/dumps --limit 50000`.
   Until the first data version exists `serve` exits with "no data ... run `jamhead ingest` first", so Dokploy
   will restart it; once the data is there it stays up.
4. **Schedule**: in the application's Schedules section, add a schedule, command
   `jh update --data /data --dumps /data/dumps`, cron `0 3 * * *`. MusicBrainz publishes new dumps a few times a
   week; when nothing is new the command prints "already up to date" and exits in a second.
   The running server picks up the new version by itself (it checks the `current` link every 10 s), so there
   is no restart and no downtime. In-flight requests finish on the old version.
5. **Health check**: the Dockerfile already declares one (`jamhead healthcheck`, every 30 s).

Optional settings for `serve`:

| flag / env | default | meaning |
|---|---|---|
| `--bind` | `0.0.0.0:8080` | listen address |
| `--watch-interval` | 10 | seconds between checks of the `current` link; 0 turns it off |
| `--admin-token` / `JAMHEAD_ADMIN_TOKEN` | none | enables `POST /admin/reload` (`Authorization: Bearer <token>`); without a token the route returns 404 |

A reload can also be triggered with `kill -HUP <pid>`. If the new version cannot be opened, the old one keeps
serving and the problem is logged.

### Disk space

Roughly: the compressed dumps (about 25 GB, deleted after each successful update) plus **three** data versions
at the moment a new one is being built (live, previous, new; the oldest is pruned afterwards). A full-data version
has **not been measured**; extrapolating from a 300,000-per-kind run gives about 66 GB per version, so about
200 GB while an update runs (estimate, see the end of [benchmarks.md](benchmarks.md)). `jh stats` prints the real
size once you have one. Pass `--keep-dumps` to keep the
downloaded dump, for example to re-ingest with different flags.

### Memory

LMDB, the name indexes and the Tantivy indexes are memory-mapped, so most of the "memory" is file cache that
the kernel can drop. Measured sizes are in [benchmarks.md](benchmarks.md). Ingest keeps a few batches of dump lines plus a name table for the search indexes: about 430 MB of private memory
for 300,000 entities per kind, estimated at 3 to 4 GB for the full release dump.

### Rolling back

Point `current` at the previous version. The server follows within 10 s. (The Dokploy menu names above are from
memory and were not checked against a running Dokploy; adjust to what your version shows.)

```
ln -sfn v2026-10-08T030000 /data/current      # run in the container's terminal
```

### Supplementary data

By default only core (CC0) data is stored. Adding `--include-supplementary` to `ingest`/`update` also stores tags,
genres, ratings and annotations, which are CC BY-NC-SA: non-commercial and share-alike. Only turn it on if that
fits how you use the API.

## What is compressed, and how

Every blob is stored three times: plain, zstd and gzip. The server picks one from `Accept-Encoding` (zstd, then
gzip, then plain) and sends the stored bytes as they are. This adds about 70% to the store size (measured in
[benchmarks.md](benchmarks.md)) and cuts a typical response to about 40% of its size. Brotli is not offered.
