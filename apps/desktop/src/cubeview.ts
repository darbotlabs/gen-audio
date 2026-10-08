/** Interactive library cube. Points come from cube JSON only. No invented layers. */

export interface CubePoint {
  t: number;
  f: number;
  z: number;
  v: number;
  layer: string;
}

export type BlendMode = "normal" | "add" | "multiply";

export interface LayerParams {
  id: string;
  on: boolean;
  opacity: number; // 0..1
  gain: number; // 0.25..3
  blend: BlendMode;
  order: number; // draw order only; the plane height comes from the layer (z) in JSON
}

/** Legend colors match the matplotlib reference PNG: blue / green / orange / pink. */
export const LAYER_COLORS: Record<string, [number, number, number]> = {
  signal: [0.12, 0.53, 1],
  tonality: [0.16, 0.87, 0.36],
  confidence: [1, 0.75, 0.12],
  quality: [0.94, 0.27, 0.56],
};

export interface CubeMeta {
  url: string;
  name: string;
  title: string;
  invHdr: number | null;
  points: number;
  timeBins: number;
  freqBins: number;
  /** WAV duration the cube JSON was computed from. */
  durationS: number | null;
  /** Seconds per cube time bin (hop x time downsample / sr). */
  binSeconds: number | null;
  /** Seconds the cube's time bins actually cover. */
  coversS: number | null;
  revision: number | null;
  pngUrl: string;
}

export interface CubePlayhead {
  seconds: number;
  bin: number;
  timeBins: number;
  x: number; // normalized 0..1 bin position
  beyond: boolean; // audio time is past what the cube covers
}

const EXPECTED = ["signal", "tonality", "confidence", "quality"] as const;

/** Box half-extents: time is the long axis (like the reference), freq is depth, layers are height. */
const BOX: [number, number, number] = [1.5, 0.85, 1.05];
const CAMERA = 4.6;
const SCALE = 2.1;
const LIFT = 0.1;

interface GlCache {
  gl: WebGLRenderingContext;
  points: WebGLProgram;
  lines: WebGLProgram;
  layerBuffers: Map<string, { buffer: WebGLBuffer; count: number }>;
  gridBuffer: WebGLBuffer | null;
  gridCount: number;
  headBuffer: WebGLBuffer | null;
}

const state = {
  points: [] as CubePoint[],
  layerIds: [] as string[],
  layers: new Map<string, LayerParams>(),
  yaw: -0.45,
  pitch: -0.55,
  zoom: 1,
  scrub: 0,
  canvas: null as HTMLCanvasElement | null,
  labels: null as HTMLCanvasElement | null,
  meta: null as CubeMeta | null,
  resize: null as ResizeObserver | null,
  unbind: null as AbortController | null,
  sourceUrl: "",
  clockListeners: new Set<(fraction: number) => void>(),
  rafId: 0 as number,
  audioClock: null as (() => number | null) | null,
  glCache: null as GlCache | null,
  glLost: false,
  glFailed: "",
  dataVersion: 0,
  builtVersion: -1,
};

function ensureLayer(id: string, order: number): LayerParams {
  const existing = state.layers.get(id);
  if (existing) return existing;
  const created: LayerParams = { id, on: true, opacity: 1, gain: 1, blend: "normal", order };
  state.layers.set(id, created);
  return created;
}

/**
 * Pitch stays above the horizon (pitch 0 is edge-on, negative looks down).
 * The planes have a fixed draw order (signal first ... quality last), which
 * only stacks correctly when viewed from above.
 */
const PITCH_MIN = -1.45;
const PITCH_MAX = -0.08;
function clampPitch(pitch: number): number {
  return Math.max(PITCH_MIN, Math.min(PITCH_MAX, pitch));
}

function dpr(): number {
  return Math.min(2, Math.max(1, window.devicePixelRatio || 1));
}

/**
 * Bind the WebGL cube canvas. The canvas fills its host: a ResizeObserver
 * sizes the backing store to CSS size x devicePixelRatio (clamped to 2), so
 * there is no fixed 640x360 buffer. An optional 2D label canvas draws ticks/axes.
 */
export function bindCube(canvas: HTMLCanvasElement, labels?: HTMLCanvasElement | null): void {
  unbindCube();
  const listeners = new AbortController();
  const signal = listeners.signal;
  state.unbind = listeners;
  state.canvas = canvas;
  state.labels = labels ?? null;
  state.glCache = null;
  state.glFailed = "";
  const fit = () => {
    const ratio = dpr();
    const width = Math.max(1, Math.round(canvas.clientWidth * ratio));
    const height = Math.max(1, Math.round(canvas.clientHeight * ratio));
    for (const node of [canvas, state.labels]) {
      if (!node) continue;
      if (node.width !== width) node.width = width;
      if (node.height !== height) node.height = height;
    }
    draw();
  };
  if (typeof ResizeObserver !== "undefined") {
    state.resize = new ResizeObserver(fit);
    state.resize.observe(canvas.parentElement ?? canvas);
  }
  window.addEventListener("resize", fit, { signal });
  // Re-arm on DPR changes (display scaling, monitor moves, zoom).
  const watchDpr = () => {
    const query = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
    query.addEventListener("change", () => {
      if (signal.aborted) return;
      fit();
      watchDpr();
    }, { once: true, signal });
  };
  watchDpr();
  canvas.addEventListener("webglcontextlost", (event) => {
    event.preventDefault();
    state.glLost = true;
    state.glCache = null;
  }, { signal });
  canvas.addEventListener("webglcontextrestored", () => {
    state.glLost = false;
    state.glCache = null;
    state.builtVersion = -1;
    draw();
  }, { signal });
  fit();
  let drag: { x: number; y: number; yaw: number; pitch: number } | null = null;
  canvas.addEventListener("pointerdown", (event) => {
    drag = { x: event.clientX, y: event.clientY, yaw: state.yaw, pitch: state.pitch };
    canvas.setPointerCapture(event.pointerId);
  }, { signal });
  canvas.addEventListener("pointermove", (event) => {
    if (!drag) return;
    state.yaw = drag.yaw + (event.clientX - drag.x) * 0.01;
    state.pitch = clampPitch(drag.pitch - (event.clientY - drag.y) * 0.01);
    draw();
  }, { signal });
  const end = () => {
    drag = null;
  };
  canvas.addEventListener("pointerup", end, { signal });
  canvas.addEventListener("pointercancel", end, { signal });
  canvas.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      state.zoom = Math.max(0.4, Math.min(2.4, state.zoom * (event.deltaY > 0 ? 0.92 : 1.08)));
      draw();
    },
    { passive: false, signal },
  );
}

/** Drop every listener/observer bindCube added (called on re-render and teardown). */
export function unbindCube(): void {
  state.unbind?.abort();
  state.unbind = null;
  state.resize?.disconnect();
  state.resize = null;
}

export function getCubeMeta(): CubeMeta | null {
  return state.meta ? { ...state.meta } : null;
}

export function boundCubeUrl(): string {
  return state.sourceUrl;
}

/** Short clip name from a cube URL: library_<name>_cube3d.json -> <name>. */
export function cubeNameFromUrl(url: string): string {
  const file = url.split("/").pop() ?? url;
  return file.replace(/\.json$/i, "").replace(/^library_/, "").replace(/_cube3d$/, "");
}

export function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const whole = Math.floor(seconds);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

/**
 * Playhead by SECONDS: audio time -> cube time bin using the cube's own bin size.
 * A cube shorter than the WAV is never stretched; past its end the bin clamps and
 * `beyond` is true.
 */
export function getCubePlayhead(): CubePlayhead | null {
  const meta = state.meta;
  if (!meta || meta.timeBins < 2) return null;
  const duration = meta.durationS ?? 0;
  const seconds = state.scrub * duration;
  const binSeconds = meta.binSeconds ?? (duration > 0 ? duration / meta.timeBins : 0);
  if (!(binSeconds > 0)) return null;
  const raw = Math.floor(seconds / binSeconds);
  const bin = Math.max(0, Math.min(meta.timeBins - 1, raw));
  const covers = meta.coversS ?? meta.timeBins * binSeconds;
  return { seconds, bin, timeBins: meta.timeBins, x: bin / (meta.timeBins - 1), beyond: seconds > covers + 1e-6 };
}

/** ONE clock: fraction 0..1 of the bound clip's WAV. Optionally notify audio scrubbers. */
export function setCubeScrub(fraction: number, opts?: { silent?: boolean }): void {
  state.scrub = Math.max(0, Math.min(1, fraction));
  draw();
  if (!opts?.silent) {
    for (const listener of state.clockListeners) listener(state.scrub);
  }
  const scrub = document.querySelector<HTMLInputElement>("#cube-scrub");
  if (scrub && !scrub.matches(":active")) scrub.value = String(Math.round(state.scrub * 1000));
  syncClockUi();
}

export function getCubeScrub(): number {
  return state.scrub;
}

/** Readout + coverage badge + covered scrub segment. Classes only (CSP style-src 'self'). */
function syncClockUi(): void {
  const meta = state.meta;
  const head = getCubePlayhead();
  const clock = document.querySelector<HTMLElement>("#cube-clock");
  if (clock) {
    clock.textContent = head && meta
      ? `${formatClock(head.seconds)} / ${formatClock(meta.durationS ?? 0)} \u00b7 bin ${head.bin} / ${head.timeBins - 1}${head.beyond ? " \u00b7 beyond cube" : ""}`
      : "";
    clock.classList.toggle("is-beyond", Boolean(head?.beyond));
  }
  const badge = document.querySelector<HTMLElement>("#cube-badge");
  if (badge) {
    if (meta && meta.durationS) {
      const covers = meta.coversS ?? meta.durationS;
      const full = covers >= meta.durationS - 1;
      badge.textContent = `Library \u00b7 cube covers 0:00\u2013${formatClock(covers)} of ${formatClock(meta.durationS)}${meta.revision ? ` \u00b7 rev ${meta.revision}` : ""}`;
      badge.classList.toggle("is-partial", !full);
      badge.classList.toggle("is-beyond", Boolean(head?.beyond));
    } else {
      badge.textContent = "";
    }
  }
}

/**
 * LIVE viewport: drive cube time-bin from audio via rAF while playing.
 * getFraction returns 0..1 from the cube's bound Library WAV, or null if idle.
 */
export function startLiveCubeClock(getFraction: () => number | null): void {
  state.audioClock = getFraction;
  if (state.rafId) return;
  const tick = () => {
    state.rafId = window.requestAnimationFrame(tick);
    if (!state.audioClock) return;
    const fraction = state.audioClock();
    if (fraction === null || !Number.isFinite(fraction)) return;
    // silent: do not re-seek audio; only advance the bitdot time-slice + scrub UI
    setCubeScrub(fraction, { silent: true });
    const fp = document.querySelector<HTMLInputElement>("#fp-scrub");
    if (fp && !fp.matches(":active")) fp.value = String(Math.round(fraction * 1000));
  };
  state.rafId = window.requestAnimationFrame(tick);
}

export function stopLiveCubeClock(): void {
  state.audioClock = null;
  if (state.rafId) {
    window.cancelAnimationFrame(state.rafId);
    state.rafId = 0;
  }
}

export function onCubeClock(listener: (fraction: number) => void): () => void {
  state.clockListeners.add(listener);
  return () => {
    state.clockListeners.delete(listener);
  };
}

export function setCubeLayer(layer: string, on: boolean): void {
  if (!state.layers.has(layer)) return; // never invent layers
  state.layers.get(layer)!.on = on;
  draw();
}

export function setLayerOpacity(layer: string, opacity: number): void {
  if (!state.layers.has(layer)) return;
  state.layers.get(layer)!.opacity = Math.max(0, Math.min(1, opacity));
  draw();
}

export function setLayerGain(layer: string, gain: number): void {
  if (!state.layers.has(layer)) return;
  state.layers.get(layer)!.gain = Math.max(0.25, Math.min(3, gain));
  draw();
}

export function setLayerBlend(layer: string, blend: BlendMode): void {
  if (!state.layers.has(layer)) return;
  state.layers.get(layer)!.blend = blend;
  draw();
}

/** Reorder by id list. Unknown ids ignored; missing kept at end. JSON layer set only. */
export function setLayerOrder(orderIds: string[]): void {
  const known = orderIds.filter((id) => state.layers.has(id));
  const rest = state.layerIds.filter((id) => !known.includes(id));
  state.layerIds = [...known, ...rest];
  state.layerIds.forEach((id, index) => {
    const params = state.layers.get(id);
    if (params) params.order = index;
  });
  draw();
}

export function listCubeLayers(): LayerParams[] {
  return state.layerIds.map((id) => ({ ...state.layers.get(id)! })).sort((a, b) => a.order - b.order);
}

function resetCube(): void {
  state.points = [];
  state.layerIds = [];
  state.layers.clear();
  state.sourceUrl = "";
  state.meta = null;
  state.dataVersion += 1;
}

export async function loadCube(url: string): Promise<string> {
  let data: {
    points_preview?: CubePoint[];
    engine?: string;
    inv_hdr?: number;
    title?: string;
    duration_s?: number;
    cube_shape_f_t?: number[];
    bin_seconds?: number;
    cube_covers_s?: number;
    cube_revision?: number;
    pngUrl?: string;
    layers?: Record<string, unknown> | string[];
  };
  try {
    const response = await fetch(url);
    if (!response.ok) {
      resetCube();
      draw();
      return `Cube JSON did not load (${response.status}).`;
    }
    data = await response.json();
  } catch (error) {
    resetCube();
    draw();
    return `Cube JSON did not load (${String(error)}).`;
  }
  const points = Array.isArray(data.points_preview) ? data.points_preview : [];
  const namesFromDict = data.layers && !Array.isArray(data.layers) ? Object.keys(data.layers) : [];
  const namesFromArr = Array.isArray(data.layers) ? data.layers.map(String) : [];
  const names = (namesFromDict.length ? namesFromDict : namesFromArr).filter((name) =>
    (EXPECTED as readonly string[]).includes(name),
  );
  // Only accept the four honesty layers present in JSON — do not invent extras.
  const missing = EXPECTED.filter((name) => !names.includes(name));
  if (points.length === 0 || missing.length > 0) {
    resetCube();
    draw();
    return "Cube JSON has no signal/tonality/confidence/quality preview.";
  }
  state.points = points.filter((point) => names.includes(point.layer));
  state.layerIds = [...names];
  state.layers.clear();
  names.forEach((id, index) => ensureLayer(id, index));
  state.scrub = 0;
  state.sourceUrl = url;
  state.dataVersion += 1;
  const name = cubeNameFromUrl(url);
  const shape = shapeOf(data);
  const durationS = typeof data.duration_s === "number" && data.duration_s > 0 ? data.duration_s : null;
  const binSeconds =
    typeof data.bin_seconds === "number" && data.bin_seconds > 0
      ? data.bin_seconds
      : durationS && shape[1] > 0
        ? durationS / shape[1]
        : null;
  state.meta = {
    url,
    name,
    title: typeof data.title === "string" && data.title ? data.title : `Inverse-HDR bitdot cube \u2014 ${name}`,
    invHdr: typeof data.inv_hdr === "number" ? data.inv_hdr : null,
    points: state.points.length,
    freqBins: shape[0],
    timeBins: shape[1],
    durationS,
    binSeconds,
    coversS:
      typeof data.cube_covers_s === "number" && data.cube_covers_s > 0
        ? data.cube_covers_s
        : binSeconds && shape[1] > 0
          ? binSeconds * shape[1]
          : durationS,
    revision: typeof data.cube_revision === "number" ? data.cube_revision : null,
    pngUrl: typeof data.pngUrl === "string" && data.pngUrl.startsWith("/") ? data.pngUrl : "",
  };
  draw();
  if (state.glFailed) showFallback(state.glFailed);
  rebuildLayerMatrixUi();
  syncClockUi();
  const inv = state.meta.invHdr === null ? "?" : state.meta.invHdr.toFixed(4);
  const span = durationS ? ` \u00b7 ${formatClock(durationS)} \u00b7 ${shape[1]} time bins` : "";
  return `${state.meta.title} \u00b7 inv_hdr ${inv} \u00b7 ${state.points.length} points${span} \u00b7 axes time bin / freq bin / layer`;
}

function shapeOf(data: { cube_shape_f_t?: number[]; layers?: Record<string, unknown> | string[] }): [number, number] {
  const direct = data.cube_shape_f_t;
  if (Array.isArray(direct) && direct.length === 2 && direct.every((n) => Number.isFinite(n) && n > 0)) {
    return [Number(direct[0]), Number(direct[1])];
  }
  if (data.layers && !Array.isArray(data.layers)) {
    for (const value of Object.values(data.layers)) {
      const shape = (value as { shape?: unknown })?.shape;
      if (Array.isArray(shape) && shape.length === 2 && shape.every((n) => Number.isFinite(n) && Number(n) > 0)) {
        return [Number(shape[0]), Number(shape[1])];
      }
    }
  }
  return [0, 0];
}

export function clearCube(message: string): void {
  resetCube();
  draw();
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  if (caption) caption.textContent = message;
  rebuildLayerMatrixUi();
  syncClockUi();
}

/** Build / refresh opacity·gain·blend·reorder controls from JSON layers only. */
export function rebuildLayerMatrixUi(): void {
  const host = document.querySelector<HTMLElement>("#cube-layer-matrix");
  if (!host) return;
  host.replaceChildren();
  const layers = listCubeLayers();
  if (layers.length === 0) {
    const empty = document.createElement("p");
    empty.className = "summary";
    empty.textContent = "No library cube layers loaded. Matrix stays empty (honest).";
    host.append(empty);
    return;
  }
  for (const layer of layers) {
    const row = document.createElement("div");
    row.className = "cube-layer-row";
    row.dataset.layerId = layer.id;

    const toggle = document.createElement("input");
    toggle.type = "checkbox";
    toggle.checked = layer.on;
    toggle.dataset.cubeLayer = layer.id;
    toggle.setAttribute("aria-label", `Toggle ${layer.id}`);
    toggle.addEventListener("change", () => setCubeLayer(layer.id, toggle.checked));

    const name = document.createElement("span");
    name.className = "cube-layer-name";
    name.textContent = layer.id;
    name.dataset.layer = layer.id; // swatch color via CSS class rules (CSP style-src self)

    const opacity = document.createElement("input");
    opacity.type = "range";
    opacity.min = "0";
    opacity.max = "100";
    opacity.value = String(Math.round(layer.opacity * 100));
    opacity.setAttribute("aria-label", `${layer.id} opacity`);
    opacity.addEventListener("input", () => setLayerOpacity(layer.id, Number(opacity.value) / 100));

    const gain = document.createElement("input");
    gain.type = "range";
    gain.min = "25";
    gain.max = "300";
    gain.value = String(Math.round(layer.gain * 100));
    gain.setAttribute("aria-label", `${layer.id} gain`);
    gain.addEventListener("input", () => setLayerGain(layer.id, Number(gain.value) / 100));

    const blend = document.createElement("select");
    blend.setAttribute("aria-label", `${layer.id} blend`);
    for (const mode of ["normal", "add", "multiply"] as BlendMode[]) {
      const opt = document.createElement("option");
      opt.value = mode;
      opt.textContent = mode;
      if (mode === layer.blend) opt.selected = true;
      blend.append(opt);
    }
    blend.addEventListener("change", () => setLayerBlend(layer.id, blend.value as BlendMode));

    const up = document.createElement("button");
    up.type = "button";
    up.textContent = "↑";
    up.setAttribute("aria-label", `Draw ${layer.id} earlier`);
    up.title = "Draw order only. Plane height stays fixed by layer.";
    up.addEventListener("click", () => {
      const ids = listCubeLayers().map((item) => item.id);
      const index = ids.indexOf(layer.id);
      if (index > 0) {
        [ids[index - 1], ids[index]] = [ids[index], ids[index - 1]];
        setLayerOrder(ids);
        rebuildLayerMatrixUi();
      }
    });

    const down = document.createElement("button");
    down.type = "button";
    down.textContent = "↓";
    down.setAttribute("aria-label", `Draw ${layer.id} later`);
    down.title = "Draw order only. Plane height stays fixed by layer.";
    down.addEventListener("click", () => {
      const ids = listCubeLayers().map((item) => item.id);
      const index = ids.indexOf(layer.id);
      if (index >= 0 && index < ids.length - 1) {
        [ids[index + 1], ids[index]] = [ids[index], ids[index + 1]];
        setLayerOrder(ids);
        rebuildLayerMatrixUi();
      }
    });

    row.append(toggle, name, opacity, gain, blend, up, down);
    host.append(row);
  }
}

/** Layer planes stack vertically like the matplotlib reference: signal bottom, quality top. Flat, no jitter. */
function planeY(z: number): number {
  return -0.9 + z * 1.8;
}

function aspect(): number {
  const canvas = state.canvas;
  if (!canvas || canvas.clientHeight === 0) return 1;
  return canvas.clientWidth / canvas.clientHeight;
}

/** Same transform as the vertex shaders, for the 2D label overlay. Returns buffer pixel coords. */
function project(x: number, y: number, z: number): { px: number; py: number } | null {
  const canvas = state.canvas;
  if (!canvas) return null;
  const cy = Math.cos(state.yaw);
  const sy = Math.sin(state.yaw);
  const cx = Math.cos(state.pitch);
  const sx = Math.sin(state.pitch);
  x *= BOX[0];
  y *= BOX[1];
  z *= BOX[2];
  const x1 = x * cy - z * sy;
  const z1 = x * sy + z * cy;
  const y2 = y * cx - z1 * sx;
  const z2 = y * sx + z1 * cx;
  const w = Math.max(0.2, CAMERA + z2);
  const a = aspect();
  let nx = (x1 * state.zoom * SCALE) / w;
  let ny = (y2 * state.zoom * SCALE) / w;
  if (a > 1) nx /= a;
  else ny *= a;
  ny += LIFT;
  return { px: ((nx + 1) / 2) * canvas.width, py: ((1 - ny) / 2) * canvas.height };
}

const SHARED_VS = `
  uniform float u_yaw;
  uniform float u_pitch;
  uniform float u_zoom;
  uniform float u_aspect;
  uniform vec3 u_box;
  uniform float u_camera;
  uniform float u_scale;
  uniform float u_lift;
  vec4 place(vec3 a) {
    vec3 p = a * u_box;
    float cy = cos(u_yaw);
    float sy = sin(u_yaw);
    float cx = cos(u_pitch);
    float sx = sin(u_pitch);
    float x1 = p.x * cy - p.z * sy;
    float z1 = p.x * sy + p.z * cy;
    float y2 = p.y * cx - z1 * sx;
    float z2 = p.y * sx + z1 * cx;
    float w = max(u_camera + z2, 0.2);
    float px = x1 * u_zoom * u_scale;
    float py = y2 * u_zoom * u_scale;
    if (u_aspect > 1.0) px = px / u_aspect; else py = py * u_aspect;
    return vec4(px, py + u_lift * w, 0.0, w);
  }
`;

const POINTS_VS = `
  attribute vec4 a_pt; // x, y(plane), z(freq), v
  uniform vec3 u_color;
  uniform float u_opacity;
  uniform float u_gain;
  uniform float u_head;
  uniform float u_band;
  uniform float u_point;
  varying vec4 v_color;
  ${SHARED_VS}
  void main() {
    gl_Position = place(a_pt.xyz);
    float g = clamp(a_pt.w * u_gain, 0.0, 1.0);
    float near = 1.0 - step(u_band, abs(a_pt.x - u_head));
    float alpha = (0.35 + 0.65 * g) * u_opacity * mix(0.72, 1.0, near);
    v_color = vec4(mix(u_color, vec3(1.0), 0.45 * near), alpha);
    gl_PointSize = u_point * (0.6 + 0.8 * g) * (1.0 + 0.9 * near);
  }
`;

const POINTS_FS = `
  precision mediump float;
  varying vec4 v_color;
  void main() {
    if (length(gl_PointCoord - vec2(0.5)) > 0.5) discard;
    gl_FragColor = vec4(v_color.rgb * v_color.a, v_color.a);
  }
`;

const LINES_VS = `
  attribute vec3 a_pos;
  attribute vec4 a_color;
  varying vec4 v_color;
  ${SHARED_VS}
  void main() {
    gl_Position = place(a_pos);
    v_color = a_color;
  }
`;

const LINES_FS = `
  precision mediump float;
  varying vec4 v_color;
  void main() { gl_FragColor = vec4(v_color.rgb * v_color.a, v_color.a); }
`;

function compile(gl: WebGLRenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("createShader failed");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS) && !gl.isContextLost()) {
    const log = gl.getShaderInfoLog(shader) ?? "unknown";
    gl.deleteShader(shader);
    throw new Error(`shader compile failed: ${log}`);
  }
  return shader;
}

function link(gl: WebGLRenderingContext, vs: string, fs: string): WebGLProgram {
  const program = gl.createProgram();
  if (!program) throw new Error("createProgram failed");
  gl.attachShader(program, compile(gl, gl.VERTEX_SHADER, vs));
  gl.attachShader(program, compile(gl, gl.FRAGMENT_SHADER, fs));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS) && !gl.isContextLost()) {
    throw new Error(`program link failed: ${gl.getProgramInfoLog(program) ?? "unknown"}`);
  }
  return program;
}

function glCache(): GlCache | null {
  const canvas = state.canvas;
  if (!canvas || state.glLost || state.glFailed) return null;
  if (state.glCache) return state.glCache;
  const gl = canvas.getContext("webgl", { alpha: false, preserveDrawingBuffer: true, antialias: true });
  if (!gl) {
    showFallback("WebGL is unavailable in this view.");
    return null;
  }
  try {
    state.glCache = {
      gl,
      points: link(gl, POINTS_VS, POINTS_FS),
      lines: link(gl, LINES_VS, LINES_FS),
      layerBuffers: new Map(),
      gridBuffer: null,
      gridCount: 0,
      headBuffer: gl.createBuffer(),
    };
    state.builtVersion = -1;
    return state.glCache;
  } catch (error) {
    console.error("[cube]", error);
    showFallback(`WebGL shader error: ${String(error)}`);
    return null;
  }
}

/** Static VBOs built once per loaded cube (and after a context restore), not per frame. */
function buildBuffers(cache: GlCache): void {
  if (state.builtVersion === state.dataVersion) return;
  const { gl } = cache;
  for (const entry of cache.layerBuffers.values()) gl.deleteBuffer(entry.buffer);
  cache.layerBuffers.clear();
  for (const id of state.layerIds) {
    const data: number[] = [];
    for (const point of state.points) {
      if (point.layer !== id) continue;
      data.push(point.t * 2 - 1, planeY(point.z), point.f * 2 - 1, point.v);
    }
    const buffer = gl.createBuffer();
    if (!buffer) continue;
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(data), gl.STATIC_DRAW);
    cache.layerBuffers.set(id, { buffer, count: data.length / 4 });
  }
  const grid = gridLines();
  if (cache.gridBuffer) gl.deleteBuffer(cache.gridBuffer);
  cache.gridBuffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, cache.gridBuffer);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(grid), gl.STATIC_DRAW);
  cache.gridCount = grid.length / 7;
  state.builtVersion = state.dataVersion;
}

function setShared(gl: WebGLRenderingContext, program: WebGLProgram): void {
  gl.uniform1f(gl.getUniformLocation(program, "u_yaw"), state.yaw);
  gl.uniform1f(gl.getUniformLocation(program, "u_pitch"), state.pitch);
  gl.uniform1f(gl.getUniformLocation(program, "u_zoom"), state.zoom);
  gl.uniform1f(gl.getUniformLocation(program, "u_aspect"), aspect());
  gl.uniform3f(gl.getUniformLocation(program, "u_box"), BOX[0], BOX[1], BOX[2]);
  gl.uniform1f(gl.getUniformLocation(program, "u_camera"), CAMERA);
  gl.uniform1f(gl.getUniformLocation(program, "u_scale"), SCALE);
  gl.uniform1f(gl.getUniformLocation(program, "u_lift"), LIFT);
}

function drawLines(cache: GlCache, buffer: WebGLBuffer | null, count: number): void {
  if (!buffer || count === 0) return;
  const { gl, lines } = cache;
  gl.useProgram(lines);
  setShared(gl, lines);
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  const pos = gl.getAttribLocation(lines, "a_pos");
  const col = gl.getAttribLocation(lines, "a_color");
  gl.enableVertexAttribArray(pos);
  gl.vertexAttribPointer(pos, 3, gl.FLOAT, false, 28, 0);
  gl.enableVertexAttribArray(col);
  gl.vertexAttribPointer(col, 4, gl.FLOAT, false, 28, 12);
  gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  gl.drawArrays(gl.LINES, 0, count);
  gl.disableVertexAttribArray(col);
}

function draw(): void {
  const canvas = state.canvas;
  if (!canvas) return;
  const cache = glCache();
  if (!cache) return;
  const { gl } = cache;
  hideFallback();
  gl.viewport(0, 0, canvas.width, canvas.height);
  gl.clearColor(0.035, 0.045, 0.07, 1);
  gl.clear(gl.COLOR_BUFFER_BIT);
  drawLabels();
  if (state.points.length === 0) return;
  buildBuffers(cache);
  gl.disable(gl.DEPTH_TEST);
  gl.depthMask(false);
  gl.enable(gl.BLEND);
  drawLines(cache, cache.gridBuffer, cache.gridCount);
  const head = getCubePlayhead();
  const headX = head ? head.x * 2 - 1 : -1;
  const band = head ? 1.01 * (2 / Math.max(1, head.timeBins - 1)) : 0;
  // Playhead frame + translucent plane at the bound clip's current time bin.
  const headLines = playheadLines(headX);
  gl.bindBuffer(gl.ARRAY_BUFFER, cache.headBuffer);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(headLines), gl.DYNAMIC_DRAW);
  drawLines(cache, cache.headBuffer, headLines.length / 7);
  const { points } = cache;
  gl.useProgram(points);
  setShared(gl, points);
  gl.uniform1f(gl.getUniformLocation(points, "u_head"), headX);
  gl.uniform1f(gl.getUniformLocation(points, "u_band"), band);
  gl.uniform1f(
    gl.getUniformLocation(points, "u_point"),
    Math.max(1.6, Math.min(5, canvas.clientHeight / 150)) * dpr() * 0.85,
  );
  const loc = gl.getAttribLocation(points, "a_pt");
  gl.enableVertexAttribArray(loc);
  for (const layer of listCubeLayers()) {
    if (!layer.on) continue;
    const entry = cache.layerBuffers.get(layer.id);
    if (!entry || entry.count === 0) continue;
    const color = LAYER_COLORS[layer.id] ?? [0.7, 0.7, 0.7];
    gl.uniform3f(gl.getUniformLocation(points, "u_color"), color[0], color[1], color[2]);
    gl.uniform1f(gl.getUniformLocation(points, "u_opacity"), layer.opacity);
    gl.uniform1f(gl.getUniformLocation(points, "u_gain"), layer.gain);
    // Premultiplied output: normal = over, add = additive, multiply = darken toward color.
    if (layer.blend === "add") gl.blendFunc(gl.ONE, gl.ONE);
    else if (layer.blend === "multiply") gl.blendFunc(gl.DST_COLOR, gl.ONE_MINUS_SRC_ALPHA);
    else gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.bindBuffer(gl.ARRAY_BUFFER, entry.buffer);
    gl.vertexAttribPointer(loc, 4, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.POINTS, 0, entry.count);
  }
}

function pushSeg(out: number[], a: number[], b: number[], rgba: number[]): void {
  out.push(a[0], a[1], a[2], ...rgba, b[0], b[1], b[2], ...rgba);
}

/** 3D bounding box, floor + back-wall grid, plane outlines. Interleaved xyz rgba. */
function gridLines(): number[] {
  const out: number[] = [];
  const box = [0.62, 0.7, 0.82, 0.55];
  const faint = [0.5, 0.58, 0.7, 0.16];
  for (const y of [-1, 1]) {
    for (const z of [-1, 1]) pushSeg(out, [-1, y, z], [1, y, z], box);
    for (const x of [-1, 1]) pushSeg(out, [x, y, -1], [x, y, 1], box);
  }
  for (const x of [-1, 1]) for (const z of [-1, 1]) pushSeg(out, [x, -1, z], [x, 1, z], box);
  const meta = state.meta;
  for (const t of ticks(meta?.timeBins ?? 0, tickStep(meta?.timeBins ?? 0, 50))) {
    const x = t * 2 - 1;
    pushSeg(out, [x, -1, -1], [x, -1, 1], faint);
    pushSeg(out, [x, -1, 1], [x, 1, 1], faint);
  }
  for (const f of ticks(meta?.freqBins ?? 0, tickStep(meta?.freqBins ?? 0, 20))) {
    const z = f * 2 - 1;
    pushSeg(out, [-1, -1, z], [1, -1, z], faint);
    pushSeg(out, [-1, -1, z], [-1, 1, z], faint);
  }
  EXPECTED.forEach((_, index) => {
    const y = planeY(index / 3);
    pushSeg(out, [-1, y, 1], [1, y, 1], faint);
    pushSeg(out, [-1, y, -1], [-1, y, 1], faint);
  });
  return out;
}

function playheadLines(x: number): number[] {
  const out: number[] = [];
  const head = [0.16, 0.83, 1, 0.9];
  const plane = [0.16, 0.83, 1, 0.12];
  pushSeg(out, [x, -1, -1], [x, -1, 1], head);
  pushSeg(out, [x, 1, -1], [x, 1, 1], head);
  pushSeg(out, [x, -1, -1], [x, 1, -1], head);
  pushSeg(out, [x, -1, 1], [x, 1, 1], head);
  // translucent plane hatching at the playhead
  for (let i = 1; i < 12; i += 1) {
    const z = -1 + (i / 12) * 2;
    pushSeg(out, [x, -1, z], [x, 1, z], plane);
  }
  return out;
}

function tickStep(bins: number, preferred: number): number {
  if (bins <= 0) return 0;
  return bins / preferred > 9 ? preferred * 2 : preferred;
}

/** Normalized 0..1 positions for integer bin ticks. */
function ticks(bins: number, step: number): number[] {
  if (bins <= 1 || step <= 0) return [];
  const out: number[] = [];
  for (let b = 0; b <= bins - 1; b += step) out.push(b / (bins - 1));
  return out;
}

export function layerCss(id: string): string {
  const c = LAYER_COLORS[id] ?? [0.7, 0.7, 0.7];
  return `rgb(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)})`;
}

/** 2D overlay: tick labels, axis names, layer plane names. Pixel-exact with the GL projection. */
function drawLabels(): void {
  const labels = state.labels;
  if (!labels) return;
  const ctx = labels.getContext("2d");
  if (!ctx) return;
  ctx.clearRect(0, 0, labels.width, labels.height);
  const meta = state.meta;
  if (!meta || state.points.length === 0) return;
  const ratio = dpr();
  const font = Math.round(Math.max(10, Math.min(15, labels.height / ratio / 48)) * ratio);
  ctx.font = `${font}px "Segoe UI", system-ui, sans-serif`;
  ctx.textBaseline = "middle";
  const ink = "rgba(200, 212, 230, 0.92)";
  ctx.fillStyle = ink;
  const text = (value: string, p: { px: number; py: number } | null, dx = 0, dy = 0, align: CanvasTextAlign = "center") => {
    if (!p) return;
    ctx.textAlign = align;
    ctx.fillText(value, p.px + dx * ratio, p.py + dy * ratio);
  };
  const tStep = tickStep(meta.timeBins, 50);
  for (let b = 0; meta.timeBins > 1 && b <= meta.timeBins - 1; b += tStep) {
    text(String(b), project((b / (meta.timeBins - 1)) * 2 - 1, -1, -1), 0, 14);
  }
  text("time bin", project(0, -1, -1.28), 0, 26);
  const fStep = tickStep(meta.freqBins, 20);
  for (let b = 0; meta.freqBins > 1 && b <= meta.freqBins - 1; b += fStep) {
    text(String(b), project(1, -1, (b / (meta.freqBins - 1)) * 2 - 1), 14, 8, "left");
  }
  text("freq bin", project(1.3, -1, 0), 20, 18, "left");
  EXPECTED.forEach((id, index) => {
    ctx.fillStyle = layerCss(id);
    text(id, project(1, planeY(index / 3), 1), 10, 0, "left");
  });
  // Axis title sits above the back-left vertical edge, clear of the layer-matrix overlay (top-right).
  ctx.fillStyle = ink;
  text("layer (+value)", project(-1, 1, 1), 0, -14, "left");
  const head = getCubePlayhead();
  if (head) {
    ctx.fillStyle = "rgba(41, 211, 255, 0.95)";
    text(`bin ${head.bin}`, project(head.x * 2 - 1, 1, -1), 0, -12);
  }
}

/** WebGL missing or shaders failed: show the matplotlib PNG as a labeled static image. */
function showFallback(reason: string): void {
  state.glFailed = state.glFailed || reason;
  const img = document.querySelector<HTMLImageElement>("#cube-fallback");
  if (img && state.meta?.pngUrl) {
    img.src = state.meta.pngUrl;
    img.alt = `Static cube image (${reason})`;
    img.hidden = false;
  }
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  if (caption) caption.textContent = `${reason} Showing the static cube PNG.`;
}

function hideFallback(): void {
  const img = document.querySelector<HTMLImageElement>("#cube-fallback");
  if (img && !img.hidden) img.hidden = true;
}
