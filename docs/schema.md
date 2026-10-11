# Schema

Decided in milestone 1 by sampling the MusicBrainz JSON dump of 2026-10-10
(`data.metabrainz.org/pub/musicbrainz/data/json-dumps/20261010-001001/`).

## Dump format (what we found)

- Files are `<entity>.tar.xz`. Each contains `mbdump/<entity>`, one JSON object per line.
- Sizes (compressed): artist 2 GB, release-group 1 GB, **release 22 GB**.
- Lines are big because every entity embeds its relationships. Sampled line sizes:
  artist median 18 KB (max 5 MB), release-group about 6 KB, release about 70 KB.
  We drop `relations` entirely, so our blobs are much smaller than the dump lines.
- Empty strings show up where `null` is meant (for example `"first-release-date": ""`).
  The ingest turns empty strings into "field absent".
- Ingest streams: it reads the `.tar.xz` directly (or an already extracted file) one line at a time.

## Conventions

- Field names are the MusicBrainz names (`sort-name`, `life-span`, ...), so MusicBrainz clients feel at home.
- IDs are MBIDs. Internally 16 raw bytes, in the API hyphenated lowercase strings.
- A field with no value is left out (no `null`, no `""`, no `[]`).
- **Core fields only by default.** Tags, genres, ratings and annotations are CC BY-NC-SA and are only
  written when ingest runs with `--include-supplementary`. Everything else is CC0.

## Artist `GET /v1/artist/{mbid}`

| field | notes |
|---|---|
| `id`, `name`, `sort-name` | always present |
| `disambiguation`, `type`, `type-id`, `gender`, `gender-id`, `country` | when set |
| `area`, `begin-area`, `end-area` | `{id, name, sort-name, iso-3166-1-codes?}` |
| `life-span` | `{begin?, end?, ended?}` |
| `aliases` | `[{name, sort-name, type?, locale?, primary?, begin?, end?, ended?}]` |
| `isnis`, `ipis` | string lists |
| `release-group-count` | computed at ingest |
| `tags`, `genres`, `rating`, `annotation` | **supplementary only** |

## Release group `GET /v1/release-group/{mbid}`

`id`, `title`, `disambiguation`, `primary-type`, `primary-type-id`, `secondary-types`,
`secondary-type-ids`, `first-release-date`, `aliases`, `release-count` (computed),
`artist-credit` = `[{name, joinphrase?, artist: {id, name, sort-name, disambiguation?}}]`.
Supplementary: `tags`, `genres`, `rating`, `annotation`.

## Release `GET /v1/release/{mbid}`

`id`, `title`, `disambiguation`, `status`, `status-id`, `date`, `country`, `barcode`, `asin`,
`packaging`, `packaging-id`, `quality`, `text-representation {language?, script?}`,
`artist-credit` (same shape as above),
`release-group` = `{id, title, primary-type?, secondary-types?, first-release-date?}`,
`release-events` = `[{date?, area?: {id, name, iso-3166-1-codes?}}]`,
`label-info` = `[{catalog-number?, label?: {id, name}}]`,
`cover-art-archive` = `{artwork, count, front, back, darkened}`,
`media` = `[{position, title?, format?, format-id?, track-count, tracks: [...]}]` where a track is
`{id, position, number, title, length?, recording: {id, title}, artist-credit?}`.
A track's `artist-credit` is only written when it differs from the release's.
Supplementary: `tags`, `genres`, `annotation`.

Foreign IDs kept for future joins (so we never re-ingest to add them): label IDs, recording IDs,
area IDs.

Dropped on purpose: `relations`, `data-tracks`, `discs`, release `aliases`, recording details beyond id/title.

## Relationships

SPEC says "embedded ID lists". We store them in their own LMDB tables instead and embed only the counts.
Reason: a single artist (for example Various Artists) can own hundreds of thousands of release groups,
which would make that one blob enormous and every artist lookup slow. A separate table also lets the list
endpoints page without parsing JSON.

| table | key | value |
|---|---|---|
| `artist_rgs` | artist MBID | packed 16-byte release-group MBIDs, sorted by (first-release-date, MBID) |
| `rg_releases` | release-group MBID | packed 16-byte release MBIDs, sorted by (date, MBID) |

An artist is linked to a release group through the release group's `artist-credit`.
A release is linked to its group through `release-group.id`.
Entries pointing at records that were not ingested (`--limit` samples) are skipped when the list is read.

## Data directory

See SPEC.md. `meta.json` fields: `schema_version`, `data_version`, `dump_date`, `created_at`,
`supplementary`, `counts {artist, release_group, release}`, `files` (size and sha256 of the small files).
