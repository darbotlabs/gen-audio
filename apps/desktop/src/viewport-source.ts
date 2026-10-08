// Release vs dev deck (PR #5 review, fix 5). Naming follows PR #4
// (`VITE_GEN_AUDIO_FIXTURES`, `fixturesRequested`, `selectViewport`). DOM-free.

/** Fixture cards load only when the dev/test flag is exactly "1". */
export function fixturesRequested(flag: string | undefined): boolean {
  return flag === "1";
}

export function selectViewport<T>(flag: string | undefined, release: T, example: T): T {
  return fixturesRequested(flag) ? example : release;
}

/** Claims that make an asset dev/test-only; mirrors Rust `asset::DEV_FIXTURE_CLAIMS`. */
export const DEV_FIXTURE_CLAIMS = ["fixture_tone", "reference_only", "sample_content"] as const;

/** Mirrors Rust `asset::is_dev_fixture`: honesty.fixture, or a dev-only claim. */
export function isDevFixture(asset: { honesty?: { fixture?: unknown; claims?: unknown } }): boolean {
  const claims = Array.isArray(asset.honesty?.claims) ? (asset.honesty?.claims as unknown[]) : [];
  return asset.honesty?.fixture === true || claims.some((claim) => (DEV_FIXTURE_CLAIMS as readonly unknown[]).includes(claim));
}
