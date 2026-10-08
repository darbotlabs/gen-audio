import { clipDrivesCube } from "./clock-bind";
import { getCubeMeta, setCubeScrub, startLiveCubeClock, stopLiveCubeClock } from "./cubeview";
import type { PlayOrigin } from "./play-control";
import { Seeker, type MediaLike, type SeekResult } from "./seek";
export type { PlayOrigin };
/** What a play listener learns: who started it. Focus is not decided here (C1: Rust reducer, PR #4). */
export interface PlayInfo {
  origin: PlayOrigin;
}
/** HTML audio transport for library tiles. Missing files stay missing. */

const players = new Map<string, HTMLAudioElement>();
/** The source each clip's transport was rendered with (before any blob swap). */
const playerSources = new Map<string, string>();


let activeId: string | null = null;
/** The clip the cube is showing. The cube clock follows only this clip, not the last one played. */
let cubeClockClip: string | null = null;

/** Asset uid of the clock source clip (seconds on this uid drive the cube). */
let cubeClockUid: string | null = null;
const playListeners = new Set<(clipId: string, info: PlayInfo) => void>();

/** Reports a UI Play click (origin "user") to the control bus; set by main.ts. */
let reportUserPlay: (clipId: string) => void = () => {};

export function setUserPlayReporter(report: (clipId: string) => void): void {
  reportUserPlay = report;
}

/** One seeker for every library transport (deferred, verified seeks; see seek.ts). */
let seeker = new Seeker();

/** Test seam: swap the seeker (fake source ops, no settle delay). */
export function setSeeker(next: Seeker): void {
  seeker.releaseAll();
  seeker = next;
}

/** Seek an element and keep the cube and floater on the clock it actually reached. */
async function seekTo(clipId: string, audio: HTMLAudioElement, seconds: number): Promise<SeekResult> {
  activeId = clipId;
  // One cached blob URL per clip (seek.ts); the clip id keys it.
  const outcome = await seeker.seek(audio as unknown as MediaLike, seconds, clipId);
  if (outcome.status !== "superseded" && drivesCube(clipId)) {
    // ONE clock: the cube shows where the audio is, not where we asked it to go.
    const fraction = cubeFractionAt(outcome.ok ? outcome.actual : audio.currentTime, audio.duration);
    if (fraction !== null) setCubeScrub(fraction, { silent: true });
  }
  syncFloater(clipId);
  return outcome;
}

/** Test seam: register a transport element without building the tile DOM. */
export function registerPlayer(clipId: string, audio: HTMLAudioElement): void {
  const source = audio.src;
  const prior = playerSources.get(clipId);
  // Same clip, new source: its cached blob copies the old file, so revoke it.
  if (prior !== undefined && prior !== source) seeker.release(clipId);
  playerSources.set(clipId, source);
  players.set(clipId, audio);
}

/**
 * Drop transports whose element left the document (a re-render removed the
 * tile) and revoke their cached blob URLs. main.ts calls it after each render.
 */
export function releaseDetachedTransports(): string[] {
  const released: string[] = [];
  for (const [clipId, audio] of players) {
    if (audio.isConnected !== false) continue;
    players.delete(clipId);
    playerSources.delete(clipId);
    seeker.release(clipId);
    released.push(clipId);
  }
  return released;
}

/** Revoke every cached blob URL (page teardown). */
export function releaseAllSeekBlobs(): void {
  seeker.releaseAll();
}

export function setCubeClockClip(clipId: string | null, uid: string | null = null): void {
  cubeClockClip = clipId;
  cubeClockUid = clipId ? uid : null;
}

/** Shared clock source: the clip (legacy tile id and asset uid) whose seconds drive the cube. */
export function getClockSource(): { clipId: string | null; uid: string | null } {
  return { clipId: cubeClockClip, uid: cubeClockUid };
}

/** Called after a library clip starts playing, with its origin. Listeners must not move focus. */
export function onClipPlay(listener: (clipId: string, info: PlayInfo) => void): () => void {
  playListeners.add(listener);
  return () => playListeners.delete(listener);
}

/** Seconds on the clip -> cube scrub fraction, using the cube's own WAV duration when loaded. */
function cubeFractionAt(seconds: number, audioDuration: number): number | null {
  const clock = getCubeMeta()?.durationS ?? (Number.isFinite(audioDuration) && audioDuration > 0 ? audioDuration : 0);
  if (!(clock > 0) || !Number.isFinite(seconds)) return null;
  return Math.max(0, Math.min(1, seconds / clock));
}

/** Park the cube at the clip's current second (pause, end, paused seek). */
function syncCubeToClip(clipId: string): void {
  const audio = player(clipId);
  if (!audio || !drivesCube(clipId)) return;
  const fraction = cubeFractionAt(audio.currentTime, audio.duration);
  if (fraction !== null) setCubeScrub(fraction, { silent: true });
}

function drivesCube(clipId: string | null): boolean {
  return clipDrivesCube(clipId, cubeClockClip);
}

function cubeFraction(): number | null {
  const current = activeId ? player(activeId) : undefined;
  if (!drivesCube(activeId)) return null;
  if (!current || current.paused) return null;
  return cubeFractionAt(current.currentTime, current.duration);
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
  registerPlayer(clipId, audio);

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
    if (audio.paused) {
      reportUserPlay(clipId);
      void playClip(clipId, "user");
    } else pauseClip(clipId);
  });
  scrub.addEventListener("input", () => {
    const duration = audio.duration || durationHint || 0;
    if (duration > 0 && wrap.dataset.missing !== "1") {
      void seekTo(clipId, audio, (Number(scrub.value) / 1000) * duration);
    }
  });

  const row = document.createElement("div");
  row.className = "transport-row";
  row.append(toggle, time);
  wrap.append(audio, row, scrub);
  return wrap;
}

function player(clipId: string): HTMLAudioElement | undefined {
  const direct = players.get(clipId);
  if (direct) return direct;
  const tile = document.querySelector<HTMLElement>(`.library-tile[data-uid="${CSS.escape(clipId)}"]`);
  const id = tile?.dataset.id;
  return id ? players.get(id) : undefined;
}

export async function playClip(clipId: string, origin: PlayOrigin = "auto"): Promise<string> {
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
    const info: PlayInfo = { origin };
    for (const listener of playListeners) listener(clipId, info);
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

/**
 * Seek a library clip (MCP ui_playback seek, unified transport). Waits for
 * metadata, makes the source seekable if it is not, and reports "seeked"
 * only when currentTime actually landed (seek.ts).
 */
export async function seekClip(clipId: string, seconds: number): Promise<string> {
  return (await seekClipOutcome(clipId, seconds)).status;
}

/** Where a seek landed: what the window reports back for an MCP seek (ui_seek_report). */
export interface SeekOutcome {
  ok: boolean;
  /** currentTime after the attempt; null when there is no element or no number. */
  actual: number | null;
  status: string;
}

export async function seekClipOutcome(clipId: string, seconds: number): Promise<SeekOutcome> {
  const audio = player(clipId);
  if (!audio || !audio.src) return { ok: false, actual: null, status: "no wav" };
  const outcome = await seekTo(clipId, audio, seconds);
  return { ok: outcome.ok, actual: Number.isFinite(outcome.actual) ? outcome.actual : null, status: outcome.status };
}

/** Seek a specific library clip by shared-clock fraction 0..1 (the clip bound to the cube). */
export async function seekClipFraction(clipId: string, fraction: number): Promise<string> {
  const audio = player(clipId);
  if (!audio || !audio.src) return "no wav";
  const duration = audio.duration;
  if (!Number.isFinite(duration) || duration <= 0) return "no duration";
  // The scrub fraction is on the cube's clock (its WAV duration); convert to seconds on this clip.
  const clock = drivesCube(clipId) ? getCubeMeta()?.durationS ?? duration : duration;
  const outcome = await seekTo(clipId, audio, Math.min(duration, Math.max(0, Math.min(1, fraction)) * clock));
  return outcome.ok ? `seeked ${clipId}` : outcome.status;
}

export function isClipPlaying(clipId: string): boolean {
  const audio = player(clipId);
  return Boolean(audio && !audio.paused && !audio.ended);
}

export function getActiveClipId(): string | null {
  return activeId;
}

/** Seek the active library clip by shared-clock fraction 0..1. */
export async function seekActiveFraction(fraction: number): Promise<string> {
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
  const clipId = activeId;
  const audio = player(clipId);
  if (!audio || !audio.src) return "no wav";
  const duration = audio.duration;
  if (!Number.isFinite(duration) || duration <= 0) return "no duration";
  const outcome = await seekTo(clipId, audio, Math.max(0, Math.min(1, fraction)) * duration);
  return outcome.ok ? `seeked ${clipId}` : outcome.status;
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
    if (audio.paused) void playClip(activeId, "resume");
    else pauseClip(activeId);
  });
  ui.scrub.addEventListener("input", () => {
    if (!activeId) return;
    const audio = player(activeId);
    if (!audio || !audio.duration) return;
    const fraction = Number(ui.scrub.value) / 1000;
    void seekTo(activeId, audio, fraction * audio.duration);
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
      syncCubeToClip(clipId);
      if (!anyPlaying()) {
        setPlayingChrome(false);
        stopLiveCubeClock();
      }
    });
    audio.addEventListener("ended", () => {
      syncCubeToClip(clipId);
      if (!anyPlaying()) {
        setPlayingChrome(false);
        stopLiveCubeClock();
      }
    });
    audio.addEventListener("timeupdate", () => {
      if (activeId === clipId) {
        syncFloater(clipId);
        const duration = audio.duration;
        // Only a playing clip drives the cube here; a paused seek already set it.
        if (!audio.paused && drivesCube(clipId)) {
          const fraction = cubeFractionAt(audio.currentTime, duration);
          if (fraction !== null) setCubeScrub(fraction, { silent: true });
        }
      }
    });
  }
}
