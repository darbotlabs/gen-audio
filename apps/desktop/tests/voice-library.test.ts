import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { Ajv2020 } from "ajv/dist/2020.js";
import library from "../../../voices/manifest.json";
import profileSchema from "../../../schemas/voice_profile.schema.json";
import librarySchema from "../../../schemas/voice-library.schema.json";

test("unified library declares valid bindings and pins all migrated acoustic samples", () => {
  const ajv = new Ajv2020({strict:false,validateFormats:false}); ajv.addSchema(profileSchema);
  const validate = ajv.compile(librarySchema);
  assert.equal(validate(library),true,JSON.stringify(validate.errors));
  const ids = new Set(library.references.map(reference=>reference.id)); assert.equal(ids.size,9);
  for (const profile of Object.values(library.profiles)) {
    assert.equal(Object.keys(profile.engineManifests).length,7);
    for (const manifest of Object.values(profile.engineManifests)) {
      if ("referenceId" in manifest) assert.ok(ids.has(String(manifest.referenceId)));
    }
  }
  for (const reference of library.references) {
    const bytes = readFileSync(new URL(`../../../voices/${reference.file}`,import.meta.url));
    assert.equal(bytes.length,reference.bytes);
    assert.equal(createHash("sha256").update(bytes).digest("hex"),reference.sha256);
    assert.equal(bytes.toString("ascii",0,4),"RIFF");
    assert.equal(bytes.readUInt16LE(22),1); assert.equal(bytes.readUInt32LE(24),24000); assert.equal(bytes.readUInt16LE(34),16);
  }
  const bad = structuredClone(library); delete (bad.profiles.alice.engineManifests as any).misaki;
  assert.equal(validate(bad),false);
  const badCapability = structuredClone(library); (badCapability.profiles.alice.engineManifests.misaki as any).capability="script_tts";
  assert.equal(validate(badCapability),false);
});
