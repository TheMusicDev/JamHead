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

Tight on disk? `--keep-versions 1` (on `ingest` and `update`) deletes the previous version as soon as the new one
is live. Peak disk is then the live version plus the new one (about 135 GB, plus 25 GB of dumps), at the price of
having no version to roll back to. The very first build needs only about 90 GB (one version plus the dumps).

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

Released versions are published to the GitHub Container Registry for both `linux/amd64` (Intel/AMD, most NAS and
servers) and `linux/arm64` (Apple silicon, Raspberry Pi), so `docker pull` picks the right one by itself:

```
docker pull ghcr.io/themusicdev/jamhead:0.1.0
```

- Tags are exact versions only (`0.1.0`). There is no `latest` and no `0.1`: pin the version you run, so you
  always know what is running and upgrading is a deliberate edit.
- A version is published by pushing a tag that matches `Cargo.toml` (`git tag v0.1.0 && git push origin v0.1.0`),
  which runs `.github/workflows/publish.yml`. Pushes to `main` never publish.
- The first time a version is published, check the package's visibility on GitHub (the repository's *Packages*
  section). It must be **public** for a NAS to pull it without logging in.

To build your own instead (development, or a platform not published):

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

## Synology NAS (DS923+ class)

Everything below uses only the DSM web interface (Container Manager). Nothing is built or copied by hand: DSM pulls
the published image itself. Steps 1 and 2 were run on a real NAS (version 0.1.0, image pulled and started). Step 3
onward is still untried, so treat its menu names as from memory.

**Serving** needs about 0.5 GB of private memory plus whatever file cache the kernel gives it. **Ingesting the
full catalog on a stock 4 GB DS923+ is untested and probably will not fit**, for two reasons that are estimates,
not measurements:

- *Memory.* Ingest keeps every record's search name in RAM while it runs. 300,000 per kind used about 430 MB, and
  the full catalog is roughly 4 times that. DSM itself uses about 1 GB. A 16 GB RAM upgrade is the simple fix.
- *Disk speed.* Records are written in dump order but keyed by random IDs, so every write lands at a random spot in
  a file of tens of GB. That is fine on an SSD and may be very slow on a spinning disk. Nobody has timed it on
  an HDD yet.

Set up:

1. In *File Station*, create the folders `data` and `dumps` inside a shared folder of your choice (the examples use
   a shared folder called `shared-apps`, so `shared-apps/jamhead/data` and `shared-apps/jamhead/dumps`). Create
   them yourself so they are not made by Docker as root-owned folders.
2. In *Container Manager > Project > Create*, name it `jamhead`. The **Path** is the project's own folder (where DSM
   keeps its `compose.yaml`), as a shared-folder path such as `/shared-apps/jamhead`. It is not a volume. Choose
   *Create docker-compose.yml* as the source and paste the file below. Pin an exact version; DSM downloads the image
   when the project starts. In the compose file the same place is written with its volume, `/volume1/shared-apps/...`.
   On the *Web portal settings* screen, **untick** "Set up web portal via Web Station": it is not needed to reach
   the API at `http://<NAS-IP>:8080` and asks you to install Web Station.

   ```yaml
   services:
     jamhead:
       image: ghcr.io/themusicdev/jamhead:0.1.0
       restart: unless-stopped
       ports: ["8080:8080"]
       volumes: ["/volume1/shared-apps/jamhead/data:/data"]
   ```

   Until a data version exists the container stops with "no data ... run `jamhead ingest` first" (seen on a real
   NAS). Because of `restart: unless-stopped` it then restarts and stops over and over, which is harmless but
   noisy: stop the project until data exists. The log is under *Container Manager > Container > (the container) >
   Details > Log*, or the project's *Log* tab.
3. Build the data with a second project, `jamhead-update`, that runs once and exits. DSM allows only one
   `compose.yaml` per project folder, so give it its own folder (Path `/shared-apps/jamhead-update`). Its volumes
   still point at the same `data` folder as the server, so both projects share one data directory:

   ```yaml
   services:
     update:
       image: ghcr.io/themusicdev/jamhead:0.1.0
       restart: "no"
       mem_limit: 3g
       command: ["update", "--data", "/data", "--dumps", "/dumps", "--keep-versions", "1"]
       volumes:
         - /volume1/shared-apps/jamhead/data:/data
         - /volume1/shared-apps/jamhead/dumps:/dumps
   ```

   - **It finishes by itself.** It downloads the dumps, ingests, prints `ingest done: v...` and
     `now serving-ready: ...`, then exits (status *Exited*, code 0). Nothing needs stopping.
   - **Why two projects, not one.** Starting a project starts every service in it, so one merged file could rerun
     the update (a new 25 GB download and a full ingest) whenever the project is restarted. The server should stay
     up for months; the update is a rare job of hours.
   - **`mem_limit: 3g`** caps the update container so a runaway ingest is killed on its own instead of starving DSM
     (4 GB total, about 1 GB used by DSM). Raise it if you have more RAM. If it is killed, a half-built
     `.building-...` folder may be left in `data/`; delete it by hand in File Station.
   - Read the log under *Container Manager > Container > (click the container) > Log*, and press the refresh
     icon at the bottom right (it does not update itself). The project page has no log tab. Memory and CPU are on the
     container's *Statistics* tab.
   - The running `jamhead` server picks the new version up within 10 s, with no restart.
4. **Changing the version.** Editing the image tag in the YAML is not enough: DSM does not pull the new image on
   *Start*, and a project in that state can get stuck (it still looks "running", and Stop, Clean and Restart do
   nothing). Do it in this order: *Stop* the project, edit the YAML, press **Action > Build** (this pulls the new
   image and recreates the container), then *Start*. If a project is already stuck, stopping and starting the
   Container Manager package in *Package Center* clears DSM's stale state (it stops every container on the NAS for a
   minute). Your `data` and `dumps` folders are never touched by any of this.
5. To refresh later, start `jamhead-update` again (or schedule it in *Control Panel > Task Scheduler* with the
   command `docker start jamhead-update-update-1`; check the exact container name in Container Manager).

**HTTPS.** JamHead speaks plain HTTP and has no login. On a home network that is fine. Before exposing it beyond
your LAN, put DSM's reverse proxy in front (*Control Panel > Login Portal > Advanced > Reverse Proxy*: HTTPS
on your hostname to HTTP `localhost:8080`, with a certificate from *Control Panel > Security > Certificate*). Web
Station is not needed for that. Menu names from memory.

**Reading the log.** `update` and `ingest` print plain lines, one per update:

```
Reading releases from release.tar.xz (22.0 GB). This is the slow part.
Reading releases: 12% (about 55 min left) (610000 records so far, 958/s)
Reading releases: done, 5012345 records in 1 h 5 min
Building search indexes for releases (a few minutes, no progress lines)...
Writing release records to the database, in order...
  release: 25% written
Compacting the database (one more copy ...). This can be quiet for a while...
DONE in 2 h 14 min. Data version v... is now the live one.
```

- **Finished** means a line starting with `DONE`. On a failure the last line starts with `FAILED:` and says why.
- **The container stops by itself when it is done** (status Exited, code 0).
- **"About N left"** is a guess from the speed so far and is only printed after the first 20 seconds. Percent is the
  share of the compressed dump file that has been read.
- **Quiet stretches are normal** before the lines that say "no progress lines" or "can be quiet": checking a big
  file's checksum, building indexes and compacting each take minutes without printing.
- **If the container ends without a DONE or FAILED line**, it was killed from outside, most likely by the memory
  cap (`mem_limit`). Look in the `data` folder for a leftover `.building-...` folder and delete it.
- **While ingesting, the build folder needs extra temporary space**, roughly 40% of the finished data size
  (an estimate, see [benchmarks.md](benchmarks.md)); it is freed as it is written.

Plan the first run in steps, and write what you measure into [benchmarks.md](benchmarks.md): a small data set
first (see the roadmap: a `sample` option is planned so this needs no terminal), then 300,000 per kind, then the
full catalog. If memory climbs toward the limit during ingest, stop the project (a failed ingest leaves the live
version untouched) and either add RAM or ask for the low-memory ingest change.

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
