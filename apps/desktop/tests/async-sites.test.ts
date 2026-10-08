// One table drives every per-site log. Deleting that site's surfaceUiError
// leaves the warning empty, so the row goes red. Nine hand-written tests
// would pin the same labels and drift.
import { test } from "node:test";
import assert from "node:assert/strict";
import { HAVE_METADATA, Seeker, type MediaLike, type SeekResult, type SourceOps } from "../src/seek.ts";
import { copyUid } from "../src/glyph.ts";
import { harvestNames } from "../src/library-meta.ts";
import { loadCube } from "../src/cubeview.ts";
import { makeFixture, play } from "../src/signal.ts";
import { loadLibraryCatalog, resetLibraryCatalogForTest } from "../src/library-assets.ts";

const globals = globalThis as unknown as Record<string, unknown>;
if (!globals.document) {
  globals.document = { querySelector: () => null, querySelectorAll: () => [] };
}

class SiteMedia implements MediaLike {
  duration = Number.NaN;
  readyState = 0;
  paused = true;
  private time = 0;
  private listeners = new Map<string, Set<() => void>>();

  constructor(
    public src: string,
    private readonly onPlay: () => Promise<void> = async () => {
      this.paused = false;
    },
  ) {}

  get seekable() {
    const end = this.readyState >= HAVE_METADATA && this.src.startsWith("blob:") ? this.duration : 0;
    const length = this.readyState >= HAVE_METADATA ? 1 : 0;
    return { length, start: () => 0, end: () => end };
  }

  get currentTime(): number {
    return this.time;
  }

  set currentTime(value: number) {
    this.time = this.src.startsWith("blob:") ? value : 0;
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

  loadMetadata(): void {
    this.readyState = HAVE_METADATA;
    this.duration = 247;
    this.emit("loadedmetadata");
  }

  load(): void {
    this.readyState = 0;
    this.time = 0;
    setTimeout(() => this.loadMetadata(), 1);
  }

  play(): Promise<void> {
    return this.onPlay();
  }

  pause(): void {
    this.paused = true;
  }
}

const fast = { timeoutMs: 200, settleMs: 0 };

function withFetch(fetchImpl: typeof fetch, run: () => Promise<void>): Promise<void> {
  const original = globalThis.fetch;
  globalThis.fetch = fetchImpl;
  return run().finally(() => {
    globalThis.fetch = original;
  });
}

const sites: Array<[string, () => Promise<void>]> = [
  [
    "play",
    async () => {
      const playback = await import("../src/playback.ts");
      const audio = {
        src: "/library/x.wav",
        paused: true,
        ended: false,
        currentTime: 0,
        duration: 10,
        async play() {
          throw new TypeError("offline");
        },
        pause() {
          audio.paused = true;
        },
      };
      playback.registerPlayer("async-site-play", audio as unknown as HTMLAudioElement);
      await playback.playClip("async-site-play");
    },
  ],
  [
    "seek",
    async () => {
      const playback = await import("../src/playback.ts");
      class Boom extends Seeker {
        override async seek(): Promise<SeekResult> {
          throw new TypeError("offline");
        }
      }
      playback.setSeeker(new Boom());
      try {
        const audio = {
          src: "/library/x.wav",
          paused: true,
          ended: false,
          currentTime: 0,
          duration: 10,
          async play() {},
          pause() {},
        };
        playback.registerPlayer("async-site-seek", audio as unknown as HTMLAudioElement);
        await playback.seekClip("async-site-seek", 1);
      } finally {
        playback.setSeeker(new Seeker());
      }
    },
  ],
  [
    "seek resume",
    async () => {
      const media = new SiteMedia("http://tauri.localhost/library/x.wav", async () => {
        throw new TypeError("offline");
      });
      media.loadMetadata();
      media.paused = false;
      const ops: SourceOps = { async toObjectUrl() { return "blob:gen-audio/resume"; } };
      await new Seeker(ops, fast).seek(media, 60);
    },
  ],
  [
    "seek reload",
    async () => {
      const media = new SiteMedia("http://tauri.localhost/library/x.wav");
      media.loadMetadata();
      const ops: SourceOps = {
        async toObjectUrl() {
          throw new TypeError("offline");
        },
      };
      await new Seeker(ops, fast).seek(media, 60);
    },
  ],
  [
    "copy uid",
    async () => {
      const previous = navigator.clipboard;
      navigator.clipboard = {
        async writeText() {
          throw new TypeError("offline");
        },
      } as unknown as Clipboard;
      try {
        assert.equal(await copyUid("ga:audio_clip:abc"), false);
      } finally {
        if (previous === undefined) delete (navigator as { clipboard?: Clipboard }).clipboard;
        else navigator.clipboard = previous;
      }
    },
  ],
  [
    "sidecar harvest",
    () =>
      withFetch(async () => {
        throw new TypeError("offline");
      }, async () => {
        const names = await harvestNames("/library/clip.wav", "/library/clip.synth.json");
        assert.equal(names.source, "filename-only");
      }),
  ],
  [
    "cube",
    () =>
      withFetch(async () => {
        throw new TypeError("offline");
      }, async () => {
        const status = await loadCube("/library/x.json");
        assert.match(status, /did not load/);
      }),
  ],
  [
    "tone playback",
    async () => {
      let ended: (() => void) | null = null;
      class FakeContext {
        destination = {};
        createBuffer(): { copyToChannel: () => void } {
          return { copyToChannel() {} };
        }
        createBufferSource(): { buffer: null; connect: () => void; start: () => void } {
          const source = {
            buffer: null,
            connect() {},
            start() {},
          };
          Object.defineProperty(source, "onended", {
            configurable: true,
            set(fn: () => void) {
              ended = fn;
            },
          });
          return source;
        }
        close(): Promise<void> {
          return Promise.reject(new TypeError("offline"));
        }
      }
      const previous = globalThis.AudioContext;
      globalThis.AudioContext = FakeContext as unknown as typeof AudioContext;
      try {
        play(makeFixture());
        ended?.();
        await new Promise((resolve) => setTimeout(resolve, 0));
      } finally {
        globalThis.AudioContext = previous;
      }
    },
  ],
  [
    "dev assets",
    () =>
      withFetch(
        async () =>
          new Response(JSON.stringify({ assets: [] }), {
            status: 200,
            headers: { "content-type": "application/json" },
          }),
        async () => {
          resetLibraryCatalogForTest();
          try {
            await loadLibraryCatalog(Promise.reject(new TypeError("offline")));
          } finally {
            resetLibraryCatalogForTest();
          }
        },
      ),
  ],
];

test("each per-site rejection is logged under its exact label", async () => {
  assert.equal(sites.length, 9);
  for (const [label, drive] of sites) {
    const warnings: string[] = [];
    const original = console.warn;
    console.warn = (message?: unknown) => {
      warnings.push(String(message));
    };
    try {
      await drive();
      assert.deepEqual(warnings, [`gen-audio: ${label}: TypeError: offline`], label);
    } finally {
      console.warn = original;
    }
  }
});
