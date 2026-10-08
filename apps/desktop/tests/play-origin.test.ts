// Optimus ruling C1: only an explicit user (or MCP ui_playback) Play is a focus
// action that rebinds the cube and the shared clock.
import { test } from "node:test";
import assert from "node:assert/strict";
import { isFocusPlay } from "../src/play-origin.ts";

test("only user and MCP play starts are focus actions", () => {
  assert.equal(isFocusPlay("user"), true);
  assert.equal(isFocusPlay("mcp"), true);
  assert.equal(isFocusPlay("resume"), false);
  assert.equal(isFocusPlay("auto"), false);
});
