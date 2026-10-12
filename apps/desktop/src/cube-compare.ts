// Same-WAV comparison geometry from the cube-compare branch, independent of the renderer.
export type LayerMethod = "library_r3" | "pipeline_r2";
export const LAYER_METHOD_LABELS: Record<LayerMethod, string> = {
  library_r3: "Library formulas, rev 3",
  pipeline_r2: "Pipeline formulas, rev 2 (PR #4)",
};

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
  hzPerBin: number;
  invHdr: number;
  layerScore: number;
}

export function layerMethodOf(doc: CubeDocFields): LayerMethod | null {
  if (doc.layer_method === "library_r3" || doc.layer_method === "pipeline_r2") return doc.layer_method;
  return doc.layer_method === undefined && typeof doc.layer_score === "number" ? "library_r3" : null;
}

const positive = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value) && value > 0;
function pair(value: unknown): [number, number] | null {
  return Array.isArray(value) && value.length === 2 && value.every(n => Number.isSafeInteger(n) && n > 0)
    ? [value[0], value[1]] : null;
}

export function sideFromDoc(url: string, doc: CubeDocFields, envelopeSha: string | null): CompareSide | string {
  const method = layerMethodOf(doc);
  if (!method) return `${url} does not record its layer method.`;
  const docSha = typeof doc.source_sha256 === "string" ? doc.source_sha256 : null;
  if (docSha && envelopeSha && docSha !== envelopeSha) return "Cube source_sha256 disagrees with its asset envelope.";
  const sha = docSha ?? envelopeSha;
  if (!sha || !/^[a-f0-9]{64}$/.test(sha)) return "Cube has no verified source_sha256; its WAV cannot be matched.";
  const shape = pair(doc.cube_shape_f_t), down = pair(doc.downsample_sf_st);
  if (!shape || !down) return "Cube has no cube_shape_f_t / downsample_sf_st.";
  if (!positive(doc.duration_s) || !positive(doc.sample_rate) || !positive(doc.bin_seconds) || !positive(doc.n_fft)) {
    return "Cube is missing duration_s, sample_rate, bin_seconds or n_fft.";
  }
  if (typeof doc.inv_hdr !== "number" || !Number.isFinite(doc.inv_hdr)
    || typeof doc.layer_score !== "number" || !Number.isFinite(doc.layer_score)) return "Cube has no measured inv_hdr / layer_score.";
  return {
    url, method, label: LAYER_METHOD_LABELS[method], sourceSha256: sha,
    durationS: doc.duration_s, sampleRate: doc.sample_rate, binSeconds: doc.bin_seconds,
    coversS: positive(doc.cube_covers_s) ? doc.cube_covers_s : shape[1] * doc.bin_seconds,
    timeBins: shape[1], freqBins: shape[0], hzPerBin: down[0] * doc.sample_rate / doc.n_fft,
    invHdr: doc.inv_hdr, layerScore: doc.layer_score,
  };
}

export function checkPair(left: CompareSide, right: CompareSide): { ok: true } | { ok: false; reason: string } {
  if (left.sourceSha256 !== right.sourceSha256) return { ok: false, reason: "Refusing to compare cubes from different WAVs (source_sha256)." };
  if (left.method === right.method) return { ok: false, reason: `Both cubes use ${left.method}.` };
  if (left.sampleRate !== right.sampleRate || Math.abs(left.durationS - right.durationS) > 1e-6) {
    return { ok: false, reason: "Same sha256 but different sample rate or duration." };
  }
  return { ok: true };
}

export interface SharedAxes { timeMaxS: number; freqMaxHz: number }
export function sharedAxes(sides: CompareSide[]): SharedAxes {
  return {
    timeMaxS: Math.max(...sides.map(side => side.durationS)),
    freqMaxHz: Math.max(...sides.map(side => Math.max(1, side.freqBins - 1) * side.hzPerBin)),
  };
}
export function sharedPoint(side: CompareSide, axes: SharedAxes, t: number, f: number): { x: number; y: number } {
  return {
    x: Math.min(1, Math.round(t * Math.max(0, side.timeBins - 1)) * side.binSeconds / axes.timeMaxS),
    y: Math.min(1, Math.round(f * Math.max(0, side.freqBins - 1)) * side.hzPerBin / axes.freqMaxHz),
  };
}
export interface SliceHead {
  method: LayerMethod;
  seconds: number;
  x: number;
  bin: number;
  timeBins: number;
  binX: number;
  binWidth: number;
  beyond: boolean;
}
export function slicesAt(seconds: number, sides: CompareSide[], axes: SharedAxes): SliceHead[] {
  const clock = Number.isFinite(seconds) ? Math.max(0, Math.min(axes.timeMaxS, seconds)) : 0;
  return sides.map(side => {
    const bin = Math.max(0, Math.min(side.timeBins - 1, Math.floor(clock / side.binSeconds)));
    return { method: side.method, seconds: clock, x: clock / axes.timeMaxS,
      bin, timeBins: side.timeBins, binX: bin * side.binSeconds / axes.timeMaxS,
      binWidth: side.binSeconds / axes.timeMaxS, beyond: clock > side.coversS + 1e-6 };
  });
}
export function formatSliceTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00.0";
  const tenths = Math.floor(seconds * 10 + 1e-6);
  return `${Math.floor(tenths / 600)}:${((tenths % 600) / 10).toFixed(1).padStart(4, "0")}`;
}
export function sliceReadout(heads: SliceHead[], durationS: number): string {
  return heads.length ? `Slice ${formatSliceTime(heads[0].seconds)} / ${formatSliceTime(durationS)} · ${heads.map(
    head => `${head.method} bin ${head.bin} / ${head.timeBins - 1}${head.beyond ? " (beyond cube)" : ""}`,
  ).join(" · ")}` : "";
}
export function paneHeading(side: CompareSide): string {
  return `${side.label} · inv_hdr ${side.invHdr.toFixed(4)} · layer_score ${side.layerScore.toFixed(4)}`;
}
