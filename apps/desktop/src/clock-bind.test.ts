import assert from "node:assert/strict";
import { clipDrivesCube } from "./clock-bind.ts";
import { backKindLabel, hiddenFace } from "./face-a11y.ts";

assert.equal(clipDrivesCube("lib-a", null), false);
assert.equal(clipDrivesCube("lib-a", "lib-b"), false);
assert.equal(clipDrivesCube("lib-a", "lib-a"), true);
assert.equal(clipDrivesCube(null, null), false);

const away = hiddenFace(false);
assert.equal(away.front, false);
assert.equal(away.back, true);
const flipped = hiddenFace(true);
assert.equal(flipped.front, true);
assert.equal(flipped.back, false);
assert.notEqual(backKindLabel("LibraryClip"), "Adaptive card");
assert.equal(backKindLabel("LibraryClip"), "LibraryClip");
