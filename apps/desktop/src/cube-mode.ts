// Cube tab mode (single | compare), DOM-free so node:test can import it.
//
// Rule 5: UI state lives in app state and is readable and drivable over MCP.
// The state's owner is the MCP server (crates/gen-audio-mcp control.rs,
// `ui_cube` / `viewport_get`). The window never flips Compare on its own: the
// Compare button posts `ui_cube` exactly like an agent does, and the window
// applies the state the server returns (tool result or `cube` bus event)
// through ONE setter, `apply` below. Contract: schemas/examples/ui_cube.contract.json.

export type CubeMode = "single" | "compare";
export const CUBE_MODES: readonly CubeMode[] = ["single", "compare"];
/** Control-bus op that carries the Cube tab state (Rust `set_cube_mode`). */
export const CUBE_CONTROL_OP = "cube";

/** viewport_get's cube_compare: which clip, and which formulas on each side. */
export interface CubeCompareState {
  tileId: string;
  clip_uid: string;
  left_method: string;
  right_method: string;
  left_cube_uid: string;
  right_cube_uid: string;
}

/** Same fields as viewport_get's cube_mode / cube_compare. */
export interface CubeModeState {
  cube_mode: CubeMode;
  cube_compare: CubeCompareState | null;
}

export const SINGLE: CubeModeState = Object.freeze({ cube_mode: "single", cube_compare: null }) as CubeModeState;

/** The tools/call a Compare button click posts (the agent posts the same). */
export function cubeModeRequest(mode: CubeMode, tileId?: string | null): { name: "ui_cube"; args: { mode: CubeMode; tileId?: string } } {
  return { name: "ui_cube", args: tileId ? { mode, tileId } : { mode } };
}

const UID = /^ga:[a-z_0-9]+:[a-z2-7]{26}$/;

/** Parse a server state (ui_cube result `args`, or a `cube` bus event's args). Null when malformed. */
export function cubeStateFrom(args: unknown): CubeModeState | null {
  if (!args || typeof args !== "object") return null;
  const record = args as Record<string, unknown>;
  if (record.cube_mode === "single") return record.cube_compare === null ? { cube_mode: "single", cube_compare: null } : null;
  if (record.cube_mode !== "compare") return null;
  const pair = record.cube_compare as Record<string, unknown> | null;
  if (!pair || typeof pair !== "object") return null;
  const text = (key: string) => (typeof pair[key] === "string" && (pair[key] as string).length > 0 ? (pair[key] as string) : null);
  const tileId = text("tileId");
  const clip = text("clip_uid");
  const left = text("left_method");
  const right = text("right_method");
  const leftCube = text("left_cube_uid");
  const rightCube = text("right_cube_uid");
  if (!tileId || !clip || !left || !right || !leftCube || !rightCube) return null;
  if (![clip, leftCube, rightCube].every((uid) => UID.test(uid)) || left === right) return null;
  return {
    cube_mode: "compare",
    cube_compare: { tileId, clip_uid: clip, left_method: left, right_method: right, left_cube_uid: leftCube, right_cube_uid: rightCube },
  };
}

/** A tool result body, an RPC error message, or null when MCP did not answer. */
export type ToolReply = { body: Record<string, unknown> } | { error: string } | null;

export type ApplyResult = { ok: true; state: CubeModeState } | { ok: false; reason: string; state: CubeModeState };

export interface CubeModeEffects {
  /** Draw Compare for this clip (bind its Library cube if another is bound, load the comparison cube). */
  enter(pair: CubeCompareState): Promise<{ ok: true } | { ok: false; reason: string }>;
  /** Back to the single cube. */
  exit(): void;
}

export interface CubeModeController {
  state(): CubeModeState;
  /** `cube` op from the control bus (an agent's ui_cube, or our own echo). */
  applyControl(args: unknown): Promise<ApplyResult>;
  /** The Compare button (and rebinds): post ui_cube, then apply what the server returns. */
  request(mode: CubeMode, tileId?: string | null): Promise<ApplyResult>;
  /** The bound cube changed to `tileId` while Compare may be on: follow it, or leave Compare and say why. */
  rebound(tileId: string): Promise<ApplyResult | null>;
  /** Resync from viewport_get (window start, or the bus ring dropped events). */
  sync(): Promise<ApplyResult>;
}

function sameState(a: CubeModeState, b: CubeModeState): boolean {
  if (a.cube_mode !== b.cube_mode) return false;
  if (!a.cube_compare || !b.cube_compare) return a.cube_compare === b.cube_compare;
  const left = a.cube_compare;
  const right = b.cube_compare;
  return (Object.keys(left) as Array<keyof CubeCompareState>).every((key) => left[key] === right[key]);
}

export const MCP_DOWN = "Compare is driven through MCP ui_cube and the MCP listener did not answer, so the Cube tab mode did not change.";

export function createCubeModeController(
  effects: CubeModeEffects,
  post: (name: string, args: Record<string, unknown>) => Promise<ToolReply>,
): CubeModeController {
  let current: CubeModeState = SINGLE;
  // One change at a time, in arrival order: a replayed compare that is still
  // loading must not land after the single that followed it.
  let chain: Promise<unknown> = Promise.resolve();
  function serial<T>(run: () => Promise<T>): Promise<T> {
    const next = chain.then(run, run);
    chain = next.catch(() => undefined);
    return next;
  }

  /**
   * THE setter: every mode change in the window goes through here. `fromServer`
   * marks a state the server sent (tool result, bus event, viewport_get); one
   * equal to what is already drawn is not drawn twice (our own call's bus
   * echo, a replay). Dedup is by state, not by seq, because seqs restart with
   * the MCP process. A local redraw (`rebound`, same clip) passes false.
   */
  async function apply(next: CubeModeState, fromServer: boolean): Promise<ApplyResult> {
    if (fromServer && sameState(next, current)) return { ok: true, state: current };
    current = next;
    if (!next.cube_compare) {
      effects.exit();
      return { ok: true, state: current };
    }
    const drawn = await effects.enter(next.cube_compare);
    if (drawn.ok) return { ok: true, state: current };
    // The server said compare but the window could not draw it: tell the
    // server, so viewport_get never reports a Compare nobody can see.
    const back = await request("single");
    if (!back.ok) {
      current = SINGLE;
      effects.exit();
    }
    return { ok: false, reason: drawn.reason, state: current };
  }

  async function request(mode: CubeMode, tileId?: string | null): Promise<ApplyResult> {
    const call = cubeModeRequest(mode, tileId);
    const reply = await post(call.name, call.args);
    if (!reply) return { ok: false, reason: MCP_DOWN, state: current };
    if ("error" in reply) return { ok: false, reason: reply.error, state: current };
    const next = cubeStateFrom(reply.body.args);
    if (!next) return { ok: false, reason: "ui_cube answered without a cube state; nothing changed.", state: current };
    return apply(next, true);
  }

  return {
    state: () => current,
    applyControl(args) {
      return serial(async () => {
        const next = cubeStateFrom(args);
        if (!next) return { ok: false, reason: "malformed cube event ignored", state: current };
        return apply(next, true);
      });
    },
    request(mode, tileId) {
      return serial(() => request(mode, tileId));
    },
    rebound(tileId) {
      return serial(async () => {
        const pair = current.cube_compare;
        if (!pair) return null;
        // Same clip, cube reloaded (loading a cube drops the second pane): draw it again.
        if (pair.tileId === tileId) return apply(current, false);
        const moved = await request("compare", tileId);
        if (moved.ok) return moved;
        const left = await request("single");
        return { ok: false, reason: moved.reason, state: left.state };
      });
    },
    sync() {
      return serial(async () => {
        const reply = await post("viewport_get", {});
        if (!reply) return { ok: false, reason: MCP_DOWN, state: current };
        if ("error" in reply) return { ok: false, reason: reply.error, state: current };
        const next = cubeStateFrom(reply.body);
        if (!next) return { ok: false, reason: "viewport_get has no cube_mode / cube_compare", state: current };
        return apply(next, true);
      });
    },
  };
}
