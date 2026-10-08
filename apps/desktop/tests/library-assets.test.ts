// A catalog fetch used to resolve null with the error thrown away, which made
// the window's catalog catch dead. An unsafe media path used to return false
// with no log.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadLibraryCatalog, resetLibraryCatalogForTest } from "../src/library-assets.ts";

const CUBE = "ga:cube_ihdr:biaxxmnibxtcur7nxdu3ffvna4";

function envelope(path: string): unknown {
  return {
    uid: CUBE,
    kind: "cube_ihdr",
    status: "real",
    fields: {},
    media: [{ role: "cube_json", path, sha256: "ab", bytes: 1 }],
    src: [],
    display: { title: "Cube", glyph: "y" },
  };
}

test("a failed library catalog fetch rejects and is tried again", async () => {
  resetLibraryCatalogForTest();
  const warnings: string[] = [];
  const original = console.warn;
  console.warn = (message?: unknown) => {
    warnings.push(String(message));
  };
  let fetches = 0;
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    fetches += 1;
    throw new TypeError("offline");
  }) as typeof fetch;
  try {
    const offline = (error: unknown) => error instanceof TypeError && error.message === "offline";
    await assert.rejects(loadLibraryCatalog(), offline);
    await assert.rejects(loadLibraryCatalog(), offline);
    assert.equal(fetches, 2, "a failed load must not stick in the cache");
    assert.deepEqual(warnings, [], "the loader must not swallow the rejection");
  } finally {
    console.warn = original;
    globalThis.fetch = originalFetch;
    resetLibraryCatalogForTest();
  }
});

test("an unsafe library media path is dropped and logged", async () => {
  resetLibraryCatalogForTest();
  const warnings: string[] = [];
  const original = console.warn;
  console.warn = (message?: unknown) => {
    warnings.push(String(message));
  };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ assets: [envelope("../secret.wav")] }), {
      status: 200,
      headers: { "content-type": "application/json" },
    })) as typeof fetch;
  try {
    const catalog = await loadLibraryCatalog();
    assert.equal(catalog?.assets.length, 0);
    assert.deepEqual(warnings, [
      'gen-audio: library asset: Error: media_path_dotdot: "../secret.wav" walks out of the library root',
    ]);
  } finally {
    console.warn = original;
    globalThis.fetch = originalFetch;
    resetLibraryCatalogForTest();
  }
});

test("a library media path inside the root is kept and not logged", async () => {
  resetLibraryCatalogForTest();
  const warnings: string[] = [];
  const original = console.warn;
  console.warn = (message?: unknown) => {
    warnings.push(String(message));
  };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ assets: [envelope("clip.wav")] }), {
      status: 200,
      headers: { "content-type": "application/json" },
    })) as typeof fetch;
  try {
    const catalog = await loadLibraryCatalog();
    assert.equal(catalog?.assets.length, 1);
    assert.equal(catalog?.assets[0]?.uid, CUBE);
    assert.deepEqual(warnings, []);
  } finally {
    console.warn = original;
    globalThis.fetch = originalFetch;
    resetLibraryCatalogForTest();
  }
});
