"""Check logic of scripts/build-tauri-windows.ps1, through scripts/lib/devenv.ps1.

The build script itself only runs on Windows. The checks it relies on live
in devenv.ps1, which is dot-sourced here in PowerShell (pwsh or Windows
PowerShell; skips without one).
"""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
DEVENV = REPO / "scripts" / "lib" / "devenv.ps1"
BUILD = REPO / "scripts" / "build-tauri-windows.ps1"
SHELL = shutil.which("pwsh") or shutil.which("powershell")
needs_shell = pytest.mark.skipif(SHELL is None, reason="needs PowerShell")


def _ps(command: str, cwd: Path) -> subprocess.CompletedProcess[str]:
    script = f". '{DEVENV}'; $ErrorActionPreference = 'Stop'; {command}"
    return subprocess.run([SHELL, "-NoProfile", "-NonInteractive", "-Command", script], cwd=cwd, capture_output=True, text=True,
                          timeout=600)


def _library(tmp_path: Path, files: dict[str, bytes], lock: dict[str, bytes]) -> Path:
    root = tmp_path / "root"
    library = root / "apps" / "desktop" / "public" / "library"
    library.mkdir(parents=True)
    (root / "schemas" / "asset-object").mkdir(parents=True)
    for name, data in files.items():
        (library / name).write_bytes(data)
    media = {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in lock.items()}
    (root / "schemas" / "asset-object" / "media.lock.json").write_text(json.dumps({"media": media}), encoding="utf-8")
    return root


# --- C1: no release build without the locked WAVs ----------------------------

@needs_shell
def test_wav_check_names_missing_short_and_edited_wavs_and_is_silent_when_all_match(tmp_path):
    good, other = b"RIFF good", b"RIFF other"
    root = _library(tmp_path, {"a.wav": good, "c.wav": b"RIFF goox", "d.wav": good + b"x"},
                    {"a.wav": good, "b.wav": other, "c.wav": good, "d.wav": good})
    out = _ps(f"Get-LibraryWavProblem -Root '{root}'", root)
    assert out.returncode == 0, out.stderr
    lines = out.stdout.strip().splitlines()
    assert any(line.startswith("WAV_MISSING b.wav") for line in lines), lines
    assert any(line.startswith("WAV_MISMATCH c.wav") for line in lines), lines  # same length, other bytes
    assert any(line.startswith("WAV_BYTES d.wav") for line in lines), lines
    assert not any(" a.wav" in line for line in lines), lines
    ok = _library(tmp_path / "ok", {"a.wav": good}, {"a.wav": good})
    clean = _ps(f"Get-LibraryWavProblem -Root '{ok}'", ok)
    assert clean.returncode == 0 and clean.stdout.strip() == "", clean.stdout + clean.stderr


@needs_shell
def test_wav_check_fails_on_an_empty_or_missing_lock_instead_of_passing(tmp_path):
    empty = _library(tmp_path, {}, {})
    assert _ps(f"Get-LibraryWavProblem -Root '{empty}'", empty).returncode != 0
    nolock = tmp_path / "nolock"
    nolock.mkdir()
    assert _ps(f"Get-LibraryWavProblem -Root '{nolock}'", nolock).returncode != 0


def test_build_script_checks_the_wavs_before_building_and_never_skips():
    text = BUILD.read_text(encoding="utf-8")
    gate = text.index("Get-LibraryWavProblem -Root $root")
    assert gate < text.index("cargo build") and gate < text.index("npx $($tauriArgs")
    after = text[gate:text.index("Write-Step 'library WAVs match", gate)]
    assert "throw" in after and "skip" not in after.lower()
