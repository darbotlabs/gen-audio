"""The installer must reject a downgrade before mutating an environment."""
import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("runtime_upgrade", ROOT / "src/gen_audio/cli/upgrade_runtime.py")
upgrade = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upgrade)


@pytest.mark.parametrize("name,before,after", [("fsspec","2026.9.0","2024.12.0"),
    ("torch","2.9.1+rocm7.2.1","2.9.1"), ("transformers","5.19.0","4.51.3")])
def test_downgrades_are_rejected_before_install(monkeypatch, tmp_path, name, before, after):
    monkeypatch.setattr(upgrade.importlib.metadata, "distributions", lambda: [SimpleNamespace(metadata={"Name":name},version=before)])
    monkeypatch.setattr(sys, "argv", ["upgrade", "--report-dir", str(tmp_path), name])
    calls=[]
    def run(argv, **kwargs):
        calls.append(argv)
        assert "--dry-run" in argv, "A package mutation must never be reached"
        Path(argv[argv.index("--report")+1]).write_text(json.dumps({"install":[{"metadata":{"name":name,"version":after}}]}))
    monkeypatch.setattr(upgrade.subprocess, "run", run)
    with pytest.raises(RuntimeError, match="Downgrade rejected"):
        upgrade.main()
    assert len(calls) == 1


def test_install_uses_the_exact_reviewed_wheel_hashes(monkeypatch, tmp_path):
    monkeypatch.setattr(upgrade.importlib.metadata, "distributions", lambda: [SimpleNamespace(metadata={"Name":"example"},version="1.0")])
    monkeypatch.setattr(sys, "argv", ["upgrade", "--report-dir", str(tmp_path), "example"])
    calls=[]
    def run(argv, **kwargs):
        calls.append(argv)
        if "--dry-run" in argv:
            Path(argv[argv.index("--report")+1]).write_text(json.dumps({"install":[{
                "metadata":{"name":"example","version":"2.0"},
                "download_info":{"url":"https://example.invalid/example-2.0-py3-none-any.whl",
                                 "archive_info":{"hashes":{"sha256":"a"*64}}}}]}))
    monkeypatch.setattr(upgrade.subprocess, "run", run)
    upgrade.main()
    assert "--require-hashes" in calls[1] and "--no-deps" in calls[1]
    assert calls[2][-1] == "check"
    assert "--hash=sha256:"+"a"*64 in (tmp_path/"resolved-wheels.txt").read_text()
