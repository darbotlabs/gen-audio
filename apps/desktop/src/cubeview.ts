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
  order: number; // lower draws first
}

/** Legend colors match the matplotlib reference PNG: blue / green / orange / pink. */
export const LAYER_COLORS: Record<string, [number, number, number]> = {
  signal: [0.12, 0.53, 1],
  tonality: [0.16, 0.87, 0.36],
  confidence: [1, 0.75, 0.12],
  quality: [0.94, 0.27, 0.56],
};
const DEFAULT_COLORS = LAYER_COLORS;

export function layerCss(id: string): string {
  const c = LAYER_COLORS[id] ?? [0.7, 0.7, 0.7];
  return `rgb(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)})`;
}

export interface CubeMeta {
  url: string;
  name: string;
  title: string;
  invHdr: number | null;
  points: number;
  timeBins: number;
  freqBins: number;
  durationS: number | null;
}

const EXPECTED = ["signal", "tonality", "confidence", "quality"] as const;

const state = {
  points: [] as CubePoint[],
  layerIds: [] as string[],
  layers: new Map<string, LayerParams>(),
  yaw: -0.45,
  pitch: -0.55,
  zoom: 1,
  scrub: 1,
  canvas: null as HTMLCanvasElement | null,
  labels: null as HTMLCanvasElement | null,
  meta: null as CubeMeta | null,
  resize: null as ResizeObserver | null,
  sourceUrl: "",
  clockListeners: new Set<(fraction: number) => void>(),
  rafId: 0 as number,
  audioClock: null as (() => number | null) | null,
};

function ensureLayer(id: string, order: number): LayerParams {
  const existing = state.layers.get(id);
  if (existing) return existing;
  const created: LayerParams = { id, on: true, opacity: 1, gain: 1, blend: "normal", order };
  state.layers.set(id, created);
  return created;
}

/**
 * Bind the WebGL cube canvas. The canvas fills its host: a ResizeObserver
 * sizes the backing store to clientWidth/Height x devicePixelRatio, so there
 * is no fixed 640x360 buffer. An optional 2D label canvas draws ticks/axes.
 */
export function bindCube(canvas: HTMLCanvasElement, labels?: HTMLCanvasElement | null): void {
  state.canvas = canvas;
  state.labels = labels ?? null;
  const fit = () => {
    const dpr = Math.max(1, window.devicePixelRatio || 1);
    const width = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const height = Math.max(1, Math.round(canvas.clientHeight * dpr));
    for (const node of [canvas, state.labels]) {
      if (!node) continue;
      if (node.width !== width) node.width = width;
      if (node.height !== height) node.height = height;
    }
    draw();
  };
  state.resize?.disconnect();
  if (typeof ResizeObserver !== "undefined") {
    state.resize = new ResizeObserver(fit);
    state.resize.observe(canvas);
  }
  window.addEventListener("resize", fit);
  fit();
  let drag: { x: number; y: number; yaw: number; pitch: number } | null = null;
  canvas.addEventListener("pointerdown", (event) => {
    drag = { x: event.clientX, y: event.clientY, yaw: state.yaw, pitch: state.pitch };
    canvas.setPointerCapture(event.pointerId);
  });
  canvas.addEventListener("pointermove", (event) => {
    if (!drag) return;
    state.yaw = drag.yaw + (event.clientX - drag.x) * 0.01;
    state.pitch = Math.max(-1.45, Math.min(0.6, drag.pitch - (event.clientY - drag.y) * 0.01));
    draw();
  });
  const end = () => {
    drag = null;
  };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
  canvas.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      state.zoom = Math.max(0.4, Math.min(2.4, state.zoom * (event.deltaY > 0 ? 0.92 : 1.08)));
      draw();
    },
    { passive: false },
  );
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

/** ONE clock: fraction 0..1 drives cube time-slice. Optionally notify audio scrubbers. */
export function setCubeScrub(fraction: number, opts?: { silent?: boolean }): void {
  state.scrub = Math.max(0, Math.min(1, fraction));
  draw();
  if (!opts?.silent) {
    for (const listener of state.clockListeners) listener(state.scrub);
  }
  const scrub = document.querySelector<HTMLInputElement>("#cube-scrub");
  if (scrub && !scrub.matches(":active")) scrub.value = String(Math.round(state.scrub * 1000));
}

export function getCubeScrub(): number {
  return state.scrub;
}

/**
 * LIVE viewport: drive cube time-bin from audio via rAF while playing.
 * getFraction returns 0..1 from the active Library WAV, or null if idle.
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
    const scrub = document.querySelector<HTMLInputElement>("#cube-scrub");
    if (scrub && !scrub.matches(":active")) scrub.value = String(Math.round(fraction * 1000));
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
  const params = state.layers.get(layer)!;
  params.on = on;
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

export async function loadCube(url: string): Promise<string> {
  const response = await fetch(url);
  if (!response.ok) return `Cube JSON did not load (${response.status}).`;
  const data = (await response.json()) as {
    points_preview?: CubePoint[];
    engine?: string;
    inv_hdr?: number;
    title?: string;
    duration_s?: number;
    cube_shape_f_t?: number[];
    layers?: Record<string, unknown> | string[];
  };
  const points = Array.isArray(data.points_preview) ? data.points_preview : [];
  const namesFromDict = data.layers && !Array.isArray(data.layers) ? Object.keys(data.layers) : [];
  const namesFromArr = Array.isArray(data.layers) ? data.layers.map(String) : [];
  const names = (namesFromDict.length ? namesFromDict : namesFromArr).filter((name) =>
    (EXPECTED as readonly string[]).includes(name),
  );
  // Only accept the four honesty layers present in JSON — do not invent extras.
  const missing = EXPECTED.filter((name) => !names.includes(name));
  if (points.length === 0 || missing.length > 0) {
    state.points = [];
    state.layerIds = [];
    state.layers.clear();
    state.sourceUrl = "";
    state.meta = null;
    draw();
    return "Cube JSON has no signal/tonality/confidence/quality preview.";
  }
  state.points = points.filter((point) => names.includes(point.layer));
  state.layerIds = [...names];
  state.layers.clear();
  names.forEach((id, index) => ensureLayer(id, index));
  state.scrub = 1;
  state.sourceUrl = url;
  const name = cubeNameFromUrl(url);
  const shape = shapeOf(data);
  state.meta = {
    url,
    name,
    title: typeof data.title === "string" && data.title ? data.title : `Inverse-HDR bitdot cube \u2014 ${name}`,
    invHdr: typeof data.inv_hdr === "number" ? data.inv_hdr : null,
    points: state.points.length,
    freqBins: shape[0],
    timeBins: shape[1],
    durationS: typeof data.duration_s === "number" ? data.duration_s : null,
  };
  draw();
  rebuildLayerMatrixUi();
  const inv = state.meta.invHdr === null ? "?" : state.meta.invHdr.toFixed(4);
  return `${state.meta.title} \u00b7 inv_hdr ${inv} \u00b7 ${state.points.length} points \u00b7 axes time bin / freq bin / layer`;
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
  state.meta = null;
  state.points = [];
  state.layerIds = [];
  state.layers.clear();
  state.sourceUrl = "";
  draw();
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  if (caption) caption.textContent = message;
  rebuildLayerMatrixUi();
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
    name.style.setProperty("--swatch", layerCss(layer.id));

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
    up.setAttribute("aria-label", `Move ${layer.id} earlier`);
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
    down.setAttribute("aria-label", `Move ${layer.id} later`);
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

function compositeColor(
  base: [number, number, number, number],
  src: [number, number, number, number],
  blend: BlendMode,
): [number, number, number, number] {
  const a = src[3];
  if (a <= 0) return base;
  let r = src[0];
  let g = src[1];
  let b = src[2];
  if (blend === "add") {
    r = Math.min(1, base[0] + r * a);
    g = Math.min(1, base[1] + g * a);
    b = Math.min(1, base[2] + b * a);
    return [r, g, b, Math.min(1, base[3] + a)];
  }
  if (blend === "multiply") {
    r = base[0] * (1 - a) + base[0] * r * a;
    g = base[1] * (1 - a) + base[1] * g * a;
    b = base[2] * (1 - a) + base[2] * b * a;
    return [r, g, b, Math.min(1, base[3] + a * (1 - base[3]))];
  }
  // normal over
  const outA = a + base[3] * (1 - a);
  if (outA <= 0) return [0, 0, 0, 0];
  return [
    (r * a + base[0] * base[3] * (1 - a)) / outA,
    (g * a + base[1] * base[3] * (1 - a)) / outA,
    (b * a + base[2] * base[3] * (1 - a)) / outA,
    outA,
  ];
}

/** Layer planes stack vertically like the matplotlib reference: signal bottom, quality top. */
function planeY(z: number, v: number): number {
  return -0.9 + z * 1.8 + (v - 0.5) * 0.12;
}

function aspect(): number {
  const canvas = state.canvas;
  if (!canvas || canvas.height === 0) return 1;
  return canvas.width / canvas.height;
}

const CAMERA = 4.6;
const SCALE = 2.1;
const LIFT = 0.1;
/** Box half-extents: time is the long axis (like the reference), freq is depth, layers are height. */
const BOX: [number, number, number] = [1.5, 0.85, 1.05];

/** Same transform as the vertex shader, for the 2D label overlay. Returns pixel coords. */
function project(x: number, y: number, z: number): { px: number; py: number; depth: number } | null {
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
  const w = CAMERA + z2;
  if (w <= 0.05) return null;
  const a = aspect();
  let nx = (x1 * state.zoom * SCALE) / w;
  let ny = (y2 * state.zoom * SCALE) / w;
  if (a > 1) nx /= a;
  else ny *= a;
  ny += LIFT;
  return { px: ((nx + 1) / 2) * canvas.width, py: ((1 - ny) / 2) * canvas.height, depth: z2 };
}

function draw(): void {
  const canvas = state.canvas;
  if (!canvas) return;
  const gl = canvas.getContext("webgl", { preserveDrawingBuffer: true, antialias: true });
  if (!gl) {
    const ctx = canvas.getContext("2d");
    if (ctx) {
      ctx.fillStyle = "#0a0c10";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.fillStyle = "#9aa6bd";
      ctx.fillText("WebGL is unavailable in this view.", 12, 24);
    }
    return;
  }
  const live = state.scrub < 0.999;
  const ordered = listCubeLayers().filter((layer) => layer.on);
  // Bucket points by rounded t/f/z so layer matrix composites at the same bitdot.
  type Key = string;
  const buckets = new Map<Key, { x: number; y: number; z: number; rgba: [number, number, number, number] }>();
  for (const layer of ordered) {
    const color = DEFAULT_COLORS[layer.id] ?? [0.7, 0.7, 0.7];
    for (const point of state.points) {
      if (point.layer !== layer.id) continue;
      // Live time-bin: the cube fills up to the shared playhead; the trailing window glows.
      if (point.t > state.scrub + 0.0001) continue;
      const trailing = !live || point.t >= state.scrub - 0.18;
      const gainV = Math.max(0, Math.min(1, point.v * layer.gain));
      const src: [number, number, number, number] = [
        color[0],
        color[1],
        color[2],
        (0.35 + gainV * 0.65) * layer.opacity * (trailing ? 1 : 0.4),
      ];
      const x = point.t * 2 - 1;
      const y = planeY(point.z, point.v);
      const z = point.f * 2 - 1;
      const key = `${point.t.toFixed(3)}|${point.f.toFixed(3)}|${point.z.toFixed(3)}`;
      const existing = buckets.get(key);
      if (!existing) {
        buckets.set(key, { x, y, z, rgba: src });
      } else {
        existing.rgba = compositeColor(existing.rgba, src, layer.blend);
      }
    }
  }
  const positions: number[] = [];
  const colors: number[] = [];
  for (const item of buckets.values()) {
    positions.push(item.x, item.y, item.z);
    colors.push(item.rgba[0], item.rgba[1], item.rgba[2], item.rgba[3]);
  }
  gl.viewport(0, 0, canvas.width, canvas.height);
  gl.clearColor(0.035, 0.045, 0.07, 1);
  gl.clear(gl.COLOR_BUFFER_BIT);
  drawLabels();
  if (state.points.length === 0) return;
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
  const program = programFor(gl);
  gl.useProgram(program);
  const dpr = Math.max(1, window.devicePixelRatio || 1);
  gl.uniform1f(gl.getUniformLocation(program, "u_yaw"), state.yaw);
  gl.uniform1f(gl.getUniformLocation(program, "u_pitch"), state.pitch);
  gl.uniform1f(gl.getUniformLocation(program, "u_zoom"), state.zoom);
  gl.uniform1f(gl.getUniformLocation(program, "u_aspect"), aspect());
  gl.uniform3f(gl.getUniformLocation(program, "u_box"), BOX[0], BOX[1], BOX[2]);
  gl.uniform1f(gl.getUniformLocation(program, "u_camera"), CAMERA);
  gl.uniform1f(gl.getUniformLocation(program, "u_scale"), SCALE);
  gl.uniform1f(gl.getUniformLocation(program, "u_lift"), LIFT);
  gl.uniform1f(gl.getUniformLocation(program, "u_point"), Math.max(1.6, Math.min(5, canvas.height / 300)) * Math.min(dpr, 2) * 0.7);
  const grid = gridLines();
  bindAttr(gl, program, "a_pos", 3, new Float32Array(grid.positions));
  bindAttr(gl, program, "a_color", 4, new Float32Array(grid.colors));
  gl.drawArrays(gl.LINES, 0, grid.positions.length / 3);
  if (positions.length === 0) return;
  bindAttr(gl, program, "a_pos", 3, new Float32Array(positions));
  bindAttr(gl, program, "a_color", 4, new Float32Array(colors));
  gl.drawArrays(gl.POINTS, 0, positions.length / 3);
}

/** 3D bounding box, floor + back-wall grid, plane outlines, and the live playhead frame. */
function gridLines(): { positions: number[]; colors: number[] } {
  const positions: number[] = [];
  const colors: number[] = [];
  const seg = (a: number[], b: number[], rgba: number[]) => {
    positions.push(a[0], a[1], a[2], b[0], b[1], b[2]);
    colors.push(...rgba, ...rgba);
  };
  const box = [0.62, 0.7, 0.82, 0.55];
  const faint = [0.5, 0.58, 0.7, 0.16];
  for (const y of [-1, 1]) {
    for (const z of [-1, 1]) seg([-1, y, z], [1, y, z], box);
    for (const x of [-1, 1]) seg([x, y, -1], [x, y, 1], box);
  }
  for (const x of [-1, 1]) for (const z of [-1, 1]) seg([x, -1, z], [x, 1, z], box);
  const meta = state.meta;
  const tStep = tickStep(meta?.timeBins ?? 0, 50);
  const fStep = tickStep(meta?.freqBins ?? 0, 20);
  for (const t of ticks(meta?.timeBins ?? 0, tStep)) {
    const x = t * 2 - 1;
    seg([x, -1, -1], [x, -1, 1], faint);
    seg([x, -1, 1], [x, 1, 1], faint);
  }
  for (const f of ticks(meta?.freqBins ?? 0, fStep)) {
    const z = f * 2 - 1;
    seg([-1, -1, z], [1, -1, z], faint);
    seg([-1, -1, z], [-1, 1, z], faint);
  }
  for (const id of EXPECTED) {
    const y = planeY(EXPECTED.indexOf(id) / 3, 0.5);
    seg([-1, y, 1], [1, y, 1], faint);
    seg([-1, y, -1], [-1, y, 1], faint);
  }
  if (state.scrub < 0.999 && state.points.length > 0) {
    const x = state.scrub * 2 - 1;
    const head = [0.16, 0.83, 1, 0.85];
    seg([x, -1, -1], [x, -1, 1], head);
    seg([x, 1, -1], [x, 1, 1], head);
    seg([x, -1, -1], [x, 1, -1], head);
    seg([x, -1, 1], [x, 1, 1], head);
  }
  return { positions, colors };
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

/** 2D overlay: tick labels, axis names, layer plane names. Pixel-exact with the GL projection. */
function drawLabels(): void {
  const labels = state.labels;
  if (!labels) return;
  const ctx = labels.getContext("2d");
  if (!ctx) return;
  ctx.clearRect(0, 0, labels.width, labels.height);
  const meta = state.meta;
  if (!meta || state.points.length === 0) return;
  const dpr = Math.max(1, window.devicePixelRatio || 1);
  const font = Math.round(Math.max(10, Math.min(15, labels.height / dpr / 48)) * dpr);
  ctx.font = `${font}px "Segoe UI", system-ui, sans-serif`;
  ctx.fillStyle = "rgba(200, 212, 230, 0.92)";
  ctx.textBaseline = "middle";
  const text = (value: string, p: { px: number; py: number } | null, dx = 0, dy = 0, align: CanvasTextAlign = "center") => {
    if (!p) return;
    ctx.textAlign = align;
    ctx.fillText(value, p.px + dx * dpr, p.py + dy * dpr);
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
  for (const id of EXPECTED) {
    const y = planeY(EXPECTED.indexOf(id) / 3, 0.5);
    ctx.fillStyle = layerCss(id);
    text(id, project(1, y, 1), 10, 0, "left");
  }
  ctx.fillStyle = "rgba(200, 212, 230, 0.92)";
  text("layer (+value)", project(1, 1.12, 1), 10, -6, "left");
}

const programs = new WeakMap<WebGLRenderingContext, WebGLProgram>();

function programFor(gl: WebGLRenderingContext): WebGLProgram {
  const cached = programs.get(gl);
  if (cached) return cached;
  const vs = `
    attribute vec3 a_pos;
    attribute vec4 a_color;
    uniform float u_yaw;
    uniform float u_pitch;
    uniform float u_zoom;
    uniform float u_aspect;
    uniform float u_point;
    uniform vec3 u_box;
    uniform float u_camera;
    uniform float u_scale;
    uniform float u_lift;
    varying vec4 v_color;
    void main() {
      vec3 p = a_pos * u_box;
      float cy = cos(u_yaw);
      float sy = sin(u_yaw);
      float cx = cos(u_pitch);
      float sx = sin(u_pitch);
      float x1 = p.x * cy - p.z * sy;
      float z1 = p.x * sy + p.z * cy;
      float y2 = p.y * cx - z1 * sx;
      float z2 = p.y * sx + z1 * cx;
      float w = u_camera + z2;
      float px = x1 * u_zoom * u_scale;
      float py = y2 * u_zoom * u_scale;
      if (u_aspect > 1.0) px = px / u_aspect; else py = py * u_aspect;
      gl_Position = vec4(px, py + u_lift * w, 0.0, w);
      gl_PointSize = u_point;
      v_color = a_color;
    }
  `;
  const fs = `
    precision mediump float;
    varying vec4 v_color;
    void main() { gl_FragColor = v_color; }
  `;
  const program = gl.createProgram();
  if (!program) throw new Error("webgl program");
  gl.attachShader(program, compile(gl, gl.VERTEX_SHADER, vs));
  gl.attachShader(program, compile(gl, gl.FRAGMENT_SHADER, fs));
  gl.linkProgram(program);
  programs.set(gl, program);
  return program;
}

function compile(gl: WebGLRenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("shader");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  return shader;
}

const buffers = new WeakMap<WebGLRenderingContext, Map<string, WebGLBuffer>>();

function bindAttr(gl: WebGLRenderingContext, program: WebGLProgram, name: string, size: number, data: Float32Array): void {
  let cache = buffers.get(gl);
  if (!cache) {
    cache = new Map();
    buffers.set(gl, cache);
  }
  let buffer = cache.get(name) ?? null;
  if (!buffer) {
    buffer = gl.createBuffer();
    if (buffer) cache.set(name, buffer);
  }
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, data, gl.DYNAMIC_DRAW);
  const loc = gl.getAttribLocation(program, name);
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
}
