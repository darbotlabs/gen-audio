// The viewport checker must not read Vite's import.meta.env. Under plain Node
// that object is missing, so the example deck's "not remeasured" note used to
// crash the check instead of being accepted. Callers pass "allowed" or "release".
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { validateViewport } from "../src/validate.ts";

const example = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../../schemas/examples/viewport.example.json", import.meta.url)), "utf8"),
);

test("the example deck is valid when the caller passes allowed", () => {
  assert.equal(validateViewport(example, "allowed"), null);
});

test("release mode rejects the example deck's not-remeasured note", () => {
  assert.match(validateViewport(example, "release") ?? "", /not measured/);
});

test("allowed still rejects a note that claims the figures were measured", () => {
  const note = structuredClone(example);
  const row = note.cards.find((card: { kind: string }) => card.kind === "BenchmarkCompare");
  row.body.sourceNote = "These numbers were measured in this window.";
  assert.match(validateViewport(note, "allowed") ?? "", /not measured/);
});

test("validate.ts takes no build settings and does not embed the release-gate needle", () => {
  const source = readFileSync(fileURLToPath(new URL("../src/validate.ts", import.meta.url)), "utf8");
  assert.equal(source.includes("import.meta.env"), false);
  assert.equal(source.includes("not remeasured"), false);
});
