# CLAUDE.md: JamHead

Read `SPEC.md` first. It is the source of truth for scope, stack and milestones.

## What this is

JamHead is a read-optimized, MusicBrainz-style API (artists, release groups, releases) in Rust. An ingest CLI builds a read-only data directory (LMDB blobs + FST + Tantivy) from MusicBrainz JSON dumps; the `serve` command answers requests from it. Tagline: tiny music brains.

## Working rules

- The owner does not read Rust. Explain non-obvious decisions in plain English in commit messages and PR descriptions, keep the code straightforward, and prefer boring, well-known crates.
- Do not change the architecture (LMDB blobs, FST search, Tantivy, versioned data dirs with atomic swap) without asking the owner first. Raise it as a question with a recommendation.
- Work milestone by milestone as listed in `SPEC.md`. Finish and verify one before starting the next.
- Never claim something is fast or correct without a test or benchmark. Add results to `docs/benchmarks.md`.
- Correctness lives in the Bun black-box tests under `tests/api/`. Do not weaken or delete a test to make it pass. If a test is wrong, say why and ask.
- Ingest must stream. Never load a whole dump file into memory.
- Never modify a live data directory. Always build a new versioned directory and flip the `current` symlink.
- Keep storage behind the `EntityStore` trait.
- Respect data licensing: core fields only by default, supplementary (CC BY-NC-SA) fields behind an explicit flag.

## Commands (keep this section current)

```
cargo build --release
cargo test
cargo fmt --check && cargo clippy -- -D warnings
cargo run --release -- ingest --input <dumps> --out ./data --limit 50000
cargo run --release -- serve --data ./data
bun test tests/api        # black-box API tests against a running server
bun run bench             # load benchmarks
docker buildx build --platform linux/arm64,linux/amd64 .
```

## Definition of done for any change

1. `cargo fmt`, `clippy`, and `cargo test` pass.
2. Bun API tests pass against a freshly ingested sample.
3. If the change touches the read path or search, benchmarks were rerun and recorded.
4. `SPEC.md` / `docs/` updated if behavior or layout changed.

## First task

Do the "Repo setup" step in `SPEC.md` (create `TheMusicDev/JamHead` as a public repo with `gh`, add these files, push), then start milestone 1. If `gh` fails on auth or permissions, stop and ask the owner.