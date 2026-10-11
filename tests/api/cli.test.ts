import { expect, test } from "bun:test";

const BIN = "./target/release/jamhead";

test("binary lists the serve and ingest commands", async () => {
  const out = await Bun.$`${BIN} --help`.text();
  expect(out).toContain("serve");
  expect(out).toContain("ingest");
});
