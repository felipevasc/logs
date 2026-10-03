import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { generateLogs } from "../bench/generate-logs.mjs";

test("benchmark fixture is deterministic and preserves representative edge cases", async () => {
  const dir = await mkdtemp(join(tmpdir(), "logs-fixture-"));
  try {
    const options = { rows: 4097, seed: 17, padding: 8 };
    const a = await generateLogs({ ...options, output: join(dir, "a.jsonl") });
    const b = await generateLogs({ ...options, output: join(dir, "b.jsonl") });
    assert.equal(a.sha256, b.sha256);
    assert.equal(a.bytes, b.bytes);
    const events = (await readFile(join(dir, "a.jsonl"), "utf8")).trimEnd().split("\n").map(JSON.parse);
    assert.equal(events.length, 4097);
    assert.equal(new Set(events.map(row => row.trace_id)).size, events.length);
    assert.equal(events[0].timestamp, events[9].timestamp);
    assert.notEqual(events[9].timestamp, events[10].timestamp);
    assert.ok(events[0].message.includes("rareneedle"));
    assert.equal(events[0].payload.length, 8);
    assert.equal(a.pointLookupId, events[Math.floor(events.length / 2)].trace_id);
    assert.notEqual(events[0].trace_id, "00000000000000000000000000000000");
    assert.equal(a.idOrder, "permuted");
    await assert.rejects(generateLogs({ ...options, output: join(dir, "a.jsonl") }), /EEXIST/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test("benchmark fixture rejects invalid resource parameters before writing", async () => {
  for (const rows of [0, -1, 1.5, NaN, Infinity]) {
    await assert.rejects(generateLogs({ rows, output: "unused.jsonl" }), /rows/);
  }
});
