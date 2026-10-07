import type { ViewportDocument } from "./validate";

export function renderBoard(board: HTMLElement, empty: HTMLElement, document: ViewportDocument): void {
  board.replaceChildren();
  board.style.setProperty("--cols", String(document.columns ?? 3));
  empty.hidden = document.cards.length > 0;
  board.hidden = document.cards.length === 0;
  document.cards.forEach((card, index) => {
    const article = window.document.createElement("article");
    article.className = "card";
    article.tabIndex = index === 0 ? 0 : -1;
    article.dataset.id = card.id;
    article.dataset.kind = card.kind;
    if (card.span) article.dataset.span = String(card.span);
    article.setAttribute("aria-label", `${card.kind}: ${card.title}`);
    const kind = window.document.createElement("div");
    kind.className = "kind";
    kind.textContent = card.kind;
    const title = window.document.createElement("h2");
    title.textContent = card.title;
    article.append(kind, title);
    article.append(bodyFor(card.kind, card.body));
    board.append(article);
  });
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
    const play = button("play-fixture", "Play fixture tone");
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
    wrap.append(pill(body.probed ? "probed" : "not probed", !body.probed));
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

function button(id: string, label: string): HTMLButtonElement {
  const node = window.document.createElement("button");
  node.type = "button";
  node.id = id;
  node.textContent = label;
  return node;
}

export function cards(board: HTMLElement): HTMLElement[] {
  return Array.from(board.querySelectorAll<HTMLElement>(".card"));
}

export function moveFocus(board: HTMLElement, direction: 1 | -1 | "home" | "end"): void {
  const items = cards(board);
  if (items.length === 0) return;
  const current = items.findIndex((item) => item === window.document.activeElement);
  let next = 0;
  if (direction === "home") next = 0;
  else if (direction === "end") next = items.length - 1;
  else next = Math.min(items.length - 1, Math.max(0, (current < 0 ? 0 : current) + direction));
  items.forEach((item, index) => {
    item.tabIndex = index === next ? 0 : -1;
  });
  items[next].focus();
}
