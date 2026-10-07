import example from "../../../schemas/examples/viewport.example.json";
import { bindCube, clearCube, loadCube, onCubeClock, rebuildLayerMatrixUi, setCubeScrub } from "./cubeview";
import { seekActiveFraction } from "./playback";
import { applyClipNames, harvestNames } from "./library-meta";
import { bindFloatingPlayback, pauseClip, playClip, seekClip } from "./playback";
import { profilePreview, type ProfilePreview, type VoiceSelection } from "./profiles";
import {
  bindSlideScroll,
  goToSlide,
  goToSlideId,
  moveFocus,
  moveSlide,
  paintProfileCanvases,
  renderBoard,
  setCardFlip,
  showRejected,
  slides,
} from "./render";
import { applySidepane, bindStudio, promptNote, readSelection, type StudioSelection } from "./studio";
import { drawCube, drawSpectrogram, makeFixture, play } from "./signal";
import { voiceById } from "./catalog";
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
  agents: ["alice"],
  voice: "kokoro_onnx",
  durationMin: 3,
};
let activePreview: ProfilePreview = profilePreview(selection);
let controlCursor = 0;
const seenControl = new Set<number>();

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
  paintProfile();
  paintProfileCanvases();
  bindCubeCanvas();
  bindRename();
  void harvestLibrary();
  bindFloatingPlayback();
  const n = slides(board).length;
  status.textContent = `${doc.cards.length} cards · ${n} snap slides · spectrogram follows side pane (preview, not speech)`;
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
  if (cube && cube.dataset.painted !== "fixture") {
    drawCube(cube, fixture);
    cube.dataset.painted = "fixture";
  }
  board.querySelectorAll<HTMLButtonElement>("[data-action='play-profile']").forEach((node) => {
    node.onclick = () => play(activePreview.before);
  });
  return before !== null;
}

function bindCubeCanvas(): void {
  const canvas = document.querySelector<HTMLCanvasElement>("#cube-viewport");
  if (!canvas || canvas.dataset.bound === "1") return;
  canvas.dataset.bound = "1";
  bindCube(canvas);
  rebuildLayerMatrixUi();
  document.querySelector("#cube-scrub")?.addEventListener("input", (event) => {
    const input = event.target as HTMLInputElement;
    const fraction = Number(input.value) / 1000;
    // ONE clock: scrubber seeks library audio AND slices cube layers.
    setCubeScrub(fraction);
    const seeked = seekActiveFraction(fraction);
    if (seeked !== "no player") status.textContent = `Shared clock ${Math.round(fraction * 100)}% · ${seeked}`;
  });
  onCubeClock((fraction) => {
    const fp = document.querySelector<HTMLInputElement>("#fp-scrub");
    if (fp && !fp.matches(":active")) fp.value = String(Math.round(fraction * 1000));
  });
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
  board.querySelectorAll<HTMLButtonElement>("[data-action='open-cube']").forEach((node) => {
    node.addEventListener("click", (event) => {
      event.stopPropagation();
      void openCube(node.dataset.cubeJson || "", "profile");
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
    clearCube("This tile has no cube JSON. Magpie, VibeVoice, and Pocket do not get a stand-in cloud.");
    return;
  }
  const message = await loadCube(url);
  if (caption) caption.textContent = `${source}: ${message}`;
  // F5 honesty: library-bound cubes must never keep a FIXTURE live-mark.
  const cubeCard = board.querySelector<HTMLElement>('.card[data-kind="Cube3D"]');
  if (cubeCard && !/did not load|no signal/i.test(message)) {
    cubeCard.dataset.cubeSource = "library";
    const live = cubeCard.querySelector<HTMLElement>(".live-mark");
    if (live) {
      live.textContent = "Library";
      live.dataset.source = "library";
    }
    const pill = cubeCard.querySelector<HTMLElement>(".pill");
    if (pill) {
      pill.textContent = "library-bound";
      pill.classList.remove("warn");
    }
    for (const node of Array.from(cubeCard.querySelectorAll<HTMLElement>(".summary"))) {
      if (/fixture|visual toy/i.test(node.textContent || "")) {
        node.textContent =
          "Interactive cube bound to library clip JSON - layers signal/tonality/confidence/quality.";
      }
    }
  }
  status.textContent = `F5 cube bound (${source}) - badge Library`;
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

document.querySelector("#show-example")?.addEventListener("click", () => {
  void refreshConnectors(example as ViewportDocument).then(show);
});
document.querySelector("#show-empty")?.addEventListener("click", () => {
  show({ version: "1.0", title: "Darbot Gen-Audio", columns: 3, cards: [] });
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
document.querySelector("#generate-podcast")?.addEventListener("click", () => {
  void submitGenerate();
});

document.querySelectorAll<HTMLButtonElement>("#layer-switch [data-layer]").forEach((button) => {
  button.addEventListener("click", () => {
    const layer = button.dataset.layer;
    const target =
      layer === "models" ? "models" : layer === "clips" ? "library" : layer === "video" ? "video" : layer === "cube" ? "spatial" : "";
    if (target) goToSlideId(board, target);
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
  try {
    const response = await fetch("http://127.0.0.1:8765/mcp", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: { name, arguments: args },
      }),
    });
    if (!response.ok) return null;
    const payload: unknown = await response.json();
    noteControlSeq(payload);
    return payload;
  } catch {
    return null;
  }
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
    goToSlideId(board, args.slide);
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
  } else if (event.op === "playback" && typeof args.tileId === "string") {
    const action = String(args.action ?? "");
    if (action === "play") void playClip(args.tileId).then((result) => {
      status.textContent = result === "playing" ? `Playing ${args.tileId}` : result;
    });
    else if (action === "pause") status.textContent = pauseClip(args.tileId);
    else if (action === "seek") status.textContent = seekClip(args.tileId, Number(args.seconds));
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

void refreshConnectors(example as ViewportDocument).then(show);
(window as unknown as { __genAudioScrub?: (f: number) => void }).__genAudioScrub = (fraction: number) => { setCubeScrub(fraction); seekActiveFraction(fraction); };
connectControl();
