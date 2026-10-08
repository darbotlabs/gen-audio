// Seek bug (PR #5): Tauri's embedded-asset protocol has no Range support, so
// WebView2 reports seekable 0-0 and a seek to 60 s lands at 0 while `seeked`
// still fires. FakeMedia models exactly that for http-ish sources and a
// fully seekable element for blob: sources.
import { test } from "node:test";
import assert from "node:assert/strict";
import { HAVE_METADATA, Seeker, landed, seekableCovers, type MediaLike, type SourceOps } from "../src/seek.ts";

class FakeMedia implements MediaLike {
  src: string;
  duration = Number.NaN;
  readyState = 0;
  paused = true;
  loads = 0;
  /** Ignore seeks entirely even on blob: (a source that can never seek). */
  neverSeeks = false;
  /** Fire `seeked` but leave currentTime alone (the honest-status case). */
  lieAboutSeek = false;
  private time = 0;
  private listeners = new Map<string, Set<() => void>>();

  constructor(src: string, private readonly realDuration = 247.067) {
    this.src = src;
  }
  get rangeCapable(): boolean {
    return this.src.startsWith("blob:") && !this.neverSeeks;
  }
  get seekable() {
    const end = this.readyState >= HAVE_METADATA && this.rangeCapable ? this.duration : 0;
    const length = this.readyState >= HAVE_METADATA ? 1 : 0;
    return { length, start: () => 0, end: () => end };
  }
  get currentTime(): number {
    return this.time;
  }
  set currentTime(value: number) {
    // A 200-only source restarts from byte 0: the clock lands at 0.
    this.time = this.rangeCapable && !this.lieAboutSeek ? value : this.lieAboutSeek ? this.time : 0;
    queueMicrotask(() => this.emit("seeked"));
  }
  addEventListener(type: string, listener: () => void): void {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(listener);
  }
  removeEventListener(type: string, listener: () => void): void {
    this.listeners.get(type)?.delete(listener);
  }
  emit(type: string): void {
    for (const listener of [...(this.listeners.get(type) ?? [])]) listener();
  }
  /** Metadata arrives (asynchronously, like a real element). */
  loadMetadata(): void {
    this.readyState = HAVE_METADATA;
    this.duration = this.realDuration;
    this.emit("loadedmetadata");
  }
  load(): void {
    this.loads += 1;
    this.readyState = 0;
    this.time = 0;
    setTimeout(() => this.loadMetadata(), 1);
  }
  async play(): Promise<void> {
    this.paused = false;
  }
  pause(): void {
    this.paused = true;
  }
}

function blobOps(): SourceOps & { calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    async toObjectUrl(url: string) {
      calls.push(url);
      return `blob:gen-audio/${calls.length}`;
    },
  };
}

const fast = { timeoutMs: 200, settleMs: 0 };

test("a 200-only source is moved onto a blob: URL once, then the seek lands and says seeked", async () => {
  const media = new FakeMedia("http://tauri.localhost/library/bitdot_braille_vibevoice.wav");
  media.loadMetadata();
  const ops = blobOps();
  const seeker = new Seeker(ops, fast);
  assert.equal(seekableCovers(media, 60), false, "seekable is 0-0 on the embedded protocol");
  const first = await seeker.seek(media, 60);
  assert.equal(first.ok, true, first.status);
  assert.equal(first.reloaded, true);
  assert.equal(media.currentTime, 60);
  assert.match(first.status, /^seeked to 1:00$/);
  assert.deepEqual(ops.calls, ["http://tauri.localhost/library/bitdot_braille_vibevoice.wav"]);
  const second = await seeker.seek(media, 90);
  assert.equal(second.ok, true);
  assert.equal(second.reloaded, false, "already on the blob: URL");
  assert.equal(ops.calls.length, 1);
});

test("a seek before metadata is deferred until loadedmetadata, then applied", async () => {
  const media = new FakeMedia("blob:gen-audio/ready");
  const seeker = new Seeker(blobOps(), fast);
  const pending = seeker.seek(media, 60);
  assert.equal(media.currentTime, 0, "nothing applied before metadata");
  setTimeout(() => media.loadMetadata(), 5);
  const result = await pending;
  assert.equal(result.deferred, true);
  assert.equal(result.ok, true, result.status);
  assert.equal(media.currentTime, 60);
});

test("metadata that never arrives is a failure, not 'seeked'", async () => {
  const media = new FakeMedia("blob:gen-audio/stuck");
  const result = await new Seeker(blobOps(), fast).seek(media, 60);
  assert.equal(result.ok, false);
  assert.match(result.status, /^seek failed: the WAV metadata never loaded$/);
});

test("a source that still cannot seek after the blob swap reports failure with where it is", async () => {
  const media = new FakeMedia("http://tauri.localhost/library/x.wav");
  media.neverSeeks = true;
  media.loadMetadata();
  const result = await new Seeker(blobOps(), fast).seek(media, 60);
  assert.equal(result.ok, false);
  assert.equal(result.reloaded, true);
  assert.doesNotMatch(result.status, /^seeked/);
  assert.match(result.status, /cannot seek \(stays at 0:00\)/);
});

test("`seeked` fired but currentTime did not move: honest failure", async () => {
  const media = new FakeMedia("blob:gen-audio/liar");
  media.loadMetadata();
  media.lieAboutSeek = true;
  const result = await new Seeker(blobOps(), fast).seek(media, 60);
  assert.equal(result.ok, false);
  assert.equal(result.status, "seek failed: at 0:00, wanted 1:00");
});

test("a failed blob fetch leaves the source alone and reports failure", async () => {
  const media = new FakeMedia("http://tauri.localhost/library/x.wav");
  media.loadMetadata();
  const ops: SourceOps = { toObjectUrl: async () => { throw new Error("fetch 404"); } };
  const result = await new Seeker(ops, fast).seek(media, 60);
  assert.equal(result.ok, false);
  assert.equal(media.src, "http://tauri.localhost/library/x.wav");
  assert.match(result.status, /^seek failed: this WAV source cannot seek/);
  // D: the fetch failure reason reaches the user-visible status.
  assert.equal(result.status, "seek failed: this WAV source cannot seek (blob reload failed: fetch 404; stays at 0:00)");
});

test("playing state survives the blob swap", async () => {
  const media = new FakeMedia("http://tauri.localhost/library/x.wav");
  media.loadMetadata();
  await media.play();
  const result = await new Seeker(blobOps(), fast).seek(media, 60);
  assert.equal(result.ok, true, result.status);
  assert.equal(media.paused, false);
});

test("validation: no wav, bad seconds, past end", async () => {
  const seeker = new Seeker(blobOps(), fast);
  const empty = new FakeMedia("");
  assert.equal((await seeker.seek(empty, 1)).status, "no wav");
  const media = new FakeMedia("blob:gen-audio/v");
  media.loadMetadata();
  assert.equal((await seeker.seek(media, -1)).status, "bad seek");
  assert.equal((await seeker.seek(media, Number.NaN)).status, "bad seek");
  assert.equal((await seeker.seek(media, 300)).status, "past end (4:07)");
});

test("latest seek wins; the older one resolves superseded", async () => {
  const media = new FakeMedia("blob:gen-audio/drag");
  const seeker = new Seeker(blobOps(), fast);
  const older = seeker.seek(media, 30);
  const newer = seeker.seek(media, 60);
  media.loadMetadata();
  assert.equal((await older).status, "superseded");
  assert.equal((await newer).ok, true);
  assert.equal(media.currentTime, 60);
});

test("landed(): tolerance paused, forward drift only while playing", () => {
  assert.equal(landed(60.2, 60, false), true);
  assert.equal(landed(60.4, 60, false), false);
  assert.equal(landed(0, 60, false), false);
  assert.equal(landed(61.1, 60, true, 1), true);
  assert.equal(landed(59.5, 60, true, 1), false);
  assert.equal(landed(Number.NaN, 60, false), false);
});

/** Source ops that count object URLs made and revoked (D: blob leak). */
function countingOps(): SourceOps & { created: string[]; revoked: string[]; live(): string[] } {
  const created: string[] = [];
  const revoked: string[] = [];
  return {
    created,
    revoked,
    live: () => created.filter((url) => !revoked.includes(url)),
    async toObjectUrl(url: string) {
      const objectUrl = `blob:gen-audio/${created.length + 1}?of=${url}`;
      created.push(objectUrl);
      return objectUrl;
    },
    revokeObjectUrl(url: string) {
      revoked.push(url);
    },
  };
}

test("D: one blob URL per clip across re-renders; replace and unmount revoke; create/revoke balance", async () => {
  const ops = countingOps();
  const seeker = new Seeker(ops, fast);
  const wav = "http://tauri.localhost/library/bitdot_braille_vibevoice.wav";
  // Five re-renders of the same tile: each is a fresh <audio> on the asset URL.
  for (let render = 0; render < 5; render++) {
    const media = new FakeMedia(wav);
    media.loadMetadata();
    const result = await seeker.seek(media, 30 + render, "lib-bitdot");
    assert.equal(result.ok, true, result.status);
    assert.equal(ops.live().length, 1, `render ${render}: no duplicate blob for the clip`);
  }
  assert.equal(ops.created.length, 1, "re-renders reuse the cached blob URL");
  // A second clip gets its own single blob.
  const other = new FakeMedia("http://tauri.localhost/library/kokoro_onnx.wav");
  other.loadMetadata();
  await seeker.seek(other, 10, "lib-kokoro");
  assert.deepEqual(seeker.cachedKeys().sort(), ["lib-bitdot", "lib-kokoro"]);
  // Replace: the clip's source changed, so its old blob is revoked.
  const replaced = new FakeMedia("http://tauri.localhost/library/bitdot_braille_vibevoice.v2.wav");
  replaced.loadMetadata();
  await seeker.seek(replaced, 12, "lib-bitdot");
  assert.equal(ops.revoked.length, 1);
  assert.match(ops.revoked[0], /of=http:\/\/tauri\.localhost\/library\/bitdot_braille_vibevoice\.wav$/);
  assert.equal(ops.live().length, 2, "one live blob per clip");
  // Unmount: release() revokes; releaseAll() clears the rest.
  seeker.release("lib-bitdot");
  seeker.releaseAll();
  assert.equal(ops.created.length, ops.revoked.length, "every createObjectURL has a revokeObjectURL");
  assert.equal(new Set(ops.revoked).size, ops.revoked.length, "nothing revoked twice");
  assert.deepEqual(seeker.cachedKeys(), []);
});

test("D: two elements of one clip reloading at once still keep one blob", async () => {
  const ops = countingOps();
  const seeker = new Seeker(ops, fast);
  const wav = "http://tauri.localhost/library/x.wav";
  const a = new FakeMedia(wav);
  const b = new FakeMedia(wav);
  a.loadMetadata();
  b.loadMetadata();
  const [ra, rb] = await Promise.all([seeker.seek(a, 20, "lib-x"), seeker.seek(b, 40, "lib-x")]);
  assert.equal(ra.ok && rb.ok, true);
  assert.equal(ops.live().length, 1);
  assert.equal(a.src, b.src);
  seeker.releaseAll();
  assert.equal(ops.live().length, 0);
});

test("D: play state carries across a seek that interrupts another seek's reload", async () => {
  const media = new FakeMedia("http://tauri.localhost/library/x.wav");
  media.loadMetadata();
  await media.play();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  const ops: SourceOps = {
    async toObjectUrl() {
      await gate;
      return "blob:gen-audio/slow";
    },
    revokeObjectUrl() {},
  };
  const seeker = new Seeker(ops, fast);
  const first = seeker.seek(media, 60, "lib-x");
  await new Promise((resolve) => setTimeout(resolve, 0));
  release();
  // Wait until the reload has paused the element and swapped the source, then interrupt.
  while (!media.src.startsWith("blob:")) await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(media.paused, true, "the reload paused it");
  const second = seeker.seek(media, 90, "lib-x");
  assert.equal((await first).status, "superseded");
  const landedResult = await second;
  assert.equal(landedResult.ok, true, landedResult.status);
  assert.equal(media.currentTime, 90);
  assert.equal(media.paused, false, "still playing after the interrupted reload");
});
