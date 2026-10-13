# Roadmap

What is left after milestones 1 to 6 (see [SPEC.md](../SPEC.md)), in the order I suggest doing it. Keep this page
current: tick things off here when they land, and add new ideas at the bottom instead of leaving them in chat.
Items that change the storage design need the owner's approval before work starts.

## Done

- Milestones 1 to 6 and `--keep-versions` (PR #1).
- Versioned images on GHCR from a version tag (#2, PR #3). Published: `0.1.0`, `0.1.1`, `0.1.2` (amd64 and arm64).
- Synology steps written from a real NAS (PR #16).
- `0.1.1`: big downloads were cut off after 2 minutes; now no time limit, retry and resume (PR #17).
- `0.1.2`: ingest writes records in id order (for spinning disks, [#8](https://github.com/TheMusicDev/JamHead/issues/8)) and prints plain-English progress with percent and time left ([#19](https://github.com/TheMusicDev/JamHead/issues/19), PR #20).
- `0.1.3` (once tagged): fixes "Too many open files" on macOS (the 0.1.2 writer held up to 768 files open; macOS allows 256), and times over a minute show seconds.

## Now: build the full catalog and get it onto the NAS

| # | What | Why this position | Status |
|---|---|---|---|
| 1 | **Build the full catalog on a Mac with a fast external SSD** (`update --dump 20261010-001001`), then `verify` | The stock NAS slowed to a crawl building it itself (see `docs/benchmarks.md`). A Mac with plenty of RAM and SSD does not have that problem. | in progress: downloads done and checked, first ingest hit the open-files bug fixed in 0.1.3 |
| 2 | **Copy the finished version folder to the NAS and run `verify` there** | Proves that data built on a Mac (Apple silicon, arm64) opens on the NAS (x86, amd64). Untested: try it with a small 300,000 build first. Also work out how the NAS points `current` at the copied folder. | after #1 |
| 3 | **Pull the image on the NAS and start `serve` on that data** ([#4](https://github.com/TheMusicDev/JamHead/issues/4)) | Pull and start were proven; the healthcheck on real data is not. | after #2 and the `v0.1.3` tag |
| 4 | **Record real numbers** (Mac build time and size, NAS memory) in `docs/benchmarks.md` and correct the estimates in `docs/deploy.md` ([#6](https://github.com/TheMusicDev/JamHead/issues/6)) | Every full-size number in the docs is still an estimate. | after #1 to #3 |
| 5 | **Decide whether the NAS can build updates itself** ([#8](https://github.com/TheMusicDev/JamHead/issues/8), [#7](https://github.com/TheMusicDev/JamHead/issues/7)) | The 0.1.2 writer should fix the slowdown, but it has only been tested where everything fits in RAM. Run a `--limit 300000` update on the NAS with 0.1.3 and watch the speed stay flat. Close #8 only if it does. | after the `v0.1.3` tag |

## Next

- Tag `v0.1.3`, then update the `0.1.0` / `0.1.1` pins in `docs/deploy.md` to it.
- A `sample` option so a small data set needs no terminal ([#5](https://github.com/TheMusicDev/JamHead/issues/5)).
- Useful JSON at `/` instead of an empty 404 ([#18](https://github.com/TheMusicDev/JamHead/issues/18)).
- Lower ingest memory, only if the NAS numbers show memory is the limit ([#7](https://github.com/TheMusicDev/JamHead/issues/7)).

## Housekeeping (small, any time)

- Pin the Rust toolchain in `.github/workflows/ci.yml`. CI follows "stable", so a new Rust release can add a lint
  and fail CI on code that passed yesterday (this already happened once). The cost is bumping the pin by hand.
- Move `actions/checkout@v4` (and the other actions) to versions that do not use Node 20, which GitHub is retiring.
- Commit convention and changelog. Commits today are plain sentences, not conventional commits, and there is no
  changelog. Suggestion: adopt `feat:` / `fix:` / `docs:` prefixes now, and let the GitHub Release notes from #1
  (generated from merged PRs) be the changelog until a tool is worth adding. Needs the owner's yes before it goes
  into `CLAUDE.md`.

## Decisions waiting for the owner

- Keep or drop the gzip copies of every blob (dropping saves about 18% of disk; clients without zstd get plain JSON).
- Alpine or `scratch` for the final image (a shell for `docker exec` costs about 8 MB; today it is Alpine).
- Keep the MIT license that was chosen without asking.

## Known gaps, not planned

- Dump PGP signatures (`.asc`) are not verified; SHA256SUMS are.
- No authentication or rate limiting on the API; put a reverse proxy in front.
- No Brotli.
- Native binaries for macOS, Windows and Linux on the GitHub Release (out of scope of #2): the code is only tested
  on Linux and the `data/current` symlink may not work on Windows.
