import type { ErrorObject } from "ajv";
import { viewportValidator, profileValidator } from "./generated/contract-validators.js";

export const CARD_KINDS = [
  "EngineStatus", "SpectrogramPanel", "Cube3D", "PodcastCast", "ServeHealth",
  "BenchmarkCompare", "ConnectorStatus", "LibraryClip", "VoiceProfile",
] as const;
export const CONNECTOR_IDS = ["mcp", "acp", "harness", "copilot", "claude", "gpt", "gemini", "local"] as const;
export const CONNECTOR_MODES = ["mock", "live", "local", "token_present", "misconfigured"] as const;
export const ENGINE_STATUSES = ["implemented", "external", "library", "weights_absent", "unavailable"] as const;
export const LIBRARY_STATUSES = ["ok", "running", "weights_absent", "unavailable", "external"] as const;
export const CARD_UID = /^ga:card:[a-z2-7]{25}[aeimquy4]$(?![\s\S])/;
export type CardKind = (typeof CARD_KINDS)[number];
export type FixturePolicy = "allowed" | "release";

export interface ViewportCard {
  id: string;
  uid?: string;
  kind: CardKind;
  title: string;
  span?: number;
  body: Record<string, unknown>;
  adaptive?: { type: "AdaptiveCard"; version: string; body: unknown[] };
}

export interface ViewportDocument {
  version: "1.0";
  title: string;
  columns?: number;
  cards: ViewportCard[];
}

function describe(errors: ErrorObject[] | null | undefined): string {
  const error = errors?.find(item => item.instancePath.endsWith("/spectrogram3d")) ?? errors?.[0];
  if (!error) return "invalid document";
  if (error.instancePath.endsWith("/sourceNote") && error.keyword === "pattern") {
    return "benchmark sourceNote must disclose that figures were not measured";
  }
  if (error.instancePath.endsWith("/sourceNote") && error.keyword === "not") {
    return "benchmark marked measured contradicts its sourceNote";
  }
  return `${error.instancePath || "/"} ${error.message}: ${JSON.stringify(error.params)}`;
}

/** Schemas validate content; production callers explicitly disallow reference benchmarks. */
export function validateViewport(document: unknown, fixtures: FixturePolicy = "allowed"): string | null {
  if (!viewportValidator(document)) return describe(viewportValidator.errors);
  const seen = new Set<string>();
  for (const card of document.cards) {
    if (seen.has(card.id)) return `duplicate card id ${card.id}`;
    seen.add(card.id);
    if (card.kind === "BenchmarkCompare") {
      const unmeasured = /not (?:re)?measured/i.test(String(card.body.sourceNote));
      if (card.body.measuredHere === false && !unmeasured) return "benchmark sourceNote must disclose that figures were not measured";
      if (card.body.measuredHere === true && unmeasured) return "benchmark marked measured contradicts its sourceNote";
      if (fixtures === "release" && card.body.measuredHere === false) return "release benchmark figures were not measured";
    }
  }
  return null;
}

export function validateVoiceProfile(profile: unknown): string | null {
  return profileValidator(profile) ? null : describe(profileValidator.errors);
}

/** Unicode code points, matching JSON Schema and Rust chars(). */
export function charLength(value: string): number {
  return [...value].length;
}
