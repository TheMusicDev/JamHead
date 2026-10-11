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
  alphabetically first names. That is the cause of the p99 above and a known ceiling.

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
