// Optimus ruling on PR #5 (C1): TS does not rebind. Autoplay leaves focus
// (the cube's clock source) unchanged; a UI Play posts origin "user" and
// leaves the decision to the Rust reducer (PR #4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { controlPlayOrigin, userPlayControl } from "../src/play-control.ts";

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
