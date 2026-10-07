import { previewImprove, type FixtureBuffer } from "./signal";

/** Side-pane selection that picks a browser voice-profile spectrogram. */
export interface VoiceSelection {
  engineId: string;
  engineTitle: string;
  agent: string;
  agents: string[];
  voice: string;
  durationMin: number;
  perspectives: string[];
}

export interface ProfilePreview {
  key: string;
  before: FixtureBuffer;
  after: FixtureBuffer;
  beforeTitle: string;
  afterTitle: string;
  heading: string;
  caption: string;
}

export interface VoiceProfileSchema {
  agentname: string;
  tone: string;
  purpose: string;
  domain: string;
  accent: string;
  traits: string[];
  refs: string[];
  ttsModel: string;
  personaId: string;
}

interface Layer {
  f0: number;
  formants: number[];
  harmonics: number;
  rolloff: number;
  bandwidth: number;
  gain: number;
  glide: number;
}

/** Agent = persona (NOT LLM connector). Alice here is the persona, not af_heart. */
const PERSONAS: Record<string, { label: string; f0: number; formants: number[]; tone: string; purpose: string; domain: string; accent: string; traits: string[] }> = {
  anton: { label: "Anton", f0: 112, formants: [500, 1300, 2500], tone: "precise", purpose: "orchestration", domain: "systems", accent: "neutral-US", traits: ["direct", "technical"] },
  alice: { label: "Alice", f0: 196, formants: [730, 2050, 2970], tone: "warm", purpose: "narration", domain: "general", accent: "neutral-US", traits: ["clear", "friendly"] },
  khortana: { label: "Khortana", f0: 175, formants: [680, 1900, 2800], tone: "calm", purpose: "guidance", domain: "ops", accent: "neutral", traits: ["steady", "strategic"] },
  rocky: { label: "Rocky", f0: 98, formants: [450, 1100, 2300], tone: "bold", purpose: "drive", domain: "execution", accent: "US-urban", traits: ["energetic", "blunt"] },
  sensei: { label: "Sensei", f0: 130, formants: [560, 1500, 2600], tone: "measured", purpose: "review", domain: "quality", accent: "neutral", traits: ["exacting", "fair"] },
  optimus: { label: "Optimus", f0: 118, formants: [520, 1400, 2550], tone: "assertive", purpose: "gating", domain: "product", accent: "neutral", traits: ["decisive", "honest"] },
};

/** Voice = TTS / voice language model (NOT persona). */
const TTS_MODELS: Record<string, { label: string; scale: number; harmonics: number; rolloff: number; shift: number; bandwidth: number }> = {
  kokoro_onnx: { label: "kokoro-onnx", scale: 1.04, harmonics: 14, rolloff: 0.92, shift: 24, bandwidth: 110 },
  kokoro_dayour: { label: "dayour/kokoro", scale: 1.02, harmonics: 13, rolloff: 0.95, shift: 18, bandwidth: 105 },
  misaki_kokoro: { label: "misaki→kokoro", scale: 1.06, harmonics: 15, rolloff: 0.88, shift: 30, bandwidth: 115 },
  magpie: { label: "Magpie TTS", scale: 1.16, harmonics: 9, rolloff: 1.25, shift: 210, bandwidth: 80 },
  vibevoice: { label: "VibeVoice", scale: 0.84, harmonics: 16, rolloff: 0.7, shift: -90, bandwidth: 150 },
  pocket_tts: { label: "Pocket TTS", scale: 0.94, harmonics: 5, rolloff: 1.7, shift: 45, bandwidth: 70 },
};

const AGENT_GLIDE: Record<string, { shift: number; tilt: number; glide: number }> = {
  anton: { shift: 10, tilt: 1.05, glide: 0.9 },
  alice: { shift: 0, tilt: 1, glide: 0.8 },
  khortana: { shift: -20, tilt: 0.95, glide: 0.7 },
  rocky: { shift: 40, tilt: 1.15, glide: 1.5 },
  sensei: { shift: -10, tilt: 0.9, glide: 0.65 },
  optimus: { shift: 20, tilt: 1.1, glide: 1.0 },
};

export function voiceProfileSchema(selection: VoiceSelection): VoiceProfileSchema {
  const persona = PERSONAS[selection.agent] ?? PERSONAS.anton;
  const tts = TTS_MODELS[selection.voice] ?? TTS_MODELS.kokoro_onnx;
  return {
    agentname: persona.label,
    tone: persona.tone,
    purpose: persona.purpose,
    domain: persona.domain,
    accent: persona.accent,
    traits: [...persona.traits],
    refs: [
      `persona:${selection.agent}`,
      `tts:${selection.voice}`,
      selection.engineId ? `engine:${selection.engineId}` : "engine:none",
    ],
    ttsModel: tts.label,
    personaId: selection.agent,
  };
}

export function profilePreview(selection: VoiceSelection): ProfilePreview {
  const persona = PERSONAS[selection.agent] ?? PERSONAS.anton;
  const agent = AGENT_GLIDE[selection.agent] ?? AGENT_GLIDE.anton;
  const engine = ttsShape(selection.voice, selection.engineId);
  const minutes = clamp(selection.durationMin, 1, 30);
  const people = selection.perspectives.filter((name) => name.trim().length > 0);
  const castAgents = (selection.agents?.length ? selection.agents : [selection.agent])
    .map((id) => PERSONAS[id]?.label ?? id);
  const bursts = Math.min(8, 1 + Math.round(minutes / 4));
  const before = renderLayers(layersFor(persona, agent, engine, people), bursts, minutes);
  const tts = TTS_MODELS[selection.voice]?.label ?? selection.voice;
  const model = selection.engineTitle || tts;
  const cast = people.length > 0 ? people.join(", ") : "none";
  return {
    key: [selection.engineId || "none", selection.agent, selection.voice, String(minutes), castAgents.join("|"), people.join("|")].join("~"),
    before,
    after: previewImprove(before),
    beforeTitle: `${persona.label} before · TTS ${tts}`,
    afterTitle: `${persona.label} after (not Python)`,
    heading: `Spectrogram · Agent ${persona.label} · Voice ${tts}`,
    caption:
      `Agent persona ${persona.label} (${selection.agent}); Voice TTS ${tts} (${selection.voice}); ` +
      `agents on track [${castAgents.join(", ")}]; ${minutes} min; loaded model ${model}; perspectives ${cast}. ` +
      "Browser drawing of that voice profile — not a podcast render and not a Python spectrogram. " +
      "Alice persona ≠ af_heart id (collision fixed).",
  };
}

function layersFor(
  voice: { f0: number; formants: number[] },
  agent: { shift: number; tilt: number; glide: number },
  engine: { scale: number; harmonics: number; rolloff: number; shift: number; bandwidth: number },
  people: string[],
): Layer[] {
  const primary: Layer = {
    f0: voice.f0 * engine.scale,
    formants: voice.formants.map((freq, index) => freq + agent.shift + engine.shift + index * 12),
    harmonics: engine.harmonics,
    rolloff: engine.rolloff / agent.tilt,
    bandwidth: engine.bandwidth,
    gain: 1,
    glide: agent.glide,
  };
  const extras = people.map((name, index): Layer => {
    const hashed = hashText(name);
    const shift = (hashed % 280) - 140;
    return {
      f0: primary.f0 * (0.72 + (hashed % 50) / 100),
      formants: primary.formants.map((freq, formant) => freq + shift + formant * 40),
      harmonics: Math.max(4, engine.harmonics - 3),
      rolloff: engine.rolloff,
      bandwidth: engine.bandwidth,
      gain: 0.62 - index * 0.08,
      glide: agent.glide + 0.35,
    };
  });
  return [primary, ...extras];
}

function renderLayers(layers: Layer[], bursts: number, minutes: number): FixtureBuffer {
  const sampleRate = 16000;
  const seconds = 0.34 + bursts * 0.07;
  const length = Math.floor(seconds * sampleRate);
  const samples = new Float32Array(length);
  for (let index = 0; index < length; index += 1) {
    const t = index / sampleRate;
    const place = (t / seconds) * bursts;
    const local = place - Math.floor(place);
    const envelope = local < 0.8 ? Math.sin(Math.PI * (local / 0.8)) : 0;
    const glide = 1 + 0.05 * Math.sin(2 * Math.PI * (t * (0.6 + minutes / 40)));
    let sample = 0;
    for (const layer of layers) {
      const f0 = layer.f0 * (1 + 0.035 * Math.sin(2 * Math.PI * layer.glide * t));
      for (let harmonic = 1; harmonic <= layer.harmonics; harmonic += 1) {
        const freq = f0 * harmonic;
        if (freq > 7600) break;
        const shaped = formantGain(freq, layer.formants, layer.bandwidth * glide);
        const amp = shaped / harmonic ** layer.rolloff;
        sample += layer.gain * amp * Math.sin(2 * Math.PI * freq * t);
      }
    }
    samples[index] = sample * envelope;
  }
  normalize(samples);
  return { sampleRate, samples };
}

function formantGain(freq: number, formants: number[], bandwidth: number): number {
  let gain = 0.05;
  for (const center of formants) {
    const distance = (freq - center) / bandwidth;
    gain += Math.exp(-distance * distance);
  }
  return gain;
}

function ttsShape(voiceId: string, engineId: string): { scale: number; harmonics: number; rolloff: number; shift: number; bandwidth: number } {
  if (TTS_MODELS[voiceId]) {
    const t = TTS_MODELS[voiceId];
    return { scale: t.scale, harmonics: t.harmonics, rolloff: t.rolloff, shift: t.shift, bandwidth: t.bandwidth };
  }
  if (TTS_MODELS[engineId]) {
    const t = TTS_MODELS[engineId];
    return { scale: t.scale, harmonics: t.harmonics, rolloff: t.rolloff, shift: t.shift, bandwidth: t.bandwidth };
  }
  if (!voiceId && !engineId) return { scale: 1, harmonics: 8, rolloff: 1, shift: 0, bandwidth: 120 };
  const hashed = hashText(voiceId || engineId);
  return {
    scale: 0.88 + (hashed % 30) / 100,
    harmonics: 6 + (hashed % 8),
    rolloff: 0.8 + (hashed % 7) / 10,
    shift: (hashed % 220) - 110,
    bandwidth: 80 + (hashed % 60),
  };
}

function normalize(samples: Float32Array): void {
  let peak = 0;
  for (const sample of samples) peak = Math.max(peak, Math.abs(sample));
  const gain = 0.7 / (peak || 1);
  for (let index = 0; index < samples.length; index += 1) samples[index] *= gain;
}

function hashText(value: string): number {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, value));
}
