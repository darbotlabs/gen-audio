/** HTML audio transport for library tiles. Missing files stay missing. */

const players = new Map<string, HTMLAudioElement>();

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
    await audio.play();
    return "playing";
  } catch (error) {
    return `play failed: ${String(error)}`;
  }
}

export function pauseClip(clipId: string): string {
  const audio = player(clipId);
  if (!audio) return "no player";
  audio.pause();
  return "paused";
}

export function seekClip(clipId: string, seconds: number): string {
  const audio = player(clipId);
  if (!audio || !audio.src) return "no wav";
  if (!Number.isFinite(seconds) || seconds < 0) return "bad seek";
  const duration = audio.duration;
  if (Number.isFinite(duration) && seconds > duration) return "past end";
  audio.currentTime = seconds;
  return "seeked";
}
