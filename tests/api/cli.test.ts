import { expect, test } from "bun:test";

const BIN = "./target/release/jamhead";

test("binary lists the serve and ingest commands", async () => {
  const out = await Bun.$`${BIN} --help`.text();
  expect(out).toContain("serve");
  expect(out).toContain("ingest");
});

// ---- plain-English progress (the log is what a person watches on a NAS) ----

import { existsSync, mkdtempSync, readdirSync, readlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FIXTURE_DIR, run } from "./harness";

test("ingest says what it is doing, in order, and ends with DONE", async () => {
  const out = mkdtempSync(join(tmpdir(), "jamhead-log-"));
  const r = await run(["ingest", "--input", FIXTURE_DIR, "--out", out]);
  expect(r.code).toBe(0);
  const steps = [
    "Reading artists from",
    "Reading artists: done, 176 records",
    "Building search indexes for artists",
    "Writing artist records to the database",
    "Compacting the database",
    "Checksumming the new files",
    "DONE in",
  ];
  let at = -1;
  for (const s of steps) {
    const i = r.stderr.indexOf(s, at + 1);
    expect(i, `missing or out of order: ${s}\n${r.stderr}`).toBeGreaterThan(at);
    at = i;
  }
  // The last line is the DONE line, so a person can tell it has finished.
  const last = r.stderr.trim().split("\n").pop()!;
  expect(last).toMatch(/^DONE in .*Data version v\S+ is now the live one\./);
});

test("ingest leaves no temporary spool files in the finished data", async () => {
  const out = mkdtempSync(join(tmpdir(), "jamhead-spool-"));
  expect((await run(["ingest", "--input", FIXTURE_DIR, "--out", out])).code).toBe(0);
  const version = readlinkSync(join(out, "current"));
  const store = readdirSync(join(out, version, "store")).sort();
  expect(store).toEqual(["data.mdb"]);
  expect(existsSync(join(out, version, "store", "spool"))).toBe(false);
});

test("a failure ends with a FAILED line that gives the reason", async () => {
  const out = mkdtempSync(join(tmpdir(), "jamhead-fail-"));
  const r = await run(["ingest", "--input", "/definitely/not/here", "--out", out]);
  expect(r.code).toBe(1);
  expect(r.stderr).toContain("FAILED:");
  expect(r.stderr.trim().split("\n").pop()).toMatch(/^FAILED: .+/);
});
