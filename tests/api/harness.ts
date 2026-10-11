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
