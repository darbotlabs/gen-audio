// Cube tab mode over MCP (rule 5): the Compare button and an agent's ui_cube
// go through one setter, and the state matches viewport_get. Contract shared
// with the Rust tests: schemas/examples/ui_cube.contract.json. The cubes are
// the committed library_r3 / pipeline_r2 JSON (real data, no fixtures).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  CUBE_CONTROL_OP,
  MCP_DOWN,
  createCubeModeController,
  cubeModeRequest,
  cubeStateFrom,
  type CubeCompareState,
  type CubeModeEffects,
  type ToolReply,
} from "../src/cube-mode.ts";

const ROOT = fileURLToPath(new URL("../../../", import.meta.url));
const LIBRARY = `${ROOT}apps/desktop/public/library/`;
const contract = JSON.parse(readFileSync(`${ROOT}schemas/examples/ui_cube.contract.json`, "utf8"));
const ENTER = contract.enter.state;
const EXIT = contract.exit.state;

type Envelope = { uid: string; kind: string; media: Array<{ role: string; path: string }>; fields: Record<string, unknown> };
const assets: Envelope[] = JSON.parse(readFileSync(`${LIBRARY}assets.json`, "utf8")).assets;
const byUid = (uid: string) => {
  const found = assets.find((asset) => asset.uid === uid);
  assert.ok(found, `assets.json has ${uid}`);
  return found;
};
const cubeUrl = (uid: string) => `/library/${byUid(uid).media.find((m) => m.role === "cube_json")!.path}`;
const sha = (uid: string) => String(byUid(uid).fields.source_sha256);

// cubeview touches the DOM only through querySelector and rAF.
const globals = globalThis as unknown as Record<string, unknown>;
globals.document = { querySelector: () => null, querySelectorAll: () => [] };
globals.window = { requestAnimationFrame: () => 1, cancelAnimationFrame: () => {} };
globals.fetch = async (url: string) => {
  try {
    const body = readFileSync(`${LIBRARY}${String(url).replace(/^\/library\//, "")}`, "utf8");
    return { ok: true, status: 200, json: async () => JSON.parse(body) };
  } catch {
    return { ok: false, status: 404, json: async () => null };
  }
};

/** Window effects over the real cubeview: bind the clip's Library cube, then load the comparison cube. */
async function realEffects(log: string[]): Promise<CubeModeEffects & { cube: typeof import("../src/cubeview.ts") }> {
  const cube = await import("../src/cubeview.ts");
  return {
    cube,
    async enter(pair: CubeCompareState) {
      log.push(`enter ${pair.tileId}`);
      await cube.loadCube(cubeUrl(pair.left_cube_uid));
      const result = await cube.loadCompareCube(cubeUrl(pair.right_cube_uid), { primary: sha(pair.left_cube_uid), compare: sha(pair.right_cube_uid) });
      return result.ok ? { ok: true } : { ok: false, reason: result.reason };
    },
    exit() {
      log.push("exit");
      cube.exitCompare();
    },
  };
}

/** A stand-in for the MCP listener that answers with the contract states the Rust tests pin. */
function server(seqStart = 0) {
  const calls: Array<{ name: string; args: Record<string, unknown> }> = [];
  let seq = seqStart;
  let state: unknown = EXIT;
  let cubeSeq = 0;
  const post = async (name: string, args: Record<string, unknown>): Promise<ToolReply> => {
    calls.push({ name, args });
    if (name === "viewport_get") return { body: { ok: true, cube_seq: cubeSeq, ...(state as object) } };
    if (name !== "ui_cube") return { error: `unknown tool ${name}` };
    if (args.mode === "single") state = EXIT;
    else if (args.mode === "compare" && (args.tileId === "lib-misaki-kokoro" || (args.tileId === undefined && state === ENTER))) state = ENTER;
    else return { error: `no comparison cube for ${String(args.tileId)} in assets.json` };
    cubeSeq = ++seq;
    return { body: { ok: true, op: "cube", seq: cubeSeq, args: state } };
  };
  return { post, calls };
}

test("the Compare button posts exactly the contract's ui_cube call; states parse strictly", () => {
  assert.deepEqual({ name: cubeModeRequest("compare", "lib-misaki-kokoro").name, arguments: cubeModeRequest("compare", "lib-misaki-kokoro").args }, contract.enter.call);
  assert.deepEqual({ name: cubeModeRequest("single").name, arguments: cubeModeRequest("single").args }, contract.exit.call);
  assert.deepEqual(cubeStateFrom(ENTER), ENTER);
  assert.deepEqual(cubeStateFrom(EXIT), EXIT);
  assert.equal(CUBE_CONTROL_OP, "cube");
  // viewport_get / the bus event must carry the field; a missing one is not "single".
  assert.equal(cubeStateFrom({ cube_mode: "single" }), null);
  assert.equal(cubeStateFrom({ cube_mode: "compare" }), null);
  assert.equal(cubeStateFrom({ cube_mode: "side-by-side", cube_compare: null }), null);
  const { right_method: _drop, ...noRight } = ENTER.cube_compare;
  assert.equal(cubeStateFrom({ cube_mode: "compare", cube_compare: noRight }), null);
  assert.equal(cubeStateFrom({ cube_mode: "compare", cube_compare: { ...ENTER.cube_compare, right_method: "library_r3" } }), null);
  assert.equal(cubeStateFrom({ cube_mode: "compare", cube_compare: { ...ENTER.cube_compare, clip_uid: "lib-misaki-kokoro" } }), null);
});

test("the button and an agent's ui_cube produce the same state, through the same setter, and draw Compare", async () => {
  const buttonLog: string[] = [];
  const fx = await realEffects(buttonLog);
  const mcp = server();
  const button = createCubeModeController(fx, mcp.post);
  const pressed = await button.request("compare", "lib-misaki-kokoro");
  assert.equal(pressed.ok, true, JSON.stringify(pressed));
  assert.deepEqual(mcp.calls, [{ name: "ui_cube", args: contract.enter.call.arguments }]);
  assert.deepEqual(button.state(), ENTER);
  assert.equal(fx.cube.isCompareOn(), true, "the handler drew Compare");
  assert.deepEqual(fx.cube.getCompareSides()!.map((side) => side.method), ["library_r3", "pipeline_r2"]);

  // Our own bus echo (same state) is not drawn twice.
  await button.applyControl(ENTER);
  assert.deepEqual(buttonLog, ["enter lib-misaki-kokoro"]);

  // An agent's ui_cube reaches another window as the `cube` bus event.
  const agentLog: string[] = [];
  const agent = createCubeModeController(await realEffects(agentLog), server().post);
  const viaBus = await agent.applyControl(ENTER);
  assert.equal(viaBus.ok, true);
  assert.deepEqual(agent.state(), button.state(), "button and op land on the same state");
  assert.deepEqual(agentLog, buttonLog, "and run the same effects");

  // Back to single: one more ui_cube, the second pane goes away.
  const back = await button.request("single");
  assert.equal(back.ok, true);
  assert.deepEqual(button.state(), EXIT);
  assert.equal(fx.cube.isCompareOn(), false);
  assert.deepEqual(mcp.calls.at(-1), { name: "ui_cube", args: contract.exit.call.arguments });
});

test("viewport_get resyncs the window; a reply without the cube fields is refused", async () => {
  const log: string[] = [];
  const fx = await realEffects(log);
  const mcp = server();
  await mcp.post("ui_cube", contract.enter.call.arguments); // an agent set Compare before this window opened
  const window = createCubeModeController(fx, mcp.post);
  const synced = await window.sync();
  assert.equal(synced.ok, true, JSON.stringify(synced));
  assert.deepEqual(window.state(), ENTER);
  assert.equal(fx.cube.isCompareOn(), true);
  // The bus replay of that same event is not drawn again.
  await window.applyControl(ENTER);
  assert.deepEqual(log, ["enter lib-misaki-kokoro"]);

  const omits = createCubeModeController(fx, async () => ({ body: { ok: true, cube_seq: 4, cube_mode: "compare" } }));
  const refused = await omits.sync();
  assert.equal(refused.ok, false);
  assert.match((refused as { reason: string }).reason, /no cube_mode \/ cube_compare/);
  const unregistered = createCubeModeController(fx, async (name) => ({ error: `unknown tool ${name}` }));
  assert.match(((await unregistered.request("compare", "lib-misaki-kokoro")) as { reason: string }).reason, /unknown tool ui_cube/);
  assert.match(((await unregistered.sync()) as { reason: string }).reason, /unknown tool viewport_get/);
});

test("MCP down: nothing changes; a Compare the window cannot draw is handed back as single", async () => {
  const log: string[] = [];
  const down = createCubeModeController(await realEffects(log), async () => null);
  const result = await down.request("compare", "lib-misaki-kokoro");
  assert.deepEqual(result, { ok: false, reason: MCP_DOWN, state: EXIT });
  assert.deepEqual(log, []);

  const mcp = server();
  const failing: CubeModeEffects = { enter: async () => ({ ok: false, reason: "Comparison cube JSON did not load (404)." }), exit: () => log.push("exit") };
  const window = createCubeModeController(failing, mcp.post);
  const shown = await window.applyControl(ENTER);
  assert.equal(shown.ok, false);
  assert.deepEqual(window.state(), EXIT);
  assert.deepEqual(mcp.calls, [{ name: "ui_cube", args: { mode: "single" } }], "the server is told, so viewport_get stays true");
});

test("rebinding follows Compare to a clip that has a pair, and leaves it (saying why) for one that has none", async () => {
  const log: string[] = [];
  const mcp = server();
  const window = createCubeModeController(await realEffects(log), mcp.post);
  assert.equal(await window.rebound("lib-kokoro-onnx"), null, "single mode ignores rebinds");
  await window.request("compare", "lib-misaki-kokoro");
  const same = await window.rebound("lib-misaki-kokoro");
  assert.equal(same!.ok, true);
  assert.deepEqual(log, ["enter lib-misaki-kokoro", "enter lib-misaki-kokoro"], "a reload of the same clip redraws the pair");
  const moved = await window.rebound("lib-kokoro-onnx");
  assert.equal(moved!.ok, false);
  assert.match((moved as { reason: string }).reason, /no comparison cube for lib-kokoro-onnx/);
  assert.deepEqual(window.state(), EXIT);
  assert.deepEqual(mcp.calls.slice(-2).map((call) => call.args), [{ mode: "compare", tileId: "lib-kokoro-onnx" }, { mode: "single" }]);
});

test("changes apply one at a time in bus order (a slow compare cannot land after the single that followed it)", async () => {
  const order: string[] = [];
  let release: () => void = () => {};
  const slow: CubeModeEffects = {
    enter: () => new Promise((resolve) => {
      release = () => {
        order.push("enter done");
        resolve({ ok: true });
      };
    }),
    exit: () => order.push("exit"),
  };
  const window = createCubeModeController(slow, server().post);
  const first = window.applyControl(ENTER);
  const second = window.applyControl(EXIT);
  await new Promise((resolve) => setTimeout(resolve, 5));
  release();
  await Promise.all([first, second]);
  assert.deepEqual(order, ["enter done", "exit"]);
  assert.deepEqual(window.state(), EXIT);
});

test("main.ts wiring: the bus routes `cube` to the controller and the Compare button posts ui_cube (no local flag)", () => {
  const main = readFileSync(`${ROOT}apps/desktop/src/main.ts`, "utf8");
  assert.match(main, /event\.op === CUBE_CONTROL_OP\) \{\s*void cubeMode\.applyControl\(args\)/);
  const button = main.slice(main.indexOf('"#cube-compare-toggle")?.addEventListener'));
  assert.match(button.slice(0, 400), /cubeMode\.request\(next, cubeClipId \|\| null\)/);
  assert.doesNotMatch(main, /compareWanted/);
  // enterCompare runs only from the controller's enter effect.
  assert.equal(main.match(/\benterCompare\(/g)?.length, 2, "one definition and one call");
  assert.match(main, /return enterCompare\(pair\);/);
  assert.match(main, /cubeMode\.sync\(\)/);
});
