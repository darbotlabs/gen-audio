import { surfaceUiError } from "./play-control";

// Seeking a library WAV, honestly.
//
// Root cause this module fixes (PR #5, SMAX install 2026-10-07): the release
// app serves /library/*.wav through Tauri's embedded-asset protocol
// (tauri-2.12.1 src/protocol/tauri.rs). It answers every request 200 with the
// whole body: no Range handling, no Accept-Ranges. Chromium (WebView2) then
// reports `seekable` as 0-0, and `currentTime = 60` lands at 0 while the
// `seeked` event still fires. The dev server (vite/sirv) answers 206, which
// is why dev never showed it. The same bytes as a blob: URL are fully
// seekable.
//
// So: wait for metadata before seeking (deferred seek), check `seekable`
// covers the target, and if it does not, swap the element onto a blob: URL
// of the same file once, then seek and verify `currentTime` actually landed.
// A seek reports success only when it did.
//
// Blob URLs are cached one per clip key (the library clip id; one id per clip
// uid): a re-rendered transport for the same clip and source reuses it, a new
// source for that clip revokes the old one, and release()/releaseAll() revoke
// on unmount. A failed fetch keeps its reason in the status, and play state
// survives a seek that interrupts another seek's reload.
//
// Transport-agnostic on purpose: it takes a MediaLike and injectable source
// ops, so a unified transport (and later the PR #4 reducer clock) can own it.

/** readyState HAVE_METADATA. */
export const HAVE_METADATA = 1;
/** A paused seek must land within this many seconds of the target. */
export const SEEK_TOLERANCE_S = 0.25;

export interface TimeRangesLike {
  readonly length: number;
  start(index: number): number;
  end(index: number): number;
}

/** The parts of HTMLMediaElement a seek touches. */
export interface MediaLike {
  src: string;
  currentTime: number;
  readonly duration: number;
  readonly readyState: number;
  readonly paused: boolean;
  readonly seekable: TimeRangesLike;
  addEventListener(type: string, listener: () => void, options?: { once?: boolean }): void;
  removeEventListener(type: string, listener: () => void): void;
  load(): void;
  play(): Promise<void>;
  pause(): void;
}

/** How to get a seekable copy of a source. Browser default: fetch -> Blob -> object URL. */
export interface SourceOps {
  toObjectUrl(url: string): Promise<string>;
  /** Free an object URL toObjectUrl made. Browser default: URL.revokeObjectURL. */
  revokeObjectUrl?(url: string): void;
}

export interface SeekOptions {
  /** Max wait for loadedmetadata / seeked. */
  timeoutMs?: number;
  /** Re-check currentTime this long after `seeked` (a non-range source snaps back late). */
  settleMs?: number;
  sleep?: (ms: number) => Promise<void>;
}

export interface SeekResult {
  ok: boolean;
  target: number;
  /** currentTime after the attempt. */
  actual: number;
  /** The seek waited for metadata first. */
  deferred: boolean;
  /** The element was moved onto a blob: URL to make it seekable. */
  reloaded: boolean;
  /** UI/status text. Says "seeked" only when ok. */
  status: string;
}

export function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const whole = Math.floor(seconds);
  return `${Math.floor(whole / 60)}:${(whole % 60).toString().padStart(2, "0")}`;
}

/** True when the element's seekable ranges include `seconds`. */
export function seekableCovers(media: Pick<MediaLike, "seekable">, seconds: number): boolean {
  const ranges = media.seekable;
  for (let i = 0; i < ranges.length; i++) {
    if (ranges.start(i) <= seconds + 1e-6 && seconds <= ranges.end(i) + 1e-6) return true;
  }
  return false;
}

/**
 * Did the seek land? Paused: within SEEK_TOLERANCE_S. Playing: the clock may
 * have run on since, by at most `elapsedS` (plus tolerance), never backwards.
 */
export function landed(actual: number, target: number, playing: boolean, elapsedS = 0): boolean {
  if (!Number.isFinite(actual)) return false;
  const low = target - SEEK_TOLERANCE_S;
  const high = target + SEEK_TOLERANCE_S + (playing ? Math.max(0, elapsedS) : 0);
  return actual >= low && actual <= high;
}

/** Resolve with the first of `events` to fire, or "timeout". */
export function waitForEvent(media: MediaLike, events: string[], timeoutMs: number): Promise<string> {
  return new Promise((resolve) => {
    const handlers = new Map<string, () => void>();
    const done = (name: string) => {
      clearTimeout(timer);
      for (const [type, handler] of handlers) media.removeEventListener(type, handler);
      resolve(name);
    };
    const timer = setTimeout(() => done("timeout"), timeoutMs);
    for (const type of events) {
      const handler = () => done(type);
      handlers.set(type, handler);
      media.addEventListener(type, handler, { once: true });
    }
  });
}

export const browserSourceOps: SourceOps = {
  async toObjectUrl(url: string): Promise<string> {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`fetch ${response.status}`);
    return URL.createObjectURL(await response.blob());
  },
  revokeObjectUrl(url: string): void {
    URL.revokeObjectURL(url);
  },
};

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const defaultSleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * One Seeker per transport. Latest seek wins per element: an older pending
 * seek resolves "superseded" instead of fighting the newer one.
 */
export class Seeker {
  private readonly tokens = new WeakMap<MediaLike, number>();
  private readonly reloads = new WeakMap<MediaLike, Promise<{ ok: boolean; reason?: string }>>();
  /** One blob URL per clip key, with the source it copies. */
  private readonly blobs = new Map<string, { source: string; url: string }>();
  /** Set when a reload paused a playing element; the seek that finishes resumes it. */
  private readonly resumeAfterReload = new WeakMap<MediaLike, true>();
  private readonly timeoutMs: number;
  private readonly settleMs: number;
  private readonly sleep: (ms: number) => Promise<void>;

  constructor(private readonly ops: SourceOps = browserSourceOps, options: SeekOptions = {}) {
    this.timeoutMs = options.timeoutMs ?? 5000;
    this.settleMs = options.settleMs ?? 200;
    this.sleep = options.sleep ?? defaultSleep;
  }

  /** Revoke and forget the blob URL cached for a clip (its transport unmounted). */
  release(key: string): void {
    const cached = this.blobs.get(key);
    if (!cached) return;
    this.blobs.delete(key);
    this.ops.revokeObjectUrl?.(cached.url);
  }

  /** Revoke every cached blob URL (page teardown). */
  releaseAll(): void {
    for (const key of [...this.blobs.keys()]) this.release(key);
  }

  /** Clip keys holding a blob URL right now. */
  cachedKeys(): string[] {
    return [...this.blobs.keys()];
  }

  /**
   * Seek `media` to `seconds`. `key` names the clip for the blob cache
   * (defaults to the element's current source).
   */
  async seek(media: MediaLike, seconds: number, key: string = media.src): Promise<SeekResult> {
    const token = (this.tokens.get(media) ?? 0) + 1;
    this.tokens.set(media, token);
    const stale = () => this.tokens.get(media) !== token;
    const result = (ok: boolean, status: string, extra: Partial<SeekResult> = {}): SeekResult => ({
      ok,
      target: seconds,
      actual: media.currentTime,
      deferred: false,
      reloaded: false,
      status,
      ...extra,
    });

    // Every outcome that is not superseded resumes a play a reload paused,
    // including one paused by an older seek this one interrupted.
    const finish = async (ok: boolean, status: string, extra: Partial<SeekResult> = {}): Promise<SeekResult> => {
      if (this.resumeAfterReload.has(media)) {
        this.resumeAfterReload.delete(media);
        if (media.paused) {
          try {
            await media.play();
          } catch (error) {
            // The result still says where the clock is. The rejection is logged.
            surfaceUiError(error, "seek resume");
          }
        }
      }
      return result(ok, status, { ...extra, actual: media.currentTime });
    };

    if (!media.src) return finish(false, "no wav");
    if (!Number.isFinite(seconds) || seconds < 0) return finish(false, "bad seek");

    let deferred = false;
    if (media.readyState < HAVE_METADATA) {
      deferred = true;
      await waitForEvent(media, ["loadedmetadata", "error"], this.timeoutMs);
      if (stale()) return result(false, "superseded", { deferred });
      if (media.readyState < HAVE_METADATA) return finish(false, "seek failed: the WAV metadata never loaded", { deferred });
    }
    if (Number.isFinite(media.duration) && seconds > media.duration) {
      return finish(false, `past end (${formatClock(media.duration)})`, { deferred });
    }

    let reloaded = false;
    let reloadFailure = "";
    if (!seekableCovers(media, seconds) && !media.src.startsWith("blob:")) {
      const reload = await this.reloadAsBlob(media, key);
      reloaded = reload.ok;
      if (!reload.ok && reload.reason) reloadFailure = `${reload.reason}; `;
      if (stale()) return result(false, "superseded", { deferred, reloaded });
    }
    if (!seekableCovers(media, seconds)) {
      return finish(false, `seek failed: this WAV source cannot seek (${reloadFailure}stays at ${formatClock(media.currentTime)})`, { deferred, reloaded });
    }

    const started = Date.now();
    media.currentTime = seconds;
    await waitForEvent(media, ["seeked", "error"], this.timeoutMs);
    if (this.settleMs > 0) await this.sleep(this.settleMs);
    if (stale()) return result(false, "superseded", { deferred, reloaded });
    const actual = media.currentTime;
    const elapsedS = (Date.now() - started) / 1000;
    if (!landed(actual, seconds, !media.paused, elapsedS)) {
      return finish(false, `seek failed: at ${formatClock(actual)}, wanted ${formatClock(seconds)}`, { deferred, reloaded });
    }
    return finish(true, `seeked to ${formatClock(actual)}`, { deferred, reloaded });
  }

  /**
   * Move the element onto a blob: URL of the same file. One blob per clip
   * key: reuse it for the same source, revoke it when the source changed.
   */
  private reloadAsBlob(media: MediaLike, key: string): Promise<{ ok: boolean; reason?: string }> {
    const pending = this.reloads.get(media);
    if (pending) return pending;
    const run = (async (): Promise<{ ok: boolean; reason?: string }> => {
      const original = media.src;
      try {
        let objectUrl: string;
        const cached = this.blobs.get(key);
        if (cached && cached.source === original) {
          objectUrl = cached.url;
        } else {
          try {
            objectUrl = await this.ops.toObjectUrl(original);
          } catch (error) {
            surfaceUiError(error, "seek reload");
            return { ok: false, reason: `blob reload failed: ${errorText(error)}` };
          }
          const prior = this.blobs.get(key);
          if (prior && prior.source === original) {
            // Another element of this clip cached it meanwhile: keep one.
            this.ops.revokeObjectUrl?.(objectUrl);
            objectUrl = prior.url;
          } else {
            if (prior) this.ops.revokeObjectUrl?.(prior.url);
            this.blobs.set(key, { source: original, url: objectUrl });
          }
        }
        if (media.src !== original) return { ok: false, reason: "blob reload failed: the source changed meanwhile" };
        if (!media.paused) {
          media.pause();
          this.resumeAfterReload.set(media, true);
        }
        media.src = objectUrl;
        media.load();
        await waitForEvent(media, ["loadedmetadata", "error"], this.timeoutMs);
        return media.readyState >= HAVE_METADATA ? { ok: true } : { ok: false, reason: "blob reload failed: the blob never loaded metadata" };
      } finally {
        this.reloads.delete(media);
      }
    })();
    this.reloads.set(media, run);
    return run;
  }
}
