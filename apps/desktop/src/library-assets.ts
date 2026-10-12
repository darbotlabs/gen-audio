// Read-only ingestion: malformed catalogs fail as a whole, never as partial UI data.
import { catalogValidator as validate } from "./generated/contract-validators.js";
import { checkEnvelopeShape, checkMediaPath, envelopeIdentityUid, floatTokenPaths, glyphFromUid, isUid, type AssetKind } from "./asset";

export interface AssetMedia {
  role: string;
  path: string;
  sha256: string;
  bytes: number;
  mime?: string;
}

export interface AssetEnvelope {
  schema_version: string;
  uid_scheme: "ga1";
  uid: string;
  kind: AssetKind;
  legacy_id?: string;
  status: "ok" | "missing" | "unavailable";
  fields: Record<string, unknown>;
  media: AssetMedia[];
  src: string[];
  honesty: { synthesized_speech: boolean; fixture: boolean; not_podcast?: boolean; claims?: string[]; note?: string };
  relations?: Record<string, unknown>;
  provenance?: Record<string, unknown> & { params?: Record<string, unknown> };
  body?: Record<string, unknown>;
  display: { title: string; glyph: string; display_rev?: number };
}

/** One real cube a voice model made (through its clip), for the model card's Cube tab. */
export interface ModelCubeRow {
  imported?: boolean;
  clipId: string;
  clipTitle: string;
  cubeUid: string;
  revision: number | null;
  shape: [number, number] | null;
  invHdr: number | null;
  layerScore: number | null;
  layerMethod: string | null;
  jsonUrl: string;
}

/**
 * What a voice model card's Cube tab shows (E4):
 * - "cubes": the model's clips' cubes; `offline` when the engine is unavailable
 *   in this app but a clip was rendered elsewhere (VibeVoice -> bitdot).
 * - "g2p": a G2P-only model (misaki) has no cube of its own; it links the clips
 *   it phonemized.
 * - "unavailable": no clip, engine unavailable, with availability.reason.
 * - "none": an available model with no clip yet. "unknown": no such voice_model.
 */
export type ModelCubes =
  | { state: "cubes"; offline: boolean; rows: ModelCubeRow[] }
  | { state: "g2p"; clips: Array<{ clipId: string; clipTitle: string }> }
  | { state: "unavailable"; reason: string }
  | { state: "none" }
  | { state: "unknown" };

const num = (value: unknown): number | null => (typeof value === "number" && Number.isFinite(value) ? value : null);

export function modelCubes(assets: readonly AssetEnvelope[], modelId: string): ModelCubes {
  const model = assets.find((asset) => asset.kind === "voice_model" && asset.legacy_id === modelId);
  if (!model) return { state: "unknown" };
  const clipsBy = (key: "voice_model" | "g2p_model") =>
    assets.filter((asset) => asset.kind === "audio_clip" && asset.provenance?.[key] === model.uid);
  if (model.honesty?.claims?.includes("g2p_only")) {
    return { state: "g2p", clips: clipsBy("g2p_model").map((clip) => ({ clipId: clip.legacy_id ?? "", clipTitle: clip.display.title })) };
  }
  const rows: ModelCubeRow[] = [];
  for (const clip of clipsBy("voice_model")) {
    const cube = assets.find((asset) => asset.kind === "cube_ihdr" && asset.src.length === 1 && asset.src[0] === clip.uid && !comparisonMethod(asset));
    if (!cube) continue;
    const shape = cube.body?.cube_shape_f_t;
    rows.push({
      imported: clip.provenance?.params?.imported === true,
      clipId: clip.legacy_id ?? "",
      clipTitle: clip.display.title,
      cubeUid: cube.uid,
      revision: num(cube.fields.cube_revision),
      shape: Array.isArray(shape) && shape.length === 2 && shape.every((n) => typeof n === "number") ? [shape[0], shape[1]] : null,
      invHdr: num(cube.body?.inv_hdr),
      layerScore: num(cube.body?.layer_score),
      layerMethod: typeof cube.provenance?.layer_method === "string" ? cube.provenance.layer_method : null,
      jsonUrl: mediaUrl(cube, "cube_json") ?? "",
    });
  }
  const unavailable = model.status === "unavailable";
  if (rows.length) return { state: "cubes", offline: unavailable, rows };
  if (unavailable) {
    const availability = model.body?.availability as { reason?: unknown } | undefined;
    return { state: "unavailable", reason: typeof availability?.reason === "string" ? availability.reason : "pending" };
  }
  return { state: "none" };
}

export interface CatalogDocument {
  assets: AssetEnvelope[];
  legacy_index: Record<string, string>;
}

export interface LibraryCatalog {
  byUid: Map<string, AssetEnvelope>;
  assets: readonly AssetEnvelope[];
  modelCubes(modelId: string): ModelCubes;
  clipForTile(tileId: string): AssetEnvelope | null;
  derivedFrom(clipUid: string, kind: "spectrogram_2d" | "cube_ihdr"): AssetEnvelope | null;
  cubeForUrl(url: string): AssetEnvelope | null;
  compareCubesFor(clipUid: string): AssetEnvelope[];
}

export function comparisonMethod(asset: AssetEnvelope): string | null {
  const method = asset.fields.layer_method ?? asset.provenance?.layer_method ?? asset.provenance?.params?.layer_method;
  return asset.kind === "cube_ihdr" && method === "pipeline_r2" ? method : null;
}

export function sourceSha256(asset: AssetEnvelope | null | undefined): string | null {
  const sha = asset?.fields.source_sha256;
  return typeof sha === "string" && /^[0-9a-f]{64}$/.test(sha) ? sha : null;
}

let pending: Promise<LibraryCatalog | null> | null = null;

export function mediaUrl(asset: AssetEnvelope, role: string): string | null {
  const item = asset.media.find((entry) => entry.role === role);
  if (!item) return null;
  checkMediaPath(item.path);
  return `/library/${item.path}`;
}

/** Tests start from an empty cache. A failed load already clears `pending` itself. */
export function resetLibraryCatalogForTest(): void {
  pending = null;
}

/** Throws with the offending field/uid; the caller can display a load error. */
export function parseLibraryCatalog(document: unknown, floatPaths: readonly string[] = []): LibraryCatalog {
  if (!validate(document)) {
    const errors = validate.errors?.map(error => `catalog${error.instancePath} ${error.message}`).join(", ");
    throw new Error(`Invalid library catalog: ${errors || "invalid document"}`);
  }
  const byUid = new Map<string, AssetEnvelope>();
  const legacy = new Map<string, string>();
  for (const [index, asset] of document.assets.entries()) {
    const prefix = `assets.${index}.`;
    checkEnvelopeShape(asset as unknown as Record<string, unknown>, floatPaths.filter((path) => path.startsWith(prefix)).map((path) => path.slice(prefix.length)));
    if (!isUid(asset.uid) || envelopeIdentityUid(asset) !== asset.uid) {
      throw new Error(`Invalid library asset identity: ${asset.uid}`);
    }
    if (glyphFromUid(asset.uid) !== asset.display.glyph) throw new Error(`Invalid library asset glyph: ${asset.uid}`);
    asset.media.forEach((item) => checkMediaPath(item.path));
    if (byUid.has(asset.uid)) throw new Error(`Duplicate library asset uid: ${asset.uid}`);
    byUid.set(asset.uid, asset);
    if (asset.legacy_id !== undefined) {
      const key = `${asset.kind}:${asset.legacy_id}`;
      if (legacy.has(key)) throw new Error(`Duplicate library legacy id: ${key}`);
      legacy.set(key, asset.uid);
    }
  }
  for (const [key, uid] of Object.entries(document.legacy_index)) {
    if (legacy.get(key) !== uid) throw new Error(`Invalid library legacy index: ${key}`);
  }
  if (legacy.size !== Object.keys(document.legacy_index).length) throw new Error("Incomplete library legacy index");
  for (const asset of document.assets) {
    for (const uid of asset.src) {
      if (!byUid.has(uid)) throw new Error(`Missing library source ${uid} for ${asset.uid}`);
    }
    for (const targets of Object.values(asset.relations ?? {})) {
      for (const uid of typeof targets === "string" ? [targets] : targets as string[]) {
        if (!byUid.has(uid)) throw new Error(`Missing library relation ${uid} for ${asset.uid}`);
      }
    }
    const voice = asset.kind === "voice_profile" ? asset.fields.voice_model : asset.provenance?.voice_model;
    for (const uid of [voice, asset.provenance?.g2p_model]) {
      if (typeof uid === "string" && byUid.get(uid)?.kind !== "voice_model") throw new Error(`Missing library voice model ${uid}`);
    }
    if (asset.kind === "cube_ihdr" || asset.kind === "spectrogram_2d") {
      const clip = byUid.get(asset.src[0])!;
      const wav = clip.media.find((item) => item.role === "wav");
      if (clip.kind !== "audio_clip" || !wav || wav.sha256 !== sourceSha256(asset)) {
        throw new Error(`Invalid library source digest for ${asset.uid}`);
      }
      const fields = asset.fields;
      const frames = Number(fields[asset.kind === "cube_ihdr" ? "bin_frames" : "hop_frames"]);
      const tolerance = Math.ceil(frames * 1000 / Math.max(1, Number(fields.sample_rate_hz)));
      if (Math.abs(Number(fields.duration_ms) - Number(clip.fields.duration_ms)) > tolerance
        || (asset.kind === "cube_ihdr" && Number(fields.covers_ms) > Number(fields.duration_ms))) {
        throw new Error(`Invalid library derived duration for ${asset.uid}`);
      }
    }
  }
  const visiting = new Set<string>();
  const visited = new Set<string>();
  const visit = (uid: string, depth: number, relations: boolean): void => {
    if (visiting.has(uid) || depth > 16) throw new Error(`Invalid library source graph: ${uid}`);
    if (visited.has(uid)) return;
    visiting.add(uid);
    const asset = byUid.get(uid)!;
    const targets = relations
      ? ["composes", "bound_to", "supersedes"].flatMap((key) => (asset.relations?.[key] as string[] | undefined) ?? [])
      : asset.src;
    for (const parent of targets) visit(parent, depth + 1, relations);
    visiting.delete(uid);
    visited.add(uid);
  };
  for (const relations of [false, true]) {
    visited.clear();
    for (const uid of byUid.keys()) visit(uid, 0, relations);
  }
  const assets = [...byUid.values()];
  return {
    byUid,
    assets,
    modelCubes: (modelId) => modelCubes(assets, modelId),
    clipForTile: (tileId) => assets.find((asset) => asset.kind === "audio_clip" && asset.legacy_id === tileId) ?? null,
    derivedFrom: (clipUid, kind) => assets.find((asset) => asset.kind === kind && asset.src.length === 1 && asset.src[0] === clipUid && !comparisonMethod(asset)) ?? null,
    cubeForUrl: (url) => assets.find((asset) => asset.kind === "cube_ihdr" && mediaUrl(asset, "cube_json") === url) ?? null,
    compareCubesFor: (clipUid) => assets.filter((asset) => comparisonMethod(asset) !== null && asset.src.length === 1 && asset.src[0] === clipUid),
  };
}

/** 404 means no optional catalog; transport, JSON and schema failures reject. */
export function loadLibraryCatalog(extraAssets: Promise<unknown[]> = Promise.resolve([])): Promise<LibraryCatalog | null> {
  pending ??= Promise.all([fetch("/library/assets.json"), extraAssets])
    .then(async ([response, extra]) => {
      if (response.status === 404) return null;
      if (!response.ok) throw new Error(`Library catalog request failed (${response.status})`);
      const text = await response.text();
      const doc: unknown = JSON.parse(text);
      const catalog = parseLibraryCatalog(doc, floatTokenPaths(text));
      if (!extra.length) return catalog;
      const assets = [...catalog.assets, ...extra] as AssetEnvelope[];
      const legacy_index = Object.fromEntries(assets.filter((asset) => asset.legacy_id !== undefined)
        .map((asset) => [`${asset.kind}:${asset.legacy_id}`, asset.uid]));
      return parseLibraryCatalog({ ...(doc as object), assets, legacy_index });
    })
    .catch((error: unknown) => {
      pending = null;
      throw error;
    });
  return pending;
}
