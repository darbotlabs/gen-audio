"""scripts/test.ps1 EOL preflight and -WithWav mode (and the conftest WAV hook), run against a temp repo.

Needs PowerShell (pwsh or Windows PowerShell) and git; skips without them.
"""

from __future__ import annotations

import hashlib
import json
import os
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
    assert "git rm --cached -r . ; git reset --hard" in out and "WARNING" in out
    assert "Eol preflight failed" in out  # later steps are skipped, not run
    assert (repo / "data.json").read_bytes() == b'{\r\n  "a": 1\r\n}\r\n', "the preflight must not rewrite files"


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
