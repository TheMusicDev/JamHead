// Load benchmark: starts a real `jamhead serve` on a data directory and measures latency
// and throughput of ID lookups and the search modes.
//
//   bun run bench -- --data ./data [--entity artist] [--seconds 5] [--concurrency 32]
//
// This runs in Bun, so the client itself costs some CPU. For cleaner server-only numbers use
// `oha`: pass --urls-dir <dir> to write one URL list per scenario (the server stays up), then run
// oha -z 10s -c 32 --urls-from-file <dir>/<scenario>.txt --no-tui

import { parseArgs } from "node:util";

const { values: opt } = parseArgs({
  options: {
    data: { type: "string", default: "./data" },
    entity: { type: "string", default: "artist" },
    seconds: { type: "string", default: "5" },
    concurrency: { type: "string", default: "32" },
    bin: { type: "string", default: process.env.JAMHEAD_BIN ?? "./target/release/jamhead" },
    "urls-dir": { type: "string" },
  },
});
const seconds = Number(opt.seconds);
const concurrency = Number(opt.concurrency);
const entity = opt.entity!;

const proc = Bun.spawn([opt.bin!, "serve", "--data", opt.data!, "--bind", "127.0.0.1:0"], {
  stdout: "pipe",
  stderr: "inherit",
});
const reader = (proc.stdout as ReadableStream<Uint8Array>).getReader();
let seen = "";
let base = "";
while (!base) {
  const { value, done } = await reader.read();
  if (done) throw new Error("server exited early");
  seen += new TextDecoder().decode(value);
  base = seen.match(/listening on (http:\/\/\S+)/)?.[1] ?? "";
}

const getJson = async (path: string) => (await fetch(base + path)).json() as Promise<any>;
const meta = await getJson("/v1/meta");

// Collect real names and ids by asking for every two-letter prefix.
const ids = new Set<string>();
const names: string[] = [];
const letters = "abcdefghijklmnopqrstuvwxyz";
for (const a of letters) {
  for (const b of letters) {
    const r = await getJson(`/v1/search/${entity}?q=${a}${b}&limit=100`);
    for (const hit of r.results ?? []) {
      if (!ids.has(hit.id)) names.push(hit.name);
      ids.add(hit.id);
    }
  }
}
const idList = [...ids];
if (idList.length === 0) throw new Error(`no ${entity} data to benchmark`);

const pick = <T>(xs: T[]) => xs[Math.floor(Math.random() * xs.length)];
const typo = (s: string) => (s.length < 5 ? s : s.slice(0, 2) + s[3] + s[2] + s.slice(4));
const scenarios: Record<string, () => string> = {
  "id lookup": () => `/v1/${entity}/${pick(idList)}`,
  "prefix search": () => `/v1/search/${entity}?q=${encodeURIComponent(pick(names).slice(0, 4))}&limit=10`,
  "fuzzy search": () => `/v1/search/${entity}?mode=fuzzy&q=${encodeURIComponent(typo(pick(names)))}&limit=10`,
};
if (meta.counts?.[entity] && (await getJson(`/v1/search/${entity}?mode=text&q=a`)).error === undefined) {
  scenarios["text search"] = () => `/v1/search/${entity}?mode=text&q=${encodeURIComponent(pick(names).split(" ")[0])}&limit=10`;
}

if (opt["urls-dir"]) {
  const { mkdirSync, writeFileSync } = await import("node:fs");
  mkdirSync(opt["urls-dir"], { recursive: true });
  for (const [name, mk] of Object.entries(scenarios)) {
    const urls = Array.from({ length: 20000 }, () => base + mk());
    writeFileSync(`${opt["urls-dir"]}/${name.replace(/ /g, "-")}.txt`, urls.join("\n") + "\n");
  }
  console.log(`server ${base} (Ctrl-C to stop); URL lists written to ${opt["urls-dir"]}`);
  await proc.exited;
  process.exit(0);
}

async function run(name: string, mk: () => string) {
  for (let i = 0; i < 200; i++) await (await fetch(base + mk())).arrayBuffer(); // warm up
  const lat: number[] = [];
  let errors = 0;
  const end = performance.now() + seconds * 1000;
  const worker = async () => {
    while (performance.now() < end) {
      const t = performance.now();
      const res = await fetch(base + mk());
      await res.arrayBuffer();
      lat.push(performance.now() - t);
      if (!res.ok) errors++;
    }
  };
  const started = performance.now();
  await Promise.all(Array.from({ length: concurrency }, worker));
  const elapsed = (performance.now() - started) / 1000;
  lat.sort((a, b) => a - b);
  const q = (p: number) => lat[Math.min(lat.length - 1, Math.floor(lat.length * p))];
  return { name, rps: Math.round(lat.length / elapsed), p50: q(0.5), p99: q(0.99), max: lat[lat.length - 1], errors };
}

console.log(`data ${meta.data_version}: ${JSON.stringify(meta.counts)}; ${idList.length} sample ids; ` + `${concurrency} connections, ${seconds}s per scenario\n`);
console.log("| scenario | req/s | p50 ms | p99 ms | max ms | errors |");
console.log("|---|---:|---:|---:|---:|---:|");
for (const [name, mk] of Object.entries(scenarios)) {
  const r = await run(name, mk);
  console.log(`| ${r.name} | ${r.rps} | ${r.p50.toFixed(2)} | ${r.p99.toFixed(2)} | ${r.max.toFixed(2)} | ${r.errors} |`);
}
proc.kill();
