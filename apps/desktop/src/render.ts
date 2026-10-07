import type { ViewportDocument } from "./validate";

const EMPTY_COPY =
  "No cards in this viewport. Load the example to see engine, spectrogram, cube, and connector cards.";

export function showRejected(board: HTMLElement, empty: HTMLElement, error: string): void {
  board.replaceChildren();
  board.hidden = true;
  empty.hidden = false;
  empty.textContent = `Viewport rejected: ${error}`;
}

const liveTimers = new WeakMap<HTMLElement, number>();

export function renderBoard(board: HTMLElement, empty: HTMLElement, document: ViewportDocument): void {
  stopLiveCycle(board);
  board.replaceChildren();
  const columns = document.columns ?? 3;
  board.style.setProperty("--cols", String(columns));
  empty.hidden = document.cards.length > 0;
  board.hidden = document.cards.length === 0;
  if (document.cards.length === 0) empty.textContent = EMPTY_COPY;
  document.cards.forEach((card, index) => {
    const article = window.document.createElement("article");
    article.className = "card livetile";
    article.tabIndex = index === 0 ? 0 : -1;
    article.dataset.id = card.id;
    article.dataset.kind = card.kind;
    const span = Math.min(card.span ?? 1, columns);
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
    board.append(article);
  });
  startLiveCycle(board);
}

function frontFace(card: ViewportDocument["cards"][number]): HTMLElement {
  const face = window.document.createElement("div");
  face.className = "face front";
  const row = window.document.createElement("div");
  row.className = "live-row";
  const kind = window.document.createElement("div");
  kind.className = "kind";
  kind.textContent = card.kind;
  const live = window.document.createElement("span");
  live.className = "live-mark";
  live.textContent = "Live";
  row.append(kind, live);
  const title = window.document.createElement("h2");
  title.textContent = card.title;
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

function backFace(card: ViewportDocument["cards"][number]): HTMLElement {
  const face = window.document.createElement("div");
  face.className = "face back";
  const kind = window.document.createElement("div");
  kind.className = "kind";
  kind.textContent = "Adaptive card";
  const title = window.document.createElement("h2");
  title.textContent = card.title;
  face.append(kind, title, adaptiveFace(card));
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
    const tiles = Array.from(board.querySelectorAll<HTMLElement>(".livetile"));
    if (tiles.length === 0) return;
    const idle = tiles.filter((tile) => !tile.matches(":hover") && !tile.matches(":focus-within"));
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
  } else if (kind === "SpectrogramPanel") {
    wrap.append(paragraph(String(body.disclaimer ?? "")));
    const before = canvas("spec-before");
    const after = canvas("spec-after");
    const play = button("Play fixture tone");
    play.dataset.action = "play-fixture";
    wrap.append(before, after, play);
  } else if (kind === "Cube3D") {
    wrap.append(paragraph(String(body.disclaimer ?? "")));
    wrap.append(canvas("cube"));
  } else if (kind === "PodcastCast") {
    wrap.append(paragraph("Sample script only. Voices are kokoro-onnx ids, not a recording."));
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
  } else if (kind === "ConnectorStatus") {
    wrap.append(pill(String(body.mode), body.mode === "mock" || body.mode === "token_present"));
    wrap.append(paragraph(String(body.detail ?? "")));
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

function adaptiveFace(card: ViewportDocument["cards"][number]): HTMLElement {
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
  if (kind === "EngineStatus") return `${String(body.engineId ?? "engine")} is ${String(body.status ?? "unknown")}.`;
  if (kind === "ConnectorStatus") return `${String(body.connectorId ?? "connector")} mode ${String(body.mode ?? "unknown")}.`;
  if (kind === "ServeHealth") return String(body.healthUrl ?? "Health URL is not set.");
  if (kind === "PodcastCast") return "Sample cast only. This face is not a second board.";
  if (kind === "BenchmarkCompare") return String(body.sourceNote ?? "Scores are not remeasured in this window.");
  return String(body.disclaimer ?? "Fixture face. Not a podcast render.");
}

export function cards(board: HTMLElement): HTMLElement[] {
  return Array.from(board.querySelectorAll<HTMLElement>(".card"));
}

export function moveFocus(board: HTMLElement, direction: 1 | -1 | "home" | "end"): void {
  const items = cards(board);
  if (items.length === 0) return;
  const active = window.document.activeElement;
  const current = items.findIndex((item) => item === active || item.contains(active));
  let next = 0;
  if (direction === "home") next = 0;
  else if (direction === "end") next = items.length - 1;
  else next = Math.min(items.length - 1, Math.max(0, (current < 0 ? 0 : current) + direction));
  items.forEach((item, index) => {
    item.tabIndex = index === next ? 0 : -1;
  });
  items[next].focus();
  items[next].scrollIntoView({ block: "nearest" });
}
