// Read-only view of /library/assets.json (asset object model v1) for the UI.
// The UI never mints uids; it only reads the generated catalog. Entries with a
// malformed uid or an unsafe media path are dropped rather than trusted.

import { checkMediaPath, isUid } from "./asset";

export interface AssetMedia {
  role: string;
  path: string;
  sha256: string;
  bytes: number;
  mime?: string;
}

export interface AssetEnvelope {
  uid: string;
  kind: string;
  legacy_id?: string;
  status: string;
  fields: Record<string, unknown>;
  media: AssetMedia[];
  src: string[];
  relations?: Record<string, unknown>;
  honesty?: { synthesized_speech?: boolean; fixture?: boolean; claims?: string[] };
  provenance?: { [key: string]: unknown; generator?: string; params?: Record<string, unknown> };
  body?: Record<string, unknown>;
  display: { title: string; glyph: string; display_rev?: number };
}

/** One real cube a voice model made (through its clip), for the model card's Cube tab. */
export interface ModelCubeRow {
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
    // The clip's own cube; a comparison cube (another layer_method) of the same WAV is not the model's cube.
    const cube = assets.find(
      (asset) => asset.kind === "cube_ihdr" && asset.src.length === 1 && asset.src[0] === clip.uid && !comparisonMethod(asset),
    );
    if (!cube) continue;
    const shape = cube.body?.cube_shape_f_t;
    rows.push({
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

export interface LibraryCatalog {
  byUid: Map<string, AssetEnvelope>;
  assets: readonly AssetEnvelope[];
  modelCubes(modelId: string): ModelCubes;
  clipForTile(tileId: string): AssetEnvelope | null;
  /** The clip's own spectrogram or cube; comparison cubes (another layer_method) never count. */
  derivedFrom(clipUid: string, kind: "spectrogram_2d" | "cube_ihdr"): AssetEnvelope | null;
  cubeForUrl(url: string): AssetEnvelope | null;
  /** Comparison cubes of the clip's WAV (cube_ihdr with provenance.params.layer_method, e.g. pipeline_r2). */
  compareCubesFor(clipUid: string): AssetEnvelope[];
}

/** layer_method of a comparison cube envelope; null for a clip's own cube. */
export function comparisonMethod(asset: AssetEnvelope): string | null {
  const method = asset.provenance?.params?.layer_method;
  return asset.kind === "cube_ihdr" && typeof method === "string" && method !== "library_r3" ? method : null;
}

/** fields.source_sha256 of a derived asset, when it is a sha256. */
export function sourceSha256(asset: AssetEnvelope | null | undefined): string | null {
  const sha = asset?.fields?.source_sha256;
  return typeof sha === "string" && /^[0-9a-f]{64}$/.test(sha) ? sha : null;
}

let pending: Promise<LibraryCatalog | null> | null = null;

function safe(asset: unknown): asset is AssetEnvelope {
  if (!asset || typeof asset !== "object") return false;
  const record = asset as Partial<AssetEnvelope>;
  if (!isUid(record.uid) || typeof record.kind !== "string" || !record.uid.startsWith(`ga:${record.kind}:`)) return false;
  if (!Array.isArray(record.media) || !Array.isArray(record.src)) return false;
  try {
    record.media.forEach((item) => checkMediaPath(item.path));
  } catch {
    return false;
  }
  return record.src.every((uid) => isUid(uid));
}

export function mediaUrl(asset: AssetEnvelope, role: string): string | null {
  const item = asset.media.find((entry) => entry.role === role);
  return item ? `/library/${item.path}` : null;
}

/** `extraAssets`: dev/test-only envelopes (VITE_GEN_AUDIO_FIXTURES=1); release passes none. */
export function loadLibraryCatalog(extraAssets: Promise<unknown[]> = Promise.resolve([])): Promise<LibraryCatalog | null> {
  pending ??= Promise.all([fetch("/library/assets.json").then((response) => (response.ok ? response.json() : null)), extraAssets.catch(() => [])])
    .then(([doc, extra]: [{ assets?: unknown[] } | null, unknown[]]) => {
      if (!doc || !Array.isArray(doc.assets)) return null;
      const assets = [...doc.assets, ...extra].filter(safe);
      const byUid = new Map(assets.map((asset) => [asset.uid, asset]));
      return {
        byUid,
        assets,
        modelCubes: (modelId) => modelCubes(assets, modelId),
        clipForTile: (tileId) => assets.find((asset) => asset.kind === "audio_clip" && asset.legacy_id === tileId) ?? null,
        derivedFrom: (clipUid, kind) =>
          assets.find((asset) => asset.kind === kind && asset.src.length === 1 && asset.src[0] === clipUid && !comparisonMethod(asset)) ?? null,
        cubeForUrl: (url) => assets.find((asset) => asset.kind === "cube_ihdr" && mediaUrl(asset, "cube_json") === url) ?? null,
        compareCubesFor: (clipUid) =>
          assets.filter((asset) => comparisonMethod(asset) !== null && asset.src.length === 1 && asset.src[0] === clipUid),
      } satisfies LibraryCatalog;
    })
    .catch(() => null);
  return pending;
}
