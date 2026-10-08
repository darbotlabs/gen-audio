// L2 (Optimus): boot the real main.ts wiring in happy-dom so the board's
// click handler and timers are actually wired. Assert body clicks and elapsed
// time never change the face. Kills M35/M36 and A-M1/C12/C13 (attach/flipcard
// flips). B8: syncCubeChrome must pass the bound cube uid into fillCubeGlyph.
// Env reads go through env.ts; bindEnv is the seam (no loader — option (a)
// temp-file loader is an antipattern).
import { after, test, mock } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Window } from "happy-dom";
import { bindEnv } from "../src/env";

bindEnv({ VITE_GEN_AUDIO_FIXTURES: undefined });

const ROOT = fileURLToPath(new URL("../../../", import.meta.url));
const HTML = readFileSync(`${ROOT}apps/desktop/index.html`, "utf8").replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, "");
const ASSETS = readFileSync(`${ROOT}apps/desktop/public/library/assets.json`, "utf8");
const CSS_TEXT = readFileSync(`${ROOT}apps/desktop/src/styles.css`, "utf8");

const happy = new Window({ url: "http://127.0.0.1/", settings: { device: { prefersReducedMotion: "no-preference" } } });
happy.document.write(HTML);
happy.document.close();
const style = happy.document.createElement("style");
style.textContent = CSS_TEXT;
happy.document.head.append(style);

for (const name of [
  "window", "document", "navigator", "CSS", "HTMLElement", "HTMLButtonElement", "HTMLInputElement",
  "HTMLCanvasElement", "HTMLSelectElement", "HTMLTextAreaElement", "Element", "Node",
  "KeyboardEvent", "MouseEvent", "Event", "Image", "requestAnimationFrame", "cancelAnimationFrame",
  "matchMedia", "getComputedStyle", "MutationObserver", "ResizeObserver", "IntersectionObserver",
]) {
  Object.defineProperty(globalThis, name, {
    configurable: true,
    writable: true,
    value: (happy as unknown as Record<string, unknown>)[name] ?? (happy.window as unknown as Record<string, unknown>)[name],
  });
}

const controlListeners: Array<(event: MessageEvent) => void> = [];
class StubEventSource {
  readyState = 2;
  addEventListener(type: string, fn: (event: MessageEvent) => void): void {
    if (type === "control") controlListeners.push(fn);
  }
  close(): void {}
  set onerror(_fn: unknown) {}
}
function fireControl(op: string, args: Record<string, unknown>, seq = 1): void {
  const data = JSON.stringify({ seq, op, args });
  for (const fn of controlListeners) fn({ data } as MessageEvent);
}
(globalThis as unknown as { EventSource: unknown }).EventSource = StubEventSource;
(happy as unknown as { EventSource: unknown }).EventSource = StubEventSource;
(happy.window as unknown as { EventSource: unknown }).EventSource = StubEventSource;

Object.defineProperty(happy.navigator, "clipboard", {
  configurable: true,
  value: { writeText: async () => {} },
});

const harvestProfile = {
  personaId: "anton",
  refs: ["persona:anton", "clip:lib-misaki-kokoro"],
};
(globalThis as unknown as { fetch: unknown }).fetch = async (url: string, init?: { body?: string }) => {
  const u = String(url);
  if (u.includes("/library/assets.json")) {
    return { ok: true, status: 200, json: async () => JSON.parse(ASSETS), text: async () => ASSETS };
  }
  if (u.includes("/mcp") && init?.body) {
    let tool = "";
    try { tool = String(JSON.parse(init.body)?.params?.name ?? ""); } catch { /* ignore */ }
    if (tool === "library_harvest") {
      const body = JSON.stringify({ profile: harvestProfile, ok: true });
      const envelope = { result: { content: [{ type: "text", text: body }] } };
      return { ok: true, status: 200, json: async () => envelope, text: async () => JSON.stringify(envelope) };
    }
    const empty = { result: { content: [{ type: "text", text: "{}" }] } };
    return { ok: true, status: 200, json: async () => empty, text: async () => JSON.stringify(empty) };
  }
  // Minimal cube JSON so loadCube sets primary.meta.url (B8 needs getCubeMeta()).
  if (u.includes("/library/") && u.endsWith(".json") && !u.includes("assets.json")) {
    const mini = {
      title: "test cube",
      inv_hdr: 0.07,
      cube_shape_f_t: [4, 4],
      duration_s: 1.0,
      layers: { signal: {}, tonality: {}, confidence: {}, quality: {} },
      points_preview: [
        { layer: "signal", bin: 0, freq: 0, value: 0.1 },
        { layer: "tonality", bin: 0, freq: 0, value: 0.1 },
        { layer: "confidence", bin: 0, freq: 0, value: 0.1 },
        { layer: "quality", bin: 0, freq: 0, value: 0.1 },
      ],
    };
    const body = JSON.stringify(mini);
    return { ok: true, status: 200, json: async () => mini, text: async () => body };
  }
  return { ok: false, status: 503, json: async () => ({}), text: async () => "" };
};

// Fake timers BEFORE importing main.ts so setInterval at module load is mocked.
mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
happy.setInterval = globalThis.setInterval as unknown as typeof happy.setInterval;
happy.setTimeout = globalThis.setTimeout as unknown as typeof happy.setTimeout;
(happy.window as unknown as { setInterval: unknown }).setInterval = globalThis.setInterval;
(happy.window as unknown as { setTimeout: unknown }).setTimeout = globalThis.setTimeout;

await import("../src/main.ts");

const render = await import("../src/render.ts");
const { loadLibraryCatalog } = await import("../src/library-assets.ts");

// N-M1: readiness waits poll against a wall-clock deadline (Date.now() is not
// mocked; only setInterval/setTimeout are), as main-env-seam.test.ts does. A
// fixed count of event-loop turns measures CPU luck: under load the dynamic
// imports and fetch stubs resolve after the turns run out ("got 0").
const READY_MS = 15_000;

async function waitForCards(min = 1): Promise<HTMLElement> {
  const board = happy.document.querySelector("#board") as unknown as HTMLElement;
  assert.ok(board, "#board from index.html");
  const started = Date.now();
  const deadline = started + READY_MS;
  let turns = 0;
  while (Date.now() < deadline) {
    if (board.querySelectorAll(".card").length >= min) return board;
    await new Promise((r) => setImmediate(r));
    turns += 1;
    if (turns % 5 === 0) mock.timers.tick(1);
  }
  assert.fail(`main.ts never rendered ${min} cards in ${Date.now() - started} ms / ${turns} turns (got ${board.querySelectorAll(".card").length})`);
}

function resetFaces(board: HTMLElement): void {
  for (const tile of board.querySelectorAll<HTMLElement>(".card")) {
    if (tile.dataset.id) render.setCardFlip(board, tile.dataset.id, false);
  }
}

const face = (tile: Element) => (tile as HTMLElement).dataset.face;
const click = (node: Element) =>
  node.dispatchEvent(new happy.MouseEvent("click", { bubbles: true, cancelable: true }) as unknown as Event);

after(() => {
  mock.timers.reset();
  void happy.happyDOM.close();
});

test("L2: main.ts wiring is live — body clicks never change face_index / face", async () => {
  const board = await waitForCards(10);
  resetFaces(board);
  const tiles = Array.from(board.querySelectorAll(".card"));
  assert.ok(tiles.length >= 10, `booted ${tiles.length} cards through main.ts`);
  assert.ok(tiles.every((t) => t.querySelector(":scope > .flip-glyph")), "every card has its flip glyph");

  for (const tile of tiles) {
    const before = face(tile);
    assert.equal(before, "front");
    const front = tile.querySelector(".face.front");
    for (const target of [tile, front, front?.querySelector("h2"), front?.querySelector("p")].filter(Boolean) as Element[]) {
      click(target);
      assert.equal(face(tile), before, `${(tile as HTMLElement).dataset.id}: body click flipped via main.ts`);
      assert.ok(!(tile as HTMLElement).classList.contains("is-flipped"), `${(tile as HTMLElement).dataset.id}: is-flipped after body click`);
    }
  }
});

test("L2: main.ts wiring is live — elapsed time never flips a card (fake timers)", async () => {
  const board = await waitForCards(10);
  resetFaces(board);
  const tiles = Array.from(board.querySelectorAll(".card"));
  for (let i = 0; i < 60; i += 1) {
    mock.timers.tick(10_000);
    for (const tile of tiles) {
      assert.equal(face(tile), "front", `${(tile as HTMLElement).dataset.id} flipped by a main.ts timer after ${(i + 1) * 10}s`);
    }
  }
});

test("L2/A-M1: flipcard bus event updates the profile and never flips it (kills C13)", async () => {
  const board = await waitForCards(10);
  resetFaces(board);
  const profile = board.querySelector<HTMLElement>('.card[data-id="profile-anton"]');
  assert.ok(profile, "anton profile tile");
  assert.equal(controlListeners.length > 0, true, "main.ts connectControl registered a control listener");
  assert.equal(face(profile!), "front");
  fireControl("flipcard", { profile: harvestProfile, tileId: "profile-anton" }, 101);
  await new Promise((r) => setImmediate(r));
  assert.equal(face(profile!), "front", "flipcard must not flip (C13 / PR4 applyFlipcard)");
  assert.match(profile!.querySelector('[data-field="refs"]')?.textContent ?? "", /clip:lib-misaki-kokoro/);
});

test("L2/A-M1: flip on tile X with a profile payload does not flip the profile tile", async () => {
  const board = await waitForCards(10);
  resetFaces(board);
  const profile = board.querySelector<HTMLElement>('.card[data-id="profile-anton"]');
  const clip = board.querySelector<HTMLElement>('.card[data-id="lib-misaki-kokoro"]');
  assert.ok(profile && clip);
  fireControl("flip", { tileId: "lib-misaki-kokoro", flipped: true, profile: harvestProfile }, 102);
  await new Promise((r) => setImmediate(r));
  assert.equal(face(clip!), "back", "tile X flipped");
  assert.equal(face(profile!), "front", "profile tile stayed put");
});

test("L2/C12: Attach to profile via the real button path never flips the profile tile", async () => {
  const board = await waitForCards(10);
  resetFaces(board);
  // studio defaults the agent slot to alice; C12 flips profile-${personaId} for that selection.
  const slot = happy.document.querySelector<HTMLSelectElement>("#agents .agent-slot");
  assert.ok(slot, "agent slot exists");
  const personaId = slot!.value || "alice";
  const profile = board.querySelector<HTMLElement>(`.card[data-id="profile-${personaId}"]`);
  assert.ok(profile, `profile tile for selected agent ${personaId}`);
  const attach = board.querySelector<HTMLButtonElement>("[data-action='attach-profile'][data-clip-id='lib-misaki-kokoro']")
    ?? board.querySelector<HTMLButtonElement>("[data-action='attach-profile']");
  assert.ok(attach, "Attach to profile control exists after main.ts bindRename");
  assert.equal(face(profile!), "front");
  attach!.dispatchEvent(new happy.MouseEvent("click", { bubbles: true, cancelable: true }) as unknown as Event);
  for (let i = 0; i < 20; i += 1) {
    await new Promise((r) => setImmediate(r));
    mock.timers.tick(1);
  }
  assert.equal(face(profile!), "front", "attach button path must not flip (C12)");
});

test("L2/B8: syncCubeChrome passes the bound cube uid into fillCubeGlyph (not null)", async () => {
  const board = await waitForCards(10);
  const catalog = await loadLibraryCatalog();
  assert.ok(catalog);
  const cube = catalog!.assets.find((a) => a.kind === "cube_ihdr" && a.legacy_id === "lib-misaki-kokoro.cube");
  assert.ok(cube, "misaki library cube in assets.json");
  // Bind the spatial cube the same way a library tile click does: open its cube JSON URL.
  const tile = board.querySelector<HTMLElement>('.card[data-id="lib-misaki-kokoro"]');
  assert.ok(tile?.dataset.cubeJson, "lib-misaki-kokoro has cubeJson after decorate");
  // Clicking the tile selects and opens the cube (main.ts selectTile → openCube → syncCubeChrome).
  click(tile!);
  const deadline = Date.now() + READY_MS;
  while (Date.now() < deadline) {
    await new Promise((r) => setImmediate(r));
    mock.timers.tick(5);
    const slot = happy.document.querySelector("#cube-glyph");
    const copy = slot?.querySelector<HTMLButtonElement>("button.copy-uid, button");
    if (copy && (copy.textContent ?? "").includes("Copy cube uid")) {
      assert.equal(copy.dataset.uid, cube!.uid, "B8: syncCubeChrome must pass the cube uid, not null");
      return;
    }
  }
  const slot = happy.document.querySelector("#cube-glyph");
  assert.fail(`B8: #cube-glyph never got Copy cube uid (slot text=${JSON.stringify(slot?.textContent)} children=${slot?.childElementCount})`);
});

// L40-1 / E5: a default boot (fixtures flag unset) must show the release deck.
// A mutant that always loads viewport.example.json puts spec-fixture on the board.
test("L40-1: default boot shows the release deck (not the fixture deck)", async () => {
  const board = await waitForCards(10);
  assert.ok(
    board.querySelector('.card[data-id="lib-misaki-kokoro"]'),
    "release deck includes lib-misaki-kokoro",
  );
  assert.equal(
    board.querySelector('.card[data-id="spec-fixture"]'),
    null,
    "release deck must not include spec-fixture",
  );
  assert.equal(
    board.querySelector('.card[data-id="cube-fixture"]'),
    null,
    "release deck must not include cube-fixture",
  );
});

// L40-1: fixtures flag gates the dev-asset merge. Default boot passes [] into
// loadLibraryCatalog; a mutant that always merges assets.dev.json puts the
// fixture card envelopes into the shared catalog pending.
test("L40-1: default boot does not merge assets.dev into the library catalog", async () => {
  await waitForCards(10);
  const catalog = await loadLibraryCatalog();
  assert.ok(catalog);
  const devIds = ["spec-fixture", "cube-fixture", "serve-node", "serve-gateway", "bench-ref", "cast-sample"];
  for (const id of devIds) {
    assert.equal(
      catalog!.assets.find((a) => a.legacy_id === id),
      undefined,
      `dev asset ${id} must not be in the catalog when fixtures are off`,
    );
  }
});
