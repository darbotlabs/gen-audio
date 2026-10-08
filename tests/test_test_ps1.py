"""scripts/test.ps1 EOL preflight and -WithWav mode (and the conftest WAV hook), run against a temp repo.

Needs PowerShell (pwsh or Windows PowerShell) and git; skips without them.
"""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
SHELL = shutil.which("pwsh") or shutil.which("powershell")
OTHER_STEPS = "Pssa,Sprawl,Regen,Cargo,Npm,Python"


def _git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "core.autocrlf=false", *args],
                   cwd=cwd, check=True, capture_output=True)


def _temp_repo(tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    (repo / "scripts" / "lib").mkdir(parents=True)
    shutil.copy2(REPO / "scripts" / "test.ps1", repo / "scripts" / "test.ps1")
    shutil.copy2(REPO / "scripts" / "lib" / "devenv.ps1", repo / "scripts" / "lib" / "devenv.ps1")
    (repo / ".gitattributes").write_bytes(b"*.json text eol=lf\n*.ps1 -text\n")
    (repo / "data.json").write_bytes(b'{\n  "a": 1\n}\n')
    _git(repo, "init", "-q")
    _git(repo, "add", ".")
    _git(repo, "commit", "-q", "-m", "seed")
    return repo


def _run(repo: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([SHELL, "-NoProfile", "-File", str(repo / "scripts" / "test.ps1"), "-Tag", "t", "-Skip", OTHER_STEPS, *extra],
                          cwd=repo, capture_output=True, text=True, timeout=300)


@pytest.mark.skipif(SHELL is None or shutil.which("git") is None, reason="needs PowerShell and git")
def test_preflight_fails_and_names_a_crlf_file_under_an_eol_lf_rule(tmp_path):
    repo = _temp_repo(tmp_path)
    clean = _run(repo)
    assert clean.returncode == 0, clean.stdout + clean.stderr
    # What a core.autocrlf=true checkout leaves behind: LF in the index, CRLF on disk.
    (repo / "data.json").write_bytes(b'{\r\n  "a": 1\r\n}\r\n')
    dirty = _run(repo)
    out = dirty.stdout + dirty.stderr
    assert dirty.returncode != 0, out
    assert "EOL_CRLF data.json (index lf)" in out
    # Per-file fix only: rewrite just the listed files from HEAD. Never a tree-wide reset.
    fix = [line.strip() for line in dirty.stdout.splitlines() if line.startswith("    git ")]
    assert fix == ["git rm -q --cached -- data.json", "git restore --source=HEAD --staged --worktree -- data.json"], dirty.stdout
    assert "reset --hard" not in out and "git rm --cached -r ." not in out
    assert "Eol preflight failed" in out  # later steps are skipped, not run
    assert (repo / "data.json").read_bytes() == b'{\r\n  "a": 1\r\n}\r\n', "the preflight must not rewrite files"
    # The printed fix works, even on an index whose stat already matches the CRLF file: run it and the preflight passes.
    subprocess.run(["git", "update-index", "-q", "--refresh"], cwd=repo, capture_output=True)
    for line in fix:
        _git(repo, *shlex.split(line)[1:])
    assert (repo / "data.json").read_bytes() == b'{\n  "a": 1\n}\n'
    assert _run(repo).returncode == 0


@pytest.mark.skipif(SHELL is None or shutil.which("git") is None, reason="needs PowerShell and git")
def test_preflight_fix_for_a_crlf_index_is_renormalize_and_commit_per_file(tmp_path):
    repo = _temp_repo(tmp_path)
    (repo / "data.json").write_bytes(b'{\r\n  "a": 2\r\n}\r\n')
    (repo / ".gitattributes").write_bytes(b"*.ps1 -text\n")
    _git(repo, "add", ".")
    _git(repo, "commit", "-q", "-m", "crlf in the index")
    (repo / ".gitattributes").write_bytes(b"*.json text eol=lf\n*.ps1 -text\n")
    _git(repo, "add", ".gitattributes")
    _git(repo, "commit", "-q", "-m", "eol=lf rule")
    out = _run(repo)
    text = out.stdout + out.stderr
    assert out.returncode != 0 and "EOL_CRLF data.json (index crlf)" in text, text
    fix = [line.strip() for line in out.stdout.splitlines() if line.startswith("    git ")]
    # HEAD holds the CRLF bytes here, so restoring from it comes only after the renormalize commit.
    assert fix == ["git add --renormalize -- data.json", "git commit -m 'Renormalize line endings' -- data.json",
                   "git rm -q --cached -- data.json", "git restore --source=HEAD --staged --worktree -- data.json"], out.stdout
    assert "reset --hard" not in text
    for line in fix:
        _git(repo, *shlex.split(line)[1:])
    assert (repo / "data.json").read_bytes() == b'{\n  "a": 2\n}\n'
    assert _run(repo).returncode == 0


@pytest.mark.skipif(SHELL is None or shutil.which("git") is None, reason="needs PowerShell and git")
def test_with_wav_mode_fails_on_a_missing_or_unlocked_wav_and_passes_on_the_locked_bytes(tmp_path):
    repo = _temp_repo(tmp_path)
    library = repo / "apps" / "desktop" / "public" / "library"
    library.mkdir(parents=True)
    lock = repo / "schemas" / "asset-object" / "media.lock.json"
    lock.parent.mkdir(parents=True)
    wav = b"RIFF fake wav bytes"
    lock.write_text(json.dumps({"media": {"a.wav": {"sha256": hashlib.sha256(wav).hexdigest()}}}), encoding="utf-8")
    missing = _run(repo, "-WithWav")
    assert missing.returncode != 0 and "WAV_MISSING a.wav" in missing.stdout + missing.stderr
    (library / "a.wav").write_bytes(wav + b"x")
    edited = _run(repo, "-WithWav")
    assert edited.returncode != 0 and "WAV_MISMATCH a.wav" in edited.stdout + edited.stderr
    (library / "a.wav").write_bytes(wav)
    ok = _run(repo, "-WithWav")
    assert ok.returncode == 0, ok.stdout + ok.stderr
    assert "1/1 staged WAVs match media.lock.json sha256" in ok.stdout
    both = _run(repo, "-WithWav", "-FromLock")
    assert both.returncode != 0 and "exclude each other" in both.stdout + both.stderr


def test_require_wavs_turns_a_wav_skip_into_a_failure(tmp_path):
    shutil.copy2(REPO / "tests" / "conftest.py", tmp_path / "conftest.py")
    (tmp_path / "test_probe.py").write_text(
        "import pytest\n"
        "def test_needs_wav():\n    pytest.skip('x.wav is gitignored and not staged here')\n"
        "def test_other_skip():\n    pytest.skip('needs PowerShell')\n",
        encoding="utf-8",
    )
    def run(require: str | None) -> subprocess.CompletedProcess[str]:
        env = {k: v for k, v in os.environ.items() if k != "GEN_AUDIO_REQUIRE_WAVS"}
        if require is not None:
            env["GEN_AUDIO_REQUIRE_WAVS"] = require
        return subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", "-o", "addopts=", str(tmp_path)],
                              cwd=tmp_path, env=env, capture_output=True, text=True, timeout=120)
    plain = run(None)
    assert plain.returncode == 0 and "2 skipped" in plain.stdout, plain.stdout
    strict = run("1")
    assert strict.returncode != 0 and "1 failed" in strict.stdout and "1 skipped" in strict.stdout, strict.stdout
    assert "forbids this skip: x.wav" in strict.stdout


@pytest.mark.skipif(SHELL is None or shutil.which("git") is None, reason="needs PowerShell and git")
def test_sprawl_gate_fails_on_a_stray_shell_script_or_any_shebang_file(tmp_path):
    """B3: the gate scans every executable script type, not only the PowerShell/Python ones it knew."""
    repo = _temp_repo(tmp_path)

    def sprawl() -> subprocess.CompletedProcess[str]:
        return subprocess.run([SHELL, "-NoProfile", "-File", str(repo / "scripts" / "test.ps1"), "-Tag", "t",
                               "-Skip", "Pssa,Regen,Cargo,Npm,Python"], cwd=repo, capture_output=True, text=True, timeout=300)

    # A Rust inner attribute starts with "#!" too; it is not a shebang.
    (repo / "main.rs").write_text("#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]\nfn main() {}\n", encoding="utf-8")
    clean = sprawl()
    assert clean.returncode == 0, clean.stdout + clean.stderr
    assert "Sprawl PASS" in clean.stdout

    # Split so this test file is not itself a tauri CLI caller to the gate it tests.
    tauri_cli = "@tauri-apps/" + "cli@2.12.1"
    (repo / "scripts" / "build-linux.sh").write_text(f"#!/usr/bin/env bash\nnpx --yes {tauri_cli} build --bundles deb\n", encoding="utf-8")
    (repo / "tools").mkdir()
    (repo / "tools" / "deploy").write_text("#!/bin/sh\necho hi\n", encoding="utf-8")
    (repo / "helper.mjs").write_text("console.log(1)\n", encoding="utf-8")
    stray = sprawl()
    out = stray.stdout + stray.stderr
    assert stray.returncode != 0, out
    assert "SPRAWL outside-allowlist scripts/build-linux.sh" in out
    assert "SPRAWL tauri-build scripts/build-linux.sh" in out
    assert "SPRAWL outside-allowlist tools/deploy" in out
    assert "SPRAWL outside-allowlist helper.mjs" in out
    assert "main.rs" not in out
