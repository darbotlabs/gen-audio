import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig, type Plugin } from "vite";

/**
 * Dev-only and stand-in cards. The generator split (is_dev_fixture) is what
 * keeps them out of the release deck and catalog. This plugin does not
 * rewrite those files: a release build fails if one is still present.
 */
const STUB_CARD_IDS = new Set([
  "serve-node",
  "serve-gateway",
  "cube-fixture",
  "bench-ref",
  "spec-fixture",
  "cast-sample",
]);

const STUB_PHRASE = /not remeasured|stand-in|\bstand in\b/i;

type Card = {
  id?: unknown;
  kind?: unknown;
  body?: { probed?: unknown; measuredHere?: unknown; host?: unknown; sampleScript?: unknown };
};

type CatalogAsset = {
  legacy_id?: unknown;
  honesty?: { fixture?: unknown };
  body?: { id?: unknown; kind?: unknown; body?: Card["body"] };
};

function cardProblems(card: Card): string[] {
  const id = typeof card.id === "string" ? card.id : "?";
  const problems: string[] = [];
  if (STUB_CARD_IDS.has(id)) problems.push(id);
  const body = card.body ?? {};
  if (card.kind === "ServeHealth" && body.probed !== true) problems.push(`${id}: unprobed ServeHealth`);
  if (card.kind === "BenchmarkCompare" && body.measuredHere !== true) problems.push(`${id}: unmeasured BenchmarkCompare`);
  if (typeof body.host === "string" && /^<[^>]+>$/.test(body.host)) problems.push(`${id}: placeholder host`);
  if (body.sampleScript === true) problems.push(`${id}: sample script`);
  if (STUB_PHRASE.test(JSON.stringify(card))) problems.push(`${id}: stand-in phrase`);
  return problems;
}

function assetProblems(asset: unknown): string[] {
  if (!asset || typeof asset !== "object") return ["asset is not an object"];
  const record = asset as CatalogAsset;
  const legacy = typeof record.legacy_id === "string" ? record.legacy_id : "";
  const cardId = typeof record.body?.id === "string" ? record.body.id : legacy;
  const label = cardId || legacy || "asset";
  const problems: string[] = [];
  if (STUB_CARD_IDS.has(cardId) || STUB_CARD_IDS.has(legacy)) problems.push(label);
  if (record.honesty?.fixture === true) problems.push(`${label}: honesty.fixture`);
  const kind = record.body?.kind;
  const inner = record.body?.body ?? {};
  if (kind === "ServeHealth" && inner.probed !== true) problems.push(`${label}: unprobed ServeHealth`);
  if (kind === "BenchmarkCompare" && inner.measuredHere !== true) problems.push(`${label}: unmeasured BenchmarkCompare`);
  if (typeof inner.host === "string" && /^<[^>]+>$/.test(inner.host)) problems.push(`${label}: placeholder host`);
  if (inner.sampleScript === true) problems.push(`${label}: sample script`);
  if (STUB_PHRASE.test(JSON.stringify(record))) problems.push(`${label}: stand-in phrase`);
  return problems;
}

/** Fail the production build when a dev-only or stand-in card is still in the release inputs. */
function failReleaseStubs(): Plugin {
  return {
    name: "fail-release-stubs",
    apply: "build",
    enforce: "pre",
    transform(code, id) {
      const path = id.split("?")[0]?.replaceAll("\\", "/");
      if (!path?.endsWith("schemas/examples/viewport.release.json")) return null;
      const document = JSON.parse(code) as { cards?: Card[] };
      const problems = (document.cards ?? []).flatMap(cardProblems);
      if (problems.length) {
        this.error(`release deck contains dev-only or stand-in cards: ${problems.join(", ")}`);
      }
      return null;
    },
    closeBundle() {
      const path = resolve("dist/library/assets.json");
      if (!existsSync(path)) return;
      const catalog = JSON.parse(readFileSync(path, "utf8")) as {
        assets?: unknown[];
        legacy_index?: Record<string, string>;
      };
      const problems = (catalog.assets ?? []).flatMap(assetProblems);
      for (const key of Object.keys(catalog.legacy_index ?? {})) {
        const tail = key.includes(":") ? key.slice(key.indexOf(":") + 1) : key;
        if (STUB_CARD_IDS.has(tail)) problems.push(key);
      }
      if (problems.length) {
        this.error(`release catalog contains dev-only or stand-in cards: ${problems.join(", ")}`);
      }
    },
  };
}

export default defineConfig({
  clearScreen: false,
  plugins: [failReleaseStubs()],
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
