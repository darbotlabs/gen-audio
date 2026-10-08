import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { validateViewport } from "./src/validate.ts";

const example = JSON.parse(
  readFileSync(new URL("../../schemas/examples/viewport.example.json", import.meta.url), "utf8"),
) as { cards: Array<{ kind: string; body: Record<string, unknown> }> };

assert.equal(validateViewport(example, "allowed"), null);
assert.match(validateViewport(example, "release") ?? "", /not measured/);

const measured = structuredClone(example);
const benchmark = measured.cards.find((card) => card.kind === "BenchmarkCompare");
if (!benchmark) throw new Error("missing benchmark card");
benchmark.body.measuredHere = true;
assert.equal(validateViewport(measured, "allowed"), null);

const castDoc = structuredClone(example);
const cast = castDoc.cards.find((card) => card.kind === "PodcastCast");
if (!cast) throw new Error("missing cast card");
cast.body.sampleScript = false;
assert.match(validateViewport(castDoc, "allowed") ?? "", /sampleScript/);

const note = structuredClone(example);
const row = note.cards.find((card) => card.kind === "BenchmarkCompare");
if (!row) throw new Error("missing benchmark card");
row.body.sourceNote = "These numbers were measured in this window.";
assert.match(validateViewport(note, "allowed") ?? "", /not measured/);

console.log("viewport checker ok");
