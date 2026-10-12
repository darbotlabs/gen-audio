import { semanticCubeValidator } from "./generated/contract-validators.js";

export const SEMANTIC_MAX_BYTES = 1024 * 1024;
export interface MetricAxis { label: string; unit: string; minimum: number; maximum: number }
export interface MetricLayer extends MetricAxis {
  id: string; metric: string; color: string; position: number;
  scale: "linear" | "log"; outside: "reject" | "clip";
  opacity: number; gain: number; blend: "normal" | "add" | "multiply"; enabled: boolean;
}
export interface MetricRecord {
  id: string; x: number; y: number; metrics: Record<string, number | null>;
  context: Record<string, string>;
}
export interface SemanticCube {
  schema_version: "1.0.0"; kind: "semantic_cube"; id: string; title: string;
  domain: "agent.session" | "storyline" | "camera" | "market.stock" | "market.crypto" | "custom";
  source: { kind: "measured" | "authored" | "example"; label: string };
  axes: { x: MetricAxis; y: MetricAxis }; layers: MetricLayer[]; records: MetricRecord[];
}
export interface MetricPoint { t: number; f: number; z: number; v: number; layer: string; recordId: string; raw: number }
export interface MetricProjection { ok: true; points: MetricPoint[]; records: number; skipped: number; clipped: number }

function range(axis: MetricAxis, label: string): void {
  if (axis.minimum >= axis.maximum) throw new Error(`${label}: minimum must be less than maximum`);
  if (axis.unit === "unix_s" && (axis.minimum < 0 || axis.maximum > 253402300799)) throw new Error(`${label}: Unix seconds must be within 1970–9999`);
}

function logSpan(value: number, base: number): number {
  const relative = (value - base) / base;
  // log(hi)-log(lo) cancels for adjacent large values. log1p preserves
  // that precision; direct logs cover ratios overflowing from subnormals.
  return Number.isFinite(relative) ? Math.log1p(relative) : Math.log(value) - Math.log(base);
}

/** Schema shape plus semantic invariants shared with the Rust projector. */
export function projectSemanticCube(value: unknown): { document: SemanticCube; projection: MetricProjection } {
  const raw = JSON.stringify(value);
  if (!raw || new TextEncoder().encode(raw).byteLength > SEMANTIC_MAX_BYTES) throw new Error("Semantic cube exceeds 1 MiB");
  if (!semanticCubeValidator(value)) {
    const error = semanticCubeValidator.errors?.[0];
    throw new Error(`Invalid semantic cube at ${error?.instancePath || "/"} (${error?.keyword ?? "schema"})`);
  }
  const doc = value;
  range(doc.axes.x, "x axis"); range(doc.axes.y, "y axis");
  const ids = new Set<string>();
  const planes: number[] = [];
  for (const layer of doc.layers) {
    if (ids.has(layer.id)) throw new Error(`Duplicate layer id: ${layer.id}`);
    ids.add(layer.id); range(layer, layer.id);
    if (planes.some(plane => Math.abs(layer.position - plane) < 0.001)) throw new Error("Layer positions must be distinct by at least 0.001");
    planes.push(layer.position);
    if (layer.scale === "log" && layer.minimum <= 0) throw new Error(`${layer.id}: log scale requires a positive minimum`);
    if (!doc.records.some(record => Object.hasOwn(record.metrics, layer.metric))) throw new Error(`Layer ${layer.id} refers to absent metric ${layer.metric}`);
  }
  ids.clear();
  const projection: MetricProjection = { ok: true, points: [], records: doc.records.length, skipped: 0, clipped: 0 };
  for (const record of doc.records) {
    if (ids.has(record.id)) throw new Error(`Duplicate record id: ${record.id}`);
    ids.add(record.id);
    if (record.x < doc.axes.x.minimum || record.x > doc.axes.x.maximum || record.y < doc.axes.y.minimum || record.y > doc.axes.y.maximum) {
      throw new Error(`Record ${record.id}: coordinate outside declared axes`);
    }
    if (doc.domain.startsWith("market.")) {
      const { open, high, low, close } = record.metrics;
      if (low! > high! || open! < low! || open! > high! || close! < low! || close! > high!) throw new Error(`Record ${record.id}: inconsistent OHLC candle`);
      const stamp = record.context.timestamp;
      const parsed = new Date(stamp);
      if (!Number.isFinite(parsed.getTime()) || parsed.toISOString().replace(".000Z", "Z") !== stamp.replace(".000Z", "Z")) {
        throw new Error(`Record ${record.id}: invalid timestamp`);
      }
    }
    for (const layer of doc.layers) {
      const raw = Object.hasOwn(record.metrics, layer.metric) ? record.metrics[layer.metric] : undefined;
      if (raw == null) { projection.skipped++; continue; }
      if (layer.scale === "log" && raw <= 0) throw new Error(`Record ${record.id}: log metric must be positive`);
      if (raw < layer.minimum || raw > layer.maximum) {
        if (layer.outside !== "clip") throw new Error(`Record ${record.id}: metric outside layer range`);
        projection.clipped++;
      }
      const value = Math.max(layer.minimum, Math.min(layer.maximum, raw));
      projection.points.push({ t: (record.x-doc.axes.x.minimum)/(doc.axes.x.maximum-doc.axes.x.minimum),
        f: (record.y-doc.axes.y.minimum)/(doc.axes.y.maximum-doc.axes.y.minimum), z: layer.position,
        v: layer.scale === "log" ? logSpan(value,layer.minimum)/logSpan(layer.maximum,layer.minimum)
          : (value-layer.minimum)/(layer.maximum-layer.minimum), layer: layer.id, recordId: record.id, raw });
    }
  }
  if (!projection.points.length) throw new Error("Semantic cube has no observed metric values");
  return { document: structuredClone(doc), projection };
}

export function parseSemanticCube(raw: string): ReturnType<typeof projectSemanticCube> {
  if (new TextEncoder().encode(raw).byteLength > SEMANTIC_MAX_BYTES) throw new Error("Semantic cube exceeds 1 MiB");
  return projectSemanticCube(JSON.parse(raw));
}

export function hexRgb(color: string): [number, number, number] {
  if (!/^#[a-f0-9]{6}$/i.test(color)) throw new Error("Layer color must be #RRGGBB");
  return [1, 3, 5].map(offset => Number.parseInt(color.slice(offset, offset + 2), 16) / 255) as [number, number, number];
}

export function axisValue(axis: MetricAxis, fraction: number): string {
  const value = axis.minimum + fraction * (axis.maximum - axis.minimum);
  if (axis.unit === "unix_s") return new Date(value * 1000).toISOString().slice(11, 19);
  return new Intl.NumberFormat("en", { maximumFractionDigits: 3, notation: Math.abs(value) >= 10000 ? "compact" : "standard" }).format(value);
}
