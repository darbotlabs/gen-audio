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
# Matched against decoded string values, so one backslash means one.
MACHINE_PATH = re.compile(r"[A-Za-z]:\\|/home/|/Users/|C:\\\\Users", re.IGNORECASE)


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
    hits = [(where, text) for where, text in _strings(doc) if MACHINE_PATH.search(text)]
    assert hits == [], f"{path.relative_to(REPO)} ships machine paths: {hits[:5]}"


def test_the_check_would_catch_the_old_abswav():
    assert MACHINE_PATH.search("D:\\gen-audio\\artifacts\\library\\library_kokoro.wav")
    assert MACHINE_PATH.search("/home/box/x.wav") and MACHINE_PATH.search("/Users/me/x.wav")
    assert not MACHINE_PATH.search("artifacts/library/library_kokoro.wav") and not MACHINE_PATH.search("/library/a.wav")


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
