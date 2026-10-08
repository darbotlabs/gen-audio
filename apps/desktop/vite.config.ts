import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig, type Plugin } from "vite";

/** Cards that stay in the dev catalog and the example viewport, and must not ship in dist. */
const STUB_CARD_IDS = new Set([
  "serve-node",
  "serve-gateway",
  "cube-fixture",
  "bench-ref",
  "spec-fixture",
  "cast-sample",
]);

const STUB_PHRASE = /not remeasured|stand-in|\bstand in\b/i;

function shipsInRelease(asset: unknown): boolean {
  if (!asset || typeof asset !== "object") return false;
  const record = asset as {
    legacy_id?: unknown;
    honesty?: { fixture?: unknown };
    body?: { id?: unknown; kind?: unknown; body?: { probed?: unknown; measuredHere?: unknown; host?: unknown } };
  };
  const legacy = typeof record.legacy_id === "string" ? record.legacy_id : "";
  const cardId = typeof record.body?.id === "string" ? record.body.id : legacy;
  if (STUB_CARD_IDS.has(cardId) || STUB_CARD_IDS.has(legacy)) return false;
  if (record.honesty?.fixture === true) return false;
  const kind = record.body?.kind;
  const inner = record.body?.body;
  if (kind === "ServeHealth" && inner?.probed !== true) return false;
  if (kind === "BenchmarkCompare" && inner?.measuredHere !== true) return false;
  if (typeof inner?.host === "string" && /^<[^>]+>$/.test(inner.host)) return false;
  return !STUB_PHRASE.test(JSON.stringify(record));
}

/** Drop stub cards from the release deck module so they are not in the JS bundle. The source file stays the example minus the dev-fixture assets (the Rust pin). */
function stripReleaseDeck(): Plugin {
  return {
    name: "strip-release-deck",
    apply: "build",
    enforce: "pre",
    transform(code, id) {
      const path = id.split("?")[0]?.replaceAll("\\", "/");
      if (!path?.endsWith("schemas/examples/viewport.release.json")) return null;
      const document = JSON.parse(code) as { cards?: Array<{ id?: string }> };
      document.cards = (document.cards ?? []).filter((card) => !STUB_CARD_IDS.has(String(card.id)));
      return { code: JSON.stringify(document), map: null };
    },
  };
}

/** Vite copies public/ as-is. Drop fixture and placeholder cards from the bundle the release gate scans. */
function stripReleaseCatalog(): Plugin {
  return {
    name: "strip-release-catalog",
    apply: "build",
    closeBundle() {
      const path = resolve("dist/library/assets.json");
      if (!existsSync(path)) return;
      const catalog = JSON.parse(readFileSync(path, "utf8")) as {
        assets?: unknown[];
        legacy_index?: Record<string, string>;
      };
      const assets = (catalog.assets ?? []).filter(shipsInRelease);
      const kept = new Set(
        assets.flatMap((asset) => {
          if (!asset || typeof asset !== "object") return [];
          const uid = (asset as { uid?: unknown }).uid;
          return typeof uid === "string" ? [uid] : [];
        }),
      );
      const index: Record<string, string> = {};
      for (const [key, uid] of Object.entries(catalog.legacy_index ?? {})) {
        const tail = key.includes(":") ? key.slice(key.indexOf(":") + 1) : key;
        if (STUB_CARD_IDS.has(tail) || !kept.has(uid)) continue;
        index[key] = uid;
      }
      catalog.assets = assets;
      catalog.legacy_index = index;
      writeFileSync(path, `${JSON.stringify(catalog, null, 2)}\n`);
    },
  };
}

export default defineConfig({
  clearScreen: false,
  plugins: [stripReleaseDeck(), stripReleaseCatalog()],
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
  },
});
