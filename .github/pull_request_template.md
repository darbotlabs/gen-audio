## What and why

<!-- What changed, why, and what you deliberately left out. -->

## Evidence

<!-- Paste the TEST_SUMMARY lines and the step table, plus anything the reviewer must reproduce. -->

## Required checks

- [ ] **with-WAV test.ps1 run (required for merge).** I staged the library WAVs in `apps/desktop/public/library` and ran `pwsh -NoProfile -File scripts/test.ps1 -Tag <pr> -WithWav` on this head SHA. The Wavs step matched every sha256 in `schemas/asset-object/media.lock.json`, and the result was `TEST_SUMMARY PASS` with no WAV skips. Machine, head SHA and log dir: <!-- e.g. SMAX, abc1234, artifacts/test-logs/... -->
  CI runs `-FromLock`. That ties each cube JSON to its WAV's sha256 but cannot see a hand-edited cube. Only the with-WAV byte-for-byte regeneration (`tests/test_cube_layers.py`) can, so the reviewer re-runs it before approving.
- [ ] CI green (`ci.yml`: python, rust regen gate `--from-lock`, desktop, windows scripts).
- [ ] No new one-off or helper scripts. Canonical entry points only (`scripts/test.ps1`, `cube_revision.py`, `build_assets`).
