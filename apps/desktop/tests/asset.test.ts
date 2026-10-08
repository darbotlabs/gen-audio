// Cross-language parity: the TS mirror and Ajv2020 must agree with the golden
// vectors that cargo test also asserts (schemas/asset-object/vectors/v1.json).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Ajv2020 } from "ajv/dist/2020.js";
import {
  AssetError,
  checkMediaPath,
  envelopeIdentityUid,
  glyphBytesFromUid,
  glyphDots,
  glyphFromUid,
  hueClass,
  mint,
  normalizeNfc,
  parseUid,
} from "../src/asset.ts";

const repo = (path: string) => fileURLToPath(new URL(`../../../${path}`, import.meta.url));
const load = (path: string) => JSON.parse(readFileSync(repo(path), "utf8"));
const vectors = load("schemas/asset-object/vectors/v1.json");

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
    let fields = JSON.parse(vector.fields_json);
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
    const fields = JSON.parse(vector.fields_json);
    assert.equal(codeOf(() => mint(vector.kind, fields)), vector.error, vector.name);
  }
});

test("uid grammar and media paths", () => {
  for (const vector of vectors.uid_parse) assert.equal(codeOf(() => parseUid(vector.uid)), vector.error, vector.uid);
  for (const vector of vectors.media_path) assert.equal(codeOf(() => checkMediaPath(vector.path)), vector.error, vector.path);
});

test("envelopes: Ajv2020 agrees with schema_valid; uids recompute", () => {
  const validate = schemaValidator();
  for (const vector of vectors.envelopes) {
    const valid = validate(vector.envelope) as boolean;
    assert.equal(valid, vector.schema_valid, `${vector.name}: ${JSON.stringify(validate.errors?.slice(0, 3))}`);
    if (vector.error === null) assert.equal(envelopeIdentityUid(vector.envelope), vector.envelope.uid, `${vector.name} uid`);
    if (vector.error === "uid_mismatch") assert.notEqual(envelopeIdentityUid(vector.envelope), vector.envelope.uid);
    if (vector.name === "title_change_keeps_uid") assert.equal(envelopeIdentityUid(vector.envelope), vector.envelope.uid);
  }
});

test("library assets.json: schema-valid, uids and glyphs recompute, legacy index pinned", () => {
  const validate = schemaValidator();
  const catalog = load("apps/desktop/public/library/assets.json");
  const fixtures = load("schemas/asset-object/vectors/fixtures_v1.json");
  assert.deepEqual(catalog.legacy_index, fixtures.legacy_index);
  for (const asset of catalog.assets) {
    assert.ok(validate(asset), `${asset.uid}: ${JSON.stringify(validate.errors?.slice(0, 3))}`);
    assert.equal(envelopeIdentityUid(asset), asset.uid, `${asset.legacy_id} uid`);
    assert.equal(glyphFromUid(asset.uid), asset.display.glyph);
  }
  const viewport = load("schemas/examples/viewport.example.json");
  for (const card of viewport.cards) assert.equal(card.uid, fixtures.legacy_index[`card:${card.id}`], card.id);
});
