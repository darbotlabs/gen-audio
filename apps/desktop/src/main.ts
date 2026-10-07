import example from "../../../schemas/examples/viewport.example.json";
import { renderBoard, moveFocus, showRejected } from "./render";
import { drawCube, drawSpectrogram, makeFixture, play, previewImprove } from "./signal";
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
const improved = previewImprove(fixture);

function show(documentIn: unknown): void {
  const error = validateViewport(documentIn);
  if (error) {
    status.textContent = `Viewport rejected: ${error}`;
    showRejected(board, empty, error);
    return;
  }
  const doc = documentIn as ViewportDocument;
  renderBoard(board, empty, doc);
  paint();
  status.textContent = `${doc.cards.length} cards. Fixture visuals are generated in this window.`;
}

function paint(): void {
  const before = board.querySelector<HTMLCanvasElement>('[data-canvas="spec-before"]');
  const after = board.querySelector<HTMLCanvasElement>('[data-canvas="spec-after"]');
  const cube = board.querySelector<HTMLCanvasElement>('[data-canvas="cube"]');
  if (before) drawSpectrogram(before, fixture, "fixture before (browser)");
  if (after) drawSpectrogram(after, improved, "browser preview after (not Python)");
  if (cube) drawCube(cube, fixture);
  board.querySelectorAll<HTMLButtonElement>("[data-action='play-fixture']").forEach((node) => {
    node.addEventListener("click", () => play(fixture));
  });
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
      ? "Python improve finished on the fixture tone."
      : `Python improve did not finish: ${JSON.stringify(result)}`;
  } catch (error) {
    status.textContent = `Python improve needs the desktop shell. ${String(error)}`;
  }
});

document.addEventListener("keydown", (event) => {
  const target = event.target as HTMLElement | null;
  if (target && ["INPUT", "TEXTAREA", "SELECT", "BUTTON", "A"].includes(target.tagName)) return;
  if (event.key === "ArrowRight" || event.key === "ArrowDown") {
    event.preventDefault();
    moveFocus(board, 1);
  } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
    event.preventDefault();
    moveFocus(board, -1);
  } else if (event.key === "Home") {
    event.preventDefault();
    moveFocus(board, "home");
  } else if (event.key === "End") {
    event.preventDefault();
    moveFocus(board, "end");
  }
});

void refreshConnectors(example as ViewportDocument).then(show);
