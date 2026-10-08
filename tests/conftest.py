"""Shared pytest hooks.

GEN_AUDIO_REQUIRE_WAVS=1 (set by ``scripts/test.ps1 -WithWav``, the merge
check) turns every skip whose reason names a ``.wav`` into a failure. Without
it, tests that need the gitignored library WAVs skip, as they must in CI.
The byte-for-byte cube regeneration is the only check that sees a hand-edited
cube JSON, and ``build_assets --from-lock`` cannot, so the merge check must
not pass while those tests skip.
"""

from __future__ import annotations

import os

import pytest


def _require_wavs() -> bool:
    return os.environ.get("GEN_AUDIO_REQUIRE_WAVS") == "1"


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    report = outcome.get_result()
    if not (_require_wavs() and report.skipped):
        return
    reason = report.longrepr[2] if isinstance(report.longrepr, tuple) else str(report.longrepr)
    reason = reason.removeprefix("Skipped: ")
    if ".wav" in reason.lower():
        report.outcome = "failed"
        report.longrepr = f"GEN_AUDIO_REQUIRE_WAVS=1 (test.ps1 -WithWav) forbids this skip: {reason}"
