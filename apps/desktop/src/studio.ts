const files: File[] = [];

const MAX_AGENTS = 8;

export interface StudioSelection {
  engineId: string;
  engineTitle: string;
  agent: string;
  agents: string[];
  voice: string;
  durationMin: number;
  perspectives: string[];
}

const PERSONA_LABELS: Record<string, string> = {
  anton: "Anton",
  alice: "Alice",
  khortana: "Khortana",
  rocky: "Rocky",
  sensei: "Sensei",
  optimus: "Optimus",
};

export function bindStudio(
  board: HTMLElement,
  status: HTMLElement,
  onChange: (selection: StudioSelection) => void,
): void {
  const drop = required("#model-drop");
  const loaded = required("#loaded-model");
  const loadedId = required("#loaded-model-id");
  const perspectives = required("#perspectives");
  const agentSlots = required("#agent-slots");
  const agentCount = required("#agent-count");
  const prompt = requiredTextarea("#prompt");
  const fileInput = requiredInput("#source-files");
  const fileList = required("#file-list");
  const ghost = document.createElement("div");
  ghost.className = "drag-ghost";
  ghost.hidden = true;
  document.body.append(ghost);

  const extraAgents: string[] = [];

  addPerspective(perspectives, "");
  required("#add-perspective").addEventListener("click", () => {
    addPerspective(perspectives, "");
    emit();
  });
  required("#add-agent").addEventListener("click", () => {
    const primary = selectValue("#agent");
    const total = 1 + extraAgents.length;
    if (total >= MAX_AGENTS) {
      status.textContent = `Max ${MAX_AGENTS} agents per track.`;
      return;
    }
    const pool = Object.keys(PERSONA_LABELS).filter((id) => id !== primary && !extraAgents.includes(id));
    const next = pool[0] ?? primary;
    extraAgents.push(next);
    renderAgentSlots();
    emit();
  });
  for (const selector of ["#agent", "#voice", "#duration"]) {
    required(selector).addEventListener("change", () => {
      if (selector === "#agent") {
        // Keep extras from duplicating primary
        const primary = selectValue("#agent");
        for (let i = extraAgents.length - 1; i >= 0; i -= 1) {
          if (extraAgents[i] === primary) extraAgents.splice(i, 1);
        }
        renderAgentSlots();
      }
      emit();
    });
  }
  perspectives.addEventListener("input", emit);
  agentSlots.addEventListener("change", (event) => {
    const target = event.target as HTMLSelectElement | null;
    if (!target || !target.dataset.agentIndex) return;
    const index = Number(target.dataset.agentIndex);
    if (!Number.isInteger(index) || index < 0 || index >= extraAgents.length) return;
    extraAgents[index] = target.value;
    emit();
  });
  agentSlots.addEventListener("click", (event) => {
    const target = event.target as HTMLElement | null;
    const btn = target?.closest<HTMLButtonElement>("[data-remove-agent]");
    if (!btn) return;
    const index = Number(btn.dataset.removeAgent);
    if (!Number.isInteger(index)) return;
    extraAgents.splice(index, 1);
    renderAgentSlots();
    emit();
  });

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
    const model = loaded.dataset.engineTitle || "none";
    const names = files.map((file) => file.name).join(", ") || "no files";
    const people = participantNames(perspectives).join(", ") || "none";
    const agents = collectAgents().map((id) => PERSONA_LABELS[id] ?? id).join(", ");
    const voice = selectValue("#voice");
    status.textContent =
      `Podcast request: Agent personas [${agents}], Voice TTS ${voice}, model ${model}, ` +
      `duration ${selectValue("#duration")} min, perspectives ${people}, files ${names}. ` +
      "This window records the request and does not synthesize audio.";
  });

  let drag: { card: HTMLElement; pointerId: number; startX: number; startY: number; active: boolean } | null = null;

  board.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (!target || target.closest("button, a, input, select, textarea, canvas, audio")) return;
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
      // Sync Voice dropdown to dragged engine when it matches a TTS id
      const voiceSel = document.querySelector<HTMLSelectElement>("#voice");
      if (voiceSel && id && Array.from(voiceSel.options).some((o) => o.value === id)) {
        voiceSel.value = id;
      }
      status.textContent = `Loaded voice model ${title} (${id}). Voice TTS synced.`;
      emit();
    }
    drop.classList.remove("is-hot");
    ghost.hidden = true;
    document.body.classList.remove("is-dragging");
    drag = null;
  };
  board.addEventListener("pointerup", finish);
  board.addEventListener("pointercancel", finish);
  renderAgentSlots();
  emit();

  function collectAgents(): string[] {
    const primary = selectValue("#agent");
    return [primary, ...extraAgents].slice(0, MAX_AGENTS);
  }

  function renderAgentSlots(): void {
    agentSlots.replaceChildren();
    extraAgents.forEach((id, index) => {
      const row = document.createElement("div");
      row.className = "agent-slot-row";
      const sel = document.createElement("select");
      sel.dataset.agentIndex = String(index);
      sel.setAttribute("aria-label", `Agent ${index + 2}`);
      for (const [value, label] of Object.entries(PERSONA_LABELS)) {
        const opt = document.createElement("option");
        opt.value = value;
        opt.textContent = label;
        if (value === id) opt.selected = true;
        sel.append(opt);
      }
      const remove = document.createElement("button");
      remove.type = "button";
      remove.dataset.removeAgent = String(index);
      remove.textContent = "×";
      remove.title = "Remove agent";
      row.append(sel, remove);
      agentSlots.append(row);
    });
    const n = 1 + extraAgents.length;
    agentCount.textContent = `${n} / ${MAX_AGENTS} agents`;
  }

  function emit(): void {
    const duration = Number(selectValue("#duration"));
    const agents = collectAgents();
    onChange({
      engineId: loaded.dataset.engineId || "",
      engineTitle: loaded.dataset.engineTitle || "",
      agent: agents[0] ?? "anton",
      agents,
      voice: selectValue("#voice"),
      durationMin: Number.isFinite(duration) ? duration : 3,
      perspectives: participantNames(perspectives),
    });
  }
}

function addPerspective(host: HTMLElement, value: string): void {
  const input = document.createElement("input");
  input.type = "text";
  input.className = "perspective-slot";
  input.placeholder = "Participant";
  input.value = value;
  input.setAttribute("aria-label", "Perspective participant");
  host.append(input);
}

function participantNames(host: HTMLElement): string[] {
  return Array.from(host.querySelectorAll<HTMLInputElement>(".perspective-slot"))
    .map((node) => node.value.trim())
    .filter((value) => value.length > 0);
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
