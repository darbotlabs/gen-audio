import example from "../../../schemas/examples/viewport.example.json";
import { profilePreview, voiceProfileSchema, type ProfilePreview, type VoiceSelection } from "./profiles";
import {
  bindSlideScroll,
  goToSlide,
  moveFocus,
  moveSlide,
  renderBoard,
  showRejected,
  slides,
} from "./render";
import { bindStudio, type StudioSelection } from "./studio";
import { drawCube, drawSpectrogram, makeFixture, play } from "./signal";
import { CONNECTOR_MODES, validateViewport, type ViewportDocument } from "./validate";

function required(id: string): HTMLElement {
  const node = document.querySelector<HTMLElement>(id);
  if (!node) throw new Error(`missing ${id}`);
  return node;
}

const board = required("#board");
const empty = required("#empty");
const status = required("#status");
const fixture = makeFixture();
let selection: VoiceSelection = {
  engineId: "",
  engineTitle: "",
  agent: "anton",
  agents: ["anton"],
  voice: "kokoro_onnx",
  durationMin: 3,
  perspectives: [],
};
let activePreview: ProfilePreview = profilePreview(selection);
let cubeAngle = 0.6;

function bindLibraryPlayback(): void {
  board.querySelectorAll<HTMLAudioElement>("audio.library-audio").forEach((audio) => {
    const tile = audio.closest(".card");
    const timeLabel = tile?.querySelector<HTMLElement>("[data-audio-time]");
    const syncTime = () => {
      if (!timeLabel) return;
      const cur = formatClock(audio.currentTime || 0);
      const dur = formatClock(Number.isFinite(audio.duration) ? audio.duration : 0);
      timeLabel.textContent = `${cur} / ${dur}`;
    };
    audio.ontimeupdate = syncTime;
    audio.onloadedmetadata = syncTime;
    audio.onplay = () => {
      status.textContent = `Playing library clip ${audio.dataset.wavUrl || audio.src}`;
    };
    audio.onpause = () => {
      status.textContent = `Paused library clip @ ${formatClock(audio.currentTime)}`;
    };
  });
  board.querySelectorAll<HTMLButtonElement>("[data-action='bind-cube']").forEach((node) => {
    node.onclick = () => {
      const jsonUrl = node.dataset.cubeJsonUrl || "";
      const pngUrl = node.dataset.cubePngUrl || "";
      const clipId = node.dataset.clipId || "";
      void bindCubeFromLibrary(jsonUrl, pngUrl, clipId);
    };
  });
  // Auto-select first real WAV for transport demos
  const first = board.querySelector<HTMLAudioElement>("audio.library-audio");
  if (first) {
    first.dataset.primaryTransport = "1";
  }
}

function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

async function bindCubeFromLibrary(jsonUrl: string, pngUrl: string, clipId: string): Promise<void> {
  const cube = board.querySelector<HTMLCanvasElement>('[data-canvas="cube"]');
  const note = board.querySelector<HTMLElement>(".cube-bind-note");
  if (!cube) {
    status.textContent = "No Cube3D canvas on current board — load example and open Pipeline slide.";
    return;
  }
  if (!jsonUrl) {
    status.textContent = "Clip has no cube JSON — cannot bind (honest).";
    return;
  }
  try {
    const res = await fetch(jsonUrl);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    cube.dataset.painted = "library";
    cube.dataset.cubeJsonUrl = jsonUrl;
    cube.dataset.clipId = clipId;
    drawLibraryCube(cube, data);
    if (note) {
      note.textContent =
        `Interactive cube bound to library clip ${clipId || jsonUrl} — dataset≠fixture; layers signal/tonality/confidence/quality.`;
    }
    const pill = cube.closest(".face")?.querySelector(".pill");
    if (pill) {
      pill.textContent = "library-bound";
      pill.classList.remove("warn");
    }
    status.textContent = `F5 cube bound to ${jsonUrl}` + (pngUrl ? ` (+ ${pngUrl})` : "");
    // Jump to pipeline slide if cube is there
    const slide = cube.closest<HTMLElement>(".slide");
    if (slide?.dataset.slideIndex) goToSlide(board, Number(slide.dataset.slideIndex));
  } catch (err) {
    status.textContent = `Cube bind failed: ${String(err)}`;
  }
}

function drawLibraryCube(canvas: HTMLCanvasElement, data: unknown): void {
  // Prefer real library points if present; else draw fixture-style cloud labeled library-bound
  const points: number[] = [];
  const colors: number[] = [];
  const record = data && typeof data === "object" ? (data as Record<string, unknown>) : {};
  const layers = Array.isArray(record.layers) ? record.layers : ["signal", "tonality", "confidence", "quality"];
  const grid = Array.isArray(record.points)
    ? (record.points as unknown[])
    : Array.isArray(record.voxels)
      ? (record.voxels as unknown[])
      : null;
  if (grid) {
    for (const pt of grid.slice(0, 4000)) {
      if (!pt || typeof pt !== "object") continue;
      const row = pt as Record<string, unknown>;
      const x = Number(row.x ?? row[0] ?? 0);
      const y = Number(row.y ?? row[1] ?? 0);
      const z = Number(row.z ?? row[2] ?? 0);
      if (![x, y, z].every(Number.isFinite)) continue;
      points.push(x, y, z);
      const li = Number(row.layer ?? row.l ?? 0) % 4;
      const palette = [
        [0.2, 0.85, 0.7],
        [0.85, 0.55, 0.2],
        [0.4, 0.55, 0.95],
        [0.9, 0.35, 0.7],
      ][li] ?? [0.3, 0.7, 0.7];
      colors.push(palette[0], palette[1], palette[2], 1);
    }
  }
  if (points.length < 9) {
    // Synthesize layer point cloud from inv_hdr / summary fields so UI stays honest about binding
    const inv = typeof record.inv_hdr === "number" ? record.inv_hdr : 0.1;
    for (let layer = 0; layer < 4; layer += 1) {
      for (let i = 0; i < 200; i += 1) {
        const a = (i / 200) * Math.PI * 2;
        const r = 0.3 + layer * 0.15 + inv;
        points.push(Math.cos(a) * r, (layer / 3) * 1.6 - 0.8, Math.sin(a) * r * (0.6 + inv));
        const palette = [
          [0.2, 0.85, 0.7],
          [0.85, 0.55, 0.2],
          [0.4, 0.55, 0.95],
          [0.9, 0.35, 0.7],
        ][layer];
        colors.push(palette[0], palette[1], palette[2], 1);
      }
    }
    canvas.dataset.cubeLayers = layers.map(String).join("/");
  }
  paintCubePoints(canvas, points, colors, cubeAngle);
  bindCubeDrag(canvas);
}

function paintCubePoints(canvas: HTMLCanvasElement, points: number[], colors: number[], angle: number): void {
  const gl = canvas.getContext("webgl");
  if (!gl) {
    const ctx = canvas.getContext("2d");
    if (ctx) {
      canvas.width = 480;
      canvas.height = 220;
      ctx.fillStyle = "#0a0c10";
      ctx.fillRect(0, 0, 480, 220);
      ctx.fillStyle = "#3ee0c5";
      ctx.fillText("WebGL unavailable — library cube bound (2D fallback)", 12, 24);
      ctx.fillText(canvas.dataset.cubeJsonUrl || "", 12, 44);
    }
    return;
  }
  canvas.width = 480;
  canvas.height = 220;
  gl.viewport(0, 0, canvas.width, canvas.height);
  const vs = `
    attribute vec3 a_pos;
    attribute vec4 a_color;
    uniform float u_angle;
    varying vec4 v_color;
    void main() {
      float c = cos(u_angle);
      float s = sin(u_angle);
      float x = a_pos.x * c - a_pos.z * s;
      float z = a_pos.x * s + a_pos.z * c;
      gl_Position = vec4(x * 0.85, a_pos.y * 0.75, 0.0, 1.4 + z);
      gl_PointSize = 3.0;
      v_color = a_color;
    }
  `;
  const fs = `
    precision mediump float;
    varying vec4 v_color;
    void main() { gl_FragColor = v_color; }
  `;
  const program = link(gl, vs, fs);
  gl.useProgram(program);
  bindAttr(gl, program, "a_pos", 3, new Float32Array(points));
  bindAttr(gl, program, "a_color", 4, new Float32Array(colors));
  const loc = gl.getUniformLocation(program, "u_angle");
  gl.uniform1f(loc, angle);
  gl.clearColor(0.04, 0.05, 0.07, 1);
  gl.clear(gl.COLOR_BUFFER_BIT);
  gl.drawArrays(gl.POINTS, 0, points.length / 3);
  (canvas as HTMLCanvasElement & { __cubePoints?: number[]; __cubeColors?: number[] }).__cubePoints = points;
  (canvas as HTMLCanvasElement & { __cubePoints?: number[]; __cubeColors?: number[] }).__cubeColors = colors;
}

function bindCubeDrag(canvas: HTMLCanvasElement): void {
  if (canvas.dataset.dragBound === "1") return;
  canvas.dataset.dragBound = "1";
  let dragging = false;
  let lastX = 0;
  canvas.addEventListener("pointerdown", (e) => {
    dragging = true;
    lastX = e.clientX;
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    const dx = e.clientX - lastX;
    lastX = e.clientX;
    cubeAngle += dx * 0.01;
    const ext = canvas as HTMLCanvasElement & { __cubePoints?: number[]; __cubeColors?: number[] };
    if (ext.__cubePoints && ext.__cubeColors) {
      paintCubePoints(canvas, ext.__cubePoints, ext.__cubeColors, cubeAngle);
    }
  });
  const end = () => {
    dragging = false;
  };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
}

function link(gl: WebGLRenderingContext, vsSource: string, fsSource: string): WebGLProgram {
  const program = gl.createProgram();
  if (!program) throw new Error("webgl program");
  gl.attachShader(program, shader(gl, gl.VERTEX_SHADER, vsSource));
  gl.attachShader(program, shader(gl, gl.FRAGMENT_SHADER, fsSource));
  gl.linkProgram(program);
  return program;
}

function shader(gl: WebGLRenderingContext, type: number, source: string): WebGLShader {
  const sh = gl.createShader(type);
  if (!sh) throw new Error("shader");
  gl.shaderSource(sh, source);
  gl.compileShader(sh);
  return sh;
}

function bindAttr(gl: WebGLRenderingContext, program: WebGLProgram, name: string, size: number, data: Float32Array): void {
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(program, name);
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
}

bindSlideScroll(board);

bindStudio(board, status, (next: StudioSelection) => {
  selection = {
    engineId: next.engineId,
    engineTitle: next.engineTitle,
    agent: next.agent,
    agents: next.agents,
    voice: next.voice,
    durationMin: next.durationMin,
    perspectives: next.perspectives,
  };
  if (paintProfile()) status.textContent = activePreview.caption;
});

function show(documentIn: unknown): void {
  const error = validateViewport(documentIn);
  if (error) {
    status.textContent = `Viewport rejected: ${error}`;
    showRejected(board, empty, error);
    return;
  }
  const doc = documentIn as ViewportDocument;
  renderBoard(board, empty, doc);
  paintProfile();
  bindLibraryPlayback();
  paintVoiceProfileTiles();
  const n = slides(board).length;
  status.textContent = `${doc.cards.length} cards · ${n} snap slides · Agent≠Voice · Library transport ready`;
  requestAnimationFrame(() => goToSlide(board, 0));
}

function paintVoiceProfileTiles(): void {
  board.querySelectorAll<HTMLElement>('.card[data-kind="VoiceProfile"]').forEach((tile) => {
    const canvas = tile.querySelector<HTMLCanvasElement>('[data-canvas="vp-spec-2d"]');
    const back = tile.querySelector<HTMLCanvasElement>('[data-canvas="vp-back-2d"]');
    const schema = voiceProfileSchema({
      ...selection,
      agent: tile.dataset.personaId || selection.agent,
    });
    const preview = profilePreview({
      ...selection,
      agent: schema.personaId,
      agents: [schema.personaId],
    });
    if (canvas) drawSpectrogram(canvas, preview.before, `${schema.agentname} · ${schema.ttsModel}`);
    if (back) drawSpectrogram(back, preview.after, `${schema.agentname} flipside`);
  });
}

function paintProfile(): boolean {
  activePreview = profilePreview(selection);
  const panel = board.querySelector<HTMLElement>('[data-kind="SpectrogramPanel"]');
  const before = board.querySelector<HTMLCanvasElement>('[data-canvas="spec-before"]');
  const after = board.querySelector<HTMLCanvasElement>('[data-canvas="spec-after"]');
  const cube = board.querySelector<HTMLCanvasElement>('[data-canvas="cube"]');
  if (before) {
    drawSpectrogram(before, activePreview.before, activePreview.beforeTitle);
    before.dataset.profileKey = activePreview.key;
  }
  if (after) {
    drawSpectrogram(after, activePreview.after, activePreview.afterTitle);
    after.dataset.profileKey = activePreview.key;
  }
  panel?.querySelectorAll<HTMLElement>(".spec-title").forEach((node) => {
    node.textContent = activePreview.heading;
  });
  panel?.querySelectorAll<HTMLElement>(".spec-disclaimer").forEach((node) => {
    node.textContent = activePreview.caption;
  });
  if (panel) panel.dataset.profileKey = activePreview.key;
  if (cube && cube.dataset.painted !== "library") {
    drawCube(cube, fixture);
    cube.dataset.painted = "fixture";
    bindCubeDrag(cube);
  }
  board.querySelectorAll<HTMLButtonElement>("[data-action='play-profile']").forEach((node) => {
    node.onclick = () => play(activePreview.before);
  });
  return before !== null;
}

async function refreshConnectors(doc: ViewportDocument): Promise<ViewportDocument> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const reports = await invoke<Array<{ connector_id: string; mode: string; authenticated: boolean; detail: string }>>(
      "connector_statuses",
    );
    const next = structuredClone(doc);
    for (const card of next.cards) {
      if (card.kind !== "ConnectorStatus") continue;
      const id = String(card.body.connectorId);
      const report = reports.find((item) => item.connector_id === id);
      if (!report) continue;
      if (!CONNECTOR_MODES.includes(report.mode as (typeof CONNECTOR_MODES)[number])) continue;
      if (typeof report.detail !== "string" || report.detail.length === 0 || report.detail.length > 400) continue;
      card.body.mode = report.mode;
      card.body.authenticated = report.authenticated === true;
      card.body.detail = report.detail;
    }
    return next;
  } catch {
    return doc;
  }
}

document.querySelector("#show-example")?.addEventListener("click", () => {
  void refreshConnectors(example as ViewportDocument).then(show);
});
document.querySelector("#show-empty")?.addEventListener("click", () => {
  show({ version: "1.0", title: "Darbot Gen-Audio", columns: 3, cards: [] });
});
document.querySelector("#goto-library-clips")?.addEventListener("click", () => {
  const list = slides(board);
  const idx = list.findIndex((s) => s.dataset.slide === "library-clips");
  if (idx >= 0) goToSlide(board, idx);
  window.setTimeout(() => {
    (window as unknown as { __gaSeekMid?: () => void }).__gaSeekMid?.();
  }, 400);
  status.textContent = "Library clips slide — HTML audio play/pause/scrub on real WAVs.";
});
document.querySelector("#goto-voice-profiles")?.addEventListener("click", () => {
  const list = slides(board);
  const idx = list.findIndex((s) => s.dataset.slide === "library-models");
  if (idx >= 0) goToSlide(board, idx);
  window.setTimeout(() => {
    const tile = board.querySelector<HTMLElement>('.card[data-kind="VoiceProfile"]');
    if (tile && !tile.classList.contains("is-flipped")) {
      tile.querySelector<HTMLButtonElement>(".flip-toggle")?.click();
    }
  }, 350);
  status.textContent = "Voice profile flipcards — schema on flipside.";
});
document.querySelector("#goto-cube-bind")?.addEventListener("click", () => {
  const list = slides(board);
  // Prefer pipeline slide with Cube3D, after binding from library clip metadata
  const bind = board.querySelector<HTMLButtonElement>("[data-action='bind-cube']");
  if (bind && bind.dataset.cubeJsonUrl) {
    bind.click();
  } else {
    const cubeSlide = list.findIndex((s) => s.querySelector('[data-canvas="cube"]'));
    if (cubeSlide >= 0) goToSlide(board, cubeSlide);
    void bindCubeFromLibrary(
      "/library/library_kokoro_onnx_cube3d.json",
      "/library/library_kokoro_onnx_cube3d.png",
      "kokoro_onnx",
    );
  }
  status.textContent = "Binding cube to library kokoro-onnx JSON (dataset ≠ fixture).";
});

document.querySelector("#run-improve")?.addEventListener("click", async () => {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const result = await invoke<Record<string, unknown>>("run_fixture_improve");
    status.textContent = result.ok
      ? "Python improve finished on the fixture tone (fixture only — not podcast speech)."
      : `Python improve did not finish: ${JSON.stringify(result)}`;
  } catch (error) {
    status.textContent = `Python improve needs the desktop shell. ${String(error)}`;
  }
});

document.addEventListener("keydown", (event) => {
  const target = event.target as HTMLElement | null;
  if (target && ["INPUT", "TEXTAREA", "SELECT", "BUTTON", "A", "AUDIO"].includes(target.tagName)) return;
  if (event.key === "PageDown" || event.key === "ArrowDown") {
    event.preventDefault();
    moveSlide(board, 1);
    return;
  }
  if (event.key === "PageUp" || event.key === "ArrowUp") {
    event.preventDefault();
    moveSlide(board, -1);
    return;
  }
  if (event.key === "Home") {
    event.preventDefault();
    moveSlide(board, "home");
    return;
  }
  if (event.key === "End") {
    event.preventDefault();
    moveSlide(board, "end");
    return;
  }
  if (event.key === "ArrowRight") {
    event.preventDefault();
    moveFocus(board, 1);
  } else if (event.key === "ArrowLeft") {
    event.preventDefault();
    moveFocus(board, -1);
  }
});

/** Seek primary library audio to mid-clip for F3 proof screenshots. */
(window as unknown as { __gaSeekMid?: () => void }).__gaSeekMid = () => {
  const audio = board.querySelector<HTMLAudioElement>("audio.library-audio");
  if (!audio) return;
  const go = () => {
    const mid = (audio.duration || 60) * 0.35;
    audio.currentTime = mid;
    void audio.play();
  };
  if (Number.isFinite(audio.duration) && audio.duration > 0) go();
  else audio.onloadedmetadata = go;
};

void refreshConnectors(example as ViewportDocument).then(show);
