// Cube tab Compare mode, DOM-free: the clip's Library cube (layer_method
// library_r3) next to the same WAV's cube under PR #4's pipeline formulas
// (pipeline_r2), on shared axes, with ONE playback slice. The slice position
// comes from one number, the bound clip's seconds, so both cubes always show
// the same instant (drift 0 by construction). Formulas: gen_audio/cube_layers.py.

export type LayerMethod = "library_r3" | "pipeline_r2";

/** Exact pane labels (gen_audio.cube_layers.LAYER_METHOD_LABELS). */
export const LAYER_METHOD_LABELS: Record<LayerMethod, string> = {
  library_r3: "Library formulas, rev 3",
  pipeline_r2: "Pipeline formulas, rev 2 (PR #4)",
};

const SHA256 = /^[0-9a-f]{64}$/;

/** The cube JSON fields Compare reads. */
export interface CubeDocFields {
  layer_method?: unknown;
  layer_score?: unknown;
  source_sha256?: unknown;
  duration_s?: unknown;
  sample_rate?: unknown;
  bin_seconds?: unknown;
  cube_covers_s?: unknown;
  cube_shape_f_t?: unknown;
  downsample_sf_st?: unknown;
  n_fft?: unknown;
  inv_hdr?: unknown;
}

/**
 * pipeline_r2 JSON records layer_method. library_r3 JSON leaves it out (so the
 * shipped Library cube bytes and their pinned uids stay put) and is the only
 * generator that writes layer_score without it. Anything else is unknown.
 */
export function layerMethodOf(doc: CubeDocFields): LayerMethod | null {
  if (doc.layer_method === "library_r3" || doc.layer_method === "pipeline_r2") return doc.layer_method;
  if (doc.layer_method === undefined && typeof doc.layer_score === "number") return "library_r3";
  return null;
}

/** One side of the comparison, read from its cube JSON (plus its asset envelope's sha). */
export interface CompareSide {
  url: string;
  method: LayerMethod;
  label: string;
  sourceSha256: string;
  durationS: number;
  sampleRate: number;
  binSeconds: number;
  coversS: number;
  timeBins: number;
  freqBins: number;
  /** Hz per cube freq bin: downsample_sf * sample_rate / n_fft. */
  hzPerBin: number;
  invHdr: number;
  layerScore: number;
}

function positive(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

function pair(value: unknown): [number, number] | null {
  return Array.isArray(value) && value.length === 2 && value.every((n) => Number.isInteger(n) && n > 0)
    ? [Number(value[0]), Number(value[1])]
    : null;
}

/**
 * Build a side from a cube JSON. `envelopeSha` is the asset envelope's
 * fields.source_sha256 (assets.json). The JSON's own source_sha256, when it has
 * one, must agree with it. Returns a reason string instead of guessing.
 */
export function sideFromDoc(url: string, doc: CubeDocFields, envelopeSha: string | null): CompareSide | string {
  const method = layerMethodOf(doc);
  if (!method) return `${url} does not record its layer method; it is not drawn in Compare.`;
  const docSha = typeof doc.source_sha256 === "string" ? doc.source_sha256 : null;
  if (docSha && envelopeSha && docSha !== envelopeSha) {
    return `${url} says source_sha256 ${docSha.slice(0, 8)}\u2026 but its asset envelope says ${envelopeSha.slice(0, 8)}\u2026.`;
  }
  const sha = docSha ?? envelopeSha;
  if (!sha || !SHA256.test(sha)) return `${url} has no source_sha256 (cube JSON or assets.json), so its WAV cannot be matched.`;
  const shape = pair(doc.cube_shape_f_t);
  const down = pair(doc.downsample_sf_st);
  if (!shape || !down) return `${url} has no cube_shape_f_t / downsample_sf_st.`;
  if (!positive(doc.duration_s) || !positive(doc.sample_rate) || !positive(doc.bin_seconds) || !positive(doc.n_fft)) {
    return `${url} is missing duration_s, sample_rate, bin_seconds or n_fft.`;
  }
  if (typeof doc.inv_hdr !== "number" || typeof doc.layer_score !== "number") return `${url} has no inv_hdr / layer_score.`;
  return {
    url,
    method,
    label: LAYER_METHOD_LABELS[method],
    sourceSha256: sha,
    durationS: doc.duration_s,
    sampleRate: doc.sample_rate,
    binSeconds: doc.bin_seconds,
    coversS: positive(doc.cube_covers_s) ? doc.cube_covers_s : shape[1] * doc.bin_seconds,
    timeBins: shape[1],
    freqBins: shape[0],
    hzPerBin: (down[0] * doc.sample_rate) / doc.n_fft,
    invHdr: doc.inv_hdr,
    layerScore: doc.layer_score,
  };
}

export type PairCheck = { ok: true } | { ok: false; reason: string };

/** Two cubes are comparable only when they are the same WAV under two different methods. */
export function checkPair(left: CompareSide, right: CompareSide): PairCheck {
  if (left.sourceSha256 !== right.sourceSha256) {
    return {
      ok: false,
      reason: `Refusing to compare: the cubes come from different WAVs (source_sha256 ${left.sourceSha256.slice(0, 12)}\u2026 vs ${right.sourceSha256.slice(0, 12)}\u2026).`,
    };
  }
  if (left.method === right.method) return { ok: false, reason: `Refusing to compare: both cubes use ${left.method}.` };
  if (left.sampleRate !== right.sampleRate || Math.abs(left.durationS - right.durationS) > 1e-6) {
    return { ok: false, reason: "Refusing to compare: same sha256 but different sample rate or duration in the cube JSON." };
  }
  return { ok: true };
}

/** Shared axes: time in seconds of the WAV, frequency in Hz. Both cubes use the same scale. */
export interface SharedAxes {
  timeMaxS: number;
  freqMaxHz: number;
}

export function sharedAxes(sides: CompareSide[]): SharedAxes {
  return {
    timeMaxS: Math.max(...sides.map((side) => side.durationS)),
    freqMaxHz: Math.max(...sides.map((side) => Math.max(1, side.freqBins - 1) * side.hzPerBin)),
  };
}

/** A preview point's normalized (t, f) in its own JSON -> 0..1 on the shared axes (bin start time, bin Hz). */
export function sharedPoint(side: CompareSide, axes: SharedAxes, t: number, f: number): { x: number; y: number } {
  const timeBin = Math.round(t * Math.max(0, side.timeBins - 1));
  const freqBin = Math.round(f * Math.max(0, side.freqBins - 1));
  return {
    x: Math.min(1, (timeBin * side.binSeconds) / axes.timeMaxS),
    y: Math.min(1, (freqBin * side.hzPerBin) / axes.freqMaxHz),
  };
}

export interface SliceHead {
  method: LayerMethod;
  /** Seconds of the bound WAV: the one clock. */
  seconds: number;
  /** Slice position on the shared time axis, 0..1. Identical on every side. */
  x: number;
  /** This cube's time bin under the slice: floor(seconds / bin_seconds), clamped. */
  bin: number;
  timeBins: number;
  /** Start of that bin on the shared time axis, 0..1. */
  binX: number;
  /** Width of one of this cube's bins on the shared time axis. */
  binWidth: number;
  /** The slice is past what this cube covers (cube_covers_s). */
  beyond: boolean;
}

/** ONE clock -> the slice on every side. */
export function slicesAt(seconds: number, sides: CompareSide[], axes: SharedAxes): SliceHead[] {
  const clock = Number.isFinite(seconds) ? Math.max(0, Math.min(axes.timeMaxS, seconds)) : 0;
  const x = axes.timeMaxS > 0 ? clock / axes.timeMaxS : 0;
  return sides.map((side) => {
    const bin = Math.max(0, Math.min(side.timeBins - 1, Math.floor(clock / side.binSeconds)));
    return {
      method: side.method,
      seconds: clock,
      x,
      bin,
      timeBins: side.timeBins,
      binX: (bin * side.binSeconds) / axes.timeMaxS,
      binWidth: side.binSeconds / axes.timeMaxS,
      beyond: clock > side.coversS + 1e-6,
    };
  });
}

/** m:ss.s for the slice readout. */
export function formatSliceTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00.0";
  const tenths = Math.floor(seconds * 10 + 1e-6);
  const minutes = Math.floor(tenths / 600);
  const rest = (tenths % 600) / 10;
  return `${minutes}:${rest.toFixed(1).padStart(4, "0")}`;
}

/** Text readout of the one slice: time, then each cube's bin under it. */
export function sliceReadout(heads: SliceHead[], durationS: number): string {
  if (heads.length === 0) return "";
  const parts = heads.map((head) => `${head.method} bin ${head.bin} / ${head.timeBins - 1}${head.beyond ? " (beyond cube)" : ""}`);
  return `Slice ${formatSliceTime(heads[0].seconds)} / ${formatSliceTime(durationS)} \u00b7 ${parts.join(" \u00b7 ")}`;
}

/** Pane header: exact method label, inv_hdr and layer_score. */
export function paneHeading(side: CompareSide): string {
  return `${side.label} \u00b7 inv_hdr ${side.invHdr.toFixed(4)} \u00b7 layer_score ${side.layerScore.toFixed(4)}`;
}
