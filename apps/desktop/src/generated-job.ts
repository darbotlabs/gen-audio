import { isUid } from "./asset";

export function generatedJob(args: Record<string, unknown>): { kind: "clip" | "spectrogram" | "cube" | "video"; clipUid: string } | null {
  if (args.phase !== "done") return null;
  const target = args.target;
  if (!isUid(target)) throw new Error("Completed generation did not identify a valid asset.");
  if (target.startsWith("ga:audio_clip:")) return { kind: args.kind === "video" ? "video" : "clip", clipUid: target };
  const kind = target.startsWith("ga:spectrogram_2d:") ? "spectrogram" : target.startsWith("ga:cube_ihdr:") ? "cube" : null;
  if (!kind || !isUid(args.clipUid) || !args.clipUid.startsWith("ga:audio_clip:")) {
    throw new Error("Completed derived generation did not identify its source clip.");
  }
  return { kind, clipUid: args.clipUid };
}
