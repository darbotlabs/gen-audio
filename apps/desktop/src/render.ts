import type { ViewportCard, ViewportDocument } from "./validate";

const EMPTY_COPY =
  "No cards in this viewport. Load the example for labeled fixture tiles and live connector probes — not a finished podcast product.";

export type SlideSchema = {
  id: string;
  title: string;
  blurb: string;
  columns: number;
  cardIds: string[];
};

/** Three full-viewport slides with distinct card schemas (PPT-style snap). */
export const SLIDE_SCHEMAS: SlideSchema[] = [
  {
    id: "studio",
    title: "Studio",
    blurb: "Engines + profile spectrogram preview (not product speech)",
    columns: 3,
    cardIds: [
      "engine-kokoro",
      "engine-vibevoice",
      "engine-magpie",
      "engine-pocket",
      "spec-fixture",
    ],
  },
  {
    id: "connectors",
    title: "Connectors",
    blurb: "Live connector probe rows (mock mode is labeled, not hidden)",
    columns: 3,
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
    cardIds: ["conn-mcp", "serve-node", "serve-gateway", "bench-ref", "cube-fixture", "cast-sample"],
  },
  {
    id: "library-models",
    title: "Library · Voice models",
    blurb: "Layer: TTS voice-model profiles (flip for full schema). Not podcast claims.",
    columns: 3,
    cardIds: [
      "vp-anton-kokoro",
      "vp-alice-kokoro",
      "vp-khortana-misaki",
      "vp-rocky-dayour",
    ],
  },
  {
    id: "library-clips",
    title: "Library · Audio clips",
    blurb: "Layer: real WAV livetiles — play/pause/scrub; unavailable stays honest",
    columns: 3,
    cardIds: [
      "lib-kokoro-onnx",
      "lib-kokoro",
      "lib-misaki-kokoro",
      "lib-magpie",
      "lib-vibevoice",
      "lib-pocket",
    ],
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
      }
      if (card.kind === "VoiceProfile") {
        article.classList.add("voice-profile-tile");
        article.dataset.personaId = String(card.body.personaId ?? "");
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
        if (target && target.closest("button") && !target.classList.contains("flip-toggle")) return;
        event.preventDefault();
        toggleFlip(article);
      });
      section.append(article);
      globalIndex += 1;
    });

    board.append(section);
  });

  buildSlideDots(slides.length);
  syncSlideChrome(0, slides.length);
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
  face.append(row, title, bodyFor(card.kind, card.body), flip);
  return face;
}

/** Honest badge: fixture tiles are Fixture, not "Live product speech". */
function liveLabel(card: ViewportCard): string {
  if (card.kind === "SpectrogramPanel" || card.kind === "Cube3D") return "Fixture";
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
  kind.textContent = card.kind === "VoiceProfile" ? "Voice profile schema" : "Adaptive card";
  const title = window.document.createElement("h2");
  title.textContent = card.title;
  face.append(kind, title);
  if (card.kind === "VoiceProfile") face.append(voiceProfileBack(card));
  else face.append(adaptiveFace(card));
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

function voiceProfileBack(card: ViewportCard): HTMLElement {
  const wrap = window.document.createElement("div");
  wrap.className = "adaptive-body voice-profile-back";
  const body = card.body;
  const facts: Array<[string, string]> = [
    ["agentname", String(body.agentname ?? "")],
    ["tone", String(body.tone ?? "")],
    ["purpose", String(body.purpose ?? "")],
    ["domain", String(body.domain ?? "")],
    ["accent", String(body.accent ?? "")],
    ["traits", Array.isArray(body.traits) ? body.traits.map(String).join(", ") : ""],
    ["refs", Array.isArray(body.refs) ? body.refs.map(String).join(" · ") : ""],
    ["ttsModel", String(body.ttsModel ?? "")],
    ["personaId", String(body.personaId ?? "")],
  ];
  const list = window.document.createElement("ul");
  list.className = "vp-facts";
  for (const [k, v] of facts) {
    const item = window.document.createElement("li");
    item.textContent = k + ": " + v;
    list.append(item);
  }
  wrap.append(list);
  wrap.append(paragraph("2D spectrogram hook"));
  wrap.append(canvas("vp-back-2d"));
  wrap.append(paragraph("3D spectrogram hook (cube layers)"));
  const hook = window.document.createElement("div");
  hook.className = "vp-3d-hook";
  hook.dataset.cubeJsonUrl = body.cubeJsonUrl ? String(body.cubeJsonUrl) : "";
  hook.textContent = body.cubeJsonUrl
    ? "Bound: " + String(body.cubeJsonUrl)
    : "3D hook ready — bind from Library clip JSON";
  wrap.append(hook);
  return wrap;
}

function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return m + ":" + String(s).padStart(2, "0");
}

function toggleFlip(article: HTMLElement): void {
  article.classList.toggle("is-flipped");
  const pressed = article.classList.contains("is-flipped");
  article.querySelectorAll<HTMLButtonElement>(".flip-toggle").forEach((node) => {
    node.setAttribute("aria-pressed", pressed ? "true" : "false");
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

function bodyFor(kind: string, body: Record<string, unknown>): HTMLElement {
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
    const note = paragraph(bound
      ? "Interactive cube bound to library clip JSON — layers signal/tonality/confidence/quality."
      : "Drag to rotate. Select a real Library clip + Bind cube to replace fixture source.");
    note.classList.add("cube-bind-note");
    wrap.append(note);
    const cube = canvas("cube");
    cube.classList.add("cube-interactive");
    if (body.cubeJsonUrl) cube.dataset.cubeJsonUrl = String(body.cubeJsonUrl);
    wrap.append(cube);
  } else if (kind === "PodcastCast") {
    wrap.append(pill("sample script", true));
    wrap.append(paragraph("Sample script only. Voices are kokoro-onnx ids — synthesizedSpeech is false here."));
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
    wrap.append(paragraph(String(body.summary ?? "")));
    if (speech) {
      const dur = typeof body.duration_s === "number" ? body.duration_s.toFixed(1) : "?";
      const sr = String(body.sample_rate ?? "?");
      wrap.append(paragraph(dur + "s · " + sr + " Hz"));
      const audio = window.document.createElement("audio");
      audio.className = "library-audio";
      audio.controls = true;
      audio.preload = "metadata";
      audio.src = String(body.wavUrl ?? "");
      audio.dataset.wavUrl = String(body.wavUrl ?? "");
      audio.dataset.cubeJsonUrl = body.cubeJsonUrl ? String(body.cubeJsonUrl) : "";
      audio.dataset.cubePngUrl = body.cubePngUrl ? String(body.cubePngUrl) : "";
      audio.setAttribute("aria-label", "Play pause scrub library clip");
      wrap.append(audio);
      const time = window.document.createElement("p");
      time.className = "audio-time summary";
      time.dataset.audioTime = "1";
      time.textContent = "0:00 / " + formatClock(typeof body.duration_s === "number" ? body.duration_s : 0);
      wrap.append(time);
      const bind = button("Bind cube");
      bind.className = "bind-cube";
      bind.dataset.action = "bind-cube";
      bind.dataset.cubeJsonUrl = body.cubeJsonUrl ? String(body.cubeJsonUrl) : "";
      bind.dataset.cubePngUrl = body.cubePngUrl ? String(body.cubePngUrl) : "";
      bind.dataset.clipId = String(body.engineId ?? "");
      wrap.append(bind);
      if (body.cubePngUrl) {
        const img = window.document.createElement("img");
        img.className = "cube-thumb";
        img.alt = "Inverse-HDR cube layers signal/tonality/confidence/quality";
        img.src = String(body.cubePngUrl);
        img.loading = "lazy";
        wrap.append(img);
      }
      if (typeof body.inv_hdr === "number") {
        wrap.append(paragraph("inv-HDR " + body.inv_hdr.toFixed(3) + " · layers signal/tonality/confidence/quality"));
      }
    } else {
      wrap.append(paragraph("No WAV — honest empty tile (" + status + ")."));
      if (body.reason) wrap.append(paragraph(String(body.reason)));
    }
    // Harvest / rename semantic + face
    const harvest = window.document.createElement("div");
    harvest.className = "harvest-fields";
    const sem = window.document.createElement("input");
    sem.type = "text";
    sem.className = "harvest-input";
    sem.placeholder = "Semantic name";
    sem.value = String(body.semanticName ?? body.title ?? "");
    sem.setAttribute("aria-label", "Semantic clip name");
    const face = window.document.createElement("input");
    face.type = "text";
    face.className = "harvest-input";
    face.placeholder = "Face name";
    face.value = String(body.faceName ?? "");
    face.setAttribute("aria-label", "Face-level clip name");
    harvest.append(sem, face);
    wrap.append(harvest);
  } else if (kind === "VoiceProfile") {
    wrap.append(pill("adaptive profile", false));
    wrap.append(paragraph("Agent " + String(body.agentname ?? "") + " · Voice " + String(body.ttsModel ?? "")));
    wrap.append(paragraph("Tone " + String(body.tone ?? "") + " · " + String(body.purpose ?? "") + " · " + String(body.domain ?? "")));
    const flipHint = paragraph("Flip for full schema + 2D/3D spectrogram hooks.");
    wrap.append(flipHint);
    const spec2d = canvas("vp-spec-2d");
    spec2d.classList.add("vp-spec");
    wrap.append(spec2d);
  } else if (kind === "ConnectorStatus") {
    wrap.append(pill(String(body.mode), body.mode === "mock" || body.mode === "token_present"));
    wrap.append(paragraph(String(body.detail ?? "")));
    if (body.authenticated === true) wrap.append(paragraph("Authenticated: yes"));
    else wrap.append(paragraph("Authenticated: no"));
  }
  return wrap;
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
  if (kind === "VoiceProfile") {
    return "Agent " + String(body.agentname ?? "") + " / Voice " + String(body.ttsModel ?? "") + " schema flipside.";
  }
  return String(body.disclaimer ?? "Fixture face. Not a podcast render.");
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

export function goToSlide(board: HTMLElement, index: number): void {
  const list = slides(board);
  if (list.length === 0) return;
  const next = Math.min(list.length - 1, Math.max(0, index));
  list[next].scrollIntoView({ block: "start", behavior: "smooth" });
  const slideCards = Array.from(list[next].querySelectorAll<HTMLElement>(".card"));
  cards(board).forEach((item) => {
    item.tabIndex = -1;
  });
  if (slideCards[0]) {
    slideCards[0].tabIndex = 0;
    slideCards[0].focus({ preventScroll: true });
  }
  syncSlideChrome(next, list.length);
}

export function moveSlide(board: HTMLElement, direction: 1 | -1 | "home" | "end"): void {
  const list = slides(board);
  if (list.length === 0) return;
  const current = currentSlideIndex(board);
  if (direction === "home") goToSlide(board, 0);
  else if (direction === "end") goToSlide(board, list.length - 1);
  else goToSlide(board, current + direction);
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
      if (board) goToSlide(board, i);
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
}

export function bindSlideScroll(board: HTMLElement): void {
  let frame = 0;
  board.addEventListener(
    "scroll",
    () => {
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        const total = slides(board).length;
        syncSlideChrome(currentSlideIndex(board), total);
      });
    },
    { passive: true },
  );
}
