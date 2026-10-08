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
