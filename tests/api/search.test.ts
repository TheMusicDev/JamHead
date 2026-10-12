import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { fixture, getJson, ingest, serve, type Server } from "./harness";

const artists = fixture("artist");
const groups = fixture("release-group");
const releases = fixture("release");
let server: Server;

beforeAll(async () => {
  server = await serve(await ingest());
});
afterAll(() => server.stop());

const text = async (entity: string, q: string, extra = "") => {
  const r = await getJson(server, `/v1/search/${entity}?mode=text&q=${encodeURIComponent(q)}${extra}`);
  expect(r.status).toBe(200);
  return r.body.results as { id: string; name: string; detail?: string }[];
};
const fold = (s: string) => s.normalize("NFKD").replace(/[̀-ͯ]/g, "");

describe("mode=text", () => {
  test("artists are found by their full name", async () => {
    for (const a of artists) {
      const hits = await text("artist", a.name, "&limit=50");
      expect(hits.map((h) => h.id)).toContain(a.id);
    }
  });

  test("artists are found by an alias", async () => {
    const a = artists.find((x) => x.aliases?.some((al: any) => al.name !== x.name && /^[\w ]{6,}$/.test(al.name)))!;
    const alias = a.aliases.find((al: any) => al.name !== a.name && /^[\w ]{6,}$/.test(al.name)).name;
    expect((await text("artist", alias, "&limit=50")).map((h) => h.id)).toContain(a.id);
  });

  test("word order and case do not matter", async () => {
    const a = artists.find((x) => /^\w+ \w+$/.test(x.name))!;
    const [w1, w2] = a.name.split(" ");
    expect((await text("artist", `${w2.toUpperCase()} ${w1}`, "&limit=50")).map((h) => h.id)).toContain(a.id);
  });

  test("accents are ignored", async () => {
    const a = artists.find((x) => fold(x.name) !== x.name)!;
    expect((await text("artist", fold(a.name), "&limit=50")).map((h) => h.id)).toContain(a.id);
  });

  test("release groups are found by title words plus the artist", async () => {
    let checked = 0;
    for (const g of groups) {
      const credit = g["artist-credit"]?.[0];
      const word = g.title.split(/\W+/).find((w: string) => w.length > 3);
      if (!credit || !word) continue;
      const hits = await text("release-group", `${credit.artist.name} ${word}`, "&limit=50");
      expect(hits.map((h) => h.id)).toContain(g.id);
      checked++;
    }
    expect(checked).toBeGreaterThan(30);
  });

  test("releases are found by title", async () => {
    for (const r of releases) {
      expect((await text("release", r.title, "&limit=100")).map((h) => h.id)).toContain(r.id);
    }
  });

  test("results carry name and detail", async () => {
    const g = groups.find((x) => x["artist-credit"]?.length)!;
    const hit = (await text("release-group", g.title, "&limit=100")).find((h) => h.id === g.id)!;
    expect(hit.name).toBe(g.title);
    expect(hit.detail).toContain(g["artist-credit"][0].name || g["artist-credit"][0].artist.name);
  });

  test("when no entity has every word, entities with any word are returned", async () => {
    const a = artists[0];
    const hits = await text("artist", `${a.name} zzzzqqqqxxxx`, "&limit=50");
    expect(hits.map((h) => h.id)).toContain(a.id);
  });

  test("query syntax characters cannot break the search", async () => {
    for (const q of ['"AC/DC" AND (foo OR bar) -x:y ~2 *', "((((", "title:", "a\\b", "\u0000"]) {
      const r = await getJson(server, `/v1/search/artist?mode=text&q=${encodeURIComponent(q)}`);
      expect(r.status).toBe(200);
    }
  });

  test("limit is respected", async () => {
    expect((await text("artist", "a", "&limit=2")).length).toBeLessThanOrEqual(2);
  });

  test("nothing matches gibberish", async () => {
    expect(await text("artist", "zzzzqqqqxxxx")).toEqual([]);
  });
});

describe("modes", () => {
  test("every mode answers for every entity", async () => {
    for (const entity of ["artist", "release-group", "release"]) {
      for (const mode of ["prefix", "fuzzy", "text"]) {
        const r = await getJson(server, `/v1/search/${entity}?mode=${mode}&q=the`);
        expect(r.status).toBe(200);
        expect(r.body.mode).toBe(mode);
      }
    }
  });

  test("prefix is the default mode", async () => {
    const r = await getJson(server, "/v1/search/artist?q=a");
    expect(r.body.mode).toBe("prefix");
  });

  test("unknown mode is a 400 that names the choices", async () => {
    const r = await getJson(server, "/v1/search/artist?q=a&mode=nope");
    expect(r.status).toBe(400);
    expect(r.body.error).toContain("text");
  });
});
