// Optimus ruling on PR #5 (C1): TS does not rebind. Autoplay leaves focus
// (the cube's clock source) unchanged; a UI Play posts origin "user" and
// leaves the decision to the Rust reducer (PR #4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { controlPlayOrigin, mcpRequestBody, seekReportControl, userPlayControl } from "../src/play-control.ts";
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
  assert.match(main, /body: JSON\.stringify\(mcpRequestBody\(name, args\)\)/);
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
