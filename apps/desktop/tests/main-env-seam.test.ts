// Option (c): env.ts is the only seam. bindEnv({ VITE_GEN_AUDIO_FIXTURES: "1" })
// must turn on fixture-only UI in main.ts. A mutant that reads import.meta.env
// directly (bypassing env.ts) stays dark under tsx and this test goes red.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Window } from "happy-dom";
import { bindEnv } from "../src/env";

bindEnv({ VITE_GEN_AUDIO_FIXTURES: "1" });

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

class StubEventSource {
  readyState = 2;
  addEventListener(): void {}
  close(): void {}
  set onerror(_fn: unknown) {}
}
(globalThis as unknown as { EventSource: unknown }).EventSource = StubEventSource;
(happy as unknown as { EventSource: unknown }).EventSource = StubEventSource;
(happy.window as unknown as { EventSource: unknown }).EventSource = StubEventSource;
Object.defineProperty(happy.navigator, "clipboard", { configurable: true, value: { writeText: async () => {} } });
(globalThis as unknown as { fetch: unknown }).fetch = async (url: string) => {
  const u = String(url);
  if (u.includes("/library/assets.json")) {
    return { ok: true, status: 200, json: async () => JSON.parse(ASSETS), text: async () => ASSETS };
  }
  return { ok: false, status: 503, json: async () => ({}), text: async () => "" };
};

await import("../src/main.ts");

test("option (c): bindEnv fixtures=1 turns on Load labeled example (kills import.meta.env bypass)", async () => {
  // fixture-controls loads via dynamic import — poll with real timers.
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 25));
    const btn = document.querySelector("#show-example") ?? happy.document.querySelector("#show-example");
    if (btn) {
      assert.equal(btn.textContent, "Load labeled example");
      const scrub = (window as unknown as { __genAudioScrub?: unknown }).__genAudioScrub
        ?? (happy.window as unknown as { __genAudioScrub?: unknown }).__genAudioScrub;
      assert.equal(typeof scrub, "function", "__genAudioScrub test hook must bind when fixtures=1");
      return;
    }
  }
  assert.fail("bindEnv seam must reach main.ts; bypassing env.ts leaves fixture UI off under tsx");
});

// N-M2: the deck and dev-asset imports test FIXTURES_BUILD || (!UNDER_VITE &&
// seam) inline so release folds them away. Under tsx UNDER_VITE is false, so
// the seam must still pick the fixture deck; a call site that drops the seam
// operand (or tests FIXTURES_BUILD alone) boots the release deck here.
test("N-M2: bindEnv fixtures=1 boots the fixture deck through the inline call-site guard", async () => {
  const env = await import("../src/env");
  assert.equal(env.UNDER_VITE, false, "tsx has no import.meta.env");
  assert.equal(env.FIXTURES_BUILD, false, "tsx has no build-time fixtures flag");
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 25));
    const board = happy.document.querySelector("#board");
    if (board?.querySelector('.card[data-id="spec-fixture"]')) {
      assert.ok(board.querySelector('.card[data-id="cube-fixture"]'), "fixture deck has cube-fixture");
      return;
    }
  }
  const ids = Array.from(happy.document.querySelectorAll("#board .card")).map((n) => (n as HTMLElement).dataset.id);
  assert.fail(`bindEnv fixtures=1 must boot the fixture deck (spec-fixture); board has ${JSON.stringify(ids)}`);
});

// L40-1: fixtures=1 must merge assets.dev.json through loadDevAssets(). A mutant
// that drops the fixtures operand (always returns []) leaves the catalog without
// the six fixture card envelopes even though the fixture deck is on the board.
test("L40-1: bindEnv fixtures=1 merges assets.dev into the library catalog", async () => {
  const { loadLibraryCatalog } = await import("../src/library-assets");
  const deadline = Date.now() + 5000;
  let catalog = null as Awaited<ReturnType<typeof loadLibraryCatalog>>;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 25));
    catalog = await loadLibraryCatalog();
    if (catalog?.assets.some((a) => a.legacy_id === "spec-fixture")) break;
  }
  assert.ok(catalog, "catalog must load");
  for (const id of ["spec-fixture", "cube-fixture", "serve-node", "serve-gateway", "bench-ref", "cast-sample"]) {
    assert.ok(
      catalog!.assets.some((a) => a.legacy_id === id),
      `fixtures=1 must merge assets.dev envelope ${id}`,
    );
  }
});
