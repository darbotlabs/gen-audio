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

export interface McpStatusReport {
  addr?: string;
  handshake_ok?: boolean;
}

/**
 * Address from `mcp_status`. A failed handshake keeps `fallback` (8765) but
 * is counted and named. A report with an address and a handshake that is not
 * explicitly false replaces the fallback and counts nothing.
 */
export function mcpOriginFromStatus(
  report: McpStatusReport | null | undefined,
  fallback: string,
  failures: McpFailureCounter,
): { origin: string; notice: string | null } {
  if (report?.addr && report.handshake_ok !== false) {
    return { origin: `http://${report.addr}`, notice: null };
  }
  const kind = report?.handshake_ok === false ? "handshake" : "missing";
  failures.note("mcp_status", kind);
  return {
    origin: fallback,
    notice: `MCP address lookup failed (${kind}); using ${fallback}`,
  };
}

/** The empty catch around the Tauri invoke. Count the error and name the fallback. */
export function noteMcpLookupError(error: unknown, fallback: string, failures: McpFailureCounter): string {
  const kind = error instanceof Error ? error.name : "error";
  failures.note("mcp_status", kind);
  return `MCP address lookup failed (${kind}); using ${fallback}`;
}

/**
 * GET one library media URL. A non-library path is not a failure. A non-2xx
 * response or a thrown fetch is counted and returns null.
 */
export async function fetchLibraryBlob(
  fetchImpl: typeof fetch,
  origin: string,
  urlPath: string,
  failures: McpFailureCounter,
): Promise<Blob | null> {
  if (!urlPath.startsWith("/library/")) return null;
  try {
    const response = await fetchImpl(`${origin}${urlPath}`);
    if (!response.ok) {
      failures.note("library_media", `http_${response.status}`);
      return null;
    }
    return await response.blob();
  } catch (error) {
    failures.note("library_media", error instanceof Error ? error.name : "error");
    return null;
  }
}

/**
 * Why a finished job cannot paint its tile. `null` when asset_resolve
 * returned ok. A missing body and `ok: false` are both failures.
 */
export function assetResolveFailure(resolved: unknown): string | null {
  if (!resolved || typeof resolved !== "object") return "asset_resolve failed";
  const record = resolved as { ok?: unknown; error?: unknown };
  if (record.ok === true) return null;
  return typeof record.error === "string" && record.error.length > 0 ? record.error : "asset_resolve failed";
}

/** Count an asset_resolve failure and the sentence the tile shows. */
export function noteAssetResolveFailure(uid: string, reason: string, failures: McpFailureCounter): string {
  failures.note("asset_resolve", "ok_false");
  return `asset_resolve failed for ${uid}: ${reason}`;
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
