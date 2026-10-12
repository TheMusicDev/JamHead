import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { existsSync, mkdirSync, readdirSync, readlinkSync, writeFileSync, copyFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fixture, FIXTURE_DIR, getJson, ingest, run, serve, type Server } from "./harness";

setDefaultTimeout(30_000);

describe("odd requests never crash the server or cause a 5xx", () => {
  let server: Server;
  const artist = fixture("artist")[0];
  beforeAll(async () => {
    server = await serve(await ingest());
  });
  afterAll(() => server.stop());

  const nasty: [string, string][] = [
    ["huge q", `/v1/search/artist?q=${"a".repeat(10_000)}`],
    ["q just over the limit", `/v1/search/artist?q=${"a".repeat(201)}`],
    ["emoji", `/v1/search/artist?q=${encodeURIComponent("🎸🎸🎸")}`],
    ["null byte", "/v1/search/artist?q=%00"],
    ["invalid utf-8", "/v1/search/artist?q=%ff%fe"],
    ["negative limit", "/v1/search/artist?q=a&limit=-1"],
    ["giant limit", "/v1/search/artist?q=a&limit=99999999999999999999"],
    ["zero limit", "/v1/search/artist?q=a&limit=0"],
    ["limit not a number", "/v1/search/artist?q=a&limit=abc"],
    ["huge limit text", "/v1/search/artist?mode=text&q=a&limit=1000000"],
    ["huge limit fuzzy", "/v1/search/artist?mode=fuzzy&q=abcdefgh&limit=1000000"],
    ["long fuzzy query", `/v1/search/artist?mode=fuzzy&q=${"abc ".repeat(40)}`],
    ["negative offset", `/v1/artist/${artist.id}/release-groups?offset=-5`],
    ["giant offset", `/v1/artist/${artist.id}/release-groups?offset=99999999999999999`],
    ["offset past the end", `/v1/artist/${artist.id}/release-groups?offset=1000000`],
    ["empty id", "/v1/artist/"],
    ["very long id", `/v1/artist/${"a".repeat(5000)}`],
    ["path traversal", "/v1/artist/..%2f..%2fetc%2fpasswd"],
    ["sql-ish id", "/v1/artist/1%27%20OR%20%271%27=%271"],
    ["double slash", `//v1//artist//${artist.id}`],
    ["trailing slash", `/v1/artist/${artist.id}/`],
    ["unknown route", "/v2/whatever"],
    ["admin without token configured", "/admin/reload"],
    ["half a uuid", "/v1/artist/ca264abf-3eb6-4d53"],
    ["uuid without hyphens", `/v1/artist/${artist.id.replaceAll("-", "")}`],
    ["urn uuid", `/v1/artist/urn:uuid:${artist.id}`],
    ["control chars in mode", "/v1/search/artist?q=a&mode=%0d%0a"],
  ];
  for (const [name, path] of nasty) {
    test(name, async () => {
      const res = await fetch(server.url + path);
      await res.arrayBuffer();
      expect(res.status).toBeLessThan(500);
    });
  }

  test("the long-query limit answers 400, and exactly 200 characters is still fine", async () => {
    expect((await getJson(server, `/v1/search/artist?q=${"a".repeat(201)}`)).status).toBe(400);
    expect((await getJson(server, `/v1/search/artist?q=${"a".repeat(200)}`)).status).toBe(200);
  });

  test("other methods are refused, HEAD works", async () => {
    expect((await fetch(`${server.url}/v1/artist/${artist.id}`, { method: "DELETE" })).status).toBe(405);
    expect((await fetch(`${server.url}/v1/artist/${artist.id}`, { method: "POST" })).status).toBe(405);
    const head = await fetch(`${server.url}/v1/artist/${artist.id}`, { method: "HEAD" });
    expect(head.status).toBe(200);
    expect(await head.text()).toBe("");
  });

  test("a burst of mixed parallel requests all succeed and the server is still healthy", async () => {
    const paths = [
      `/v1/artist/${artist.id}`,
      "/v1/search/artist?q=a",
      "/v1/search/release?mode=text&q=the",
      "/v1/search/release-group?mode=fuzzy&q=loveee",
      "/v1/meta",
    ];
    const results = await Promise.all(Array.from({ length: 500 }, (_, i) => fetch(server.url + paths[i % paths.length]).then(async (r) => (await r.arrayBuffer(), r.status))));
    expect(results.every((s) => s === 200)).toBe(true);
    expect((await fetch(server.url + "/healthz")).status).toBe(200);
  });
});

describe("bad dumps", () => {
  const tmp = () => mkdtempSync(join(tmpdir(), "jamhead-bad-"));
  const dump = (artistLines: string[]) => {
    const input = tmp();
    mkdirSync(join(input, "mbdump"));
    writeFileSync(join(input, "mbdump", "artist"), artistLines.join("\n") + "\n");
    return input;
  };
  const good = Object.fromEntries(fixture("artist").slice(0, 3).map((a) => [a.id, JSON.stringify(a)]));

  test("unreadable lines are skipped and reported, the rest is ingested", async () => {
    const out = tmp();
    const input = dump([
      ...Object.values(good).slice(0, 2),
      "{this is not json",
      "",
      '{"id":"not-a-uuid","name":"x"}',
      '{"name":"no id at all"}',
      "[1,2,3]",
      Object.values(good)[2],
    ]);
    const r = await run(["ingest", "--input", input, "--out", out]);
    expect(r.code).toBe(0);
    expect(r.stderr).toContain("skipped 4 unreadable lines");
    const server = await serve(out);
    try {
      expect((await getJson(server, "/v1/meta")).body.counts.artist).toBe(3);
      for (const id of Object.keys(good)) expect((await getJson(server, `/v1/artist/${id}`)).status).toBe(200);
    } finally {
      await server.stop();
    }
  });

  test("a cut-off .tar.xz fails cleanly: non-zero exit, nothing half-built, live data untouched", async () => {
    const out = await ingest(); // live data that must survive
    const live = readlinkSync(join(out, "current"));
    const input = tmp();
    const full = join(input, "full.tar.xz");
    await Bun.$`tar -cJf ${full} -C ${FIXTURE_DIR} mbdump/artist`.quiet();
    const bytes = new Uint8Array(await Bun.file(full).arrayBuffer());
    writeFileSync(join(input, "artist.tar.xz"), bytes.subarray(0, Math.floor(bytes.length / 2)));
    const r = await run(["ingest", "--input", input, "--out", out]);
    expect(r.code).not.toBe(0);
    expect(readdirSync(out).filter((n) => n.startsWith(".building"))).toEqual([]);
    expect(readlinkSync(join(out, "current"))).toBe(live);
    expect((await run(["verify", "--data", out])).code).toBe(0);
  });

  test("an input with no dumps in it is an error that says so", async () => {
    const r = await run(["ingest", "--input", tmp(), "--out", tmp()]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("no dumps found");
  });

  test("--limit 0 builds an empty but valid data directory", async () => {
    const out = tmp();
    const r = await run(["ingest", "--input", FIXTURE_DIR, "--out", out, "--limit", "0"]);
    expect(r.code).toBe(0);
    expect((await run(["verify", "--data", out])).code).toBe(0);
    const server = await serve(out);
    try {
      expect((await getJson(server, "/v1/meta")).body.counts.artist).toBe(0);
      expect((await getJson(server, "/v1/search/artist?q=a")).body.results).toEqual([]);
      expect((await getJson(server, "/v1/search/artist?mode=text&q=a")).body.results).toEqual([]);
    } finally {
      await server.stop();
    }
  });

  test("serve on an empty directory says what to do", async () => {
    const proc = Bun.spawn(["./target/release/jamhead", "serve", "--data", tmp(), "--bind", "127.0.0.1:0"], { stdout: "pipe", stderr: "pipe" });
    expect(await proc.exited).not.toBe(0);
    expect(await new Response(proc.stderr).text()).toContain("ingest");
  });
});
