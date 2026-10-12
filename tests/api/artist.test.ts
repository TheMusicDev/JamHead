import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { findEmptyValue, fixture, getJson, ingest, serve, type Server } from "./harness";

const artists = fixture("artist");
let server: Server;

beforeAll(async () => {
  server = await serve(await ingest());
});
afterAll(() => server.stop());

describe("operational endpoints", () => {
  test("healthz", async () => {
    const res = await fetch(server.url + "/healthz");
    expect(res.status).toBe(200);
  });

  test("meta reports version and counts", async () => {
    const { status, body } = await getJson(server, "/v1/meta");
    expect(status).toBe(200);
    expect(body.counts.artist).toBe(artists.length);
    expect(body.data_version).toMatch(/^v\d{4}-\d{2}-\d{2}T\d{6}$/);
    expect(body.supplementary).toBe(false);
  });
});

describe("GET /v1/artist/{mbid}", () => {
  test("every fixture artist is served with its id and name", async () => {
    for (const a of artists) {
      const { status, body, headers } = await getJson(server, `/v1/artist/${a.id}`);
      expect(status).toBe(200);
      expect(headers.get("content-type")).toContain("application/json");
      expect(body.id).toBe(a.id);
      expect(body.name).toBe(a.name);
      if (a.type) expect(body.type).toBe(a.type);
      if (a.country) expect(body.country).toBe(a.country);
    }
  });

  test("uppercase MBIDs work", async () => {
    const a = artists[0];
    const { status, body } = await getJson(server, `/v1/artist/${a.id.toUpperCase()}`);
    expect(status).toBe(200);
    expect(body.id).toBe(a.id);
  });

  test("relations and supplementary fields are not served by default", async () => {
    for (const a of artists) {
      const { body } = await getJson(server, `/v1/artist/${a.id}`);
      for (const key of ["relations", "tags", "genres", "rating", "annotation"]) {
        expect(body[key]).toBeUndefined();
      }
    }
  });

  test("no null, empty string or empty list values", async () => {
    for (const a of artists) {
      const { body } = await getJson(server, `/v1/artist/${a.id}`);
      expect(findEmptyValue(body)).toBeNull();
    }
  });

  test("aliases are kept", async () => {
    const withAliases = artists.find((a) => a.aliases?.length);
    expect(withAliases).toBeDefined();
    const { body } = await getJson(server, `/v1/artist/${withAliases.id}`);
    expect(body.aliases.map((x: any) => x.name)).toContain(withAliases.aliases[0].name);
  });

  test("unknown id is 404, malformed id is 400", async () => {
    expect((await getJson(server, "/v1/artist/00000000-0000-0000-0000-000000000000")).status).toBe(404);
    expect((await getJson(server, "/v1/artist/not-an-id")).status).toBe(400);
  });
});

describe("GET /v1/search/artist", () => {
  test("prefix search finds each artist by the start of its name", async () => {
    for (const a of artists) {
      const q = a.name.slice(0, 5);
      const { status, body } = await getJson(server, `/v1/search/artist?q=${encodeURIComponent(q)}&limit=100`);
      expect(status).toBe(200);
      expect(body.results.map((r: any) => r.id)).toContain(a.id);
    }
  });

  test("results carry id and name, and disambiguation as detail", async () => {
    const a = artists.find((x) => x.disambiguation);
    const { body } = await getJson(server, `/v1/search/artist?q=${encodeURIComponent(a.name)}&limit=100`);
    const hit = body.results.find((r: any) => r.id === a.id);
    expect(hit.name).toBe(a.name);
    expect(hit.detail).toBe(a.disambiguation);
  });

  test("prefix search ignores case and accents", async () => {
    const accented = artists.find((a) => a.name !== a.name.normalize("NFKD").replace(/[̀-ͯ]/g, ""));
    expect(accented).toBeDefined();
    const folded = accented.name.normalize("NFKD").replace(/[̀-ͯ]/g, "").toUpperCase();
    const { body } = await getJson(server, `/v1/search/artist?q=${encodeURIComponent(folded)}&limit=100`);
    expect(body.results.map((r: any) => r.id)).toContain(accented.id);
  });

  test("two artists with the same name are both found", async () => {
    const byName = new Map<string, string[]>();
    for (const a of artists) byName.set(a.name, [...(byName.get(a.name) ?? []), a.id]);
    const [name, ids] = [...byName.entries()].find(([, v]) => v.length > 1)!;
    const { body } = await getJson(server, `/v1/search/artist?q=${encodeURIComponent(name)}&limit=100`);
    for (const id of ids) expect(body.results.map((r: any) => r.id)).toContain(id);
  });

  test("limit is respected and results are unique", async () => {
    const { body } = await getJson(server, "/v1/search/artist?q=a&limit=3");
    expect(body.results.length).toBeLessThanOrEqual(3);
    const ids = body.results.map((r: any) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  test("fuzzy mode tolerates a typo", async () => {
    const a = artists.find((x) => x.name.length >= 8 && /^[a-z ]+$/i.test(x.name))!;
    const typo = a.name.slice(0, 3) + a.name[4] + a.name[3] + a.name.slice(5); // swap two letters
    const { status, body } = await getJson(server, `/v1/search/artist?mode=fuzzy&q=${encodeURIComponent(typo)}&limit=100`);
    expect(status).toBe(200);
    expect(body.results.map((r: any) => r.id)).toContain(a.id);
  });

  test("bad requests", async () => {
    expect((await getJson(server, "/v1/search/artist")).status).toBe(400);
    expect((await getJson(server, "/v1/search/artist?q=x&mode=nope")).status).toBe(400);
    expect((await getJson(server, "/v1/search/widget?q=x")).status).toBe(404);
  });

  test("no match is an empty list, not an error", async () => {
    const { status, body } = await getJson(server, "/v1/search/artist?q=zzzzqqqqxxxx");
    expect(status).toBe(200);
    expect(body.results).toEqual([]);
  });
});

describe("supplementary data flag", () => {
  test("tags appear only when ingest is run with --include-supplementary", async () => {
    const withTags = artists.find((a) => a.tags?.length);
    expect(withTags).toBeDefined();
    const s = await serve(await ingest(["--include-supplementary"]));
    try {
      const { body } = await getJson(s, `/v1/artist/${withTags.id}`);
      expect(body.tags.length).toBeGreaterThan(0);
    } finally {
      await s.stop();
    }
  });
});
