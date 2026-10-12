import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { comparisonMethod, loadLibraryCatalog, parseLibraryCatalog, resetLibraryCatalogForTest, sourceSha256 } from "../src/library-assets.ts";
import { envelopeIdentityUid, glyphFromUid } from "../src/asset.ts";

const document = JSON.parse(readFileSync(new URL("../public/library/assets.json", import.meta.url), "utf8"));
const dev = JSON.parse(readFileSync(new URL("../../../schemas/asset-object/fixtures/assets.dev.json", import.meta.url), "utf8"));

test("a failed library catalog fetch rejects and is tried again", async () => {
  resetLibraryCatalogForTest();
  let fetches = 0;
  const original = globalThis.fetch;
  globalThis.fetch = async () => { fetches++; throw new TypeError("offline"); };
  try {
    await assert.rejects(loadLibraryCatalog(), /offline/);
    await assert.rejects(loadLibraryCatalog(), /offline/);
    assert.equal(fetches, 2);
  } finally {
    globalThis.fetch = original;
    resetLibraryCatalogForTest();
  }
});

test("unsafe media rejects the complete catalog instead of showing partial data", () => {
  const bad = structuredClone(document);
  bad.assets.find((asset: any) => asset.media.length).media[0].path = "../secret.wav";
  assert.throws(() => parseLibraryCatalog(bad), /Invalid library catalog/);
  assert.equal(parseLibraryCatalog(document).assets.length, document.assets.length);
});

test("only explicit extra assets add the development deck", async () => {
  resetLibraryCatalogForTest();
  const original = globalThis.fetch;
  globalThis.fetch = async () => Response.json(document);
  try {
    const release = await loadLibraryCatalog();
    assert.equal(release?.assets.length, document.assets.length);
    resetLibraryCatalogForTest();
    const expanded = await loadLibraryCatalog(Promise.resolve(dev.assets));
    assert.equal(expanded?.assets.length, document.assets.length + dev.assets.length);
  } finally {
    globalThis.fetch = original;
    resetLibraryCatalogForTest();
  }
});

test("dangling relationships and relationship cycles are rejected", () => {
  const bad = structuredClone(document);
  const card = bad.assets.find((asset: any) => asset.kind === "card");
  card.relations = { bound_to: ["ga:card:aaaaaaaaaaaaaaaaaaaaaaaaaa"] };
  assert.throws(() => parseLibraryCatalog(bad), /Missing library relation/);
  card.relations = { bound_to: [card.uid] };
  assert.throws(() => parseLibraryCatalog(bad), /Invalid library source graph/);
});

test("raw float tokens in identity are not silently coerced to integers", async () => {
  resetLibraryCatalogForTest();
  const original = globalThis.fetch;
  const text = JSON.stringify(document).replace(/"duration_ms":(\d+)/, '"duration_ms":$1.0');
  globalThis.fetch = async () => new Response(text);
  try {
    await assert.rejects(loadLibraryCatalog(), /float_in_identity/);
  } finally {
    globalThis.fetch = original;
    resetLibraryCatalogForTest();
  }
});

test("comparison cubes never replace the primary cube or its model row", () => {
  const doc = structuredClone(document);
  const primary = doc.assets.find((asset: any) => asset.kind === "cube_ihdr" && asset.fields.layer_method === "library_r3");
  const comparison = structuredClone(primary);
  comparison.legacy_id = `${primary.legacy_id}.test-comparison`;
  comparison.fields.layer_method = "pipeline_r2";
  comparison.fields.cube_revision = 2;
  comparison.provenance.layer_method = "pipeline_r2";
  comparison.provenance.params.layer_method = "pipeline_r2";
  comparison.uid = envelopeIdentityUid(comparison);
  comparison.display.glyph = glyphFromUid(comparison.uid);
  doc.assets.unshift(comparison);
  doc.legacy_index[`cube_ihdr:${comparison.legacy_id}`] = comparison.uid;
  const catalog = parseLibraryCatalog(doc);
  assert.equal(catalog.derivedFrom(primary.src[0], "cube_ihdr")?.uid, primary.uid);
  assert.ok(catalog.compareCubesFor(primary.src[0]).some((cube) => cube.uid === comparison.uid));
  assert.equal(sourceSha256(comparison), sourceSha256(primary));
  const clip = catalog.byUid.get(primary.src[0])!;
  const model = catalog.byUid.get(String(clip.provenance?.voice_model))!;
  const rows = catalog.modelCubes(model.legacy_id!);
  assert.equal(rows.state, "cubes");
  if (rows.state === "cubes") {
    assert.ok(rows.rows.some((row) => row.cubeUid === primary.uid));
    assert.ok(rows.rows.every((row) => row.cubeUid !== comparison.uid));
  }
  const nextPrimary = structuredClone(primary);
  nextPrimary.fields.layer_method = "library_r4";
  assert.equal(comparisonMethod(nextPrimary), null, "new primary revisions are not comparisons");
});
