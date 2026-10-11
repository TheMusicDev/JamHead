import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { fixture, getJson, ingest, serve, type Server } from "./harness";

const artists = fixture("artist");
const groups = fixture("release-group");
let server: Server;

beforeAll(async () => {
  server = await serve(await ingest());
});
afterAll(() => server.stop());

/** Fetch without letting Bun decode the body, so we see exactly what the server sent. */
const raw = (path: string, headers: Record<string, string> = {}) =>
  fetch(server.url + path, { headers, decompress: false } as RequestInit);

describe("compression", () => {
  const a = artists[0];

  test("zstd is served pre-compressed and decodes to the plain JSON", async () => {
    const plain = await (await raw(`/v1/artist/${a.id}`)).text();
    const res = await raw(`/v1/artist/${a.id}`, { "Accept-Encoding": "zstd, gzip" });
    expect(res.headers.get("content-encoding")).toBe("zstd");
    expect(res.headers.get("vary")).toContain("Accept-Encoding");
    const body = new Uint8Array(await res.arrayBuffer());
    expect(new TextDecoder().decode(Bun.zstdDecompressSync(body))).toBe(plain);
  });

  test("gzip when zstd is not accepted", async () => {
    const plain = await (await raw(`/v1/artist/${a.id}`)).text();
    const res = await raw(`/v1/artist/${a.id}`, { "Accept-Encoding": "gzip, deflate, br" });
    expect(res.headers.get("content-encoding")).toBe("gzip");
    expect(new TextDecoder().decode(Bun.gunzipSync(new Uint8Array(await res.arrayBuffer())))).toBe(plain);
  });

  test("plain when nothing we support is accepted", async () => {
    const res = await raw(`/v1/artist/${a.id}`, { "Accept-Encoding": "br" });
    expect(res.headers.get("content-encoding")).toBeNull();
    expect(JSON.parse(await res.text()).id).toBe(a.id);
  });

  test("list endpoints are compressed too, and decode to the same JSON", async () => {
    const withGroups = artists.find((x) => groups.some((g) => g["artist-credit"]?.some((c: any) => c.artist.id === x.id)))!;
    const path = `/v1/artist/${withGroups.id}/release-groups?limit=100`;
    const plain = JSON.parse(await (await raw(path)).text());
    expect(plain["release-groups"].length).toBeGreaterThan(0);
    // Lists are several compressed pieces joined together (valid per the zstd and gzip specs).
    // Decode them with the standard command line tools as an independent check.
    for (const [enc, tool] of [
      ["zstd", ["zstd", "-dc"]],
      ["gzip", ["gzip", "-dc"]],
    ] as const) {
      const res = await raw(path, { "Accept-Encoding": enc });
      expect(res.headers.get("content-encoding")).toBe(enc);
      const proc = Bun.spawn([...tool], { stdin: new Uint8Array(await res.arrayBuffer()), stdout: "pipe" });
      const decoded = JSON.parse(await new Response(proc.stdout).text());
      expect(decoded).toEqual(plain);
    }
  });
});

describe("ETag and caching", () => {
  test("ETag is the data version plus the MBID, and differs per encoding", async () => {
    const a = artists[1];
    const { data_version } = (await getJson(server, "/v1/meta")).body;
    const plain = await raw(`/v1/artist/${a.id}`);
    expect(plain.headers.get("etag")).toBe(`"${data_version}-${a.id}"`);
    expect(plain.headers.get("cache-control")).toContain("max-age");
    const zstd = await raw(`/v1/artist/${a.id}`, { "Accept-Encoding": "zstd" });
    expect(zstd.headers.get("etag")).not.toBe(plain.headers.get("etag"));
  });

  test("If-None-Match gets an empty 304", async () => {
    const a = artists[1];
    const etag = (await raw(`/v1/artist/${a.id}`)).headers.get("etag")!;
    const res = await raw(`/v1/artist/${a.id}`, { "If-None-Match": etag });
    expect(res.status).toBe(304);
    expect(await res.text()).toBe("");
    const other = await raw(`/v1/artist/${a.id}`, { "If-None-Match": '"something-else"' });
    expect(other.status).toBe(200);
  });

  test("list endpoints have ETags and 304 too", async () => {
    const path = `/v1/artist/${artists[2].id}/release-groups`;
    const etag = (await raw(path)).headers.get("etag")!;
    expect(etag).toBeTruthy();
    expect((await raw(path, { "If-None-Match": etag })).status).toBe(304);
  });

  test("meta is never cached", async () => {
    expect((await raw("/v1/meta")).headers.get("cache-control")).toBe("no-cache");
  });
});
