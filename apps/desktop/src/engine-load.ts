export interface EngineLoadState {
  engine: string;
  phase: "loading" | "loaded" | "failed";
  pid?: number;
  error?: string;
  execution?: { backend: string; precision: string; hardware: string };
}

function executionState(result: Record<string, unknown>): Pick<EngineLoadState, "execution"> {
  const value = result.execution as { backend?: unknown; precision?: unknown; hardware?: { name?: unknown } } | undefined;
  if (!value || !["cpu", "cuda", "rocm"].includes(String(value.backend)) ||
      !["float32", "float16", "bfloat16"].includes(String(value.precision))) return {};
  return { execution: { backend: String(value.backend), precision: String(value.precision),
    hardware: typeof value.hardware?.name === "string" ? value.hardware.name.slice(0, 120) : String(value.backend) } };
}

/** Only the current backend load receipt can change the selected engine's state. */
export function createEngineLoader(
  load: (engine: string) => Promise<Record<string, unknown>>,
  update: (state: EngineLoadState) => void,
): { load(engine: string): Promise<void>; wait(): Promise<void>; observe(result: Record<string, unknown>): void; unavailable(): void } {
  let revision = 0;
  let queued = Promise.resolve();
  let current: EngineLoadState | undefined;
  const publish = (state: EngineLoadState): void => { current = state; update(state); };
  return {
    load(engine: string): Promise<void> {
      const requested = ++revision;
      publish({ engine, phase: "loading" });
      const pending = queued.then(async () => {
      if (requested !== revision) return;
      try {
        const result = await load(engine);
        if (requested !== revision) return;
        if (result.engine !== engine || result.loaded !== true || !Number.isInteger(result.pid) || Number(result.pid) < 1) {
          const errors = Array.isArray(result.errors) ? result.errors : [];
          const reason = errors.map(error => error && typeof error === "object" && "message" in error ? String(error.message) : "").filter(Boolean).join("; ");
          throw new Error(reason || "The backend did not confirm loaded model weights.");
        }
        publish({ engine, phase: "loaded", pid: Number(result.pid), ...executionState(result) });
      } catch (error) {
        if (requested === revision) publish({ engine, phase: "failed", error: (error instanceof Error ? error.message : "Engine loading failed.").slice(0, 4000) });
      }
      });
      queued = pending;
      return pending;
    },
    wait(): Promise<void> { return queued; },
    observe(result): void {
      if (!current || current.phase === "loading" || result.loaded === null) return;
      if (result.loaded === true && result.engine === current.engine && Number.isInteger(result.pid) && Number(result.pid) > 0) {
        if (current.phase !== "loaded" || current.pid !== result.pid) publish({ engine: current.engine, phase: "loaded", pid: Number(result.pid), ...executionState(result) });
      } else if (current.phase === "loaded") {
        publish({ engine: current.engine, phase: "failed", error: "The selected resident model is no longer loaded. Reload its engine card." });
      }
    },
    unavailable(): void {
      ++revision;
      if (current) publish({ engine: current.engine, phase: "failed", error: "The backend cannot currently confirm the resident model." });
    },
  };
}
