# Roadmap

What is left after milestones 1 to 6 (see [SPEC.md](../SPEC.md)), in the order I suggest doing it. Keep this page
current: tick things off here when they land, and add new ideas at the bottom instead of leaving them in chat.
Items that change the storage design need the owner's approval before work starts.

## Done

- Milestones 1 to 6 and `--keep-versions` (PR #1).
- Versioned images on GHCR from a version tag (#2, PR #3). Published: `0.1.0` to `0.1.3` (amd64 and arm64).
- Synology steps written from a real NAS (PR #16).
- `0.1.1`: big downloads were cut off after 2 minutes; now no time limit, retry and resume (PR #17).
- `0.1.2`: ingest writes records in id order (for spinning disks, [#8](https://github.com/TheMusicDev/JamHead/issues/8)) and prints plain-English progress with percent and time left ([#19](https://github.com/TheMusicDev/JamHead/issues/19), PR #20).
- `0.1.3` (PRs #21 and #22, tagged and published): fixes "Too many open files" on macOS (the 0.1.2 writer held up to 768 files open; macOS allows 256), times over a minute show seconds, and every log line starts with the date and time.

## Now: finish measuring the full catalog on the NAS

The full catalog was built **on the NAS itself** (image 0.1.3, 1 h 54 min, about 72 GB) and is being served from
there; numbers are in `docs/benchmarks.md`. That made the old plan (build on a Mac, copy to the NAS) unnecessary.

| # | What | Status |
|---|---|---|
| 1 | **Record the full-run numbers and fix the estimates in `docs/deploy.md`** ([#6](https://github.com/TheMusicDev/JamHead/issues/6)) | done for time, size and speed; still missing: peak memory, peak disk during the build, download time |
| 2 | **Run `verify --deep` on the NAS data** | not run yet; note `data.mdb` (70 GB) has a size check but no checksum |
| 3 | **Confirm the container health check passes on this data** ([#4](https://github.com/TheMusicDev/JamHead/issues/4)) | `/healthz` answers `ok`; the Container Manager health status was not looked at |
| 4 | **Load test the NAS API** (`oha`, cold and warm cache) | not run; single requests suggest the first request for a record is slower (up to 0.4 s) on cold disks |
| 5 | **Can the NAS build updates itself?** ([#8](https://github.com/TheMusicDev/JamHead/issues/8), [#7](https://github.com/TheMusicDev/JamHead/issues/7)) | yes: sorted writes held their speed and the run fit in a 3 GB cap. #8 can close; keep #7 open only if peak memory turns out to be tight |

## Next

- A `sample` option so a small data set needs no terminal ([#5](https://github.com/TheMusicDev/JamHead/issues/5)).
- Useful JSON at `/` instead of an empty 404 ([#18](https://github.com/TheMusicDev/JamHead/issues/18)).
- Lower ingest memory, only if the NAS numbers show memory is the limit ([#7](https://github.com/TheMusicDev/JamHead/issues/7)).
- Prefix search matches aliases, so "love" returns Calvin Harris (alias "Love Regenerator"). Decide whether alias
  matches should rank below name matches.
- Unexplained: a full ingest on a Mac (external exFAT drive) crashed with a segmentation fault inside LMDB while
  writing; it could not be reproduced with synthetic or real replayed data and did not happen on the NAS. Only
  worth chasing if someone needs to build on a Mac.

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
