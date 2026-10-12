import test from "node:test";
import assert from "node:assert/strict";
import { checkPair, sideFromDoc, sharedAxes, sharedPoint, slicesAt, type CompareSide } from "../src/cube-compare.ts";
import { clearCube, getComparePlayheads, loadCompareCube, loadCube, setCubeScrub } from "../src/cubeview.ts";

const sha = "1".repeat(64);
const doc = (method = "library_r3") => ({
  layer_method: method, source_sha256: sha, layer_score: 0.6, inv_hdr: 0.7,
  duration_s: 100, sample_rate: 24000, n_fft: 1024, bin_seconds: 0.25,
  cube_covers_s: 80, cube_shape_f_t: [20, 320], downsample_sf_st: [2, 3],
  layers: ["signal", "tonality", "confidence", "quality"],
  points_preview: ["signal", "tonality", "confidence", "quality"].map((layer, index) => ({ t: 0.5, f: 0.5, z: index / 3, v: 0.5, layer })),
});
const side = (method: string) => {
  const value = sideFromDoc(method, doc(method), sha);
  assert.notEqual(typeof value, "string");
  return value as CompareSide;
};

test("comparison rejects different WAVs, forged hashes, unknown methods and nonfinite measures", () => {
  const left = side("library_r3"), right = side("pipeline_r2");
  assert.equal(checkPair(left, right).ok, true);
  assert.equal(checkPair(left, { ...right, sourceSha256: "2".repeat(64) }).ok, false);
  assert.equal(checkPair(left, { ...right, durationS: 99 }).ok, false);
  assert.equal(checkPair(left, left).ok, false);
  assert.equal(typeof sideFromDoc("x", doc(), "2".repeat(64)), "string");
  assert.equal(typeof sideFromDoc("x", doc("unknown"), sha), "string");
  assert.equal(typeof sideFromDoc("x", { ...doc(), layer_score: NaN }, sha), "string");
});

test("differently binned cubes share physical seconds/Hz and exactly one slice", () => {
  const sides = [side("library_r3"), { ...side("pipeline_r2"), binSeconds: 0.5, timeBins: 160 }];
  const axes = sharedAxes(sides);
  const heads = slicesAt(40, sides, axes);
  assert.deepEqual(heads.map(head => head.seconds), [40, 40]);
  assert.deepEqual(heads.map(head => head.x), [0.4, 0.4]);
  assert.deepEqual(heads.map(head => head.bin), [160, 80]);
  assert.equal(slicesAt(90, sides, axes).every(head => head.beyond), true);
  assert.equal(sharedPoint(sides[0], axes, 1, 1).x, 79.75 / 100);
});

test("renderer comparison follows primary scrub and refuses mismatched audio", async () => {
  const original = globalThis.fetch;
  Object.assign(globalThis, { document: { querySelector: () => null } });
  globalThis.fetch = async url => new Response(JSON.stringify(doc(String(url).includes("pipeline") ? "pipeline_r2" : "library_r3")));
  try {
    await loadCube("/library/left.json");
    assert.equal((await loadCompareCube("/library/pipeline.json", { primary: sha, compare: sha })).ok, true);
    setCubeScrub(0.45, { silent: true });
    assert.deepEqual(getComparePlayheads()?.map(head => head.seconds), [45, 45]);
    clearCube("none");
    assert.equal(getComparePlayheads(), null);
    assert.equal((await loadCompareCube("/library/pipeline.json", { primary: sha, compare: sha })).ok, false);
  } finally { globalThis.fetch = original; }
});

test("a late failed cube load cannot erase a newer successful binding", async () => {
  const original = globalThis.fetch;
  let rejectOld!: (error: Error) => void;
  Object.assign(globalThis, { document: { querySelector: () => null } });
  globalThis.fetch = async url => String(url).includes("old")
    ? await new Promise<Response>((_, reject) => { rejectOld = reject; })
    : new Response(JSON.stringify(doc()));
  try {
    const old = loadCube("/library/old.json");
    await loadCube("/library/new.json");
    rejectOld(new Error("old fetch failed"));
    assert.match(await old, /superseded/);
    assert.equal((await loadCompareCube("/library/pipeline.json", { primary: sha, compare: sha })).ok, false);
  } finally { globalThis.fetch = original; clearCube("done"); }
});
