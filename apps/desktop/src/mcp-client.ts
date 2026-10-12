import { checkMediaPath } from "./asset";

export interface McpConfig {
  addr: string;
  token: string;
}

export interface NativeMcpStatus {
  addr: string;
  token?: string | null;
  handshake_ok: boolean;
  detail: string;
}

export interface ControlEvent {
  seq?: number;
  op?: string;
  args?: Record<string, unknown>;
}

export interface ControlDelta {
  cursor: number;
  gap: boolean;
  events: ControlEvent[];
}

export function controlNeedsResync(delta: ControlDelta, after: number): boolean {
  if (delta.gap || delta.cursor < after) return true;
  let expected = after + 1;
  for (const event of delta.events) {
    if (event.seq! <= after) continue;
    if (event.seq !== expected++) return true;
  }
  return delta.cursor !== expected - 1;
}

export interface AudioArtifact {
  wav: string;
  manifest: string;
  sample_rate: number;
  samples: number;
  duration_s: number;
  sha256: string;
}

export interface SpeechResult extends AudioArtifact {
  ok: boolean;
  synthesizedSpeech: true;
  complete?: boolean;
  limit_reached?: boolean;
  artifact_verified?: boolean;
  assetUid?: string;
}

export interface MasteredAudioResult extends AudioArtifact {
  ok: true;
  processedAudio: true;
  synthesizedSpeech: false;
  sourceSha256: string;
  report: Record<string, unknown>;
}

const JSON_LIMIT = 1024 * 1024;
const WAV_LIMIT = 128 * 1024 * 1024;
const ARTIFACT_NAME = /^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,254}$/;

function record(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

export function validateMcpConfig(config: McpConfig): McpConfig {
  if (!config || typeof config.addr !== "string"
    || !/^(?:127\.0\.0\.1|localhost|\[::1\]):[1-9]\d{0,4}$/.test(config.addr)
    || Number(config.addr.slice(config.addr.lastIndexOf(":") + 1)) > 65535
    || typeof config.token !== "string" || !/^[a-fA-F0-9]{64}$/.test(config.token)) {
    throw new Error("MCP configuration requires an explicit loopback host:port and a 64-hex bearer token.");
  }
  return { addr: config.addr, token: config.token };
}

export async function resolveMcpConfig(
  nativeStatus?: () => Promise<NativeMcpStatus>,
  explicitBrowserConfig?: McpConfig,
): Promise<McpConfig> {
  if (nativeStatus) {
    const status = await nativeStatus();
    if (!status.handshake_ok || !status.token) throw new Error("The owned desktop MCP listener is unavailable.");
    return validateMcpConfig({ addr: status.addr, token: status.token });
  }
  if (explicitBrowserConfig) return validateMcpConfig(explicitBrowserConfig);
  throw new Error("Native bridge unavailable. Browser preview needs explicit in-memory MCP address/token configuration; no endpoint was contacted.");
}

export class McpHttpError extends Error {
  constructor(readonly status: number) {
    super(`MCP HTTP ${status}${status === 401 || status === 403 ? ": authorization refused" : ""}`);
  }
}

async function boundedBytes(response: Response, cap: number): Promise<Uint8Array> {
  const declared = response.headers.get("content-length");
  if (declared && (!/^\d+$/.test(declared) || Number(declared) > cap)) {
    await response.body?.cancel();
    throw new Error("MCP response exceeds the download limit.");
  }
  if (!response.body) throw new Error("MCP response body is missing.");
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > cap) {
        await reader.cancel();
        throw new Error("MCP response exceeds the download limit.");
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  if (declared && Number(declared) !== size) throw new Error("MCP response was truncated.");
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return bytes;
}

export function toolBody(payload: unknown, allowIncomplete = false): Record<string, unknown> {
  const envelope = record(payload);
  const error = record(envelope?.error);
  if (error) throw new Error(typeof error.message === "string" ? error.message.slice(0, 600) : "MCP request failed.");
  const result = record(envelope?.result);
  const content = Array.isArray(result?.content) ? result.content : [];
  const text = record(content[0])?.text;
  if (typeof text !== "string") throw new Error("MCP tool returned no JSON result.");
  let body: Record<string, unknown> | null;
  try { body = record(JSON.parse(text)); }
  catch { throw new Error("MCP tool returned invalid JSON."); }
  if (!body) throw new Error("MCP tool returned an invalid result.");
  const partial = allowIncomplete && body.ok === false && body.synthesizedSpeech === true && body.artifact_verified === true && body.complete === false && body.limit_reached === true;
  if ((result?.isError === true || body.ok === false) && !partial) {
    const errors = Array.isArray(body.errors) ? body.errors.map(item => record(item)?.message).filter(item => typeof item === "string").join("; ") : "";
    throw new Error(typeof body.error === "string" ? body.error.slice(0, 1000) : errors ? errors.slice(0, 2000) : typeof body.reason === "string" ? body.reason.slice(0, 1000) : "MCP tool failed; no speech was produced.");
  }
  return body;
}

export function speechResult(payload: unknown, allowIncomplete = false): SpeechResult {
  const body = toolBody(payload, allowIncomplete);
  const validName = (value: unknown, suffix: string): value is string => typeof value === "string"
    && ARTIFACT_NAME.test(value) && value.endsWith(suffix);
  if ((body.ok !== true && !(allowIncomplete && body.complete === false && body.limit_reached === true && body.artifact_verified === true)) || body.synthesizedSpeech !== true || !validName(body.wav, ".wav")
    || !validName(body.manifest, ".json") || !Number.isSafeInteger(body.sample_rate)
    || Number(body.sample_rate) < 8000 || Number(body.sample_rate) > 192000
    || !Number.isSafeInteger(body.samples) || Number(body.samples) <= 0
    || typeof body.duration_s !== "number" || !Number.isFinite(body.duration_s) || body.duration_s <= 0
    || Math.abs(body.duration_s - Number(body.samples) / Number(body.sample_rate)) > 1e-6
    || typeof body.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(body.sha256)) {
    throw new Error("No verified speech WAV/manifest was returned.");
  }
  return {
    ok: body.ok === true,
    synthesizedSpeech: true,
    wav: body.wav,
    manifest: body.manifest,
    sample_rate: Number(body.sample_rate),
    samples: Number(body.samples),
    duration_s: body.duration_s,
    sha256: body.sha256,
    complete: body.complete !== false,
    limit_reached: body.limit_reached === true,
    artifact_verified: body.artifact_verified === true,
    ...(typeof body.assetUid === "string" ? { assetUid: body.assetUid } : {}),
  };
}

function validateAudioArtifact(value: AudioArtifact): void {
  if (!ARTIFACT_NAME.test(value.wav) || !value.wav.endsWith(".wav")
    || !ARTIFACT_NAME.test(value.manifest) || !value.manifest.endsWith(".json")
    || !Number.isSafeInteger(value.sample_rate) || value.sample_rate < 8000 || value.sample_rate > 192000
    || !Number.isSafeInteger(value.samples) || value.samples <= 0
    || !Number.isFinite(value.duration_s) || value.duration_s <= 0
    || Math.abs(value.duration_s - value.samples / value.sample_rate) > 1e-6
    || !/^[a-f0-9]{64}$/.test(value.sha256)) {
    throw new Error("Invalid audio artifact metadata.");
  }
}

export function masteredAudioResult(payload: unknown): MasteredAudioResult {
  const body = toolBody(payload);
  const report = record(body.report);
  if (body.ok !== true || body.processedAudio !== true || body.synthesizedSpeech !== false
    || typeof body.wav !== "string" || typeof body.manifest !== "string"
    || typeof body.sample_rate !== "number" || typeof body.samples !== "number"
    || typeof body.duration_s !== "number" || typeof body.sha256 !== "string"
    || typeof body.sourceSha256 !== "string" || !/^[a-f0-9]{64}$/.test(body.sourceSha256) || !report) {
    throw new Error("No verified mastering result was returned.");
  }
  const result: MasteredAudioResult = {
    ok: true, processedAudio: true, synthesizedSpeech: false,
    wav: body.wav, manifest: body.manifest, sample_rate: body.sample_rate,
    samples: body.samples, duration_s: body.duration_s, sha256: body.sha256,
    sourceSha256: body.sourceSha256, report,
  };
  validateAudioArtifact(result);
  return result;
}

export class McpClient {
  private readonly config: McpConfig;
  private nextId = 1;

  constructor(config: McpConfig, private readonly fetcher: typeof fetch = (input, init) => globalThis.fetch(input, init)) {
    this.config = validateMcpConfig(config);
  }

  private async request(path: string, init: RequestInit, cap: number, timeoutMs: number, expectedMime?: string): Promise<Uint8Array> {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), timeoutMs);
    try {
      // CSP host sources cannot express IPv6 literals. localhost resolves
      // the negotiated IPv6 loopback without permitting remote origins.
      const host = this.config.addr.replace(/^\[::1\]:/, "localhost:");
      const response = await this.fetcher(`http://${host}${path}`, {
        ...init, signal: controller.signal, credentials: "omit", cache: "no-store", redirect: "error",
        headers: { ...init.headers, Authorization: `Bearer ${this.config.token}` },
      }).catch(() => { throw new Error("MCP connection unavailable."); });
      if (!response.ok) {
        await response.body?.cancel();
        throw new McpHttpError(response.status);
      }
      const mime = response.headers.get("content-type")?.split(";")[0].trim().toLowerCase();
      if (mime !== (expectedMime ?? (path.endsWith(".wav") ? "audio/wav" : "application/json"))) {
        await response.body?.cancel();
        throw new Error("MCP returned an unexpected content type.");
      }
      return await boundedBytes(response, cap);
    } catch (error) {
      if (error instanceof McpHttpError) throw error;
      if (controller.signal.aborted) throw new Error("MCP request timed out; speech completion is unconfirmed.");
      // Never include request headers, URLs with credentials, or fetch implementation diagnostics.
      throw new Error(error instanceof Error ? error.message.replaceAll(this.config.token, "[redacted]") : "MCP request failed.");
    } finally {
      clearTimeout(timeout);
    }
  }

  async call(name: string, args: Record<string, unknown>): Promise<unknown> {
    const id = this.nextId++;
    const bytes = await this.request("/mcp", {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id, method: "tools/call", params: { name, arguments: args } }),
    }, JSON_LIMIT, ["synth", "model_generate"].includes(name) ? 630_000 : ["master_audio", "draft_generate", "engine_load"].includes(name) ? 300_000 : ["studio_preflight", "model_preflight"].includes(name) ? 150_000 : 30_000);
    let payload: unknown;
    try { payload = JSON.parse(new TextDecoder().decode(bytes)); }
    catch { throw new Error("MCP returned invalid JSON."); }
    if (record(payload)?.id !== id) throw new Error("MCP response ID did not match the request.");
    return payload;
  }

  async controls(after: number): Promise<ControlDelta> {
    if (!Number.isSafeInteger(after) || after < 0) throw new Error("Invalid control cursor.");
    const bytes = await this.request(`/control?after=${after}`, { method: "GET" }, JSON_LIMIT, 10_000);
    let payload: Record<string, unknown> | null;
    try { payload = record(JSON.parse(new TextDecoder().decode(bytes))); }
    catch { throw new Error("MCP returned invalid control JSON."); }
    if (!Number.isSafeInteger(payload?.cursor) || Number(payload?.cursor) < 0
      || !Array.isArray(payload?.events) || payload.events.length > 512
      || (payload.gap !== undefined && typeof payload.gap !== "boolean")
      || payload.events.some((event) => !record(event) || !Number.isSafeInteger(event.seq) || event.seq < 1
        || typeof event.op !== "string" || !record(event.args))) {
      throw new Error("Invalid control response.");
    }
    return { cursor: Number(payload.cursor), gap: payload.gap === true, events: payload.events as ControlEvent[] };
  }

  async libraryBlob(path: string): Promise<Blob> {
    if (!path.startsWith("/library/")) {
      throw new Error("Invalid library media path.");
    }
    try { checkMediaPath(path.slice("/library/".length)); }
    catch { throw new Error("Invalid library media path."); }
    const mime = path.endsWith(".wav") ? "audio/wav" : path.endsWith(".json") ? "application/json"
      : path.endsWith(".png") ? "image/png" : path.endsWith(".mp4") ? "video/mp4" : null;
    if (!mime) throw new Error("Unsupported library media type.");
    const bytes = await this.request(path, { method: "GET" }, mime === "application/json" ? 16 * JSON_LIMIT : WAV_LIMIT, 30_000, mime);
    return new Blob([bytes], { type: mime });
  }

  async speechBlob(result: SpeechResult): Promise<Blob> {
    // Validate even when the caller didn't use speechResult().
    speechResult({ result: { content: [{ text: JSON.stringify(result) }] } }, result.complete === false);
    return this.audioBlob(result);
  }

  async audioBlob(result: AudioArtifact): Promise<Blob> {
    validateAudioArtifact(result);
    return this.verifiedWav(result.wav, result.sha256);
  }

  async referenceBlob(result: { wav: string; sha256: string }): Promise<Blob> {
    if (!ARTIFACT_NAME.test(result.wav) || !result.wav.endsWith(".wav") || !/^[a-f0-9]{64}$/.test(result.sha256)) {
      throw new Error("Invalid reference audio receipt.");
    }
    return this.verifiedWav(result.wav, result.sha256);
  }

  private async verifiedWav(wav: string, expectedDigest: string): Promise<Blob> {
    const bytes = await this.request(`/artifacts/${wav}`, { method: "GET" }, WAV_LIMIT, 30_000);
    if (bytes.byteLength < 44 || new TextDecoder().decode(bytes.subarray(0, 4)) !== "RIFF"
      || new TextDecoder().decode(bytes.subarray(8, 12)) !== "WAVE") {
      throw new Error("The speech artifact is not a WAV file.");
    }
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
    const hex = Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
    if (hex !== expectedDigest) throw new Error("Speech download failed SHA-256 verification.");
    return new Blob([bytes], { type: "audio/wav" });
  }

  async manifestBlob(result: AudioArtifact): Promise<Blob> {
    validateAudioArtifact(result);
    const bytes = await this.request(`/artifacts/${result.manifest}`, { method: "GET" }, JSON_LIMIT, 30_000);
    JSON.parse(new TextDecoder().decode(bytes));
    return new Blob([bytes], { type: "application/json" });
  }
}

export class BoundedSequences {
  private readonly values = new Set<number>();
  constructor(private readonly capacity = 512) {
    if (!Number.isSafeInteger(capacity) || capacity < 1) throw new Error("Invalid sequence capacity.");
  }
  has(seq: number): boolean { return this.values.has(seq); }
  add(seq: number): void {
    if (!Number.isSafeInteger(seq) || seq < 0) return;
    this.values.add(seq);
    while (this.values.size > this.capacity) this.values.delete(this.values.values().next().value!);
  }
  get size(): number { return this.values.size; }
  clear(): void { this.values.clear(); }
}

export function controlBackoff(failures: number): number {
  return Math.min(30_000, 1000 * 2 ** Math.min(Math.max(0, failures), 5));
}
