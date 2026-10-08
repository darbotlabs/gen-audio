// Flat 2D spectrogram strip on an audio-clip livetile.
//
// Data: the clip's spectrogram_2d asset (a PNG computed from the real WAV by
// gen_audio.spectrogram_strip). The playhead is transport-local: it reads this
// tile's own <audio>.currentTime in seconds every frame while playing, and is
// placed against the clip duration. Past the strip's coverage it is marked
// "beyond". Clips without data say so; nothing is drawn in their place.

import { copyUidButton, glyphMark } from "./glyph";
import type { AssetEnvelope, LibraryCatalog } from "./library-assets";
import { mediaUrl } from "./library-assets";

const STRIP_HEIGHT = 48;

function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const whole = Math.floor(seconds);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

function intField(asset: AssetEnvelope, key: string): number {
  const value = asset.fields[key];
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

export interface StripHandle {
  root: HTMLElement;
  redraw(): void;
}

/** Honest empty strip: says why there is no spectrogram. */
export function emptyStrip(reason: string): HTMLElement {
  const root = document.createElement("div");
  root.className = "spec-strip";
  root.dataset.state = "none";
  const label = document.createElement("p");
  label.className = "spec-strip-label";
  label.textContent = `No spectrogram \u2014 ${reason}`;
  root.append(label);
  return root;
}

export function spectrogramStrip(spec: AssetEnvelope, clip: AssetEnvelope, audio: HTMLAudioElement | null): StripHandle {
  const root = document.createElement("div");
  root.className = "spec-strip";
  root.dataset.state = "loading";
  root.dataset.uid = spec.uid;
  root.dataset.clipUid = clip.uid;
  const canvas = document.createElement("canvas");
  canvas.className = "spec-strip-canvas";
  canvas.setAttribute("role", "img");
  const label = document.createElement("p");
  label.className = "spec-strip-label";
  root.append(canvas, label);

  const durationMs = intField(clip, "duration_ms") || intField(spec, "duration_ms");
  const coversMs = intField(spec, "covers_ms");
  const bands = intField(spec, "n_bands");
  // Caption text comes from the clip's honesty claims (status -> honesty table), never hard-coded.
  const source = clip.honesty?.claims?.includes("real_wav") ? "real WAV" : "WAV (no real_wav claim)";
  const caption = `2D spectrogram \u00b7 ${bands} bands \u00b7 ${source}`;
  canvas.setAttribute("aria-label", `${caption}, ${formatClock(durationMs / 1000)} long. Playhead follows this tile's audio.`);
  const image = new Image();
  let ready = false;
  let frame = 0;

  const seconds = () => (audio && Number.isFinite(audio.currentTime) ? audio.currentTime : 0);

  const redraw = () => {
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const ratio = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.round((canvas.clientWidth || 240) * ratio));
    const height = Math.max(1, Math.round((canvas.clientHeight || STRIP_HEIGHT) * ratio));
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;
    ctx.fillStyle = "#06090f";
    ctx.fillRect(0, 0, width, height);
    if (!ready || durationMs <= 0) return;
    // Time axis is the clip duration; the image spans only what it covers (never stretched past it).
    const coveredWidth = Math.min(width, (coversMs / durationMs) * width);
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(image, 0, 0, image.naturalWidth, image.naturalHeight, 0, 0, coveredWidth, height);
    if (coveredWidth < width - 0.5) {
      ctx.fillStyle = "rgba(255,255,255,0.06)";
      ctx.fillRect(coveredWidth, 0, width - coveredWidth, height);
    }
    const nowMs = seconds() * 1000;
    const x = Math.max(0, Math.min(width - 1, (nowMs / durationMs) * width));
    const beyond = nowMs > coversMs + 1;
    // Not yet played: dimmed. Played: full color.
    ctx.fillStyle = "rgba(4,6,10,0.42)";
    ctx.fillRect(x, 0, width - x, height);
    ctx.fillStyle = beyond ? "#ff8a3d" : "#f4fbff";
    ctx.fillRect(Math.round(x) - Math.max(1, ratio), 0, Math.max(2, 2 * ratio), height);
    root.classList.toggle("is-beyond", beyond);
  };

  const syncLabel = () => {
    const now = seconds();
    const beyond = now * 1000 > coversMs + 1;
    label.textContent = `${caption} \u00b7 ${formatClock(now)} / ${formatClock(durationMs / 1000)}${beyond ? " \u00b7 beyond strip" : ""}`;
    root.dataset.seconds = now.toFixed(2);
  };

  const loop = () => {
    redraw();
    if (audio && !audio.paused && !audio.ended) frame = window.requestAnimationFrame(loop);
    else frame = 0;
  };
  const kick = () => {
    syncLabel();
    if (!frame) frame = window.requestAnimationFrame(loop);
  };

  const url = mediaUrl(spec, "spectrogram_png");
  if (!url) {
    root.dataset.state = "none";
    label.textContent = "No spectrogram \u2014 the asset lists no image.";
  } else {
    image.addEventListener("load", () => {
      ready = true;
      root.dataset.state = "ready";
      syncLabel();
      redraw();
    });
    image.addEventListener("error", () => {
      root.dataset.state = "missing";
      label.textContent = "No spectrogram \u2014 the strip image did not load.";
      redraw();
    });
    image.src = url;
    label.textContent = `${caption} \u00b7 loading`;
  }

  if (audio) {
    for (const name of ["play", "playing", "seeked", "timeupdate", "loadedmetadata"]) audio.addEventListener(name, kick);
    for (const name of ["pause", "ended"]) {
      audio.addEventListener(name, () => {
        syncLabel();
        redraw();
      });
    }
  }
  if (typeof ResizeObserver !== "undefined") new ResizeObserver(() => redraw()).observe(canvas);
  return { root, redraw };
}

/**
 * Fill each library tile's strip slot from the catalog and add the bound
 * audio_clip glyph. Tiles keep their legacy id; the clip uid is added beside it.
 */
export interface DecorateOptions {
  /** Status line after a Copy clip uid press (copied, or the clipboard refused). */
  onCopy?: (uid: string, copied: boolean) => void;
}

/**
 * The clip's identity row on the Clip (front) face: its glyph as a passive
 * mark, the uid, and the labelled Copy clip uid button (Optimus ruling 1). The
 * card's own glyph in the corner is the flip control and copies nothing.
 */
function clipIdentity(uid: string, options: DecorateOptions): HTMLElement | null {
  // Second ruling 4: the button says whose uid it copies (the clip's, not the card's).
  const copy = copyUidButton(uid, { label: "Copy clip uid", onCopy: options.onCopy });
  if (!copy) return null;
  const row = document.createElement("div");
  row.className = "clip-identity";
  row.dataset.clipUid = uid;
  const mark = glyphMark(uid, { role: "clip" });
  const code = document.createElement("code");
  code.className = "clip-uid";
  code.textContent = uid;
  if (mark) row.append(mark);
  row.append(code, copy);
  return row;
}

export function decorateLibraryTiles(board: HTMLElement, catalog: LibraryCatalog | null, options: DecorateOptions = {}): void {
  board.querySelectorAll<HTMLElement>(".library-tile").forEach((tile) => {
    const slot = tile.querySelector<HTMLElement>(".spec-strip-slot");
    const id = tile.dataset.id ?? "";
    const clip = catalog?.clipForTile(id) ?? null;
    if (clip) {
      tile.dataset.clipUid = clip.uid;
      const title = tile.querySelector(".face.front > h2");
      if (title && !tile.querySelector(".face.front .clip-identity")) {
        const node = clipIdentity(clip.uid, options);
        if (node) title.after(node);
      }
      const cube = catalog?.derivedFrom(clip.uid, "cube_ihdr");
      if (cube) tile.dataset.cubeUid = cube.uid;
    }
    if (!slot) return;
    if (!catalog) {
      slot.replaceWith(emptyStrip("the library asset catalog did not load."));
      return;
    }
    if (!clip) {
      slot.replaceWith(emptyStrip("no audio clip asset for this tile."));
      return;
    }
    const spec = catalog.derivedFrom(clip.uid, "spectrogram_2d");
    if (!spec) {
      slot.replaceWith(emptyStrip("no spectrogram was computed for this clip."));
      return;
    }
    slot.replaceWith(spectrogramStrip(spec, clip, tile.querySelector("audio")).root);
  });
}
