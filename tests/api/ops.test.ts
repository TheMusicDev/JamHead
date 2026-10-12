import { describe, expect, setDefaultTimeout, test } from "bun:test";

setDefaultTimeout(30_000);
import { existsSync, readdirSync, readFileSync, readlinkSync, symlinkSync, renameSync, truncateSync, mkdirSync, writeFileSync, openSync, writeSync, closeSync } from "node:fs";
import { join } from "node:path";
import { fixture, getJson, ingest, run, serve, startMirror, waitFor, FIXTURE_DIR } from "./harness";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";

const versions = (root: string) => readdirSync(root).filter((n) => n.startsWith("v") && existsSync(join(root, n, "meta.json")));
const counts = () => ({ artist: fixture("artist").length, "release-group": fixture("release-group").length, release: fixture("release").length });

describe("verify and stats", () => {
  test("verify passes on fresh data, including checksums", async () => {
    const root = await ingest();
    const r = await run(["verify", "--data", root, "--deep"]);
    expect(r.code).toBe(0);
    expect(r.stdout).toContain("0 failed");
  });

  test("verify fails when a file is cut short", async () => {
    const root = await ingest();
    const dir = join(root, readlinkSync(join(root, "current")));
    truncateSync(join(dir, "fst", "artist.ids"), 100);
    const r = await run(["verify", "--data", root]);
    expect(r.code).toBe(1);
    expect(r.stdout).toContain("FAIL");
    expect(r.stdout).toContain("fst/artist.ids");
  });

  test("verify --deep catches a changed byte even when the size is the same", async () => {
    const root = await ingest();
    const dir = join(root, readlinkSync(join(root, "current")));
    const fd = openSync(join(dir, "fst", "artist.ids"), "r+");
    writeSync(fd, new Uint8Array([0xff]), 0, 1, 40);
    closeSync(fd);
    expect((await run(["verify", "--data", root])).code).toBe(0); // sizes alone cannot see it
    const deep = await run(["verify", "--data", root, "--deep"]);
    expect(deep.code).toBe(1);
    expect(deep.stdout).toContain("checksum");
  });

  test("stats prints counts and sizes", async () => {
    const root = await ingest();
    const r = await run(["stats", "--data", root]);
    expect(r.code).toBe(0);
    expect(r.stdout).toContain(`artist        ${counts().artist}`);
    expect(r.stdout).toContain("size total");
    expect(r.stdout).toContain("versions kept 1");
  });
});

describe("versions", () => {
  test("each ingest makes a new version, current moves, and only two are kept", async () => {
    const root = await ingest();
    const seen = [readlinkSync(join(root, "current"))];
    for (let i = 0; i < 3; i++) {
      const r = await run(["ingest", "--input", FIXTURE_DIR, "--out", root, "--limit", "5"]);
      expect(r.code).toBe(0);
      seen.push(readlinkSync(join(root, "current")));
    }
    expect(new Set(seen).size).toBe(4);
    expect(versions(root).sort()).toEqual(seen.slice(-2).sort());
    expect((await run(["verify", "--data", root])).code).toBe(0);
  });

  test("--keep-versions 1 leaves only the live version", async () => {
    const root = await ingest();
    const r = await run(["ingest", "--input", FIXTURE_DIR, "--out", root, "--limit", "5", "--keep-versions", "1"]);
    expect(r.code).toBe(0);
    expect(versions(root)).toEqual([readlinkSync(join(root, "current"))]);
    expect((await run(["verify", "--data", root])).code).toBe(0);
  });

  test("--keep-versions 0 is refused", async () => {
    const root = await ingest();
    const r = await run(["ingest", "--input", FIXTURE_DIR, "--out", root, "--keep-versions", "0"]);
    expect(r.code).not.toBe(0);
    expect(versions(root).length).toBe(1);
  });
});

describe("reload while serving", () => {
  test("the server notices a new current version by itself, without failing requests", async () => {
    const root = await ingest();
    const server = await serve(root, ["--watch-interval", "1"]);
    try {
      const before = (await getJson(server, "/v1/meta")).body;
      expect(before.counts.artist).toBe(counts().artist);
      const artistId = fixture("artist")[0].id;

      let failures = 0;
      let requests = 0;
      let stop = false;
      const hammer = (async () => {
        while (!stop) {
          const res = await fetch(`${server.url}/v1/artist/${artistId}`).catch(() => null);
          requests++;
          if (!res || res.status !== 200) failures++;
          await res.arrayBuffer?.();
        }
      })();

      expect((await run(["ingest", "--input", FIXTURE_DIR, "--out", root, "--limit", "10"])).code).toBe(0);
      const after = await waitFor("new data version", async () => {
        const m = (await getJson(server, "/v1/meta")).body;
        return m.data_version !== before.data_version ? m : undefined;
      });
      stop = true;
      await hammer;
      expect(after.counts.artist).toBe(10);
      expect(requests).toBeGreaterThan(20);
      expect(failures).toBe(0);
    } finally {
      await server.stop();
    }
  });

  test("SIGHUP reloads when the watcher is off", async () => {
    const root = await ingest();
    const server = await serve(root, ["--watch-interval", "0"]);
    try {
      const before = (await getJson(server, "/v1/meta")).body.data_version;
      await run(["ingest", "--input", FIXTURE_DIR, "--out", root, "--limit", "7"]);
      await Bun.sleep(1500);
      expect((await getJson(server, "/v1/meta")).body.data_version).toBe(before); // nobody told it yet
      server.proc.kill("SIGHUP");
      const m = await waitFor("reload after SIGHUP", async () => {
        const x = (await getJson(server, "/v1/meta")).body;
        return x.data_version !== before ? x : undefined;
      });
      expect(m.counts.artist).toBe(7);
    } finally {
      await server.stop();
    }
  });

  test("POST /admin/reload needs the token, and is off without one", async () => {
    const root = await ingest();
    const server = await serve(root, ["--watch-interval", "0"], { JAMHEAD_ADMIN_TOKEN: "s3cret" });
    const open = await serve(root, ["--watch-interval", "0"], { JAMHEAD_ADMIN_TOKEN: "" });
    try {
      const post = (s: typeof server, token?: string) =>
        fetch(s.url + "/admin/reload", { method: "POST", headers: token ? { Authorization: `Bearer ${token}` } : {} });
      expect((await post(open, "anything")).status).toBe(404);
      expect((await post(server)).status).toBe(401);
      expect((await post(server, "wrong")).status).toBe(401);

      const first = (await getJson(server, "/v1/meta")).body.data_version;
      const nothing = await (await post(server, "s3cret")).json();
      expect(nothing).toEqual({ reloaded: false, data_version: first });

      await run(["ingest", "--input", FIXTURE_DIR, "--out", root, "--limit", "3"]);
      const res = await post(server, "s3cret");
      expect(res.status).toBe(200);
      const body = await res.json();
      expect(body.reloaded).toBe(true);
      expect(body.data_version).not.toBe(first);
      expect((await getJson(server, "/v1/meta")).body.counts.artist).toBe(3);
    } finally {
      await server.stop();
      await open.stop();
    }
  });

  test("a broken new version is ignored and the old one keeps serving", async () => {
    const root = await ingest();
    const server = await serve(root, ["--watch-interval", "1"]);
    try {
      const good = readlinkSync(join(root, "current"));
      const before = (await getJson(server, "/v1/meta")).body.data_version;
      mkdirSync(join(root, "v9999-broken"));
      writeFileSync(join(root, "v9999-broken", "meta.json"), "not json");
      symlinkSync("v9999-broken", join(root, ".tmp-link"));
      renameSync(join(root, ".tmp-link"), join(root, "current"));
      await Bun.sleep(2500);
      const art = fixture("artist")[0];
      expect((await getJson(server, `/v1/artist/${art.id}`)).status).toBe(200);
      expect((await getJson(server, "/v1/meta")).body.data_version).toBe(before);

      symlinkSync(good, join(root, ".tmp-link2"));
      renameSync(join(root, ".tmp-link2"), join(root, "current"));
      await Bun.sleep(1500);
      expect((await getJson(server, "/v1/meta")).body.data_version).toBe(before);
    } finally {
      await server.stop();
    }
  });
});

describe("download and update", () => {
  const tmp = () => mkdtempSync(join(tmpdir(), "jamhead-ops-"));

  test("download fetches checksummed dumps that ingest can read", async () => {
    const mirror = await startMirror();
    try {
      const out = tmp();
      const r = await run(["download", "--out", out, "--base-url", mirror.url]);
      expect(r.code).toBe(0);
      const dir = join(out, mirror.date);
      expect(readFileSync(join(dir, "DUMP_DATE"), "utf8")).toBe(mirror.date);
      for (const e of ["artist", "release-group", "release"]) expect(existsSync(join(dir, `${e}.tar.xz`))).toBe(true);
      expect(readdirSync(dir).some((f) => f.endsWith(".part"))).toBe(false);

      const data = tmp();
      expect((await run(["ingest", "--input", dir, "--out", data])).code).toBe(0); // straight from the .tar.xz
      const meta = (await getJson(await serve(data), "/v1/meta")).body;
      expect(meta.dump_date).toBe(mirror.date);
      expect(meta.counts).toEqual(counts());
    } finally {
      mirror.stop();
    }
  });

  test("an interrupted download is resumed with a Range request", async () => {
    const mirror = await startMirror();
    try {
      const out = tmp();
      const dir = join(out, mirror.date);
      mkdirSync(dir, { recursive: true });
      // Pretend an earlier run died after 100 bytes of artist.tar.xz.
      const sample = await Bun.$`tar -cJf - -C ${FIXTURE_DIR} mbdump/artist`.quiet();
      writeFileSync(join(dir, "artist.tar.xz.part"), sample.stdout.subarray(0, 100));
      const r = await run(["download", "--out", out, "--base-url", mirror.url, "--entities", "artist"]);
      expect(r.code).toBe(0);
      expect(mirror.requests.some((q) => q.path.endsWith("artist.tar.xz") && q.range === "bytes=100-")).toBe(true);
      expect(existsSync(join(dir, "artist.tar.xz"))).toBe(true);
    } finally {
      mirror.stop();
    }
  });

  test("a download that fails its checksum is rejected and deleted", async () => {
    const mirror = await startMirror();
    try {
      mirror.tamper.add("release.tar.xz");
      const out = tmp();
      const r = await run(["download", "--out", out, "--base-url", mirror.url]);
      expect(r.code).not.toBe(0);
      expect(r.stderr).toContain("checksum mismatch");
      const dir = join(out, mirror.date);
      expect(existsSync(join(dir, "release.tar.xz"))).toBe(false);
      expect(existsSync(join(dir, "release.tar.xz.part"))).toBe(false);
    } finally {
      mirror.stop();
    }
  });

  test("a file already downloaded is not fetched again", async () => {
    const mirror = await startMirror();
    try {
      const out = tmp();
      await run(["download", "--out", out, "--base-url", mirror.url, "--entities", "artist"]);
      const before = mirror.requests.filter((q) => q.path.endsWith("artist.tar.xz")).length;
      await run(["download", "--out", out, "--base-url", mirror.url, "--entities", "artist"]);
      expect(mirror.requests.filter((q) => q.path.endsWith("artist.tar.xz")).length).toBe(before);
    } finally {
      mirror.stop();
    }
  });

  test("update downloads, builds, flips current, cleans up, and then says it is up to date", async () => {
    const mirror = await startMirror();
    try {
      const data = tmp();
      const dumps = tmp();
      const args = ["update", "--data", data, "--dumps", dumps, "--base-url", mirror.url];
      const first = await run(args);
      expect(first.code).toBe(0);
      expect(versions(data).length).toBe(1);
      expect(readdirSync(dumps)).toEqual([]); // the dump was deleted after the build

      const again = await run(args);
      expect(again.code).toBe(0);
      expect(again.stdout).toContain("already up to date");
      expect(versions(data).length).toBe(1);

      const forced = await run([...args, "--force", "--keep-dumps"]);
      expect(forced.code).toBe(0);
      expect(versions(data).length).toBe(2);
      expect(readdirSync(dumps)).toEqual([mirror.date]);

      const server = await serve(data);
      try {
        expect((await getJson(server, "/v1/meta")).body.dump_date).toBe(mirror.date);
      } finally {
        await server.stop();
      }
    } finally {
      mirror.stop();
    }
  });

  test("unknown entity names are refused", async () => {
    const r = await run(["download", "--entities", "artist,widget", "--out", tmp()]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("widget");
  });
});

describe("healthcheck", () => {
  test("exits 0 against a running server and non-zero against nothing", async () => {
    const server = await serve(await ingest());
    try {
      expect((await run(["healthcheck", "--url", `${server.url}/healthz`])).code).toBe(0);
    } finally {
      await server.stop();
    }
    expect((await run(["healthcheck", "--url", "http://127.0.0.1:9/healthz"])).code).not.toBe(0);
  });
});
