import test from "node:test";
import assert from "node:assert/strict";
import { createCubeModeController, cubeStateFrom, SINGLE } from "../src/cube-mode.ts";

const pair = { tileId: "lib-kokoro", clip_uid: `ga:audio_clip:${"a".repeat(26)}`,
  left_cube_uid: `ga:cube_ihdr:${"b".repeat(26)}`, right_cube_uid: `ga:cube_ihdr:${"c".repeat(26)}`,
  left_method: "library_r3", right_method: "pipeline_r2" };
const compare = { cube_mode: "compare" as const, cube_compare: pair };

test("cube protocol validates exact methods and full UIDs", () => {
  assert.deepEqual(cubeStateFrom(compare), compare);
  assert.equal(cubeStateFrom({ ...compare, cube_compare: { ...pair, clip_uid: "fake" } }), null);
  assert.equal(cubeStateFrom({ ...compare, cube_compare: { ...pair, right_method: "library_r3" } }), null);
  assert.equal(cubeStateFrom({ cube_mode: "single", cube_compare: pair }), null);
});

test("Compare posts ui_cube; response and bus echo draw once, then serialize Single", async () => {
  const calls: unknown[] = [], draws: string[] = [];
  const controller = createCubeModeController({
    async enter() { draws.push("compare"); return { ok: true }; },
    exit() { draws.push("single"); },
  }, async (name, args) => {
    calls.push({ name, args });
    return { body: { args: args.mode === "compare" ? compare : SINGLE } };
  });
  await controller.request("compare", pair.tileId);
  await controller.applyControl(compare);
  await controller.applyControl(SINGLE);
  assert.deepEqual(draws, ["compare", "single"]);
  assert.deepEqual(calls, [{ name: "ui_cube", args: { mode: "compare", tileId: pair.tileId } }]);
});

test("failed comparison drawing repairs server mode, without inventing local success", async () => {
  const calls: string[] = [];
  const controller = createCubeModeController({
    async enter() { return { ok: false, reason: "different WAV" }; }, exit() {},
  }, async (_name, args) => {
    calls.push(String(args.mode));
    return { body: { args: args.mode === "single" ? SINGLE : compare } };
  });
  assert.equal((await controller.request("compare", pair.tileId)).ok, false);
  assert.deepEqual(calls, ["compare", "single"]);
  assert.deepEqual(controller.state(), SINGLE);
});

test("a slow Compare never lands after a later Single event", async () => {
  const draws: string[] = [];
  let ready!: () => void;
  const wait = new Promise<void>(resolve => { ready = resolve; });
  const controller = createCubeModeController({
    async enter() { await wait; draws.push("compare"); return { ok: true }; },
    exit() { draws.push("single"); },
  }, async () => null);
  const first = controller.applyControl(compare), second = controller.applyControl(SINGLE);
  ready();
  await Promise.all([first, second]);
  assert.deepEqual(draws, ["compare", "single"]);
  assert.equal(controller.state().cube_mode, "single");
});
