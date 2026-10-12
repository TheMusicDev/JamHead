# Roadmap

What is left after milestones 1 to 6 (see [SPEC.md](../SPEC.md)), in the order I suggest doing it. Each item has a GitHub issue; this page is the order and the why. Keep this page
current: tick things off here when they land, and add new ideas at the bottom instead of leaving them in chat.
Items that change the storage design need the owner's approval before work starts.

## Now: get it onto the NAS without a terminal

| # | What | Why this position | Status |
|---|---|---|---|
| 1 | **Publish versioned images to GHCR on a version tag** ([#2](https://github.com/TheMusicDev/JamHead/issues/2), PR #3) | The NAS (or any host) can then just point at `ghcr.io/themusicdev/jamhead:<version>`. Nothing else is deployable without it. | workflow and docs merged; #2 is closed, but only a real tag (#4) proves it works |
| 2 | **Make the first tag, check the package is public, pull it on the NAS** ([#4](https://github.com/TheMusicDev/JamHead/issues/4)) | Proves #1 end to end, including the amd64 image on real hardware (so far it was only built, never load-tested). | next |
| 3 | **A `sample` option so a small data set needs no terminal** ([#5](https://github.com/TheMusicDev/JamHead/issues/5)). Fetch only the start of each dump (HTTP range request), build a linked-up data set of N releases plus the release groups and artists they belong to, about 100 MB | Today a small trial needs hand-typed `curl` commands. In Container Manager the only other choice is a 25 GB download. Without this the NAS trial would have to start with the full catalog. | not started |

## Then: measure on the NAS and fix what the numbers show

| # | What | Why this position | Status |
|---|---|---|---|
| 4 | **NAS ramp-up** ([#6](https://github.com/TheMusicDev/JamHead/issues/6)): small sample, then 300,000 per kind, then the full catalog. Record memory, disk speed and time in `docs/benchmarks.md`; correct the estimates in `docs/deploy.md` | Every full-size number in the docs is an estimate. The stock DS923+ (4 GB RAM, spinning disks) is the likely worst case. | needs #2 and #3 |
| 5 | **Lower ingest memory** ([#7](https://github.com/TheMusicDev/JamHead/issues/7)): write the search names to a temp file instead of keeping them all in RAM | Expected to be needed for 4 GB. Do it only if #4 shows memory is the limit. | waiting on #4 |
| 6 | **HDD-friendly ingest** ([#8](https://github.com/TheMusicDev/JamHead/issues/8)): sort records by key before writing, so the disk is written in order instead of at random spots | Expected to matter on spinning disks. Changes how ingest writes into LMDB (not the file format): ask the owner first. | waiting on #4, needs approval |

## Housekeeping (small, any time)

- [#9](https://github.com/TheMusicDev/JamHead/issues/9): pin the Rust toolchain in `.github/workflows/ci.yml`. CI follows "stable", so a new Rust release can add a lint
  and fail CI on code that passed yesterday (this already happened once). The cost is bumping the pin by hand.
- [#10](https://github.com/TheMusicDev/JamHead/issues/10): move `actions/checkout@v4` (and the other actions) to versions that do not use Node 20, which GitHub is retiring.
- [#11](https://github.com/TheMusicDev/JamHead/issues/11): commit convention and changelog. Commits today are plain sentences, not conventional commits, and there is no
  changelog. Suggestion: adopt `feat:` / `fix:` / `docs:` prefixes now, and let the GitHub Release notes from #1
  (generated from merged PRs) be the changelog until a tool is worth adding. Needs the owner's yes before it goes
  into `CLAUDE.md`.

## Decisions waiting for the owner

- [#12](https://github.com/TheMusicDev/JamHead/issues/12): keep or drop the gzip copies of every blob (dropping saves about 18% of disk; clients without zstd get plain JSON).
- [#13](https://github.com/TheMusicDev/JamHead/issues/13): Alpine or `scratch` for the final image (a shell for `docker exec` costs about 8 MB; today it is Alpine).
- [#14](https://github.com/TheMusicDev/JamHead/issues/14): keep the MIT license that was chosen without asking.

## Known gaps, not planned

- Dump PGP signatures (`.asc`) are not verified; SHA256SUMS are.
- No authentication or rate limiting on the API; put a reverse proxy in front.
- No Brotli.
- Native binaries for macOS, Windows and Linux on the GitHub Release (out of scope of #2): the code is only tested
  on Linux and the `data/current` symlink may not work on Windows.
