import test from "node:test";
import assert from "node:assert/strict";
import { applyViewportViews, setCardFace } from "../src/render.ts";

class Node {
  className = "";
  dataset: Record<string, string> = {};
  children: Node[] = [];
  parent: Node | null = null;
  hidden = false;
  disabled = false;
  textContent = "";
  attributes = new Map<string, string>();
  constructor(readonly tagName = "DIV") {}
  classList = {
    add: (...values: string[]) => { this.className = [...new Set([...this.className.split(" "), ...values])].join(" "); },
    remove: (...values: string[]) => { this.className = this.className.split(" ").filter(value => !values.includes(value)).join(" "); },
  };
  append(...nodes: Node[]) {
    for (const node of nodes) { node.remove(); node.parent = this; this.children.push(node); }
  }
  remove() {
    if (this.parent) this.parent.children = this.parent.children.filter(node => node !== this);
    this.parent = null;
  }
  setAttribute(name: string, value: string) { this.attributes.set(name, value); }
  removeAttribute(name: string) { this.attributes.delete(name); }
  addEventListener() {}
  matches(selector: string) {
    const classes = [...selector.matchAll(/\.([\w-]+)/g)].map(match => match[1]);
    if (classes.some(value => !this.className.split(" ").includes(value))) return false;
    for (const match of selector.matchAll(/\[data-([\w-]+)(?:="([^"]*)")?\]/g)) {
      const key = match[1].replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
      if (!(key in this.dataset) || (match[2] !== undefined && this.dataset[key] !== match[2])) return false;
    }
    return true;
  }
  querySelectorAll(selector: string): Node[] {
    return this.children.flatMap(node => [...(node.matches(selector) ? [node] : []), ...node.querySelectorAll(selector)]);
  }
  querySelector(selector: string): Node | null { return this.querySelectorAll(selector)[0] ?? null; }
}

function fixture() {
  const board = new Node(), tile = new Node(), flip = new Node(), front = new Node(), back = new Node(), tools = new Node();
  tile.className = "card";
  tile.dataset.id = "profile-alice";
  flip.className = "flip";
  front.className = "face front";
  back.className = "face back";
  tools.className = "profile-back";
  back.append(tools); flip.append(front, back); tile.append(flip); board.append(tile);
  Object.assign(globalThis, {
    window: { document: { createElement: (name: string) => new Node(name.toUpperCase()) } },
    CSS: { escape: (value: string) => value },
  });
  const view = {
    id: "view:profile-alice", asset: "profile-alice",
    faces: [{ id: "profile", name: "Profile" }, { id: "persona", name: "Persona" }, { id: "relations", name: "Relations" }],
    snapshot: { title: "Alice", sections: [{ id: "relations", status: "real", facts: [{ label: "Voice", value: "Kokoro" }] }] },
  };
  return { board, tile, front, back, tools, view };
}

test("Rust's three faces stay independently addressable and only the visible face is accessible", () => {
  const { board, tile, front, back, view } = fixture();
  applyViewportViews(board as unknown as HTMLElement, [view]);
  assert.equal(tile.querySelectorAll(".face[data-face-id]").length, 3);
  assert.equal(setCardFace(board as unknown as HTMLElement, "view:profile-alice", "relations"), true);
  const relations = tile.querySelector('[data-face-id="relations"]')!;
  assert.equal(relations.hidden, false);
  assert.equal(relations.attributes.get("aria-hidden"), "false");
  assert.equal(relations.attributes.has("inert"), false);
  assert.equal(front.hidden, true);
  assert.equal(front.attributes.has("inert"), true);
  assert.equal(back.hidden, true);
  assert.equal(setCardFace(board as unknown as HTMLElement, "profile-alice", "unknown"), false);
});

test("resync preserves profile controls and active third face without duplicating panes", () => {
  const { board, tile, tools, view } = fixture();
  applyViewportViews(board as unknown as HTMLElement, [view]);
  setCardFace(board as unknown as HTMLElement, "profile-alice", "relations");
  applyViewportViews(board as unknown as HTMLElement, [view]);
  assert.equal(tile.querySelector(".profile-back"), tools);
  assert.equal(tools.parent?.dataset.faceId, "persona");
  assert.equal(tile.dataset.faceId, "relations");
  assert.equal(tile.querySelectorAll(".face[data-face-id]").length, 3);
});

test("a narrower snapshot keeps local tools accessible and a later snapshot moves them back", () => {
  const { board, tile, front, tools, view } = fixture();
  applyViewportViews(board as unknown as HTMLElement, [view]);
  applyViewportViews(board as unknown as HTMLElement, [{ ...view, faces: view.faces.slice(0, 1) }]);
  assert.equal(tools.parent, front);
  assert.equal(front.hidden, false);
  applyViewportViews(board as unknown as HTMLElement, [view]);
  assert.equal(tools.parent?.dataset.faceId, "persona");
  assert.equal(tile.querySelectorAll(".profile-back").length, 1);
});

test("layer statistics and formulas from the snapshot are not discarded", () => {
  const { board, tile, view } = fixture();
  const layer = { name: "signal", value: 0.25, stats: { p90: 0.4 }, formula: { text: "pending" } };
  applyViewportViews(board as unknown as HTMLElement, [{
    ...view, faces: [...view.faces, { id: "layers", name: "Layers" }],
    snapshot: { title: "Alice", sections: [{ id: "layers", layers: [layer] }] },
  }]);
  const pane = tile.querySelector('[data-face-id="layers"]')!;
  const detail = pane.children.find(node => node.tagName === "DETAILS")!;
  assert.equal(detail.children[0].textContent, "signal");
  assert.deepEqual(JSON.parse(detail.children[1].textContent), layer);
});
