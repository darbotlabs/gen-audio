// Cross-language parity: the TS mirror and Ajv2020 must agree with the golden
// vectors that cargo test also asserts (schemas/asset-object/vectors/v1.json).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Ajv2020 } from "ajv/dist/2020.js";
import {
  AssetError,
  binFramesInferred,
  checkEnvelopeShape,
  floatTokenPaths,
  checkMediaPath,
  envelopeIdentityUid,
  glyphBytesFromUid,
  glyphDots,
  glyphFromUid,
  hueClass,
  mint,
  msFromFrames,
  normalizeNfc,
  parseIdentityJson,
  parseUid,
  roundHalfUp,
} from "../src/asset.ts";

const repo = (path: string) => fileURLToPath(new URL(`../../../${path}`, import.meta.url));
const load = (path: string) => JSON.parse(readFileSync(repo(path), "utf8"));
const vectorsText = readFileSync(repo("schemas/asset-object/vectors/v1.json"), "utf8");
const vectors = JSON.parse(vectorsText);
const vectorFloatPaths = floatTokenPaths(vectorsText);
/** Float-token paths inside one subtree, relative to it (`envelopes.12.envelope.` -> `fields.x`). */
const floatPathsUnder = (paths: string[], prefix: string) => paths.filter((path) => path.startsWith(prefix)).map((path) => path.slice(prefix.length));

function codeOf(run: () => unknown): string | null {
  try {
    run();
    return null;
  } catch (error) {
    if (error instanceof AssetError) return error.code;
    throw error;
  }
}

function schemaValidator() {
  const ajv = new Ajv2020({ allErrors: true, strict: false, validateFormats: false });
  ajv.addSchema(load("schemas/card-viewport.schema.json"));
  ajv.addSchema(load("schemas/voice_profile.schema.json"));
  return ajv.compile(load("schemas/asset-object.schema.json"));
}

test("mint vectors: canonical, preimage, digest, uid, glyph", () => {
  for (const vector of vectors.mint) {
    let fields = parseIdentityJson(vector.fields_json);
    if (vector.normalize_nfc) fields = normalizeNfc(fields);
    const minted = mint(vector.kind, fields, vector.media, vector.src);
    const expect = vector.expect;
    assert.equal(minted.canonical, expect.canonical, `${vector.name} canonical`);
    assert.equal(minted.preimageHex, expect.preimage_hex, `${vector.name} preimage`);
    assert.equal(minted.digestHex, expect.digest_hex, `${vector.name} digest`);
    assert.equal(minted.uid, expect.uid, `${vector.name} uid`);
    assert.equal(minted.glyph, expect.glyph, `${vector.name} glyph`);
    assert.equal(glyphFromUid(minted.uid), expect.glyph, `${vector.name} glyph from uid`);
    assert.deepEqual([...expect.glyph].map((ch) => ch.codePointAt(0)), expect.glyph_codepoints);
    assert.deepEqual(glyphBytesFromUid(minted.uid).map(glyphDots), expect.glyph_dots, `${vector.name} dots`);
    assert.equal(hueClass(vector.kind), expect.hue_class);
  }
});

test("canonical rejects", () => {
  for (const vector of vectors.canonical_reject) {
    let error: string | null;
    try {
      error = codeOf(() => mint(vector.kind, parseIdentityJson(vector.fields_json)));
    } catch {
      error = "lone_surrogate"; // JSON.parse accepts lone surrogates; mint rejects them, so this is unreachable
    }
    assert.equal(error, vector.error, vector.name);
  }
});

test("identity rejects: duplicate media role, duplicate src, src fan-out", () => {
  for (const vector of vectors.identity_reject) {
    assert.equal(codeOf(() => mint(vector.kind, vector.fields, vector.media, vector.src)), vector.error, vector.name);
  }
});

test("rounding: half up, same table as Rust and gen_audio.identity", () => {
  for (const vector of vectors.rounding) {
    const got =
      vector.op === "ms_from_frames"
        ? msFromFrames(vector.frames, vector.rate)
        : vector.op === "bin_frames_inferred"
          ? binFramesInferred(vector.duration_s, vector.sample_rate_hz, vector.time_bins)
          : roundHalfUp(vector.value * vector.scale);
    assert.equal(got, vector.expect, JSON.stringify(vector));
  }
  for (const vector of vectors.mint.filter((item: { derived_from?: unknown }) => item.derived_from)) {
    const fields = JSON.parse(vector.fields_json);
    assert.equal(fields.duration_ms, msFromFrames(vector.derived_from.frames, vector.derived_from.sample_rate_hz), vector.name);
  }
  assert.equal(msFromFrames(24_008, 16_000), 1501);
  assert.equal(binFramesInferred(157.134, 24_000, 96), 39_283, "B1' near-tie: binary64 two-step");
});

test("uid grammar and media paths", () => {
  for (const vector of vectors.uid_parse) assert.equal(codeOf(() => parseUid(vector.uid)), vector.error, vector.uid);
  for (const vector of vectors.media_path) assert.equal(codeOf(() => checkMediaPath(vector.path)), vector.error, vector.path);
});

const TS_SHAPE_CODES = new Set([
  "unknown_root_key", "bad_legacy_id", "uid_mismatch", "missing_glyph", "bad_title", "glyph_mismatch", "bad_display_rev",
  "unknown_display_key", "bad_wav_url", "fan_out_exceeded", "missing_media_role", "duplicate_media_role", "float_in_identity",
  "bad_honesty",
]);

test("envelopes: Ajv2020 agrees with schema_valid; uids recompute", () => {
  const validate = schemaValidator();
  vectors.envelopes.forEach((vector: { name: string; envelope: Record<string, unknown> & { uid: string }; schema_valid: boolean; error: string | null }, index: number) => {
    const floatPaths = floatPathsUnder(vectorFloatPaths, `envelopes.${index}.envelope.`);
    const valid = validate(vector.envelope) as boolean;
    assert.equal(valid, vector.schema_valid, `${vector.name}: ${JSON.stringify(validate.errors?.slice(0, 3))}`);
    if (vector.error === null) assert.equal(envelopeIdentityUid(vector.envelope), vector.envelope.uid, `${vector.name} uid`);
    if (vector.error === "uid_mismatch") assert.notEqual(envelopeIdentityUid(vector.envelope), vector.envelope.uid);
    if (vector.name === "title_change_keeps_uid") assert.equal(envelopeIdentityUid(vector.envelope), vector.envelope.uid);
    // The TS structural checks agree with Rust wherever they speak; the codes
    // they own must match exactly.
    const tsCode = codeOf(() => checkEnvelopeShape(vector.envelope, floatPaths));
    if (tsCode !== null) assert.equal(tsCode, vector.error, `${vector.name} TS shape`);
    if (vector.error !== null && TS_SHAPE_CODES.has(vector.error)) assert.equal(tsCode, vector.error, `${vector.name} TS shape owns ${vector.error}`);
  });
});

test("floatTokenPaths finds fraction and exponent tokens by path", () => {
  assert.deepEqual(floatTokenPaths('{"a":1,"b":[1.0,{"c":1e3}],"d":"2.5","e":-0.0,"f":true}'), ["b.0", "b.1.c", "e"]);
});

test("library assets.json: schema-valid, uids and glyphs recompute, legacy index pinned", () => {
  const validate = schemaValidator();
  const catalogText = readFileSync(repo("apps/desktop/public/library/assets.json"), "utf8");
  const catalog = JSON.parse(catalogText);
  const catalogFloats = floatTokenPaths(catalogText);
  const fixtures = load("schemas/asset-object/vectors/fixtures_v1.json");
  // fixtures_v1.json pins release + dev; assets.json carries the release half.
  const dev = load("schemas/asset-object/fixtures/assets.dev.json");
  assert.deepEqual({ ...catalog.legacy_index, ...dev.legacy_index }, fixtures.legacy_index);
  catalog.assets.forEach((asset: Record<string, unknown> & { uid: string; legacy_id?: string; display: { glyph: string } }, index: number) => {
    assert.ok(validate(asset), `${asset.uid}: ${JSON.stringify(validate.errors?.slice(0, 3))}`);
    assert.equal(envelopeIdentityUid(asset), asset.uid, `${asset.legacy_id} uid`);
    assert.equal(glyphFromUid(asset.uid), asset.display.glyph);
    checkEnvelopeShape(asset, floatPathsUnder(catalogFloats, `assets.${index}.`));
  });
  const viewport = load("schemas/examples/viewport.example.json");
  for (const card of viewport.cards) assert.equal(card.uid, fixtures.legacy_index[`card:${card.id}`], card.id);
});
