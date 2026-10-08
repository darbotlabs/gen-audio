"""F: no machine paths in shipped JSON.

apps/desktop/public/library is copied into dist and embedded in the release
app, and the viewport decks ship (release) or seed dev builds (example).
None may carry a drive-letter path, a home directory, or C:\\Users.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
SHIPPED = sorted((REPO / "apps" / "desktop" / "public" / "library").glob("*.json")) + sorted(
    (REPO / "schemas" / "examples").glob("viewport*.json")
)
# Any absolute filesystem path, matched against decoded string values (one
# backslash means one): a drive letter (C:\ or C:/), a UNC share (\\host), or
# a leading "/" that starts a path rather than continuing a URL or a relative
# path. A leading "/" is allowed only when it is a URL into the app's own web
# root, i.e. its first segment is a top-level entry of apps/desktop/public
# (/library/...): that is where dist serves from, not a machine path.
DRIVE_OR_UNC = re.compile(r"(?<![A-Za-z0-9])[A-Za-z]:[\\/]|(?<!\\)\\\\(?=[\w.$-])")
POSIX_ABS = re.compile(r"(?<![\w.:/~-])/([\w.~-][^\s\"'<>]*)")
WEB_ROOT = {path.name for path in (REPO / "apps" / "desktop" / "public").iterdir()}


def machine_paths(text: str) -> list[str]:
    hits = [match.group(0) for match in DRIVE_OR_UNC.finditer(text)]
    for match in POSIX_ABS.finditer(text):
        if match.group(1).split("/", 1)[0] not in WEB_ROOT:
            hits.append(match.group(0))
    return hits


def _strings(value, where="$"):
    if isinstance(value, str):
        yield where, value
    elif isinstance(value, dict):
        for key, item in value.items():
            yield where + "." + key, key
            yield from _strings(item, f"{where}.{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from _strings(item, f"{where}[{index}]")


def test_the_shipped_set_is_what_we_think():
    names = {path.name for path in SHIPPED}
    assert {"assets.json", "manifest.json", "viewport.release.json", "viewport.example.json"} <= names
    assert any(name.endswith("_cube3d.json") for name in names)


@pytest.mark.parametrize("path", SHIPPED, ids=lambda path: path.name)
def test_no_machine_paths_in_shipped_json(path):
    doc = json.loads(path.read_text(encoding="utf-8"))
    hits = [(where, text) for where, text in _strings(doc) if machine_paths(text)]
    assert hits == [], f"{path.relative_to(REPO)} ships machine paths: {hits[:5]}"


def test_the_check_catches_any_absolute_path_not_only_the_ones_seen_before():
    caught = [
        "D:\\gen-audio\\artifacts\\library\\library_kokoro.wav", "c:/x/y.wav", "\\\\smax\\share\\a.wav",
        "/home/box/x.wav", "/Users/me/x.wav", "/workspace/genaid-podcast-compare/models/VibeVoice-1.5B",
        "/tmp/a.wav", "/opt/models/k.onnx", "/mnt/d/x", "/srv/a",
        "cd /workspace/x && python run.py --out /data/out.wav", "PYTHONPATH=/workspace/m python",
    ]
    for text in caught:
        assert machine_paths(text), text
    allowed = [
        "artifacts/library/library_kokoro.wav", "/library/a.wav", "/library/library_kokoro_onnx.synth.json",
        "https://github.com/darbotlabs/gen-audio", "audio/wav", "1/2", "../x/y.json", "a/b/c", "http://127.0.0.1:8765/mcp",
    ]
    for text in allowed:
        assert not machine_paths(text), text


def test_manifest_generator_drops_abswav(tmp_path):
    from gen_audio.library_manifest import render, sync_manifest

    manifest = json.loads((REPO / "apps" / "desktop" / "public" / "library" / "manifest.json").read_text(encoding="utf-8"))
    manifest["clips"][0]["absWav"] = "D:\\gen-audio\\artifacts\\library\\x.wav"
    library = tmp_path / "library"
    library.mkdir()
    for path in (REPO / "apps" / "desktop" / "public" / "library").glob("*_cube3d.json"):
        (library / path.name).write_bytes(path.read_bytes())
    target = library / "manifest.json"
    target.write_text(render(manifest), encoding="utf-8")
    assert sync_manifest(target) == [manifest["clips"][0]["id"]]
    assert "absWav" not in target.read_text(encoding="utf-8")


SIDE = {
    "engine": "kokoro_onnx",
    "model": "/workspace/genaid-podcast-compare/forks/kokoro-onnx/models/kokoro-v1.0.onnx",
    "out": "D:\\gen-audio\\artifacts\\library\\x.wav",
    "share": "\\\\smax\\share\\voices\\cast_map.json",
    "home": "/home/box/models/voices.bin",
    "command": "cd /workspace/m/VibeVoice && PYTHONPATH=/workspace/m /workspace/v/bin/python run.py --out /workspace/out",
    "url": "https://github.com/darbotlabs/gen-audio",
    "wavUrl": "/library/x.wav",
    "already": "../genaid-podcast-compare/voices/cast_map.json",
    "n_turns": 18,
    "n_words": 816,
    "turns": [{"note": "see /workspace/gen-audio-library/out/script.txt"}],
}
RELATIVE = {
    "engine": "kokoro_onnx",
    "model": "../genaid-podcast-compare/forks/kokoro-onnx/models/kokoro-v1.0.onnx",
    "out": "../gen-audio/artifacts/library/x.wav",
    "share": "../share/voices/cast_map.json",
    "home": "../models/voices.bin",
    "command": "cd ../m/VibeVoice && PYTHONPATH=../m ../v/bin/python run.py --out ../out",
    "url": "https://github.com/darbotlabs/gen-audio",
    "wavUrl": "/library/x.wav",
    "already": "../genaid-podcast-compare/voices/cast_map.json",
    "n_turns": 18,
    "n_words": 816,
    "turns": [{"note": "see ../gen-audio-library/out/script.txt"}],
}


def test_manifest_sync_rewrites_synth_sidecar_paths_repo_relative_and_is_idempotent(tmp_path):
    """F: provenance paths are kept, rewritten relative to the repo (whose parent holds the sibling checkouts)."""
    from gen_audio.library_manifest import render, sync_manifest

    library = tmp_path / "library"
    library.mkdir()
    for path in (REPO / "apps" / "desktop" / "public" / "library").glob("*_cube3d.json"):
        (library / path.name).write_bytes(path.read_bytes())
    (library / "manifest.json").write_bytes((REPO / "apps" / "desktop" / "public" / "library" / "manifest.json").read_bytes())
    sidecar = library / "a.synth.json"
    sidecar.write_text(json.dumps(SIDE, indent=2), encoding="utf-8")  # no trailing newline, as the old run scripts wrote
    canonical = library / "b.synth.json"
    canonical.write_text(render({"engine": "kokoro_onnx", "turns": []}), encoding="utf-8")
    before = canonical.read_bytes()
    changed = sync_manifest(library / "manifest.json")
    assert "a.synth.json" in changed and "b.synth.json" not in changed
    assert json.loads(sidecar.read_text(encoding="utf-8")) == RELATIVE
    assert sidecar.read_text(encoding="utf-8") == render(RELATIVE)
    assert canonical.read_bytes() == before, "a sidecar with no absolute path is left byte for byte"
    once = sidecar.read_bytes()
    assert sync_manifest(library / "manifest.json") == [] and sidecar.read_bytes() == once, "a second sync changes 0 bytes"
    assert not machine_paths(sidecar.read_text(encoding="utf-8"))


def test_synth_sidecar_writer_records_input_paths_repo_relative(tmp_path):
    """F: the kokoro-onnx CLI's sidecar keeps its inputs as provenance, never as absolute paths."""
    from gen_audio.cli.synth import sidecar_payload

    repo = tmp_path / "work" / "gen-audio"
    (repo / "out").mkdir(parents=True)
    sibling = tmp_path / "work" / "genaid-podcast-compare" / "models"
    inputs = {"script": repo / "scripts" / "podcast.txt", "cast_map": sibling.parent / "voices" / "cast_map.json",
              "model": sibling / "kokoro-v1.0.onnx", "voices": sibling / "voices-v1.0.bin", "output": repo / "out" / "x.wav"}
    payload = sidecar_payload({"engine": "kokoro_onnx", "turns": []}, "PCM_16", inputs, repo)
    assert payload["wav_subtype"] == "PCM_16" and payload["engine"] == "kokoro_onnx"
    assert payload["inputs"] == {
        "script": "scripts/podcast.txt",
        "cast_map": "../genaid-podcast-compare/voices/cast_map.json",
        "model": "../genaid-podcast-compare/models/kokoro-v1.0.onnx",
        "voices": "../genaid-podcast-compare/models/voices-v1.0.bin",
        "output": "out/x.wav",
    }
    assert not machine_paths(json.dumps(payload))
