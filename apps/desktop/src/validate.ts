export const CARD_KINDS = [
  "EngineStatus",
  "SpectrogramPanel",
  "Cube3D",
  "PodcastCast",
  "ServeHealth",
  "BenchmarkCompare",
  "ConnectorStatus",
] as const;

export type CardKind = (typeof CARD_KINDS)[number];

export interface ViewportCard {
  id: string;
  kind: CardKind;
  title: string;
  span?: number;
  body: Record<string, unknown>;
  adaptive?: {
    type: "AdaptiveCard";
    version: string;
    body: unknown[];
  };
}

export interface ViewportDocument {
  version: "1.0";
  title: string;
  columns?: number;
  cards: ViewportCard[];
}

export function validateViewport(document: unknown): string | null {
  if (!isRecord(document)) return "viewport must be an object";
  if (document.version !== "1.0") return "version must be 1.0";
  if (typeof document.title !== "string" || document.title.length === 0) return "title is required";
  if (document.columns !== undefined) {
    if (typeof document.columns !== "number" || document.columns < 1 || document.columns > 4) {
      return "columns must be from 1 to 4";
    }
  }
  if (!Array.isArray(document.cards)) return "cards must be an array";
  const seen = new Set<string>();
  for (const card of document.cards) {
    const error = validateCard(card, seen);
    if (error) return error;
  }
  return null;
}

function validateCard(card: unknown, seen: Set<string>): string | null {
  if (!isRecord(card)) return "card must be an object";
  if (typeof card.id !== "string" || !/^[A-Za-z0-9_-]{1,64}$/.test(card.id)) return "card id is invalid";
  if (seen.has(card.id)) return `duplicate card id ${card.id}`;
  seen.add(card.id);
  if (typeof card.kind !== "string" || !CARD_KINDS.includes(card.kind as CardKind)) {
    return `card ${card.id} has an unknown kind`;
  }
  if (typeof card.title !== "string" || card.title.length === 0) return "card title is required";
  if (!isRecord(card.body)) return `card ${card.id} is missing body`;
  if (card.kind === "SpectrogramPanel" || card.kind === "Cube3D") {
    if (card.body.source !== "fixture-tone" || card.body.notPodcast !== true) {
      return `card ${card.id} must be a labeled fixture, not a podcast claim`;
    }
    if (typeof card.body.disclaimer !== "string" || !card.body.disclaimer.toLowerCase().includes("not")) {
      return `card ${card.id} disclaimer must say the visual is not a podcast render`;
    }
  }
  if (card.kind === "BenchmarkCompare" && card.body.measuredHere !== false) {
    return "benchmark cards cannot claim they were measured in this app";
  }
  return null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
