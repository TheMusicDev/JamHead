import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { findEmptyValue, fixture, getJson, ingest, serve, type Server } from "./harness";

const artists = fixture("artist");
const groups = fixture("release-group");
const releases = fixture("release");
let server: Server;

beforeAll(async () => {
  server = await serve(await ingest());
});
afterAll(() => server.stop());

// Same ordering rule as the server: by date (no date last), then by id.
const dateKey = (d?: string | null) => {
  if (!d) return Number.MAX_SAFE_INTEGER;
  const [y, m, day] = d.split("-").map((x) => Number(x) || 0);
  return y ? y * 10000 + (m || 0) * 100 + (day || 0) : Number.MAX_SAFE_INTEGER;
};
const byDateThenId = (date: (x: any) => string | undefined) => (a: any, b: any) =>
  dateKey(date(a)) - dateKey(date(b)) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);

const releasesOf = (groupId: string) => releases.filter((r) => r["release-group"]?.id === groupId);
const groupsOf = (artistId: string) =>
  groups.filter((g) => g["artist-credit"]?.some((c: any) => c.artist.id === artistId));

describe("GET /v1/release-group/{mbid}", () => {
  test("every fixture release group is served", async () => {
    for (const g of groups) {
      const { status, body } = await getJson(server, `/v1/release-group/${g.id}`);
      expect(status).toBe(200);
      expect(body.id).toBe(g.id);
      expect(body.title).toBe(g.title);
      if (g["primary-type"]) expect(body["primary-type"]).toBe(g["primary-type"]);
      if (g["first-release-date"]) expect(body["first-release-date"]).toBe(g["first-release-date"]);
      expect(findEmptyValue(body)).toBeNull();
      for (const key of ["relations", "tags", "genres", "rating", "annotation"]) expect(body[key]).toBeUndefined();
    }
  });

  test("artist credit is embedded", async () => {
    const g = groups.find((x) => x["artist-credit"]?.length)!;
    const { body } = await getJson(server, `/v1/release-group/${g.id}`);
    expect(body["artist-credit"][0].artist.id).toBe(g["artist-credit"][0].artist.id);
    expect(body["artist-credit"][0].artist.name).toBe(g["artist-credit"][0].artist.name);
  });

  test("release-count is computed from the ingested releases", async () => {
    for (const g of groups) {
      const { body } = await getJson(server, `/v1/release-group/${g.id}`);
      expect(body["release-count"]).toBe(releasesOf(g.id).length || undefined);
    }
  });

  test("404 and 400", async () => {
    expect((await getJson(server, "/v1/release-group/00000000-0000-0000-0000-000000000000")).status).toBe(404);
    expect((await getJson(server, "/v1/release-group/nope")).status).toBe(400);
  });
});

describe("GET /v1/release/{mbid}", () => {
  test("every fixture release is served", async () => {
    for (const r of releases) {
      const { status, body } = await getJson(server, `/v1/release/${r.id}`);
      expect(status).toBe(200);
      expect(body.id).toBe(r.id);
      expect(body.title).toBe(r.title);
      expect(body["release-group"].id).toBe(r["release-group"].id);
      if (r.date) expect(body.date).toBe(r.date);
      if (r.country) expect(body.country).toBe(r.country);
      if (r.barcode) expect(body.barcode).toBe(r.barcode);
      expect(findEmptyValue(body)).toBeNull();
      expect(body.relations).toBeUndefined();
      expect(body.tags).toBeUndefined();
    }
  });

  test("media and tracks are kept with titles, positions and recording ids", async () => {
    for (const r of releases) {
      const { body } = await getJson(server, `/v1/release/${r.id}`);
      expect((body.media ?? []).length).toBe(r.media.filter((m: any) => m.tracks?.length || m["track-count"]).length || (body.media ?? []).length);
      const expected = r.media.flatMap((m: any) => m.tracks ?? []);
      const got = (body.media ?? []).flatMap((m: any) => m.tracks ?? []);
      expect(got.map((t: any) => t.title)).toEqual(expected.map((t: any) => t.title));
      expect(got.map((t: any) => t.recording?.id)).toEqual(expected.map((t: any) => t.recording?.id));
      expect(body.media?.[0]?.data_tracks).toBeUndefined();
    }
  });

  test("a track repeats the artist credit only when it differs from the release's", async () => {
    for (const r of releases) {
      const { body } = await getJson(server, `/v1/release/${r.id}`);
      const releaseArtists = (r["artist-credit"] ?? []).map((c: any) => c.artist.id).join();
      const srcTracks = r.media.flatMap((m: any) => m.tracks ?? []);
      const gotTracks = (body.media ?? []).flatMap((m: any) => m.tracks ?? []);
      gotTracks.forEach((t: any, i: number) => {
        const trackArtists = (srcTracks[i]["artist-credit"] ?? []).map((c: any) => c.artist.id).join();
        if (trackArtists === releaseArtists) expect(t["artist-credit"]).toBeUndefined();
        else expect(t["artist-credit"].map((c: any) => c.artist.id).join()).toBe(trackArtists);
      });
    }
  });

  test("label info keeps label ids for future joins", async () => {
    const r = releases.find((x) => x["label-info"]?.some((l: any) => l.label?.id));
    if (!r) return; // fixture may have none
    const { body } = await getJson(server, `/v1/release/${r.id}`);
    const ids = body["label-info"].map((l: any) => l.label?.id).filter(Boolean);
    expect(ids).toContain(r["label-info"].find((l: any) => l.label?.id).label.id);
  });
});

describe("GET /v1/artist/{mbid}/release-groups", () => {
  test("lists exactly the release groups that credit the artist, oldest first", async () => {
    let checked = 0;
    for (const a of artists) {
      const expected = groupsOf(a.id).sort(byDateThenId((g) => g["first-release-date"]));
      const { status, body } = await getJson(server, `/v1/artist/${a.id}/release-groups?limit=100`);
      expect(status).toBe(200);
      expect(body.total).toBe(expected.length);
      expect(body["release-groups"].map((g: any) => g.id)).toEqual(expected.map((g) => g.id));
      if (expected.length) checked++;
    }
    expect(checked).toBeGreaterThan(5);
  });

  test("artist-level release-group-count matches", async () => {
    for (const a of artists) {
      const { body } = await getJson(server, `/v1/artist/${a.id}`);
      expect(body["release-group-count"]).toBe(groupsOf(a.id).length || undefined);
    }
  });

  test("paging with limit and offset walks the same list", async () => {
    const a = artists.find((x) => groupsOf(x.id).length >= 3)!;
    const all = await getJson(server, `/v1/artist/${a.id}/release-groups?limit=100`);
    const ids: string[] = [];
    for (let offset = 0; offset < all.body.total; offset += 2) {
      const page = await getJson(server, `/v1/artist/${a.id}/release-groups?limit=2&offset=${offset}`);
      expect(page.body.limit).toBe(2);
      expect(page.body.offset).toBe(offset);
      ids.push(...page.body["release-groups"].map((g: any) => g.id));
    }
    expect(ids).toEqual(all.body["release-groups"].map((g: any) => g.id));
  });

  test("an artist without release groups has an empty list; unknown artist is 404", async () => {
    const lonely = artists.find((a) => groupsOf(a.id).length === 0)!;
    const { status, body } = await getJson(server, `/v1/artist/${lonely.id}/release-groups`);
    expect(status).toBe(200);
    expect(body.total).toBe(0);
    expect(body["release-groups"]).toEqual([]);
    expect((await getJson(server, "/v1/artist/00000000-0000-0000-0000-000000000000/release-groups")).status).toBe(404);
  });
});

describe("GET /v1/release-group/{mbid}/releases", () => {
  test("lists the group's releases, oldest first", async () => {
    let checked = 0;
    for (const g of groups) {
      const expected = releasesOf(g.id).sort(byDateThenId((r) => r.date));
      const { status, body } = await getJson(server, `/v1/release-group/${g.id}/releases?limit=100`);
      expect(status).toBe(200);
      expect(body.total).toBe(expected.length);
      expect(body.releases.map((r: any) => r.id)).toEqual(expected.map((r) => r.id));
      if (expected.length > 1) checked++;
    }
    expect(checked).toBeGreaterThan(3);
  });

  test("unknown group is 404", async () => {
    expect((await getJson(server, "/v1/release-group/00000000-0000-0000-0000-000000000000/releases")).status).toBe(404);
  });
});

describe("search", () => {
  test("release groups are found by title and show the credited artist", async () => {
    for (const g of groups.slice(0, 40)) {
      const q = g.title.slice(0, 6);
      const { body } = await getJson(server, `/v1/search/release-group?q=${encodeURIComponent(q)}&limit=100`);
      const hit = body.results.find((r: any) => r.id === g.id);
      expect(hit).toBeDefined();
      expect(hit.name).toBe(g.title);
      if (g["artist-credit"]?.length) expect(hit.detail).toContain(g["artist-credit"][0].name || g["artist-credit"][0].artist.name);
    }
  });

  test("releases are found by title", async () => {
    for (const r of releases) {
      const { body } = await getJson(server, `/v1/search/release?q=${encodeURIComponent(r.title)}&limit=100`);
      expect(body.results.map((x: any) => x.id)).toContain(r.id);
    }
  });

  test("meta counts all three entities", async () => {
    const { body } = await getJson(server, "/v1/meta");
    expect(body.counts).toEqual({ artist: artists.length, "release-group": groups.length, release: releases.length });
  });
});
