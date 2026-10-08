/**
 * Dev/test-only fixture controls. Imported only from env.installFixtureHooks
 * when the fixtures flag is on, so a release build never ships these strings.
 */
export type FixtureInstallCtx = {
  loadShippedDocument: () => Promise<unknown>;
  refreshConnectors: (doc: unknown) => Promise<unknown>;
  show: (doc: unknown) => void;
  status: HTMLElement;
  setCubeScrub: (fraction: number, opts: { silent: boolean }) => void;
  seekBoundClip: (fraction: number) => Promise<string>;
};

export function installFixtureControls(ctx: FixtureInstallCtx): void {
  const toolbar = document.querySelector(".ga-header-toolbar .toolbar");
  const showExample = document.createElement("button");
  showExample.type = "button";
  showExample.id = "show-example";
  showExample.textContent = "Load labeled example";
  showExample.addEventListener("click", () => {
    void ctx.loadShippedDocument().then(ctx.refreshConnectors).then(ctx.show);
  });
  const runImprove = document.createElement("button");
  runImprove.type = "button";
  runImprove.id = "run-improve";
  runImprove.textContent = "Run Python improve on fixture";
  runImprove.addEventListener("click", async () => {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const result = await invoke<Record<string, unknown>>("run_fixture_improve");
      ctx.status.textContent = result.ok
        ? "Python improve finished on the fixture tone (fixture only — not podcast speech)."
        : `Python improve did not finish: ${JSON.stringify(result)}`;
    } catch (error) {
      ctx.status.textContent = `Python improve needs a debug desktop shell. ${String(error)}`;
    }
  });
  toolbar?.prepend(showExample);
  toolbar?.append(runImprove);

  (window as unknown as { __genAudioScrub?: (f: number) => Promise<string> }).__genAudioScrub = (fraction: number) => {
    ctx.setCubeScrub(fraction, { silent: true });
    return ctx.seekBoundClip(fraction);
  };
}
