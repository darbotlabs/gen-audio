"""The deployment entry point resolves paths from its checkout, not its caller."""

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest


SHELL = shutil.which("pwsh") or shutil.which("powershell")
REPO = Path(__file__).resolve().parents[1]


@pytest.mark.skipif(SHELL is None, reason="needs PowerShell")
@pytest.mark.parametrize("service,port", [("Mcp", 8765), ("Node", 8002), ("Desktop", 8765), ("Preview", 1420)])
def test_launch_plan_is_checkout_relative(tmp_path, service, port):
    repo = tmp_path / "checkout with spaces"
    scripts = repo / "scripts"
    scripts.mkdir(parents=True)
    shutil.copy2(REPO / "scripts" / "run.ps1", scripts / "run.ps1")
    result = subprocess.run(
        [SHELL, "-NoProfile", "-File", str(scripts / "run.ps1"), "-Service", service, "-Check"],
        cwd=tmp_path,
        env={**os.environ, "GENAID_MODEL_BASE": str(tmp_path / "stale-models")},
        capture_output=True, text=True, check=True, timeout=30,
    )
    plan = json.loads(result.stdout)
    env = plan["environment"]
    assert Path(env["GEN_AUDIO_REPO"]) == repo
    assert Path(env["GENAID_MODEL_BASE"]) == repo / "models"
    assert env["GENAID_MODEL_BASE"] == env["GEN_AUDIO_MODEL_DIR"]
    assert Path(env["GEN_AUDIO_WORK_DIR"]) == repo / "artifacts" / "runtime" / "mcp"
    assert env["GEN_AUDIO_MCP_ADDR"] == f"127.0.0.1:{port if service in ('Mcp', 'Desktop') else 8765}"
    assert not (repo / "artifacts").exists(), "-Check must not mutate the checkout"
    if service == "Desktop":
        assert Path(plan["executable"]).name == "gen-audio.exe"
    elif service == "Preview":
        assert "--strictPort" in plan["arguments"]


@pytest.mark.skipif(SHELL is None, reason="needs PowerShell")
def test_native_build_entry_point_parses():
    path = str(REPO / "scripts" / "build-tauri-windows.ps1").replace("'", "''")
    result = subprocess.run(
        [SHELL, "-NoProfile", "-Command",
         f"$e=$null; [void][System.Management.Automation.Language.Parser]::ParseFile('{path}',"
         "[ref]$null,[ref]$e); if($e.Count){$e | Out-String | Write-Output; exit 1}"],
        capture_output=True, text=True, timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
