import { test } from "node:test";
import assert from "node:assert/strict";
import { draftPreview } from "../src/authoring-panel.ts";
import { masteredAudioResult, speechResult } from "../src/mcp-client.ts";

const envelope = (value: object) => ({ result: { content: [{ text: JSON.stringify(value) }] } });

test("draft review captures the exact displayed request without mutable form state", () => {
  const request = { topic: "Sound synthesis", agents: ["alice"], durationMinutes: 3 };
  const preview = draftPreview(envelope({ prompt: "Exact provider request", promptSha256: "a".repeat(64) }), request, "gpt");
  request.topic = "Changed after preview";
  request.agents.push("frank");
  assert.equal(preview.prompt, "Exact provider request");
  assert.equal(preview.request.topic, "Sound synthesis");
  assert.deepEqual(preview.request.agents, ["alice"]);
  assert.throws(() => draftPreview(envelope({ prompt: "x", promptSha256: "invalid" }), request, "gpt"));
  assert.throws(() => draftPreview(envelope({ prompt: "x", promptSha256: "a".repeat(64) }), request, "untrusted"));
});

test("mastered audio is a processing result, never a fabricated speech claim", () => {
  const result = {
    ok: true, processedAudio: true, synthesizedSpeech: false, wav: "master.wav", manifest: "master.json",
    sample_rate: 24000, samples: 24000, duration_s: 1, sha256: "a".repeat(64), sourceSha256: "b".repeat(64),
    report: { profile: "podcast" },
  };
  assert.equal(masteredAudioResult(envelope(result)).synthesizedSpeech, false);
  assert.throws(() => speechResult(envelope(result)));
  for (const patch of [
    { synthesizedSpeech: true }, { samples: 1 }, { wav: "../master.wav" },
    { sourceSha256: "" }, { report: null }, { duration_s: 0 },
  ]) {
    assert.throws(() => masteredAudioResult(envelope({ ...result, ...patch })));
  }
});
