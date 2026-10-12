# Fixture dump

`dump/mbdump/{artist,release-group,release}` is a small extract of the real MusicBrainz JSON dump of
2026-10-10 (CC0 core data), in the same line-per-entity format the real dumps use.

- 176 artists, 100 release groups, about 100 releases. `relations` were emptied to keep the files small.
- Artists and groups that releases point to were taken from the releases' own embedded copies, so the
  links between the three files are consistent. A few more real artists with non-ASCII names, aliases
  and tags were added so the search and supplementary-data tests have something to find.
- **Hand-made rows**: one artist (`00000000-0000-0000-0000-000000001234`) copies another artist's name so
  tests can check that two entities with the same name are both searchable, and releases with
  "fixture copy" in the disambiguation are copies of real releases with new IDs, other dates (one with no
  date) and country `XW`, so release groups have several releases to order.

The Bun tests under `tests/api/` read these files to work out what the API must return, so adding rows
here is safe as long as the three files stay consistent with each other.
