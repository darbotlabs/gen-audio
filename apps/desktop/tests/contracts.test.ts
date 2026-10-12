import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { Ajv2020 } from "ajv/dist/2020.js";
import { validateViewport, validateVoiceProfile } from "../src/validate.ts";
import { parseLibraryCatalog, loadLibraryCatalog, mediaUrl } from "../src/library-assets.ts";
import { PERSONAS, VOICE_MODELS, personaKokoroPack } from "../src/catalog.ts";
import { voiceProfileForModel } from "../src/profiles.ts";

const load = (path: string) => JSON.parse(readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8"));
const viewport = load("schemas/examples/viewport.example.json");
const profile = load("schemas/examples/voice_profile.alice.json");
const catalog = load("apps/desktop/public/library/assets.json");
const vectors = load("schemas/validation-vectors.json");
const ajv = new Ajv2020({ strict: false, validateFormats: false });
const profileSchema = ajv.compile(load("schemas/voice_profile.schema.json"));
const viewportSchema = ajv.compile(load("schemas/card-viewport.schema.json"));
const assetSchema = ajv.compile(load("schemas/asset-object.schema.json"));
const wrap = (card: unknown) => ({ version: "1.0", title: "Contract test", cards: [card] });
const cardForProfile = (body: unknown) => wrap({ id: "profile-test", title: "Profile", kind: "VoiceProfile", body });

function edit(value: Record<string, any>, pointer: string, remove: boolean, replacement?: unknown): void {
  const parts = pointer.slice(1).split("/").map((part) => part.replace(/~1/g, "/").replace(/~0/g, "~"));
  const key = parts.pop()!;
  const parent = parts.reduce((node, part) => node[part], value);
  if (remove) delete parent[key];
  else parent[key] = replacement;
}

test("shared schema/runtime validation vectors", () => {
  for (const vector of vectors.cases) {
    const [target, kind] = vector.base.split(":");
    const base = target === "viewport" ? viewport : target === "profile" ? profile
      : target === "card" ? viewport.cards.find((card: any) => card.kind === kind)
        : catalog.assets.find((asset: any) => asset.kind === kind);
    assert.ok(base, vector.name);
    const value = structuredClone(base);
    for (const [pointer, replacement] of Object.entries(vector.set ?? {})) edit(value, pointer, false, replacement);
    for (const pointer of vector.remove ?? []) edit(value, pointer, true);
    const doc = target === "card" ? wrap(value) : value;
    const schema = target === "profile" ? profileSchema : target === "asset" ? assetSchema : viewportSchema;
    assert.equal(schema(doc), vector.valid, `${vector.name}: ${JSON.stringify(schema.errors)}`);
    if (target === "asset") {
      const altered = structuredClone(catalog);
      altered.assets[altered.assets.findIndex((asset: any) => asset.uid === base.uid)] = value;
      if (vector.valid) assert.doesNotThrow(() => parseLibraryCatalog(altered), vector.name);
      else assert.throws(() => parseLibraryCatalog(altered), undefined, vector.name);
    } else {
      const runtime = target === "profile" ? validateVoiceProfile(value) : validateViewport(doc, "allowed");
      assert.equal(runtime === null, vector.valid, `${vector.name}: ${runtime}`);
      if (target === "profile") {
        assert.equal(viewportSchema(cardForProfile(value)), vector.valid, `${vector.name}: embedded schema`);
        assert.equal(validateViewport(cardForProfile(value)) === null, vector.valid, `${vector.name}: embedded runtime`);
      }
    }
  }
});

test("only explicit persona refs assign Kokoro packs", () => {
  for (const [persona, pack] of Object.entries(vectors.packs)) {
    assert.equal(personaKokoroPack(persona) ?? null, pack, persona);
  }
});

test("profile framework describes all known models without claiming readiness or speech", () => {
  for (const persona of PERSONAS) {
    for (const model of VOICE_MODELS) {
      const value = voiceProfileForModel(persona.id, model.id)!;
      assert.equal(validateVoiceProfile(value), null, `${persona.id}/${model.id}`);
      assert.equal(validateViewport(cardForProfile(value)), null);
      assert.equal(value.synthesizedSpeech, false);
      assert.equal(value.notPodcast, true);
      assert.equal(Object.hasOwn(value, "wavUrl"), false);
      if (model.id !== "kokoro_onnx") assert.equal(value.spectrogram3d, "none");
    }
  }
  for (const id of ["personaplex"]) {
    const model = VOICE_MODELS.find((entry) => entry.id === id)!;
    assert.equal(model.synthAdapter, false);
    assert.equal(model.unavailable, true);
    assert.match(String(voiceProfileForModel("alice", id)?.disclaimer), /unavailable.*not a podcast render/);
  }
  const vibevoice = VOICE_MODELS.find(entry => entry.id === "vibevoice")!;
  assert.equal(vibevoice.synthAdapter, true);
  assert.equal(vibevoice.unavailable, false);
  for (const person of PERSONAS) {
    const profile = voiceProfileForModel(person.id, "vibevoice")!;
    assert.equal(profile.profileVersion, "2.0");
    assert.deepEqual(Object.keys(profile.engineManifests as object).sort(), VOICE_MODELS.map(model => model.id).sort());
  }
  assert.equal(voiceProfileForModel("missing", "kokoro_onnx"), undefined);
  assert.equal(voiceProfileForModel("alice", "af_heart"), undefined);
});

test("profile v2 descriptors stay synchronized across generated files, assets, cards and vectors", () => {
  for (const persona of PERSONAS) {
    const expected = voiceProfileForModel(persona.id, "kokoro_onnx")!;
    const asset = catalog.assets.find((entry: any) => entry.kind === "voice_profile" && entry.legacy_id === persona.id);
    assert.ok(asset, persona.id);
    assert.deepEqual(asset.body, expected, `${persona.id}: asset body`);
    assert.deepEqual(load(`apps/desktop/public/library/voice_profile.${persona.id}.json`), expected, `${persona.id}: profile file`);
    const card = viewport.cards.find((entry: any) => entry.kind === "VoiceProfile" && entry.body.personaId === persona.id);
    assert.ok(card, persona.id);
    assert.equal(card.body.profileVersion, expected.profileVersion);
    assert.deepEqual(card.body.engineManifests, expected.engineManifests, `${persona.id}: viewport bindings`);
    const cardAsset = catalog.assets.find((entry: any) => entry.kind === "card" && entry.legacy_id === card.id);
    assert.ok(cardAsset, persona.id);
    assert.deepEqual(cardAsset.body.body, card.body, `${persona.id}: card asset body`);
    assert.deepEqual(cardAsset.relations.bound_to, [asset.uid], `${persona.id}: profile binding`);
  }
  const golden = load("schemas/asset-object/vectors/v1.json");
  const optimus = voiceProfileForModel("optimus", "kokoro_onnx")!;
  for (const name of ["ok_profile_v2", "profile_claims_speech", "profile_not_marked_not_podcast"]) {
    const vector = golden.envelopes.find((entry: any) => entry.name === name);
    assert.ok(vector, name);
    assert.deepEqual(vector.envelope.body, optimus, `${name}: stale profile descriptor`);
  }
});

test("library ingestion validates the complete catalog, uid, glyph, paths and index", () => {
  const parsed = parseLibraryCatalog(catalog);
  assert.equal(parsed.byUid.size, catalog.assets.length);
  const clip = parsed.clipForTile("lib-kokoro-onnx")!;
  assert.equal(mediaUrl(clip, "wav"), "/library/library_kokoro_onnx.wav");
  assert.equal(parsed.derivedFrom(clip.uid, "cube_ihdr")?.kind, "cube_ihdr");
  assert.equal(parsed.cubeForUrl("/library/library_kokoro_onnx_cube3d.json")?.kind, "cube_ihdr");
  const mutations: Array<(doc: any) => void> = [
    (doc) => { doc.extra = true; },
    (doc) => { delete doc.assets; },
    (doc) => { doc.assets.push(structuredClone(doc.assets[0])); },
    (doc) => { doc.assets[0].uid = doc.assets[1].uid; },
    (doc) => { doc.assets[0].display.glyph = "⣿⣿"; },
    (doc) => { doc.assets[0].fields.engine = "tampered"; },
    (doc) => { doc.assets[0].media[0].path = "a..b.wav"; },
    (doc) => { doc.assets[0].media[0].bytes = -1; },
    (doc) => { doc.assets[0].media[0].sha256 = "broken"; },
    (doc) => { doc.legacy_index = {}; },
    (doc) => { doc.legacy_index["audio_clip:nonexistent"] = doc.assets[0].uid; },
  ];
  for (const mutate of mutations) {
    const doc = structuredClone(catalog);
    mutate(doc);
    assert.throws(() => parseLibraryCatalog(doc));
  }
});

test("catalog load surfaces errors, retries, then shares the validated result", async () => {
  const original = globalThis.fetch;
  let calls = 0;
  globalThis.fetch = async () => {
    calls++;
    if (calls === 1) return new Response("not JSON", { status: 200 });
    if (calls === 2) return Response.json({ assets: [] });
    if (calls === 3) return new Response("Unavailable", { status: 503 });
    return Response.json(catalog);
  };
  try {
    await assert.rejects(loadLibraryCatalog());
    await assert.rejects(loadLibraryCatalog(), /Invalid library catalog/);
    await assert.rejects(loadLibraryCatalog(), /503/);
    const one = loadLibraryCatalog();
    const two = loadLibraryCatalog();
    assert.equal(one, two);
    assert.equal((await one)?.byUid.size, catalog.assets.length);
    assert.equal(calls, 4);
  } finally {
    globalThis.fetch = original;
  }
});

test("non-JSON numeric values never bypass viewport validation", () => {
  for (const value of [NaN, Infinity, -Infinity]) {
    for (const field of ["duration_s", "sample_rate", "inv_hdr"]) {
      const clip = structuredClone(viewport.cards.find((card: any) => card.kind === "LibraryClip"));
      clip.body[field] = value;
      assert.notEqual(validateViewport(wrap(clip)), null, field);
    }
  }
  const duplicate = structuredClone(viewport);
  duplicate.cards.push(structuredClone(duplicate.cards[0]));
  assert.match(validateViewport(duplicate, "allowed") ?? "", /duplicate card id/);
});
