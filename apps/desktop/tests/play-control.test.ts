// Optimus ruling on PR #5 (C1): TS does not rebind. Autoplay leaves focus
// (the cube's clock source) unchanged; a UI Play posts origin "user" and
// leaves the decision to the Rust reducer (PR #4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { controlPlayOrigin, McpFailureCounter, mcpRequestBody, postMcp, seekReportControl, userPlayControl } from "../src/play-control.ts";
import { Seeker } from "../src/seek.ts";

const repo = (path: string) => fileURLToPath(new URL(`../../../${path}`, import.meta.url));
const load = (path: string) => JSON.parse(readFileSync(repo(path), "utf8"));

// playback.ts only touches the DOM through querySelector and rAF while playing.
const globals = globalThis as unknown as Record<string, unknown>;
globals.document = { querySelector: () => null, querySelectorAll: () => [] };
globals.window = { requestAnimationFrame: () => 1, cancelAnimationFrame: () => {} };

function fakeAudio(): HTMLAudioElement {
  const audio = {
    src: "/library/x.wav",
    paused: true,
    ended: false,
    currentTime: 0,
    duration: 10,
    async play() {
      audio.paused = false;
    },
    pause() {
      audio.paused = true;
    },
  };
  return audio as unknown as HTMLAudioElement;
}

test("autoplay leaves focus unchanged", async () => {
  const playback = await import("../src/playback.ts");
  playback.registerPlayer("lib-focused", fakeAudio());
  playback.registerPlayer("lib-other", fakeAudio());
  playback.setCubeClockClip("lib-focused", "ga:audio_clip:focused");
  const seen: Array<[string, unknown]> = [];
  const off = playback.onClipPlay((clipId, info) => seen.push([clipId, info]));
  assert.equal(await playback.playClip("lib-other", "auto"), "playing");
  assert.deepEqual(playback.getClockSource(), { clipId: "lib-focused", uid: "ga:audio_clip:focused" });
  assert.deepEqual(seen, [["lib-other", { origin: "auto" }]]);
  // Default origin is auto, and a user-origin start does not rebind in TS either.
  assert.equal(await playback.playClip("lib-other"), "playing");
  assert.equal(await playback.playClip("lib-other", "user"), "playing");
  assert.deepEqual(playback.getClockSource(), { clipId: "lib-focused", uid: "ga:audio_clip:focused" });
  assert.deepEqual(seen.map(([, info]) => info), [{ origin: "auto" }, { origin: "auto" }, { origin: "user" }]);
  off();
});

test("a UI Play posts ui_playback with origin user; the bus default is user", () => {
  assert.deepEqual(userPlayControl("lib-bitdot-braille-vibevoice"), {
    name: "ui_playback",
    args: { tileId: "lib-bitdot-braille-vibevoice", action: "play", origin: "user" },
  });
  assert.equal(controlPlayOrigin({ origin: "auto" }), "auto");
  assert.equal(controlPlayOrigin({ origin: "user" }), "user");
  assert.equal(controlPlayOrigin({}), "user");
});

// One contract, tested from both sides: these fixtures are exactly what the
// window posts, and crates/gen-audio-mcp/src/lib.rs asserts the server accepts them.
test("contract: the Play button's ui_playback body is schemas/examples/control/desktop-user-play.json", () => {
  const fixture = load("schemas/examples/control/desktop-user-play.json");
  const request = userPlayControl(fixture.params.arguments.tileId);
  assert.deepEqual(mcpRequestBody(request.name, request.args), fixture);
  // main.ts posts through these exact builders (Play click and Cube tab Play).
  const main = readFileSync(repo("apps/desktop/src/main.ts"), "utf8");
  // mcpCall goes through postMcp on the discovered origin (8765–8770), which posts
  // exactly mcpRequestBody (tested against a live listener below).
  assert.match(main, /postMcp\(fetch, `\$\{origin\}\/mcp`, name, args, mcpFailures\)/);
  assert.match(main, /const origin = await mcpReady/);
  const control = readFileSync(repo("apps/desktop/src/play-control.ts"), "utf8");
  assert.match(control, /body: JSON\.stringify\(mcpRequestBody\(name, args\)\)/);
  assert.equal(main.match(/const request = userPlayControl\(/g)?.length, 2);
});

test("contract: the window's seek report body is the seek-round-trip fixture", () => {
  const fixture = load("schemas/examples/control/seek-round-trip.json");
  const seq = fixture.desktopReport.params.arguments.seq;
  const requested = fixture.agentSeek.params.arguments.seconds;
  const report = seekReportControl(seq, requested, fixture.landing);
  assert.deepEqual(mcpRequestBody(report.name, report.args), fixture.desktopReport);
  assert.deepEqual(
    { requested_t: report.args.requested_t, landed_t: report.args.landed_t, ok: report.args.ok, reason: report.args.reason },
    fixture.agentResult,
  );
  const main = readFileSync(repo("apps/desktop/src/main.ts"), "utf8");
  assert.match(main, /const report = seekReportControl\(event\.seq, requested, landing\);\s*void mcpCall\(report\.name, report\.args\);/);
  // Superseded and missing-element landings are reported honestly.
  assert.deepEqual(seekReportControl(3, 10, { ok: false, actual: 4, status: "superseded" }).args, {
    seq: 3, requested_t: 10, landed_t: 4, ok: false, reason: "superseded by a newer seek",
  });
  assert.equal(seekReportControl(3, 10, { ok: false, actual: null, status: "no wav" }).args.landed_t, null);
});

test("D: a re-registered clip with a new source, or a removed tile, releases its seek blob", async () => {
  const playback = await import("../src/playback.ts");
  const released: string[] = [];
  class SpySeeker extends Seeker {
    override release(key: string): void {
      released.push(key);
      super.release(key);
    }
  }
  playback.setSeeker(new SpySeeker());
  const first = fakeAudio();
  playback.registerPlayer("lib-blob", first);
  playback.registerPlayer("lib-blob", fakeAudio());
  assert.deepEqual(released, [], "a re-render with the same source keeps the blob");
  const moved = fakeAudio();
  (moved as unknown as { src: string }).src = "/library/y.wav";
  playback.registerPlayer("lib-blob", moved);
  assert.deepEqual(released, ["lib-blob"], "a new source revokes the old blob");
  (moved as unknown as { isConnected: boolean }).isConnected = false;
  assert.ok(playback.releaseDetachedTransports().includes("lib-blob"));
  assert.deepEqual(released, ["lib-blob", "lib-blob"]);
});

// Observability: a window->MCP failure is counted and warned once, never swallowed.
async function listener(reply: (req: IncomingMessage, body: string, res: ServerResponse) => void): Promise<{ url: string; close: () => Promise<void> }> {
  const server = createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => reply(req, body, res));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return { url: `http://127.0.0.1:${port}/mcp`, close: () => new Promise((resolve) => server.close(() => resolve())) };
}

test("observability: a rejected POST increments the MCP failure counter and warns once per kind", async () => {
  const seen: Array<{ type: string | undefined; body: string }> = [];
  const server = await listener((req, body, res) => {
    seen.push({ type: req.headers["content-type"], body });
    res.writeHead(415, { "content-type": "application/json" }).end('{"error":"content-type must be application/json"}');
  });
  const warnings: string[] = [];
  const snapshots: Array<Record<string, number>> = [];
  const failures = new McpFailureCounter((message) => warnings.push(message), (snapshot) => snapshots.push(snapshot));
  const report = seekReportControl(2, 60, { ok: true, actual: 60, status: "seeked to 1:00" });
  try {
    assert.equal(await postMcp(fetch, server.url, report.name, report.args, failures), null);
    assert.equal(await postMcp(fetch, server.url, report.name, report.args, failures), null);
  } finally {
    await server.close();
  }
  assert.deepEqual(failures.snapshot(), { "ui_seek_report:http_415": 2 });
  assert.equal(warnings.length, 1, "warn once per tool:kind");
  assert.match(warnings[0], /ui_seek_report failed \(http_415\)/);
  assert.deepEqual(snapshots.at(-1), { "ui_seek_report:http_415": 2 });
  // What went on the wire is the contract body, typed application/json.
  assert.equal(seen[0].type, "application/json");
  assert.deepEqual(JSON.parse(seen[0].body), mcpRequestBody(report.name, report.args));
});

test("observability: network errors and JSON-RPC errors are counted too; success is not", async () => {
  const server = await listener((_req, body, res) => {
    const name = JSON.parse(body).params.name;
    const payload = name === "ui_navigate"
      ? { jsonrpc: "2.0", id: 1, error: { code: -32602, message: "unknown slide" } }
      : { jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "{}" }], isError: false } };
    res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(payload));
  });
  const failures = new McpFailureCounter(() => {});
  let closedUrl = "";
  try {
    const rpc = await postMcp(fetch, server.url, "ui_navigate", { slide: "slide:nope" }, failures);
    assert.equal((rpc as { error: { code: number } }).error.code, -32602, "the payload still reaches the caller");
    assert.notEqual(await postMcp(fetch, server.url, "library_rename", { clipId: "x" }, failures), null);
    closedUrl = server.url;
  } finally {
    await server.close();
  }
  assert.equal(await postMcp(fetch, closedUrl, "ui_generate", {}, failures), null);
  assert.deepEqual(failures.snapshot(), { "ui_navigate:rpc_-32602": 1, "ui_generate:TypeError": 1 });
  assert.equal(failures.total(), 2);
});

// CSP: the window may connect to itself, Tauri's IPC, and the loopback MCP it posts to. Nothing else, no wildcards.
test("csp: connect-src is exactly self, Tauri IPC and the MCP origin mcpCall posts to", () => {
  const csp: string = load("apps/desktop/src-tauri/tauri.conf.json").app.security.csp;
  const directives = new Map(csp.split(";").map((part) => part.trim().split(/\s+/)).map(([name, ...values]) => [name, values]));
  const ports = ["8765", "8766", "8767", "8768", "8769", "8770"];
  assert.deepEqual(directives.get("connect-src"), [
    "'self'",
    "ipc:",
    "http://ipc.localhost",
    ...ports.map((port) => `http://127.0.0.1:${port}`),
    ...ports.map((port) => `http://localhost:${port}`),
  ]);
  assert.ok(!csp.includes("*"), csp);
  const main = readFileSync(repo("apps/desktop/src/main.ts"), "utf8");
  const control = readFileSync(repo("apps/desktop/src/play-control.ts"), "utf8");
  assert.match(main, /postMcp\(fetch, `\$\{origin\}\/mcp`, name, args, mcpFailures\)/);
  assert.equal(main.match(/fetch\(/g)?.length ?? 0, 0, "main.ts does not open its own fetch");
  assert.equal(main.match(/fetchLibraryBlob\(fetch,/g)?.length, 1, "library media is the only other fetch");
  assert.equal(control.match(/fetchImpl\(/g)?.length, 2, "tool POST and library GET");
  // Only connect-src changed.
  assert.deepEqual(directives.get("default-src"), ["'self'"]);
  assert.deepEqual(directives.get("script-src"), ["'self'"]);
});
