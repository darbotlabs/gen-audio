import { previewImprove, type FixtureBuffer } from "./signal";

/** Side-pane selection that picks a browser voice-profile spectrogram. */
export interface VoiceSelection {
  engineId: string;
  engineTitle: string;
  agent: string;
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

interface Layer {
  f0: number;
  formants: number[];
  harmonics: number;
  rolloff: number;
  bandwidth: number;
  gain: number;
  glide: number;
}

const VOICES: Record<string, { label: string; f0: number; formants: number[] }> = {
  af_heart: { label: "Alice", f0: 196, formants: [730, 2050, 2970] },
  am_michael: { label: "Frank", f0: 108, formants: [480, 1160, 2390] },
  cast: { label: "Cast", f0: 142, formants: [610, 1620, 2680] },
};

const AGENTS: Record<string, { shift: number; tilt: number; glide: number }> = {
  local: { shift: 0, tilt: 1, glide: 0.8 },
  copilot: { shift: 70, tilt: 1.2, glide: 1.4 },
  claude: { shift: -80, tilt: 0.82, glide: 0.55 },
  gpt: { shift: 30, tilt: 1.05, glide: 1.1 },
  gemini: { shift: 160, tilt: 1.35, glide: 1.8 },
};

const ENGINES: Record<string, { scale: number; harmonics: number; rolloff: number; shift: number; bandwidth: number }> = {
  kokoro_onnx: { scale: 1.04, harmonics: 14, rolloff: 0.92, shift: 24, bandwidth: 110 },
  vibevoice: { scale: 0.84, harmonics: 16, rolloff: 0.7, shift: -90, bandwidth: 150 },
  magpie: { scale: 1.16, harmonics: 9, rolloff: 1.25, shift: 210, bandwidth: 80 },
  pocket_tts: { scale: 0.94, harmonics: 5, rolloff: 1.7, shift: 45, bandwidth: 70 },
};

export function profilePreview(selection: VoiceSelection): ProfilePreview {
  const voice = VOICES[selection.voice] ?? VOICES.cast;
  const agent = AGENTS[selection.agent] ?? AGENTS.local;
  const engine = engineShape(selection.engineId);
  const minutes = clamp(selection.durationMin, 1, 30);
  const people = selection.perspectives.filter((name) => name.trim().length > 0);
  const bursts = Math.min(8, 1 + Math.round(minutes / 4));
  const before = renderLayers(layersFor(voice, agent, engine, people), bursts, minutes);
  const label = voice.label;
  const model = selection.engineTitle || "no model";
  const cast = people.length > 0 ? people.join(", ") : "none";
  return {
    key: [selection.engineId || "none", selection.agent, selection.voice, String(minutes), people.join("|")].join("~"),
    before,
    after: previewImprove(before),
    beforeTitle: `${label} before · ${selection.agent}`,
    afterTitle: `${label} after (not Python)`,
    heading: `Spectrogram · ${label} · ${model}`,
    caption:
      `Profile map ${label} (${selection.voice}), agent ${selection.agent}, ${minutes} min, ` +
      `model ${model}, perspectives ${cast}. Browser drawing of that voice profile, ` +
      "not a podcast render and not a Python spectrogram.",
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

function engineShape(engineId: string): { scale: number; harmonics: number; rolloff: number; shift: number; bandwidth: number } {
  if (ENGINES[engineId]) return ENGINES[engineId];
  if (!engineId) return { scale: 1, harmonics: 8, rolloff: 1, shift: 0, bandwidth: 120 };
  const hashed = hashText(engineId);
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
