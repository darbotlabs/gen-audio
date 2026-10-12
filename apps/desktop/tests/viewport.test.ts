import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { validateViewport } from "../src/validate.ts";

const example = JSON.parse(
  readFileSync(new URL("../../../schemas/examples/viewport.example.json", import.meta.url), "utf8"),
) as { cards: Array<{ kind: string; body: Record<string, unknown> }> };

test("example viewport matches the runtime contract", () => {
  assert.equal(validateViewport(example), null);
});

test("reference benchmarks cannot claim local measurements", () => {
  const measured = structuredClone(example);
  const benchmark = measured.cards.find((card) => card.kind === "BenchmarkCompare");
  assert.ok(benchmark, "missing benchmark card");
  benchmark.body.measuredHere = true;
  assert.match(validateViewport(measured) ?? "", /measured/);
});

test("example cast must remain labeled as a sample", () => {
  const castDoc = structuredClone(example);
  const cast = castDoc.cards.find((card) => card.kind === "PodcastCast");
  assert.ok(cast, "missing cast card");
  cast.body.sampleScript = false;
  assert.match(validateViewport(castDoc) ?? "", /sampleScript/);
});

test("reference source notes must disclose that figures are not remeasured", () => {
  const note = structuredClone(example);
  const row = note.cards.find((card) => card.kind === "BenchmarkCompare");
  assert.ok(row, "missing benchmark card");
  row.body.sourceNote = "These numbers were measured in this window.";
  assert.match(validateViewport(note) ?? "", /not measured/);
});
