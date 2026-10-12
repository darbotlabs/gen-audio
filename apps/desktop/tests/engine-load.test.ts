import assert from "node:assert/strict";
import { test } from "node:test";
import { createEngineLoader, type EngineLoadState } from "../src/engine-load";

test("confirmed GPU details come from the loaded runtime receipt", async () => {
  const states: EngineLoadState[] = [];
  await createEngineLoader(async engine => ({ engine, loaded: true, pid: 123,
    execution: { backend: "rocm", precision: "bfloat16", hardware: { name: "Radeon 8060S" } } }),
    state => states.push(state)).load("vibevoice");
  assert.deepEqual(states.at(-1)?.execution, { backend: "rocm", precision: "bfloat16", hardware: "Radeon 8060S" });
});

test("model selection stays loading until an owned model receipt confirms loaded weights", async () => {
  const states: EngineLoadState[] = [];
  let finish!: (value: Record<string, unknown>) => void;
  const loader = createEngineLoader(async engine => { assert.equal(engine, "kokoro_onnx"); return new Promise(resolve => { finish = resolve; }); }, state => states.push(state));
  const pending = loader.load("kokoro_onnx");
  assert.deepEqual(states, [{ engine: "kokoro_onnx", phase: "loading" }]);
  await Promise.resolve();
  finish({ engine: "kokoro_onnx", loaded: true, pid: 123 });
  await pending;
  assert.deepEqual(states.at(-1), { engine: "kokoro_onnx", phase: "loaded", pid: 123 });
});

test("a readiness report or failed load cannot masquerade as resident model weights", async () => {
  for (const result of [
    { engine: "vibevoice", ready: true, loaded: false, errors: [{ message: "Insufficient RAM" }] },
    { engine: "vibevoice", loaded: true },
    { engine: "kokoro_onnx", loaded: true, pid: 123 },
  ]) {
    const states: EngineLoadState[] = [];
    await createEngineLoader(async () => result, state => states.push(state)).load("vibevoice");
    assert.equal(states.at(-1)?.phase, "failed");
    assert.equal(states.some(state => state.phase === "loaded"), false);
  }
});

test("a late load response cannot overwrite a newer engine selection", async () => {
  const states: EngineLoadState[] = [];
  const finishes = new Map<string, (value: Record<string, unknown>) => void>();
  const loader = createEngineLoader(engine => new Promise(resolve => { finishes.set(engine, resolve); }), state => states.push(state));
  const first = loader.load("vibevoice");
  await Promise.resolve();
  const second = loader.load("kokoro_onnx");
  assert.equal(finishes.has("kokoro_onnx"), false, "resident loads must not compete for memory");
  finishes.get("vibevoice")!({ engine: "vibevoice", loaded: true, pid: 456 });
  await first;
  await Promise.resolve();
  finishes.get("kokoro_onnx")!({ engine: "kokoro_onnx", loaded: true, pid: 123 });
  await second;
  assert.deepEqual(states.at(-1), { engine: "kokoro_onnx", phase: "loaded", pid: 123 });
});

test("backend unload or disconnect clears the confirmed loaded state", async () => {
  const states: EngineLoadState[] = [];
  const loader = createEngineLoader(async engine => ({ engine, loaded: true, pid: 123 }), state => states.push(state));
  await loader.load("kokoro_onnx");
  loader.observe({ loaded: false, phase: "unloaded" });
  assert.equal(states.at(-1)?.phase, "failed");
  loader.observe({ engine: "kokoro_onnx", loaded: true, pid: 456 });
  assert.equal(states.at(-1)?.pid, 456);
  loader.unavailable();
  assert.equal(states.at(-1)?.phase, "failed");
});

test("generation waits for loading and disconnect invalidates a pending confirmation", async () => {
  const states: EngineLoadState[] = [];
  let finish!: (value: Record<string, unknown>) => void;
  const loader = createEngineLoader(() => new Promise(resolve => { finish = resolve; }), state => states.push(state));
  void loader.load("kokoro_onnx");
  let settled = false;
  const waiting = loader.wait().then(() => { settled = true; });
  await Promise.resolve();
  assert.equal(settled, false);
  loader.unavailable();
  finish({ engine: "kokoro_onnx", loaded: true, pid: 123 });
  await waiting;
  assert.equal(settled, true);
  assert.equal(states.at(-1)?.phase, "failed");
  assert.equal(states.some(state => state.phase === "loaded"), false);
});
