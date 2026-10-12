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
import sys
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
DEVENV = REPO / "scripts" / "lib" / "devenv.ps1"
BUILD = REPO / "scripts" / "build-tauri-windows.ps1"
SHELL = shutil.which("pwsh") or shutil.which("powershell")
needs_shell = pytest.mark.skipif(SHELL is None, reason="needs PowerShell")


@needs_shell
def test_installed_runtime_payload_prepares_prompts_without_development_checkout(tmp_path):
    root = tmp_path / "checkout"
    root.mkdir()
    shutil.copy2(REPO / "pyproject.toml", root / "pyproject.toml")
    shutil.copytree(REPO / "src" / "gen_audio", root / "src" / "gen_audio",
                    ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    catalog = root / "crates" / "gen-audio-core" / "src"
    catalog.mkdir(parents=True)
    shutil.copy2(REPO / "crates" / "gen-audio-core" / "src" / "catalog.rs", catalog / "catalog.rs")
    scripts = root / "scripts"
    scripts.mkdir()
    for name in ("improve.py", "spectrogram.py", "cube_revision.py", "compare_wavs.py",
                 "synth_kokoro_onnx.py", "generate.py"):
        shutil.copy2(REPO / "scripts" / name, scripts / name)
    for name in ("voices", "examples", "schemas", "runtimes", "apps/desktop/public/library"):
        (root / name).mkdir(parents=True, exist_ok=True)
    shutil.copy2(REPO / "runtimes/vibevoice-modern.json", root / "runtimes/vibevoice-modern.json")
    result = _ps(f"New-RuntimePayload -Root '{root}'", root)
    assert result.returncode == 0, result.stdout + result.stderr
    payload = root / "apps" / "desktop" / "src-tauri" / "gen" / "runtime"
    inventory = json.loads((payload / "payload-sha256.json").read_text(encoding="utf-8-sig"))
    assert "runtimes/vibevoice-modern.json" in inventory
    for relative, digest in inventory.items():
        assert hashlib.sha256((payload / relative).read_bytes()).hexdigest() == digest
    # Destroy checkout inputs so this cannot pass by importing development files.
    shutil.rmtree(root / "src")
    shutil.rmtree(root / "crates")
    probe = subprocess.run([sys.executable, "-c",
        "from gen_audio.prompt_script import prepare_prompt; "
        "p = prepare_prompt('Packaged prompt works.', ['alice'], 'kokoro_onnx', 6); "
        "assert p.cast['speakers']['1']['voice'] == 'af_heart'"],
        cwd=tmp_path, env={**os.environ, "PYTHONPATH": str(payload / "src"),
                         "GEN_AUDIO_REPO": str(payload)}, capture_output=True, text=True, timeout=30)
    assert probe.returncode == 0, probe.stdout + probe.stderr


@needs_shell
@pytest.mark.parametrize("version,installed,expected", [
    ("1.0.1", "", "1.0.1.0"), ("1.0.1", "1.0.1.0", "1.0.1.1"),
    ("1.0.1", "1.0.1.12", "1.0.1.13"), ("1.0.2", "1.0.1.12", "1.0.2.0"),
])
def test_msix_rebuild_increments_package_revision_without_uninstall(tmp_path, version, installed, expected):
    helper = REPO / "scripts" / "lib" / "msix.ps1"
    result = _ps(f". '{helper}'; Get-MsixPackageVersion -Version '{version}' -InstalledVersion '{installed}'", tmp_path)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == expected


@needs_shell
@pytest.mark.parametrize("version,installed", [("1.0.1", "1.0.2.0"), ("1.0.1", "1.0.1.65535"), ("65536.0.0", "")])
def test_msix_refuses_downgrade_and_revision_overflow(tmp_path, version, installed):
    helper = REPO / "scripts" / "lib" / "msix.ps1"
    assert _ps(f". '{helper}'; Get-MsixPackageVersion -Version '{version}' -InstalledVersion '{installed}'", tmp_path).returncode != 0


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
    assert gate < text.index("Invoke-CargoBinBuild -Package") and gate < text.index("npm $($tauriArgs")
    after = text[gate:text.index("Write-Step 'library WAVs match", gate)]
    assert "throw" in after and "skip" not in after.lower()


# --- C2: sidecar freshness is cargo's fingerprint + content, never mtime ---------

def _crate(tmp_path: Path) -> Path:
    crate = tmp_path / "probe"
    (crate / "src").mkdir(parents=True)
    (crate / "Cargo.toml").write_text('[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n\n[[bin]]\nname = "probe"\n'
                                      'path = "src/main.rs"\n\n[workspace]\n', encoding="utf-8")
    (crate / "src" / "main.rs").write_text('fn main() { println!("v1"); }\n', encoding="utf-8")
    (crate / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "probe"\nversion = "0.1.0"\n', encoding="utf-8")
    return crate


def _build(crate: Path) -> dict:
    env_target = crate / "target"
    out = _ps(f"Import-VsDevEnv; $env:CARGO_BUILD_JOBS = '2'; $env:CARGO_TARGET_DIR = '{env_target}'; "
              "Invoke-CargoBinBuild -Package probe -Bin probe | ConvertTo-Json -Compress", crate)
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
    duplicate = _ps(f"{load}; Select-CargoBinArtifact -Line ($l + $l) -Bin gen-audio-mcp", tmp_path)
    assert duplicate.returncode != 0 and "multiple compiler-artifacts" in duplicate.stdout + duplicate.stderr


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
    shutil.copy2(REPO / "scripts" / "lib" / "msix.ps1", root / "scripts" / "lib" / "msix.ps1")
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


# --- Low: dist, exe and installers are judged by content, never mtime ---------

@needs_shell
def test_a_noop_rebuild_output_is_unchanged_not_stale_and_a_missing_one_fails(tmp_path):
    import os

    out = tmp_path / "gen-audio.exe"
    out.write_bytes(b"MZ v1")
    snap = "$b = Get-OutputFingerprint -LiteralPath '{p}'"
    noop = _ps(f"{snap.format(p=out)}; [IO.File]::SetLastWriteTime('{out}', [datetime]'2001-01-01'); "
               f"Assert-BuildOutput -LiteralPath '{out}' -What exe -Before $b | ConvertTo-Json -Compress", tmp_path)
    assert noop.returncode == 0 and "stale" not in noop.stdout + noop.stderr, noop.stdout + noop.stderr
    result = json.loads(noop.stdout.strip().splitlines()[-1])
    assert result["State"] == "unchanged" and result["Sha256"] == hashlib.sha256(b"MZ v1").hexdigest()
    assert os.path.getmtime(out) < 1e9, "the output really is older than any run start"
    changed = _ps(f"{snap.format(p=out)}; [IO.File]::WriteAllText('{out}', 'MZ v2'); "
                  f"(Assert-BuildOutput -LiteralPath '{out}' -What exe -Before $b).State", tmp_path)
    assert changed.stdout.strip() == "rebuilt", changed.stdout + changed.stderr
    new = tmp_path / "new.msi"
    created = _ps(f"{snap.format(p=new)}; [IO.File]::WriteAllText('{new}', 'msi'); "
                  f"(Assert-BuildOutput -LiteralPath '{new}' -What installer -Before $b).State", tmp_path)
    assert created.stdout.strip() == "rebuilt", created.stdout + created.stderr
    missing = _ps(f"Assert-BuildOutput -LiteralPath '{tmp_path / 'gone.exe'}' -What exe -Before $null", tmp_path)
    assert missing.returncode != 0 and "exe is missing after the build" in missing.stdout + missing.stderr


@needs_shell
def test_dist_copy_drift_names_missing_and_changed_files_and_is_silent_when_equal(tmp_path):
    public, dist = tmp_path / "public", tmp_path / "dist"
    for root in (public, dist):
        (root / "library").mkdir(parents=True)
        (root / "library" / "assets.json").write_text("{}", encoding="utf-8")
    clean = _ps(f"Get-DistCopyDrift -Source '{public}' -Dist '{dist}'", tmp_path)
    assert clean.returncode == 0 and clean.stdout.strip() == "", clean.stdout + clean.stderr
    (public / "library" / "assets.json").write_text('{"new": 1}', encoding="utf-8")
    (public / "library" / "a.wav").write_bytes(b"RIFF")
    drift = _ps(f"Get-DistCopyDrift -Source '{public}' -Dist '{dist}'", tmp_path)
    lines = sorted(drift.stdout.strip().splitlines())
    assert lines == ["DIST_CHANGED library/assets.json", "DIST_MISSING library/a.wav"], drift.stdout + drift.stderr


def test_build_script_has_no_mtime_freshness_left():
    text = BUILD.read_text(encoding="utf-8")
    for old in ("Assert-Fresh", "LastWriteTime", "$runStart"):
        assert old not in text, f"{old} is an mtime check"
    assert "$indexPath = Join-Path $desktop 'dist\\index.html'" in text and "$exePath = Join-Path $releaseDir 'gen-audio.exe'" in text
    for var in ("$indexPath", "$exePath", "$installer"):
        assert f"Assert-BuildOutput -LiteralPath {var} " in text, var
    assert "Get-DistCopyDrift -Source" in text
    assert "$conf.version" in text, "installers are picked by this build's version, not by newest mtime"


def test_desktop_build_uses_the_locked_local_cli_and_one_frontend_hook():
    desktop = REPO / "apps" / "desktop"
    package = json.loads((desktop / "package.json").read_text(encoding="utf-8"))
    lock = json.loads((desktop / "package-lock.json").read_text(encoding="utf-8"))
    config = json.loads((desktop / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
    cli = "@tauri-apps/" + "cli"
    version = package["devDependencies"][cli]
    assert re.fullmatch(r"\d+\.\d+\.\d+", version)
    assert lock["packages"][""]["devDependencies"][cli] == version
    assert lock["packages"]["node_modules/" + cli]["version"] == version
    assert package["scripts"]["tauri"] == "tauri"
    assert config["build"]["beforeBuildCommand"] == "npm run build"
    assert package["scripts"]["dev"] == "tauri dev"
    assert config["build"]["beforeDevCommand"] == "npm run frontend:dev"
    assert package["scripts"]["frontend:dev"] == "vite"
    text = BUILD.read_text(encoding="utf-8")
    assert not (REPO / "scripts" / "build-tauri.sh").exists()
    assert "npx" not in text and "--http" not in text
    clean_command = "Invoke-Checked cargo " + "clean --target-dir $cleanTarget"
    assert clean_command in text and "Stop-Process" not in text
    assert text.index("if ($Clean)", text.index("library WAVs match")) < text.index(clean_command)
    assert "--locked" in text and "--smoke" in text
    assert text.count("Invoke-Checked npm.cmd @tauriArgs") == 1
    assert "function Invoke-Checked" not in text, "native execution belongs to the shared helper"


WINDOWS_MOCKS = r"""
function global:rustc { $global:LASTEXITCODE = 0; "host: x86_64-pc-windows-msvc" }
function global:cl.exe {}
function global:node {}
function global:cargo {
    Add-Content $env:MOCK_CALLS "cargo $args"
    if ($env:CARGO_BUILD_JOBS -ne "2") { throw "Expected bounded build jobs" }
    Set-Content $env:BUILD_TARGET $env:CARGO_TARGET_DIR
    $global:LASTEXITCODE = 0
    if ($env:FAIL_STAGE -eq "cargo") { $global:LASTEXITCODE = 9; return }
    if ($env:FAIL_STAGE -ne "artifact") {
        @{
            reason = "compiler-artifact"
            target = @{ name = "gen-audio-mcp"; kind = @("bin") }
            executable = $env:MOCK_SIDECAR
            fresh = $true
        } | ConvertTo-Json -Compress
    }
    '{"reason":"build-finished","success":true}'
}
function global:npm.cmd {
    Add-Content $env:MOCK_CALLS "npm $args"
    $global:LASTEXITCODE = 0
    if ($args[0] -eq "ci") {
        if ($env:FAIL_STAGE -eq "install") { $global:LASTEXITCODE = 9; return }
        if ($env:FAIL_STAGE -ne "cli") {
            New-Item -ItemType Directory -Force "node_modules\.bin" | Out-Null
            Set-Content "node_modules\.bin\tauri.cmd" "mock CLI"
        }
        New-Item -ItemType Directory -Force "node_modules\@tauri-apps\cli" | Out-Null
        $version = if ($env:FAIL_STAGE -eq "cli-version") { "0.0.0" } else { "2.12.1" }
        @{version=$version} | ConvertTo-Json | Set-Content "node_modules\@tauri-apps\cli\package.json"
        return
    }
    if ($env:CARGO_BUILD_JOBS -ne "2") { throw "Expected bounded packaging jobs" }
    if ($env:VITE_GEN_AUDIO_FIXTURES) { throw "Fixtures leaked into the release" }
    if ($env:CARGO_TARGET_DIR -ne (Get-Content $env:BUILD_TARGET)) { throw "Cargo output directory changed" }
    if ($env:FAIL_STAGE -eq "frontend") { $global:LASTEXITCODE = 9; return }
    $release = Split-Path -Parent $env:MOCK_SIDECAR
    switch ($env:FAIL_STAGE) {
        "dist" { Remove-Item "dist\index.html" }
        "drift" { Set-Content "dist\library\a.wav" "wrong bytes" }
        "asset" { Remove-Item "dist\assets\app.js" }
        "dev-html" { Add-Content "dist\index.html" '<script src="/src/main.ts"></script>' }
        "fixture" {
            Set-Content "public\library\assets.json" '{"assets":[{"claim":"fixture_tone"}]}'
            Copy-Item "public\library\assets.json" "dist\library\assets.json"
        }
        "example" { Set-Content "dist\assets\viewport.example-x.js" "fixture deck" }
        "dev-cfg" { Set-Content (Join-Path $release "build\gen-audio-desktop-x\output") "cargo:rustc-cfg=dev" }
        "fingerprints" {
            New-Item -ItemType Directory -Force (Join-Path $release ".fingerprint\gen-audio-desktop-y") | Out-Null
            Set-Content (Join-Path $release ".fingerprint\gen-audio-desktop-y\lib-gen_audio_desktop_lib.json") "{}"
        }
        "exe" { Remove-Item (Join-Path $release "gen-audio.exe") }
        "installer" { Remove-Item (Join-Path $release "bundle\nsis\test_0.1.0_x64-setup.exe") }
    }
}
$parameters = @{}
if ($env:BUILD_MODE -eq "alias") { $parameters.NoBundle = $true }
elseif ($env:BUILD_MODE -in @("NoBundle", "Msi")) { $parameters.Mode = $env:BUILD_MODE }
try {
    & $env:BUILD_SCRIPT @parameters
} finally {
    Set-Content $env:RESTORED_STATE "$env:GEN_AUDIO_WORK_DIR`n$PWD`n$env:CARGO_TARGET_DIR`njobs=$env:CARGO_BUILD_JOBS`nfixtures=$env:VITE_GEN_AUDIO_FIXTURES"
}
"""
MOCK_SIDECAR = r"""
Add-Content $env:MOCK_CALLS "smoke $args"
Set-Content $env:SMOKE_PATH $env:GEN_AUDIO_WORK_DIR
Set-Content (Join-Path $env:GEN_AUDIO_WORK_DIR "fixture.wav") "fixture"
$global:LASTEXITCODE = if ($env:FAIL_STAGE -eq "smoke") { 9 } else { 0 }
"""


@pytest.mark.skipif(os.name != "nt", reason="Windows build entry point")
@pytest.mark.parametrize("shell_name", ["powershell", "pwsh"])
@pytest.mark.parametrize(
    ("failure", "mode"),
    [("", "Full"), ("", "Msi"), ("", "NoBundle"), ("", "alias")]
    + [(stage, "Full") for stage in (
        "wav", "icon", "cargo", "artifact", "smoke", "install", "cli",
        "cli-version", "frontend", "dist", "drift", "asset", "dev-html",
        "fixture", "example", "dev-cfg", "fingerprints", "exe", "installer",
    )],
)
def test_windows_build_flow(tmp_path, failure, mode, shell_name):
    """PS5/PS7 fail-closed orchestration; unchanged, old outputs are valid."""
    shell = shutil.which(shell_name)
    if not shell:
        pytest.skip(f"{shell_name} is not installed")
    repo = _library(tmp_path, {"a.wav": b"RIFF"}, {"a.wav": b"RIFF"})
    repo = repo.rename(tmp_path / "checkout with spaces")
    scripts = repo / "scripts"
    (scripts / "lib").mkdir(parents=True)
    shutil.copy2(BUILD, scripts / BUILD.name)
    shutil.copy2(DEVENV, scripts / "lib" / DEVENV.name)
    shutil.copy2(REPO / "scripts" / "lib" / "msix.ps1", scripts / "lib" / "msix.ps1")
    desktop = repo / "apps" / "desktop"
    src = desktop / "src-tauri"
    src.mkdir()
    (src / "icon.ico").write_bytes(b"icon")
    (src / "hooks.nsh").write_text("; hook\n", encoding="utf-8")
    (src / "tauri.conf.json").write_text(json.dumps({
        "version": "0.1.0",
        "bundle": {"icon": ["icon.ico"], "windows": {"nsis": {"installerHooks": "hooks.nsh"}}},
        "build": {"beforeBuildCommand": "npm run build"},
    }), encoding="utf-8")
    cli = "@tauri-apps/" + "cli"
    (desktop / "package.json").write_text(json.dumps({
        "scripts": {"tauri": "tauri"}, "devDependencies": {cli: "2.12.1"},
    }), encoding="utf-8")
    (desktop / "public" / "library" / "assets.json").write_text('{"assets":[]}', encoding="utf-8")
    shutil.copytree(desktop / "public", desktop / "dist")
    (desktop / "dist" / "assets").mkdir()
    (desktop / "dist" / "assets" / "app.js").write_text("release", encoding="utf-8")
    (desktop / "dist" / "index.html").write_text(
        '<div class="brand-row ga-header-toolbar"></div><script src="/assets/app.js"></script>', encoding="utf-8",
    )
    release = tmp_path / "custom Cargo output" / "release"
    for directory, filename in (("nsis", "test_0.1.0_x64-setup.exe"), ("msi", "test_0.1.0_x64.msi")):
        bundle = release / "bundle" / directory
        bundle.mkdir(parents=True)
        (bundle / filename).write_bytes(b"unchanged installer")
    (release / "gen-audio.exe").write_bytes(b"unchanged app")
    fingerprint = release / ".fingerprint" / "gen-audio-desktop-x"
    fingerprint.mkdir(parents=True)
    (fingerprint / "lib-gen_audio_desktop_lib.json").write_text("{}", encoding="utf-8")
    build_output = release / "build" / "gen-audio-desktop-x"
    build_output.mkdir(parents=True)
    (build_output / "output").write_text("cargo:rustc-cfg=custom_protocol", encoding="utf-8")
    for output in [*release.rglob("*"), *desktop.joinpath("dist").rglob("*")]:
        if output.is_file():
            os.utime(output, (978307200, 978307200))
    if failure == "wav":
        (desktop / "public" / "library" / "a.wav").unlink()
    if failure == "icon":
        (src / "icon.ico").unlink()
    sidecar = release / "sidecar.ps1"
    sidecar.write_text(MOCK_SIDECAR, encoding="utf-8")
    runner = tmp_path / "runner.ps1"
    runner.write_text(WINDOWS_MOCKS, encoding="utf-8")
    calls_file, restored, smoke, target = [tmp_path / name for name in ("calls.log", "state.txt", "smoke.txt", "target.txt")]
    env = {
        **os.environ,
        "BUILD_SCRIPT": str(scripts / BUILD.name), "MOCK_SIDECAR": str(sidecar),
        "MOCK_CALLS": str(calls_file), "RESTORED_STATE": str(restored),
        "SMOKE_PATH": str(smoke), "BUILD_TARGET": str(target),
        "CARGO_TARGET_DIR": "custom-target", "CARGO_BUILD_JOBS": "8",
        "GEN_AUDIO_WORK_DIR": "preserve-user-setting", "VITE_GEN_AUDIO_FIXTURES": "1",
        "FAIL_STAGE": failure, "BUILD_MODE": mode,
    }
    result = subprocess.run([shell, "-NoProfile", "-NonInteractive", "-File", str(runner)],
                            cwd=tmp_path, env=env, capture_output=True, text=True, timeout=120)
    output = result.stdout + result.stderr
    assert (result.returncode == 0) == (not failure), output
    assert restored.read_text(encoding="utf-8-sig").splitlines() == [
        "preserve-user-setting", str(tmp_path), "custom-target", "jobs=8", "fixtures=1",
    ]
    calls = calls_file.read_text(encoding="utf-8-sig").splitlines() if calls_file.exists() else []
    if failure not in {"wav", "icon"}:
        assert calls[0] == "cargo build --locked -p gen-audio-mcp --bin gen-audio-mcp --message-format=json-render-diagnostics --release"
        assert Path(target.read_text(encoding="utf-8-sig").strip()) == repo / "custom-target"
    else:
        assert calls == []
    early = {"wav", "icon", "cargo", "artifact", "smoke", "install", "cli", "cli-version"}
    tauri_calls = [call for call in calls if "run tauri" in call]
    if failure in early:
        assert not tauri_calls
    else:
        option = {"Full":"--bundles nsis,msi", "Msi":"--bundles msi"}.get(mode, "--no-bundle")
        assert tauri_calls == ["npm run " + f"tauri -- build {option} -- --locked"]
    if smoke.exists():
        assert not Path(smoke.read_text(encoding="utf-8-sig").strip()).exists(), "smoke scratch leaked"
    if not failure:
        staged = src / "binaries" / "gen-audio-mcp-x86_64-pc-windows-msvc.exe"
        assert staged.read_bytes() == sidecar.read_bytes()
        assert output.count("BUILD_OK ") == 1 and "state=unchanged" in output
        assert output.count("INSTALLER path=") == {"Full":2, "Msi":1}.get(mode, 0)
    else:
        assert "BUILD_OK " not in output and "BUILD_FAIL " in output
    logs = list((repo / "target" / "logs").glob("build-*.log"))
    assert len(logs) == 1
    transcript = logs[0].read_text(encoding="utf-8-sig")
    assert "PowerShell transcript end" in transcript
    assert ("BUILD_FAIL " if failure else "BUILD_OK ") in transcript
