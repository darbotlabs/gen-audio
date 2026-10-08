/**
 * Single seam for Vite env reads. main.ts imports every import.meta.env value
 * from here (never reads import.meta.env itself). Vite replaces import.meta.env
 * inside this module at build time.
 *
 * installFixtureHooks keeps a literal `import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1"`
 * check so the fixture-controls chunk is DCE'd out of release dist. bindEnv is the
 * test seam (option (c)): under tsx, import.meta.env is undefined, so tests call
 * bindEnv({ VITE_GEN_AUDIO_FIXTURES: "1" }) before importing main. Production never
 * references bindEnv, so it is tree-shaken out of dist and the test-only branch
 * folds away.
 *
 * Option (a) — a temp-file Node loader that stubs import.meta.env — is an
 * antipattern: it satisfied the sprawl checker, not the rule (same shape as
 * AP-OPT-2 wrapper sprawl).
 */
export type GenAudioEnv = {
  VITE_GEN_AUDIO_FIXTURES?: string;
};

const meta = (import.meta as ImportMeta & { env?: GenAudioEnv }).env;

/** Vite-replaced flag (used by fixturesFlag when no bindEnv override). */
export const VITE_GEN_AUDIO_FIXTURES: string | undefined = meta?.VITE_GEN_AUDIO_FIXTURES;

/**
 * N-M2: compile-time fixture switches. Vite rewrites `import.meta.env` to an
 * object literal at build time, so both are literal-valued constants Rollup
 * can fold: in a release build FIXTURES_BUILD is `false` and UNDER_VITE is
 * `true`. A guarded dynamic import must test these constants at its call
 * site (`FIXTURES_BUILD || (!UNDER_VITE && ...)`) so the whole branch, and the
 * lazy chunk behind it, is dropped. A function call (fixturesFlag()) or a
 * re-exported `let` is opaque to Rollup and keeps the chunk. So is
 * `typeof import.meta.env` on its own (Rollup does not fold typeof of the
 * replaced object), and `import.meta.env?.VITE_GEN_AUDIO_FIXTURES` when the
 * variable is unset (Vite only substitutes keys it knows). What folds:
 * `import.meta.env?.MODE` (always substituted with a string literal) and
 * `<anything> && import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1"` (the
 * right-hand side reads a missing key of the replaced literal: false). Under tsx `import.meta.env` is undefined: UNDER_VITE is false and
 * the bindEnv seam decides.
 */
export const UNDER_VITE: boolean = import.meta.env?.MODE !== undefined;
export const FIXTURES_BUILD: boolean =
  typeof import.meta.env !== "undefined" && import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1";

let bound: GenAudioEnv | null = null;

/** Test seam: replace the env binding before main.ts evaluates. */
export function bindEnv(next: GenAudioEnv): void {
  bound = { ...next };
}

export function fixturesFlag(): string | undefined {
  return bound ? bound.VITE_GEN_AUDIO_FIXTURES : VITE_GEN_AUDIO_FIXTURES;
}

export type FixtureHooksCtx = {
  loadShippedDocument: () => Promise<unknown>;
  refreshConnectors: (doc: unknown) => Promise<unknown>;
  show: (doc: unknown) => void;
  status: HTMLElement;
  setCubeScrub: (fraction: number, opts: { silent: boolean }) => void;
  seekBoundClip: (fraction: number) => Promise<string>;
};

async function loadControls(ctx: FixtureHooksCtx): Promise<void> {
  const { installFixtureControls } = await import("./fixture-controls");
  installFixtureControls(ctx);
}

/**
 * Installs fixture-only UI when the flag is on. The literal import.meta.env
 * comparison is what Vite DCE's in release. The bindEnv branch exists for tests
 * under tsx; with bindEnv tree-shaken, `bound` stays null and that branch dies.
 */
export function installFixtureHooks(ctx: FixtureHooksCtx): Promise<void> {
  // Guard for tsx (import.meta.env is undefined). Vite still replaces the
  // property access so the === "1" branch DCE's out of release dist.
  const viteOn =
    typeof import.meta.env !== "undefined" && import.meta.env.VITE_GEN_AUDIO_FIXTURES === "1";
  if (viteOn) return loadControls(ctx);
  if (bound !== null && bound.VITE_GEN_AUDIO_FIXTURES === "1") return loadControls(ctx);
  return Promise.resolve();
}
