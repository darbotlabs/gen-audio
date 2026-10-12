export type CubeMode = "single" | "compare";
export const CUBE_MODES: readonly CubeMode[] = ["single", "compare"];
export const CUBE_CONTROL_OP = "cube";
export interface CubeCompareState {
  tileId: string;
  clip_uid: string;
  left_method: string;
  right_method: string;
  left_cube_uid: string;
  right_cube_uid: string;
}
export interface CubeModeState { cube_mode: CubeMode; cube_compare: CubeCompareState | null }
export const SINGLE: CubeModeState = Object.freeze({ cube_mode: "single", cube_compare: null });
export function cubeModeRequest(mode: CubeMode, tileId?: string | null) {
  return { name: "ui_cube" as const, args: tileId ? { mode, tileId } : { mode } };
}
export function cubeStateFrom(value: unknown): CubeModeState | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (record.cube_mode === "single") return record.cube_compare === null ? { ...SINGLE } : null;
  if (record.cube_mode !== "compare" || !record.cube_compare || typeof record.cube_compare !== "object") return null;
  const pair = record.cube_compare as Record<string, unknown>;
  const keys = ["tileId", "clip_uid", "left_method", "right_method", "left_cube_uid", "right_cube_uid"] as const;
  if (keys.some(key => typeof pair[key] !== "string" || !pair[key])) return null;
  if (pair.left_method !== "library_r3" || pair.right_method !== "pipeline_r2") return null;
  if (["clip_uid", "left_cube_uid", "right_cube_uid"].some(key => !/^ga:[a-z_0-9]+:[a-z2-7]{26}$/.test(String(pair[key])))) return null;
  return { cube_mode: "compare", cube_compare: Object.fromEntries(keys.map(key => [key, pair[key]])) as unknown as CubeCompareState };
}
export type ToolReply = { body: Record<string, unknown> } | { error: string } | null;
export type ApplyResult = { ok: true; state: CubeModeState } | { ok: false; reason: string; state: CubeModeState };
export interface CubeModeEffects {
  enter(pair: CubeCompareState): Promise<{ ok: true } | { ok: false; reason: string }>;
  exit(): void;
}
export const MCP_DOWN = "Compare requires the authenticated MCP ui_cube tool; the mode did not change.";

export function createCubeModeController(effects: CubeModeEffects, post: (name: string, args: Record<string, unknown>) => Promise<ToolReply>) {
  let current: CubeModeState = SINGLE;
  let rendered = true;
  let chain: Promise<unknown> = Promise.resolve();
  function serial<T>(run: () => Promise<T>): Promise<T> {
    const next = chain.then(run, run);
    chain = next.catch(() => undefined);
    return next;
  }
  async function apply(next: CubeModeState, force = false): Promise<ApplyResult> {
    if (!force && rendered && JSON.stringify(next) === JSON.stringify(current)) return { ok: true, state: current };
    current = next;
    if (!next.cube_compare) { effects.exit(); rendered = true; return { ok: true, state: current }; }
    let drawn: Awaited<ReturnType<CubeModeEffects["enter"]>>;
    try { drawn = await effects.enter(next.cube_compare); }
    catch (error) { drawn = { ok: false, reason: error instanceof Error ? error.message : "Comparison failed." }; }
    rendered = drawn.ok;
    if (drawn.ok) return { ok: true, state: current };
    effects.exit();
    // Do not fabricate a local single state if the backend refuses the repair.
    await request("single");
    return { ok: false, reason: drawn.reason, state: current };
  }
  async function request(mode: CubeMode, tileId?: string | null): Promise<ApplyResult> {
    const call = cubeModeRequest(mode, tileId);
    let reply: ToolReply;
    try { reply = await post(call.name, call.args); }
    catch (error) { reply = { error: error instanceof Error ? error.message : MCP_DOWN }; }
    if (!reply) return { ok: false, reason: MCP_DOWN, state: current };
    if ("error" in reply) return { ok: false, reason: reply.error, state: current };
    const next = cubeStateFrom(reply.body.args);
    return next ? apply(next) : { ok: false, reason: "ui_cube returned no cube state.", state: current };
  }
  return {
    state: () => current,
    request: (mode: CubeMode, tileId?: string | null) => serial(() => request(mode, tileId)),
    applyControl: (args: unknown) => serial(async (): Promise<ApplyResult> => {
      const next = cubeStateFrom(args);
      return next ? apply(next) : { ok: false, reason: "Malformed cube state.", state: current };
    }),
    rebound: (tileId: string) => serial(async (): Promise<ApplyResult | null> => {
      if (!current.cube_compare) return null;
      if (current.cube_compare.tileId === tileId) return apply(current, true);
      const result = await request("compare", tileId);
      if (!result.ok) { await request("single"); return { ...result, state: current }; }
      return result;
    }),
    sync: () => serial(async (): Promise<ApplyResult> => {
      const reply = await post("viewport_get", {});
      if (!reply || "error" in reply) return { ok: false, reason: reply?.error ?? MCP_DOWN, state: current };
      const next = cubeStateFrom(reply.body) ?? cubeStateFrom(reply.body.ui);
      return next ? apply(next) : { ok: false, reason: "viewport_get has no cube mode.", state: current };
    }),
  };
}
