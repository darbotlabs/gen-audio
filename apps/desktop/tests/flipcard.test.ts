// livetile-faces v2, Optimus rulings 1 and 2 (2026-10-08), rendered in a real
// DOM (happy-dom) from the release deck (schemas/examples/viewport.release.json)
// and the committed asset catalog (public/library/assets.json). No fixtures.
//  1. The glyph has one meaning: flip. Copy clip uid is a labelled button on the
//     Clip face; Copy cube uid is the spatial cube's labelled button.
//  2. No Flip / Show front buttons, no body click or body Enter flip, no timer
//     flips (engine and connector tiles included). The glyph is a real button
//     in the same top-right corner on every face; click, Enter and Space step
//     through the faces and wrap. MCP Flip (setCardFlip) stays. Reduced motion
//     turns the flip transition off.
import { after, test, mock } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Window } from "happy-dom";

const ROOT = fileURLToPath(new URL("../../../", import.meta.url));
const RELEASE = JSON.parse(readFileSync(`${ROOT}schemas/examples/viewport.release.json`, "utf8"));
const ASSETS = readFileSync(`${ROOT}apps/desktop/public/library/assets.json`, "utf8");
const CSS_TEXT = readFileSync(`${ROOT}apps/desktop/src/styles.css`, "utf8");

const happy = new Window({ url: "http://127.0.0.1/", settings: { device: { prefersReducedMotion: "no-preference" } } });
const copied: string[] = [];
Object.defineProperty(happy.navigator, "clipboard", {
  configurable: true,
  value: { writeText: async (text: string) => void copied.push(text) },
});
const g = globalThis as unknown as Record<string, unknown>;
for (const name of ["window", "document", "navigator", "CSS", "HTMLElement", "HTMLButtonElement", "HTMLInputElement", "Element", "Node", "KeyboardEvent", "MouseEvent", "Event", "Image"]) {
  Object.defineProperty(g, name, { configurable: true, writable: true, value: (happy as unknown as Record<string, unknown>)[name] });
}
g.fetch = async (url: string) => {
  assert.equal(url, "/library/assets.json");
  return { ok: true, json: async () => JSON.parse(ASSETS) };
};
const style = happy.document.createElement("style");
style.textContent = CSS_TEXT;
happy.document.head.append(style);

// Closing the window cancels any timer the code under test left behind, so a
// regression that schedules flips fails its test instead of hanging the run.
after(() => happy.happyDOM.close());

const render = await import("../src/render.ts");
const { decorateLibraryTiles } = await import("../src/livestrip.ts");
const { loadLibraryCatalog } = await import("../src/library-assets.ts");

function mount(): HTMLElement {
  happy.document.body.replaceChildren();
  const board = happy.document.createElement("div");
  board.className = "board";
  board.id = "board";
  const empty = happy.document.createElement("p");
  happy.document.body.append(board, empty);
  render.renderBoard(board as unknown as HTMLElement, empty as unknown as HTMLElement, structuredClone(RELEASE));
  return board as unknown as HTMLElement;
}

const tiles = (board: HTMLElement) => Array.from(board.querySelectorAll<HTMLElement>(".card"));
const glyphOf = (tile: HTMLElement) => {
  const found = Array.from(tile.children).filter((node) => node.classList.contains("flip-glyph"));
  assert.equal(found.length, 1, `${tile.dataset.id}: exactly one flip glyph, a direct child of the card`);
  return found[0] as HTMLButtonElement;
};
const face = (tile: HTMLElement) => tile.dataset.face;
const click = (node: Element) => node.dispatchEvent(new happy.MouseEvent("click", { bubbles: true, cancelable: true }) as unknown as Event);
const key = (node: Element, k: string) => {
  const event = new happy.KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }) as unknown as KeyboardEvent;
  node.dispatchEvent(event);
  return event;
};

test("ruling 2: no Flip or Show front button on any face", () => {
  const board = mount();
  assert.equal(board.querySelectorAll(".flip-toggle").length, 0);
  const labels = Array.from(board.querySelectorAll("button")).map((b) => (b.textContent ?? "").trim());
  assert.ok(!labels.includes("Flip"), "no front Flip button");
  assert.ok(!labels.includes("Show front"), "no back Show front button");
});

test("ruling 2: a click or Enter/Space on the tile body never flips (play/scrub/select only)", () => {
  const board = mount();
  const kinds = new Set<string>();
  for (const tile of tiles(board)) {
    kinds.add(tile.dataset.kind ?? "");
    const front = tile.querySelector(".face.front") as HTMLElement;
    for (const target of [tile, front, front.querySelector("h2"), front.querySelector("p")]) {
      if (!target) continue;
      click(target);
      key(target, "Enter");
      key(target, " ");
      assert.equal(face(tile), "front", `${tile.dataset.id}: body input flipped the card`);
      assert.ok(!tile.classList.contains("is-flipped"), `${tile.dataset.id}: is-flipped after a body input`);
    }
  }
  for (const kind of ["LibraryClip", "EngineStatus", "ConnectorStatus", "VoiceProfile"]) assert.ok(kinds.has(kind), kind);
});

test("ruling 2: no timer flips, engine and connector tiles included (fake timers)", () => {
  mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  try {
    const realInterval = happy.setInterval;
    const realTimeout = happy.setTimeout;
    happy.setInterval = globalThis.setInterval as unknown as typeof happy.setInterval;
    happy.setTimeout = globalThis.setTimeout as unknown as typeof happy.setTimeout;
    try {
      assert.equal(happy.matchMedia("(prefers-reduced-motion: reduce)").matches, false, "motion allowed, so a timer would run");
      const board = mount();
      const watched = tiles(board).filter((t) => t.dataset.kind === "EngineStatus" || t.dataset.kind === "ConnectorStatus");
      assert.ok(watched.length >= 10, "engine + connector tiles rendered");
      for (let i = 0; i < 60; i += 1) {
        mock.timers.tick(10_000);
        for (const tile of tiles(board)) assert.equal(face(tile), "front", `${tile.dataset.id} flipped by a timer after ${(i + 1) * 10}s`);
      }
    } finally {
      happy.setInterval = realInterval;
      happy.setTimeout = realTimeout;
    }
  } finally {
    mock.timers.reset();
  }
});

test("ruling 2: the glyph is a focusable button; aria-label is 'Flip card, face N of M: <next>'; clicks wrap", () => {
  const board = mount();
  for (const tile of tiles(board)) {
    const glyph = glyphOf(tile);
    assert.equal(glyph.tagName, "BUTTON");
    assert.equal(glyph.type, "button");
    assert.ok(!glyph.disabled && glyph.tabIndex >= 0, "focusable");
    const faces = (tile.dataset.faces ?? "").split(" ");
    const m = faces.length;
    assert.ok(m >= 2, `${tile.dataset.id}: ${m} faces`);
    for (let step = 0; step <= m; step += 1) {
      const n = step % m;
      assert.equal(face(tile), faces[n]);
      assert.equal(glyph.getAttribute("aria-label"), `Flip card, face ${n + 1} of ${m}: ${faces[(n + 1) % m]}`);
      click(glyph);
    }
    assert.equal(face(tile), faces[1 % m], "wrapped around past the last face");
  }
});

test("ruling 2: Enter and Space on the glyph flip (and are consumed)", () => {
  const board = mount();
  const tile = tiles(board).find((t) => t.dataset.kind === "ConnectorStatus")!;
  const glyph = glyphOf(tile);
  const enter = key(glyph, "Enter");
  assert.equal(face(tile), "back");
  assert.ok(enter.defaultPrevented, "Enter consumed so the browser does not also click");
  const space = key(glyph, " ");
  assert.equal(face(tile), "front");
  assert.ok(space.defaultPrevented);
  key(glyph, "a");
  assert.equal(face(tile), "front", "other keys do nothing");
});

test("ruling 2: the glyph keeps the same top-right corner on every face", () => {
  const board = mount();
  for (const tile of tiles(board)) {
    const glyph = glyphOf(tile);
    const faces = (tile.dataset.faces ?? "").split(" ");
    const seen = new Set<string>();
    for (let i = 0; i < faces.length; i += 1) {
      assert.equal(glyphOf(tile), glyph, "same node on every face (never re-rendered per face)");
      assert.equal(glyph.closest(".face"), null, "outside every face, so it never rotates away");
      assert.equal(glyph.closest(".flip"), null, "outside the rotating .flip");
      const cs = happy.getComputedStyle(glyph as unknown as never);
      assert.equal(cs.position, "absolute");
      seen.add(`${cs.top}|${cs.right}|${cs.left}|${cs.bottom}|${cs.transform}`);
      click(glyph);
    }
    assert.equal(seen.size, 1, `${tile.dataset.id}: corner moved between faces: ${[...seen].join(" / ")}`);
  }
  const card = happy.getComputedStyle(board.querySelector(".card") as unknown as never);
  assert.equal(card.position, "relative", "the card anchors the corner");
});

test("ruling 1: clicking the glyph flips and never copies", async () => {
  const board = mount();
  copied.length = 0;
  for (const tile of tiles(board)) {
    const glyph = glyphOf(tile);
    assert.ok(!/copy/i.test(glyph.getAttribute("aria-label") ?? ""), "the label says flip, not copy");
    assert.ok(!/copy/i.test(glyph.title ?? ""), "the tooltip says flip, not copy");
    click(glyph);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(face(tile), "back");
  }
  assert.deepEqual(copied, [], "no glyph click wrote the clipboard");
});

test("ruling 1 + second ruling 4: 'Copy clip uid' exists only on the Clip face of Audio Clips tiles and copies the clip uid", async () => {
  const board = mount();
  const catalog = await loadLibraryCatalog();
  assert.ok(catalog, "assets.json loaded");
  decorateLibraryTiles(board, catalog, { onCopy: () => {} });
  copied.length = 0;
  let withClip = 0;
  for (const tile of tiles(board)) {
    const buttons = Array.from(tile.querySelectorAll<HTMLButtonElement>("button")).filter((b) => /copy/i.test(b.textContent ?? "") || /copy/i.test(b.getAttribute("aria-label") ?? ""));
    const clip = tile.dataset.kind === "LibraryClip" ? catalog!.clipForTile(tile.dataset.id ?? "") : null;
    if (!clip) {
      assert.equal(buttons.length, 0, `${tile.dataset.id}: no copy control off the Clip face`);
      continue;
    }
    withClip += 1;
    assert.equal(buttons.length, 1, `${tile.dataset.id}: one Copy clip uid button`);
    const copy = buttons[0];
    assert.equal(copy.textContent, "Copy clip uid", "it copies the clip's uid, not the card's, and says so");
    assert.equal(copy.dataset.uid, clip.uid);
    assert.ok(copy.closest(".face.front"), "on the Clip (front) face");
    assert.equal(tile.querySelector(".face.back button[data-action='copy-uid']"), null);
    // The only top-right glyph on the tile is the flip glyph.
    assert.equal(tile.querySelectorAll(".live-row button.ga-glyph").length, 0, `${tile.dataset.id}: no second interactive glyph in the corner row`);
    click(copy);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(copied.at(-1), clip.uid);
    assert.equal(face(tile), "front", "copying does not flip");
  }
  assert.ok(withClip >= 4, `${withClip} clip tiles`);
});

test("MCP Flip stays: setCardFlip drives the same face state and keeps the glyph label truthful", () => {
  const board = mount();
  const tile = tiles(board).find((t) => t.dataset.id === "engine-kokoro")!;
  const glyph = glyphOf(tile);
  assert.equal(render.setCardFlip(board, "engine-kokoro", true), true);
  assert.equal(face(tile), "back");
  assert.ok(tile.classList.contains("is-flipped"));
  assert.equal(glyph.getAttribute("aria-label"), "Flip card, face 2 of 2: front");
  assert.equal(render.setCardFlip(board, "engine-kokoro", false), true);
  assert.equal(face(tile), "front");
  assert.equal(render.setCardFlip(board, "no-such-tile", true), false);
});

test("the hidden face is inert, so Tab never lands on a control you cannot see", () => {
  const board = mount();
  const tile = tiles(board).find((t) => t.dataset.id === "engine-kokoro")!;
  const front = tile.querySelector(".face.front") as HTMLElement;
  const back = tile.querySelector(".face.back") as HTMLElement;
  assert.equal(back.inert, true);
  assert.equal(front.inert, false);
  click(glyphOf(tile));
  assert.equal(back.inert, false);
  assert.equal(front.inert, true);
});

test("reduced motion: the flip turns with no transition, and still turns", () => {
  const reduced = new Window({ settings: { device: { prefersReducedMotion: "reduce" } } });
  const css = reduced.document.createElement("style");
  css.textContent = CSS_TEXT;
  reduced.document.head.append(css);
  const flip = reduced.document.createElement("div");
  flip.className = "flip";
  reduced.document.body.append(flip);
  assert.equal(reduced.matchMedia("(prefers-reduced-motion: reduce)").matches, true);
  assert.match(reduced.getComputedStyle(flip).transitionDuration || reduced.getComputedStyle(flip).transition, /^(none|0s)/);
  const motion = happy.document.createElement("div");
  motion.className = "flip";
  happy.document.body.append(motion);
  assert.match(happy.getComputedStyle(motion).transition, /transform/, "motion allowed: the flip animates");
  void reduced.happyDOM.close();
});

test("second ruling 5 + Q12: the spatial cube's glyph never copies; its labelled 'Copy cube uid' button does", async () => {
  const board = mount();
  const catalog = await loadLibraryCatalog();
  const cube = catalog!.assets.find((asset) => asset.kind === "cube_ihdr" && asset.legacy_id === "lib-misaki-kokoro.cube");
  assert.ok(cube, "the misaki library cube is in assets.json");
  const slot = board.querySelector<HTMLElement>("#cube-glyph");
  assert.ok(slot, "the spatial slide has its glyph slot");
  const announced: Array<[string, boolean]> = [];
  render.fillCubeGlyph(slot!, cube!.uid, (uid, ok) => announced.push([uid, ok]));
  copied.length = 0;
  const glyph = slot!.querySelector<HTMLElement>(".ga-glyph");
  assert.ok(glyph, "the cube glyph is shown");
  assert.notEqual(glyph!.tagName, "BUTTON", "the glyph is a passive mark, not a control");
  click(glyph!);
  key(glyph!, "Enter");
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(copied, [], "clicking the glyph copied nothing");
  const buttons = Array.from(slot!.querySelectorAll<HTMLButtonElement>("button"));
  assert.equal(buttons.length, 1, "one control in the slot");
  assert.equal(buttons[0].textContent, "Copy cube uid");
  click(buttons[0]);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(copied, [cube!.uid], "the labelled button copied the cube uid");
  assert.deepEqual(announced, [[cube!.uid, true]]);
  render.fillCubeGlyph(slot!, null, () => {});
  assert.equal(slot!.childElementCount, 0, "nothing bound: the slot is empty, no stand-in uid");
});

function profileTile(board: HTMLElement): HTMLElement {
  const tile = board.querySelector<HTMLElement>('.card[data-id="profile-anton"]');
  assert.ok(tile, "the release doc has the Anton profile tile");
  return tile!;
}

const attachedProfile = { personaId: "anton", refs: ["persona:anton", "clip:lib-misaki-kokoro"] };

test("second ruling 6: Attach to profile updates the profile tile and announces it, but never flips it", () => {
  for (const leftOn of ["front", "back"]) {
    const board = mount();
    const tile = profileTile(board);
    if (leftOn === "back") click(glyphOf(tile));
    assert.equal(face(tile), leftOn, "the user left the tile on this face");
    // The Attach to profile button's path (main.ts bindRename) ...
    render.applyProfileUpdate(board, attachedProfile);
    assert.equal(face(tile), leftOn, "attach left the face where the user left it");
    assert.equal(tile.classList.contains("is-flipped"), leftOn === "back");
    // ... and the library_harvest apply bus event ("flipcard" op).
    assert.equal(render.applyCardOp(board, "flipcard", { personaId: "anton", tileId: "profile-anton", profile: attachedProfile }), true);
    assert.equal(face(tile), leftOn, "the flipcard bus event did not flip either");
    const badge = tile.querySelector<HTMLElement>(":scope > .profile-status");
    assert.ok(badge, "a status badge sits on the tile, outside the faces, so it shows on any face");
    assert.equal(badge!.getAttribute("role"), "status", "the badge is a polite live region");
    assert.match(badge!.textContent ?? "", /clip:lib-misaki-kokoro/);
    // Low (a): name the face from data-faces (refs live on face index 1), never hardcode "face 2".
    const faces = (tile.dataset.faces ?? "").split(" ");
    assert.ok(faces.length >= 2, "profile has a face that holds refs");
    assert.match(badge!.textContent ?? "", new RegExp(`Refs are on the ${faces[1]} face`), `badge must name faces[1]=${faces[1]}, not a hardcoded index`);
    assert.doesNotMatch(badge!.textContent ?? "", /face 2/, "no hardcoded face number");
    assert.equal(tile.querySelector('[data-field="refs"]')?.textContent, "persona:anton, clip:lib-misaki-kokoro");
  }
});

test("Low (a): the status badge clears on the next user action without moving the face", () => {
  const board = mount();
  const tile = profileTile(board);
  assert.equal(face(tile), "front");
  render.applyProfileUpdate(board, attachedProfile);
  const badge = tile.querySelector<HTMLElement>(":scope > .profile-status");
  assert.ok(badge?.textContent, "badge announced the attach");
  // Next user action on this tile: glyph flip to back and back to front.
  click(glyphOf(tile));
  assert.equal(face(tile), "back");
  assert.equal(tile.querySelector(":scope > .profile-status"), null, "badge cleared when the user flipped");
  click(glyphOf(tile));
  assert.equal(face(tile), "front", "clearing the badge did not steal the face");
  assert.equal(tile.querySelector(":scope > .profile-status"), null);
});

test("second ruling 6: an explicit ui_flip (the flip op) still flips the profile tile", () => {
  const board = mount();
  const tile = profileTile(board);
  assert.equal(face(tile), "front");
  assert.equal(render.applyCardOp(board, "flip", { tileId: "profile-anton", flipped: true, profile: attachedProfile }), true);
  assert.equal(face(tile), "back", "ui_flip flipped:true shows face 2");
  assert.equal(tile.querySelector('[data-field="refs"]')?.textContent, "persona:anton, clip:lib-misaki-kokoro");
  assert.equal(render.applyCardOp(board, "flip", { tileId: "profile-anton", flipped: false, profile: attachedProfile }), true);
  assert.equal(face(tile), "front", "ui_flip flipped:false is honoured even with a profile payload");
  assert.equal(render.applyCardOp(board, "flip", { tileId: "profile-anton" }), true);
  assert.equal(face(tile), "back", "flipped defaults to true, as ui_flip documents");
});

test("Low (b): ui_flip on tile X with persona P does not flip P's profile tile", () => {
  const board = mount();
  const profile = profileTile(board);
  const clip = tiles(board).find((t) => t.dataset.id === "lib-misaki-kokoro");
  assert.ok(clip, "release deck has lib-misaki-kokoro");
  assert.equal(face(profile), "front");
  assert.equal(face(clip!), "front");
  // Mutant C4: a flip op that carries a profile payload also flipped the profile tile.
  assert.equal(
    render.applyCardOp(board, "flip", {
      tileId: "lib-misaki-kokoro",
      flipped: true,
      profile: attachedProfile,
    }),
    true,
  );
  assert.equal(face(clip!), "back", "tile X flipped");
  assert.equal(face(profile), "front", "persona P's profile tile did not flip");
  assert.equal(profile.querySelector('[data-field="refs"]')?.textContent, "persona:anton, clip:lib-misaki-kokoro", "profile still received the refs update");
});
