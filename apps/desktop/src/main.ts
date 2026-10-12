import releaseDoc from "../../../schemas/examples/viewport.release.json";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { generatedJob } from "./generated-job";
import {
  BoundedSequences, McpClient, McpHttpError, controlBackoff, controlNeedsResync, resolveMcpConfig,
  speechResult, toolBody as requireToolBody, type McpConfig, type NativeMcpStatus,
} from "./mcp-client";
import {
  bindCube,
  bindCompareCube,
  boundCubeUrl,
  clearCube,
  cubeNameFromUrl,
  getCubeMeta,
  loadCube,
  loadCompareCube,
  exitCompare,
  onCubeClock,
  rebuildLayerMatrixUi,
  setCubeScrub,
  unbindCube,
} from "./cubeview";
import { clearSpeechOutput, getActiveClipId, isClipPlaying, seekActiveFraction, seekClipFraction, setCubeClockClip, showSpeechOutput } from "./playback";
import { applyClipNames, harvestNames } from "./library-meta";
import { bindFloatingPlayback, pauseClip, pauseClipByUser, playClip, releaseAllSeekBlobs, releaseDetachedTransports, renderTransport, seekClipOutcome, setUserPlayReporter, setUserPauseReporter } from "./playback";
import {
  assetResolveFailure,
  controlPlayOrigin,
  McpFailureCounter,
  noteAssetResolveFailure,
  reportAsync,
  seekReportControl,
  setReportSink,
  surfaceUiError,
  userPlayControl,
} from "./play-control";
import { selectViewport, snapshotFaces } from "./viewport-source";
import { glyphBadge } from "./glyph";
import { loadLibraryCatalog, mediaUrl, comparisonMethod, sourceSha256, type LibraryCatalog, type AssetEnvelope } from "./library-assets";
import { createCubeModeController, cubeStateFrom, type CubeCompareState } from "./cube-mode";
import { bindSemanticCubePanel } from "./semantic-cube-panel";
import { isSemanticCube, loadSemanticCube, semanticCubeDocument } from "./cubeview";
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
  setCardFace,
  setFaceRequester,
  applyViewportViews,
  showRejected,
} from "./render";
import { slideKey } from "./snap";
import { applySidepane, bindStudio, readSpeechRequest, readSelection, readExecution, setScriptText, type StudioSelection } from "./studio";
import { bindAuthoringPanel } from "./authoring-panel";
import { bindAgentProfileLibrary } from "./agent-profile-library";
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
setReportSink((message) => {
  status.textContent = message;
});
// Window->MCP failures are counted on McpFailureCounter and mirrored to
// <html data-mcp-failures>. viewport_get (resyncViewport) is the MCP-readable
// status. A failed address lookup, library fetch, or asset_resolve is counted
// and shown; none of those failures is swallowed.
const mcpFailures = new McpFailureCounter(undefined, (snapshot) => {
  document.documentElement.dataset.mcpFailures = JSON.stringify(snapshot);
});
const backendStatus = required("#backend-status");
const clientReady = connectBackend();
const releaseProfileSamples = bindAgentProfileLibrary(board, () => clientReady, message => { status.textContent = message; });
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
const seenControl = new BoundedSequences();
let libraryCatalog: LibraryCatalog | null = null;
let controlAttempt = 0;
let generating = false;
let outputRevision = 0;

function failUi(error: unknown, where: string): void {
  status.textContent = surfaceUiError(error, where);
}

/** The gap listener and a skipped control seq both resync through this. */
let controlResync: () => Promise<unknown> = () => resyncViewport();

function onControlGap(): Promise<unknown> {
  return reportAsync(controlResync(), "viewport");
}

/** Test seam. Production leaves the real viewport_get resync in place. */
export function setControlResyncForTest(work: (() => Promise<unknown>) | null): void {
  controlResync = work ?? (() => resyncViewport());
}

/**
 * Paint a catalog load. The rejection lands here: loadLibraryCatalog does not
 * swallow it, so removing this catch leaves the failure unlogged.
 */
export function presentLibraryCatalog(work: Promise<LibraryCatalog | null>): void {
  void work
    .then((catalog) => {
      libraryCatalog = catalog;
      decorateLibraryTiles(board, catalog, (uid) => glyphBadge(uid, { role: "clip", onCopy: announceCopy }));
      fillModelCubes(board, catalog, {
        openCube: (url, source) => reportAsync(openCube(url, source), "cube"),
        selectTile,
      });
      syncCubeChrome();
    })
    .catch((error: unknown) => failUi(error, "library catalog"));
}

/** One control-stream frame. A bad frame is counted and shown; the stream stays up. */
export function acceptControlFrame(data: string): void {
  try {
    const parsed = JSON.parse(data) as { seq?: number; op?: string; args?: Record<string, unknown> };
    if (typeof parsed.seq === "number" && parsed.seq > controlCursor + 1) {
      void onControlGap();
      return;
    }
    applyControl(parsed);
  } catch (error) {
    // Keep the stream. One bad frame must not look like a disconnect.
    mcpFailures.note("control_stream", "malformed");
    failUi(error, "control event");
  }
}

async function connectBackend(): Promise<McpClient | null> {
  try {
    const browserConfig = (window as unknown as { __GEN_AUDIO_MCP__?: McpConfig }).__GEN_AUDIO_MCP__;
    const config = await resolveMcpConfig(
      isTauri() ? () => invoke<NativeMcpStatus>("mcp_status") : undefined,
      browserConfig,
    );
    const client = new McpClient(config);
    backendStatus.textContent = "";
    return client;
  } catch (error) {
    mcpFailures.note("mcp_status", error instanceof Error ? error.name : "unknown");
    failUi(error, "MCP configuration");
    backendStatus.textContent = error instanceof Error ? error.message : "MCP configuration unavailable.";
    return null;
  }
}

bindSlideScroll(board);

const engineLoader = bindStudio(board, status, (next: StudioSelection) => {
  selection = next;
  if (paintProfile()) status.textContent = activePreview.caption;
}, async engine => {
  const client = await clientReady;
  if (!client) throw new Error("The owned desktop backend is unavailable.");
  return requireToolBody(await client.call("engine_load", { engine, ...(engine === "vibevoice" ? { execution: readExecution() } : {}) }));
});
const authoringPanel = bindAuthoringPanel({ client: () => clientReady, setScript: setScriptText });
setFaceRequester(tileId => {
  reportAsync(mcpCall("ui_flip", { tileId, next: true }).then(payload => {
    const body = requireToolBody(payload);
    if (body.args && typeof body.args === "object") {
      applyControl({ op: "flip", args: body.args as Record<string, unknown> });
      if (Number.isSafeInteger(body.seq)) seenControl.add(Number(body.seq));
    }
  }), "flip");
});
board.addEventListener("open-cube", event => {
  const { url, tileId } = (event as CustomEvent<{ url: string; tileId: string }>).detail;
  reportAsync(openCube(url, tileId), "cube");
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
  releaseProfileSamples();
  renderBoard(board, empty, doc);
  // D: tiles this render removed give back their cached seek blob URLs.
  releaseDetachedTransports();
  paintProfile();
  bindCubeCanvas();
  bindRename();
  reportAsync(harvestLibrary(), "library harvest");
  bindFloatingPlayback();
  presentLibraryCatalog(loadLibraryCatalog(loadDevAssets()));
  if (status.dataset.control !== "disconnected") {
    status.textContent = "";
  }
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

function clipUid(clipId: string): string | null {
  const direct = libraryCatalog?.byUid.get(clipId);
  return direct?.kind === "audio_clip" ? direct.uid : libraryCatalog?.clipForTile(clipId)?.uid ?? libraryTile(clipId)?.dataset.uid ?? null;
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
  const compare = document.querySelector<HTMLButtonElement>("#cube-compare-toggle");
  if (compare) {
    const enabled = cubeMode.state().cube_mode === "compare";
    compare.setAttribute("aria-pressed", String(enabled));
    compare.textContent = enabled ? "Single" : "Compare";
    compare.disabled = !enabled && !compareCubeFor(cubeClipId);
    compare.title = compare.disabled ? "No same-WAV pipeline_r2 comparison in the release catalog." : "One clock, real Library vs Pipeline formulas";
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
  if (isSemanticCube()) return;
  if (boundCubeUrl() || noCubeClipId) return;
  const sources = cubeSources();
  const preferred = sources.find((item) => item.clipId === DEFAULT_CUBE_CLIP) ?? sources[0];
  if (!preferred) {
    clearCube("No library clip has cube JSON yet. Nothing is drawn.");
    return;
  }
  reportAsync(bindCubeSource(preferred.clipId, preferred.url, "default"), "cube");
}

async function bindCubeSource(clipId: string, url: string, source: string): Promise<void> {
  if (await loadBoundCube(clipId, url, source)) {
    const result = await cubeMode.rebound(clipId);
    if (result) reportCubeMode(result);
  }
}

async function loadBoundCube(clipId: string, url: string, source: string): Promise<boolean> {
  const seq = ++cubeBindSeq;
  cubeClipId = clipId;
  noCubeClipId = "";
  setCubeClockClip(clipId, clipUid(clipId));
  const caption = document.querySelector<HTMLElement>("#cube-caption");
  const message = await loadCube(url);
  if (seq !== cubeBindSeq) return false;
  setCubeClockClip(clipId, clipUid(clipId));
  if (caption) caption.textContent = source === "default" ? message : `${source}: ${message}`;
  syncCubeChrome();
  return boundCubeUrl() === url;
}

function compareCubeFor(clipId: string): AssetEnvelope | null {
  const uid = clipUid(clipId);
  return uid ? libraryCatalog?.compareCubesFor(uid).find(asset => comparisonMethod(asset) === "pipeline_r2") ?? null : null;
}

const cubeMode = createCubeModeController({
  async enter(pair: CubeCompareState) {
    libraryCatalog ??= await loadLibraryCatalog(loadDevAssets());
    const source = cubeSources().find(item => item.clipId === pair.tileId);
    if (!source) return { ok: false, reason: "No library cube for the requested clip." };
    if (cubeClipId !== pair.tileId || boundCubeUrl() !== source.url) {
      if (!await loadBoundCube(pair.tileId, source.url, "ui_cube")) return { ok: false, reason: "Cube binding superseded." };
    }
    const left = libraryCatalog?.cubeForUrl(source.url) ?? null;
    const right = compareCubeFor(pair.tileId);
    const url = right ? mediaUrl(right, "cube_json") : null;
    if (left?.uid !== pair.left_cube_uid || right?.uid !== pair.right_cube_uid || clipUid(pair.tileId) !== pair.clip_uid || !url) {
      return { ok: false, reason: "ui_cube and the release catalog disagree on the same-WAV comparison assets." };
    }
    const result = await loadCompareCube(url, { primary: sourceSha256(left), compare: sourceSha256(right) });
    if (result.ok) {
      goToSlideId(board, "spatial");
      document.querySelector("#cube-layer-matrix")?.classList.add("is-collapsed");
      const caption = document.querySelector("#cube-caption");
      if (caption) caption.textContent = result.message;
    }
    return result;
  },
  exit() { exitCompare(); syncCubeChrome(); },
}, async (name, args) => {
  try { return { body: requireToolBody(await mcpCall(name, args)) }; }
  catch (error) { return { error: error instanceof Error ? error.message : "MCP cube request failed." }; }
});

function reportCubeMode(result: { ok: boolean; reason?: string }): void {
  syncCubeChrome();
  if (!result.ok) failUi(new Error(result.reason ?? "Cube mode failed."), "cube mode");
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

const localPlaybackRequests = new Set<string>();
let localPlaybackQueue = Promise.resolve();
let localPlaybackRevision = 0;
function reportPlaybackIntent(clipId: string, action: "play" | "pause", origin = "user"): void {
  if (!libraryTile(clipId)) return;
  ++localPlaybackRevision;
  const id = crypto.randomUUID();
  localPlaybackRequests.add(id);
  if (localPlaybackRequests.size > 512) localPlaybackRequests.delete(localPlaybackRequests.values().next().value!);
  const request = userPlayControl(clipId);
  const args = action === "play" ? { ...request.args, origin, clientRequestId: id } : { tileId: clipId, action, clientRequestId: id };
  localPlaybackQueue = localPlaybackQueue.then(async () => { requireToolBody(await mcpCall(request.name, args)); })
    .catch(error => failUi(error, "playback control"));
}
setUserPlayReporter((clipId, origin) => reportPlaybackIntent(clipId, "play", origin === "resume" ? "auto" : "user"));
setUserPauseReporter(clipId => reportPlaybackIntent(clipId, "pause"));

function libraryTile(clipId: string): HTMLElement | null {
  const escaped = CSS.escape(clipId);
  return (
    board.querySelector<HTMLElement>(`.library-tile[data-id="${escaped}"]`) ??
    board.querySelector<HTMLElement>(`.library-tile[data-uid="${escaped}"]`) ??
    board.querySelector<HTMLElement>(`.library-tile[data-clip-uid="${escaped}"]`) ??
    board.querySelector<HTMLElement>(`.library-tile[data-asset-uid="${escaped}"]`)
  );
}

async function bindCubeToPlayingClip(clipId: string): Promise<void> {
  if (isSemanticCube()) return;
  const tile = libraryTile(clipId);
  if (!tile) return;
  clipId = tile.dataset.id || clipId;
  const url = tile.dataset.cubeJson || "";
  if (url) {
    if (cubeClipId !== clipId || boundCubeUrl() !== url) await bindCubeSource(clipId, url, "focus");
    return;
  }
  cubeBindSeq += 1;
  cubeClipId = clipId;
  noCubeClipId = clipId;
  setCubeClockClip(clipId, clipUid(clipId));
  clearCube(`No cube for this clip (${tileTitle(clipId)}). Nothing is drawn; another clip's cube is not borrowed.`);
  if (cubeMode.state().cube_mode === "compare") reportAsync(cubeMode.request("single").then(reportCubeMode), "cube mode");
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
  const comparison = document.querySelector<HTMLCanvasElement>("#cube-compare-viewport");
  if (comparison) bindCompareCube(comparison, document.querySelector<HTMLCanvasElement>("#cube-compare-labels"));
  document.querySelector("#cube-compare-toggle")?.addEventListener("click", () => {
    reportAsync(cubeMode.request(cubeMode.state().cube_mode === "compare" ? "single" : "compare", cubeClipId).then(reportCubeMode), "cube mode");
  });
  rebuildLayerMatrixUi();
  const picker = document.querySelector<HTMLElement>(".cube-picker");
  if (picker) bindSemanticCubePanel(picker, async (doc, current) => {
    const previousBind = cubeBindSeq;
    const client = await clientReady;
    if (!client) throw new Error("The native schema validator is unavailable.");
    const result = requireToolBody(await client.call("semantic_cube_project", { document: doc, limit: 1 }));
    if (result.ok !== true) throw new Error("Native metric projection failed.");
    if (!current() || previousBind !== cubeBindSeq) return;
    if (cubeMode.state().cube_mode === "compare") {
      const mode = await cubeMode.request("single");
      if (!mode.ok) throw new Error(mode.reason);
      if (!current() || previousBind !== cubeBindSeq) return;
    }
    // Cancel pending audio-cube loads before committing a metric view.
    cubeBindSeq++;
    cubeClipId = ""; noCubeClipId = "";
    setCubeClockClip(null);
    const caption = document.querySelector<HTMLElement>("#cube-caption");
    const message = loadSemanticCube(doc);
    if (caption) caption.textContent = message;
    const select = document.querySelector<HTMLSelectElement>("#cube-source");
    if (select) select.selectedIndex = -1;
    document.querySelector("#cube-layer-matrix")?.classList.remove("is-collapsed");
    const toggle = document.querySelector("#cube-matrix-toggle");
    if (toggle) { toggle.textContent = "Layers ▾"; toggle.setAttribute("aria-expanded", "true"); }
    syncCubeChrome();
  });
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
      if (option?.dataset.cubeJson) reportAsync(bindCubeSource(option.value, option.dataset.cubeJson, option.value), "cube");
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
    const action = isClipPlaying(cubeClipId) ? pauseClipByUser(cubeClipId) : null;
    if (action) {
      syncCubeChrome();
      return;
    }
    reportPlaybackIntent(cubeClipId, "play");
    void playClip(cubeClipId, "user")
      .then((result) => {
        status.textContent = result === "playing" ? `Cube live clock follows ${cubeClipId}` : result;
        syncCubeChrome();
      })
      .catch((error: unknown) => failUi(error, "play"));
  });
  document.querySelector("#cube-scrub")?.addEventListener("input", (event) => {
    const input = event.target as HTMLInputElement;
    const fraction = Number(input.value) / 1000;
    // ONE clock: scrubber seeks library audio AND slices cube layers.
    setCubeScrub(fraction);
    if (isSemanticCube()) return;
    void (cubeClipId ? seekClipFraction(cubeClipId, fraction) : seekActiveFraction(fraction))
      .then((seeked) => {
        if (seeked !== "no player" && seeked !== "superseded") status.textContent = `Shared clock ${Math.round(fraction * 100)}% \u00b7 ${seeked}`;
      })
      .catch((error: unknown) => failUi(error, "seek"));
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
  const metric = semanticCubeDocument();
  if (metric) { loadSemanticCube(metric); syncCubeChrome(); }
  else if (already) reportAsync(bindCubeSource(cubeClipId || DEFAULT_CUBE_CLIP, already, "default"), "cube");
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
      reportAsync(mcpCall("library_rename", { clipId: id, semanticName: semantic, faceName: face }), "library rename");
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
      void mcpCall("library_harvest", { clipId: id, personaId, apply: true })
        .then((payload) => {
          const body = toolBody(payload);
          if (!body) {
            status.textContent = `Names stay on ${id}. MCP is not listening, so the profile ref was not written.`;
            return;
          }
          status.textContent = `Attached clip:${id} to persona ${personaId}. Audio was not decoded. synthesizedSpeech is false.`;
          if (body.profile && typeof body.profile === "object") {
            applyFlipcard(body.profile as Record<string, unknown>);
          }
        })
        .catch((error: unknown) => failUi(error, "library harvest"));
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
  if (!url) {
    cubeClipId = "";
    setCubeClockClip(null);
    clearCube("This tile has no cube JSON. Magpie, VibeVoice, and Pocket do not get a substitute cloud.");
    return;
  }
  const owner = board.querySelector<HTMLElement>(`.library-tile[data-cube-json="${CSS.escape(url)}"]`);
  await bindCubeSource(owner?.dataset.id ?? "", url, source);
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
  if (cube) reportAsync(openCube(cube, id), "cube");
}

async function refreshConnectors(doc: ViewportDocument): Promise<ViewportDocument> {
  try {
    if (!isTauri()) return doc;
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
  } catch (error) {
    failUi(error, "connector status");
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
    void loadShippedDocument()
      .then(refreshConnectors)
      .then(show)
      .catch((error: unknown) => failUi(error, "viewport"));
  });
  const runImprove = document.createElement("button");
  runImprove.type = "button";
  runImprove.id = "run-improve";
  runImprove.textContent = "Run Python improve on fixture";
  runImprove.addEventListener("click", async () => {
    try {
      const result = await invoke<Record<string, unknown>>("run_fixture_improve");
      status.textContent = result.ok
        ? "Python improve finished on the fixture tone (fixture only — not podcast speech)."
        : `Python improve did not finish: ${JSON.stringify(result)}`;
    } catch (error) {
      surfaceUiError(error, "python improve");
      status.textContent = `Python improve needs a debug desktop shell. ${String(error)}`;
    }
  });
  toolbar?.prepend(showExample);
  toolbar?.append(runImprove);
}
document.querySelector("#generate-podcast")?.addEventListener("click", () => {
  reportAsync(submitGenerate(), "generate");
});

const layerSwitch = document.querySelector<HTMLElement>("#layer-switch");
layerSwitch?.addEventListener("keydown", event => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-slide]");
    if (!button) return;
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>("#layer-switch [data-layer]"));
    const current = tabs.indexOf(button);
    const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 :
      (current + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
    tabs[next]?.click(); tabs[next]?.focus();
});
layerSwitch?.addEventListener("click", event => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-slide]");
    if (!button || button.disabled) return;
    const target = button.dataset.slide;
    if (target) goToSlideId(board, target);
    button.focus({ preventScroll: true });
    if (target === "spatial") {
      ensureDefaultCube();
      syncCubeChrome();
    }
});

document.addEventListener("reveal-playing-audio", event => {
  const audio = event.target as HTMLAudioElement;
  const tile = audio.closest<HTMLElement>(".card");
  const slide = tile?.closest<HTMLElement>(".slide");
  if (tile?.hidden && slide?.dataset.slide === "profiles") {
    const search = slide.querySelector<HTMLInputElement>("input[type=search]");
    const filter = slide.querySelector<HTMLSelectElement>(".profile-library-tools select");
    if (search) search.value = "";
    if (filter) filter.value = "";
    search?.dispatchEvent(new Event("input"));
  }
  if (slide?.dataset.slide) goToSlideId(board, slide.dataset.slide);
  if (tile?.dataset.id) {
    const pane = audio.closest<HTMLElement>(".face[data-face-id]");
    if (pane?.dataset.faceId) setCardFace(board, tile.dataset.id, pane.dataset.faceId);
    else setCardFlip(board, tile.dataset.id, false);
    tile.classList.add("is-selected");
  }
  const target = tile?.querySelector<HTMLElement>(".transport-toggle") ?? audio;
  target.scrollIntoView({ block: "nearest" });
  target.focus({ preventScroll: true });
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
  if (generating) return;
  generating = true;
  const button = document.querySelector<HTMLButtonElement>("#generate-podcast");
  if (button) button.disabled = true;
  const selectionNow = readSelection();
  const revision = ++outputRevision;
  clearSpeechOutput();
  authoringPanel.setAudioSource(null);
  try {
    const request = await readSpeechRequest();
    await engineLoader.wait();
    const client = await clientReady;
    if (!client) throw new Error(backendStatus.textContent ?? "The authenticated MCP backend is unavailable.");
    if (request.voice === "vibevoice") {
      showRun(request.voice, "preflight", "Checking the selected voice references and VibeVoice runtime…", false);
      const ready = requireToolBody(await client.call("studio_preflight", { ...request }));
      if (ready.ready !== true) throw new Error("VibeVoice is not ready. Review its preflight blockers.");
    }
    showRun(request.voice, "running", "Synthesizing the supplied script and selected personas…", false);
    const result = speechResult(await client.call("synth", { ...request }), true);
    void refreshGenerations();
    if (!await presentSpeech(client, result, revision)) return;
    showRun(request.voice, result.complete === false ? "incomplete" : "ok", `${result.duration_s.toFixed(2)} seconds of verified audio${result.complete === false ? "; generation cap reached — incomplete" : ""}. ${result.assetUid ? "Saved in your library." : "Download to keep it."} Transcript not verified.`, true);
  } catch (error) {
    if (revision === outputRevision) showRun(selectionNow.voice, "refused", error instanceof Error ? error.message : "Speech generation failed.", false);
  } finally {
    generating = false;
    if (button) button.disabled = false;
  }
}

let manifestObjectUrl: string | undefined;
async function presentSpeech(client: McpClient, result: import("./mcp-client").SpeechResult, revision: number): Promise<boolean> {
  const [audio, manifest] = await Promise.all([client.speechBlob(result), client.manifestBlob(result)]);
  if (revision !== outputRevision) return false;
  if (manifestObjectUrl) URL.revokeObjectURL(manifestObjectUrl);
  showSpeechOutput(audio, result.wav, result.duration_s);
  authoringPanel.setAudioSource(result);
  manifestObjectUrl = URL.createObjectURL(manifest);
  const link = document.querySelector<HTMLAnchorElement>("#manifest-download");
  if (link) { link.href = manifestObjectUrl; link.download = result.manifest; }
  return true;
}
async function refreshGenerations(): Promise<void> {
  const list = document.querySelector<HTMLUListElement>("#generation-list"); if (!list) return;
  try {
    const client = await clientReady; if (!client) return;
    const data = requireToolBody(await client.call("generated_library_list", {}));
    list.replaceChildren();
    if (!Array.isArray(data.assets)) throw new Error("Invalid saved library response.");
    for (const value of data.assets.slice(0, 100)) {
      if (!value || typeof value !== "object" || typeof value.uid !== "string") continue;
      const row = document.createElement("li"), open = document.createElement("button");
      open.textContent = String(value.display?.title ?? value.uid);
      open.addEventListener("click", () => { void (async () => {
        const revision = ++outputRevision;
        clearSpeechOutput();
        authoringPanel.setAudioSource(null);
        try {
          const result = speechResult(await client.call("generated_library_open", { uid: value.uid }), true);
          if (!await presentSpeech(client, result, revision)) return;
          status.textContent = `Saved ${result.complete === false ? "incomplete " : ""}generation opened. Audio integrity verified; transcript not verified.`;
        } catch (error) { if (revision === outputRevision) failUi(error, "saved generation"); }
      })(); });
      row.append(open); list.append(row);
    }
    if (!list.children.length) { const row = document.createElement("li"); row.textContent = "No generated audio has been saved yet."; list.append(row); }
  } catch (error) { list.textContent = error instanceof Error ? error.message : "Saved library unavailable."; }
}
document.querySelector("#refresh-generations")?.addEventListener("click", () => { void refreshGenerations(); });
document.querySelector("#check-vibevoice")?.addEventListener("click", () => { void (async () => {
  try {
    const request = await readSpeechRequest(), client = await clientReady;
    if (!client) throw new Error("The desktop MCP backend is unavailable.");
    status.textContent = "Checking VibeVoice voices, tokenizer, checkpoint and memory…";
    const result = requireToolBody(await client.call("studio_preflight", { ...request }));
    status.textContent = result.ready === true ? "VibeVoice preflight passed. Ready to generate the supplied script." : "VibeVoice preflight has blockers.";
  } catch (error) { status.textContent = error instanceof Error ? error.message : "VibeVoice preflight failed."; }
})(); });
void refreshGenerations();

function toolBody(payload: unknown): Record<string, unknown> | null {
  try { return requireToolBody(payload); }
  catch (error) {
    failUi(error, "tool result");
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
      ? `${safePhase}: verified speech WAV. ${detail}`
      : `${safePhase}: synthesizedSpeech is false. ${detail}`;
  });
  status.textContent = speech
    ? `${safePhase} for ${voice}. ${detail}`
    : `${safePhase} for ${voice}. synthesizedSpeech is false. ${detail}`;
}

function applyFlipcard(profile: Record<string, unknown>, reveal = true): void {
  const personaId = String(profile.personaId ?? "");
  if (!personaId) return;
  const card = board.querySelector<HTMLElement>(`.card[data-id="profile-${CSS.escape(personaId)}"]`);
  if (!card) return;
  const refs = Array.isArray(profile.refs) ? profile.refs.map((item) => String(item)).join(", ") : "";
  const slot = card.querySelector<HTMLElement>('[data-field="refs"]');
  if (slot && refs) slot.textContent = refs;
  if (reveal) setCardFlip(board, `profile-${personaId}`, true);
}

async function mcpCall(name: string, args: Record<string, unknown>): Promise<unknown | null> {
  try {
    const client = await clientReady;
    if (!client) throw new Error("Authenticated MCP is unavailable.");
    const payload = await client.call(name, args);
    requireToolBody(payload);
    return payload;
  } catch (error) {
    mcpFailures.note(name, error instanceof McpHttpError ? `http_${error.status}` : error instanceof Error ? error.name : "unknown");
    throw error;
  }
}

function applyControl(event: { seq?: number; op?: string; args?: Record<string, unknown> }): void {
  if (typeof event.seq === "number") {
    controlCursor = Math.max(controlCursor, event.seq);
    if (seenControl.has(event.seq)) return;
    seenControl.add(event.seq);
  }
  const args = event.args ?? {};
  // The browser already applied these local actions. Delayed echoes must never
  // undo a later Pause/Close; other MCP clients' commands still take effect.
  if (["play", "pause", "playback"].includes(event.op ?? "") && typeof args.clientRequestId === "string"
    && localPlaybackRequests.has(args.clientRequestId)) return;
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
    if (typeof args.face_id === "string") {
      if (!setCardFace(board, args.tileId, args.face_id)) reportAsync(resyncViewport(), "viewport faces");
    } else setCardFlip(board, args.tileId, args.flipped !== false);
    if (args.profile && typeof args.profile === "object") applyFlipcard(args.profile as Record<string, unknown>, typeof args.face_id !== "string");
  } else if (event.op === "flipcard" && args.profile && typeof args.profile === "object") {
    applyFlipcard(args.profile as Record<string, unknown>);
  } else if (event.op === "cube") {
    reportAsync(cubeMode.applyControl(args).then(reportCubeMode), "cube mode");
  } else if (event.op === "progress" && typeof args.voice === "string") {
    if (!generating) showRun(args.voice, String(args.phase ?? "unavailable"), String(args.detail ?? ""), false);
  } else if (event.op === "focus" && typeof args.uid === "string") {
    focusClipTile(args.uid);
    reportAsync(bindCubeToPlayingClip(args.uid), "cube focus");
  } else if (event.op === "play" && typeof args.playing === "string") {
    const origin = args.origin === "user" ? "user" : "auto";
    void playClip(args.playing, origin)
      .then((result) => {
        status.textContent = result === "playing" ? `Playing ${args.playing}` : result;
        syncCubeChrome();
      })
      .catch((error: unknown) => failUi(error, "play"));
  } else if (event.op === "playback" && typeof args.tileId === "string") {
    const action = String(args.action ?? "");
    if (action === "play") {
      void playClip(args.tileId, controlPlayOrigin(args))
        .then((result) => {
          status.textContent = result === "playing" ? `Playing ${args.tileId}` : result;
        })
        .catch((error: unknown) => failUi(error, "play"));
    }
    else if (action === "pause") status.textContent = pauseClip(args.tileId);
    else if (action === "seek") {
      const requested = Number(args.seconds);
      void seekClipOutcome(args.tileId, requested)
        .then((landing) => {
          if (landing.status !== "superseded") status.textContent = landing.status;
          // Tell MCP where it landed: ui_playback answers the agent with {requested_t, landed_t, ok, reason}.
          if (typeof event.seq === "number") {
            const report = seekReportControl(event.seq, requested, landing);
            reportAsync(mcpCall(report.name, report.args), "seek report");
          }
        })
        .catch((error: unknown) => failUi(error, "seek"));
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
    reportAsync(applyJob(args), "job");
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

function setDerived(parentUid: string, role: string, honesty: string): void {
  if (!parentUid || !honesty) return;
  const node = libraryTile(parentUid)?.querySelector<HTMLElement>(`[data-derived="${role}"]`);
  if (node) node.textContent = `${role}: ${honesty}`;
}

export async function mediaBlob(urlPath: string): Promise<string | null> {
  try {
    const client = await clientReady;
    if (!client) throw new Error("Authenticated MCP is unavailable.");
    const url = URL.createObjectURL(await client.libraryBlob(urlPath));
    mediaUrls.add(url);
    return url;
  } catch (error) {
    mcpFailures.note("library_media", error instanceof McpHttpError ? `http_${error.status}` : "error");
    failUi(error, "library media");
    return null;
  }
}

const mediaUrls = new Set<string>();

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

export async function insertGeneratedTile(uid: string, args: Record<string, unknown>): Promise<void> {
  const slide = board.querySelector<HTMLElement>('[data-slide="library"]');
  if (!slide) return;
  let resolved: Record<string, unknown> | null = null;
  let failure = "";
  try { resolved = requireToolBody(await mcpCall("asset_resolve", { uid })); }
  catch (error) { failure = error instanceof Error ? error.message : "asset_resolve failed"; }
  const resolveError = assetResolveFailure(resolved);
  if (resolveError || !resolved) {
    const message = noteAssetResolveFailure(uid, failure || resolveError || "asset_resolve failed", mcpFailures);
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
    const url = await mediaBlob(args.cubeUrl);
    if (url) {
      tile.dataset.cubeJson = url;
      if (!tile.querySelector("[data-generated-cube]")) {
        const open = document.createElement("button");
        open.type = "button";
        open.dataset.generatedCube = "1";
        open.textContent = "Open generated cube";
        open.addEventListener("click", () => reportAsync(openCube(url, legacy), "generated cube"));
        tile.append(open);
      }
      setDerived(uid, "cube", "real");
    }
  }
  if (typeof args.specUrl === "string" && args.specUrl.startsWith("/library/") && !tile.querySelector("[data-generated-spectrogram]")) {
    const url = await mediaBlob(args.specUrl);
    if (url) {
      const image = document.createElement("img");
      image.src = url;
      image.alt = "Spectrogram of this generated clip";
      image.dataset.generatedSpectrogram = "1";
      image.className = "spectrogram";
      tile.append(image);
      setDerived(uid, "spectrogram", "real");
    }
  }
  if (typeof args.videoUrl === "string") await mountGeneratedVideo(args.videoUrl);
}

async function applyJob(args: Record<string, unknown>): Promise<void> {
  const phase = String(args.phase ?? "");
  const voice = typeof args.voice === "string" ? args.voice : "";
  if (phase === "running") {
    if (voice) showRun(voice, "running", "generating", false);
    return;
  }
  const completed = generatedJob(args);
  if (completed?.kind === "clip") await insertGeneratedTile(completed.clipUid, args);
  if (completed?.kind === "spectrogram" || completed?.kind === "cube") {
    setDerived(completed.clipUid, completed.kind, "real");
  }
  if (typeof args.videoUrl === "string") await mountGeneratedVideo(args.videoUrl);
}

async function resyncViewport(): Promise<void> {
  await localPlaybackQueue;
  const playbackRevision = localPlaybackRevision;
  const payload = await mcpCall("viewport_get", {});
  const body = requireToolBody(payload);
  applyViewportViews(board, body.views);
  if (Number.isSafeInteger(body.cursor) && Number(body.cursor) >= 0) {
    controlCursor = Number(body.cursor);
    seenControl.clear();
  } else throw new Error("viewport_get returned no valid control cursor.");
  const ui = body.ui;
  if (ui && typeof ui === "object") {
    const record = ui as Record<string, unknown>;
    if (typeof record.slide === "string") goToSlideId(board, record.slide);
    if (typeof record.focus === "string") {
      focusClipTile(record.focus);
    } else if (record.focus === null) {
      board.querySelectorAll(".card.is-selected").forEach(node => node.classList.remove("is-selected"));
    }
    const clock = record.clock as { source?: unknown; t?: unknown } | undefined;
    if (typeof clock?.source === "string") {
      await bindCubeToPlayingClip(clock.source);
      if (!isSemanticCube()) setCubeClockClip(clock.source, clipUid(clock.source));
      if (typeof clock.t === "number" && Number.isFinite(clock.t)) {
        const landing = await seekClipOutcome(clock.source, clock.t);
        if (!landing.ok) failUi(new Error(landing.status), "viewport seek");
      }
    } else if (clock?.source === null) setCubeClockClip(null);
    for (const face of snapshotFaces(record)) {
      if (face.faceId) setCardFace(board, face.tileId, face.faceId);
      else setCardFlip(board, face.tileId, face.flipped);
    }
    if (playbackRevision === localPlaybackRevision && typeof record.playing === "string" && !isClipPlaying(record.playing)) {
      const result = await playClip(record.playing, "auto");
      if (result !== "playing") status.textContent = `Viewport playback: ${result}`;
    } else if (playbackRevision === localPlaybackRevision && record.playing === null) {
      const active = getActiveClipId();
      if (active && libraryTile(active)) pauseClip(active);
    }
    board.querySelectorAll<HTMLElement>(".card.is-compare").forEach((node) => node.classList.remove("is-compare"));
    if (Array.isArray(record.compare)) {
      for (const uid of record.compare) {
        if (typeof uid !== "string") continue;
        libraryTile(uid)?.classList.add("is-compare");
      }
    }
  }
  const cube = cubeStateFrom(body) ?? cubeStateFrom(body.ui);
  if (cube) reportCubeMode(await cubeMode.applyControl(cube));
  if (Array.isArray(body.jobs)) {
    for (const job of body.jobs) {
      if (!job || typeof job !== "object") continue;
      const record = job as { voice?: unknown; phase?: unknown; reason?: unknown; synthesized?: unknown };
      if (typeof record.voice === "string" && record.voice) {
        if (!generating) showRun(record.voice, String(record.phase ?? "unavailable"), String(record.reason ?? ""), false);
      }
    }
  }
}

const boardReady = loadShippedDocument()
  .then(refreshConnectors)
  .then(show)
  .catch((error: unknown) => failUi(error, "viewport"));

// Test hook (dev/test builds only): scrubs the cube and ONLY the clip the cube is bound to.
if (import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1") {
  (window as unknown as { __genAudioScrub?: (f: number) => Promise<string> }).__genAudioScrub = (fraction: number) => {
    setCubeScrub(fraction, { silent: true });
    return cubeClipId ? seekClipFraction(cubeClipId, fraction) : Promise.resolve("no cube clip");
  };
}
let controlConnected = false;
export async function connectControl(): Promise<void> {
  if (controlConnected) return;
  const client = await clientReady;
  if (!client) return;
  controlConnected = true;
  let closed = false;
  window.addEventListener("pagehide", () => {
    closed = true;
    outputRevision++;
    clearSpeechOutput();
    authoringPanel.setAudioSource(null);
    if (manifestObjectUrl) URL.revokeObjectURL(manifestObjectUrl);
    for (const url of mediaUrls) URL.revokeObjectURL(url);
    mediaUrls.clear();
    unbindCube();
  }, { once: true });
  await boardReady;
  let needsSnapshot = true;
  let lastRuntimeCheck = 0;
  while (!closed) {
    try {
      if (needsSnapshot) { await resyncViewport(); needsSnapshot = false; }
      const delta = await client.controls(controlCursor);
      if (closed) return;
      if (Date.now() - lastRuntimeCheck >= 5000) {
        lastRuntimeCheck = Date.now();
        engineLoader.observe(requireToolBody(await client.call("engine_status", {})));
      }
      if (controlNeedsResync(delta, controlCursor)) {
        await onControlGap();
      } else {
        for (const event of delta.events) applyControl(event);
        controlCursor = delta.cursor;
      }
      backendStatus.textContent = "";
      controlAttempt = 0;
      delete status.dataset.control;
    } catch (error) {
      engineLoader.unavailable();
      controlAttempt += 1;
      needsSnapshot = true;
      status.dataset.control = "disconnected";
      mcpFailures.note("control_stream", error instanceof McpHttpError ? `http_${error.status}` : "error");
      if (controlAttempt === 1) {
        failUi(error, "control stream");
        backendStatus.textContent = `MCP control connection unavailable; retrying with backoff. ${error instanceof Error ? error.message : ""}`;
      }
      if (error instanceof McpHttpError && (error.status === 401 || error.status === 403)) {
        backendStatus.textContent = "MCP authorization refused. Reopen the desktop to negotiate a new owned listener.";
        return;
      }
    }
    await new Promise((resolve) => window.setTimeout(resolve, controlAttempt ? controlBackoff(controlAttempt - 1) : 250));
  }
}

void connectControl().catch((error: unknown) => failUi(error, "control stream"));
