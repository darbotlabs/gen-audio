// Asset object model v1 (docs/ASSET_OBJECT_MODEL.md), TypeScript mirror of
// crates/gen-audio-core/src/asset.rs. The UI only *reads* uids from the
// generated library/assets.json; minting lives here for cross-language parity
// tests against schemas/asset-object/vectors/v1.json.

export const SCHEMA_MAJOR = 1;
export const UID_SCHEME = "ga1";
export const DOMAIN_TAG = "ga-asset-v1";
export const KINDS = [
  "voice_model",
  "voice_profile",
  "audio_clip",
  "spectrogram_2d",
  "cube_ihdr",
  "layer",
  "podcast_script",
  "transcript",
  "card",
  "mcp_tool",
] as const;
export type AssetKind = (typeof KINDS)[number];

const BASE32 = "abcdefghijklmnopqrstuvwxyz234567";
const MAX_SAFE = Number.MAX_SAFE_INTEGER;
/** Max parents in `src` (schema maxItems). */
export const MAX_SRC = 16;
/** Max targets per relation list (composes, bound_to, supersedes). */
export const MAX_FAN_OUT = 8;
/** `honesty.note` cap in code points (schema `maxLength: 400`, Rust `MAX_HONESTY_NOTE_CHARS`). */
export const MAX_HONESTY_NOTE_CHARS = 400;
export const ROOT_KEYS = [
  "schema_version", "uid_scheme", "kind", "uid", "legacy_id", "status", "fields", "media", "src", "relations", "honesty",
  "provenance", "display", "body", "extensions",
] as const;
const DISPLAY_KEYS = ["title", "summary", "semantic_name", "face_name", "glyph", "display_rev"];

export class AssetError extends Error {
  constructor(
    readonly code: string,
    detail: string,
  ) {
    super(`${code}: ${detail}`);
  }
}

export function isKind(kind: string): kind is AssetKind {
  return (KINDS as readonly string[]).includes(kind);
}

// ------------------------------------------------------------ canonical JSON

function checkIdentityString(text: string, path: string): void {
  for (let index = 0; index < text.length; index += 1) {
    const unit = text.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = text.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        index += 1;
        continue;
      }
      throw new AssetError("lone_surrogate", `${path} holds a lone surrogate`);
    }
    if (unit >= 0xdc00 && unit <= 0xdfff) throw new AssetError("lone_surrogate", `${path} holds a lone surrogate`);
  }
  for (const ch of text) {
    const cp = ch.codePointAt(0) ?? 0;
    if (cp < 0x20 || cp === 0x7f) throw new AssetError("control_character", `${path} holds U+${cp.toString(16).toUpperCase().padStart(4, "0")}`);
  }
  if (text.normalize("NFC") !== text) throw new AssetError("non_nfc_string", `${path} is not NFC; normalize at ingest`);
}

function writeString(text: string): string {
  return `"${text.replace(/[\\"]/g, (ch) => `\\${ch}`)}"`;
}

function canonicalInteger(value: number, path: string): string {
  if (!Number.isFinite(value)) throw new AssetError("non_finite_number", `${path} is NaN or infinite`);
  if (Object.is(value, -0)) throw new AssetError("negative_zero", `${path} is -0`);
  if (!Number.isInteger(value)) throw new AssetError("float_in_identity", `${path} is not an integer; keep float views outside identity`);
  if (Math.abs(value) > MAX_SAFE) throw new AssetError("integer_out_of_range", `${path} is outside +/-(2^53-1)`);
  return String(value);
}

/**
 * JSON.parse for identity text. JS numbers cannot tell `139375.0` from
 * `139375`, so integral floats are rejected on the raw tokens before parsing
 * (Rust sees them as f64 and rejects them in canonicalize). Same codes as Rust:
 * a fraction or exponent is `float_in_identity`; `-0` / `-0.0` is `negative_zero`.
 */
export function parseIdentityJson(text: string): unknown {
  let inString = false;
  for (let index = 0; index < text.length; index += 1) {
    const ch = text[index];
    if (inString) {
      if (ch === "\\") index += 1;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
      continue;
    }
    if (ch === "-" || (ch >= "0" && ch <= "9")) {
      const match = /^-?\d+(\.\d+)?([eE][+-]?\d+)?/.exec(text.slice(index));
      if (!match) continue;
      const token = match[0];
      if (match[1] || match[2]) {
        if (Number(token) === 0 && token.startsWith("-")) throw new AssetError("negative_zero", `${token} is -0`);
        throw new AssetError("float_in_identity", `${token} is not an integer token; keep float views outside identity`);
      }
      index += token.length - 1;
    }
  }
  return JSON.parse(text);
}

/**
 * Paths (dot-joined, array indexes as numbers) of every number token in
 * `text` that has a fraction or exponent, such as `fields.duration_ms` for
 * `"duration_ms": 139375.0`. JSON.parse loses that distinction, so the
 * envelope checks below take these paths alongside the parsed object.
 */
export function floatTokenPaths(text: string): string[] {
  const out: string[] = [];
  let index = 0;
  const skipWs = () => {
    while (index < text.length && " \t\r\n".includes(text[index])) index += 1;
  };
  const readString = (): string => {
    const start = index;
    index += 1;
    while (index < text.length && text[index] !== '"') index += text[index] === "\\" ? 2 : 1;
    index += 1;
    return JSON.parse(text.slice(start, index)) as string;
  };
  const value = (path: string): void => {
    skipWs();
    const ch = text[index];
    if (ch === "{") {
      index += 1;
      skipWs();
      if (text[index] === "}") {
        index += 1;
        return;
      }
      for (;;) {
        skipWs();
        const key = readString();
        skipWs();
        index += 1; // ':'
        value(path ? `${path}.${key}` : key);
        skipWs();
        if (text[index++] === "}") return;
      }
    }
    if (ch === "[") {
      index += 1;
      skipWs();
      if (text[index] === "]") {
        index += 1;
        return;
      }
      for (let item = 0; ; item += 1) {
        value(path ? `${path}.${item}` : String(item));
        skipWs();
        if (text[index++] === "]") return;
      }
    }
    if (ch === '"') {
      readString();
      return;
    }
    const match = /^-?\d+(\.\d+)?([eE][+-]?\d+)?|^(true|false|null)/.exec(text.slice(index, index + 64));
    if (!match) throw new AssetError("bad_json", `unexpected token at offset ${index}`);
    if (match[1] || match[2]) out.push(path);
    index += match[0].length;
  };
  value("");
  return out;
}

// ------------------------------------------------------------ rounding (normative)

/**
 * Inferred cube bin_frames (B1'): roundHalfUp(fl(fl(duration_s * sample_rate_hz) / time_bins)),
 * exactly two binary64 ops, multiply first, from the cube JSON's float duration_s.
 * 157.134 s, 24 kHz, 96 bins -> 39283 (exact rational math would give 39284).
 */
export function binFramesInferred(durationS: number, sampleRateHz: number, timeBins: number): number {
  const product = durationS * sampleRateHz;
  return roundHalfUp(product / Math.max(1, timeBins));
}

/** Whole ms from frames: round half up in exact integer arithmetic, (frames*1000 + rate div 2) div rate. */
export function msFromFrames(frames: number, rate: number): number {
  if (rate <= 0) return 0;
  const result = (BigInt(frames) * 1000n + BigInt(rate) / 2n) / BigInt(rate);
  return Number(result);
}

/** Round a non-negative double half up (== ties away from zero for x >= 0); matches Rust f64::round. */
export function roundHalfUp(value: number): number {
  if (!Number.isFinite(value) || value < 0) throw new AssetError("bad_rounding_input", `${value} must be finite and >= 0`);
  const floor = Math.floor(value);
  const rounded = value - floor >= 0.5 ? floor + 1 : floor;
  if (rounded > MAX_SAFE) throw new AssetError("integer_out_of_range", `${value} rounds outside 2^53-1`);
  return rounded;
}

/** RFC 8785 JCS restricted to the v1 identity subset (integers, ASCII keys, NFC strings). */
export function canonicalize(value: unknown, path = "$"): string {
  if (value === null || value === undefined) throw new AssetError("null_in_identity", `${path} is null; omit the key instead`);
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") return canonicalInteger(value, path);
  if (typeof value === "string") {
    checkIdentityString(value, path);
    return writeString(value);
  }
  if (Array.isArray(value)) return `[${value.map((item, index) => canonicalize(item, `${path}[${index}]`)).join(",")}]`;
  if (typeof value === "object") {
    const record = value as Record<string, unknown>;
    const keys = Object.keys(record);
    for (const key of keys) {
      if (!key || !/^[\x20-\x7e]+$/.test(key)) throw new AssetError("non_ascii_key", `${path} key ${JSON.stringify(key)} must be printable ASCII`);
    }
    // ASCII keys: code-unit order equals the UTF-16 order JCS specifies.
    keys.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
    return `{${keys.map((key) => `${writeString(key)}:${canonicalize(record[key], `${path}.${key}`)}`).join(",")}}`;
  }
  throw new AssetError("bad_identity_value", `${path} has an unsupported type`);
}

/** NFC-normalize every string and key; ingest-time only, hashing never normalizes. */
export function normalizeNfc<T>(value: T): T {
  if (typeof value === "string") return value.normalize("NFC") as T;
  if (Array.isArray(value)) return value.map((item) => normalizeNfc(item)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>).map(([key, item]) => [key.normalize("NFC"), normalizeNfc(item)])) as T;
  }
  return value;
}

// ------------------------------------------------------------ sha256 (sync)

const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
  0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
  0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
  0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
  0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
  0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

export function sha256(data: Uint8Array): Uint8Array {
  const h = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
  const length = data.length;
  const padded = new Uint8Array(((length + 9 + 63) >> 6) << 6);
  padded.set(data);
  padded[length] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(padded.length - 8, Math.floor((length * 8) / 2 ** 32));
  view.setUint32(padded.length - 4, (length * 8) >>> 0);
  const w = new Uint32Array(64);
  const rotr = (x: number, n: number) => (x >>> n) | (x << (32 - n));
  for (let offset = 0; offset < padded.length; offset += 64) {
    for (let t = 0; t < 16; t += 1) w[t] = view.getUint32(offset + t * 4);
    for (let t = 16; t < 64; t += 1) {
      const s0 = rotr(w[t - 15], 7) ^ rotr(w[t - 15], 18) ^ (w[t - 15] >>> 3);
      const s1 = rotr(w[t - 2], 17) ^ rotr(w[t - 2], 19) ^ (w[t - 2] >>> 10);
      w[t] = (w[t - 16] + s0 + w[t - 7] + s1) >>> 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let t = 0; t < 64; t += 1) {
      const t1 = (hh + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[t] + w[t]) >>> 0;
      const t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) >>> 0;
      hh = g;
      g = f;
      f = e;
      e = (d + t1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) >>> 0;
    }
    h[0] += a;
    h[1] += b;
    h[2] += c;
    h[3] += d;
    h[4] += e;
    h[5] += f;
    h[6] += g;
    h[7] += hh;
  }
  const out = new Uint8Array(32);
  const outView = new DataView(out.buffer);
  h.forEach((word, index) => outView.setUint32(index * 4, word));
  return out;
}

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

// ------------------------------------------------------------ uid + glyph

export function base32Lower(bytes: Uint8Array): string {
  let out = "";
  let buffer = 0;
  let bits = 0;
  for (const byte of bytes) {
    buffer = ((buffer << 8) | byte) & 0xffff;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out += BASE32[(buffer >> bits) & 31];
    }
  }
  if (bits > 0) out += BASE32[(buffer << (5 - bits)) & 31];
  return out;
}

export interface ParsedUid {
  kind: AssetKind;
  body: string;
}

export function parseUid(uid: string): ParsedUid {
  if (!uid.startsWith("ga:")) throw new AssetError("bad_uid", `${JSON.stringify(uid)} must start with ga:`);
  const rest = uid.slice(3);
  const colon = rest.indexOf(":");
  if (colon < 0) throw new AssetError("bad_uid", `${JSON.stringify(uid)} must be ga:<kind>:<26 chars>`);
  const kind = rest.slice(0, colon);
  const body = rest.slice(colon + 1);
  if (!/^[a-z0-9_]+$/.test(kind)) throw new AssetError("bad_uid", `${JSON.stringify(uid)} kind token must be snake_case`);
  if (body.length !== 26) throw new AssetError("bad_uid_length", `${JSON.stringify(uid)} body must be 26 chars, got ${body.length}`);
  if (![...body].every((ch) => BASE32.includes(ch))) throw new AssetError("bad_uid_alphabet", `${JSON.stringify(uid)} body must be lowercase RFC 4648 base32`);
  if (BASE32.indexOf(body[25]) & 0b11) throw new AssetError("bad_uid_padding", `${JSON.stringify(uid)} last char must have its 2 pad bits zero`);
  if (!isKind(kind)) throw new AssetError("unknown_kind", `${JSON.stringify(uid)} kind ${kind} is not in the v1 enum`);
  return { kind, body };
}

/** Same check as parseUid, without throwing. */
export function isUid(text: unknown): text is string {
  if (typeof text !== "string") return false;
  try {
    parseUid(text);
    return true;
  } catch {
    return false;
  }
}

export function glyphFromBytes(first: number, second: number): string {
  return String.fromCodePoint(0x2800 + first, 0x2800 + second);
}

/** Digest bytes 0 and 1, recovered from the first 16 bits of the uid text. */
export function glyphBytesFromUid(uid: string): [number, number] {
  const { body } = parseUid(uid);
  let acc = 0;
  for (const ch of body.slice(0, 4)) acc = (acc << 5) | BASE32.indexOf(ch);
  const top = acc >> 4;
  return [(top >> 8) & 0xff, top & 0xff];
}

export function glyphFromUid(uid: string): string {
  const [first, second] = glyphBytesFromUid(uid);
  return glyphFromBytes(first, second);
}

/** Lit braille dots (1..8) for one byte: bit k lights dot k+1. */
export function glyphDots(byte: number): number[] {
  const dots: number[] = [];
  for (let bit = 0; bit < 8; bit += 1) if (byte & (1 << bit)) dots.push(bit + 1);
  return dots;
}

export function hueClass(kind: string): string {
  return isKind(kind) ? `ga-kind-${kind}` : "ga-kind-unknown";
}

// ------------------------------------------------------------ minting

export interface MediaDigest {
  role: string;
  sha256: string;
}

export interface Minted {
  canonical: string;
  preimageHex: string;
  digestHex: string;
  uid: string;
  glyph: string;
}

export function identityValue(kind: string, fields: unknown, media: MediaDigest[], src: string[]): Record<string, unknown> {
  if (!isKind(kind)) throw new AssetError("unknown_kind", `kind ${JSON.stringify(kind)} is not in the v1 enum`);
  if (!fields || typeof fields !== "object" || Array.isArray(fields)) throw new AssetError("bad_fields", "fields must be an object");
  for (const item of media) {
    if (!/^[0-9a-f]{64}$/.test(item.sha256)) throw new AssetError("bad_sha256", `media ${item.role} sha256 must be 64 lowercase hex`);
    if (!/^[a-z0-9_]+$/.test(item.role)) throw new AssetError("bad_media_role", `media role ${JSON.stringify(item.role)} must be snake_case`);
  }
  const roles = new Set<string>();
  for (const item of media) {
    if (roles.has(item.role)) throw new AssetError("duplicate_media_role", `${kind} carries media role ${JSON.stringify(item.role)} twice`);
    roles.add(item.role);
  }
  const sortedMedia = [...media]
    .map(({ role, sha256: digest }) => ({ role, sha256: digest }))
    .sort((a, b) => (a.role < b.role ? -1 : a.role > b.role ? 1 : a.sha256 < b.sha256 ? -1 : a.sha256 > b.sha256 ? 1 : 0));
  if (src.length > MAX_SRC) throw new AssetError("fan_out_exceeded", `src has ${src.length} parents (max ${MAX_SRC})`);
  src.forEach((uid) => parseUid(uid));
  if (new Set(src).size !== src.length) throw new AssetError("duplicate_src", "src lists a parent twice");
  const parents = [...src].sort();
  return { kind, schema_major: SCHEMA_MAJOR, fields, media: sortedMedia, src: parents };
}

export function mint(kind: string, fields: unknown, media: MediaDigest[] = [], src: string[] = []): Minted {
  const identity = identityValue(kind, fields, media, src);
  const canonical = canonicalize(identity);
  const encoder = new TextEncoder();
  const tag = encoder.encode(DOMAIN_TAG);
  const body = encoder.encode(canonical);
  const preimage = new Uint8Array(tag.length + 1 + body.length);
  preimage.set(tag, 0);
  preimage[tag.length] = 0;
  preimage.set(body, tag.length + 1);
  const digest = sha256(preimage);
  const uid = `ga:${kind}:${base32Lower(digest.slice(0, 16))}`;
  return { canonical, preimageHex: toHex(preimage), digestHex: toHex(digest), uid, glyph: glyphFromBytes(digest[0], digest[1]) };
}

/** Recompute the uid an envelope should carry from its hashed parts. */
export function envelopeIdentityUid(envelope: { kind: string; fields: unknown; media?: MediaDigest[]; src?: string[] }): string {
  return mint(envelope.kind, envelope.fields, envelope.media ?? [], envelope.src ?? []).uid;
}

// ------------------------------------------------------------ media paths

/** Media paths are relative to the library root: `name.ext` or `dir/name.ext`. */
export function checkMediaPath(path: string): void {
  const fail = (code: string, detail: string): never => {
    throw new AssetError(code, `${JSON.stringify(path)} ${detail}`);
  };
  if (!path) fail("media_path_empty", "is empty");
  if (path.startsWith("\\\\") || path.startsWith("//")) fail("media_path_unc", "is a UNC path");
  if (path.startsWith("/") || path.startsWith("\\")) fail("media_path_absolute", "is absolute");
  if (/^[A-Za-z]:/.test(path)) fail("media_path_drive", "has a drive letter");
  if (path.includes(":")) fail("media_path_scheme", "has a URL scheme");
  if (path.includes("\\")) fail("media_path_backslash", "uses backslashes");
  const segments = path.split("/");
  for (const segment of segments) {
    if (segment === ".." || segment === "." || segment.includes("..")) fail("media_path_dotdot", "walks out of the library root");
    if (!/^[A-Za-z0-9_-][A-Za-z0-9_.-]*$/.test(segment) || segment.length > 128) fail("media_path_chars", `segment ${JSON.stringify(segment)} is not [A-Za-z0-9_-][A-Za-z0-9_.-]*`);
  }
  if (segments.length > 4) fail("media_path_chars", "is nested too deep");
}

/** Web URL for a library-relative media path. */
export function libraryUrl(path: string): string {
  checkMediaPath(path);
  return `/library/${path}`;
}

// ------------------------------------------------------------ envelope shape

const REQUIRED_MEDIA: Record<string, string[]> = {
  audio_clip: ["wav"],
  spectrogram_2d: ["spectrogram_png"],
  cube_ihdr: ["cube_json"],
  podcast_script: ["script_txt"],
};

/** A cube_ihdr is "real" when it claims library_cube and is not a fixture (B2 ruling: then cube_png is required too). */
export function cubeIsReal(envelope: Record<string, unknown>): boolean {
  const honesty = (envelope.honesty ?? {}) as { fixture?: unknown; claims?: unknown };
  return Array.isArray(honesty.claims) && honesty.claims.includes("library_cube") && honesty.fixture !== true;
}

/**
 * The structural subset of Rust `validate_envelope` (D1/B2/D3): root keys,
 * legacy_id, kind/uid prefix, channels, uid recompute, display (title, glyph, display_rev),
 * wav_url, relation fan-out, required media roles (a real cube needs cube_png)
 * and the honesty.note cap. Per-kind field typing stays Rust-side; the schema
 * covers it for Ajv.
 *
 * `floatPaths` are the envelope-relative paths from `floatTokenPaths` on the
 * raw JSON text. With them, a `139375.0` token in `fields` is
 * `float_in_identity` and a `3.0` display_rev is `bad_display_rev`, as in Rust.
 */
export function checkEnvelopeShape(envelope: Record<string, unknown>, floatPaths: readonly string[] = []): void {
  const unknownKey = Object.keys(envelope).find((key) => !(ROOT_KEYS as readonly string[]).includes(key));
  if (unknownKey) throw new AssetError("unknown_root_key", `envelope key ${JSON.stringify(unknownKey)} is not in the v1 schema`);
  const legacy = envelope.legacy_id;
  if (legacy !== undefined && !(typeof legacy === "string" && /^[A-Za-z0-9_.-]{1,96}$/.test(legacy) && !legacy.includes(".."))) {
    throw new AssetError("bad_legacy_id", `legacy_id ${JSON.stringify(legacy)} must match [A-Za-z0-9_.-]{1,96} without '..'`);
  }
  const kind = String(envelope.kind);
  if (!isKind(kind)) throw new AssetError("unknown_kind", `kind ${JSON.stringify(kind)} is not in the v1 enum`);
  const parsed = parseUid(String(envelope.uid));
  if (parsed.kind !== kind) throw new AssetError("kind_mismatch", `uid prefix ${parsed.kind} != kind ${kind}`);
  const fields = (envelope.fields ?? {}) as Record<string, unknown>;
  const floatField = floatPaths.find((path) => path.startsWith("fields."));
  if (floatField) throw new AssetError("float_in_identity", `${kind}.${floatField} is not an integer token; keep float views outside identity`);
  if (kind === "audio_clip" && fields.channels !== undefined) {
    const channels = fields.channels;
    if (!(typeof channels === "number" && Number.isInteger(channels) && channels >= 1 && channels <= 32)) {
      throw new AssetError("bad_field_type", "audio_clip.fields.channels must be an integer in 1..32");
    }
  }
  const media = (Array.isArray(envelope.media) ? envelope.media : []) as MediaDigest[];
  const minted = mint(kind, fields, media, (envelope.src as string[] | undefined) ?? []);
  if (minted.uid !== envelope.uid) throw new AssetError("uid_mismatch", `uid ${String(envelope.uid)} != recomputed ${minted.uid}`);
  const display = envelope.display as Record<string, unknown> | undefined;
  if (!display || typeof display !== "object") throw new AssetError("missing_glyph", "display with title and glyph is required");
  const badKey = Object.keys(display).find((key) => !DISPLAY_KEYS.includes(key));
  if (badKey) throw new AssetError("unknown_display_key", `display.${badKey} is not in the v1 schema`);
  const title = display.title;
  if (!(typeof title === "string" && title.length > 0 && [...title].length <= 160)) throw new AssetError("bad_title", "display.title must be a 1..160 char string");
  if (typeof display.glyph !== "string") throw new AssetError("missing_glyph", "display.glyph is required");
  if (display.glyph !== minted.glyph) throw new AssetError("glyph_mismatch", `glyph ${display.glyph} != ${minted.glyph}`);
  const rev = display.display_rev;
  if (rev !== undefined && (!(typeof rev === "number" && Number.isInteger(rev) && rev >= 0 && rev <= MAX_SAFE) || floatPaths.includes("display.display_rev"))) {
    throw new AssetError("bad_display_rev", "display.display_rev must be a non-negative integer");
  }
  const body = envelope.body as Record<string, unknown> | undefined;
  if (kind === "audio_clip" && body && body.wav_url !== undefined) {
    const url = body.wav_url;
    if (typeof url !== "string" || !url.startsWith("/library/")) throw new AssetError("bad_wav_url", `wav_url ${JSON.stringify(url)} must be /library/<media path>`);
    try {
      checkMediaPath(url.slice("/library/".length));
    } catch (error) {
      throw new AssetError("bad_wav_url", error instanceof Error ? error.message : String(error));
    }
  }
  const relations = (envelope.relations ?? {}) as Record<string, unknown>;
  for (const field of ["composes", "bound_to", "supersedes"]) {
    const targets = relations[field];
    if (Array.isArray(targets) && targets.length > MAX_FAN_OUT) {
      throw new AssetError("fan_out_exceeded", `relations.${field} has ${targets.length} targets (max ${MAX_FAN_OUT})`);
    }
  }
  for (const role of REQUIRED_MEDIA[kind] ?? []) {
    if (!media.some((item) => item.role === role)) {
      throw new AssetError(kind === "audio_clip" ? "clip_missing_wav" : "missing_media_role", `${kind} needs a ${role} media entry`);
    }
  }
  if (kind === "transcript" && media.length === 0) throw new AssetError("missing_media_role", "transcript needs transcript_json or transcript_txt");
  if (kind === "cube_ihdr" && cubeIsReal(envelope) && !media.some((item) => item.role === "cube_png")) {
    throw new AssetError("missing_media_role", "a real cube_ihdr (claims library_cube) needs a cube_png media entry");
  }
  const note = ((envelope.honesty ?? {}) as { note?: unknown }).note;
  if (note !== undefined && !(typeof note === "string" && [...note].length <= MAX_HONESTY_NOTE_CHARS)) {
    throw new AssetError("bad_honesty", `honesty.note must be a string of at most ${MAX_HONESTY_NOTE_CHARS} chars`);
  }
}
