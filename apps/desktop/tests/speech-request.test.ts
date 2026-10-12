import assert from "node:assert/strict";
import test from "node:test";
import { buildSpeechRequest, MAX_SCRIPT_BYTES, type TextAttachment } from "../src/speech-request.ts";

const alice = { agents: ["alice"], voice: "kokoro_onnx" };

test("VibeVoice forwards the selected execution contract without mutating it", async () => {
  const execution = { backend: "rocm", device: "gpu", device_index: 0, precision: "bfloat16", fallback: "error" } as const;
  const request = await buildSpeechRequest("Hello", [], { agents:["alice"], voice:"vibevoice", execution });
  assert.deepEqual(request.execution, execution);
  assert.notEqual(request.execution, execution);
});
function file(name: string, content: string, type = "text/plain"): TextAttachment {
  return { name, type, size: Buffer.byteLength(content), text: async () => content };
}

test("speech sends the entire supplied prose and selected voice/personas, never an example", async () => {
  const prose = `  ${"Supplied words. ".repeat(40)}\nFinal requested line.  `;
  const request = await buildSpeechRequest(prose, [], alice);
  assert.deepEqual(request, { scriptText: prose, agents: ["alice"], voice: "kokoro_onnx" });
  assert.equal(request.scriptText.length, prose.length);
  assert.ok(!("script" in request));
  assert.ok(!("output" in request));
  assert.notEqual(request.agents, alice.agents);
});

test("text attachments are included intact and in order", async () => {
  const request = await buildSpeechRequest("Opening words.", [file("one.txt", "First attachment."), file("two.md", "Second attachment.", "text/markdown")], alice);
  assert.equal(request.scriptText, "Opening words.\n\nFirst attachment.\n\nSecond attachment.");
  assert.equal((await buildSpeechRequest("", [file("only.txt", "File only script.")], alice)).scriptText, "File only script.");
});

test("unsupported attachments fail explicitly before any file is read", async () => {
  let read = false;
  const first = file("one.txt", "Words");
  first.text = async () => { read = true; return "Words"; };
  await assert.rejects(buildSpeechRequest("Prompt", [first, file("document.pdf", "Not plain text", "application/pdf")], alice), /Unsupported attachment.*document.pdf/);
  assert.equal(read, false);
  await assert.rejects(buildSpeechRequest("Prompt", [file("audio.wav", "RIFF", "audio/wav")], alice), /Unsupported attachment/);
  await assert.rejects(buildSpeechRequest("Prompt", [file("broken.txt", "\uFFFD")], alice), /valid UTF-8/);
});

test("byte and aggregate limits reject rather than silently truncate", async () => {
  await assert.rejects(buildSpeechRequest("é".repeat(MAX_SCRIPT_BYTES / 2 + 1), [], alice), /256 KiB/);
  await assert.rejects(buildSpeechRequest("Opening", [{ ...file("big.txt", ""), size: MAX_SCRIPT_BYTES }], alice), /256 KiB/);
  await assert.rejects(buildSpeechRequest("", [file("lying.txt", "x".repeat(MAX_SCRIPT_BYTES + 1))], alice), /256 KiB/);
  await assert.rejects(buildSpeechRequest("", [], alice), /Enter the words/);
  await assert.rejects(buildSpeechRequest("bad\0input", [], alice), /control characters/);
});

test("multiple selected speakers require valid nonempty numbered turns", async () => {
  const selection = { agents: ["frank", "alice"], voice: "kokoro_onnx" };
  const script = "Speaker 1 (Frank): First supplied line.\nSpeaker 2: Second supplied line.";
  assert.deepEqual(await buildSpeechRequest(script, [], selection), { scriptText: script, ...selection });
  await assert.rejects(buildSpeechRequest("Unassigned prose.", [], selection), /Multiple personas require Speaker/);
  await assert.rejects(buildSpeechRequest("Speaker 3: Not selected.", [], selection), /no selected persona/);
  await assert.rejects(buildSpeechRequest("Speaker 1:\nSpeaker 2: Words.", [], selection), /needs text/);
  await assert.rejects(buildSpeechRequest("Speaker 2: Uncast.", [], alice), /no selected persona/);
});

test("unsupported voices and unmapped personas cannot masquerade as Kokoro speech", async () => {
  await assert.rejects(buildSpeechRequest("Words", [], { agents: ["anton"], voice: "kokoro_onnx" }), /no Kokoro pack mapping/);
  await assert.rejects(buildSpeechRequest("Words", [], { agents: ["alice"], voice: "not-a-model" }), /No speech adapter/);
  await assert.rejects(buildSpeechRequest("Words", [], { agents: [], voice: "kokoro_onnx" }), /Choose/);
  await assert.rejects(buildSpeechRequest("Words", [], { agents: ["alice", "alice"], voice: "kokoro_onnx" }), /distinct/);
});

test("VibeVoice uses explicit acoustic references and bounded output without borrowing a persona voice", async () => {
  const request = await buildSpeechRequest("Speaker 1: Hello.\nSpeaker 2: Hi.", [], { agents:["alice","frank"], voice:"vibevoice" });
  assert.deepEqual(request.referenceIds, ["en-Alice_woman","en-Frank_man"]);
  assert.equal(request.maxSeconds, 30);
  await assert.rejects(buildSpeechRequest("Hello", [], {agents:["nova"],voice:"vibevoice"}), /managed VibeVoice reference/);
  const explicit = await buildSpeechRequest("Hello", [], {agents:["nova"],voice:"vibevoice",referenceIds:["en-Carter_man"]});
  assert.deepEqual(explicit.referenceIds,["en-Carter_man"]);
  await assert.rejects(buildSpeechRequest("Hello", [], {agents:["alice"],voice:"vibevoice",referenceIds:["../private"]}), /managed VibeVoice/);
  await assert.rejects(buildSpeechRequest("x".repeat(8193), [], {agents:["alice"],voice:"vibevoice"}), /8192 characters/);
  await assert.rejects(buildSpeechRequest("Hello", [], {agents:["alice"],voice:"vibevoice",maxSeconds:61}), /1–60 seconds/);
});
