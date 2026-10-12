import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { SLIDE_SCHEMAS } from "../src/render";
import { profileMatches, voiceProfileForModel } from "../src/profiles";
import { validateVoiceProfile } from "../src/validate";
import { PERSONAS } from "../src/catalog";

test("agent profile library has a separate tab and mapped-engine search", () => {
  assert.equal(SLIDE_SCHEMAS.find(slide => slide.id === "models")?.layer, "models");
  assert.equal(SLIDE_SCHEMAS.find(slide => slide.id === "profiles")?.layer, "profiles");
  const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  assert.match(html, /id="layer-switch"[^>]*role="tablist"/);
  assert.equal(SLIDE_SCHEMAS.find(slide => slide.id === "profiles")?.title, "Agent Voice Profiles");
  const alice = voiceProfileForModel("alice", "kokoro_onnx")!;
  assert.equal(profileMatches(alice, "alice SDK", "kokoro_onnx"), true);
  assert.equal(profileMatches(alice, "", "magpie"), false);
  assert.equal(profileMatches(alice, "not-a-profile", ""), false);
});

test("every agent exports schema-valid bindings, projects, samples and Adaptive Card actions", () => {
  for (const persona of PERSONAS) {
    const profile = voiceProfileForModel(persona.id, "kokoro_onnx")!;
    assert.equal(validateVoiceProfile(profile), null, persona.id);
    assert.deepEqual(profile.metadata, { artist: persona.name });
    assert.ok((profile.linkedProjects as any[]).length >= 2);
    const card = profile.adaptiveCard as Record<string, any>;
    assert.equal(card.type, "AdaptiveCard");
    assert.ok(card.body.some((item: any) => item.type === "FactSet" && item.facts.length === 7));
    assert.deepEqual(card.actions[0].data.arguments, { agents: [persona.id] });
    const bad = structuredClone(profile); bad.metadata = { artist: "Kokoro" };
    assert.notEqual(validateVoiceProfile(bad), null);
    const unsafe = structuredClone(profile); (unsafe.linkedProjects as any[])[0].path = "apps/../secrets";
    assert.notEqual(validateVoiceProfile(unsafe), null);
  }
});
