# Name search

Per entity there is an FST (`fst/<entity>.fst`) plus a postings file (`fst/<entity>.ids`).

- **Key**: a normalized name (lowercase, accents removed, punctuation turned into single spaces, so
  `Björk` -> `bjork`, `AC/DC` -> `ac dc`). An artist is indexed under its name, sort name and every alias.
- **Value**: `(score << 40) | offset`. `offset` points into the postings file, where every entity that has
  that exact name is listed, together with the display name and a short detail string (the
  disambiguation for artists). `score` is the number of children (release groups per artist), so popular
  artists come first.
- **Modes**
  - `prefix`: names starting with the query. Ranked by key score, then shorter names first.
  - `fuzzy`: names within edit distance 1 (3-5 letters) or 2 (6+ letters); a swapped pair counts as one edit.
    One- and two-letter queries only match exactly. Ranked by distance, then score.
- Results are deduplicated by entity and return `id`, `name` and `detail`. Fetch the blob by `id` for the rest.

Known ceilings (also commented in `src/search.rs`):

- At most 20,000 matching names are examined per query.
- Ranking is per matching name, not per entity: a low-scoring artist who shares an exact name with a
  popular one is listed right after it, before other names that have a higher score than he does.
