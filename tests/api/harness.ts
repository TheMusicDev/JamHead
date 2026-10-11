// Helpers shared by the API tests: build a data directory from the checked-in fixture
// dump, start a real `jamhead serve`, and read the fixture back for expected values.

import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const BIN = process.env.JAMHEAD_BIN ?? "./target/release/jamhead";
export const FIXTURE_DIR = "tests/fixtures/dump";

/** Parsed lines of a fixture dump file ("artist", "release-group" or "release"). */
export function fixture(entity: string): any[] {
  return readFileSync(join(FIXTURE_DIR, "mbdump", entity), "utf8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l));
}

/** Run ingest into a fresh temp directory and return the data root. */
export async function ingest(extraArgs: string[] = [], input = FIXTURE_DIR): Promise<string> {
  const out = mkdtempSync(join(tmpdir(), "jamhead-test-"));
  const proc = Bun.spawn([BIN, "ingest", "--input", input, "--out", out, ...extraArgs], {
    stdout: "ignore",
    stderr: "pipe",
  });
  const code = await proc.exited;
  if (code !== 0) throw new Error(`ingest failed (${code}): ${await new Response(proc.stderr).text()}`);
  return out;
}

export interface Server {
  url: string;
  proc: ReturnType<typeof Bun.spawn>;
  stop(): Promise<void>;
}

/** Start `jamhead serve` on a free port and wait until it says where it listens. */
export async function serve(dataRoot: string, extraArgs: string[] = [], env: Record<string, string> = {}): Promise<Server> {
  const proc = Bun.spawn([BIN, "serve", "--data", dataRoot, "--bind", "127.0.0.1:0", ...extraArgs], {
    stdout: "pipe",
    stderr: "ignore",
    env: { ...process.env, ...env },
  });
  const reader = (proc.stdout as ReadableStream<Uint8Array>).getReader();
  const decoder = new TextDecoder();
  let seen = "";
  while (true) {
    const { value, done } = await reader.read();
    if (done) throw new Error(`server exited early: ${seen}`);
    seen += decoder.decode(value);
    const m = seen.match(/listening on (http:\/\/\S+)/);
    if (m) {
      reader.releaseLock();
      return {
        url: m[1],
        proc,
        stop: async () => {
          proc.kill();
          await proc.exited;
        },
      };
    }
  }
}

export async function getJson(server: Server, path: string): Promise<{ status: number; body: any; headers: Headers }> {
  const res = await fetch(server.url + path);
  const text = await res.text();
  let body: any = text;
  try {
    body = JSON.parse(text);
  } catch {}
  return { status: res.status, body, headers: res.headers };
}

/** True if an object (at any depth) has a null, "" or [] value. The API must never send those. */
export function findEmptyValue(v: any, path = ""): string | null {
  if (v === null || v === "" || (Array.isArray(v) && v.length === 0)) return path || "(root)";
  if (Array.isArray(v)) {
    for (let i = 0; i < v.length; i++) {
      const r = findEmptyValue(v[i], `${path}[${i}]`);
      if (r) return r;
    }
  } else if (typeof v === "object") {
    for (const [k, x] of Object.entries(v)) {
      const r = findEmptyValue(x, `${path}.${k}`);
      if (r) return r;
    }
  }
  return null;
}

/** Run a jamhead command and return exit code plus output. */
export async function run(args: string[], env: Record<string, string> = {}) {
  const proc = Bun.spawn([BIN, ...args], { stdout: "pipe", stderr: "pipe", env: { ...process.env, ...env } });
  const [stdout, stderr] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text()]);
  return { code: await proc.exited, stdout, stderr };
}

export interface Mirror {
  url: string;
  date: string;
  requests: { path: string; range: string | null }[];
  /** Serve different bytes for this file name (to test checksum failures). */
  tamper: Set<string>;
  stop(): void;
}

/**
 * A tiny stand-in for data.metabrainz.org: serves the fixture dump as .tar.xz files, with
 * LATEST, SHA256SUMS and Range support, and records every request.
 */
export async function startMirror(date = "20261010-001001"): Promise<Mirror> {
  const dir = mkdtempSync(join(tmpdir(), "jamhead-mirror-"));
  const files = new Map<string, Uint8Array>();
  const sums: string[] = [];
  for (const entity of ["artist", "release-group", "release"]) {
    const name = `${entity}.tar.xz`;
    await Bun.$`tar -cJf ${join(dir, name)} -C ${FIXTURE_DIR} mbdump/${entity}`.quiet();
    const bytes = new Uint8Array(await Bun.file(join(dir, name)).arrayBuffer());
    files.set(`/${date}/${name}`, bytes);
    sums.push(`${new Bun.CryptoHasher("sha256").update(bytes).digest("hex")}  ${name}`);
  }
  files.set(`/${date}/SHA256SUMS`, new TextEncoder().encode(sums.join("\n") + "\n"));
  files.set("/LATEST", new TextEncoder().encode(date + "\n"));

  const requests: Mirror["requests"] = [];
  const tamper = new Set<string>();
  const server = Bun.serve({
    port: 0,
    fetch(req) {
      const path = new URL(req.url).pathname;
      const range = req.headers.get("range");
      requests.push({ path, range });
      let bytes = files.get(path);
      if (!bytes) return new Response("not found", { status: 404 });
      if (tamper.has(path.split("/").pop()!)) {
        bytes = bytes.slice();
        bytes[bytes.length - 1] ^= 0xff;
      }
      const m = range?.match(/^bytes=(\d+)-$/);
      if (m) {
        const start = Number(m[1]);
        if (start >= bytes.length) return new Response("", { status: 416 });
        return new Response(bytes.slice(start), { status: 206 });
      }
      return new Response(bytes);
    },
  });
  return { url: `http://127.0.0.1:${server.port}`, date, requests, tamper, stop: () => server.stop(true) };
}

export async function waitFor<T>(what: string, fn: () => Promise<T | undefined | false>, ms = 8000): Promise<T> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const v = await fn();
    if (v) return v;
    await Bun.sleep(100);
  }
  throw new Error(`timed out waiting for ${what}`);
}
