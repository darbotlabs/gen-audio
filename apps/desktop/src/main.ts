import releaseDoc from "../../../schemas/examples/viewport.release.json";
import {
  bindCompareCube,
  bindCube,
  boundCubeUrl,
  clearCube,
  cubeNameFromUrl,
  exitCompare,
  getCubeMeta,
  isCompareOn,
  loadCompareCube,
  loadCube,
  onCubeClock,
  rebuildLayerMatrixUi,
  setCubeScrub,
  unbindCube,
} from "./cubeview";
import { isClipPlaying, seekActiveFraction, seekClipFraction, setCubeClockClip } from "./playback";
import { applyClipNames, harvestNames } from "./library-meta";
import { bindFloatingPlayback, pauseClip, playClip, releaseAllSeekBlobs, releaseDetachedTransports, seekClipOutcome, setUserPlayReporter } from "./playback";
import { controlPlayOrigin, McpFailureCounter, postMcp, seekReportControl, userPlayControl } from "./play-control";
import { fixturesRequested, selectViewport } from "./viewport-source";
import { loadLibraryCatalog, mediaUrl, sourceSha256, type LibraryCatalog } from "./library-assets";
import { decorateLibraryTiles } from "./livestrip";
import { profilePreview, type ProfilePreview, type VoiceSelection } from "./profiles";
import {
  announceCopy,
  applyCardOp,
  applyProfileUpdate,
  fillCubeGlyph,
  fillModelCubes,
  bindSlideScroll,
  goToSlide,
  goToSlideId,
  isSnapping,
  moveFocus,
  moveSlide,
  renderBoard,
  showRejected,
  slides,
} from "./render";
import { slideKey } from "./snap";
import { applySidepane, bindStudio, promptNote, readSelection, type StudioSelection } from "./studio";
import { drawCube, drawSpectrogram, makeFixture, play } from "./signal";
import { voiceById } from "./catalog";
import { CUBE_CONTROL_OP, createCubeModeController, type CubeCompareState, type ToolReply } from "./cube-mode";
import { CONNECTOR_MODES, validateViewport, type ViewportDocument } from "./validate";
import { fixturesFlag, installFixtureHooks } from "./env";

function required(id: string): HTMLElement {
  const node = document.querySelector<HTMLElement>(id);
  if (!node) throw new Error(`missing ${id}`);
  return node;
}

/**
 * Release builds boot the real Library deck (viewport.release.json) and serve
 * an assets.json without the dev fixtures (spec-fixture, cube-fixture,
 * bench-ref). VITE_GEN_AUDIO_FIXTURES=1 (dev/test only) loads the example
 * deck and the dev assets instead; both stay out of the release bundle's
 * main chunk (dynamic import).
 */
const fixtureFlag: string | undefined = fixturesFlag();
const loadShippedDocument: () => Promise<ViewportDocument> = selectViewport(
  fixtureFlag,
  async () => releaseDoc as ViewportDocument,
  async () => (await import("../../../schemas/examples/viewport.example.json")).default as ViewportDocument,
);
const loadDevAssets = async (): Promise<unknown[]> =>
  fixturesRequested(fixtureFlag) ? (await import("../../../schemas/asset-object/fixtures/assets.dev.json")).default.assets : [];

const board = required("#board");
const empty = required("#empty");
const status = required("#status");
const fixture = makeFixture();
let selection: VoiceSelection = {
  engineId: "",
  engineTitle: "",
  agents: ["alice"],
  voice: "kokoro_onnx",
  durationMin: 3,
};
let activePreview: ProfilePreview = profilePreview(selection);
let controlCursor = 0;
const seenControl = new Set<number>();
/** Resolves once the first board (with the Cube stage) has rendered; bus events can arrive before that. */
let markBoardReady: () => void = () => {};
const boardReady = new Promise<void>((resolve) => {
  markBoardReady = resolve;
});

bindSlideScroll(board);

bindStudio(board, status, (next: StudioSelection) => {
  selection = next;
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
  // D: tiles this render removed give back their cached seek blob URLs.
  releaseDetachedTransports();
  paintProfile();
  bindCubeCanvas();
  bindRename();
  void harvestLibrary();
  bindFloatingPlayback();
  void loadLibraryCatalog(loadDevAssets()).then((catalog) => {
    libraryCatalog = catalog;
    decorateLibraryTiles(board, catalog, { onCopy: announceCopy });
    fillModelCubes(board, catalog, { openCube: (url, source) => void openCube(url, source), selectTile });
    syncCubeChrome();
  });
  const n = slides(board).length;
  status.textContent = `${doc.cards.length} cards · ${n} snap slides · spectrogram follows side pane (preview, not speech)`;
  requestAnimationFrame(() => goToSlide(board, 0));
  markBoardReady();
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
  if (cube && cube.dataset.painted !== "fixture") {
    drawCube(cube, fixture);
    cube.dataset.painted = "fixture";
  }
  board.querySelectorAll<HTMLButtonElement>("[data-action='play-profile']").forEach((node) => {
    node.onclick = () => play(activePreview.before);
  });
  return before !== null;
}

/** Default Cube-tab binding: the misaki\u2192kokoro Inverse-HDR cube (real WAV + cube JSON). */
const DEFAULT_CUBE_CLIP = "lib-misaki-kokoro";
let cubeClipId = "";
let cubeBindSeq = 0;
let libraryCatalog: LibraryCatalog | null = null;

function clipUid(clipId: string): string | null {
  return libraryCatalog?.clipForTile(clipId)?.uid ?? null;
}

function cubeSources(): Array<{ clipId: string; url: string; label: string }> {
  return Array.from(board.querySelectorAll<HTMLElement>(".library-tile[data-cube-json]"))
    .filter((tile) => Boolean(tile.dataset.cubeJson && tile.dataset.id))
    .map((tile) => ({
      clipId: tile.dataset.id as string,
      url: tile.dataset.cubeJson as string,
      label: `${tile.querySelector("h2")?.textContent?.trim() || tile.dataset.id} \u00b7 ${cubeNameFromUrl(tile.dataset.cubeJson as string)}`,
    }));
}

function syncCubeChrome(): void {
  const meta = getCubeMeta();
  const title = document.querySelector<HTMLElement>("#cube-title");
  if (title) {
    title.textContent = meta ? meta.title : "Inverse-HDR cube \u2014 nothing bound";
  }
  const glyphSlot = document.querySelector<HTMLElement>("#cube-glyph");
  if (glyphSlot) {
    const cube = meta && libraryCatalog ? libraryCatalog.cubeForUrl(meta.url) : null;
    fillCubeGlyph(glyphSlot, cube?.uid ?? null, announceCopy);
  }
  const select = document.querySelector<HTMLSelectElement>("#cube-source");
  if (select && cubeClipId) select.value = cubeClipId;
  const compareToggle = document.querySelector<HTMLButtonElement>("#cube-compare-toggle");
  if (compareToggle) {
    compareToggle.setAttribute("aria-pressed", cubeMode.state().cube_mode === "compare" ? "true" : "false");
    const available = Boolean(compareCubeFor(cubeClipId));
    compareToggle.disabled = !isCompareOn() && !available;
    compareToggle.title = available || isCompareOn()
      ? "Library formulas (rev 3) beside pipeline formulas (rev 2, PR #4) on the same WAV, one playback slice"
      : "No comparison cube for this clip in assets.json";
  }
  const play = document.querySelector<HTMLButtonElement>("#cube-play");
  if (play) {
    const tile = cubeClipId ? board.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(cubeClipId)}"]`) : null;
    play.disabled = !tile?.dataset.wavUrl;
    play.textContent = cubeClipId && isClipPlaying(cubeClipId) ? "Pause" : "Play";
  }
}

/** When the Cube tab opens with nothing bound, bind the default library cube. */
function ensureDefaultCube(): void {
  if (boundCubeUrl()) return;
  const sources = cubeSources();
  const preferred = sources.find((item) => item.clipId === DEFAULT_CUBE_CLIP) ?? sources[0];
  if (!preferred) {
    clearCube("No library clip has cube JSON yet. Nothing is drawn.");
    return;
  }
  void bindCubeSource(preferred.clipId, preferred.url, "default");
}

/** Bind a clip's Library cube to the Cube tab. False when a newer binding won. Never touches the Cube mode. */
async function loadBoundCube(clipId: string, url: string, source: string): Promise<boolean> {
  const seq = ++cubeBindSeq;
  cubeClipId = clipId;
  setCubeClockClip(clipId, clipUid(clipId));
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  const message = await loadCube(url);
  if (seq !== cubeBindSeq) return false; // a newer binding won
  if (caption) caption.textContent = source === "default" ? message : `${source}: ${message}`;
  syncCubeChrome();
  return true;
}

async function bindCubeSource(clipId: string, url: string, source: string): Promise<void> {
  if (await loadBoundCube(clipId, url, source)) await followCubeMode(clipId);
}

/** Loading a cube drops the second pane. If Compare is on, follow the new clip through ui_cube (or leave Compare and say why). */
async function followCubeMode(clipId: string): Promise<void> {
  if (!clipId) {
    if (cubeMode.state().cube_mode === "compare") reportCubeMode(await cubeMode.request("single"));
    return;
  }
  const result = await cubeMode.rebound(clipId);
  if (result) reportCubeMode(result);
}

/** The bound clip's comparison cube envelope (release assets.json only; no stand-in). */
function compareCubeFor(clipId: string) {
  const uid = clipId ? clipUid(clipId) : null;
  return uid && libraryCatalog ? libraryCatalog.compareCubesFor(uid)[0] ?? null : null;
}

/**
 * Compare mode: the bound clip's Library cube beside the same WAV's
 * pipeline_r2 cube. Both slices follow the one cube clock (setCubeScrub),
 * which the bound clip's audio drives, so Play/Pause/Seek/scrub move both.
 */
async function enterCompare(pair: CubeCompareState): Promise<{ ok: true } | { ok: false; reason: string }> {
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  const meta = getCubeMeta();
  const compare = compareCubeFor(cubeClipId);
  const url = compare ? mediaUrl(compare, "cube_json") : null;
  const bound = meta && libraryCatalog ? libraryCatalog.cubeForUrl(meta.url) : null;
  let refusal = "";
  if (!meta) refusal = "Bind a library cube before comparing.";
  else if (!compare || !url) refusal = `No comparison cube for ${cubeClipId || "this cube"} in assets.json. Nothing is drawn in its place.`;
  else if (compare.uid !== pair.right_cube_uid || bound?.uid !== pair.left_cube_uid) {
    refusal = `ui_cube named ${pair.left_cube_uid} | ${pair.right_cube_uid}, but this window has ${bound?.uid ?? "no catalog cube"} | ${compare.uid}. Not drawn.`;
  }
  if (refusal || !compare || !url || !meta) {
    exitCompare();
    if (caption) caption.textContent = refusal;
    syncCubeChrome();
    return { ok: false, reason: refusal };
  }
  const seq = cubeBindSeq;
  const result = await loadCompareCube(url, {
    primary: sourceSha256(bound),
    compare: sourceSha256(compare),
  });
  if (seq !== cubeBindSeq) return { ok: false, reason: "The bound cube changed while the comparison cube loaded." };
  if (caption) caption.textContent = result.ok ? result.message : result.reason;
  status.textContent = result.ok ? `Compare on: one slice follows ${cubeClipId}` : result.reason;
  if (result.ok) {
    // Two half-width cubes: fold the layer matrix away (the Layers button reopens it).
    const matrix = document.querySelector("#cube-layer-matrix");
    const layersToggle = document.querySelector<HTMLButtonElement>("#cube-matrix-toggle");
    if (matrix && !matrix.classList.contains("is-collapsed")) {
      matrix.classList.add("is-collapsed");
      if (layersToggle) {
        layersToggle.textContent = "Layers \u25b8";
        layersToggle.setAttribute("aria-expanded", "false");
      }
    }
  }
  syncCubeChrome();
  return result.ok ? { ok: true } : { ok: false, reason: result.reason };
}

/**
 * Cube tab mode (rule 5). The MCP server owns it (ui_cube / viewport_get);
 * the Compare button posts ui_cube like any agent, and the server's answer,
 * or its `cube` bus event, is applied through the controller's one setter.
 */
const cubeMode = createCubeModeController(
  {
    async enter(pair) {
      await boardReady;
      libraryCatalog ??= await loadLibraryCatalog(loadDevAssets());
      goToSlideId(board, "spatial");
      if (cubeClipId !== pair.tileId || !boundCubeUrl()) {
        const source = cubeSources().find((item) => item.clipId === pair.tileId);
        if (!source) return { ok: false, reason: `No library tile ${pair.tileId} with cube JSON on this board.` };
        if (!(await loadBoundCube(pair.tileId, source.url, "ui_cube"))) {
          return { ok: false, reason: "A newer cube binding won while Compare loaded." };
        }
      }
      return enterCompare(pair);
    },
    exit() {
      exitCompare();
      syncCubeChrome();
    },
  },
  async (name, args): Promise<ToolReply> => {
    const payload = await mcpCall(name, args);
    if (!payload) return null;
    const error = (payload as { error?: { message?: unknown } }).error;
    if (error) return { error: String(error.message ?? "MCP error") };
    const body = toolBody(payload);
    return body ? { body } : { error: `${name} returned no body` };
  },
);

function reportCubeMode(result: { ok: boolean; reason?: string; state: { cube_mode: string } }): void {
  syncCubeChrome();
  if (result.ok) return;
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  if (caption && result.reason) caption.textContent = result.reason;
  if (result.reason) status.textContent = `Cube mode stays ${result.state.cube_mode}: ${result.reason}`;
}

/**
 * Play origin (Optimus ruling on PR #5, C1): TS never rebinds the Cube tab or
 * the shared clock on Play. A UI Play click plays locally and posts
 * `ui_playback {action:"play", origin:"user"}` on the control bus; focus and
 * rebind belong to the Rust viewport reducer (PR #4), not to this file.
 * Autoplay and other non-user starts use `playClip(..., "auto")` and change
 * nothing but the audio.
 */
window.addEventListener("pagehide", () => releaseAllSeekBlobs());

setUserPlayReporter((clipId) => {
  const request = userPlayControl(clipId);
  void mcpCall(request.name, request.args);
});

function bindCubeCanvas(): void {
  const canvas = document.querySelector<HTMLCanvasElement>("#cube-viewport");
  if (!canvas) {
    unbindCube(); // stage gone: drop the old canvas's window/DPR listeners
    return;
  }
  if (canvas.dataset.bound === "1") return;
  canvas.dataset.bound = "1";
  bindCube(canvas, document.querySelector<HTMLCanvasElement>("#cube-labels"));
  const compareCanvas = document.querySelector<HTMLCanvasElement>("#cube-compare-viewport");
  if (compareCanvas) bindCompareCube(compareCanvas, document.querySelector<HTMLCanvasElement>("#cube-compare-labels"));
  // Same path as an agent: post ui_cube, apply what the server returns.
  document.querySelector<HTMLButtonElement>("#cube-compare-toggle")?.addEventListener("click", () => {
    const next = cubeMode.state().cube_mode === "compare" ? "single" : "compare";
    void cubeMode.request(next, cubeClipId || null).then(reportCubeMode);
  });
  rebuildLayerMatrixUi();
  const select = document.querySelector<HTMLSelectElement>("#cube-source");
  if (select) {
    select.replaceChildren();
    for (const item of cubeSources()) {
      const option = document.createElement("option");
      option.value = item.clipId;
      option.textContent = item.label;
      option.dataset.cubeJson = item.url;
      select.append(option);
    }
    select.addEventListener("change", () => {
      const option = select.selectedOptions[0];
      if (option?.dataset.cubeJson) void bindCubeSource(option.value, option.dataset.cubeJson, option.value);
    });
  }
  const layersToggle = document.querySelector<HTMLButtonElement>("#cube-matrix-toggle");
  layersToggle?.addEventListener("click", () => {
    const collapsed = document.querySelector("#cube-layer-matrix")?.classList.toggle("is-collapsed") ?? false;
    layersToggle.textContent = collapsed ? "Layers \u25b8" : "Layers \u25be";
    layersToggle.setAttribute("aria-expanded", collapsed ? "false" : "true");
  });
  document.querySelector<HTMLButtonElement>("#cube-play")?.addEventListener("click", () => {
    if (!cubeClipId) return;
    const action = isClipPlaying(cubeClipId) ? pauseClip(cubeClipId) : null;
    if (action) {
      syncCubeChrome();
      return;
    }
    const request = userPlayControl(cubeClipId);
    void mcpCall(request.name, request.args);
    void playClip(cubeClipId, "user").then((result) => {
      status.textContent = result === "playing" ? `Cube live clock follows ${cubeClipId}` : result;
      syncCubeChrome();
    });
  });
  document.querySelector("#cube-scrub")?.addEventListener("input", (event) => {
    const input = event.target as HTMLInputElement;
    const fraction = Number(input.value) / 1000;
    // ONE clock: scrubber seeks library audio AND slices cube layers.
    setCubeScrub(fraction);
    void (cubeClipId ? seekClipFraction(cubeClipId, fraction) : seekActiveFraction(fraction)).then((seeked) => {
      if (seeked !== "no player" && seeked !== "superseded") status.textContent = `Shared clock ${Math.round(fraction * 100)}% \u00b7 ${seeked}`;
    });
  });
  onCubeClock((fraction) => {
    const fp = document.querySelector<HTMLInputElement>("#fp-scrub");
    if (fp && !fp.matches(":active")) fp.value = String(Math.round(fraction * 1000));
  });
  const spatial = canvas.closest<HTMLElement>(".slide");
  if (spatial && typeof IntersectionObserver !== "undefined") {
    new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          ensureDefaultCube();
          syncCubeChrome();
        }
      },
      { root: board, threshold: 0.5 },
    ).observe(spatial);
  }
  // Re-render keeps the bound cube; first render binds the default cube.
  const already = boundCubeUrl();
  if (already) void bindCubeSource(cubeClipId || DEFAULT_CUBE_CLIP, already, "default");
  else ensureDefaultCube();
}

function bindRename(): void {
  board.querySelectorAll<HTMLButtonElement>("[data-action='rename-clip']").forEach((node) => {
    node.addEventListener("click", (event) => {
      event.stopPropagation();
      const id = node.dataset.clipId;
      const tile = node.closest<HTMLElement>(".card");
      if (!id || !tile) return;
      const semantic = tile.querySelector<HTMLInputElement>("[data-field='semantic']")?.value ?? "";
      const face = tile.querySelector<HTMLInputElement>("[data-field='face']")?.value ?? "";
      applyClipNames(tile, semantic, face);
      status.textContent = `Renamed ${id} in this window. The WAV file was not rewritten.`;
      void mcpCall("library_rename", { clipId: id, semanticName: semantic, faceName: face });
    });
  });
  board.querySelectorAll<HTMLButtonElement>("[data-action='attach-profile']").forEach((node) => {
    node.addEventListener("click", (event) => {
      event.stopPropagation();
      const id = node.dataset.clipId;
      const tile = node.closest<HTMLElement>(".card");
      if (!id || !tile) return;
      const personaId = readSelection().agents[0];
      if (!personaId) {
        status.textContent = "Add an Agent persona before attaching a clip ref.";
        return;
      }
      const semantic = tile.querySelector<HTMLInputElement>("[data-field='semantic']")?.value ?? "";
      const face = tile.querySelector<HTMLInputElement>("[data-field='face']")?.value ?? "";
      applyClipNames(tile, semantic, face);
      void mcpCall("library_harvest", { clipId: id, personaId, apply: true }).then((payload) => {
        const body = toolBody(payload);
        if (!body) {
          status.textContent = `Names stay on ${id}. MCP is not listening, so the profile ref was not written.`;
          return;
        }
        status.textContent = `Attached clip:${id} to persona ${personaId}. Audio was not decoded. synthesizedSpeech is false.`;
        // Second ruling 6: attach updates the profile tile and announces it; it does not flip.
        if (body.profile && typeof body.profile === "object") {
          applyProfileUpdate(board, body.profile as Record<string, unknown>);
        }
      });
    });
  });
}

async function harvestLibrary(): Promise<void> {
  const tiles = board.querySelectorAll<HTMLElement>(".library-tile");
  for (const tile of Array.from(tiles)) {
    const wav = tile.dataset.wavUrl;
    if (!wav) continue;
    const harvested = await harvestNames(wav, tile.dataset.sidecarUrl || undefined);
    const semantic = tile.querySelector<HTMLInputElement>("[data-field='semantic']");
    const face = tile.querySelector<HTMLInputElement>("[data-field='face']");
    if (semantic && !semantic.value) semantic.value = harvested.semantic;
    if (face && !face.value) face.value = tile.dataset.faceName || harvested.face;
    const note = tile.querySelector(".harvest-note");
    if (note) {
      note.textContent = `Harvested from ${harvested.source}. Rename changes the labels in this window only.`;
    }
    if (!tile.dataset.semanticName) tile.dataset.semanticName = harvested.semantic;
  }
}

async function openCube(url: string, source: string): Promise<void> {
  goToSlideId(board, "spatial");
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  if (!url) {
    cubeClipId = "";
    setCubeClockClip(null);
    clearCube("This tile has no cube JSON. Magpie, VibeVoice, and Pocket do not get a stand-in cloud.");
    return;
  }
  const owner = board.querySelector<HTMLElement>(`.library-tile[data-cube-json="${CSS.escape(url)}"]`);
  cubeBindSeq += 1;
  cubeClipId = owner?.dataset.id ?? "";
  setCubeClockClip(cubeClipId || null, cubeClipId ? clipUid(cubeClipId) : null);
  const message = await loadCube(url);
  if (caption) caption.textContent = `${source}: ${message}`;
  syncCubeChrome();
  await followCubeMode(cubeClipId);
  // The Pipeline cube-fixture card keeps its FIXTURE mark: its canvas still paints the
  // fixture tone. Only the Cube tab stage (badge "Library ...") draws the library cube.
  status.textContent = `Cube tab bound (${source}). The Pipeline cube card stays FIXTURE.`;
}

function selectTile(id: string): void {
  const tile = board.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(id)}"]`);
  if (!tile) {
    status.textContent = `No tile ${id}.`;
    return;
  }
  board.querySelectorAll<HTMLElement>(".card.is-selected").forEach((node) => node.classList.remove("is-selected"));
  tile.classList.add("is-selected");
  const slide = tile.closest<HTMLElement>(".slide");
  if (slide?.dataset.slide) goToSlideId(board, slide.dataset.slide);
  tile.focus({ preventScroll: true });
  const cube = tile.dataset.cubeJson;
  if (cube) void openCube(cube, id);
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

document.querySelector("#show-empty")?.addEventListener("click", () => {
  show({ version: "1.0", title: "Darbot Gen-Audio", columns: 3, cards: [] });
});

document.querySelector("#generate-podcast")?.addEventListener("click", () => {
  void submitGenerate();
});

document.querySelectorAll<HTMLButtonElement>("#layer-switch [data-layer]").forEach((button) => {
  button.addEventListener("click", () => {
    const layer = button.dataset.layer;
    const target =
      layer === "models" ? "models" : layer === "clips" ? "library" : layer === "video" ? "video" : layer === "cube" ? "spatial" : "";
    if (target) goToSlideId(board, target);
    if (target === "spatial") {
      ensureDefaultCube();
      syncCubeChrome();
    }
  });
});

board.addEventListener("click", (event) => {
  const target = event.target as HTMLElement | null;
  if (!target || target.closest("button, input, audio, select, textarea, a")) return;
  const card = target.closest<HTMLElement>(".card");
  if (!card?.dataset.id) return;
  selectTile(card.dataset.id);
});

document.addEventListener("keydown", (event) => {
  const target = event.target as HTMLElement | null;
  if (target && ["INPUT", "TEXTAREA", "SELECT", "BUTTON", "A"].includes(target.tagName)) return;
  const move = slideKey(event.key);
  if (move) {
    event.preventDefault();
    // Exactly one slide per press; auto-repeat while a glide runs is dropped.
    if (!event.repeat || !isSnapping(board)) moveSlide(board, move);
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

async function submitGenerate(): Promise<void> {
  const selectionNow = readSelection();
  const voice = voiceById(selectionNow.voice);
  showRun(selectionNow.voice, voice?.synthAdapter ? "running" : "unavailable", "waiting for MCP", false);
  const recorded = await mcpCall("ui_generate", {
    agents: selectionNow.agents,
    voice: selectionNow.voice,
    durationMin: selectionNow.durationMin,
    promptNote: promptNote(),
  });
  if (!recorded) {
    showRun(selectionNow.voice, "unavailable", "MCP is not listening on 127.0.0.1:8765", false);
    return;
  }
  if (voice?.synthAdapter) {
    const synth = await mcpCall("synth", {
      output: "request.wav",
      script: "examples/podcast_script_sample.txt",
    });
    const speech = toolClaimsSpeech(synth);
    showRun(
      selectionNow.voice,
      speech ? "ok" : "refused",
      "sample script, not the pasted prompt",
      speech,
    );
    return;
  }
  showRun(selectionNow.voice, "unavailable", `No synth adapter for ${voice?.label ?? selectionNow.voice}`, false);
}

function toolBody(payload: unknown): Record<string, unknown> | null {
  if (!payload || typeof payload !== "object") return null;
  const text = (payload as { result?: { content?: Array<{ text?: string }> } }).result?.content?.[0]?.text;
  if (!text) return null;
  try {
    const body = JSON.parse(text) as unknown;
    return body && typeof body === "object" ? (body as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

function toolClaimsSpeech(payload: unknown): boolean {
  const body = toolBody(payload);
  return body?.synthesizedSpeech === true;
}

function showRun(voice: string, phase: string, detail: string, speech: boolean): void {
  const safePhase = speech && phase === "ok" ? "ok" : phase === "ok" ? "refused" : phase;
  board.querySelectorAll<HTMLElement>(`.card[data-engine-id="${CSS.escape(voice)}"]`).forEach((tile) => {
    tile.dataset.run = safePhase;
    let node = tile.querySelector<HTMLElement>(".run-state");
    if (!node) {
      node = document.createElement("p");
      node.className = "run-state";
      tile.querySelector(".face.front")?.prepend(node);
    }
    node.textContent = speech
      ? `${safePhase}: a synth result claimed speech.`
      : `${safePhase}: synthesizedSpeech is false. ${detail}`;
  });
  status.textContent = speech
    ? `${safePhase} for ${voice}.`
    : `${safePhase} for ${voice}. synthesizedSpeech is false. ${detail}`;
}

// Window->MCP failures are counted, not swallowed. This branch's viewport_get
// is the interim Cube-mode read (cube_mode / cube_compare, folded into PR #4's
// reducer later) and takes no window reports, so the counts live on
// <html data-mcp-failures> and console.warn.
const mcpFailures = new McpFailureCounter(undefined, (snapshot) => {
  document.documentElement.dataset.mcpFailures = JSON.stringify(snapshot);
});

async function mcpCall(name: string, args: Record<string, unknown>): Promise<unknown | null> {
  const payload = await postMcp(fetch, "http://127.0.0.1:8765/mcp", name, args, mcpFailures);
  if (payload !== null) noteControlSeq(payload);
  return payload;
}

function noteControlSeq(payload: unknown): void {
  if (!payload || typeof payload !== "object") return;
  const text = (payload as { result?: { content?: Array<{ text?: string }> } }).result?.content?.[0]?.text;
  if (!text) return;
  try {
    const body = JSON.parse(text) as { seq?: number; sidepane?: { seq?: number } };
    if (typeof body.seq === "number") seenControl.add(body.seq);
    if (typeof body.sidepane?.seq === "number") seenControl.add(body.sidepane.seq);
  } catch {
    /* tool text is not a control envelope */
  }
}

function applyControl(event: { seq?: number; op?: string; args?: Record<string, unknown> }): void {
  if (typeof event.seq === "number") {
    if (seenControl.has(event.seq)) return;
    seenControl.add(event.seq);
    controlCursor = Math.max(controlCursor, event.seq);
  }
  const args = event.args ?? {};
  if (event.op === "navigate" && typeof args.slide === "string") {
    // C5: MCP sends the canonical "slide:<slug>"; a bare slug is the deprecated alias.
    const slug = args.slide.startsWith("slide:") ? args.slide.slice("slide:".length) : args.slide;
    goToSlideId(board, slug);
    if (slug === "spatial") {
      ensureDefaultCube();
      syncCubeChrome();
    }
    if (typeof args.tileId === "string") selectTile(args.tileId);
  } else if (event.op === "select" && typeof args.tileId === "string") {
    selectTile(args.tileId);
  } else if (event.op === "flip" || event.op === "flipcard") {
    applyCardOp(board, event.op, args);
  } else if (event.op === "progress" && typeof args.voice === "string") {
    showRun(args.voice, String(args.phase ?? "unavailable"), String(args.detail ?? ""), args.synthesizedSpeech === true);
  } else if (event.op === "playback" && typeof args.tileId === "string") {
    const action = String(args.action ?? "");
    if (action === "play") void playClip(args.tileId, controlPlayOrigin(args)).then((result) => {
      status.textContent = result === "playing" ? `Playing ${args.tileId}` : result;
    });
    else if (action === "pause") status.textContent = pauseClip(args.tileId);
    else if (action === "seek") {
      const requested = Number(args.seconds);
      void seekClipOutcome(args.tileId, requested).then((landing) => {
        if (landing.status !== "superseded") status.textContent = landing.status;
        // Tell MCP where it landed: ui_playback answers the agent with {requested_t, landed_t, ok, reason}.
        if (typeof event.seq === "number") {
          const report = seekReportControl(event.seq, requested, landing);
          void mcpCall(report.name, report.args);
        }
      });
    }
  } else if (event.op === "sidepane") {
    applySidepane({
      agents: Array.isArray(args.agents) ? args.agents.map(String) : undefined,
      voice: typeof args.voice === "string" ? args.voice : undefined,
      durationMin: typeof args.durationMin === "number" ? args.durationMin : undefined,
    });
  } else if (event.op === "generate") {
    applySidepane({
      agents: Array.isArray(args.agents) ? args.agents.map(String) : undefined,
      voice: typeof args.voice === "string" ? args.voice : undefined,
      durationMin: typeof args.durationMin === "number" ? args.durationMin : undefined,
    });
    status.textContent = "Remote generate request recorded. synthesizedSpeech is false.";
  } else if (event.op === CUBE_CONTROL_OP) {
    void cubeMode.applyControl(args).then(reportCubeMode);
  } else if (event.op === "rename" && typeof args.clipId === "string") {
    const tile = board.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(args.clipId)}"]`);
    if (!tile) return;
    applyClipNames(
      tile,
      typeof args.semanticName === "string" ? args.semanticName : undefined,
      typeof args.faceName === "string" ? args.faceName : undefined,
    );
    const semantic = tile.querySelector<HTMLInputElement>("[data-field='semantic']");
    const face = tile.querySelector<HTMLInputElement>("[data-field='face']");
    if (semantic && typeof args.semanticName === "string") semantic.value = args.semanticName;
    if (face && typeof args.faceName === "string") face.value = args.faceName;
  }
}

function connectControl(): void {
  let source: EventSource;
  try {
    source = new EventSource(`http://127.0.0.1:8765/control/stream?after=${controlCursor}&wait=2000`);
  } catch {
    return;
  }
  source.addEventListener("control", (event) => {
    try {
      applyControl(JSON.parse((event as MessageEvent).data) as { seq?: number; op?: string; args?: Record<string, unknown> });
    } catch {
      /* ignore malformed events */
    }
  });
  source.onerror = () => {
    source.close();
    window.setTimeout(connectControl, 50);
  };
}

void loadShippedDocument().then(refreshConnectors).then(show);
// The bus ring holds 128 events; viewport_get is the Cube mode's resync.
void boardReady.then(() => cubeMode.sync()).then(reportCubeMode);

// E1 addendum: fixture controls live behind env.installFixtureHooks so Vite DCE
// drops them from release dist (viewport-source.test.ts greps for the strings).
void installFixtureHooks({
  loadShippedDocument: () => loadShippedDocument() as Promise<unknown>,
  refreshConnectors: (doc) => refreshConnectors(doc as ViewportDocument) as Promise<unknown>,
  show: (doc) => show(doc as ViewportDocument),
  status,
  setCubeScrub,
  seekBoundClip: (fraction) => (cubeClipId ? seekClipFraction(cubeClipId, fraction) : Promise.resolve("no cube clip")),
});
connectControl();
