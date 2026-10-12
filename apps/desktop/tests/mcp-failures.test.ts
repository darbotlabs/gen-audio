import { test } from "node:test";
import assert from "node:assert/strict";
import { McpClient, resolveMcpConfig } from "../src/mcp-client.ts";
import { McpFailureCounter, assetResolveFailure, fetchLibraryBlob, noteAssetResolveFailure } from "../src/play-control.ts";

const config = { addr: "127.0.0.1:19345", token: "ab".repeat(32) };

test("failed discovery never falls back to an unauthenticated listener", async () => {
  await assert.rejects(resolveMcpConfig(async () => ({ addr: config.addr, handshake_ok: false, detail: "failed" }), config), /owned desktop/);
  await assert.rejects(resolveMcpConfig(async () => { throw new TypeError("invoke"); }, config), /invoke/);
  await assert.rejects(resolveMcpConfig(), /no endpoint was contacted/);
});

test("authenticated library failures are counted once per kind and every attempt", async () => {
  const warnings: string[] = [];
  const failures = new McpFailureCounter(message => warnings.push(message));
  const client = new McpClient(config, async (url, init) => {
    assert.equal(url, `http://${config.addr}/library/x.wav`);
    assert.equal(new Headers(init?.headers).get("authorization"), `Bearer ${config.token}`);
    return new Response("no", { status: 404 });
  });
  for (let n = 0; n < 2; n++) assert.equal(await fetchLibraryBlob(client, "/library/x.wav", failures), null);
  assert.deepEqual(failures.snapshot(), { "library_media:http_404": 2 });
  assert.equal(warnings.length, 1);
});

test("library network failures are counted and unsafe paths never issue a request", async () => {
  const failures = new McpFailureCounter(() => {});
  let calls = 0;
  const client = new McpClient(config, async () => { calls++; throw new TypeError("offline"); });
  assert.equal(await fetchLibraryBlob(client, "/library/x.wav", failures), null);
  assert.equal(failures.snapshot()["library_media:Error"], 1);
  for (const path of ["/private", "/library/../private.wav", "/library/%2e%2e.wav", "/library/x.wav?token=x", "http://other/library/x.wav"]) {
    await assert.rejects(client.libraryBlob(path), /Invalid library/);
  }
  assert.equal(calls, 1);
});

test("asset_resolve failures remain visible with their UID", () => {
  const failures = new McpFailureCounter(() => {});
  assert.equal(assetResolveFailure({ ok: false }), "asset_resolve failed");
  assert.equal(assetResolveFailure({ ok: false, error: "missing uid" }), "missing uid");
  assert.equal(assetResolveFailure({ ok: true }), null);
  const message = noteAssetResolveFailure("ga:audio_clip:abc", "missing uid", failures);
  assert.match(message, /ga:audio_clip:abc.*missing uid/);
  assert.equal(failures.snapshot()["asset_resolve:ok_false"], 1);
});
