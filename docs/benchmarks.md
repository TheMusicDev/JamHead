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
