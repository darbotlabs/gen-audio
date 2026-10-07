/** Filename and sidecar harvest. Renames stay in the window. */

export interface HarvestedNames {
  semantic: string;
  face: string;
  source: "filename" | "sidecar" | "filename-only";
}

export async function harvestNames(wavUrl: string, sidecarUrl?: string): Promise<HarvestedNames> {
  const file = decodeURIComponent(wavUrl.split("/").pop() || wavUrl);
  const stem = file.replace(/\.(wav|mp4|webm|mov)$/i, "");
  const face = stem.replace(/[_-]+/g, " ").trim() || stem;
  let semantic = face;
  let source: HarvestedNames["source"] = "filename";
  if (sidecarUrl) {
    try {
      const response = await fetch(sidecarUrl);
      if (response.ok) {
        const meta = (await response.json()) as Record<string, unknown>;
        const engine = typeof meta.engine === "string" ? meta.engine : "";
        const words = typeof meta.n_words === "number" ? `${meta.n_words} words` : "";
        const turns = typeof meta.n_turns === "number" ? `${meta.n_turns} turns` : "";
        const harvested = [engine, turns, words].filter((part) => part.length > 0).join(" · ");
        if (harvested) {
          semantic = harvested;
          source = "sidecar";
        }
      }
    } catch {
      source = "filename-only";
    }
  }
  return { semantic, face, source };
}

export function applyClipNames(tile: HTMLElement, semantic?: string, face?: string): void {
  if (face && face.trim()) {
    tile.dataset.faceName = face.trim();
    tile.querySelectorAll("h2").forEach((node) => {
      node.textContent = face.trim();
    });
  }
  if (semantic && semantic.trim()) {
    tile.dataset.semanticName = semantic.trim();
    const line = tile.querySelector<HTMLElement>(".semantic-name");
    if (line) line.textContent = semantic.trim();
  }
}
