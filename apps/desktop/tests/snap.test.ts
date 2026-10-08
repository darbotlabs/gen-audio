import { test } from "node:test";
import assert from "node:assert/strict";
import { GESTURE_QUIET_MS, WheelGesture, easeInOutCubic, slideKey, stepIndex } from "../src/snap.ts";

function run(gesture: WheelGesture, events: Array<[number, number, number?, boolean?]>): number[] {
  return events.map(([t, dy, mode = 0, locked = false]) => gesture.feed(dy, mode, t, locked)).filter((step) => step !== 0);
}

test("a mouse wheel notch moves exactly one slide", () => {
  assert.deepEqual(run(new WheelGesture(), [[0, 100]]), [1]);
  assert.deepEqual(run(new WheelGesture(), [[0, -3, 1]]), [-1]); // line mode, 3 lines up
});

test("a trackpad swipe with a long inertia tail moves exactly one slide", () => {
  const events: Array<[number, number]> = [];
  let delta = 6;
  for (let t = 0; t < 1400; t += 16) {
    events.push([t, delta]);
    delta = t < 200 ? delta + 4 : Math.max(1, delta * 0.94);
  }
  assert.deepEqual(run(new WheelGesture(), events), [1]);
});

test("input during the animation is swallowed, a fresh gesture after a pause moves again", () => {
  const gesture = new WheelGesture();
  assert.deepEqual(run(gesture, [[0, 120]]), [1]);
  assert.deepEqual(run(gesture, [[50, 120, 0, true], [120, 120, 0, true], [300, 120, 0, true]]), []);
  assert.deepEqual(run(gesture, [[300 + GESTURE_QUIET_MS + 1, 120]]), [1]);
});

test("small jitter below the threshold does nothing; direction reversal resets", () => {
  assert.deepEqual(run(new WheelGesture(), [[0, 10], [16, 10], [32, -10], [48, 10]]), []);
  assert.deepEqual(run(new WheelGesture(), [[0, 30], [16, -45]]), [-1]);
});

test("keys map to one slide and clamp at the ends", () => {
  assert.equal(slideKey("PageDown"), "next");
  assert.equal(slideKey("ArrowDown"), "next");
  assert.equal(slideKey("PageUp"), "prev");
  assert.equal(slideKey("ArrowUp"), "prev");
  assert.equal(slideKey("ArrowLeft"), null);
  assert.equal(stepIndex(2, "next", 8), 3);
  assert.equal(stepIndex(0, "prev", 8), 0);
  assert.equal(stepIndex(7, 1, 8), 7);
  assert.equal(stepIndex(4, "home", 8), 0);
  assert.equal(stepIndex(4, "end", 8), 7);
  assert.equal(stepIndex(0, "next", 0), 0);
});

test("easing is monotone from 0 to 1", () => {
  let previous = -1;
  for (let i = 0; i <= 20; i += 1) {
    const value = easeInOutCubic(i / 20);
    assert.ok(value >= previous);
    previous = value;
  }
  assert.equal(easeInOutCubic(0), 0);
  assert.equal(easeInOutCubic(1), 1);
});
