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

/**
 * Window->MCP failures, counted by `tool:status` (`http_415`, `rpc_-32602`)
 * or `tool:<error class>` (`TypeError` is what a CORS or network failure
 * throws). A failed call stays non-fatal but is never silent: the first
 * failure per key warns once, and every failure is counted.
 */
export class McpFailureCounter {
  private readonly counts = new Map<string, number>();
  private readonly warned = new Set<string>();

  constructor(
    private readonly warn: (message: string) => void = (message) => console.warn(message),
    private readonly onChange: (snapshot: Record<string, number>) => void = () => {},
  ) {}

  note(tool: string, kind: string): number {
    const key = `${tool}:${kind}`;
    const count = (this.counts.get(key) ?? 0) + 1;
    this.counts.set(key, count);
    if (!this.warned.has(key)) {
      this.warned.add(key);
      this.warn(`gen-audio: MCP ${tool} failed (${kind}); later failures of this kind are counted, not logged`);
    }
    this.onChange(this.snapshot());
    return count;
  }

  snapshot(): Record<string, number> {
    return Object.fromEntries(this.counts);
  }

  total(): number {
    let total = 0;
    for (const count of this.counts.values()) total += count;
    return total;
  }
}

/**
 * POST one tool call to the loopback MCP. Returns the JSON-RPC payload, or
 * null when the request fails (non-2xx, network/CORS error, bad JSON); each
 * failure, and each JSON-RPC error payload, is counted in `failures`.
 */
export async function postMcp(
  fetchImpl: typeof fetch,
  url: string,
  name: string,
  args: Record<string, unknown>,
  failures: McpFailureCounter,
): Promise<unknown | null> {
  try {
    const response = await fetchImpl(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      // The exact body is the desktop half of the UI/agent contract
      // (schemas/examples/control/*.json, tested from both sides).
      body: JSON.stringify(mcpRequestBody(name, args)),
    });
    if (!response.ok) {
      failures.note(name, `http_${response.status}`);
      return null;
    }
    const payload: unknown = await response.json();
    const code = (payload as { error?: { code?: unknown } } | null)?.error?.code;
    if (code !== undefined) failures.note(name, `rpc_${String(code)}`);
    return payload;
  } catch (error) {
    failures.note(name, error instanceof Error ? error.name : "error");
    return null;
  }
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
