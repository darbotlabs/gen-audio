/** Interactive library cube. Points come from cube JSON. No stand-in cloud. */

interface CubePoint {
  t: number;
  f: number;
  z: number;
  v: number;
  layer: string;
}

const LAYER_COLOR: Record<string, [number, number, number]> = {
  signal: [0.29, 0.64, 1],
  tonality: [0.24, 0.8, 0.43],
  confidence: [0.94, 0.76, 0.29],
  quality: [1, 0.35, 0.82],
};

const state = {
  points: [] as CubePoint[],
  yaw: 0.7,
  pitch: 0.45,
  zoom: 1,
  scrub: 1,
  layers: new Set(["signal", "tonality", "confidence", "quality"]),
  canvas: null as HTMLCanvasElement | null,
};

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

export function setCubeScrub(fraction: number): void {
  state.scrub = Math.max(0, Math.min(1, fraction));
  draw();
}

export function setCubeLayer(layer: string, on: boolean): void {
  if (on) state.layers.add(layer);
  else state.layers.delete(layer);
  draw();
}

export async function loadCube(url: string): Promise<string> {
  const response = await fetch(url);
  if (!response.ok) return `Cube JSON did not load (${response.status}).`;
  const data = (await response.json()) as { points_preview?: CubePoint[]; engine?: string; inv_hdr?: number; layers?: Record<string, unknown> };
  const points = Array.isArray(data.points_preview) ? data.points_preview : [];
  const names = data.layers ? Object.keys(data.layers) : [];
  const expected = ["signal", "tonality", "confidence", "quality"];
  const missing = expected.filter((name) => !names.includes(name));
  if (points.length === 0 || missing.length > 0) {
    state.points = [];
    draw();
    return "Cube JSON has no signal/tonality/confidence/quality preview.";
  }
  state.points = points.filter((point) => expected.includes(point.layer));
  state.scrub = 1;
  draw();
  const inv = typeof data.inv_hdr === "number" ? data.inv_hdr.toFixed(3) : "?";
  return `${data.engine ?? "library"} cube · ${state.points.length} preview points · inv-HDR ${inv} · layers signal / tonality / confidence / quality. Drag to rotate, wheel to zoom, scrub the time span. Not a mastering grade.`;
}

export function clearCube(message: string): void {
  state.points = [];
  draw();
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  if (caption) caption.textContent = message;
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
  const visible = state.points.filter((point) => state.layers.has(point.layer) && point.t <= state.scrub + 0.0001);
  const positions: number[] = [];
  const colors: number[] = [];
  for (const point of visible) {
    const color = LAYER_COLOR[point.layer] ?? [0.7, 0.7, 0.7];
    const x = point.t * 2 - 1;
    const y = point.f * 2 - 1;
    const z = (point.z * 2 - 1) * 0.85 + (point.v - 0.5) * 0.15;
    positions.push(x, y, z);
    colors.push(color[0], color[1], color[2], 0.45 + point.v * 0.55);
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
