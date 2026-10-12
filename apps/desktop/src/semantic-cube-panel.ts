import examples from "../../../schemas/examples/semantic-cubes.json";
import cubeSchema from "../../../schemas/semantic-cube.schema.json";
import recordSchema from "../../../schemas/semantic-record.schema.json";
import { parseSemanticCube, SEMANTIC_MAX_BYTES, type SemanticCube } from "./semantic-cube";
import { semanticCubeDocument } from "./cubeview";

function download(name: string, value: unknown): void {
  const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2) + "\n"], { type: "application/json" }));
  const link = document.createElement("a"); link.href = url; link.download = name; link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** A bounded JSON editor exposes the entire portable contract, not a lossy form. */
export function bindSemanticCubePanel(host: HTMLElement, apply: (doc: SemanticCube, current: () => boolean) => Promise<void>): void {
  const button = document.createElement("button");
  button.type = "button"; button.id = "cube-metrics"; button.textContent = "Metrics…";
  host.append(button);
  const dialog = document.createElement("dialog"); dialog.className = "semantic-editor"; dialog.id = "semantic-editor";
  dialog.setAttribute("aria-labelledby", "semantic-editor-title");
  const title = document.createElement("h2"); title.id = "semantic-editor-title"; title.textContent = "Metric cube";
  const toolbar = document.createElement("div"); toolbar.className = "semantic-toolbar";
  const presets = document.createElement("select"); presets.setAttribute("aria-label", "Metric schema example");
  for (const [index, doc] of examples.entries()) {
    const option = document.createElement("option"); option.value = String(index); option.textContent = doc.title; presets.append(option);
  }
  const editor = document.createElement("textarea"); editor.id = "semantic-document"; editor.spellcheck = false;
  editor.setAttribute("aria-label", "Semantic cube JSON"); editor.maxLength = SEMANTIC_MAX_BYTES;
  const feedback = document.createElement("p"); feedback.className = "semantic-feedback"; feedback.setAttribute("role", "status");
  const loadExample = () => { editor.value = JSON.stringify(examples[Number(presets.value)], null, 2); feedback.textContent = ""; };
  presets.addEventListener("change", loadExample); loadExample();
  const file = document.createElement("input"); file.type = "file"; file.accept = ".json,application/json";
  file.setAttribute("aria-label", "Import metric cube JSON");
  let revision = 0;
  file.addEventListener("change", () => {
    const selected = file.files?.[0]; const request = ++revision;
    if (!selected) return;
    if (selected.size > SEMANTIC_MAX_BYTES) { feedback.textContent = "Semantic cube exceeds 1 MiB"; file.value = ""; return; }
    void selected.text().then(raw => {
      if (request !== revision) return;
      const { document: doc } = parseSemanticCube(raw);
      editor.value = JSON.stringify(doc, null, 2); feedback.textContent = "Imported. Choose Render cube to apply.";
    }).catch((error: unknown) => { if (request === revision) feedback.textContent = error instanceof Error ? error.message : "Import failed"; })
      .finally(() => { if (request === revision) file.value = ""; });
  });
  const render = document.createElement("button"); render.type = "button"; render.id = "semantic-render"; render.textContent = "Render cube";
  render.addEventListener("click", () => {
    const request = ++revision;
    render.disabled = true; feedback.textContent = "Validating…";
    void Promise.resolve().then(() => parseSemanticCube(editor.value)).then(({ document: doc }) => apply(doc, () => request === revision && dialog.open && dialog.isConnected))
      .then(() => { if (request === revision) { revision++; dialog.close(); } })
      .catch((error: unknown) => { if (request === revision) feedback.textContent = error instanceof Error ? error.message : "Invalid metric cube"; })
      .finally(() => { render.disabled = false; });
  });
  const save = document.createElement("button"); save.type = "button"; save.textContent = "Export JSON"; save.id = "semantic-export";
  save.addEventListener("click", () => {
    try { const { document: doc } = parseSemanticCube(editor.value); download(`${doc.id}.json`, doc); }
    catch (error) { feedback.textContent = error instanceof Error ? error.message : "Invalid metric cube"; }
  });
  const schema = document.createElement("button"); schema.type = "button"; schema.textContent = "Export schemas";
  schema.addEventListener("click", () => download("semantic-schema-library.json", { schemaVersion: "1.0.0", schemas: [recordSchema, cubeSchema], examples }));
  const close = document.createElement("button"); close.type = "button"; close.textContent = "Close";
  close.addEventListener("click", () => dialog.close());
  toolbar.append(presets, file, save, schema);
  const actions = document.createElement("div"); actions.className = "semantic-toolbar"; actions.append(render, close);
  dialog.append(title, toolbar, editor, feedback, actions); host.closest(".slide")!.append(dialog);
  button.addEventListener("click", () => {
    const current = semanticCubeDocument();
    if (current) editor.value = JSON.stringify(current, null, 2);
    feedback.textContent = ""; dialog.showModal();
  });
  dialog.addEventListener("close", () => { revision++; });
}
