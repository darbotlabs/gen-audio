import { MAX_AGENTS, PERSONAS, VOICE_MODELS, personaById, voiceById } from "./catalog";

const files: File[] = [];

export interface StudioSelection {
  engineId: string;
  engineTitle: string;
  agents: string[];
  voice: string;
  durationMin: number;
}

let emitFn: () => void = () => {};

export function bindStudio(
  board: HTMLElement,
  status: HTMLElement,
  onChange: (selection: StudioSelection) => void,
): void {
  const drop = required("#model-drop");
  const loaded = required("#loaded-model");
  const loadedId = required("#loaded-model-id");
  const prompt = requiredTextarea("#prompt");
  const fileInput = requiredInput("#source-files");
  const fileList = required("#file-list");
  const voice = requiredSelect("#voice");
  voice.replaceChildren();
  for (const model of VOICE_MODELS) {
    const option = document.createElement("option");
    option.value = model.id;
    option.textContent = model.label;
    voice.append(option);
  }
  voice.value = "kokoro_onnx";
  renderAgents(["alice"]);

  const ghost = document.createElement("div");
  ghost.className = "drag-ghost";
  ghost.hidden = true;
  document.body.append(ghost);

  required("#add-agent").addEventListener("click", () => {
    const current = readAgents();
    if (current.length >= MAX_AGENTS) return;
    const next = PERSONAS.find((persona) => !current.includes(persona.id));
    if (!next) return;
    renderAgents([...current, next.id]);
    emit();
  });
  voice.addEventListener("change", emit);
  required("#duration").addEventListener("change", emit);

  fileInput.addEventListener("change", () => {
    if (fileInput.files) addFiles(fileInput.files, fileList);
    fileInput.value = "";
  });
  prompt.addEventListener("paste", (event) => {
    const pasted = event.clipboardData?.files;
    if (pasted && pasted.length > 0) {
      addFiles(pasted, fileList);
      event.preventDefault();
    }
  });
  required("#generate-podcast").addEventListener("click", () => {
    status.textContent = describeRequest();
  });

  let drag: { card: HTMLElement; pointerId: number; startX: number; startY: number; active: boolean } | null = null;

  board.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (!target || target.closest("button, a, input, select, textarea, canvas")) return;
    const card = target.closest<HTMLElement>(".engine-source");
    if (!card) return;
    drag = { card, pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, active: false };
    card.setPointerCapture(event.pointerId);
  });

  board.addEventListener("pointermove", (event) => {
    if (!drag || event.pointerId !== drag.pointerId) return;
    const dx = event.clientX - drag.startX;
    const dy = event.clientY - drag.startY;
    if (!drag.active && Math.hypot(dx, dy) < 8) return;
    if (!drag.active) {
      drag.active = true;
      document.body.classList.add("is-dragging");
      ghost.hidden = false;
      ghost.textContent = drag.card.dataset.engineTitle || "Engine";
    }
    ghost.style.left = `${event.clientX + 12}px`;
    ghost.style.top = `${event.clientY + 12}px`;
    const hot = overDrop(drop, event.clientX, event.clientY);
    drop.classList.toggle("is-hot", hot);
  });

  const finish = (event: PointerEvent) => {
    if (!drag || event.pointerId !== drag.pointerId) return;
    if (drag.active && overDrop(drop, event.clientX, event.clientY)) {
      const title = drag.card.dataset.engineTitle || "Engine";
      const id = drag.card.dataset.engineId || "";
      loaded.textContent = title;
      loaded.dataset.engineTitle = title;
      loaded.dataset.engineId = id;
      loadedId.textContent = id;
      drop.classList.add("is-loaded");
      if (voiceById(id)) voice.value = id;
      status.textContent = `Loaded voice model ${title} (${id}).`;
      emit();
    }
    drop.classList.remove("is-hot");
    ghost.hidden = true;
    document.body.classList.remove("is-dragging");
    drag = null;
  };
  board.addEventListener("pointerup", finish);
  board.addEventListener("pointercancel", finish);

  emitFn = () => {
    onChange(readSelection());
  };
  emit();

  function emit(): void {
    emitFn();
  }
}

export function applySidepane(next: { agents?: string[]; voice?: string; durationMin?: number }): void {
  if (next.agents && next.agents.length > 0) {
    const ids = next.agents.filter((id) => personaById(id)).slice(0, MAX_AGENTS);
    if (ids.length > 0) renderAgents(ids);
  }
  if (next.voice && voiceById(next.voice)) {
    const voice = document.querySelector<HTMLSelectElement>("#voice");
    if (voice) voice.value = next.voice;
  }
  if (typeof next.durationMin === "number") {
    const duration = document.querySelector<HTMLSelectElement>("#duration");
    if (duration) duration.value = String(next.durationMin);
  }
  emitFn();
}

export function readSelection(): StudioSelection {
  const loaded = document.querySelector<HTMLElement>("#loaded-model");
  const duration = Number(selectValue("#duration"));
  return {
    engineId: loaded?.dataset.engineId || "",
    engineTitle: loaded?.dataset.engineTitle || "",
    agents: readAgents(),
    voice: selectValue("#voice") || "kokoro_onnx",
    durationMin: Number.isFinite(duration) ? duration : 3,
  };
}

export function describeRequest(): string {
  const selection = readSelection();
  const names = files.map((file) => file.name).join(", ") || "no files";
  const personas = selection.agents.map((id) => personaById(id)?.name ?? id).join(", ");
  const voice = voiceById(selection.voice);
  const model = selection.engineTitle || "none";
  return (
    `Podcast request recorded: agents ${personas}, voice model ${voice?.label ?? selection.voice}, ` +
    `loaded card ${model}, duration ${selection.durationMin} min, files ${names}. ` +
    "synthesizedSpeech is false until a synth tool returns audio."
  );
}

export function promptText(): string {
  const node = document.querySelector<HTMLTextAreaElement>("#prompt");
  return node?.value ?? "";
}

export function promptNote(): string {
  return promptText().slice(0, 200);
}

function renderAgents(ids: string[]): void {
  const host = required("#agents");
  const chosen = unique(ids).slice(0, MAX_AGENTS);
  const list = chosen.length > 0 ? chosen : ["alice"];
  host.replaceChildren();
  list.forEach((id, index) => {
    const row = document.createElement("div");
    row.className = "agent-row";
    const select = document.createElement("select");
    select.className = "agent-slot";
    select.setAttribute("aria-label", `Agent ${index + 1}`);
    for (const persona of PERSONAS) {
      const option = document.createElement("option");
      option.value = persona.id;
      option.textContent = persona.name;
      if (persona.id === id) option.selected = true;
      select.append(option);
    }
    select.addEventListener("change", () => {
      renderAgents(readAgents());
      emitFn();
    });
    row.append(select);
    if (index > 0) {
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "agent-remove";
      remove.textContent = "×";
      remove.setAttribute("aria-label", `Remove agent ${index + 1}`);
      remove.addEventListener("click", () => {
        const next = readAgents().filter((_, slot) => slot !== index);
        renderAgents(next);
        emitFn();
      });
      row.append(remove);
    }
    host.append(row);
  });
  const add = document.querySelector<HTMLButtonElement>("#add-agent");
  if (add) add.disabled = list.length >= MAX_AGENTS;
}

function readAgents(): string[] {
  return unique(
    Array.from(document.querySelectorAll<HTMLSelectElement>("#agents .agent-slot")).map((node) => node.value),
  ).slice(0, MAX_AGENTS);
}

function unique(ids: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const id of ids) {
    if (!id || seen.has(id) || !personaById(id)) continue;
    seen.add(id);
    out.push(id);
  }
  return out;
}

function addFiles(list: FileList, host: HTMLElement): void {
  for (const file of Array.from(list)) {
    files.push(file);
    const item = document.createElement("li");
    item.textContent = file.name;
    host.append(item);
  }
}

function overDrop(drop: HTMLElement, x: number, y: number): boolean {
  const rect = drop.getBoundingClientRect();
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

function selectValue(selector: string): string {
  const node = document.querySelector<HTMLSelectElement>(selector);
  return node?.value ?? "";
}

function required(selector: string): HTMLElement {
  const node = document.querySelector<HTMLElement>(selector);
  if (!node) throw new Error(`missing ${selector}`);
  return node;
}

function requiredSelect(selector: string): HTMLSelectElement {
  const node = document.querySelector<HTMLSelectElement>(selector);
  if (!node) throw new Error(`missing ${selector}`);
  return node;
}

function requiredTextarea(selector: string): HTMLTextAreaElement {
  const node = document.querySelector<HTMLTextAreaElement>(selector);
  if (!node) throw new Error(`missing ${selector}`);
  return node;
}

function requiredInput(selector: string): HTMLInputElement {
  const node = document.querySelector<HTMLInputElement>(selector);
  if (!node) throw new Error(`missing ${selector}`);
  return node;
}
