import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";
import {
  BoundedSequences, McpClient, McpHttpError, controlBackoff, resolveMcpConfig,
  speechResult, toolBody, validateMcpConfig, type SpeechResult,
} from "../src/mcp-client.ts";

const config = { addr: "127.0.0.1:19345", token: "ab".repeat(32) };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const envelope = (body: unknown, id = 1) => ({ jsonrpc: "2.0", id, result: { content: [{ type: "text", text: JSON.stringify(body) }] } });

test("browser preview fails closed; native or explicit in-memory config is mandatory", async () => {
  await assert.rejects(resolveMcpConfig(), /Native bridge unavailable.*explicit/);
  assert.deepEqual(await resolveMcpConfig(undefined, config), config);
  assert.deepEqual(await resolveMcpConfig(async () => ({ ...config, handshake_ok: true, detail: "owned" })), config);
  await assert.rejects(resolveMcpConfig(async () => ({ ...config, handshake_ok: false, detail: "failed" }), config), /owned desktop MCP listener/);
  for (const addr of ["evil.example:8765", "127.0.0.1.evil:8765", "127.0.0.1:8765/path", "127.0.0.1:8765?token=x", "user@localhost:8765", "127.0.0.1:0", "localhost:65536"]) {
    assert.throws(() => validateMcpConfig({ ...config, addr }), /explicit loopback/);
  }
  assert.throws(() => validateMcpConfig({ ...config, token: "" }), /bearer token/);
  assert.equal(validateMcpConfig({ ...config, addr: "[::1]:19345" }).addr, "[::1]:19345");
});

test("IPv6 loopback uses the CSP-compatible localhost alias with negotiated port and bearer", async () => {
  let requested = "";
  const client = new McpClient({ ...config, addr: "[::1]:19345" }, async (url, init) => {
    requested = String(url);
    assert.equal(new Headers(init?.headers).get("authorization"), `Bearer ${config.token}`);
    return json(envelope({ ok: true }, JSON.parse(String(init?.body)).id));
  });
  await client.call("viewport_get", {});
  assert.equal(requested, "http://localhost:19345/mcp");
});

test("every MCP and control request uses the negotiated address and header-only authorization", async () => {
  const seen: Array<{ url: string; init: RequestInit }> = [];
  const fetcher: typeof fetch = async (url, init = {}) => {
    seen.push({ url: String(url), init });
    return String(url).includes("/control")
      ? json({ cursor: 3, events: [{ seq: 3, op: "navigate", args: { slide: "library" } }] })
      : json(envelope({ ok: true }, JSON.parse(String(init.body)).id));
  };
  const client = new McpClient(config, fetcher);
  await client.call("synth", { scriptText: "Full supplied script", agents: ["alice"], voice: "kokoro_onnx" });
  await client.call("tools_test", {});
  const controls = await client.controls(2);
  assert.equal(controls.cursor, 3);
  assert.equal(seen[0].url, "http://127.0.0.1:19345/mcp");
  assert.equal(JSON.parse(String(seen[0].init.body)).params.arguments.scriptText, "Full supplied script");
  assert.notEqual(JSON.parse(String(seen[0].init.body)).id, JSON.parse(String(seen[1].init.body)).id);
  for (const { url, init } of seen) {
    assert.equal(new Headers(init.headers).get("authorization"), `Bearer ${config.token}`);
    assert.equal(init.redirect, "error");
    assert.equal(init.credentials, "omit");
    assert.ok(!url.includes(config.token));
    assert.ok(!url.includes("8765"));
  }
});

test("HTTP failures, mismatched IDs, oversized and malformed replies are not speech", async () => {
  await assert.rejects(new McpClient(config, async () => json({}, 401)).call("synth", {}), (error: unknown) => error instanceof McpHttpError && error.status === 401);
  await assert.rejects(new McpClient(config, async () => json(envelope({}, 99))).call("synth", {}), /ID did not match/);
  await assert.rejects(new McpClient(config, async () => new Response("x", { headers: { "content-type": "text/html" } })).call("synth", {}), /content type/);
  await assert.rejects(new McpClient(config, async () => new Response("x", { headers: { "content-type": "application/json", "content-length": "1048577" } })).call("synth", {}), /download limit/);
  await assert.rejects(new McpClient(config, async () => json({ cursor: 0, events: [{ seq: "evil" }] })).controls(0), /control response/);
  await assert.rejects(new McpClient(config, async () => { throw new TypeError(`secret ${config.token}`); }).call("synth", {}), (error: Error) => !error.message.includes(config.token));
  assert.throws(() => toolBody({ error: { message: "Model missing; no speech" } }), /Model missing/);
  assert.throws(() => toolBody(envelope({ ok: false, error: "Synthesis failed" })), /Synthesis failed/);
  assert.throws(() => speechResult(envelope({ synthesizedSpeech: true })), /No verified speech/);
});

function wav(): Uint8Array {
  const bytes = new Uint8Array(48);
  const view = new DataView(bytes.buffer);
  const text = (offset: number, value: string) => bytes.set(new TextEncoder().encode(value), offset);
  text(0, "RIFF"); view.setUint32(4, bytes.length - 8, true);
  text(8, "WAVE"); text(12, "fmt "); view.setUint32(16, 16, true);
  view.setUint16(20, 1, true); view.setUint16(22, 1, true);
  view.setUint32(24, 24000, true); view.setUint32(28, 48000, true);
  view.setUint16(32, 2, true); view.setUint16(34, 16, true);
  text(36, "data"); view.setUint32(40, 4, true);
  view.setInt16(44, 100, true); view.setInt16(46, -100, true);
  return bytes;
}
function receipt(bytes: Uint8Array): SpeechResult {
  return { ok: true, synthesizedSpeech: true, wav: "speech-123.wav", manifest: "speech-123.json", sample_rate: 24000, samples: 2, duration_s: 2 / 24000, sha256: createHash("sha256").update(bytes).digest("hex") };
}

test("incomplete worker audio is playable only with explicit verified-artifact evidence", () => {
  const bytes = new Uint8Array(44);
  const partial = {...receipt(bytes),ok:false,complete:false,limit_reached:true,artifact_verified:true};
  assert.throws(()=>speechResult(envelope(partial)), /failed/);
  const result = speechResult(envelope(partial),true);
  assert.equal(result.ok,false); assert.equal(result.complete,false);
  assert.throws(()=>speechResult(envelope({...partial,artifact_verified:false}),true), /failed/);
  assert.throws(()=>speechResult(envelope({...partial,limit_reached:false}),true), /failed/);
  assert.throws(()=>speechResult(envelope({...partial,wav:"../private.wav"}),true), /verified speech/);
});

test("speech downloads only the issued relative WAV with bearer auth and verifies its digest", async () => {
  const bytes = wav();
  const result = speechResult(envelope(receipt(bytes)));
  const client = new McpClient(config, async (url, init) => {
    assert.equal(url, "http://127.0.0.1:19345/artifacts/speech-123.wav");
    assert.equal(new Headers(init?.headers).get("authorization"), `Bearer ${config.token}`);
    return new Response(bytes, { headers: { "Content-Type": "audio/wav", "Content-Length": String(bytes.length) } });
  });
  const blob = await client.speechBlob(result);
  assert.equal(blob.type, "audio/wav");
  assert.deepEqual(new Uint8Array(await blob.arrayBuffer()), bytes);
  const corrupt = bytes.slice(); corrupt[45] ^= 1;
  await assert.rejects(new McpClient(config, async () => new Response(corrupt, { headers: { "Content-Type": "audio/wav" } })).speechBlob(result), /SHA-256/);
  for (const wavName of ["../private.wav", "%2e%2e.wav", "http://evil/file.wav", "private.json", "a.wav?token=secret"]) {
    assert.throws(() => speechResult(envelope({ ...result, wav: wavName })), /No verified/);
  }
  assert.throws(() => speechResult(envelope({ ...result, synthesizedSpeech: false })), /No verified/);
  assert.throws(() => speechResult(envelope({ ...result, samples: 500 })), /No verified/);
});

test("generated media is downloaded from validated nested library paths", async () => {
  const path = "/library/generated/clip-123/spectrogram.png";
  const client = new McpClient(config, async (url, init) => {
    assert.equal(String(url), `http://${config.addr}${path}`);
    assert.equal(new Headers(init?.headers).get("authorization"), `Bearer ${config.token}`);
    return new Response(new Uint8Array([137, 80, 78, 71]), { headers: { "content-type": "image/png" } });
  });
  assert.equal((await client.libraryBlob(path)).type, "image/png");
  for (const invalid of ["/library/generated/../private.png", "/library/generated/%2e%2e/private.png",
    "/library//private.png", "/library/generated/a.png?token=secret", "/library/generated/a.png#fragment", "/library/C:/private.png"]) {
    await assert.rejects(client.libraryBlob(invalid));
  }
});

test("control history is bounded and error backoff is never a rapid retry loop", () => {
  const seen = new BoundedSequences(3);
  for (let seq = 0; seq < 10000; seq += 1) seen.add(seq);
  assert.equal(seen.size, 3);
  assert.equal(seen.has(0), false);
  assert.equal(seen.has(9999), true);
  seen.add(NaN);
  assert.equal(seen.size, 3);
  assert.deepEqual([0, 1, 2, 3, 4, 5, 100].map(controlBackoff), [1000, 2000, 4000, 8000, 16000, 30000, 30000]);
});
