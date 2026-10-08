"""F: no machine paths in shipped JSON.

apps/desktop/public/library is copied into dist and embedded in the release
app, and the viewport decks ship (release) or seed dev builds (example).
None may carry a drive-letter path, a home directory, or C:\\Users.
"""

from __future__ import annotations

import hashlib
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
# (/library/...): that is where dist serves from, not a machine path. The
# "/" after an <outside-repo> label (B3) is not one either.
DRIVE_OR_UNC = re.compile(r"(?<![A-Za-z0-9])[A-Za-z]:[\\/]|(?<!\\)\\\\(?=[\w.$-])")
POSIX_ABS = re.compile(r"(?<![\w.:/~-])(?<!<outside-repo>)/([\w.~-][^\s\"'<>]*)")
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
        "<outside-repo>/genaid-podcast-compare/models/k.onnx", "cd <outside-repo>/m && <outside-repo>/v/bin/python run.py",
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


OUTSIDE = "<outside-repo>/"
MARKED = re.compile(r"<outside-repo>/[^\s\"'<>]+")
# The bitdot run as it was recorded on the box (before any rewrite): absolute
# paths, so its `cd` changed nothing for the other arguments.
BITDOT_RECORDED = (
    "cd /workspace/genaid-podcast-compare/models/VibeVoice-community && "
    "PYTHONPATH=/workspace/genaid-podcast-compare/models/VibeVoice-community "
    "/workspace/genaid-podcast-compare/venvs/vibevoice/bin/python demo/inference_from_file.py "
    "--model_path /workspace/genaid-podcast-compare/models/VibeVoice-1.5B "
    "--txt_path /workspace/genaid-podcast-compare/podcast_bitdot_braille.txt --speaker_names Alice Frank "
    "--output_dir /workspace/genaid-podcast-compare/audio/vv_bitdot_braille --device cpu --dtype float16 "
    "--cfg_scale 1.3 --seed 42"
)
SIDECARS = sorted((REPO / "apps" / "desktop" / "public" / "library").glob("*.synth.json"))


def _work(tmp_path):
    """A repo at <tmp>/work/gen-audio with a library, beside sibling work dirs."""
    repo = tmp_path / "work" / "gen-audio"
    library = repo / "apps" / "desktop" / "public" / "library"
    library.mkdir(parents=True)
    for path in (REPO / "apps" / "desktop" / "public" / "library").glob("*_cube3d.json"):
        (library / path.name).write_bytes(path.read_bytes())
    (library / "manifest.json").write_bytes((REPO / "apps" / "desktop" / "public" / "library" / "manifest.json").read_bytes())
    return repo, library


def _file(path, data: bytes):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}


def test_outside_repo_labels_never_fake_a_relative_path():
    """B3: /tmp/a.wav is not ../a.wav. Inside the repo a path is repo-relative;
    beside it (the repo's parent) it is <outside-repo>/<rest>; anywhere else
    only its name is kept. The sha256 recorded with it is the identity."""
    from gen_audio.library_manifest import outside_repo_label

    repo = "/workspace/gen-audio"
    assert outside_repo_label("/tmp/a.wav", repo) == "<outside-repo>/a.wav"
    assert outside_repo_label("/workspace/genaid-podcast-compare/models/k.onnx", repo) == "<outside-repo>/genaid-podcast-compare/models/k.onnx"
    assert outside_repo_label("/workspace/gen-audio/scripts/x.txt", repo) == "scripts/x.txt"
    assert outside_repo_label("/workspace/gen-audio-library/out/x.wav", repo) == "<outside-repo>/gen-audio-library/out/x.wav"
    assert outside_repo_label("/home/box/models/voices.bin", repo) == "<outside-repo>/voices.bin"
    assert outside_repo_label("D:\\gen-audio\\artifacts\\x.wav", "D:\\gen-audio-pr5") == "<outside-repo>/gen-audio/artifacts/x.wav"
    assert outside_repo_label("D:\\gen-audio-pr5\\scripts\\x.txt", "D:\\gen-audio-pr5") == "scripts/x.txt"
    assert outside_repo_label("\\\\smax\\share\\voices\\cast_map.json", "D:\\gen-audio-pr5") == "<outside-repo>/cast_map.json"
    assert outside_repo_label("D:\\gen-audio\\x.wav", repo) == "<outside-repo>/x.wav"


def test_manifest_sync_marks_outside_repo_inputs_with_sha256_and_is_idempotent(tmp_path):
    """B3: an absolute or ../ path in a synth sidecar becomes an explicit
    <outside-repo>/ label, and the sidecar records that file's sha256."""
    from gen_audio.library_manifest import render, sync_manifest

    repo, library = _work(tmp_path)
    work = repo.parent
    model = _file(work / "genaid-podcast-compare" / "forks" / "m.onnx", b"model bytes")
    cast = _file(work / "genaid-podcast-compare" / "voices" / "cast_map.json", b"{}")
    elsewhere = _file(tmp_path / "elsewhere" / "a.wav", b"RIFF a")
    script = _file(work / "gen-audio-library" / "out" / "script.txt", b"Speaker 1: hi")
    (work / "m" / "VibeVoice").mkdir(parents=True)
    _file(work / "v" / "bin" / "python", b"#!python")
    side = {
        "engine": "kokoro_onnx",
        "model": str(work / "genaid-podcast-compare" / "forks" / "m.onnx"),
        "cast_map": "../genaid-podcast-compare/voices/cast_map.json",
        "out": str(tmp_path / "elsewhere" / "a.wav"),
        "command": f"cd {work}/m/VibeVoice && {work}/v/bin/python run.py --out {tmp_path}/elsewhere/a.wav",
        "url": "https://github.com/darbotlabs/gen-audio",
        "wavUrl": "/library/x.wav",
        "turns": [{"note": "see ../gen-audio-library/out/script.txt"}],
    }
    sidecar = library / "a.synth.json"
    sidecar.write_text(json.dumps(side, indent=2), encoding="utf-8")
    canonical = library / "b.synth.json"
    canonical.write_text(render({"engine": "kokoro_onnx", "turns": []}), encoding="utf-8")
    before = canonical.read_bytes()
    changed = sync_manifest(library / "manifest.json")
    assert "a.synth.json" in changed and "b.synth.json" not in changed
    doc = json.loads(sidecar.read_text(encoding="utf-8"))
    assert doc["model"] == "<outside-repo>/genaid-podcast-compare/forks/m.onnx"
    assert doc["cast_map"] == "<outside-repo>/genaid-podcast-compare/voices/cast_map.json"
    assert doc["out"] == "<outside-repo>/a.wav"
    assert doc["command"] == "cd <outside-repo>/m/VibeVoice && <outside-repo>/v/bin/python run.py --out <outside-repo>/a.wav"
    assert doc["turns"] == [{"note": "see <outside-repo>/gen-audio-library/out/script.txt"}]
    assert doc["url"] == side["url"] and doc["wavUrl"] == "/library/x.wav"
    assert doc["outside_repo"] == {
        "<outside-repo>/a.wav": elsewhere,
        "<outside-repo>/gen-audio-library/out/script.txt": script,
        "<outside-repo>/genaid-podcast-compare/forks/m.onnx": model,
        "<outside-repo>/genaid-podcast-compare/voices/cast_map.json": cast,
        "<outside-repo>/m/VibeVoice": {"kind": "directory"},
        "<outside-repo>/v/bin/python": {"sha256": hashlib.sha256(b"#!python").hexdigest(), "bytes": 8},
    }
    assert sidecar.read_text(encoding="utf-8") == render(doc)
    assert canonical.read_bytes() == before, "a sidecar with no outside path is left byte for byte"
    once = sidecar.read_bytes()
    assert sync_manifest(library / "manifest.json") == [] and sidecar.read_bytes() == once, "a second sync changes 0 bytes"
    text = sidecar.read_text(encoding="utf-8")
    assert not machine_paths(text) and "../" not in text


def test_manifest_sync_refuses_an_outside_path_it_cannot_hash(tmp_path):
    """No sha256, no label: a path that isn't on this machine is not rewritten
    into provenance nobody can check; the sync names it and fails."""
    from gen_audio.library_manifest import sync_manifest

    _, library = _work(tmp_path)
    sidecar = library / "a.synth.json"
    sidecar.write_text(json.dumps({"engine": "x", "out": "/nonexistent-gen-audio-b3/a.wav"}), encoding="utf-8")
    before = sidecar.read_bytes()
    with pytest.raises(FileNotFoundError, match="a.synth.json.*<outside-repo>/a.wav"):
        sync_manifest(library / "manifest.json")
    assert sidecar.read_bytes() == before


def test_synth_sidecar_writer_records_outside_inputs_as_labels_with_sha256(tmp_path):
    """B3: the kokoro-onnx CLI's sidecar keeps in-repo inputs repo-relative and
    outside ones as <outside-repo>/ labels with their sha256."""
    from gen_audio.cli.synth import sidecar_payload

    repo = tmp_path / "work" / "gen-audio"
    sibling = tmp_path / "work" / "genaid-podcast-compare" / "models"
    facts = {
        "cast_map": _file(sibling.parent / "voices" / "cast_map.json", b"{}"),
        "model": _file(sibling / "kokoro-v1.0.onnx", b"onnx"),
        "voices": _file(sibling / "voices-v1.0.bin", b"bin"),
        "output": _file(tmp_path / "out" / "x.wav", b"RIFF x"),
    }
    _file(repo / "scripts" / "podcast.txt", b"Speaker 1: hi")
    inputs = {"script": repo / "scripts" / "podcast.txt", "cast_map": sibling.parent / "voices" / "cast_map.json",
              "model": sibling / "kokoro-v1.0.onnx", "voices": sibling / "voices-v1.0.bin", "output": tmp_path / "out" / "x.wav"}
    payload = sidecar_payload({"engine": "kokoro_onnx", "turns": []}, "PCM_16", inputs, repo)
    assert payload["wav_subtype"] == "PCM_16" and payload["engine"] == "kokoro_onnx"
    assert payload["inputs"] == {
        "script": "scripts/podcast.txt",
        "cast_map": "<outside-repo>/genaid-podcast-compare/voices/cast_map.json",
        "model": "<outside-repo>/genaid-podcast-compare/models/kokoro-v1.0.onnx",
        "voices": "<outside-repo>/genaid-podcast-compare/models/voices-v1.0.bin",
        "output": "<outside-repo>/x.wav",
    }
    assert payload["outside_repo"] == {payload["inputs"][name]: fact for name, fact in facts.items()}
    assert not machine_paths(json.dumps(payload)) and "../" not in json.dumps(payload)


@pytest.mark.parametrize("path", SIDECARS, ids=lambda path: path.name)
def test_shipped_sidecars_label_every_outside_path_and_record_its_sha256(path):
    text = path.read_text(encoding="utf-8")
    doc = json.loads(text)
    assert "../" not in text, f"{path.name}: a ../ path is a guess about the checkout layout"
    labels = set(MARKED.findall(json.dumps({k: v for k, v in doc.items() if k != "outside_repo"})))
    recorded = doc.get("outside_repo", {})
    assert labels == set(recorded), f"{path.name}: labels without facts {labels - set(recorded)}, facts without labels {set(recorded) - labels}"
    for label, fact in recorded.items():
        assert fact == {"kind": "directory"} or (re.fullmatch(r"[0-9a-f]{64}", fact["sha256"]) and fact["bytes"] > 0), (label, fact)


def test_the_shipped_bitdot_command_runs_as_written_from_the_repo_root():
    """B3: '../' paths after `cd X &&` pointed somewhere else than the run used.
    With <outside-repo> set to the dir that held the run (/workspace), the
    shipped command is the recorded one, from any cwd."""
    doc = json.loads((REPO / "apps" / "desktop" / "public" / "library" / "bitdot_braille_vibevoice.synth.json").read_text(encoding="utf-8"))
    assert doc["command"].replace(OUTSIDE, "/workspace/") == BITDOT_RECORDED
    assert doc["model_path"] == "<outside-repo>/genaid-podcast-compare/models/VibeVoice-1.5B"


def test_the_shipped_kokoro_onnx_sidecar_out_is_the_locked_wav():
    doc = json.loads((REPO / "apps" / "desktop" / "public" / "library" / "library_kokoro_onnx.synth.json").read_text(encoding="utf-8"))
    lock = json.loads((REPO / "schemas" / "asset-object" / "media.lock.json").read_text(encoding="utf-8"))["media"]
    out = doc["outside_repo"][doc["out"]]
    assert out == {"sha256": lock["library_kokoro_onnx.wav"]["sha256"], "bytes": lock["library_kokoro_onnx.wav"]["bytes"]}
