import example from "../../../schemas/examples/viewport.example.json";
import { profilePreview, type ProfilePreview, type VoiceSelection } from "./profiles";
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
  agent: "local",
  voice: "af_heart",
  durationMin: 3,
  perspectives: [],
};
let activePreview: ProfilePreview = profilePreview(selection);


function bindLibraryPlayback(): void {
  board.querySelectorAll<HTMLButtonElement>("[data-action='play-library']").forEach((node) => {
    node.onclick = () => {
      const url = node.dataset.wavUrl;
      if (!url) {
        status.textContent = "Library tile has no wavUrl (honest empty).";
        return;
      }
      const audio = new Audio(url);
      void audio.play().then(() => {
        status.textContent = `Playing library clip ${url}`;
      }).catch((err) => {
        status.textContent = `Library play failed: ${String(err)}`;
      });
    };
  });
}

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
  bindLibraryPlayback();
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

void refreshConnectors(example as ViewportDocument).then(show);
