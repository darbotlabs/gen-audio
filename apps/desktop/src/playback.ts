import { getCubeMeta, setCubeScrub, startLiveCubeClock, stopLiveCubeClock } from "./cubeview";
/** HTML audio transport for library tiles. Missing files stay missing. */

const players = new Map<string, HTMLAudioElement>();

let activeId: string | null = null;
/** The clip the cube is showing. The cube clock follows only this clip, not the last one played. */
let cubeClockClip: string | null = null;

export function setCubeClockClip(clipId: string | null): void {
  cubeClockClip = clipId;
}

function drivesCube(clipId: string | null): boolean {
  return Boolean(clipId) && (!cubeClockClip || clipId === cubeClockClip);
}

function cubeFraction(): number | null {
  const current = activeId ? player(activeId) : undefined;
  if (!drivesCube(activeId)) return null;
  if (!current || current.paused || !Number.isFinite(current.duration) || current.duration <= 0) return null;
  return current.currentTime / current.duration;
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const whole = Math.floor(seconds);
  const minutes = Math.floor(whole / 60);
  const rest = whole % 60;
  return `${minutes}:${rest.toString().padStart(2, "0")}`;
}

export function renderTransport(clipId: string, wavUrl: string | undefined, durationHint?: number): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "transport";
  wrap.dataset.clipId = clipId;
  const audio = document.createElement("audio");
  audio.preload = "metadata";
  audio.dataset.clipId = clipId;
  if (wavUrl) audio.src = wavUrl;
  players.set(clipId, audio);

  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "transport-toggle";
  toggle.dataset.action = "toggle-playback";
  toggle.dataset.clipId = clipId;
  toggle.textContent = "Play";

  const time = document.createElement("span");
  time.className = "transport-time";
  time.textContent = `0:00 / ${formatTime(durationHint ?? 0)}`;

  const scrub = document.createElement("input");
  scrub.type = "range";
  scrub.min = "0";
  scrub.max = "1000";
  scrub.value = "0";
  scrub.dataset.action = "scrub";
  scrub.setAttribute("aria-label", `Scrub ${clipId}`);

  const setMissing = (reason: string) => {
    toggle.disabled = true;
    scrub.disabled = true;
    time.textContent = reason;
    wrap.dataset.missing = "1";
  };

  if (!wavUrl) {
    setMissing("No WAV");
  }

  audio.addEventListener("error", () => {
    setMissing("WAV missing");
  });
  audio.addEventListener("loadedmetadata", () => {
    time.textContent = `0:00 / ${formatTime(audio.duration)}`;
    wrap.dataset.missing = "0";
  });
  audio.addEventListener("timeupdate", () => {
    const duration = audio.duration || durationHint || 0;
    time.textContent = `${formatTime(audio.currentTime)} / ${formatTime(duration)}`;
    if (!scrub.matches(":active") && duration > 0) {
      scrub.value = String(Math.round((audio.currentTime / duration) * 1000));
    }
  });
  audio.addEventListener("play", () => {
    toggle.textContent = "Pause";
    toggle.setAttribute("aria-pressed", "true");
  });
  audio.addEventListener("pause", () => {
    toggle.textContent = "Play";
    toggle.setAttribute("aria-pressed", "false");
  });
  audio.addEventListener("ended", () => {
    toggle.textContent = "Play";
  });

  toggle.addEventListener("click", (event) => {
    event.stopPropagation();
    if (audio.paused) void playClip(clipId);
    else pauseClip(clipId);
  });
  scrub.addEventListener("input", () => {
    const duration = audio.duration || durationHint || 0;
    if (duration > 0 && wrap.dataset.missing !== "1") {
      audio.currentTime = (Number(scrub.value) / 1000) * duration;
    }
  });

  const row = document.createElement("div");
  row.className = "transport-row";
  row.append(toggle, time);
  wrap.append(audio, row, scrub);
  return wrap;
}

function player(clipId: string): HTMLAudioElement | undefined {
  return players.get(clipId);
}

export async function playClip(clipId: string): Promise<string> {
  const audio = player(clipId);
  if (!audio || !audio.src) return "no wav";
  for (const [id, other] of players) {
    if (id !== clipId && !other.paused) other.pause();
  }
  try {
    activeId = clipId;
    setPlayingChrome(true);
    await audio.play();
    syncFloater(clipId);
    startLiveCubeClock(cubeFraction);
    return "playing";
  } catch (error) {
    if (!anyPlaying()) setPlayingChrome(false);
    return `play failed: ${String(error)}`;
  }
}

export function pauseClip(clipId: string): string {
  const audio = player(clipId);
  if (!audio) return "no player";
  audio.pause();
  let playing = false;
  for (const other of players.values()) {
    if (!other.paused && !other.ended) { playing = true; break; }
  }
  if (!playing) stopLiveCubeClock();
  return "paused";
}

export function seekClip(clipId: string, seconds: number): string {
  const audio = player(clipId);
  if (!audio || !audio.src) return "no wav";
  if (!Number.isFinite(seconds) || seconds < 0) return "bad seek";
  const duration = audio.duration;
  if (Number.isFinite(duration) && seconds > duration) return "past end";
  audio.currentTime = seconds;
  activeId = clipId;
  // Keep ONE clock: MCP/UI seek must slice bitdot layers + floating scrub together,
  // but only for the clip the cube is showing.
  if (drivesCube(clipId)) {
    // Before metadata loads, audio.duration is NaN; the cube JSON carries the same WAV duration.
    const clock = Number.isFinite(duration) && duration > 0 ? duration : getCubeMeta()?.durationS ?? 0;
    if (clock > 0) setCubeScrub(Math.min(1, seconds / clock), { silent: true });
  }
  syncFloater(clipId);
  if (audio.readyState > 0 && audio.seekable.length === 0) {
    return `cube at ${formatTime(seconds)}; this WAV source does not support seeking`;
  }
  return "seeked";
}


/** Seek a specific library clip by shared-clock fraction 0..1 (the clip bound to the cube). */
export function seekClipFraction(clipId: string, fraction: number): string {
  const audio = player(clipId);
  if (!audio || !audio.src) return "no wav";
  const duration = audio.duration;
  if (!Number.isFinite(duration) || duration <= 0) return "no duration";
  activeId = clipId;
  audio.currentTime = Math.max(0, Math.min(1, fraction)) * duration;
  syncFloater(clipId);
  return `seeked ${clipId}`;
}

export function isClipPlaying(clipId: string): boolean {
  const audio = player(clipId);
  return Boolean(audio && !audio.paused && !audio.ended);
}

export function getActiveClipId(): string | null {
  return activeId;
}

/** Seek the active library clip by shared-clock fraction 0..1. */
export function seekActiveFraction(fraction: number): string {
  if (!activeId) {
    // Prefer first available player with a WAV.
    for (const [id, audio] of players) {
      if (audio.src) {
        activeId = id;
        break;
      }
    }
  }
  if (!activeId) return "no player";
  const audio = player(activeId);
  if (!audio || !audio.src) return "no wav";
  const duration = audio.duration;
  if (!Number.isFinite(duration) || duration <= 0) return "no duration";
  const clamped = Math.max(0, Math.min(1, fraction));
  audio.currentTime = clamped * duration;
  syncFloater(activeId);
  return `seeked ${activeId}`;
}

/** Floating transport + viewport playing chrome. */

function floater(): {
  root: HTMLElement;
  btn: HTMLButtonElement;
  title: HTMLElement;
  scrub: HTMLInputElement;
  time: HTMLElement;
} | null {
  const root = document.querySelector<HTMLElement>("#floating-playback");
  const btn = document.querySelector<HTMLButtonElement>("#fp-playpause");
  const title = document.querySelector<HTMLElement>("#fp-title");
  const scrub = document.querySelector<HTMLInputElement>("#fp-scrub");
  const time = document.querySelector<HTMLElement>("#fp-time");
  if (!root || !btn || !title || !scrub || !time) return null;
  return { root, btn, title, scrub, time };
}

function syncFloater(clipId: string): void {
  const audio = player(clipId);
  const ui = floater();
  if (!audio || !ui) return;
  const tile = document.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(clipId)}"]`);
  ui.title.textContent = tile?.querySelector("h2")?.textContent?.trim() || clipId;
  const duration = audio.duration || 0;
  const cur = audio.currentTime || 0;
  ui.time.textContent = `${formatTime(cur)} / ${formatTime(duration)}`;
  ui.scrub.value = duration > 0 ? String(Math.round((cur / duration) * 1000)) : "0";
  ui.btn.textContent = audio.paused ? "Play" : "Pause";
}

function setPlayingChrome(on: boolean): void {
  document.querySelector(".viewport")?.classList.toggle("is-playing", on);
  const ui = floater();
  if (!ui) return;
  ui.root.hidden = !on;
}

function anyPlaying(): boolean {
  for (const audio of players.values()) {
    if (!audio.paused && !audio.ended) return true;
  }
  return false;
}

export function bindFloatingPlayback(): void {
  const ui = floater();
  if (!ui || ui.root.dataset.bound === "1") return;
  ui.root.dataset.bound = "1";
  ui.btn.addEventListener("click", () => {
    if (!activeId) return;
    const audio = player(activeId);
    if (!audio) return;
    if (audio.paused) void playClip(activeId);
    else pauseClip(activeId);
  });
  ui.scrub.addEventListener("input", () => {
    if (!activeId) return;
    const audio = player(activeId);
    if (!audio || !audio.duration) return;
    const fraction = Number(ui.scrub.value) / 1000;
    audio.currentTime = fraction * audio.duration;
    if (drivesCube(activeId)) setCubeScrub(fraction);
    syncFloater(activeId);
  });
  for (const [clipId, audio] of players) {
    audio.addEventListener("play", () => {
      activeId = clipId;
      setPlayingChrome(true);
      syncFloater(clipId);
      startLiveCubeClock(cubeFraction);
    });
    audio.addEventListener("pause", () => {
      if (activeId === clipId) syncFloater(clipId);
      if (!anyPlaying()) {
        setPlayingChrome(false);
        stopLiveCubeClock();
      }
    });
    audio.addEventListener("ended", () => {
      if (!anyPlaying()) {
        setPlayingChrome(false);
        stopLiveCubeClock();
      }
    });
    audio.addEventListener("timeupdate", () => {
      if (activeId === clipId) {
        syncFloater(clipId);
        const duration = audio.duration;
        // Only a playing clip drives the cube here. A paused seek already set the cube;
        // a source without range support snaps currentTime back to 0 and must not undo it.
        if (!audio.paused && drivesCube(clipId) && Number.isFinite(duration) && duration > 0) {
          setCubeScrub(audio.currentTime / duration, { silent: true });
        }
      }
    });
  }
}
