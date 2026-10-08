import { profileForPersona } from "./profiles";
import { glyphBadge } from "./glyph";
import type { LibraryCatalog, ModelCubes } from "./library-assets";
import { emptyStrip } from "./livestrip";
import { renderTransport } from "./playback";
import { drawSpectrogram } from "./signal";
import { SnapAnimator, WheelGesture, stepIndex, type SlideKey } from "./snap";
import type { ViewportCard, ViewportDocument } from "./validate";

const EMPTY_COPY =
  "No cards in this viewport. Load the example for labeled fixture tiles and live connector probes — not a finished podcast product.";

export type SlideSchema = {
  id: string;
  title: string;
  blurb: string;
  columns: number;
  cardIds: string[];
  layer: string;
};

/** Snap slides. Layers group voice models, clips, and an honest empty video pane. */
export const SLIDE_SCHEMAS: SlideSchema[] = [
  {
    id: "models",
    title: "Voice models",
    blurb: "TTS and G2P models. Magpie, VibeVoice, and Pocket stay unavailable.",
    columns: 3,
    layer: "models",
    cardIds: ["engine-kokoro", "engine-kokoro-dayour", "engine-misaki", "engine-vibevoice", "engine-magpie", "engine-pocket"],
  },
  {
    id: "profiles",
    title: "Voice profiles",
    blurb: "Agent personas. Flip a tile for tone, purpose, domain, accent, traits, and refs.",
    columns: 3,
    layer: "models",
    cardIds: ["profile-anton", "profile-alice", "profile-khortana", "profile-rocky", "profile-optimus"],
  },
  {
    id: "studio",
    title: "Studio",
    blurb: "Browser spectrogram of the side-pane personas and voice model — not product speech",
    columns: 3,
    layer: "models",
    cardIds: ["spec-fixture"],
  },
  {
    id: "library",
    title: "Audio clips",
    blurb: "One livetile per clip — real WAVs play when the file loads; unavailable stays honest",
    columns: 3,
    layer: "clips",
    cardIds: [
      "lib-cube-explainer",
      "lib-kokoro-onnx",
      "lib-kokoro",
      "lib-misaki-kokoro",
      "lib-bitdot-braille-vibevoice",
      "lib-magpie",
      "lib-vibevoice",
      "lib-pocket",
    ],
  },
  {
    id: "connectors",
    title: "Connectors",
    blurb: "LLM connectors live here, not on the Agent control",
    columns: 3,
    layer: "connectors",
    cardIds: [
      "conn-acp",
      "conn-harness",
      "conn-copilot",
      "conn-claude",
      "conn-gpt",
      "conn-gemini",
    ],
  },
  {
    id: "pipeline",
    title: "Pipeline / Ready",
    blurb: "MCP + serve health + compare notes — readiness, not a completed render",
    columns: 3,
    layer: "pipeline",
    cardIds: ["conn-mcp", "serve-node", "serve-gateway", "bench-ref", "cube-fixture", "cast-sample"],
  },
];

export function showRejected(board: HTMLElement, empty: HTMLElement, error: string): void {
  stopLiveCycle(board);
  board.replaceChildren();
  board.hidden = true;
  empty.hidden = false;
  empty.textContent = `Viewport rejected: ${error}`;
  syncSlideChrome(0, 0);
}

const liveTimers = new WeakMap<HTMLElement, number>();

export function renderBoard(board: HTMLElement, empty: HTMLElement, document: ViewportDocument): void {
  stopLiveCycle(board);
  board.replaceChildren();
  empty.hidden = document.cards.length > 0;
  board.hidden = document.cards.length === 0;
  if (document.cards.length === 0) {
    empty.textContent = EMPTY_COPY;
    syncSlideChrome(0, 0);
    return;
  }

  const byId = new Map(document.cards.map((card) => [card.id, card]));
  const slides = SLIDE_SCHEMAS.map((schema) => {
    const cards = schema.cardIds.map((id) => byId.get(id)).filter((c): c is ViewportCard => Boolean(c));
    return { schema, cards };
  }).filter((slide) => slide.cards.length > 0);

  // Any leftover cards not claimed by a schema land on a catch-all slide.
  const claimed = new Set(slides.flatMap((s) => s.cards.map((c) => c.id)));
  const leftovers = document.cards.filter((c) => !claimed.has(c.id));
  if (leftovers.length > 0) {
    slides.push({
      schema: {
        id: "more",
        title: "More",
        blurb: "Additional cards from the viewport document",
        columns: document.columns ?? 3,
        layer: "more",
        cardIds: leftovers.map((c) => c.id),
      },
      cards: leftovers,
    });
  }

  let globalIndex = 0;
  slides.forEach((slide, slideIndex) => {
    const section = window.document.createElement("section");
    section.className = "slide";
    section.dataset.slide = slide.schema.id;
    section.dataset.layer = slide.schema.layer;
    section.dataset.slideIndex = String(slideIndex);
    section.style.setProperty("--cols", String(slide.schema.columns));
    section.setAttribute("aria-label", `Slide ${slideIndex + 1}: ${slide.schema.title}`);

    const banner = window.document.createElement("div");
    banner.className = "slide-banner";
    const heading = window.document.createElement("h3");
    heading.textContent = slide.schema.title;
    const blurb = window.document.createElement("p");
    blurb.textContent = slide.schema.blurb;
    banner.append(heading, blurb);
    section.append(banner);

    slide.cards.forEach((card) => {
      const article = window.document.createElement("article");
      article.className = "card livetile";
      article.tabIndex = globalIndex === 0 ? 0 : -1;
      article.dataset.id = card.id;
      article.dataset.kind = card.kind;
      article.dataset.slideIndex = String(slideIndex);
      if (card.kind === "LibraryClip") {
        article.classList.add("library-tile");
        if (card.body.synthesizedSpeech === true) article.dataset.hasWav = "1";
        article.dataset.wavUrl = typeof card.body.wavUrl === "string" ? card.body.wavUrl : "";
        article.dataset.cubeJson = typeof card.body.cubeJsonUrl === "string" ? card.body.cubeJsonUrl : "";
        article.dataset.sidecarUrl = typeof card.body.sidecarUrl === "string" ? card.body.sidecarUrl : "";
        article.dataset.faceName = card.title;
        article.dataset.semanticName = typeof card.body.semanticName === "string" ? card.body.semanticName : "";
      }
      if (card.kind === "VoiceProfile") {
        article.classList.add("profile-tile");
        article.dataset.personaId = String(card.body.personaId ?? "");
        article.dataset.voiceModel = String(card.body.voiceModel ?? "");
        article.dataset.cubeJson = typeof card.body.cubeJsonUrl === "string" ? card.body.cubeJsonUrl : "";
      }
      if (card.kind === "EngineStatus") {
        article.classList.add("engine-source");
        article.dataset.engineId = String(card.body.engineId ?? card.id);
        article.dataset.engineTitle = card.title;
      }
      const span = Math.min(card.span ?? 1, slide.schema.columns);
      if (span > 1) article.dataset.span = String(span);
      article.setAttribute("aria-label", `${card.kind}: ${card.title}`);

      const flip = window.document.createElement("div");
      flip.className = "flip";
      flip.append(frontFace(card), backFace(card));
      article.append(flip);
      article.addEventListener("keydown", (event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        const target = event.target as HTMLElement | null;
        if (target && target.closest("button, input, audio, select, textarea") && !target.classList.contains("flip-toggle")) return;
        event.preventDefault();
        toggleFlip(article);
      });
      section.append(article);
      globalIndex += 1;
    });

    board.append(section);
  });

  const extra = appendUtilitySlides(board, slides.length);
  const total = slides.length + extra;
  buildSlideDots(total);
  syncSlideChrome(0, total);
  startLiveCycle(board);
  board.scrollTop = 0;
}

function frontFace(card: ViewportCard): HTMLElement {
  const face = window.document.createElement("div");
  face.className = "face front";
  const row = window.document.createElement("div");
  row.className = "live-row";
  const kind = window.document.createElement("div");
  kind.className = "kind";
  kind.textContent = card.kind;
  const live = window.document.createElement("span");
  live.className = "live-mark";
  live.textContent = liveLabel(card);
  row.append(kind, live);
  // Asset object model v1: every card shows its uid glyph (uid on hover, copy on click).
  const badge = glyphBadge(card.uid, { onCopy: announceCopy });
  if (badge) row.append(badge);
  const title = window.document.createElement("h2");
  title.textContent = card.title;
  if (card.kind === "SpectrogramPanel") title.classList.add("spec-title");
  const flip = button("Flip");
  flip.className = "flip-toggle";
  flip.setAttribute("aria-pressed", "false");
  flip.addEventListener("click", (event) => {
    event.stopPropagation();
    const article = flip.closest(".card");
    if (article instanceof HTMLElement) toggleFlip(article);
  });
  face.append(row, title, bodyFor(card, card.kind, card.body), flip);
  return face;
}

/** Honest badge: fixture tiles are Fixture, not "Live product speech". */
function liveLabel(card: ViewportCard): string {
  if (card.kind === "Cube3D") {
    if (card.body.datasetBound === "library" || card.body.source === "library-clip") return "Library";
    return "Fixture";
  }
  if (card.kind === "SpectrogramPanel") return "Fixture";
  if (card.kind === "PodcastCast") return "Sample";
  if (card.kind === "BenchmarkCompare") return "Ref only";
  if (card.kind === "EngineStatus") {
    const status = String(card.body.status ?? "");
    if (status === "implemented") return "Engine";
    return status || "Status";
  }
  if (card.kind === "ConnectorStatus") {
    return card.body.mode === "live" || card.body.mode === "local" ? "Probed" : String(card.body.mode ?? "Status");
  }
  if (card.kind === "ServeHealth") {
    if (card.body.probed !== true) return "Unprobed";
    return card.body.ok === true ? "Up" : "Down";
  }
  if (card.kind === "LibraryClip") {
    if (card.body.synthesizedSpeech === true) return "Real WAV";
    return String(card.body.status ?? "Status");
  }
  if (card.kind === "VoiceProfile") return "Profile";
  return "Tile";
}

function backFace(card: ViewportCard): HTMLElement {
  const face = window.document.createElement("div");
  face.className = "face back";
  const kind = window.document.createElement("div");
  kind.className = "kind";
  kind.textContent = "Adaptive card";
  const title = window.document.createElement("h2");
  title.textContent = card.title;
  if (card.kind === "VoiceProfile") face.append(voiceProfileBack(card));
  else if (card.kind === "EngineStatus") face.append(engineBack(card));
  else face.append(adaptiveFace(card));
  if (card.kind === "LibraryClip") face.append(renameBlock(card));
  const flip = button("Show front");
  flip.className = "flip-toggle";
  flip.addEventListener("click", (event) => {
    event.stopPropagation();
    const article = flip.closest(".card");
    if (article instanceof HTMLElement) toggleFlip(article);
  });
  face.append(flip);
  return face;
}

function toggleFlip(article: HTMLElement): void {
  setCardFlipState(article, !article.classList.contains("is-flipped"));
}

export function setCardFlip(board: HTMLElement, tileId: string, flipped: boolean): boolean {
  const tile = board.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(tileId)}"]`);
  if (!tile) return false;
  setCardFlipState(tile, flipped);
  return true;
}

function setCardFlipState(article: HTMLElement, flipped: boolean): void {
  article.classList.toggle("is-flipped", flipped);
  article.querySelectorAll<HTMLButtonElement>(".flip-toggle").forEach((node) => {
    node.setAttribute("aria-pressed", flipped ? "true" : "false");
  });
}

function syncLayerTabs(slideId: string | undefined): void {
  const layer =
    slideId === "models" ? "models" : slideId === "library" ? "clips" : slideId === "video" ? "video" : slideId === "spatial" ? "cube" : "";
  window.document.querySelectorAll<HTMLButtonElement>("#layer-switch [data-layer]").forEach((button) => {
    button.setAttribute("aria-selected", button.dataset.layer === layer ? "true" : "false");
  });
}

function startLiveCycle(board: HTMLElement): void {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  let cursor = 0;
  const timer = window.setInterval(() => {
    const slide = currentSlide(board);
    const tiles = slide
      ? Array.from(slide.querySelectorAll<HTMLElement>(".livetile"))
      : Array.from(board.querySelectorAll<HTMLElement>(".livetile"));
    // Auto-flip only engine/connector tiles — never fixture theater.
    const idle = tiles.filter(
      (tile) =>
        !tile.matches(":hover") &&
        !tile.matches(":focus-within") &&
        tile.dataset.kind !== "SpectrogramPanel" &&
        tile.dataset.kind !== "Cube3D" &&
        tile.dataset.kind !== "PodcastCast" &&
        tile.dataset.kind !== "LibraryClip" &&
        tile.dataset.kind !== "VoiceProfile",
    );
    if (idle.length === 0) return;
    toggleFlip(idle[cursor % idle.length]);
    cursor += 1;
  }, 7000);
  liveTimers.set(board, timer);
}

function stopLiveCycle(board: HTMLElement): void {
  const timer = liveTimers.get(board);
  if (timer !== undefined) window.clearInterval(timer);
  liveTimers.delete(board);
}

function bodyFor(card: ViewportCard, kind: string, body: Record<string, unknown>): HTMLElement {
  const wrap = window.document.createElement("div");
  if (kind === "EngineStatus") {
    wrap.append(pill(String(body.status), body.status !== "implemented"));
    wrap.append(paragraph(String(body.summary ?? "")));
    if (body.synthesizedSpeech !== true) {
      wrap.append(paragraph("No synthesized speech claimed — status only."));
    }
  } else if (kind === "SpectrogramPanel") {
    wrap.append(pill("profile preview", true));
    const note = paragraph(String(body.disclaimer ?? ""));
    note.classList.add("spec-disclaimer");
    wrap.append(note);
    wrap.append(paragraph("Browser profile map — not synthesized speech."));
    const before = canvas("spec-before");
    const after = canvas("spec-after");
    const play = button("Play profile preview");
    play.dataset.action = "play-profile";
    wrap.append(before, after, play);
  } else if (kind === "Cube3D") {
    const bound = body.datasetBound === "library" || body.source === "library-clip";
    wrap.append(pill(bound ? "library-bound" : "fixture theater", !bound));
    wrap.append(paragraph(String(body.disclaimer ?? "")));
    wrap.append(paragraph(bound
      ? "Interactive cube bound to library clip JSON - layers signal/tonality/confidence/quality."
      : "Visual toy only until a real Library clip binds cube JSON. Not a render pipeline output."));
    const cube = canvas("cube");
    if (body.cubeJsonUrl) cube.dataset.cubeJsonUrl = String(body.cubeJsonUrl);
    wrap.append(cube);
  } else if (kind === "PodcastCast") {
    wrap.append(pill("sample script", true));
    wrap.append(paragraph("Sample script only. Speaker names are personas. af_heart and am_michael are Kokoro pack ids, not the Voice selector. synthesizedSpeech is false here."));
    const list = window.document.createElement("ul");
    const speakers = Array.isArray(body.speakers) ? body.speakers : [];
    for (const speaker of speakers) {
      if (!speaker || typeof speaker !== "object") continue;
      const row = speaker as Record<string, unknown>;
      const item = window.document.createElement("li");
      item.textContent = `Speaker ${row.id}: ${row.name} → ${row.voice}`;
      list.append(item);
    }
    wrap.append(list);
  } else if (kind === "ServeHealth") {
    const probed = body.probed === true;
    const ok = body.ok === true;
    const label = !probed ? "not probed" : ok ? "reachable" : "unreachable";
    wrap.append(pill(label, !probed || !ok));
    wrap.append(paragraph(String(body.healthUrl ?? "")));
    wrap.append(paragraph(`Role: ${String(body.role ?? "")}`));
  } else if (kind === "BenchmarkCompare") {
    wrap.append(pill("not remeasured", true));
    wrap.append(paragraph(String(body.sourceNote ?? "")));
    const table = window.document.createElement("table");
    const head = window.document.createElement("tr");
    for (const label of ["Engine", "Metric", "Value"]) {
      const cell = window.document.createElement("th");
      cell.textContent = label;
      head.append(cell);
    }
    table.append(head);
    const rows = Array.isArray(body.rows) ? body.rows : [];
    for (const row of rows) {
      if (!row || typeof row !== "object") continue;
      const record = row as Record<string, unknown>;
      const tr = window.document.createElement("tr");
      for (const key of ["engine", "metric", "value"]) {
        const cell = window.document.createElement("td");
        cell.textContent = String(record[key] ?? "");
        tr.append(cell);
      }
      table.append(tr);
    }
    wrap.append(table);
  } else if (kind === "LibraryClip") {
    const speech = body.synthesizedSpeech === true;
    const status = String(body.status ?? "unavailable");
    wrap.append(pill(speech ? "real synth" : status, !speech));
    const semantic = window.document.createElement("p");
    semantic.className = "summary semantic-name";
    semantic.textContent = typeof body.semanticName === "string" && body.semanticName ? body.semanticName : String(body.summary ?? "");
    wrap.append(semantic);
    if (speech) {
      const dur = typeof body.duration_s === "number" ? body.duration_s : undefined;
      const sr = String(body.sample_rate ?? "?");
      wrap.append(paragraph((dur ? dur.toFixed(1) : "?") + "s · " + sr + " Hz"));
      wrap.append(renderTransport(card.id, typeof body.wavUrl === "string" ? body.wavUrl : undefined, dur));
      // Filled from /library/assets.json once it loads (spectrogram strip + clip glyph).
      const slot = window.document.createElement("div");
      slot.className = "spec-strip-slot";
      wrap.append(slot);
      if (body.cubePngUrl) {
        const img = window.document.createElement("img");
        img.className = "cube-thumb";
        img.alt = "Inverse-HDR cube layers";
        img.src = String(body.cubePngUrl);
        img.loading = "lazy";
        wrap.append(img);
      }
      if (typeof body.inv_hdr === "number") {
        wrap.append(paragraph("inv-HDR " + body.inv_hdr.toFixed(3) + " · layers signal/tonality/confidence/quality"));
      }
    } else {
      wrap.append(paragraph("No WAV — honest empty tile."));
      wrap.append(renderTransport(card.id, undefined));
      wrap.append(emptyStrip("this clip has no WAV, so there is nothing to analyze."));
    }
  } else if (kind === "VoiceProfile") {
    wrap.append(pill(String(body.voiceModel ?? "voice"), false));
    wrap.append(paragraph(String(body.tone ?? "")));
    wrap.append(paragraph(String(body.purpose ?? "")));
  } else if (kind === "ConnectorStatus") {
    wrap.append(pill(String(body.mode), body.mode === "mock" || body.mode === "token_present"));
    wrap.append(paragraph(String(body.detail ?? "")));
    if (body.authenticated === true) wrap.append(paragraph("Authenticated: yes"));
    else wrap.append(paragraph("Authenticated: no"));
  }
  return wrap;
}

export function announceCopy(uid: string, copied: boolean): void {
  const status = window.document.querySelector<HTMLElement>("#status");
  if (status) status.textContent = copied ? `Copied ${uid}` : `Clipboard unavailable. uid: ${uid}`;
}

function pill(text: string, warn: boolean): HTMLElement {
  const node = window.document.createElement("span");
  node.className = warn ? "pill warn" : "pill";
  node.textContent = text;
  return node;
}

function paragraph(text: string): HTMLElement {
  const node = window.document.createElement("p");
  node.className = "summary";
  node.textContent = text;
  return node;
}

function canvas(name: string): HTMLCanvasElement {
  const node = window.document.createElement("canvas");
  node.dataset.canvas = name;
  return node;
}

function button(label: string): HTMLButtonElement {
  const node = window.document.createElement("button");
  node.type = "button";
  node.textContent = label;
  return node;
}

function adaptiveFace(card: ViewportCard): HTMLElement {
  const wrap = window.document.createElement("div");
  wrap.className = "adaptive-body";
  const blocks = card.adaptive?.type === "AdaptiveCard" && Array.isArray(card.adaptive.body)
    ? card.adaptive.body
    : [];
  let wrote = false;
  for (const block of blocks) {
    if (!block || typeof block !== "object") continue;
    const record = block as Record<string, unknown>;
    if (record.type === "TextBlock" && typeof record.text === "string") {
      wrap.append(paragraph(record.text));
      wrote = true;
    } else if (record.type === "FactSet" && Array.isArray(record.facts)) {
      const list = window.document.createElement("ul");
      for (const fact of record.facts) {
        if (!fact || typeof fact !== "object") continue;
        const row = fact as Record<string, unknown>;
        const item = window.document.createElement("li");
        item.textContent = `${String(row.title ?? "")}: ${String(row.value ?? "")}`;
        list.append(item);
      }
      if (list.childElementCount > 0) {
        wrap.append(list);
        wrote = true;
      }
    }
  }
  if (!wrote) {
    wrap.append(paragraph(fallbackBack(card.kind, card.body)));
  }
  return wrap;
}

/**
 * Voice model (EngineStatus) back face: a Status tab (the adaptive card) and a
 * Cube tab listing the engine's real cubes (E4). The Cube tab reads "pending"
 * until /library/assets.json loads; fillModelCubes then fills it.
 */
function engineBack(card: ViewportCard): HTMLElement {
  const engineId = String(card.body.engineId ?? card.id);
  const wrap = window.document.createElement("div");
  wrap.className = "engine-back";
  const list = window.document.createElement("div");
  list.className = "back-tabs";
  list.setAttribute("role", "tablist");
  list.setAttribute("aria-label", `${card.title} details`);
  const panels: HTMLElement[] = [];
  const tabs: HTMLButtonElement[] = [];
  const select = (index: number, focus: boolean) => {
    tabs.forEach((tab, i) => {
      tab.setAttribute("aria-selected", String(i === index));
      tab.tabIndex = i === index ? 0 : -1;
      panels[i].hidden = i !== index;
    });
    if (focus) tabs[index].focus();
  };
  const status = adaptiveFace(card);
  const cube = window.document.createElement("div");
  cube.className = "model-cubes";
  cube.dataset.modelCubes = engineId;
  cube.append(paragraph("pending"));
  ([["Status", status], ["Cube", cube]] as const).forEach(([label, panel], index) => {
    const id = `${card.id}-tab-${label.toLowerCase()}`;
    const tab = button(label);
    tab.id = id;
    tab.dataset.tab = label.toLowerCase();
    tab.setAttribute("role", "tab");
    tab.setAttribute("aria-controls", `${id}-panel`);
    tab.addEventListener("click", (event) => {
      event.stopPropagation();
      select(index, false);
    });
    tab.addEventListener("keydown", (event) => {
      const next = event.key === "ArrowRight" ? index + 1 : event.key === "ArrowLeft" ? index - 1 : event.key === "Home" ? 0 : event.key === "End" ? 1 : null;
      if (next === null) return;
      event.preventDefault();
      event.stopPropagation();
      select((next + 2) % 2, true);
    });
    panel.id = `${id}-panel`;
    panel.setAttribute("role", "tabpanel");
    panel.setAttribute("aria-labelledby", id);
    tabs.push(tab);
    panels.push(panel);
    list.append(tab);
  });
  wrap.append(list, ...panels);
  select(0, false);
  return wrap;
}

export interface ModelCubeActions {
  openCube(jsonUrl: string, source: string): void;
  selectTile(tileId: string): void;
}

const fixed = (value: number | null, digits: number) => (value === null ? "pending" : value.toFixed(digits));

/** Fill every voice model card's Cube tab from the asset catalog (E4). */
export function fillModelCubes(board: HTMLElement, catalog: LibraryCatalog | null, actions: ModelCubeActions): void {
  board.querySelectorAll<HTMLElement>("[data-model-cubes]").forEach((panel) => {
    const view: ModelCubes = catalog ? catalog.modelCubes(panel.dataset.modelCubes ?? "") : { state: "unknown" };
    panel.dataset.state = view.state;
    panel.replaceChildren();
    if (view.state === "unknown") {
      panel.append(paragraph("pending"));
    } else if (view.state === "none") {
      panel.append(paragraph("no cube: no Library clip from this engine yet"));
    } else if (view.state === "unavailable") {
      panel.append(paragraph(`no cube: engine unavailable (${view.reason})`));
    } else if (view.state === "g2p") {
      const first = view.clips[0];
      panel.append(paragraph(first ? `G2P only, see ${first.clipTitle}` : "G2P only: no clip yet"));
      for (const clip of view.clips) {
        const link = button(`Open ${clip.clipTitle}`);
        link.dataset.action = "select-tile";
        link.dataset.tileId = clip.clipId;
        link.addEventListener("click", (event) => {
          event.stopPropagation();
          actions.selectTile(clip.clipId);
        });
        panel.append(link);
      }
    } else {
      if (view.offline) panel.append(paragraph("Offline run: rendered outside this app; the engine has no adapter here."));
      const table = window.document.createElement("table");
      table.className = "model-cube-table";
      const head = window.document.createElement("tr");
      for (const label of ["Clip", "Cube uid", "rev", "shape", "inv-HDR", "layer score", ""]) {
        const th = window.document.createElement("th");
        th.textContent = label;
        head.append(th);
      }
      table.append(head);
      for (const row of view.rows) {
        const tr = window.document.createElement("tr");
        tr.dataset.cubeUid = row.cubeUid;
        const cells = [
          view.offline ? `${row.clipTitle} (offline run)` : row.clipTitle,
          row.cubeUid,
          row.revision === null ? "pending" : String(row.revision),
          row.shape ? `${row.shape[0]}\u00d7${row.shape[1]}` : "pending",
          fixed(row.invHdr, 4),
          fixed(row.layerScore, 4),
        ];
        cells.forEach((text, i) => {
          const td = window.document.createElement("td");
          td.textContent = text;
          if (i === 1) td.className = "mono";
          tr.append(td);
        });
        const open = button("Open 3D cube");
        open.dataset.action = "open-cube";
        open.dataset.cubeJson = row.jsonUrl;
        open.addEventListener("click", (event) => {
          event.stopPropagation();
          actions.openCube(row.jsonUrl, row.clipId);
        });
        const td = window.document.createElement("td");
        td.append(open);
        tr.append(td);
        table.append(tr);
      }
      panel.append(table);
    }
  });
}

function fallbackBack(kind: string, body: Record<string, unknown>): string {
  if (kind === "EngineStatus") return `${String(body.engineId ?? "engine")} is ${String(body.status ?? "unknown")}. No speech claim.`;
  if (kind === "ConnectorStatus") return `${String(body.connectorId ?? "connector")} mode ${String(body.mode ?? "unknown")}.`;
  if (kind === "ServeHealth") return String(body.healthUrl ?? "Health URL is not set.");
  if (kind === "PodcastCast") return "Sample cast only. synthesizedSpeech is false.";
  if (kind === "BenchmarkCompare") return String(body.sourceNote ?? "Scores are not remeasured in this window.");
  if (kind === "LibraryClip") {
    if (body.synthesizedSpeech === true) {
      return "Real clip " + String(body.duration_s ?? "?") + "s — " + String(body.wavUrl ?? "");
    }
    return String(body.engineId ?? "engine") + ": " + String(body.status ?? "unavailable") + " — no fake audio.";
  }
  return String(body.disclaimer ?? "Fixture face. Not a podcast render.");
}

function voiceProfileBack(card: ViewportCard): HTMLElement {
  const wrap = window.document.createElement("div");
  wrap.className = "profile-back";
  const body = card.body;
  const rows: Array<[string, string]> = [
    ["Agent", String(body.agentName ?? "")],
    ["Voice model", String(body.voiceModel ?? "")],
    ["Tone", String(body.tone ?? "")],
    ["Purpose", String(body.purpose ?? "")],
    ["Domain", String(body.domain ?? "")],
    ["Accent", String(body.accent ?? "")],
    ["Traits", String(body.traits ?? "")],
    ["Refs", Array.isArray(body.refs) ? body.refs.map((item) => String(item)).join(", ") : ""],
    ["2D spectrogram", String(body.spectrogram2d ?? "")],
    ["3D spectrogram", String(body.spectrogram3d ?? "none")],
  ];
  const list = window.document.createElement("dl");
  list.className = "profile-schema";
  for (const [label, value] of rows) {
    const term = window.document.createElement("dt");
    term.textContent = label;
    const detail = window.document.createElement("dd");
    detail.dataset.field = label.toLowerCase().replace(/\s+/g, "-");
    detail.textContent = value;
    list.append(term, detail);
  }
  wrap.append(list);
  wrap.append(paragraph(String(body.disclaimer ?? "")));
  const canvas = window.document.createElement("canvas");
  canvas.dataset.canvas = "profile-spec";
  canvas.dataset.personaId = String(body.personaId ?? "");
  canvas.dataset.voiceModel = String(body.voiceModel ?? "");
  wrap.append(canvas);
  const open = button(body.spectrogram3d === "library-cube-hook" ? "Open 3D cube" : "No library cube");
  open.dataset.action = "open-cube";
  open.dataset.cubeJson = typeof body.cubeJsonUrl === "string" ? body.cubeJsonUrl : "";
  open.disabled = body.spectrogram3d !== "library-cube-hook";
  wrap.append(open);
  return wrap;
}

export function paintProfileCanvases(): void {
  document.querySelectorAll<HTMLCanvasElement>('[data-canvas="profile-spec"]').forEach((canvas) => {
    const preview = profileForPersona(canvas.dataset.personaId || "alice", canvas.dataset.voiceModel || "kokoro_onnx");
    drawSpectrogram(canvas, preview.before, preview.beforeTitle);
  });
}

function renameBlock(card: ViewportCard): HTMLElement {
  const wrap = window.document.createElement("div");
  wrap.className = "rename-block";
  const note = window.document.createElement("p");
  note.className = "summary harvest-note";
  note.textContent = "Names harvest from the filename and sidecar counts. Audio is not decoded. Attach writes a clip ref on a persona in this process only.";
  const semantic = window.document.createElement("input");
  semantic.type = "text";
  semantic.dataset.field = "semantic";
  semantic.maxLength = 80;
  semantic.placeholder = "Semantic name";
  semantic.setAttribute("aria-label", `Semantic name for ${card.id}`);
  semantic.value = typeof card.body.semanticName === "string" ? card.body.semanticName : "";
  const face = window.document.createElement("input");
  face.type = "text";
  face.dataset.field = "face";
  face.maxLength = 80;
  face.placeholder = "Face / display name";
  face.setAttribute("aria-label", `Face name for ${card.id}`);
  face.value = typeof card.body.faceName === "string" ? card.body.faceName : card.title;
  const apply = button("Apply names");
  apply.dataset.action = "rename-clip";
  apply.dataset.clipId = card.id;
  const attach = button("Attach to profile");
  attach.dataset.action = "attach-profile";
  attach.dataset.clipId = card.id;
  wrap.append(note, semantic, face, apply, attach);
  return wrap;
}

function appendUtilitySlides(board: HTMLElement, startIndex: number): number {
  const video = window.document.createElement("section");
  video.className = "slide";
  video.dataset.slide = "video";
  video.dataset.layer = "video";
  video.dataset.slideIndex = String(startIndex);
  video.setAttribute("aria-label", "Video layer");
  const videoBanner = window.document.createElement("div");
  videoBanner.className = "slide-banner";
  const videoTitle = window.document.createElement("h3");
  videoTitle.textContent = "Video";
  const videoCopy = window.document.createElement("p");
  videoCopy.textContent = "No video clips in this library. This layer is reserved. Nothing here is a podcast.";
  videoBanner.append(videoTitle, videoCopy);
  video.append(videoBanner);
  board.append(video);

  const spatial = window.document.createElement("section");
  spatial.className = "slide spatial-slide";
  spatial.dataset.slide = "spatial";
  spatial.dataset.layer = "cube";
  spatial.dataset.slideIndex = String(startIndex + 1);
  spatial.setAttribute("aria-label", "Spatial cube: Inverse-HDR cube from library cube JSON");

  // The WebGL cube is the whole card. Everything else is a translucent overlay.
  const stage = window.document.createElement("div");
  stage.className = "cube-stage";
  const canvas = window.document.createElement("canvas");
  canvas.id = "cube-viewport";
  canvas.dataset.canvas = "cube-viewport";
  canvas.setAttribute("aria-label", "Inverse-HDR cube. Drag to rotate, wheel to zoom.");
  const labels = window.document.createElement("canvas");
  labels.id = "cube-labels";
  labels.className = "cube-labels";
  labels.setAttribute("aria-hidden", "true");

  const title = window.document.createElement("p");
  title.id = "cube-title";
  title.className = "cube-overlay cube-title";
  title.textContent = "Inverse-HDR cube";

  const legend = window.document.createElement("ul");
  legend.className = "cube-overlay cube-legend";
  legend.setAttribute("aria-label", "Cube layer legend");
  for (const id of ["signal", "tonality", "confidence", "quality"]) {
    const item = window.document.createElement("li");
    item.dataset.layer = id;
    item.textContent = id;
    legend.append(item);
  }

  const picker = window.document.createElement("div");
  picker.className = "cube-overlay cube-picker";
  const select = window.document.createElement("select");
  select.id = "cube-source";
  select.setAttribute("aria-label", "Library cube source");
  const play = window.document.createElement("button");
  play.type = "button";
  play.id = "cube-play";
  play.textContent = "Play";
  play.setAttribute("aria-label", "Play the clip bound to this cube");
  const layersToggle = window.document.createElement("button");
  layersToggle.type = "button";
  layersToggle.id = "cube-matrix-toggle";
  layersToggle.className = "cube-matrix-toggle";
  layersToggle.textContent = "Layers \u25be";
  layersToggle.setAttribute("aria-expanded", "true");
  layersToggle.setAttribute("aria-controls", "cube-layer-matrix");
  const cubeGlyph = window.document.createElement("span");
  cubeGlyph.id = "cube-glyph";
  cubeGlyph.className = "cube-glyph";
  picker.append(cubeGlyph, select, play, layersToggle);

  const badge = window.document.createElement("p");
  badge.id = "cube-badge";
  badge.className = "cube-overlay cube-badge";

  const clock = window.document.createElement("p");
  clock.id = "cube-clock";
  clock.className = "cube-overlay cube-clock";
  clock.setAttribute("aria-live", "off");

  const fallback = window.document.createElement("img");
  fallback.id = "cube-fallback";
  fallback.className = "cube-fallback";
  fallback.hidden = true;
  fallback.alt = "Static cube image";

  const caption = window.document.createElement("p");
  caption.id = "cube-caption";
  caption.className = "cube-overlay cube-caption";
  caption.textContent = "Loading the library cube JSON.";

  const matrix = window.document.createElement("div");
  matrix.id = "cube-layer-matrix";
  matrix.className = "cube-overlay cube-layer-matrix";
  matrix.setAttribute("aria-label", "Cube layer matrix: opacity, gain, blend, order");

  const scrub = window.document.createElement("input");
  scrub.type = "range";
  scrub.id = "cube-scrub";
  scrub.className = "cube-overlay";
  scrub.min = "0";
  scrub.max = "1000";
  scrub.value = "0";
  scrub.setAttribute("aria-label", "Shared cube and audio clock");

  stage.append(canvas, labels, fallback, title, badge, legend, picker, caption, clock, matrix, scrub);
  spatial.append(stage);
  board.append(spatial);
  return 2;
}

export function goToSlideId(board: HTMLElement, id: string): boolean {
  const index = slides(board).findIndex((slide) => slide.dataset.slide === id);
  if (index < 0) return false;
  goToSlide(board, index);
  return true;
}

export function cards(board: HTMLElement): HTMLElement[] {
  return Array.from(board.querySelectorAll<HTMLElement>(".card"));
}

export function slides(board: HTMLElement): HTMLElement[] {
  return Array.from(board.querySelectorAll<HTMLElement>(".slide"));
}

export function currentSlideIndex(board: HTMLElement): number {
  const list = slides(board);
  if (list.length === 0) return 0;
  const mid = board.scrollTop + board.clientHeight / 2;
  let best = 0;
  let bestDist = Number.POSITIVE_INFINITY;
  list.forEach((slide, index) => {
    const center = slide.offsetTop + slide.offsetHeight / 2;
    const dist = Math.abs(center - mid);
    if (dist < bestDist) {
      bestDist = dist;
      best = index;
    }
  });
  return best;
}

export function currentSlide(board: HTMLElement): HTMLElement | null {
  const list = slides(board);
  return list[currentSlideIndex(board)] ?? null;
}

const animators = new WeakMap<HTMLElement, SnapAnimator>();

function animatorFor(board: HTMLElement): SnapAnimator {
  let animator = animators.get(board);
  if (!animator) {
    animator = new SnapAnimator(board, (index) => syncSlideChrome(index, slides(board).length));
    animators.set(board, animator);
  }
  return animator;
}

/** True while a slide animation is running (input lock). */
export function isSnapping(board: HTMLElement): boolean {
  return animatorFor(board).locked;
}

/**
 * Jump or glide to a slide by its position in the DOM slide list. Dots, label and
 * layer tabs update to the target immediately and again when the glide lands.
 */
export function goToSlide(board: HTMLElement, index: number, options: { animate?: boolean } = {}): void {
  const list = slides(board);
  if (list.length === 0) return;
  const next = Math.min(list.length - 1, Math.max(0, index));
  const slide = list[next];
  const slideCards = Array.from(slide.querySelectorAll<HTMLElement>(".card"));
  cards(board).forEach((item) => {
    item.tabIndex = -1;
  });
  syncSlideChrome(next, list.length);
  syncLayerTabs(slide.dataset.slide);
  animatorFor(board).go(slide, next, options.animate === true);
  if (slideCards[0]) {
    slideCards[0].tabIndex = 0;
    slideCards[0].focus({ preventScroll: true });
  }
}

/** One slide per call (keys, wheel); ignored while the previous glide is still running. */
export function moveSlide(board: HTMLElement, direction: 1 | -1 | Exclude<SlideKey, null>): boolean {
  const list = slides(board);
  if (list.length === 0) return false;
  const animator = animatorFor(board);
  if (animator.locked) return false;
  const current = currentSlideIndex(board);
  const target = stepIndex(current, direction, list.length);
  if (target === current) return false;
  goToSlide(board, target, { animate: true });
  return true;
}

export function moveFocus(board: HTMLElement, direction: 1 | -1 | "home" | "end"): void {
  const slide = currentSlide(board);
  const items = slide ? Array.from(slide.querySelectorAll<HTMLElement>(".card")) : cards(board);
  if (items.length === 0) return;
  const active = window.document.activeElement;
  const current = items.findIndex((item) => item === active || item.contains(active));
  let next = 0;
  if (direction === "home") next = 0;
  else if (direction === "end") next = items.length - 1;
  else next = Math.min(items.length - 1, Math.max(0, (current < 0 ? 0 : current) + direction));
  cards(board).forEach((item) => {
    item.tabIndex = -1;
  });
  items[next].tabIndex = 0;
  items[next].focus();
  items[next].scrollIntoView({ block: "nearest" });
}

function buildSlideDots(count: number): void {
  const host = window.document.querySelector<HTMLElement>("#slide-dots");
  if (!host) return;
  host.replaceChildren();
  for (let i = 0; i < count; i += 1) {
    const dot = window.document.createElement("button");
    dot.type = "button";
    dot.className = "slide-dot";
    dot.setAttribute("role", "tab");
    dot.setAttribute("aria-label", `Go to slide ${i + 1}`);
    dot.dataset.slideIndex = String(i);
    dot.addEventListener("click", () => {
      const board = window.document.querySelector<HTMLElement>("#board");
      if (board) goToSlide(board, i, { animate: true });
    });
    host.append(dot);
  }
}

export function syncSlideChrome(index: number, total: number): void {
  const label = window.document.querySelector<HTMLElement>("#slide-label");
  if (label) {
    label.textContent = total > 0 ? `Slide ${index + 1}/${total}` : "Slide —";
  }
  const host = window.document.querySelector<HTMLElement>("#slide-dots");
  host?.querySelectorAll<HTMLButtonElement>(".slide-dot").forEach((dot, i) => {
    dot.setAttribute("aria-selected", i === index ? "true" : "false");
  });
  const board = window.document.querySelector<HTMLElement>("#board");
  const layer = board ? slides(board)[index]?.dataset.layer : undefined;
  window.document.body.classList.toggle("cube-active", layer === "cube");
  window.document.querySelectorAll<HTMLButtonElement>("#layer-switch [data-layer]").forEach((button) => {
    button.setAttribute("aria-selected", button.dataset.layer === layer ? "true" : "false");
  });
}

/** Elements that keep their own wheel (cube zoom, range sliders, scrollable lists). */
function ownsWheel(target: EventTarget | null, deltaY: number): boolean {
  let node = target instanceof Element ? target : null;
  while (node && !node.classList.contains("board")) {
    if (node.matches("select, input[type='range'], textarea")) return true;
    if (node instanceof HTMLElement && node.scrollHeight > node.clientHeight + 1) {
      const overflow = window.getComputedStyle(node).overflowY;
      if (overflow === "auto" || overflow === "scroll") {
        const canScroll = deltaY > 0 ? node.scrollTop + node.clientHeight < node.scrollHeight - 1 : node.scrollTop > 0;
        if (canScroll) return true;
      }
    }
    node = node.parentElement;
  }
  return false;
}

export function bindSlideScroll(board: HTMLElement): void {
  let frame = 0;
  board.addEventListener(
    "scroll",
    () => {
      if (frame || isSnapping(board)) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        const total = slides(board).length;
        syncSlideChrome(currentSlideIndex(board), total);
      });
    },
    { passive: true },
  );
  const gesture = new WheelGesture();
  board.addEventListener(
    "wheel",
    (event) => {
      // The cube canvas zooms on wheel and calls preventDefault first.
      if (event.defaultPrevented || event.ctrlKey || Math.abs(event.deltaY) < Math.abs(event.deltaX)) return;
      if (ownsWheel(event.target, event.deltaY)) return;
      event.preventDefault();
      const step = gesture.feed(event.deltaY, event.deltaMode, event.timeStamp || performance.now(), isSnapping(board));
      if (step !== 0) moveSlide(board, step);
    },
    { passive: false },
  );
}
