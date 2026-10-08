// The debug-only run_fixture_improve command must not be granted by the
// capability a release build embeds. dev.json holds that grant, and
// tauri.conf.json lists only "default" so an empty list cannot pull dev in.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const repo = (path: string) => fileURLToPath(new URL(`../../../${path}`, import.meta.url));
const load = (path: string) => JSON.parse(readFileSync(repo(path), "utf8"));

test("the release capability does not grant allow-run-fixture-improve", () => {
  const release = load("apps/desktop/src-tauri/capabilities/default.json");
  const dev = load("apps/desktop/src-tauri/capabilities/dev.json");
  const conf = load("apps/desktop/src-tauri/tauri.conf.json");
  assert.equal(JSON.stringify(release).includes("allow-run-fixture-improve"), false);
  assert.deepEqual(release.permissions, ["allow-connector-statuses", "allow-mcp-status"]);
  assert.deepEqual(dev.permissions, ["allow-run-fixture-improve"]);
  assert.equal(dev.identifier, "dev");
  assert.deepEqual(conf.app.security.capabilities, ["default"]);
});
