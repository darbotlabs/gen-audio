import { type McpClient, type SpeechResult, masteredAudioResult, toolBody } from "./mcp-client";
import { registerPlayer, bindFloatingPlayback, getActiveClipId, closeFloatingPlayback } from "./playback";
import { readSelection } from "./studio";

interface PanelHost {
  client(): Promise<McpClient | null>;
  setScript(text: string): void;
}

export interface DraftPreview {
  prompt: string;
  promptSha256: string;
  request: Record<string, unknown>;
  connector: string;
}

export function draftPreview(payload: unknown, request: Record<string, unknown>, connector: string): DraftPreview {
  const body = toolBody(payload);
  if (typeof body.prompt !== "string" || !body.prompt || typeof body.promptSha256 !== "string"
    || !/^[a-f0-9]{64}$/.test(body.promptSha256)
    || !["gpt", "claude", "copilot", "gemini"].includes(connector)) {
    throw new Error("Invalid draft preview; nothing was sent to a provider.");
  }
  return { prompt: body.prompt, promptSha256: body.promptSha256, request: structuredClone(request), connector };
}

function element<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Missing authoring control: ${id}`);
  return node as T;
}

export function bindAuthoringPanel(host: PanelHost): { setAudioSource(source: SpeechResult | null): void } {
  const info = element("authoring-info");
  const script = element<HTMLTextAreaElement>("prompt");
  const output = element<HTMLPreElement>("authoring-result");
  const topic = element<HTMLTextAreaElement>("draft-topic");
  const style = element<HTMLSelectElement>("draft-style");
  const rewrite = element<HTMLInputElement>("draft-rewrite");
  const connector = element<HTMLSelectElement>("draft-connector");
  const indexing = element<HTMLSelectElement>("script-indexing");
  const apply = element<HTMLButtonElement>("apply-script");
  const send = element<HTMLButtonElement>("send-draft");
  const previewBox = element("draft-preview");
  const previewText = element<HTMLPreElement>("draft-preview-text");
  const recipient = element("draft-recipient");
  const master = element<HTMLButtonElement>("master-audio");
  const masterInfo = element("master-info");
  const masterAudio = element<HTMLAudioElement>("master-player");
  const masterDownload = element<HTMLAnchorElement>("master-download");
  const reportDownload = element<HTMLAnchorElement>("master-report-download");
  let reviewed: DraftPreview | null = null;
  let proposed: string | null = null;
  let source: SpeechResult | null = null;
  let audioUrl: string | null = null;
  let reportUrl: string | null = null;
  let revision = 0;
  let busy = false;
  let reviewRevision = 0;

  const resetReview = () => { reviewRevision++; reviewed = null; send.disabled = true; previewBox.hidden = true; };
  element("authoring-panel").addEventListener("input", resetReview);
  element("agents").addEventListener("change", resetReview);
  element("duration").addEventListener("change", resetReview);
  script.addEventListener("input", resetReview);

  async function client(): Promise<McpClient> {
    const value = await host.client();
    if (!value) throw new Error("The authenticated desktop backend is unavailable.");
    return value;
  }

  function showScript(body: Record<string, unknown>): void {
    if (typeof body.scriptText !== "string") throw new Error("Backend returned no validated script.");
    proposed = body.scriptText;
    const diagnostics = Array.isArray(body.warnings) ? body.warnings : [];
    output.textContent = proposed;
    const duration = typeof body.estimatedSeconds === "number" ? `${Math.round(body.estimatedSeconds)} seconds estimated` : "Duration is an estimate";
    info.textContent = `${body.wordCount ?? "?"} words; ${duration}. ${diagnostics.map(value => typeof value === "string" ? value : value && typeof value.message === "string" ? value.message : JSON.stringify(value)).join(" ")}`;
    apply.disabled = false;
  }

  async function perform(button: HTMLButtonElement, action: () => Promise<void>): Promise<void> {
    if (busy) return;
    busy = true;
    button.disabled = true;
    proposed = null;
    apply.disabled = true;
    info.textContent = "Working...";
    try { await action(); }
    catch (error) { info.textContent = error instanceof Error ? error.message : "Authoring failed."; }
    finally { busy = false; button.disabled = button === send && reviewed === null; }
  }

  const analyze = element<HTMLButtonElement>("analyze-script");
  analyze.addEventListener("click", () => { void perform(analyze, async () => {
    const selection = readSelection();
    showScript(toolBody(await (await client()).call("script_analyze", {
      scriptText: script.value, agents: selection.agents, indexing: indexing.value,
      durationMinutes: selection.durationMin,
    })));
  }); });

  const plan = element<HTMLButtonElement>("plan-draft");
  plan.addEventListener("click", () => { void perform(plan, async () => {
    resetReview();
    const version = reviewRevision;
    const selection = readSelection();
    const request = {
      topic: topic.value, agents: selection.agents, durationMinutes: selection.durationMin,
      language: "en", style: style.value,
      ...(rewrite.checked ? { existingScript: script.value } : {}),
    };
    const chosenConnector = connector.value;
    const result = await (await client()).call("draft_plan", { request, connector: chosenConnector });
    if (version !== reviewRevision) throw new Error("Draft inputs changed during planning. Preview the updated request before sending.");
    reviewed = draftPreview(result, request, chosenConnector);
    recipient.textContent = `Recipient: configured ${chosenConnector} provider. This message contains your supplied topic and selected persona details. Review the exact text below before sending.`;
    previewText.textContent = reviewed.prompt;
    previewBox.hidden = false;
    send.disabled = false;
    info.textContent = "Prompt prepared locally. No provider request has been sent.";
  }); });

  send.addEventListener("click", () => { void perform(send, async () => {
    const preview = reviewed;
    if (!preview) throw new Error("Preview the draft request before sending.");
    reviewed = null;
    showScript(toolBody(await (await client()).call("draft_generate", {
      request: preview.request, connector: preview.connector,
      promptSha256: preview.promptSha256, confirmed: true,
    })));
    previewBox.hidden = true;
  }); });

  apply.addEventListener("click", () => {
    if (proposed !== null) {
      host.setScript(proposed);
      resetReview();
      info.textContent = "Validated script applied. Review it before generating speech.";
    }
  });

  master.addEventListener("click", () => { void (async () => {
    const original = source;
    if (!original || master.disabled) return;
    const version = revision;
    master.disabled = true;
    masterInfo.textContent = "Measuring and mastering with FFmpeg; the original WAV is preserved.";
    try {
      const backend = await client();
      const result = masteredAudioResult(await backend.call("master_audio", { input: original.wav }));
      if (result.sourceSha256 !== original.sha256) throw new Error("Mastering source no longer matches the selected generation.");
      const blob = await backend.audioBlob(result);
      const report = await backend.manifestBlob(result);
      if (version !== revision) return;
      if (audioUrl) URL.revokeObjectURL(audioUrl);
      if (reportUrl) URL.revokeObjectURL(reportUrl);
      audioUrl = URL.createObjectURL(blob);
      reportUrl = URL.createObjectURL(report);
      masterAudio.src = audioUrl;
      masterAudio.hidden = false;
      masterAudio.dataset.playbackTitle = result.wav;
      registerPlayer("mastered-audio", masterAudio); bindFloatingPlayback();
      masterDownload.href = audioUrl;
      masterDownload.download = result.wav;
      masterDownload.hidden = false;
      reportDownload.href = reportUrl;
      reportDownload.download = result.manifest;
      reportDownload.hidden = false;
      masterInfo.textContent = `Podcast master ready (${result.duration_s.toFixed(2)}s). Original unchanged; measured loudness and provenance are in the report.`;
    } catch (error) {
      if (version === revision) masterInfo.textContent = error instanceof Error ? error.message : "Mastering failed.";
    } finally {
      if (version === revision) master.disabled = source === null;
    }
  })(); });

  return {
    setAudioSource(value) {
      if (getActiveClipId() === "mastered-audio") closeFloatingPlayback();
      revision++;
      source = value;
      master.disabled = !value;
      masterInfo.textContent = value ? "Optional: create a separate -16 LUFS podcast master. Requires local FFmpeg." : "";
      masterAudio.pause();
      masterAudio.removeAttribute("src");
      masterAudio.hidden = masterDownload.hidden = reportDownload.hidden = true;
      if (audioUrl) URL.revokeObjectURL(audioUrl);
      if (reportUrl) URL.revokeObjectURL(reportUrl);
      audioUrl = reportUrl = null;
    },
  };
}
