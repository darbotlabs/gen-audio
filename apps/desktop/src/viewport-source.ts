// Release vs dev deck (PR #5 review, fix 5). Naming follows PR #4
// (`VITE_GEN_AUDIO_FIXTURES`, `fixturesRequested`, `selectViewport`). DOM-free.

/** Fixture cards load only when the dev/test flag is exactly "1". */
export function fixturesRequested(flag: string | undefined): boolean {
  return flag === "1";
}

export function selectViewport<T>(flag: string | undefined, release: T, example: T): T {
  return fixturesRequested(flag) ? example : release;
}

/** Full face ids take precedence over the deprecated front/back snapshot aliases. */
export function snapshotFaces(ui: Record<string, unknown>): Array<{ tileId: string; faceId: string | null; flipped: boolean }> {
  const faces = ui.faces && typeof ui.faces === "object" ? ui.faces as Record<string, unknown> : {};
  const flipped = ui.flipped && typeof ui.flipped === "object" ? ui.flipped as Record<string, unknown> : {};
  return [...new Set([...Object.keys(faces), ...Object.keys(flipped)])].map(view => {
    const face = faces[view] as { face_id?: unknown; tileId?: unknown } | undefined;
    const legacy = flipped[view] as { face?: unknown } | undefined;
    return { tileId: typeof face?.tileId === "string" ? face.tileId : view.replace(/^view:/, ""),
      faceId: typeof face?.face_id === "string" ? face.face_id : null, flipped: legacy?.face === "back" };
  });
}

/** Claims that make an asset dev/test-only; mirrors Rust `asset::DEV_FIXTURE_CLAIMS`. */
export const DEV_FIXTURE_CLAIMS = ["fixture_tone", "reference_only", "sample_content"] as const;

type CardBody = {
  kind?: unknown;
  body?: { probed?: unknown; host?: unknown; sampleScript?: unknown };
};

/** Mirrors Rust `asset::is_dev_fixture`: honesty.fixture, a dev-only claim, or a stand-in card. */
export function isDevFixture(asset: { kind?: unknown; honesty?: { fixture?: unknown; claims?: unknown }; body?: CardBody }): boolean {
  const claims = Array.isArray(asset.honesty?.claims) ? (asset.honesty?.claims as unknown[]) : [];
  return asset.honesty?.fixture === true || claims.some((claim) => (DEV_FIXTURE_CLAIMS as readonly unknown[]).includes(claim)) || isStandInCard(asset);
}

/** Unprobed serve rows, placeholder hosts, and sample scripts. Mirrors Rust `is_stand_in_card`. */
function isStandInCard(asset: { kind?: unknown; body?: CardBody }): boolean {
  if (asset.kind !== "card") return false;
  const kind = asset.body?.kind;
  const inner = asset.body?.body;
  if (kind === "ServeHealth" && inner?.probed !== true) return true;
  if (inner?.sampleScript === true) return true;
  return typeof inner?.host === "string" && inner.host.startsWith("<") && inner.host.endsWith(">") && inner.host.length > 2;
}
