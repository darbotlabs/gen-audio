// PR #5 review, fix 5: release builds boot the Library deck without the dev
// fixtures; VITE_GEN_AUDIO_FIXTURES=1 (PR #4's flag) brings them back.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { DEV_FIXTURE_CLAIMS, fixturesRequested, isDevFixture, selectViewport } from "../src/viewport-source.ts";

const repo = (path: string) => fileURLToPath(new URL(`../../../${path}`, import.meta.url));
const load = (path: string) => JSON.parse(readFileSync(repo(path), "utf8"));
const DEV_ONLY = ["bench-ref", "cast-sample", "cube-fixture", "serve-gateway", "serve-node", "spec-fixture"];

test("only the exact flag value 1 requests fixtures", () => {
  assert.equal(fixturesRequested("1"), true);
  for (const flag of [undefined, "", "0", "true", "yes"]) assert.equal(fixturesRequested(flag), false, String(flag));
  assert.equal(selectViewport(undefined, "release", "example"), "release");
  assert.equal(selectViewport("1", "release", "example"), "example");
});

test("release assets.json and the release deck leave out the fixture, reference and sample/stub cards", () => {
  const release = load("apps/desktop/public/library/assets.json");
  const dev = load("schemas/asset-object/fixtures/assets.dev.json");
  assert.deepEqual(release.assets.filter(isDevFixture), []);
  assert.ok(dev.assets.every(isDevFixture));
  const devCards = dev.assets.filter((asset: { kind: string }) => asset.kind === "card").map((asset: { fields: { card_id: string } }) => asset.fields.card_id);
  assert.deepEqual([...devCards].sort(), DEV_ONLY);
  const releaseText = readFileSync(repo("apps/desktop/public/library/assets.json"), "utf8");
  for (const claim of DEV_FIXTURE_CLAIMS) assert.ok(!releaseText.includes(`"${claim}"`), claim);

  const shipped = load("schemas/examples/viewport.release.json");
  const example = load("schemas/examples/viewport.example.json");
  const ids = shipped.cards.map((card: { id: string }) => card.id);
  for (const id of DEV_ONLY) assert.ok(!ids.includes(id), `${id} in the release deck`);
  assert.deepEqual(
    shipped.cards,
    example.cards.filter((card: { id: string }) => !DEV_ONLY.includes(card.id)),
    "viewport.release.json is the example minus the dev fixtures (rerun build_assets)",
  );
  assert.ok(ids.includes("lib-bitdot-braille-vibevoice") && ids.includes("lib-misaki-kokoro"));
});

test("E1: no stub ids and no stand-in / not-remeasured text in the release catalog or deck", () => {
  for (const path of ["apps/desktop/public/library/assets.json", "schemas/examples/viewport.release.json"]) {
    const text = readFileSync(repo(path), "utf8");
    for (const id of ["cast-sample", "serve-node", "serve-gateway"]) assert.ok(!text.includes(`"${id}"`), `${id} in ${path}`);
    assert.doesNotMatch(text, /stand-in|not remeasured|<node>/i, path);
  }
});
