import test from "node:test";
import assert from "node:assert/strict";
import { McpClient, controlNeedsResync, toolBody } from "../src/mcp-client.ts";
import { snapshotFaces } from "../src/viewport-source.ts";
import { seekReportControl, userPlayControl } from "../src/play-control.ts";

const config = { addr: "127.0.0.1:19847", token: "a1".repeat(32) };
const json = (value: unknown) => new Response(JSON.stringify(value), { headers: { "content-type": "application/json" } });
const event = (seq: number) => ({ seq, op: "focus", args: { uid: "lib-kokoro" } });

test("ring gaps, skipped frames and listener restarts require a snapshot, not partial replay", () => {
  assert.equal(controlNeedsResync({ cursor: 3, gap: false, events: [event(2), event(3)] }, 1), false);
  assert.equal(controlNeedsResync({ cursor: 1, gap: false, events: [] }, 1), false);
  assert.equal(controlNeedsResync({ cursor: 10, gap: true, events: [event(9), event(10)] }, 1), true);
  assert.equal(controlNeedsResync({ cursor: 3, gap: false, events: [event(3)] }, 1), true);
  assert.equal(controlNeedsResync({ cursor: 3, gap: false, events: [event(2)] }, 1), true);
  assert.equal(controlNeedsResync({ cursor: 0, gap: false, events: [] }, 100), true);
});

test("snapshot face ids preserve third and later faces instead of collapsing to back", () => {
  assert.deepEqual(snapshotFaces({
    faces: { "view:clip": { face_id: "spectrogram", face_index: 3, face_count: 5 } },
    flipped: { "view:clip": { face: "back" }, "view:legacy": { face: "front" } },
  }), [
    { tileId: "clip", faceId: "spectrogram", flipped: true },
    { tileId: "legacy", faceId: null, flipped: false },
  ]);
});

test("authenticated control polling and seek ACK use the same negotiated client", async () => {
  const calls: Array<{ name: string; arguments: Record<string, unknown> }> = [];
  const client = new McpClient(config, async (url, init) => {
    assert.equal(new Headers(init?.headers).get("authorization"), `Bearer ${config.token}`);
    assert.ok(!String(url).includes(config.token));
    if (String(url).includes("/control?")) return json({ cursor: 8, gap: true, oldest: 7, events: [event(7), event(8)] });
    const rpc = JSON.parse(String(init?.body));
    calls.push(rpc.params);
    return json({ jsonrpc: "2.0", id: rpc.id, result: { content: [{ text: JSON.stringify({ ok: true, cursor: 8, ui: { clock: { source: "lib-kokoro", t: 4 } } }) }] } });
  });
  const delta = await client.controls(1);
  assert.equal(controlNeedsResync(delta, 1), true);
  const snapshot = toolBody(await client.call("viewport_get", {}));
  assert.equal(snapshot.cursor, 8);
  const play = userPlayControl("lib-kokoro");
  await client.call(play.name, play.args);
  const ack = seekReportControl(9, 4, { ok: false, actual: 0, status: "seek failed" });
  await client.call(ack.name, ack.args);
  assert.deepEqual(calls.map(call => call.name), ["viewport_get", "ui_playback", "ui_seek_report"]);
  assert.equal(calls[1].arguments.origin, "user");
  assert.deepEqual(calls[2].arguments, { seq: 9, requested_t: 4, landed_t: 0, ok: false, reason: "seek failed" });
});
