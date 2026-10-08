// Play origin on the control bus (Optimus ruling on PR #5, C1). DOM-free so
// node:test can import it. No focus or rebind decision lives in TS: the Rust
// viewport reducer (PR #4) owns that.

/**
 * Who started playback: "user" (a Play click, or a control-bus play that is
 * not marked auto), "resume" (the floating bar), "auto" (autoplay and any
 * other programmatic start). In TS the origin is informational only.
 */
export type PlayOrigin = "user" | "resume" | "auto";

/** The control-bus request a UI Play click posts (MCP `ui_playback`, origin "user"). */
export function userPlayControl(tileId: string): {
  name: "ui_playback";
  args: { tileId: string; action: "play"; origin: "user" };
} {
  return { name: "ui_playback", args: { tileId, action: "play", origin: "user" } };
}

/** Origin of a control-bus `playback` event: "auto" only when it says so, else "user" (the bus default). */
export function controlPlayOrigin(args: Record<string, unknown>): PlayOrigin {
  return args.origin === "auto" ? "auto" : "user";
}

/** The JSON-RPC body the window POSTs to the loopback MCP for a tool call. */
export function mcpRequestBody(name: string, args: Record<string, unknown>): {
  jsonrpc: "2.0";
  id: 1;
  method: "tools/call";
  params: { name: string; arguments: Record<string, unknown> };
} {
  return { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } };
}

/** Where a control-bus seek landed, as the window measured it. */
export interface SeekLanding {
  ok: boolean;
  /** currentTime after the attempt, or null. */
  actual: number | null;
  status: string;
}

/**
 * The report the window posts after an MCP `ui_playback` seek (bus event
 * `seq`) finishes: MCP returns it to the agent as
 * {requested_t, landed_t, ok, reason}.
 */
export function seekReportControl(
  seq: number,
  requested: number,
  landing: SeekLanding,
): {
  name: "ui_seek_report";
  args: { seq: number; requested_t: number; landed_t: number | null; ok: boolean; reason: string };
} {
  const landedT = landing.actual !== null && Number.isFinite(landing.actual) ? Math.round(landing.actual * 1000) / 1000 : null;
  const reason = landing.status === "superseded" ? "superseded by a newer seek" : landing.status;
  return { name: "ui_seek_report", args: { seq, requested_t: requested, landed_t: landedT, ok: landing.ok, reason: reason.slice(0, 240) } };
}
