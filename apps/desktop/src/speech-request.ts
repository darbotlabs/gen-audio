import { MAX_AGENTS, personaById, voiceById } from "./catalog";
import { engineManifest, managedVoiceReferences } from "./profiles";

export const MAX_SCRIPT_BYTES = 256 * 1024;

export interface TextAttachment {
  name: string;
  type: string;
  size: number;
  text(): Promise<string>;
}

export interface SpeechRequest {
  scriptText: string;
  agents: string[];
  voice: string;
  referenceIds?: string[];
  maxSeconds?: number;
  maxNewTokens?: number;
  seed?: number;
  execution?: ExecutionPreferences;
}

export interface ExecutionPreferences {
  backend: "auto" | "cpu" | "rocm" | "cuda";
  device: "auto" | "cpu" | "gpu";
  device_index: number;
  precision: "auto" | "float32" | "float16" | "bfloat16";
  attention?: "auto" | "math" | "efficient";
  fallback: "error" | "cpu";
}

function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** Text is sent intact. Never truncate it or replace it with a bundled example. */
export async function buildSpeechRequest(
  prompt: string,
  files: readonly TextAttachment[],
  selection: { agents: string[]; voice: string; referenceIds?: string[]; maxSeconds?: number; maxNewTokens?: number; seed?: number; execution?: ExecutionPreferences },
): Promise<SpeechRequest> {
  const agents = [...selection.agents];
  if (!agents.length || agents.length > MAX_AGENTS || new Set(agents).size !== agents.length
    || agents.some((id) => !personaById(id))) {
    throw new Error("Choose 1–8 distinct known personas.");
  }
  const model = voiceById(selection.voice);
  if (!model?.synthAdapter) throw new Error(`No speech adapter is available for ${model?.label ?? selection.voice}.`);
  if (selection.voice === "kokoro_onnx") {
    const unmapped = agents.find((id) => !personaById(id)?.refs.some((ref) => ref.startsWith("kokoro-pack:")));
    if (unmapped) throw new Error(`${personaById(unmapped)?.name} has no Kokoro pack mapping. Choose Alice or Frank.`);
  }
  let total = byteLength(prompt);
  if (total > MAX_SCRIPT_BYTES) throw new Error("Script exceeds 256 KiB; shorten it explicitly.");
  // Validate the entire attachment list before reading any file.
  if (files.length > 16) throw new Error("At most 16 text attachments are supported.");
  for (const file of files) {
    if (!/\.(txt|md)$/i.test(file.name) || (file.type && !["text/plain", "text/markdown"].includes(file.type))) {
      throw new Error(`Unsupported attachment "${file.name}". Only UTF-8 .txt and .md files are supported; nothing was sent.`);
    }
    if (!Number.isSafeInteger(file.size) || file.size < 0 || file.size > MAX_SCRIPT_BYTES) {
      throw new Error(`Attachment "${file.name}" exceeds 256 KiB.`);
    }
    total += file.size + 2;
    if (total > MAX_SCRIPT_BYTES) throw new Error("Combined script and attachments exceed 256 KiB.");
  }
  const parts = prompt ? [prompt] : [];
  for (const file of files) {
    const text = await file.text();
    if (text.includes("\uFFFD")) throw new Error(`Attachment "${file.name}" is not valid UTF-8 text.`);
    parts.push(text);
  }
  const scriptText = parts.join("\n\n");
  if (byteLength(scriptText) > MAX_SCRIPT_BYTES) throw new Error("Combined script exceeds 256 KiB.");
  if (!scriptText.trim()) throw new Error("Enter the words to speak, or attach a text script.");
  if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/.test(scriptText)) throw new Error("The script contains unsupported control characters.");
  validateSpeakerTurns(scriptText, agents.length);
  if (selection.voice === "vibevoice") {
    if (agents.length > 4) throw new Error("VibeVoice supports at most four personas per render.");
    if ([...scriptText].length > 8192 || byteLength(scriptText) > 32768) throw new Error("VibeVoice supports 8192 characters / 32768 UTF-8 bytes; shorten the script explicitly.");
    const references = selection.referenceIds ?? agents.map(id => String(engineManifest(id, "vibevoice")?.referenceId ?? ""));
    if (references.length !== agents.length || references.some(id => !managedVoiceReferences.some(ref => ref.id === id))) {
      throw new Error("Choose one managed VibeVoice reference for every selected persona.");
    }
    const maxSeconds = selection.maxSeconds ?? 30, maxNewTokens = selection.maxNewTokens ?? 512, seed = selection.seed ?? 0;
    if (!Number.isFinite(maxSeconds) || maxSeconds < 1 || maxSeconds > 60 || !Number.isSafeInteger(maxNewTokens) || maxNewTokens < 1 || maxNewTokens > 1024
      || !Number.isSafeInteger(seed) || seed < 0 || seed > 4294967295) throw new Error("Choose an output cap of 1–60 seconds, 1–1024 tokens, and a valid seed.");
    return { scriptText, agents, voice: selection.voice, referenceIds: [...references], maxSeconds, maxNewTokens, seed,
      ...(selection.execution ? { execution: { ...selection.execution } } : {}) };
  }
  return { scriptText, agents, voice: selection.voice };
}

function validateSpeakerTurns(script: string, agentCount: number): void {
  const lines = script.split(/\r?\n/).map((line) => line.trim()).filter((line) => line && !line.startsWith("#"));
  const header = /^Speaker\s+(\d+)\s*(?:\([^)]*\)\s*)?:\s*(.*)$/i;
  const hasHeaders = lines.some((line) => header.test(line));
  if (agentCount === 1 && !hasHeaders) return;
  let inTurn = false;
  let hasText = false;
  for (const line of lines) {
    const match = header.exec(line);
    if (match) {
      if (inTurn && !hasText) throw new Error("Each Speaker turn needs text.");
      const speaker = Number(match[1]);
      if (!Number.isSafeInteger(speaker) || speaker < 1 || speaker > agentCount) {
        throw new Error(`Speaker ${match[1]} has no selected persona.`);
      }
      inTurn = true;
      hasText = Boolean(match[2]);
    } else {
      if (!inTurn) throw new Error("Multiple personas require Speaker 1: / Speaker 2: turns in selection order.");
      hasText = true;
    }
  }
  if (!inTurn || !hasText) throw new Error("Each selected script needs nonempty Speaker N: turns.");
}
