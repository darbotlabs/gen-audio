/** Agent personas vs TTS voice models. Connector ids are not agents. */

export const MAX_AGENTS = 8;

export interface Persona {
  id: string;
  name: string;
  tone: string;
  purpose: string;
  domain: string;
  accent: string;
  traits: string;
  refs: string[];
  f0: number;
  formants: number[];
}

export interface VoiceModel {
  id: string;
  label: string;
  waveform: boolean;
  synthAdapter: boolean;
  unavailable: boolean;
  /** No in-app adapter; library clips were rendered elsewhere (availability offline_only). */
  offlineReason?: string;
  note: string;
}

export const PERSONAS: Persona[] = [
  {
    id: "anton",
    name: "Anton",
    tone: "measured",
    purpose: "Host a Gen-Audio briefing",
    domain: "Desktop app and SDK layout",
    accent: "General American",
    traits: "Low-mid pitch, even pace, short pauses. Not a connector model.",
    refs: ["persona:anton"],
    f0: 102,
    formants: [500, 1400, 2400],
  },
  {
    id: "alice",
    name: "Alice",
    tone: "warm",
    purpose: "Walk through a labeled example",
    domain: "SDK onboarding",
    accent: "General American",
    traits: "Persona Alice is the agent name. Kokoro pack id af_heart is a reference on this profile, not the Voice selector.",
    refs: ["kokoro-pack:af_heart", "persona:alice"],
    f0: 196,
    formants: [730, 2050, 2970],
  },
  {
    id: "khortana",
    name: "Khortana",
    tone: "bright",
    purpose: "Offer a contrasting take",
    domain: "Product critique",
    accent: "General American",
    traits: "Higher placement and a quicker glide. Not a TTS model id.",
    refs: ["persona:khortana"],
    f0: 220,
    formants: [800, 2200, 3100],
  },
  {
    id: "rocky",
    name: "Rocky",
    tone: "gravel",
    purpose: "Short reactions between turns",
    domain: "Studio banter",
    accent: "General American",
    traits: "Low pitch and a narrow band. Not a TTS model id.",
    refs: ["persona:rocky"],
    f0: 90,
    formants: [450, 1100, 2200],
  },
  {
    id: "optimus",
    name: "Optimus Timelarp",
    tone: "assertive",
    purpose: "Gate and harden product stamps",
    domain: "Product / A-E stamps",
    accent: "General American",
    traits: "Decisive, honest, screenshot-proof. AP-7 hard kill. Not a TTS model id.",
    refs: ["persona:optimus", "tts:kokoro_onnx", "cube:library_cube_explainer", "clip:lib-cube-explainer"],
    f0: 118,
    formants: [520, 1400, 2550],
  },
  {
    id: "frank",
    name: "Frank",
    tone: "plain",
    purpose: "Second chair on a briefing",
    domain: "Multi-speaker scripts",
    accent: "General American",
    traits: "Persona Frank is the agent name. Kokoro pack id am_michael is a reference, not the Voice selector.",
    refs: ["kokoro-pack:am_michael", "persona:frank"],
    f0: 108,
    formants: [480, 1160, 2390],
  },
  {
    id: "nova",
    name: "Nova",
    tone: "clear",
    purpose: "Read a status line",
    domain: "Readiness and health",
    accent: "General American",
    traits: "Mid pitch, little glide. Not a connector.",
    refs: ["persona:nova"],
    f0: 165,
    formants: [650, 1750, 2750],
  },
  {
    id: "ivo",
    name: "Ivo",
    tone: "dry",
    purpose: "Name what is stubbed",
    domain: "Engine honesty",
    accent: "General American",
    traits: "Flat delivery. Magpie, VibeVoice, and Pocket stay unavailable when this persona is selected.",
    refs: ["persona:ivo"],
    f0: 128,
    formants: [540, 1500, 2500],
  },
  {
    id: "sable",
    name: "Sable",
    tone: "soft",
    purpose: "Close a segment",
    domain: "Clip outros",
    accent: "General American",
    traits: "Softer high end. Does not imply a video track.",
    refs: ["persona:sable"],
    f0: 185,
    formants: [700, 1900, 2900],
  },
];

export const VOICE_MODELS: VoiceModel[] = [
  {
    id: "kokoro_onnx",
    label: "kokoro-onnx",
    waveform: true,
    synthAdapter: true,
    unavailable: false,
    note: "Python kokoro-onnx path. Needs local ONNX and voices files. Weights are not in this repo.",
  },
  {
    id: "kokoro_dayour",
    label: "dayour/kokoro (offline only)",
    waveform: true,
    synthAdapter: false,
    unavailable: true,
    note: "dayour/kokoro torch runtime. Not vendored here. Its library WAVs are offline runs; this repo has no synth adapter.",
    offlineReason: "offline runs only: rendered with the dayour/kokoro torch runtime outside this app; no in-app adapter, so Generate cannot produce it",
  },
  {
    id: "misaki",
    label: "misaki (G2P, not a waveform)",
    waveform: false,
    synthAdapter: false,
    unavailable: false,
    note: "Grapheme-to-phoneme for Kokoro. It does not emit a waveform by itself.",
    offlineReason: "G2P only, used offline for the misaki\u2192kokoro clip; no in-app adapter",
  },
  {
    id: "vibevoice",
    label: "VibeVoice (unavailable)",
    waveform: true,
    synthAdapter: false,
    unavailable: true,
    note: "No adapter and no verified synth in this app.",
  },
  {
    id: "magpie",
    label: "Magpie (unavailable)",
    waveform: true,
    synthAdapter: false,
    unavailable: true,
    note: "GGUF may exist on a machine; magpie-tts.cpp is not built here. No fake audio.",
  },
  {
    id: "pocket_tts",
    label: "Pocket TTS (unavailable)",
    waveform: true,
    synthAdapter: false,
    unavailable: true,
    note: "pocket_tts is not installed in this app. No teaser audio is reused.",
  },
];

export const SLIDE_IDS = ["models", "profiles", "studio", "library", "video", "spatial", "connectors", "pipeline"] as const;

export function personaById(id: string): Persona | undefined {
  return PERSONAS.find((item) => item.id === id);
}

export function voiceById(id: string): VoiceModel | undefined {
  return VOICE_MODELS.find((item) => item.id === id);
}
