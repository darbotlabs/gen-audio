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

const DEFAULT_COLORS: Record<string, [number, number, number]> = {
  signal: [0.29, 0.64, 1],
  tonality: [0.24, 0.8, 0.43],
  confidence: [0.94, 0.76, 0.29],
  quality: [1, 0.35, 0.82],
};

const EXPECTED = ["signal", "tonality", "confidence", "quality"] as const;

const state = {
  points: [] as CubePoint[],
  layerIds: [] as string[],
  layers: new Map<string, LayerParams>(),
  yaw: 0.7,
  pitch: 0.45,
  zoom: 1,
  scrub: 1,
  canvas: null as HTMLCanvasElement | null,
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

export function bindCube(canvas: HTMLCanvasElement): void {
  state.canvas = canvas;
  canvas.width = 640;
  canvas.height = 360;
  let drag: { x: number; y: number; yaw: number; pitch: number } | null = null;
  canvas.addEventListener("pointerdown", (event) => {
    drag = { x: event.clientX, y: event.clientY, yaw: state.yaw, pitch: state.pitch };
    canvas.setPointerCapture(event.pointerId);
  });
  canvas.addEventListener("pointermove", (event) => {
    if (!drag) return;
    state.yaw = drag.yaw + (event.clientX - drag.x) * 0.01;
    state.pitch = Math.max(-1.2, Math.min(1.2, drag.pitch + (event.clientY - drag.y) * 0.01));
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
    draw();
    return "Cube JSON has no signal/tonality/confidence/quality preview.";
  }
  state.points = points.filter((point) => names.includes(point.layer));
  state.layerIds = [...names];
  state.layers.clear();
  names.forEach((id, index) => ensureLayer(id, index));
  state.scrub = 1;
  state.sourceUrl = url;
  draw();
  rebuildLayerMatrixUi();
  const inv = typeof data.inv_hdr === "number" ? data.inv_hdr.toFixed(3) : "?";
  return `${data.engine ?? "library"} cube · ${state.points.length} preview points · inv-HDR ${inv} · layers ${names.join(" / ")}. Drag to rotate, wheel to zoom, scrub the shared clock. Not a mastering grade.`;
}

export function clearCube(message: string): void {
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

function draw(): void {
  const canvas = state.canvas;
  if (!canvas) return;
  const gl = canvas.getContext("webgl");
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
  const ordered = listCubeLayers().filter((layer) => layer.on);
  // Bucket points by rounded t/f/z so layer matrix composites at the same bitdot.
  type Key = string;
  const buckets = new Map<Key, { x: number; y: number; z: number; rgba: [number, number, number, number] }>();
  for (const layer of ordered) {
    const color = DEFAULT_COLORS[layer.id] ?? [0.7, 0.7, 0.7];
    for (const point of state.points) {
      if (point.layer !== layer.id) continue;
      // Live time-bin: keep a trailing window ending at the shared playhead.
      const window = 0.08;
      if (point.t > state.scrub + 0.0001 || point.t < state.scrub - window) continue;
      const gainV = Math.max(0, Math.min(1, point.v * layer.gain));
      const src: [number, number, number, number] = [
        color[0],
        color[1],
        color[2],
        (0.35 + gainV * 0.65) * layer.opacity,
      ];
      const x = point.t * 2 - 1;
      const y = point.f * 2 - 1;
      const z = (point.z * 2 - 1) * 0.85 + (point.v - 0.5) * 0.15;
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
  gl.clearColor(0.04, 0.05, 0.08, 1);
  gl.clear(gl.COLOR_BUFFER_BIT);
  if (positions.length === 0) return;
  const program = programFor(gl);
  gl.useProgram(program);
  bindAttr(gl, program, "a_pos", 3, new Float32Array(positions));
  bindAttr(gl, program, "a_color", 4, new Float32Array(colors));
  gl.uniform1f(gl.getUniformLocation(program, "u_yaw"), state.yaw);
  gl.uniform1f(gl.getUniformLocation(program, "u_pitch"), state.pitch);
  gl.uniform1f(gl.getUniformLocation(program, "u_zoom"), state.zoom);
  gl.drawArrays(gl.POINTS, 0, positions.length / 3);
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
    varying vec4 v_color;
    void main() {
      float cy = cos(u_yaw);
      float sy = sin(u_yaw);
      float cx = cos(u_pitch);
      float sx = sin(u_pitch);
      float x1 = a_pos.x * cy - a_pos.z * sy;
      float z1 = a_pos.x * sy + a_pos.z * cy;
      float y2 = a_pos.y * cx - z1 * sx;
      float z2 = a_pos.y * sx + z1 * cx;
      gl_Position = vec4(x1 * u_zoom * 0.82, y2 * u_zoom * 0.82, 0.0, 1.6 + z2);
      gl_PointSize = 3.5;
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

function bindAttr(gl: WebGLRenderingContext, program: WebGLProgram, name: string, size: number, data: Float32Array): void {
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, data, gl.DYNAMIC_DRAW);
  const loc = gl.getAttribLocation(program, name);
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
}
