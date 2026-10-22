// Cube tab Compare mode: the misaki Library cube (library_r3) beside the same
// WAV's pipeline_r2 cube, driven by ONE clock. Uses the committed cube JSON
// files and asset envelopes from public/library (real data, no fixtures).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  checkPair,
  layerMethodOf,
  LAYER_METHOD_LABELS,
  paneHeading,
  sideFromDoc,
  sliceReadout,
  slicesAt,
  sharedAxes,
  type CompareSide,
} from "../src/cube-compare.ts";

const LIBRARY = fileURLToPath(new URL("../public/library/", import.meta.url));
const read = (name: string) => JSON.parse(readFileSync(`${LIBRARY}${name}`, "utf8"));
const MISAKI = "library_genaid_full_misaki_kokoro_cube3d.json";
const MISAKI_R2 = "library_genaid_full_misaki_kokoro_pipeline_r2_cube3d.json";
const BITDOT = "library_bitdot_braille_vibevoice_cube3d.json";
const BITDOT_R2 = "library_bitdot_braille_vibevoice_pipeline_r2_cube3d.json";

const assets: Array<{ kind: string; media: Array<{ role: string; path: string }>; fields: Record<string, unknown>; provenance?: { params?: Record<string, unknown> } }> =
  read("assets.json").assets;
function envelopeSha(jsonName: string): string {
  const cube = assets.find((asset) => asset.kind === "cube_ihdr" && asset.media.some((m) => m.role === "cube_json" && m.path === jsonName));
  assert.ok(cube, `assets.json has a cube_ihdr for ${jsonName}`);
  return String(cube.fields.source_sha256);
}

// cubeview/playback touch the DOM only through querySelector and rAF.
const frames: Array<() => void> = [];
const globals = globalThis as unknown as Record<string, unknown>;
globals.document = { querySelector: () => null, querySelectorAll: () => [] };
globals.window = {
  requestAnimationFrame: (callback: () => void) => {
    frames.push(callback);
    return frames.length;
  },
  cancelAnimationFrame: () => {},
};
globals.fetch = async (url: string) => {
  const name = String(url).replace(/^\/library\//, "");
  try {
    const body = readFileSync(`${LIBRARY}${name}`, "utf8");
    return { ok: true, status: 200, json: async () => JSON.parse(body) };
  } catch {
    return { ok: false, status: 404, json: async () => null };
  }
};
/** Run the frame the live cube clock queued (one rAF tick). */
function tick(): void {
  const pending = frames.splice(0);
  for (const callback of pending) callback();
}

function side(name: string): CompareSide {
  const result = sideFromDoc(`/library/${name}`, read(name), envelopeSha(name));
  assert.equal(typeof result, "object", String(result));
  return result as CompareSide;
}

test("layer methods are read from the cube JSON, never guessed", () => {
  assert.equal(layerMethodOf(read(MISAKI)), "library_r3");
  assert.equal(layerMethodOf(read(MISAKI_R2)), "pipeline_r2");
  // Every Library cube now names its method in provenance (#5 E4: all five at rev 3).
  assert.equal(layerMethodOf(read("library_kokoro_onnx_cube3d.json")), "library_r3");
  // Never guessed: a JSON that records no method is unknown, layer_score or not.
  assert.equal(layerMethodOf({ layer_score: 0.25 }), null);
  assert.equal(layerMethodOf({ provenance: { layer_method: "pipeline_r9" } }), null);
  assert.equal(LAYER_METHOD_LABELS.library_r3, "Library formulas, rev 3");
  assert.equal(LAYER_METHOD_LABELS.pipeline_r2, "Pipeline formulas, rev 2 (PR #4)");
});

test("pane headings carry the exact label, inv_hdr and layer_score of each cube", () => {
  const left = side(MISAKI);
  const right = side(MISAKI_R2);
  assert.equal(paneHeading(left), `Library formulas, rev 3 \u00b7 inv_hdr ${read(MISAKI).inv_hdr.toFixed(4)} \u00b7 layer_score ${read(MISAKI).layer_score.toFixed(4)}`);
  assert.match(paneHeading(right), /^Pipeline formulas, rev 2 \(PR #4\) \u00b7 inv_hdr 0\.0702 \u00b7 layer_score 0\.\d{4}$/);
  assert.equal(left.invHdr, right.invHdr, "inv_hdr is rms/peak of the same WAV for both methods");
  assert.deepEqual(checkPair(left, right), { ok: true });
});

test("a mismatched source_sha256 is refused", () => {
  const left = side(MISAKI);
  const other = side(BITDOT_R2);
  const verdict = checkPair(left, other);
  assert.equal(verdict.ok, false);
  assert.match((verdict as { reason: string }).reason, /different WAVs/);
  // The JSON's own sha must agree with its envelope's.
  const forged = { ...read(MISAKI_R2), source_sha256: "0".repeat(64) };
  assert.match(String(sideFromDoc(`/library/${MISAKI_R2}`, forged, envelopeSha(MISAKI_R2))), /asset envelope says/);
  // A library_r3 JSON records its WAV's sha (#5 E4), so it matches without the envelope;
  // a cube JSON with no sha and no envelope cannot be matched.
  assert.equal((sideFromDoc(`/library/${MISAKI}`, read(MISAKI), null) as CompareSide).sourceSha256, read(MISAKI).source_sha256);
  const { source_sha256: _dropped, ...noSha } = read(MISAKI);
  assert.match(String(sideFromDoc(`/library/${MISAKI}`, noSha, null)), /no source_sha256/);
});

test("seek maps seconds to each cube's own bin on one shared slice (bitdot: 0.619 s vs 0.352 s bins)", () => {
  const sides = [side(BITDOT), side(BITDOT_R2)];
  assert.deepEqual(checkPair(sides[0], sides[1]), { ok: true });
  const axes = sharedAxes(sides);
  const [library, pipeline] = slicesAt(100, sides, axes);
  assert.equal(library.x, pipeline.x, "one slice: same x on both cubes");
  assert.equal(library.seconds, 100);
  assert.equal(library.bin, Math.floor(100 / read(BITDOT).bin_seconds));
  assert.equal(pipeline.bin, Math.floor(100 / read(BITDOT_R2).bin_seconds));
  assert.notEqual(library.bin, pipeline.bin);
  // Past cube_covers_s the bin clamps and the slice says so.
  const end = slicesAt(read(BITDOT).duration_s, sides, axes);
  assert.equal(end[0].bin, read(BITDOT).cube_shape_f_t[1] - 1);
  assert.equal(end[1].bin, read(BITDOT_R2).cube_shape_f_t[1] - 1);
  assert.ok(end.every((head) => head.beyond));
  assert.match(sliceReadout(end, read(BITDOT).duration_s), /^Slice 4:07\.0 \/ 4:07\.0 \u00b7 library_r3 bin 398 \/ 398 \(beyond cube\) \u00b7 pipeline_r2 bin 700 \/ 700 \(beyond cube\)$/);
});

test("one clock drives both slices: play, pause, seek and scrub; drift 0", async () => {
  const cube = await import("../src/cubeview.ts");
  const playback = await import("../src/playback.ts");
  const { Seeker } = await import("../src/seek.ts");
  // #5's verified seeks (seek.ts): a fully seekable element that fires `seeked`, no settle delay.
  playback.setSeeker(new Seeker(undefined, { timeoutMs: 500, settleMs: 0 }));
  const listeners = new Map<string, Set<() => void>>();
  let time = 0;
  const audio = {
    src: `/library/genaid_full_misaki_kokoro.wav`,
    paused: true,
    ended: false,
    get currentTime() {
      return time;
    },
    set currentTime(value: number) {
      time = value;
      queueMicrotask(() => listeners.get("seeked")?.forEach((listener) => listener()));
    },
    duration: read(MISAKI).duration_s,
    readyState: 4,
    seekable: { length: 1, start: () => 0, end: () => read(MISAKI).duration_s },
    addEventListener(type: string, listener: () => void) {
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type)!.add(listener);
    },
    removeEventListener(type: string, listener: () => void) {
      listeners.get(type)?.delete(listener);
    },
    load() {},
    async play() {
      audio.paused = false;
    },
    pause() {
      audio.paused = true;
    },
  };
  playback.registerPlayer("lib-misaki-kokoro", audio as unknown as HTMLAudioElement);
  playback.setCubeClockClip("lib-misaki-kokoro", "ga:audio_clip:vtwxksrsuci7zygslimzfy7kdy");
  assert.match(await cube.loadCube(`/library/${MISAKI}`), /inv_hdr 0\.0702/);

  // Refused pair: the bitdot comparison cube is another WAV. Nothing is drawn in pane two.
  const refused = await cube.loadCompareCube(`/library/${BITDOT_R2}`, { primary: envelopeSha(MISAKI), compare: envelopeSha(BITDOT_R2) });
  assert.equal(refused.ok, false);
  assert.match((refused as { reason: string }).reason, /different WAVs/);
  assert.equal(cube.isCompareOn(), false);
  assert.equal(cube.getComparePlayheads(), null);
  // A missing comparison JSON is refused too; no fallback cube.
  const missing = await cube.loadCompareCube("/library/library_nope_pipeline_r2_cube3d.json", { primary: envelopeSha(MISAKI), compare: envelopeSha(MISAKI_R2) });
  assert.equal(missing.ok, false);
  assert.equal(cube.isCompareOn(), false);

  const entered = await cube.loadCompareCube(`/library/${MISAKI_R2}`, { primary: envelopeSha(MISAKI), compare: envelopeSha(MISAKI_R2) });
  assert.equal(entered.ok, true, JSON.stringify(entered));
  assert.equal(cube.isCompareOn(), true);
  const [left, right] = cube.getCompareSides()!;
  assert.deepEqual([left.method, right.method], ["library_r3", "pipeline_r2"]);

  const binSeconds = read(MISAKI).bin_seconds;
  const expectBoth = (seconds: number) => {
    const heads = cube.getComparePlayheads()!;
    assert.equal(heads.length, 2);
    assert.equal(heads[0].seconds, heads[1].seconds, "one clock");
    assert.equal(heads[0].x - heads[1].x, 0, "slice drift between the two views is 0");
    assert.ok(Math.abs(heads[0].seconds - seconds) < 1e-9, `slice at ${heads[0].seconds}, audio at ${seconds}`);
    assert.equal(heads[0].bin, Math.floor((seconds + 1e-9) / binSeconds));
    assert.equal(heads[1].bin, heads[0].bin, "misaki: both methods bin 0.352 s");
    // The single-cube playhead reads the same clock.
    assert.equal(cube.getCubePlayhead()!.bin, heads[0].bin);
    return heads;
  };

  // Play: the live cube clock reads the bound clip's currentTime every frame.
  assert.equal(await playback.playClip("lib-misaki-kokoro", "user"), "playing");
  audio.currentTime = 42.5;
  tick();
  expectBoth(42.5);
  audio.currentTime = 43.25;
  tick();
  const playing = expectBoth(43.25);

  // Pause holds both slices even if the element's time moves.
  playback.pauseClip("lib-misaki-kokoro");
  audio.currentTime = 90;
  tick();
  assert.deepEqual(cube.getComparePlayheads(), playing);

  // Seek (MCP ui_playback seek / UI) moves both, paused or not.
  // Seeks are verified since #5: the result says where the clock landed.
  assert.equal(await playback.seekClip("lib-misaki-kokoro", 100), "seeked to 1:40");
  expectBoth(100);

  // Scrub the shared cube scrubber: one fraction, both slices.
  cube.setCubeScrub(0.5);
  expectBoth(0.5 * read(MISAKI).duration_s);
  // The scrub's seek is async (seek.ts); let it land on the same clock before moving on.
  await new Promise((resolve) => setTimeout(resolve, 0));
  expectBoth(0.5 * read(MISAKI).duration_s);

  // Leaving Compare drops the second pane; rebinding the cube also ends it.
  cube.exitCompare();
  assert.equal(cube.isCompareOn(), false);
  assert.equal((await cube.loadCompareCube(`/library/${MISAKI_R2}`, { primary: envelopeSha(MISAKI), compare: envelopeSha(MISAKI_R2) })).ok, true);
  await cube.loadCube(`/library/${BITDOT}`);
  assert.equal(cube.isCompareOn(), false, "a new bound cube ends Compare (the comparison cube was another WAV)");
});

test("assets.json: comparison cubes are real cube_ihdr envelopes of the same WAV, never the clip's own cube", () => {
  const compare = assets.filter((asset) => asset.kind === "cube_ihdr" && asset.provenance?.params?.layer_method === "pipeline_r2");
  assert.equal(compare.length, 2);
  for (const envelope of compare) {
    const roles = envelope.media.map((m) => m.role).sort();
    assert.deepEqual(roles, ["cube_json", "cube_png"], "B2: a real cube carries cube_json and cube_png");
    const json = envelope.media.find((m) => m.role === "cube_json")!.path;
    assert.equal(read(json).source_sha256, envelope.fields.source_sha256);
    assert.equal(read(json).layer_method, "pipeline_r2");
  }
});
