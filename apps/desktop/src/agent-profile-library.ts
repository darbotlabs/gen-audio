import { type McpClient, toolBody } from "./mcp-client";
import { applySidepane } from "./studio";
import { goToSlideId } from "./render";
import { bindFloatingPlayback, registerPlayer, releaseDetachedTransports, getActiveClipId, closeFloatingPlayback } from "./playback";

/** One delegated handler survives native face changes and viewport refreshes. */
export function bindAgentProfileLibrary(board: HTMLElement, client: () => Promise<McpClient | null>, notice: (message: string) => void): () => void {
  const resources = new Map<HTMLAudioElement, string>();
  const pending = new WeakSet<HTMLButtonElement>();
  const release = (): void => {
    if (getActiveClipId() === "profile-reference") closeFloatingPlayback();
    for (const [audio, url] of resources) { audio.pause(); audio.removeAttribute("src"); audio.load(); URL.revokeObjectURL(url); }
    resources.clear();
    releaseDetachedTransports();
  };
  board.addEventListener("click", event => {
    const button = (event.target as Element).closest<HTMLButtonElement>("button[data-action]");
    if (!button) return;
    if (button.dataset.action === "use-agent" && button.dataset.personaId) {
      event.stopPropagation(); applySidepane({ agents: [button.dataset.personaId] }); goToSlideId(board, "studio");
      document.querySelector<HTMLTextAreaElement>("#prompt")?.focus();
      notice(`Selected ${button.dataset.personaId} for the Studio.`);
    } else if (button.dataset.action === "open-profile-project" && button.dataset.projectSlide) {
      event.stopPropagation(); goToSlideId(board, button.dataset.projectSlide);
      if (button.dataset.projectSlide === "studio") document.querySelector<HTMLTextAreaElement>("#prompt")?.focus();
    } else if (button.dataset.action === "audition-reference") {
      event.stopPropagation();
      if (pending.has(button)) return;
      const row = button.closest(".profile-sample");
      const audio = row?.querySelector("audio"); const status = row?.querySelector<HTMLElement>("[data-sample-status]");
      if (!audio || !status) return;
      pending.add(button); button.disabled = true; status.textContent = "Verifying reference sample…";
      void (async () => {
        try {
          const backend = await client(); if (!backend) throw new Error("Connect to the desktop backend to audition references.");
          const result = toolBody(await backend.call("voice_reference_prepare", { referenceId: button.dataset.referenceId }));
          if (result.id !== button.dataset.referenceId || result.synthesizedSpeech !== false || typeof result.wav !== "string" || typeof result.sha256 !== "string") throw new Error("Invalid reference preparation receipt.");
          const blob = await backend.referenceBlob({ wav: result.wav, sha256: result.sha256 });
          if (!audio.isConnected) return;
          release(); const url = URL.createObjectURL(blob); resources.set(audio, url); audio.src = url;
          audio.dataset.playbackTitle = `${button.dataset.referenceId} · Reference sample`;
          registerPlayer("profile-reference", audio); bindFloatingPlayback();
          status.textContent = "Verified acoustic reference. Use the audio controls to listen.";
          // Explicit playback after async verification can require a second user gesture.
          try { await audio.play(); } catch { /* Native controls remain available. */ }
        } catch (error) { status.textContent = error instanceof Error ? error.message : "Reference audition failed."; }
        finally { pending.delete(button); button.disabled = false; }
      })();
    }
  });
  window.addEventListener("pagehide", release);
  return release;
}
