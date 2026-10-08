// Optimus livetile ruling C1, kept DOM-free so node:test can import it.

/**
 * Who started playback. Only an explicit, user-initiated Play is a *focus*
 * action (Optimus ruling C1): a click on a tile's Play or the Cube tab's Play
 * ("user"), or MCP `ui_playback {action:"play"}` ("mcp"). Resuming the active
 * clip from the floating bar ("resume") and any non-user start ("auto":
 * autoplay, snap-scroll, programmatic) never move focus, so they never rebind
 * the cube or the shared clock.
 */
export type PlayOrigin = "user" | "mcp" | "resume" | "auto";
export interface PlayInfo {
  origin: PlayOrigin;
  /** True only for user/mcp starts: the clip takes focus; the cube and clock follow focus. */
  focus: boolean;
}
export function isFocusPlay(origin: PlayOrigin): boolean {
  return origin === "user" || origin === "mcp";
}
