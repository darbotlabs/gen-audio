// PR #5 review, fix 5: release builds boot the Library deck without the dev
// fixtures; VITE_GEN_AUDIO_FIXTURES=1 (PR #4's flag) brings them back.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { DEV_FIXTURE_CLAIMS, fixturesRequested, isDevFixture, selectViewport } from "../src/viewport-source.ts";
import { modelCubes, type AssetEnvelope } from "../src/library-assets.ts";

const repo = (path: string) => fileURLToPath(new URL(`../../../${path}`, import.meta.url));
const load = (path: string) => JSON.parse(readFileSync(repo(path), "utf8"));
const DEV_ONLY = ["bench-ref", "cast-sample", "cube-fixture", "serve-gateway", "serve-node", "spec-fixture"];

test("only the exact flag value 1 requests fixtures", () => {
  assert.equal(fixturesRequested("1"), true);
  for (const flag of [undefined, "", "0", "true", "yes"]) assert.equal(fixturesRequested(flag), false, String(flag));
  assert.equal(selectViewport(undefined, "release", "example"), "release");
  assert.equal(selectViewport("1", "release", "example"), "example");
});

test("release assets.json and the release deck leave out the fixture, reference and sample/stub cards", () => {
  const release = load("apps/desktop/public/library/assets.json");
  const dev = load("schemas/asset-object/fixtures/assets.dev.json");
  assert.deepEqual(release.assets.filter(isDevFixture), []);
  assert.ok(dev.assets.every(isDevFixture));
  const devCards = dev.assets.filter((asset: { kind: string }) => asset.kind === "card").map((asset: { fields: { card_id: string } }) => asset.fields.card_id);
  assert.deepEqual([...devCards].sort(), DEV_ONLY);
  const releaseText = readFileSync(repo("apps/desktop/public/library/assets.json"), "utf8");
  for (const claim of DEV_FIXTURE_CLAIMS) assert.ok(!releaseText.includes(`"${claim}"`), claim);

  const shipped = load("schemas/examples/viewport.release.json");
  const example = load("schemas/examples/viewport.example.json");
  const ids = shipped.cards.map((card: { id: string }) => card.id);
  for (const id of DEV_ONLY) assert.ok(!ids.includes(id), `${id} in the release deck`);
  assert.deepEqual(
    shipped.cards,
    example.cards.filter((card: { id: string }) => !DEV_ONLY.includes(card.id)),
    "viewport.release.json is the example minus the dev fixtures (rerun build_assets)",
  );
  assert.ok(ids.includes("lib-bitdot-braille-vibevoice") && ids.includes("lib-misaki-kokoro"));
});

test("E1: no stub ids and no stand-in / not-remeasured text in the release catalog or deck", () => {
  for (const path of ["apps/desktop/public/library/assets.json", "schemas/examples/viewport.release.json"]) {
    const text = readFileSync(repo(path), "utf8");
    for (const id of ["cast-sample", "serve-node", "serve-gateway"]) assert.ok(!text.includes(`"${id}"`), `${id} in ${path}`);
    assert.doesNotMatch(text, /stand-in|not remeasured|<node>/i, path);
  }
});

// E1 addendum (widened): fixture controls, the fixture header text and the
// scrub test hook never reach a release build; the fixture Tauri commands are
// not registered in a release exe.
const RELEASE_FORBIDDEN = ["Load labeled example", "Run Python improve", "fixture canvases", "__genAudioScrub", "run_fixture_improve"];

function distText(dir: string): string {
  const parts: string[] = [];
  const walk = (at: string) => {
    for (const name of readdirSync(at)) {
      const path = join(at, name);
      if (statSync(path).isDirectory()) walk(path);
      else if (/\.(html|js|css|json)$/.test(name)) parts.push(readFileSync(path, "utf8"));
    }
  };
  walk(dir);
  return parts.join("\n");
}

async function viteBuild(flag: string | undefined): Promise<string> {
  const { build } = await import("vite");
  const outDir = mkdtempSync(join(tmpdir(), "ga-dist-"));
  const saved = process.env.VITE_GEN_AUDIO_FIXTURES;
  if (flag === undefined) delete process.env.VITE_GEN_AUDIO_FIXTURES;
  else process.env.VITE_GEN_AUDIO_FIXTURES = flag;
  try {
    await build({ root: repo("apps/desktop"), logLevel: "silent", build: { outDir, emptyOutDir: true } });
    return distText(outDir);
  } finally {
    if (saved === undefined) delete process.env.VITE_GEN_AUDIO_FIXTURES;
    else process.env.VITE_GEN_AUDIO_FIXTURES = saved;
    rmSync(outDir, { recursive: true, force: true });
  }
}

test("E1 addendum: index.html has no fixture controls or fixture header text", () => {
  const html = readFileSync(repo("apps/desktop/index.html"), "utf8");
  for (const text of RELEASE_FORBIDDEN) assert.ok(!html.includes(text), text);
  assert.ok(!html.includes('id="show-example"') && !html.includes('id="run-improve"'));
});

test("E1 addendum: a release vite build contains none of the fixture strings; a dev build does", async () => {
  const release = await viteBuild(undefined);
  for (const text of RELEASE_FORBIDDEN) assert.equal(release.split(text).length - 1, 0, `${text} in release dist`);
  // Not vacuous: the same build with the dev flag carries the controls and the hook.
  const dev = await viteBuild("1");
  for (const text of ["Load labeled example", "Run Python improve", "__genAudioScrub", "run_fixture_improve"]) {
    assert.ok(dev.includes(text), `${text} missing from the dev build`);
  }
});

test("E1 addendum: release Tauri builds do not register the fixture commands", () => {
  const lib = readFileSync(repo("apps/desktop/src-tauri/src/lib.rs"), "utf8");
  const release = lib.match(/#\[cfg\(not\(debug_assertions\)\)\]\s*fn invoke_handler\(\)[^{]*\{([^}]*)\}/);
  assert.ok(release, "a cfg(not(debug_assertions)) invoke_handler");
  const registered = release[1].replace(/tauri::generate_handler!\[|\]/g, "").split(",").map((name) => name.trim()).filter(Boolean);
  assert.deepEqual(registered.sort(), ["connector_statuses", "mcp_status"]);
  for (const name of ["viewport_example", "run_fixture_improve"]) {
    assert.ok(!registered.includes(name), `${name} registered in release`);
    // The function itself is not compiled in release, so no release handler could name it.
    assert.match(lib, new RegExp(`#\\[cfg\\(debug_assertions\\)\\]\\s*#\\[tauri::command\\]\\s*fn ${name}\\(`), `${name} is debug-only`);
  }
  assert.match(lib, /\.invoke_handler\(invoke_handler\(\)\)/);
  // Release grants no fixture permission: build.rs reads only capabilities/default.json there.
  const caps = readFileSync(repo("apps/desktop/src-tauri/capabilities/default.json"), "utf8");
  assert.doesNotMatch(caps, /fixture|viewport-example/);
  const build = readFileSync(repo("apps/desktop/src-tauri/build.rs"), "utf8");
  assert.match(build, /const RELEASE_COMMANDS: &\[&str\] = &\["connector_statuses", "mcp_status"\];/);
  assert.match(build, /CARGO_CFG_DEBUG_ASSERTIONS/);
  assert.match(build, /capabilities_path_pattern\("\.\/capabilities\/default\.json"\)/);
  assert.ok(!/generate_handler!\[[^\]]*run_fixture_improve/.test(lib.replace(/#\[cfg\(debug_assertions\)\][^]*?\n}\n/g, "")));
});

// E4: the voice model cards' Cube tabs, from the real release catalog.
test("E4: each voice model's Cube tab lists that engine's real rev 3 cubes", () => {
  const assets: AssetEnvelope[] = load("apps/desktop/public/library/assets.json").assets;
  const manifest = load("apps/desktop/public/library/manifest.json");
  const titles = (view: ReturnType<typeof modelCubes>) => (view.state === "cubes" ? view.rows.map((row) => row.clipTitle).sort() : view.state);
  const onnx = modelCubes(assets, "kokoro_onnx");
  assert.deepEqual(titles(onnx), ["Cube explainer (kokoro-onnx)", "kokoro-onnx"]);
  assert.equal(onnx.state === "cubes" && onnx.offline, false);
  const dayour = modelCubes(assets, "kokoro_dayour");
  assert.deepEqual(titles(dayour), ["dayour/kokoro", "misaki\u2192kokoro"]);
  const vibe = modelCubes(assets, "vibevoice");
  assert.deepEqual(titles(vibe), ["Bitdot braille (VibeVoice-1.5B)"]);
  assert.equal(vibe.state === "cubes" && vibe.offline, true, "VibeVoice's bitdot cube is an offline run");
  for (const view of [onnx, dayour, vibe]) {
    assert.ok(view.state === "cubes");
    for (const row of view.rows) {
      assert.equal(row.revision, 3);
      assert.equal(row.layerMethod, "library_r3");
      assert.ok(row.shape && row.invHdr !== null && row.layerScore !== null && row.jsonUrl.startsWith("/library/"), row.clipTitle);
    }
  }
  const misaki = modelCubes(assets, "misaki");
  assert.deepEqual(misaki, { state: "g2p", clips: [{ clipId: "lib-misaki-kokoro", clipTitle: "misaki\u2192kokoro" }] });
  for (const id of ["magpie", "pocket_tts"]) {
    const reason = manifest.clips.find((clip: { engineId: string; status: string }) => clip.engineId === id && clip.status !== "ok").reason;
    assert.deepEqual(modelCubes(assets, id), { state: "unavailable", reason }, id);
  }
  assert.deepEqual(modelCubes(assets, "nope"), { state: "unknown" });
});

test("E4: every audio clip with a WAV has exactly one rev 3 cube made from that WAV", () => {
  const assets: AssetEnvelope[] = load("apps/desktop/public/library/assets.json").assets;
  const clips = assets.filter((asset) => asset.kind === "audio_clip" && asset.media.some((item) => item.role === "wav"));
  assert.equal(clips.length, 5);
  for (const clip of clips) {
    const wav = clip.media.find((item) => item.role === "wav")!;
    const cubes = assets.filter((asset) => asset.kind === "cube_ihdr" && asset.src.includes(clip.uid));
    assert.equal(cubes.length, 1, clip.legacy_id);
    assert.equal(cubes[0].fields.cube_revision, 3, clip.legacy_id);
    assert.equal(cubes[0].fields.source_sha256, wav.sha256, clip.legacy_id);
    const doc = load(`apps/desktop/public${mediaPath(cubes[0])}`);
    assert.equal(doc.source_sha256, wav.sha256, `${clip.legacy_id} cube JSON source_sha256`);
    assert.equal(doc.provenance.layer_method, "library_r3");
    assert.equal(doc.provenance.generator, "src/gen_audio/cube_layers.py");
    assert.equal((cubes[0].provenance?.params as Record<string, unknown>).bins_inferred_from_shape, false, clip.legacy_id);
  }
});

const mediaPath = (asset: AssetEnvelope) => `/library/${asset.media.find((item) => item.role === "cube_json")!.path}`;

test("Library cards' hand-written cube facts match the cube JSON", () => {
  const example = load("schemas/examples/viewport.example.json");
  for (const card of example.cards.filter((c: { kind: string; body: { cubeJsonUrl?: string } }) => c.kind === "LibraryClip" && c.body.cubeJsonUrl)) {
    const doc = load(`apps/desktop/public${card.body.cubeJsonUrl}`);
    if (typeof card.body.inv_hdr === "number") assert.ok(Math.abs(card.body.inv_hdr - doc.inv_hdr) < 5e-6, `${card.id} body.inv_hdr`);
    const facts: Array<{ title: string; value: string }> = (card.adaptive?.body ?? []).flatMap((block: { facts?: unknown[] }) => block.facts ?? []);
    const inv = facts.find((fact) => fact.title === "inv-HDR");
    if (inv) assert.equal(inv.value, doc.inv_hdr.toFixed(3), `${card.id} adaptive inv-HDR (not layer_score)`);
    const cube = facts.find((fact) => fact.title === "Cube");
    if (cube) assert.ok(cube.value.startsWith(`rev ${doc.cube_revision} \u00b7 ${doc.cube_shape_f_t[1]} time bins`), `${card.id} adaptive Cube: ${cube.value}`);
  }
});
