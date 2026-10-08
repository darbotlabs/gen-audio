"""Check logic of scripts/build-tauri-windows.ps1, through scripts/lib/devenv.ps1.

The build script itself only runs on Windows. The checks it relies on live
in devenv.ps1, which is dot-sourced here in PowerShell (pwsh or Windows
PowerShell; skips without one).
"""

from __future__ import annotations

import hashlib
import json
import os
import re
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
    assert gate < text.index("Invoke-CargoBinBuild -Package") and gate < text.index("npx $($tauriArgs")
    after = text[gate:text.index("Write-Step 'library WAVs match", gate)]
    assert "throw" in after and "skip" not in after.lower()


# --- C2: sidecar freshness is cargo's fingerprint + content, never mtime ---------

def _crate(tmp_path: Path) -> Path:
    crate = tmp_path / "probe"
    (crate / "src").mkdir(parents=True)
    (crate / "Cargo.toml").write_text('[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n\n[[bin]]\nname = "probe"\n'
                                      'path = "src/main.rs"\n\n[workspace]\n', encoding="utf-8")
    (crate / "src" / "main.rs").write_text('fn main() { println!("v1"); }\n', encoding="utf-8")
    return crate


def _build(crate: Path) -> dict:
    env_target = crate / "target"
    out = _ps(f"$env:CARGO_TARGET_DIR = '{env_target}'; Invoke-CargoBinBuild -Package probe -Bin probe | ConvertTo-Json -Compress", crate)
    assert out.returncode == 0, out.stdout + out.stderr
    assert " is stale" not in out.stdout + out.stderr
    return json.loads(out.stdout.strip().splitlines()[-1])


@needs_shell
@pytest.mark.skipif(shutil.which("cargo") is None, reason="needs cargo")
def test_noop_rebuild_is_not_stale_even_when_the_binary_mtime_is_old(tmp_path):
    import os

    crate = _crate(tmp_path)
    first = _build(crate)
    assert first["Fresh"] is False and Path(first["Executable"]).is_file()
    exe = Path(first["Executable"])
    before = hashlib.sha256(exe.read_bytes()).hexdigest()
    os.utime(exe, (978307200, 978307200))  # 2001-01-01: far older than any run start (the SMAX case)
    second = _build(crate)
    assert second == {"Executable": first["Executable"], "Fresh": True}, "a no-op rebuild must not be stale or rebuilt"
    assert hashlib.sha256(exe.read_bytes()).hexdigest() == before
    (crate / "src" / "main.rs").write_text('fn main() { println!("v2"); }\n', encoding="utf-8")
    third = _build(crate)
    assert third["Fresh"] is False and hashlib.sha256(exe.read_bytes()).hexdigest() != before, "a source change rebuilds"


@needs_shell
def test_select_cargo_bin_artifact_reads_cargo_json_and_fails_without_the_bin(tmp_path):
    lines = [
        json.dumps({"reason": "compiler-artifact", "target": {"name": "gen_audio_core", "kind": ["lib"]}, "executable": None, "fresh": True}),
        json.dumps({"reason": "compiler-artifact", "target": {"name": "gen-audio-mcp", "kind": ["bin"]},
                    "executable": "C:/t/release/gen-audio-mcp.exe", "fresh": True}),
        json.dumps({"reason": "build-finished", "success": True}),
    ]
    feed = tmp_path / "lines.json"
    feed.write_text(json.dumps(lines), encoding="utf-8")
    load = f"$l = [string[]](Get-Content -LiteralPath '{feed}' -Raw | ConvertFrom-Json)"
    out = _ps(f"{load}; Select-CargoBinArtifact -Line $l -Bin gen-audio-mcp | ConvertTo-Json -Compress", tmp_path)
    assert out.returncode == 0, out.stderr
    assert json.loads(out.stdout) == {"Executable": "C:/t/release/gen-audio-mcp.exe", "Fresh": True}
    missing = _ps(f"{load}; Select-CargoBinArtifact -Line $l -Bin other", tmp_path)
    assert missing.returncode != 0 and "no compiler-artifact for bin other" in missing.stdout + missing.stderr


def test_build_script_judges_the_sidecar_by_cargo_and_hash_not_mtime():
    text = BUILD.read_text(encoding="utf-8")
    block = text[text.index("Write-Step 'build gen-audio-mcp sidecar'"):text.index("Write-Step 'frontend dependencies")]
    assert "Invoke-CargoBinBuild -Package gen-audio-mcp -Bin gen-audio-mcp" in block
    assert "Assert-Fresh" not in block and "LastWriteTime" not in block
    assert "$stagedHash -ne $sidecarHash" in block


# --- B4: every build run leaves its full log in target/logs -------------------

@needs_shell
@pytest.mark.skipif(shutil.which("git") is None or os.name == "nt", reason="needs git; on Windows this would start a real build")
def test_build_script_saves_its_transcript_even_when_it_throws(tmp_path):
    root = tmp_path / "repo"
    (root / "scripts" / "lib").mkdir(parents=True)
    shutil.copy2(BUILD, root / "scripts" / BUILD.name)
    shutil.copy2(DEVENV, root / "scripts" / "lib" / DEVENV.name)
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
    subprocess.run([*git, "init", "-q"], cwd=root, check=True)
    subprocess.run([*git, "add", "."], cwd=root, check=True)
    subprocess.run([*git, "commit", "-q", "-m", "seed"], cwd=root, check=True)
    sha = subprocess.run(["git", "rev-parse", "--short=12", "HEAD"], cwd=root, check=True, capture_output=True, text=True).stdout.strip()
    # Off Windows the script throws right after it starts; the log must still be written and closed.
    run = subprocess.run([SHELL, "-NoProfile", "-File", str(root / "scripts" / BUILD.name), "-Mode", "NoBundle"], cwd=root,
                         capture_output=True, text=True, timeout=120)
    out = run.stdout + run.stderr
    assert run.returncode != 0 and "run it on Windows" in out, out
    logs = sorted((root / "target" / "logs").glob(f"build-{sha}-*.log"))
    assert len(logs) == 1, (out, list((root / "target").rglob("*")) if (root / "target").exists() else "no target/")
    assert re.fullmatch(rf"build-{sha}-\d{{8}}-\d{{6}}\.log", logs[0].name), logs[0].name
    text = logs[0].read_text(encoding="utf-8-sig")
    assert "build-tauri-windows.ps1 -Mode NoBundle" in text and "run it on Windows" in text
    assert "PowerShell transcript end" in text, "Stop-Transcript runs in finally, even on a throw"
    assert f"BUILD_LOG {logs[0]}" in out or f"BUILD_LOG {logs[0].as_posix()}" in out, out


def test_build_ok_names_the_log_and_the_transcript_stops_in_finally():
    text = BUILD.read_text(encoding="utf-8")
    assert re.search(r'Write-Output "BUILD_OK [^"]*log=\$buildLog', text), "BUILD_OK carries the log path"
    start = text.index("Start-Transcript")
    body = text[start:]
    assert re.search(r"\}\s*finally\s*\{\s*Stop-Transcript", body), "the transcript stops in a finally block"
    assert text.index("Write-Step \"build-tauri-windows.ps1 -Mode") > start, "the whole run is inside the transcript"
