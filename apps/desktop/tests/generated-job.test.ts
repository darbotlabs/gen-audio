import { test } from "node:test";
import assert from "node:assert/strict";
import { generatedJob } from "../src/generated-job.ts";
const clipUid = "ga:audio_clip:aaaaaaaaaaaaaaaaaaaaaaaaaa";

test("generated and derived events identify actual assets and their source clip", () => {
  assert.deepEqual(generatedJob({ phase: "done", target: clipUid }), { kind: "clip", clipUid });
  for (const [assetKind, kind] of [["spectrogram_2d", "spectrogram"], ["cube_ihdr", "cube"]]) {
    assert.deepEqual(generatedJob({ phase: "done", target: `ga:${assetKind}:aaaaaaaaaaaaaaaaaaaaaaaaaa`, clipUid }), { kind, clipUid });
  }
  assert.deepEqual(generatedJob({ phase: "done", target: clipUid, kind: "video" }), { kind: "video", clipUid });
  for (const suffix of ["spectrogram", "cube", "video"]) {
    assert.throws(() => generatedJob({ phase: "done", target: `${clipUid}:${suffix}` }), /valid asset/);
  }
  assert.throws(() => generatedJob({ phase: "done", target: "ga:cube_ihdr:aaaaaaaaaaaaaaaaaaaaaaaaaa" }), /source clip/);
  assert.equal(generatedJob({ phase: "running" }), null);
});
