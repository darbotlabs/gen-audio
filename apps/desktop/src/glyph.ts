// Glyph badge: a 2-cell braille SVG drawn from the first 16 bits of an asset uid.
// On a card the glyph is the flip control and nothing else (Optimus ruling 1,
// 2026-10-08); elsewhere it is a passive mark. Copying a uid is the labelled
// "Copy clip uid" button on a tile's Clip face (and "Copy uid" on the
// spatial slide's cube).
// CSP: no style attributes. Hue comes from the fixed per-kind class (ga-kind-<kind>).

import { glyphBytesFromUid, glyphFromUid, hueClass, isUid, parseUid } from "./asset";

const SVG_NS = "http://www.w3.org/2000/svg";
/** Braille dot k+1 for bit k: [column, row] inside one cell. */
const DOT_POSITIONS: Array<[number, number]> = [
  [0, 0], [0, 1], [0, 2], // dots 1-3
  [1, 0], [1, 1], [1, 2], // dots 4-6
  [0, 3], [1, 3], // dots 7-8
];

export function glyphSvg(uid: string): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 10 9");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.classList.add("ga-glyph-svg");
  const bytes = glyphBytesFromUid(uid);
  bytes.forEach((byte, cell) => {
    DOT_POSITIONS.forEach(([column, row], bit) => {
      const dot = document.createElementNS(SVG_NS, "circle");
      dot.setAttribute("cx", String(1.5 + cell * 5 + column * 2));
      dot.setAttribute("cy", String(1.5 + row * 2));
      dot.setAttribute("r", "0.75");
      dot.classList.add("ga-dot", byte & (1 << bit) ? "is-on" : "is-off");
      svg.append(dot);
    });
  });
  return svg;
}

export interface MarkOptions {
  /** Short visible role tag, e.g. "clip" for the bound audio clip on a library tile. */
  role?: string;
}

function glyphNode<T extends HTMLElement>(node: T, uid: string, kind: string, role?: string): T {
  node.className = `ga-glyph ${hueClass(kind)}`;
  node.dataset.uid = uid;
  node.dataset.glyph = glyphFromUid(uid);
  node.append(glyphSvg(uid));
  if (role) {
    const tag = document.createElement("span");
    tag.className = "ga-glyph-role";
    tag.textContent = role;
    node.append(tag);
  }
  return node;
}

/**
 * A passive glyph: the uid's braille mark with the uid in its title. It is not
 * a control (Optimus ruling 1: the glyph has one meaning, and on a card that
 * meaning is flip). Returns null for anything that is not a valid v1 uid.
 */
export function glyphMark(uid: unknown, options: MarkOptions = {}): HTMLSpanElement | null {
  if (!isUid(uid)) return null;
  const { kind } = parseUid(uid);
  const mark = glyphNode(document.createElement("span"), uid, kind, options.role);
  mark.classList.add("ga-glyph-mark");
  mark.setAttribute("role", "img");
  mark.setAttribute("aria-label", `${kind.replace(/_/g, " ")} ${uid}`);
  mark.title = uid;
  return mark;
}

/**
 * The card's flip control: the card uid's glyph as a real button. The caller
 * owns the faces and keeps aria-label current (flipGlyphLabel). Click, Enter
 * and Space all call onFlip once; Enter/Space are consumed so the browser does
 * not also synthesize a click. It never copies anything.
 */
export function flipGlyph(uid: unknown, onFlip: () => void): HTMLButtonElement | null {
  if (!isUid(uid)) return null;
  const { kind } = parseUid(uid);
  const button = glyphNode(document.createElement("button"), uid, kind);
  button.type = "button";
  button.classList.add("flip-glyph");
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    onFlip();
  });
  button.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    event.stopPropagation();
    onFlip();
  });
  return button;
}

/** "Flip card, face N of M: <next face name>" for face index `index` of `faces`. */
export function flipGlyphLabel(index: number, faces: readonly string[]): string {
  const count = faces.length;
  return `Flip card, face ${index + 1} of ${count}: ${faces[(index + 1) % count]}`;
}

export interface CopyOptions {
  /** Visible label; it names whose uid is copied ("Copy clip uid", "Copy uid"). */
  label: string;
  onCopy?: (uid: string, copied: boolean) => void;
}

/**
 * The explicit, labelled copy control (rulings 1, 4, 5). Copy is always a
 * labelled button, never the glyph. MCP keeps its own path to uids.
 */
export function copyUidButton(uid: unknown, options: CopyOptions): HTMLButtonElement | null {
  if (!isUid(uid)) return null;
  const { label, onCopy } = options;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "copy-uid";
  button.dataset.action = "copy-uid";
  button.dataset.uid = uid;
  button.textContent = label;
  button.title = uid;
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    void copyText(uid).then((copied) => {
      button.classList.toggle("is-copied", copied);
      window.setTimeout(() => button.classList.remove("is-copied"), 1200);
      onCopy?.(uid, copied);
    });
  });
  return button;
}

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
