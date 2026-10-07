import { personaById, voiceById } from "./catalog";

export const CARD_KINDS = [
  "EngineStatus",
  "SpectrogramPanel",
  "Cube3D",
  "PodcastCast",
  "ServeHealth",
  "BenchmarkCompare",
  "ConnectorStatus",
  "LibraryClip",
  "VoiceProfile",
] as const;

export const CONNECTOR_IDS = ["mcp", "acp", "harness", "copilot", "claude", "gpt", "gemini"] as const;
export const CONNECTOR_MODES = ["mock", "live", "local", "token_present", "misconfigured"] as const;
export const ENGINE_STATUSES = ["implemented", "external", "library", "weights_absent", "unavailable"] as const;
export const LIBRARY_STATUSES = ["ok", "running", "weights_absent", "unavailable", "external"] as const;

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
  if (!boundedString(document.title, 1, 160)) return "title is required";
  if (document.columns !== undefined) {
    const columns = document.columns;
    if (typeof columns !== "number" || !Number.isInteger(columns) || columns < 1 || columns > 4) {
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
  if (!boundedString(card.title, 1, 120)) return "card title is required";
  if (card.span !== undefined) {
    const span = card.span;
    if (typeof span !== "number" || !Number.isInteger(span) || span < 1 || span > 3) {
      return `card ${card.id} span must be 1, 2, or 3`;
    }
  }
  if (!isRecord(card.body)) return `card ${card.id} is missing body`;
  const bodyError = validateBody(card.id, card.kind, card.body);
  if (bodyError) return bodyError;
  if (card.adaptive !== undefined) {
    return validateAdaptive(card.id, card.adaptive);
  }
  return null;
}

function validateBody(id: string, kind: string, body: Record<string, unknown>): string | null {
  if (kind === "EngineStatus") {
    if (!boundedString(body.engineId, 1, 40)) return `card ${id} engineId is required`;
    if (typeof body.status !== "string" || !ENGINE_STATUSES.includes(body.status as (typeof ENGINE_STATUSES)[number])) {
      return `card ${id} has unknown engine status`;
    }
    if (!boundedString(body.summary, 1, 400)) return `card ${id} summary is required`;
  }
  if (kind === "SpectrogramPanel" || kind === "Cube3D") {
    if (body.source !== "fixture-tone" || body.notPodcast !== true) {
      return `card ${id} must be a labeled fixture, not a podcast claim`;
    }
    if (!boundedString(body.disclaimer, 12, 400) || !String(body.disclaimer).toLowerCase().includes("not")) {
      return `card ${id} disclaimer must say the visual is not a podcast render`;
    }
  }
  if (kind === "PodcastCast") {
    if (body.sampleScript !== true) return `card ${id} must set sampleScript true`;
    if (!Array.isArray(body.speakers) || body.speakers.length === 0) return "speakers must not be empty";
    for (const speaker of body.speakers) {
      if (!isRecord(speaker)) return "speaker must be an object";
      if (!boundedString(speaker.id, 1, 8) || !boundedString(speaker.name, 1, 40) || !boundedString(speaker.voice, 1, 40)) {
        return `card ${id} speaker is missing id, name, or voice`;
      }
    }
  }
  if (kind === "ServeHealth") {
    if (!boundedString(body.host, 1, 80)) return `card ${id} host is required`;
    if (!Number.isInteger(body.port) || Number(body.port) < 1 || Number(body.port) > 65535) return "port out of range";
    if (!boundedString(body.baseUrl, 1, 200) || !boundedString(body.healthUrl, 1, 220)) return `card ${id} serve URLs are required`;
    if (typeof body.probed !== "boolean") return "probed must be a boolean";
    if (body.probed && typeof body.ok !== "boolean") return "probed serve cards must include ok";
    if (body.role !== "node" && body.role !== "shared-gateway") return "serve role must be node or shared-gateway";
  }
  if (kind === "BenchmarkCompare") {
    if (body.measuredHere !== false) return "benchmark cards cannot claim they were measured in this app";
    if (!boundedString(body.sourceNote, 12, 400) || !String(body.sourceNote).toLowerCase().includes("not remeasured")) {
      return "benchmark sourceNote must say the figures are not remeasured here";
    }
    if (!Array.isArray(body.rows)) return "rows must be an array";
    for (const row of body.rows) {
      if (!isRecord(row)) return "benchmark row must be an object";
      if (!boundedString(row.engine, 1, 40) || !boundedString(row.metric, 1, 40)) return "benchmark row is incomplete";
      if (typeof row.value !== "number" && typeof row.value !== "string") return "benchmark value must be a number or string";
    }
  }

  if (kind === "LibraryClip") {
    if (!boundedString(body.engineId, 1, 40)) return `card ${id} engineId is required`;
    if (typeof body.status !== "string" || !LIBRARY_STATUSES.includes(body.status as (typeof LIBRARY_STATUSES)[number])) {
      return `card ${id} has unknown library status`;
    }
    if (typeof body.synthesizedSpeech !== "boolean") return `card ${id} synthesizedSpeech must be boolean`;
    if (!boundedString(body.summary, 1, 400)) return `card ${id} summary is required`;
    if (body.synthesizedSpeech === true) {
      if (!boundedString(body.wavUrl, 1, 260)) return `card ${id} wavUrl required for real speech`;
      if (typeof body.duration_s !== "number" || !(body.duration_s > 0)) return `card ${id} duration_s must be > 0`;
    } else if (body.wavUrl) {
      return `card ${id} must not set wavUrl unless synthesizedSpeech is true`;
    }
    if (body.semanticName !== undefined && !boundedString(body.semanticName, 1, 80)) return `card ${id} semanticName is invalid`;
    if (body.faceName !== undefined && !boundedString(body.faceName, 1, 80)) return `card ${id} faceName is invalid`;
    if (body.sidecarUrl !== undefined && !boundedString(body.sidecarUrl, 1, 260)) return `card ${id} sidecarUrl is invalid`;
    if (body.cubeJsonUrl !== undefined && !boundedString(body.cubeJsonUrl, 1, 260)) return `card ${id} cubeJsonUrl is invalid`;
  }
  if (kind === "VoiceProfile") {
    if (!boundedString(body.agentName, 1, 40)) return `card ${id} agentName is required`;
    if (!boundedString(body.personaId, 1, 40) || !personaById(String(body.personaId))) {
      return `card ${id} personaId is not a persona`;
    }
    if (personaById(String(body.personaId))?.name !== body.agentName) {
      return `card ${id} agentName must match the persona`;
    }
    if (!boundedString(body.voiceModel, 1, 40) || !voiceById(String(body.voiceModel))) {
      return `card ${id} voiceModel is not a TTS model`;
    }
    if (!boundedString(body.tone, 1, 80)) return `card ${id} tone is required`;
    if (!boundedString(body.purpose, 1, 160)) return `card ${id} purpose is required`;
    if (!boundedString(body.domain, 1, 160)) return `card ${id} domain is required`;
    if (!boundedString(body.accent, 1, 80)) return `card ${id} accent is required`;
    if (!boundedString(body.traits, 12, 400)) return `card ${id} traits are required`;
    if (!Array.isArray(body.refs) || body.refs.length < 1 || body.refs.length > 8) return `card ${id} refs must contain 1 to 8 strings`;
    for (const reference of body.refs) {
      if (!boundedString(reference, 1, 80)) return `card ${id} ref is invalid`;
    }
    if (body.spectrogram2d !== "browser-profile-map") return `card ${id} spectrogram2d must be a browser profile map`;
    if (body.spectrogram3d !== "none" && body.spectrogram3d !== "library-cube-hook" && body.spectrogram3d !== "fixture-cube") {
      return `card ${id} spectrogram3d is not a known hook`;
    }
    if (body.notPodcast !== true) return `card ${id} must set notPodcast true`;
    if (!boundedString(body.disclaimer, 12, 400) || !String(body.disclaimer).toLowerCase().includes("not")) {
      return `card ${id} disclaimer must say the profile is not a podcast render`;
    }
    if (body.spectrogram3d === "library-cube-hook") {
      if (!boundedString(body.cubeJsonUrl, 1, 260)) return `card ${id} cubeJsonUrl is required for a library cube hook`;
    } else if (body.cubeJsonUrl) {
      return `card ${id} cubeJsonUrl is only set for a library-cube-hook`;
    }
  }
  if (kind === "ConnectorStatus") {
    if (typeof body.connectorId !== "string" || !CONNECTOR_IDS.includes(body.connectorId as (typeof CONNECTOR_IDS)[number])) {
      return `card ${id} has an unknown connector`;
    }
    if (typeof body.mode !== "string" || !CONNECTOR_MODES.includes(body.mode as (typeof CONNECTOR_MODES)[number])) {
      return `card ${id} has unknown connector mode`;
    }
    if (typeof body.authenticated !== "boolean") return "authenticated must be a boolean";
    if (!boundedString(body.detail, 1, 400)) return `card ${id} detail is required`;
  }
  return null;
}

function validateAdaptive(id: string, adaptive: unknown): string | null {
  if (!isRecord(adaptive)) return `card ${id} adaptive must be an object`;
  if (adaptive.type !== "AdaptiveCard") return `card ${id} adaptive.type must be AdaptiveCard`;
  if (!boundedString(adaptive.version, 1, 8)) return `card ${id} adaptive version is required`;
  if (!Array.isArray(adaptive.body) || adaptive.body.length === 0) return `card ${id} adaptive body is empty`;
  return null;
}

function boundedString(value: unknown, min: number, max: number): value is string {
  return typeof value === "string" && value.length >= min && value.length <= max;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
