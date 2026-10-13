# Benchmarks

Nothing in this repo is called fast without a number in this file. Rerun with `bun run bench`.

Machine for all numbers below: Apple Silicon laptop, 10 cores, 32 GB RAM, macOS. Client and server share
the machine, so the client takes some of the CPU: real numbers on a dedicated server will differ.

## How to run

```
cargo build --release
./target/release/jamhead ingest --input <dumps> --out ./data --limit 50000
bun run bench -- --data ./data                  # Bun client, prints a table
bun run bench -- --data ./data --urls-dir /tmp/urls   # writes URL lists and keeps the server up, then:
oha -z 10s -c 32 --urls-from-file /tmp/urls/id-lookup.txt --no-tui
```

## Milestone 2: artists (2026-10-11)

Data: the first 100,000 artists of the 2026-10-10 MusicBrainz dump with `relations` removed
(the real ingest drops them anyway; stripping just keeps the sample file small).
That is about 4% of the full 2.7M artists, so a full dump has a bigger FST and LMDB file.

Ingest (release build):

| input | time | peak memory |
|---|---:|---:|
| 100,000 artists, 95 MB of JSON | 0.93 s (about 185,000 records/s) | 262 MB |
| 3,000 artists as the dump ships them (with relations), 278 MB | 0.21 s | 216 MB |

Output size for the 100k set: LMDB store 133 MB, name index (FST + postings) 11 MB.

Request latency, 32 concurrent connections, `oha`, 6 s per scenario, random IDs / names from the data:

| scenario | req/s | p50 | p99 |
|---|---:|---:|---:|
| `GET /v1/artist/{mbid}` | 121,500 | 0.26 ms | 0.47 ms |
| `GET /v1/search/artist?q=<4 letters>` (prefix) | 56,300 | 0.42 ms | 3.2 ms |
| `GET /v1/search/artist?mode=fuzzy&q=<typo>` | 10,300 | 2.9 ms | 8.1 ms |

Same scenarios through the Bun client (`bun run bench`, 5 s each): 71,000 / 50,000 / 10,600 req/s.
The client is the bottleneck for ID lookups there, which is why `oha` is used for the headline numbers.

Findings worth keeping:

- Fuzzy search first used the `fst` crate's built-in Levenshtein automaton. It rebuilds its state machine on
  every query: 1,800 req/s at distance 2 (24,000 req/s at distance 1). Switching to the
  `levenshtein_automata` crate, whose expensive tables are built once at startup, gave 10,300 req/s at
  distance 2 and also counts a swapped pair of letters as one typo.
- Prefix search looks at most 20,000 matching names per query (`SCAN_CAP`). A one-letter query only sees the
  alphabetically first names. That is the cause of the p99 above and a known ceiling. (Short prefixes got a
  smaller cap in milestone 6 after measuring them; see below.)

## Milestone 3: release groups and releases (2026-10-11)

Data: the first 20,000 releases of the 2026-10-10 dump (with `relations` emptied) plus the 17,159 release
groups and 5,621 artists those releases point to (taken from the releases' embedded copies), so every
parent/child list is consistent. That is a small slice (the full dump has about 5 million releases), so the
name indexes and the LMDB file are much smaller than a real run.

Ingest of all three files (1.55 GB of release JSON): **1.46 s** (about 1 GB/s of JSON), 445 MB peak memory.
Output: LMDB store 154 MB, name indexes 3.9 MB.

`oha`, 32 connections, 5 s per scenario, random IDs / names from the data:

| scenario | req/s | p50 | p99 | response size |
|---|---:|---:|---:|---:|
| `GET /v1/release/{mbid}` | 121,700 | 0.26 ms | 0.47 ms | 3.7 KB |
| `GET /v1/artist/{mbid}` | 122,900 | 0.25 ms | 0.56 ms | 0.4 KB |
| `GET /v1/artist/{mbid}/release-groups?limit=25` | 121,000 | 0.25 ms | 0.72 ms | 1.3 KB |
| prefix search, release | 108,800 | 0.26 ms | 0.93 ms | 0.6 KB |
| prefix search, artist | 92,700 | 0.30 ms | 1.17 ms | 0.5 KB |
| fuzzy search, release | 28,100 | 1.0 ms | 3.2 ms | 0.2 KB |
| fuzzy search, artist | 22,900 | 1.3 ms | 3.6 ms | 0.2 KB |

Reading the table: all lookups level off at about 122,000 req/s whatever the payload, which means the limit is
the load generator and the loopback network on one machine, not the server. Treat the lookup rows as
"at least this fast", not as the server's ceiling.

Server memory after the runs (`bun run bench`): 117 MB (artist), 185 MB (release group), 198 MB (release).
Most of that is memory-mapped file pages that the OS can drop under pressure.

## Milestone 4: Tantivy text search (2026-10-11)

Same data sets as before. Ingest now also builds a Tantivy index per entity.

| data set | ingest time | peak memory | Tantivy size | FST size |
|---|---:|---:|---:|---:|
| 100,000 artists | 1.7 s (was 0.9 s) | 359 MB | 6.6 MB | 11 MB |
| 20,000 releases + 17,159 groups + 5,621 artists | 2.9 s (was 1.5 s) | 401 MB | 3.4 MB | 3.9 MB |

`oha`, 32 connections, 5 s per scenario. Text queries are the first word of a random real name:

| scenario | req/s | p50 | p99 |
|---|---:|---:|---:|
| text search, 100k artists | 63,000 | 0.47 ms | 1.4 ms |
| text search, artist (small set) | 101,000 | 0.27 ms | 1.1 ms |
| text search, release group | 97,600 | 0.29 ms | 1.1 ms |
| text search, release | 92,400 | 0.30 ms | 1.3 ms |
| (same run) prefix, 100k artists | 53,000 | 0.44 ms | 3.4 ms |
| (same run) fuzzy, 100k artists | 10,400 | 2.8 ms | 7.4 ms |
| (same run) id lookup, 100k artists | 113,000 | 0.27 ms | 0.81 ms |

Not yet known: how text search behaves on a very common word ("love", "the") against the full 5-million-release
index, where a query matches a large share of all documents. That is a milestone 6 measurement.

## Milestone 5: operations (2026-10-11)

Same 20k-release data set as milestone 3.

**Ingest with compressed copies and compaction**: 3.8 s (was 2.9 s), peak memory 737 MB (was 401 MB).
LMDB store: 266 MB (was 154 MB with plain blobs only). Each blob is now stored plain, zstd and gzip, so the
store grew 73%; compacting the file at the end of ingest took 29 MB off (294 -> 266 MB).

**Response size and speed by encoding** (`oha`, 32 connections, 5 s, release lookups / release-group release lists):

| request | req/s | p50 | p99 | bytes per response |
|---|---:|---:|---:|---:|
| release, plain | 114,200 | 0.26 ms | 0.89 ms | 3.66 KiB |
| release, `Accept-Encoding: zstd` | 116,000 | 0.26 ms | 0.70 ms | 1.55 KiB |
| release, `Accept-Encoding: gzip` | 117,700 | 0.26 ms | 0.58 ms | 1.58 KiB |
| release-group releases list, plain | 114,000 | 0.26 ms | 0.79 ms | 4.60 KiB |
| release-group releases list, zstd | 118,600 | 0.25 ms | 0.67 ms | 1.99 KiB |

Compression costs nothing at request time (the stored bytes are sent as they are) and cuts the bytes by 57%.
As before, the request rate is capped by the load generator on one machine.

**Switching data versions while serving** (the check that matters for "no downtime"): one `oha` run of 12 s
against 32 connections, `current` flipped to the previous version at 3 s and back at 7 s, server polling every
1 s: **1,445,860 requests, all HTTP 200** (the 25 requests `oha` reports as "aborted due to deadline" are the
ones still running when its 12 s ended). The slowest request was 38 ms. The same property is also a test:
`tests/api/ops.test.ts` "the server notices a new current version by itself, without failing requests".

## Milestone 6: hardening and a large real-data run (2026-10-11)

### What was and was not run

**A full-dump run was not done.** The full set is about 25 GB to download and tens of GB to build, and this
machine had 3 to 24 GB of free disk while working. Instead: the real `.tar.xz` dumps (first 1.7 GB of
`release.tar.xz`, 700 MB of `artist.tar.xz`, 260 MB of `release-group.tar.xz`, cut off with HTTP range requests)
were ingested with `--limit 300000`, reading the compressed archives directly like a real run would.
That is **300,000 artists + 300,000 release groups + 300,000 releases**, about 7% of the release dump.
Everything labelled "estimate" below is a straight-line extrapolation from this run, not a measurement.
The full run is for the real server; the commands are at the end of this section.

### Ingest at this scale (Apple Silicon laptop, 10 cores, release build, unlimited input from the real dumps)

| | records/s | wall time for 300k |
|---|---:|---:|
| releases (about 70 KB of JSON each, mostly `relations` we drop) | 3,400 to 3,600 | about 90 s |
| release groups | 10,500 | about 28 s |
| artists | 8,900 | about 34 s |
| **all three, including indexes and compaction** | | **166 s** (240 s of CPU) |

- Memory: the process **resident size peaked at 5.4 GB, but only about 430 MB of that is private memory**
  (`vmmap` physical footprint, peak 432 MB). The rest is the LMDB file being written, which the kernel counts
  as resident but can drop at any time. Plan RAM by the 430 MB figure plus file cache.
- Output: 4.4 GB total: store 4.3 GB, name indexes (FST) 75 MB, Tantivy 64 MB.

`jamhead stats` now shows the store per table:

| table | entries | size | bytes per entry |
|---|---:|---:|---:|
| release (plain) | 300,000 | 1.9 GB | 6,821 |
| release_zstd | 300,000 | 777 MB | 2,714 |
| release_gzip | 300,000 | 795 MB | 2,777 |
| release_group (plain / zstd / gzip) | 300,000 each | 186 / 126 / 128 MB | 648 / 439 / 446 |
| artist (plain / zstd / gzip) | 300,000 each | 161 / 107 / 109 MB | 563 / 373 / 381 |
| artist_rgs, rg_releases (child lists) | 93k, 254k | 7.4 MB, 11 MB | 83, 45 |

### Serving at this scale (`oha`, 32 connections, 4 s each, all responses 200)

| scenario | artist | release group | release |
|---|---:|---:|---:|
| ID lookup, req/s (p50 / p99) | 113,900 (0.27 / 0.55 ms) | 119,600 (0.25 / 0.71) | 114,700 (0.26 / 0.77) |
| child list, req/s | 64,500 (0.43 / 1.6 ms) | 70,600 (0.42 / 1.2) | |
| prefix, 3 to 4 letters | 30,400 (0.76 / 6.5) | 64,600 (0.41 / 2.3) | 92,600 (0.30 / 1.2) |
| prefix, 1 letter (after the fix below) | 20,600 (1.4 / 3.7) | | 17,200 (1.7 / 4.0) |
| fuzzy | 4,700 (6.3 / 16.6) | 9,600 (3.0 / 9.1) | 10,900 (2.7 / 7.4) |
| text, first word of a name | 39,200 (0.77 / 2.0) | 34,800 (0.89 / 2.0) | 41,400 (0.75 / 1.7) |
| text, one of the 10 most common words | 24,000 (1.3 / 3.1) | 13,700 (2.1 / 6.1) | 29,700 (1.0 / 2.9) |

Server resident memory after those runs: 300 to 590 MB (mostly mapped file pages).

### Findings and fixes

1. **One-letter prefix search was slow** (3,262 req/s, p50 9.3 ms on artists). It scanned up to 20,000 matching
   names and sorted them all. Now one- and two-letter queries scan at most 2,000 / 5,000 names and only the best
   few hundred are sorted: **20,600 req/s, p50 1.4 ms** (artists), 17,200 req/s (releases; was 4,568). The price: a
   one-letter query only ranks among the alphabetically first names. Documented in `docs/search.md`.
2. **Fuzzy search on artists is the slowest mode** (4,700 req/s here, p99 17 ms): the automaton walks many more
   names in a 300k-name index than in the 100k one. Fine for a typo fallback, not for per-keystroke use.
3. **Text search on very common words** costs 2x to 3x a normal query but stays at 1 to 2 ms (p50). Not yet known
   at 5 million releases.
4. **Failed ingests leave nothing behind** (the half-built directory is deleted) and a cut-off `.tar.xz` is an
   error, not silent data loss. Both are tests.
5. `panic = "abort"` was removed: a bug that panics in one request now ends that connection, not the server.

### LMDB vs a custom store: decision = keep LMDB

Raw store reads with no HTTP (`cargo test --release store_read_speed -- --ignored --nocapture`, 400,000 random
gets over the 300k-entity data):

| kind (avg blob) | 1 thread | 8 threads |
|---|---:|---:|
| artist (353 B) | 1.06 M gets/s, 946 ns each | 7.0 M gets/s |
| release group (410 B) | 1.06 M gets/s, 947 ns each | 7.1 M gets/s |
| release (4.3 KB) | 0.73 M gets/s, 1.36 us each | 4.7 M gets/s |

A get costs 1 to 1.4 microseconds and scales almost linearly with threads. The whole API answers about
120,000 requests per second on this machine (the same ceiling for small and large payloads, so it looks like the
load generator and loopback network, not the server), which is at most about 8 microseconds of wall-clock per
request, and several cores share that work. So the store is at most one eighth of the time a request takes, and
probably much less; a faster store could not make the API noticeably faster. **Reason to keep LMDB: speed is not a problem.**

The real cost of LMDB here is **disk space**: a release's plain blob averages 4.3 KB but takes 6.8 KB on disk
(values over about 2 KB go into whole 4 KB pages), and the zstd and gzip copies add 68% more. Options if disk
size ever matters, cheapest first: drop the gzip copy (-18%); store only the zstd copy and decompress for plain
requests (about -50%, costs a few microseconds per plain request); train a zstd dictionary on release JSON
(typically a large win on small similar documents; not measured); a custom packed file (about -30% by removing page
rounding; not worth it alone). None needs a change of storage engine, and `EntityStore` keeps that door open.

### Estimates for the full dump (NOT measured: straight-line from the 300k run)

| | per 1 M entities | for 5 M releases + 3 M release groups + 2.5 M artists |
|---|---:|---:|
| store size, releases | 11.7 GB | 58 GB |
| store size, release groups / artists | 1.5 GB / 1.3 GB | 4.4 GB / 3.1 GB |
| ingest time on this laptop, releases | 4.7 min | 24 min |
| ingest time, release groups / artists | 1.6 min / 1.9 min | 5 min / 5 min |
| ingest private memory (rough: (432 MB peak - about 220 MB base) / 300k records) | about 0.7 KB per record held for the name indexes | roughly 3 to 4 GB for the release pass |

So a full update is roughly 35 minutes of ingest plus the download, and about 66 GB per data version; with three
versions on disk while one is being built that is about 200 GB, so plan the volume accordingly (see
`docs/deploy.md`, whose requirements table needs correcting once measured).
Unknowns only a real run can answer: latency with a cold page cache on the server's disk, text search on common
words at 5 M releases, and whether the 4 GB name-table peak is comfortable on the target machine.

To measure it on a machine with about 250 GB free:

```
time jh update --data /data --dumps /data/dumps
jh stats  --data /data
jh verify --data /data --deep
bun run bench -- --data /data --entity release       # from a machine with Bun; or oha, see the top of this file
```

## First full-catalog attempt on a NAS: ingest slows down (2026-10-12)

**Measured** on a Synology DS923+ (4 GB RAM, spinning disks, shared with other users and services), Docker image
0.1.1, real MusicBrainz dump 20261010-001001, container capped at 3 GB. The three dumps downloaded and verified
(about 25 GB); then the release pass of `update` was timed from its own log lines:

| records ingested | log time | average since the start |
|---:|---|---:|
| 100,517 | 17:43 | 958 / s |
| 200,295 | 17:46 | 807 / s |
| 300,443 | 18:11 | 170 / s |

The third batch of 100,000 took 25 minutes (about 67 / s). While it slowed down, the container's CPU fell from
100 to 250% to about 10% and its memory stayed flat at about 1.3 GB, so it was waiting, not computing. The run was
stopped: at that speed, and falling, the full catalog would not finish in any useful time.

**Cause (a likely explanation, not proven on the NAS):** ingest wrote each record into LMDB in dump order, but
keys are random ids, so every insert landed at a random place in a file that grows to tens of GB. While the file
fits in the page cache that is fast (the laptop runs above); once it does not, each insert costs a disk seek.
`NO_SYNC` was already on, so it was not flushing. The disks are shared with other machines, so some of the
slowdown may come from them, but a 14x drop within 30 minutes matches the growth effect.

**Change:** ingest now spools each record into one of 256 temporary files (by the first byte of its id, which is
random, so they come out about equal) and writes them into LMDB one file at a time, sorted, so the database is
filled front to back (`src/store.rs`). Peak extra disk while ingesting is the uncompressed JSON of the kind being
written, roughly 40% of the finished store (estimate from the plain / zstd / gzip table sizes), freed as each file is
written. The finished data is identical in layout.

**Checked on a laptop (SSD, fits in RAM, so it cannot show the NAS problem):** 300,000 real release groups from the
real dump ingested in 14 s including compaction; `jamhead verify --deep` 26 checks, 0 failed; all three blob tables
hold 300,000 entries. All Rust and Bun tests pass unchanged.

**Not yet proven:** that the NAS now keeps its speed past 300,000 records. To check, rerun on the NAS
(`ingest` or `update`, smaller catalog first) and watch whether the "records so far / s" figure in the log stays
roughly flat. Record the result here.
