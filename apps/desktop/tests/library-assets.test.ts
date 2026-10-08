// A catalog fetch used to resolve null with the error thrown away.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadLibraryCatalog } from "../src/library-assets.ts";

test("a failed library catalog fetch is logged and returns null", async () => {
  const warnings: string[] = [];
  const original = console.warn;
  console.warn = (message?: unknown) => {
    warnings.push(String(message));
  };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new TypeError("offline");
  }) as typeof fetch;
  try {
    assert.equal(await loadLibraryCatalog(), null);
    assert.match(warnings.join("\n"), /library catalog/);
    assert.match(warnings.join("\n"), /TypeError/);
    assert.match(warnings.join("\n"), /offline/);
  } finally {
    console.warn = original;
    globalThis.fetch = originalFetch;
  }
});
