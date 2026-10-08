import { personaById, voiceById, type Persona } from "./catalog";
import { previewImprove, type FixtureBuffer } from "./signal";

/** Side-pane selection that picks a browser voice-profile spectrogram. */
export interface VoiceSelection {
  engineId: string;
  engineTitle: string;
  agents: string[];
  voice: string;
  durationMin: number;
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

const ENGINES: Record<string, { scale: number; harmonics: number; rolloff: number; shift: number; bandwidth: number }> = {
  kokoro_onnx: { scale: 1.04, harmonics: 14, rolloff: 0.92, shift: 24, bandwidth: 110 },
  kokoro_dayour: { scale: 0.98, harmonics: 12, rolloff: 0.88, shift: 10, bandwidth: 120 },
  misaki: { scale: 1, harmonics: 6, rolloff: 1.4, shift: 0, bandwidth: 90 },
  vibevoice: { scale: 0.84, harmonics: 16, rolloff: 0.7, shift: -90, bandwidth: 150 },
  magpie: { scale: 1.16, harmonics: 9, rolloff: 1.25, shift: 210, bandwidth: 80 },
  pocket_tts: { scale: 0.94, harmonics: 5, rolloff: 1.7, shift: 45, bandwidth: 70 },
};

export function profilePreview(selection: VoiceSelection): ProfilePreview {
  const people = selection.agents.map((id) => personaById(id)).filter((item): item is Persona => Boolean(item));
  const primary = people[0] ?? personaById("alice");
  const voice = voiceById(selection.voice);
  const engine = engineShape(selection.engineId || selection.voice);
  const minutes = clamp(selection.durationMin, 1, 30);
  const bursts = Math.min(8, 1 + Math.round(minutes / 4));
  const before = primary
    ? renderLayers(layersFor(primary, people.slice(1), engine), bursts, minutes)
    : renderLayers([], bursts, minutes);
  const names = people.map((item) => item.name).join(", ") || "no persona";
  const voiceLabel = voice?.label ?? selection.voice;
  const model = selection.engineTitle || voiceLabel;
  const honesty = voice?.unavailable
    ? " This voice model is unavailable here."
    : voice && !voice.waveform
      ? " This voice model is G2P and does not emit a waveform."
      : "";
  return {
    key: [selection.engineId || "none", people.map((item) => item.id).join("+"), selection.voice, String(minutes)].join("~"),
    before,
    after: previewImprove(before),
    beforeTitle: `${names} before · ${voiceLabel}`,
    afterTitle: `${names} after (trim → HP → peak, browser only)`,
    heading: `Spectrogram · ${names} · ${model}`,
    caption:
      `Profile map for persona ${names} on voice model ${voiceLabel}, ${minutes} min, loaded card ${model}. ` +
      "Browser drawing of that profile, not a podcast render and not a Python spectrogram." +
      honesty,
  };
}

function layersFor(
  primaryPersona: Persona,
  extras: Persona[],
  engine: { scale: number; harmonics: number; rolloff: number; shift: number; bandwidth: number },
): Layer[] {
  const primary: Layer = {
    f0: primaryPersona.f0 * engine.scale,
    formants: primaryPersona.formants.map((freq, index) => freq + engine.shift + index * 12),
    harmonics: engine.harmonics,
    rolloff: engine.rolloff,
    bandwidth: engine.bandwidth,
    gain: 1,
    glide: 0.8,
  };
  const more = extras.map((persona, index): Layer => ({
    f0: persona.f0 * engine.scale,
    formants: persona.formants.map((freq, formant) => freq + engine.shift + formant * 20),
    harmonics: Math.max(4, engine.harmonics - 3),
    rolloff: engine.rolloff,
    bandwidth: engine.bandwidth,
    gain: 0.62 - index * 0.08,
    glide: 1.1,
  }));
  return [primary, ...more];
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
