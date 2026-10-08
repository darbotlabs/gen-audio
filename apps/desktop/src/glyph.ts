// Glyph badge: a 2-cell braille SVG drawn from the first 16 bits of an asset uid.
// The glyph is a visual hint only; the uid lives in aria-label/title and copies on click.
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

export interface BadgeOptions {
  /** Short visible role tag, e.g. "clip" for the bound audio clip on a library tile. */
  role?: string;
  onCopy?: (uid: string, copied: boolean) => void;
}

/** A focusable badge button. Returns null for anything that is not a valid v1 uid. */
export function glyphBadge(uid: unknown, options: BadgeOptions = {}): HTMLButtonElement | null {
  if (!isUid(uid)) return null;
  const { kind } = parseUid(uid);
  const badge = document.createElement("button");
  badge.type = "button";
  badge.className = `ga-glyph ${hueClass(kind)}`;
  badge.dataset.uid = uid;
  badge.dataset.glyph = glyphFromUid(uid);
  const label = `${kind.replace(/_/g, " ")} ${uid}`;
  badge.setAttribute("aria-label", `${label}. Activate to copy the uid.`);
  badge.title = `${uid}\nClick to copy`;
  badge.append(glyphSvg(uid));
  if (options.role) {
    const tag = document.createElement("span");
    tag.className = "ga-glyph-role";
    tag.textContent = options.role;
    badge.append(tag);
  }
  badge.addEventListener("click", (event) => {
    event.stopPropagation();
    void copyText(uid).then((copied) => {
      badge.classList.toggle("is-copied", copied);
      window.setTimeout(() => badge.classList.remove("is-copied"), 1200);
      options.onCopy?.(uid, copied);
    });
  });
  return badge;
}

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
