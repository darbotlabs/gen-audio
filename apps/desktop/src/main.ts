import releaseDoc from "../../../schemas/examples/viewport.release.json";
import {
  bindCube,
  boundCubeUrl,
  clearCube,
  cubeNameFromUrl,
  getCubeMeta,
  loadCube,
  onCubeClock,
  rebuildLayerMatrixUi,
  setCubeScrub,
  unbindCube,
} from "./cubeview";
import { isClipPlaying, seekActiveFraction, seekClipFraction, setCubeClockClip } from "./playback";
import { applyClipNames, harvestNames } from "./library-meta";
import { bindFloatingPlayback, pauseClip, playClip, releaseAllSeekBlobs, releaseDetachedTransports, renderTransport, seekClipOutcome, setUserPlayReporter } from "./playback";
import {
  assetResolveFailure,
  controlPlayOrigin,
  fetchLibraryBlob,
  McpFailureCounter,
  mcpOriginFromStatus,
  noteAssetResolveFailure,
  noteMcpLookupError,
  postMcp,
  seekReportControl,
  userPlayControl,
} from "./play-control";
import { selectViewport } from "./viewport-source";
import { glyphBadge } from "./glyph";
import { loadLibraryCatalog, type LibraryCatalog } from "./library-assets";
import { decorateLibraryTiles } from "./livestrip";
import { profilePreview, type ProfilePreview, type VoiceSelection } from "./profiles";
import {
  announceCopy,
  fillModelCubes,
  bindSlideScroll,
  goToSlide,
  goToSlideId,
  isSnapping,
  moveFocus,
  moveSlide,
  renderBoard,
  setCardFlip,
  showRejected,
  slides,
} from "./render";
import { slideKey } from "./snap";
import { applySidepane, bindStudio, promptNote, promptText, readSelection, type StudioSelection } from "./studio";
import { drawCube, drawSpectrogram, makeFixture, play } from "./signal";
import { CONNECTOR_MODES, validateViewport, type ViewportDocument } from "./validate";

function required(id: string): HTMLElement {
  const node = document.querySelector<HTMLElement>(id);
  if (!node) throw new Error(`missing ${id}`);
  return node;
}

/**
 * Release builds boot the real Library deck (viewport.release.json) and serve
 * an assets.json without the dev fixtures and stand-in cards (spec-fixture,
 * cube-fixture, bench-ref, cast-sample, serve-node, serve-gateway).
 * VITE_GEN_AUDIO_FIXTURES=1 (dev/test only) loads the example deck and the
 * dev assets instead; both stay out of the release bundle's main chunk
 * (dynamic import). The Vite build fails if a stand-in is still in the
 * release deck or catalog.
 */
const fixtureFlag: string | undefined = import.meta.env.VITE_GEN_AUDIO_FIXTURES;
const loadExampleDocument: () => Promise<ViewportDocument> =
  import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1"
    ? async () => (await import("../../../schemas/examples/viewport.example.json")).default as ViewportDocument
    : async () => releaseDoc as ViewportDocument;
const loadShippedDocument: () => Promise<ViewportDocument> = selectViewport(
  fixtureFlag,
  async () => releaseDoc as ViewportDocument,
  loadExampleDocument,
);
const loadDevAssets = async (): Promise<unknown[]> =>
  import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1"
    ? (await import("../../../schemas/asset-object/fixtures/assets.dev.json")).default.assets
    : [];

const board = required("#board");
const empty = required("#empty");
const status = required("#status");
// Window->MCP failures are counted on McpFailureCounter and mirrored to
// <html data-mcp-failures>. viewport_get (resyncViewport) is the MCP-readable
// status. A failed address lookup, library fetch, or asset_resolve is counted
// and shown; none of those failures is swallowed.
const mcpFailures = new McpFailureCounter(undefined, (snapshot) => {
  document.documentElement.dataset.mcpFailures = JSON.stringify(snapshot);
});
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
let mcpOrigin = "http://127.0.0.1:8765";

async function discoverMcp(): Promise<string> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const reported = await invoke<{ addr?: string; handshake_ok?: boolean }>("mcp_status");
    const found = mcpOriginFromStatus(reported, mcpOrigin, mcpFailures);
    mcpOrigin = found.origin;
    if (found.notice) status.textContent = found.notice;
  } catch (error) {
    status.textContent = noteMcpLookupError(error, mcpOrigin, mcpFailures);
  }
  return mcpOrigin;
}

const mcpReady = discoverMcp();

bindSlideScroll(board);

bindStudio(board, status, (next: StudioSelection) => {
  selection = next;
  if (paintProfile()) status.textContent = activePreview.caption;
});

function show(documentIn: unknown): void {
  const error = validateViewport(
    documentIn,
    import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1" ? "allowed" : "release",
  );
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
    decorateLibraryTiles(board, catalog, (uid) => glyphBadge(uid, { role: "clip", onCopy: announceCopy }));
    fillModelCubes(board, catalog, { openCube: (url, source) => void openCube(url, source), selectTile });
    syncCubeChrome();
  });
  const n = slides(board).length;
  status.textContent = `${doc.cards.length} cards · ${n} snap slides · spectrogram follows side pane`;
  requestAnimationFrame(() => goToSlide(board, 0));
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
  if (cube && cube.dataset.painted !== "tone") {
    drawCube(cube, fixture);
    cube.dataset.painted = "tone";
  }
  board.querySelectorAll<HTMLButtonElement>("[data-action='play-profile']").forEach((node) => {
    node.onclick = () => play(activePreview.before);
  });
  return before !== null;
}

/** Default Cube-tab binding: the misaki\u2192kokoro Inverse-HDR cube (real WAV + cube JSON). */
const DEFAULT_CUBE_CLIP = "lib-misaki-kokoro";
let cubeClipId = "";
/** Set when focus binds the Cube tab to a clip that has no cube: the tab says so and borrows nothing. */
let noCubeClipId = "";
let cubeBindSeq = 0;
let libraryCatalog: LibraryCatalog | null = null;

function clipUid(clipId: string): string | null {
  return libraryCatalog?.clipForTile(clipId)?.uid ?? null;
}

function tileTitle(clipId: string): string {
  return libraryTile(clipId)?.querySelector("h2")?.textContent?.trim() || clipId;
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
    title.textContent = meta
      ? meta.title
      : noCubeClipId
        ? `No cube for this clip — ${tileTitle(noCubeClipId)}`
        : "Inverse-HDR cube \u2014 nothing bound";
  }
  const glyphSlot = document.querySelector<HTMLElement>("#cube-glyph");
  if (glyphSlot) {
    const cube = meta && libraryCatalog ? libraryCatalog.cubeForUrl(meta.url) : null;
    const badge = cube ? glyphBadge(cube.uid, { role: "cube", onCopy: announceCopy }) : null;
    if (badge) glyphSlot.replaceChildren(badge);
    else glyphSlot.replaceChildren();
  }
  const select = document.querySelector<HTMLSelectElement>("#cube-source");
  if (select && cubeClipId) select.value = cubeClipId;
  const play = document.querySelector<HTMLButtonElement>("#cube-play");
  if (play) {
    const tile = cubeClipId ? board.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(cubeClipId)}"]`) : null;
    play.disabled = !tile?.dataset.wavUrl;
    play.textContent = cubeClipId && isClipPlaying(cubeClipId) ? "Pause" : "Play";
  }
}

/** When the Cube tab opens with nothing bound, bind the default library cube. */
function ensureDefaultCube(): void {
  if (boundCubeUrl() || noCubeClipId) return;
  const sources = cubeSources();
  const preferred = sources.find((item) => item.clipId === DEFAULT_CUBE_CLIP) ?? sources[0];
  if (!preferred) {
    clearCube("No library clip has cube JSON yet. Nothing is drawn.");
    return;
  }
  void bindCubeSource(preferred.clipId, preferred.url, "default");
}

async function bindCubeSource(clipId: string, url: string, source: string): Promise<void> {
  const seq = ++cubeBindSeq;
  cubeClipId = clipId;
  noCubeClipId = "";
  setCubeClockClip(clipId, clipUid(clipId));
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  const message = await loadCube(url);
  if (seq !== cubeBindSeq) return; // a newer binding won
  if (caption) caption.textContent = source === "default" ? message : `${source}: ${message}`;
  syncCubeChrome();
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

function libraryTile(clipId: string): HTMLElement | null {
  const escaped = CSS.escape(clipId);
  return (
    board.querySelector<HTMLElement>(`.library-tile[data-id="${escaped}"]`) ??
    board.querySelector<HTMLElement>(`.library-tile[data-uid="${escaped}"]`)
  );
}

function bindCubeToPlayingClip(clipId: string): void {
  const tile = libraryTile(clipId);
  if (!tile) return;
  const url = tile.dataset.cubeJson || "";
  if (url) {
    if (cubeClipId !== clipId || boundCubeUrl() !== url) void bindCubeSource(clipId, url, "focus");
    return;
  }
  cubeBindSeq += 1;
  cubeClipId = clipId;
  noCubeClipId = clipId;
  setCubeClockClip(clipId, clipUid(clipId));
  clearCube(`No cube for this clip (${tileTitle(clipId)}). Nothing is drawn; another clip's cube is not borrowed.`);
  syncCubeChrome();
}

/** Mark the focused clip without scrolling the deck (the user is already looking at it, or asked over MCP). */
function focusClipTile(clipId: string): void {
  const tile = libraryTile(clipId);
  if (!tile) return;
  board.querySelectorAll<HTMLElement>(".card.is-selected").forEach((node) => node.classList.remove("is-selected"));
  tile.classList.add("is-selected");
}

function bindCubeCanvas(): void {
  const canvas = document.querySelector<HTMLCanvasElement>("#cube-viewport");
  if (!canvas) {
    unbindCube(); // stage gone: drop the old canvas's window/DPR listeners
    return;
  }
  if (canvas.dataset.bound === "1") return;
  canvas.dataset.bound = "1";
  bindCube(canvas, document.querySelector<HTMLCanvasElement>("#cube-labels"));
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
        if (body.profile && typeof body.profile === "object") {
          applyFlipcard(body.profile as Record<string, unknown>);
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
    clearCube("This tile has no cube JSON. Magpie, VibeVoice, and Pocket do not get a substitute cloud.");
    return;
  }
  const owner = board.querySelector<HTMLElement>(`.library-tile[data-cube-json="${CSS.escape(url)}"]`);
  cubeBindSeq += 1;
  cubeClipId = owner?.dataset.id ?? "";
  noCubeClipId = "";
  setCubeClockClip(cubeClipId || null, cubeClipId ? clipUid(cubeClipId) : null);
  const message = await loadCube(url);
  if (caption) caption.textContent = `${source}: ${message}`;
  syncCubeChrome();
  status.textContent = `Cube tab bound (${source}).`;
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
// E1 addendum: the fixture controls exist only in dev/test builds. The check is
// a literal import.meta.env comparison so Vite replaces it at build time and
// the release bundle drops this block, labels and command name included
// (apps/desktop/tests/viewport-source.test.ts greps dist for them).
if (import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1") {
  const toolbar = document.querySelector(".ga-header-toolbar .toolbar");
  const showExample = document.createElement("button");
  showExample.type = "button";
  showExample.id = "show-example";
  showExample.textContent = "Load labeled example";
  showExample.addEventListener("click", () => {
    void loadShippedDocument().then(refreshConnectors).then(show);
  });
  const runImprove = document.createElement("button");
  runImprove.type = "button";
  runImprove.id = "run-improve";
  runImprove.textContent = "Run Python improve on fixture";
  runImprove.addEventListener("click", async () => {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const result = await invoke<Record<string, unknown>>("run_fixture_improve");
      status.textContent = result.ok
        ? "Python improve finished on the fixture tone (fixture only — not podcast speech)."
        : `Python improve did not finish: ${JSON.stringify(result)}`;
    } catch (error) {
      status.textContent = `Python improve needs a debug desktop shell. ${String(error)}`;
    }
  });
  toolbar?.prepend(showExample);
  toolbar?.append(runImprove);
}
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
  const recorded = await mcpCall("ui_generate", {
    agents: selectionNow.agents,
    voice: selectionNow.voice,
    durationMin: selectionNow.durationMin,
    duration_s: selectionNow.durationMin * 60,
    prompt: promptText(),
    promptNote: promptNote(),
  });
  if (!recorded) {
    showRun(selectionNow.voice, "unavailable", `MCP is not listening on ${mcpOrigin}`, false);
    return;
  }
  const body = toolBody(recorded);
  const phase = String(body?.phase ?? "refused");
  const detail = String(body?.note ?? "");
  showRun(selectionNow.voice, phase, detail, body?.synthesizedSpeech === true);
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

function applyFlipcard(profile: Record<string, unknown>): void {
  const personaId = String(profile.personaId ?? "");
  if (!personaId) return;
  const card = board.querySelector<HTMLElement>(`.card[data-id="profile-${CSS.escape(personaId)}"]`);
  if (!card) return;
  const refs = Array.isArray(profile.refs) ? profile.refs.map((item) => String(item)).join(", ") : "";
  const slot = card.querySelector<HTMLElement>('[data-field="refs"]');
  if (slot && refs) slot.textContent = refs;
  setCardFlip(board, `profile-${personaId}`, true);
}

async function mcpCall(name: string, args: Record<string, unknown>): Promise<unknown | null> {
  const origin = await mcpReady;
  const payload = await postMcp(fetch, `${origin}/mcp`, name, args, mcpFailures);
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
    controlCursor = Math.max(controlCursor, event.seq);
    if (seenControl.has(event.seq)) return;
    seenControl.add(event.seq);
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
  } else if (event.op === "flip" && typeof args.tileId === "string") {
    setCardFlip(board, args.tileId, args.flipped !== false);
    if (args.profile && typeof args.profile === "object") applyFlipcard(args.profile as Record<string, unknown>);
  } else if (event.op === "flipcard" && args.profile && typeof args.profile === "object") {
    applyFlipcard(args.profile as Record<string, unknown>);
  } else if (event.op === "progress" && typeof args.voice === "string") {
    showRun(args.voice, String(args.phase ?? "unavailable"), String(args.detail ?? ""), args.synthesizedSpeech === true);
  } else if (event.op === "focus" && typeof args.uid === "string") {
    focusClipTile(args.uid);
    bindCubeToPlayingClip(args.uid);
  } else if (event.op === "play" && typeof args.playing === "string") {
    const origin = args.origin === "user" ? "user" : "auto";
    void playClip(args.playing, origin).then((result) => {
      status.textContent = result === "playing" ? `Playing ${args.playing}` : result;
      syncCubeChrome();
    });
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
  } else if (event.op === "job") {
    void applyJob(args);
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

function honestyOf(value: unknown): string {
  if (!value || typeof value !== "object") return "";
  const honesty = (value as { honesty?: unknown }).honesty;
  return typeof honesty === "string" ? honesty : "";
}

function setDerived(parentUid: string, role: string, honesty: string): void {
  if (!parentUid || !honesty) return;
  const node = libraryTile(parentUid)?.querySelector<HTMLElement>(`[data-derived="${role}"]`);
  if (node) node.textContent = `${role}: ${honesty}`;
}

async function mediaBlob(urlPath: string): Promise<string | null> {
  const blob = await fetchLibraryBlob(fetch, await mcpReady, urlPath, mcpFailures);
  return blob ? URL.createObjectURL(blob) : null;
}

async function mountGeneratedVideo(urlPath: string): Promise<void> {
  const section = board.querySelector<HTMLElement>('[data-slide="video"]');
  if (!section) return;
  const blob = await mediaBlob(urlPath);
  if (!blob) return;
  const status = section.querySelector<HTMLElement>("[data-video-status]");
  if (status) status.textContent = "Generated clip.";
  const slot = section.querySelector<HTMLElement>("[data-video-slot]") ?? section;
  let video = slot.querySelector("video");
  if (!video) {
    video = document.createElement("video");
    video.controls = true;
    video.dataset.generated = "1";
    slot.append(video);
  }
  video.src = blob;
}

async function insertGeneratedTile(uid: string, args: Record<string, unknown>): Promise<void> {
  const slide = board.querySelector<HTMLElement>('[data-slide="library"]');
  if (!slide) return;
  const resolved = toolBody(await mcpCall("asset_resolve", { uid }));
  const resolveError = assetResolveFailure(resolved);
  if (resolveError || !resolved) {
    const message = noteAssetResolveFailure(uid, resolveError ?? "asset_resolve failed", mcpFailures);
    let tile = libraryTile(uid);
    if (!tile) {
      tile = document.createElement("article");
      tile.className = "card livetile library-tile";
      tile.tabIndex = -1;
      slide.append(tile);
    }
    tile.dataset.uid = uid;
    tile.dataset.kind = "LibraryClip";
    tile.dataset.resolve = "failed";
    let node = tile.querySelector<HTMLElement>(".resolve-error");
    if (!node) {
      node = document.createElement("p");
      node.className = "resolve-error";
      tile.append(node);
    }
    node.textContent = message;
    status.textContent = message;
    return;
  }
  const legacy = String(resolved.tileId ?? args.legacyId ?? uid);
  const asset = (resolved.asset && typeof resolved.asset === "object" ? resolved.asset : {}) as Record<string, unknown>;
  const display = (asset.display && typeof asset.display === "object" ? asset.display : {}) as Record<string, unknown>;
  const title = String(display.title ?? "Generated clip");
  let tile = libraryTile(uid) ?? libraryTile(legacy);
  if (!tile) {
    tile = document.createElement("article");
    tile.className = "card livetile library-tile";
    tile.tabIndex = -1;
    const heading = document.createElement("h2");
    heading.textContent = title;
    const spec = document.createElement("p");
    spec.dataset.derived = "spectrogram";
    const cube = document.createElement("p");
    cube.dataset.derived = "cube";
    tile.append(heading, spec, cube);
    slide.append(tile);
  }
  tile.dataset.id = legacy;
  tile.dataset.uid = uid;
  tile.dataset.kind = "LibraryClip";
  const wavPath = typeof args.wavUrl === "string" ? args.wavUrl : "";
  if (wavPath && !tile.querySelector("audio")) {
    const blob = await mediaBlob(wavPath);
    if (blob) {
      tile.dataset.wavUrl = blob;
      tile.dataset.hasWav = "1";
      tile.append(renderTransport(legacy, blob));
    }
  }
  if (typeof args.cubeUrl === "string" && args.cubeUrl.startsWith("/library/")) {
    const origin = await mcpReady;
    tile.dataset.cubeJson = `${origin}${args.cubeUrl}`;
  }
  setDerived(uid, "spectrogram", honestyOf(args.spectrogram) || "pending");
  setDerived(uid, "cube", honestyOf(args.cube) || "pending");
  if (typeof args.videoUrl === "string") await mountGeneratedVideo(args.videoUrl);
}

async function applyJob(args: Record<string, unknown>): Promise<void> {
  const phase = String(args.phase ?? "");
  const voice = typeof args.voice === "string" ? args.voice : "";
  if (phase === "running") {
    if (voice) showRun(voice, "running", "generating", false);
    return;
  }
  const target = typeof args.target === "string" ? args.target : "";
  if (phase === "done" && target.startsWith("ga:audio_clip:")) await insertGeneratedTile(target, args);
  if (phase === "done" && target.endsWith(":spectrogram")) {
    setDerived(target.slice(0, -":spectrogram".length), "spectrogram", String(args.honesty ?? "real"));
  }
  if (phase === "done" && target.endsWith(":cube")) {
    setDerived(target.slice(0, -":cube".length), "cube", String(args.honesty ?? "real"));
  }
  if (typeof args.videoUrl === "string") await mountGeneratedVideo(args.videoUrl);
}

async function resyncViewport(): Promise<void> {
  const payload = await mcpCall("viewport_get", {});
  const body = toolBody(payload);
  if (!body) return;
  if (typeof body.cursor === "number") controlCursor = body.cursor;
  const ui = body.ui;
  if (ui && typeof ui === "object") {
    const record = ui as { slide?: unknown; focus?: unknown; flipped?: unknown; compare?: unknown };
    if (typeof record.slide === "string") goToSlideId(board, record.slide);
    if (typeof record.focus === "string") {
      focusClipTile(record.focus);
      bindCubeToPlayingClip(record.focus);
      setCubeClockClip(record.focus, clipUid(record.focus));
    } else if (record.focus === null) {
      setCubeClockClip(null);
    }
    if (record.flipped && typeof record.flipped === "object") {
      for (const [view, state] of Object.entries(record.flipped as Record<string, { face?: string }>)) {
        const tileId = view.startsWith("view:") ? view.slice("view:".length) : view;
        setCardFlip(board, tileId, state?.face === "back");
      }
    }
    board.querySelectorAll<HTMLElement>(".card.is-compare").forEach((node) => node.classList.remove("is-compare"));
    if (Array.isArray(record.compare)) {
      for (const uid of record.compare) {
        if (typeof uid !== "string") continue;
        board.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(uid)}"]`)?.classList.add("is-compare");
      }
    }
  }
  if (Array.isArray(body.jobs)) {
    for (const job of body.jobs) {
      if (!job || typeof job !== "object") continue;
      const record = job as { voice?: unknown; phase?: unknown; reason?: unknown; synthesized?: unknown };
      if (typeof record.voice === "string" && record.voice) {
        showRun(record.voice, String(record.phase ?? "unavailable"), String(record.reason ?? ""), record.synthesized === true);
      }
    }
  }
}

void loadShippedDocument().then(refreshConnectors).then(show);

function connectControl(): void {
  void mcpReady.then((origin) => {
    let source: EventSource;
    try {
      source = new EventSource(`${origin}/control/stream?after=${controlCursor}&wait=2000`);
    } catch {
      return;
    }
    source.addEventListener("gap", () => {
      void resyncViewport();
    });
    source.addEventListener("control", (event) => {
      try {
        const parsed = JSON.parse((event as MessageEvent).data) as { seq?: number; op?: string; args?: Record<string, unknown> };
        if (typeof parsed.seq === "number" && parsed.seq > controlCursor + 1) {
          void resyncViewport();
          return;
        }
        applyControl(parsed);
      } catch {
        /* ignore malformed events */
      }
    });
    source.onerror = () => {
      source.close();
      window.setTimeout(connectControl, 50);
    };
  });
}
// Test hook (dev/test builds only): scrubs the cube and ONLY the clip the cube is bound to.
if (import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1") {
  (window as unknown as { __genAudioScrub?: (f: number) => Promise<string> }).__genAudioScrub = (fraction: number) => {
    setCubeScrub(fraction, { silent: true });
    return cubeClipId ? seekClipFraction(cubeClipId, fraction) : Promise.resolve("no cube clip");
  };
}
connectControl();
