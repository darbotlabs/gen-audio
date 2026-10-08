"""Four-layer Library cube (gen_audio.cube_layers, cube_revision.py layers)."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
import pytest

from gen_audio.audio_io import read_wav, write_wav
from gen_audio.cli.cube import main as cube_main
from gen_audio.cube_layers import (
    GENERATOR_PATH,
    LAYER_METHOD,
    LAYER_NAMES,
    LIBRARY_PARAMS,
    LIBRARY_REVISION,
    REPO_ROOT,
    CubeParams,
    generator_commit,
    library_cube,
)
from gen_audio.cube_revision import measure

LIBRARY = Path(__file__).resolve().parents[1] / "apps" / "desktop" / "public" / "library"
SHA = "0" * 64
COMMIT = "1" * 40


def _speechish(seconds: float = 3.0, sr: int = 24000) -> np.ndarray:
    t = np.arange(int(seconds * sr)) / sr
    env = 0.5 + 0.5 * np.sin(2 * np.pi * 3 * t)
    tone = sum(np.sin(2 * np.pi * f * t) / k for k, f in enumerate((220, 440, 880, 1760, 3520), start=1))
    noise = np.random.default_rng(0).normal(0, 0.01, len(t))
    return (0.3 * env * tone / 2.3 + noise).astype(np.float32)


def test_library_cube_shape_and_scrub_mapping():
    audio, sr = _speechish(), 24000
    doc, _cloud = library_cube(audio, sr, stem="tone", engine="fixture", source_sha256=SHA, commit=COMMIT)
    params = CubeParams()
    nf, nt = doc["cube_shape_f_t"]
    sf, st = doc["downsample_sf_st"]
    assert nf == (params.n_fft // 2 + 1) // sf and nt <= params.max_t
    assert doc["bin_seconds"] == st * params.hop / sr
    assert doc["cube_covers_s"] == pytest.approx(nt * doc["bin_seconds"])
    assert doc["cube_covers_s"] <= doc["duration_s"] + doc["bin_seconds"]  # STFT padding
    assert doc["cube_revision"] == 3 and doc["wavUrl"] == "/library/tone.wav"
    assert set(doc["layers"]) == set(LAYER_NAMES)
    assert 0 < doc["inv_hdr"] < 1 and 0 < doc["layer_score"] < 1
    assert doc["inv_hdr"] == pytest.approx(measure(audio, sr).inv_hdr)
    for point in doc["points_preview"]:
        assert 0 <= point["t"] <= 1 and 0 <= point["f"] <= 1 and 0 <= point["v"] <= 1
    assert doc["n_points"] == len(doc["points_preview"]) <= params.per_layer * 4


def test_library_cube_is_deterministic():
    audio = _speechish(1.5)
    first, _ = library_cube(audio, 24000, stem="a", engine="e", source_sha256=SHA, commit=COMMIT)
    second, _ = library_cube(audio.copy(), 24000, stem="a", engine="e", source_sha256=SHA, commit=COMMIT)
    assert json.dumps(first) == json.dumps(second)


def test_library_cube_rejects_empty_audio():
    with pytest.raises(ValueError):
        library_cube(np.zeros(0, np.float32), 24000, stem="x", engine="e", source_sha256=SHA, commit=COMMIT)


def test_library_cube_records_rev3_provenance_and_rejects_bad_ids():
    doc, _ = library_cube(_speechish(1.0), 24000, stem="a", engine="e", source_sha256=SHA, commit=COMMIT)
    assert doc["cube_revision"] == LIBRARY_REVISION == 3
    assert doc["source_sha256"] == SHA
    assert doc["provenance"] == {
        "generator": "src/gen_audio/cube_layers.py",
        "generator_commit": COMMIT,
        "layer_method": "library_r3",
        "params": {"n_fft": 1024, "hop": 256, "max_f": 128, "max_t": 400, "thresh": 0.12, "per_layer": 900},
    }
    assert LIBRARY_PARAMS == CubeParams()
    with pytest.raises(ValueError):
        library_cube(_speechish(1.0), 24000, stem="a", engine="e", source_sha256="ABC", commit=COMMIT)
    with pytest.raises(ValueError):
        library_cube(_speechish(1.0), 24000, stem="a", engine="e", source_sha256=SHA, commit="abc")


def test_generator_path_exists():
    """provenance.generator is a real repo path (E4)."""
    assert (REPO_ROOT / GENERATOR_PATH).is_file()
    assert (REPO_ROOT / GENERATOR_PATH).resolve() == Path(__file__).resolve().parents[1] / "src" / "gen_audio" / "cube_layers.py"


def test_layers_cli_writes_json_and_png(tmp_path):
    wav = write_wav(tmp_path / "tone.wav", _speechish(2.0), 24000)
    out, png = tmp_path / "cube.json", tmp_path / "cube.png"
    assert cube_main(["layers", str(wav), str(out), "--stem", "tone", "--engine", "fixture", "--png", str(png)]) == 0
    doc = json.loads(out.read_text(encoding="utf-8"))
    assert doc["engine"] == "fixture" and doc["cube_revision"] == 3
    assert doc["source_sha256"] == hashlib.sha256(wav.read_bytes()).hexdigest()
    assert doc["provenance"]["generator_commit"] == generator_commit()
    assert png.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"


# (wav stem, cube JSON, engine, label): every Library cube, all rev 3 (E4).
GENERATED = [
    ("bitdot_braille_vibevoice", "library_bitdot_braille_vibevoice_cube3d.json", "vibevoice", "Bitdot braille (VibeVoice-1.5B)"),
    ("genaid_full_misaki_kokoro", "library_genaid_full_misaki_kokoro_cube3d.json", "misaki_kokoro", "misaki\u2192kokoro"),
    ("library_cube_explainer_kokoro_onnx", "library_cube_explainer_kokoro_onnx_cube3d.json", "kokoro_onnx", "Cube explainer (kokoro-onnx)"),
    ("library_kokoro_onnx", "library_kokoro_onnx_cube3d.json", "kokoro_onnx", "kokoro-onnx"),
    ("library_kokoro", "library_kokoro_cube3d.json", "kokoro_dayour", "dayour/kokoro"),
]
LIBRARY_CUBES = [(stem, cube) for stem, cube, _engine, _label in GENERATED]


def _need_wav(stem: str) -> Path:
    wav = LIBRARY / f"{stem}.wav"
    if not wav.exists():
        pytest.skip(f"{wav.name} is gitignored and not staged here (present on SMAX and the box)")
    return wav


def _full_history() -> bool:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--is-shallow-repository"], cwd=REPO_ROOT, capture_output=True, text=True, check=True
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return False
    return out == "false"


def test_every_library_cube_json_is_listed_here():
    assert sorted(path.name for path in LIBRARY.glob("*_cube3d.json")) == sorted(cube for _s, cube, _e, _l in GENERATED)


@pytest.mark.parametrize(("stem", "cube", "engine", "label"), GENERATED)
def test_reproduces_shipped_cube_byte_for_byte(stem, cube, engine, label):
    """The regen-diff gate for cubes: same WAV + generator commit -> same bytes."""
    wav = _need_wav(stem)
    shipped = (LIBRARY / cube).read_text(encoding="utf-8")
    recorded = json.loads(shipped)["provenance"]["generator_commit"]
    commit = generator_commit() if _full_history() else recorded
    audio, sr = read_wav(wav)
    doc, _ = library_cube(
        audio, sr, stem=stem, engine=engine, source_sha256=hashlib.sha256(wav.read_bytes()).hexdigest(), commit=commit, label=label
    )
    assert json.dumps(doc, ensure_ascii=False) == shipped, "regenerate: python scripts/cube_revision.py layers ..."


@pytest.mark.parametrize(("stem", "cube", "engine", "label"), GENERATED)
def test_cube_provenance_is_rev3_with_the_generators_last_commit(stem, cube, engine, label):
    """No WAV needed. The commit check needs full history (CI: fetch-depth 0)."""
    doc = json.loads((LIBRARY / cube).read_text(encoding="utf-8"))
    assert doc["cube_revision"] == LIBRARY_REVISION and doc["engine"] == engine
    assert doc["provenance"]["layer_method"] == LAYER_METHOD
    assert doc["provenance"]["generator"] == GENERATOR_PATH
    assert doc["provenance"]["params"] == {k: getattr(LIBRARY_PARAMS, k) for k in doc["provenance"]["params"]}
    assert len(doc["source_sha256"]) == 64
    if not _full_history():
        pytest.skip("shallow clone or no git: cannot resolve git log -1 on the generator")
    assert doc["provenance"]["generator_commit"] == generator_commit(), f"{cube}: generator changed since; regenerate"


@pytest.mark.parametrize(("stem", "cube"), LIBRARY_CUBES)
def test_library_cube_inv_hdr_is_rms_over_peak_of_its_wav(stem, cube):
    """inv_hdr in every Library cube JSON is rms/peak of its WAV (cube_revision.measure)."""
    audio, sr = read_wav(_need_wav(stem))
    doc = json.loads((LIBRARY / cube).read_text(encoding="utf-8"))
    assert doc["inv_hdr"] == pytest.approx(measure(audio, sr).inv_hdr, abs=1e-6), cube
    assert doc["duration_s"] == pytest.approx(len(audio) / sr, abs=1e-9)


# --- manifest cube mirror (gen_audio.library_manifest) ---------------------
# Needs no WAV: it reads only the committed cube JSON and manifest.

def test_committed_manifest_mirrors_every_generated_cube(tmp_path):
    from gen_audio.library_manifest import sync_manifest

    copy = tmp_path / "manifest.json"
    copy.write_text((LIBRARY / "manifest.json").read_text(encoding="utf-8"), encoding="utf-8")
    assert sync_manifest(copy, LIBRARY) == [], "run: python scripts/cube_revision.py manifest"
    assert copy.read_bytes() == (LIBRARY / "manifest.json").read_bytes()


def test_manifest_sync_repairs_drift_wires_new_cubes_and_leaves_legacy_cubes_alone(tmp_path):
    from gen_audio.library_manifest import render, sync_manifest

    library = tmp_path / "library"
    library.mkdir()
    for path in LIBRARY.glob("*_cube3d.json"):
        (library / path.name).write_bytes(path.read_bytes())
    # A cube without cube_revision stands for the retired generator.
    legacy = json.loads((library / "library_kokoro_onnx_cube3d.json").read_text(encoding="utf-8"))
    del legacy["cube_revision"]
    (library / "library_kokoro_onnx_cube3d.json").write_text(json.dumps(legacy), encoding="utf-8")
    manifest = json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))
    by_id = {clip["id"]: clip for clip in manifest["clips"]}
    by_id["lib-misaki-kokoro"]["cube"]["cube_revision"] = 2
    by_id["lib-misaki-kokoro"]["cube"]["note"] = "Inverse-HDR bitdot cube rev 2"
    by_id["lib-kokoro"]["cube"] = None
    legacy_before = json.dumps(by_id["lib-kokoro-onnx"]["cube"])
    path = library / "manifest.json"
    path.write_text(render(manifest), encoding="utf-8")
    assert sync_manifest(path) == ["lib-kokoro", "lib-misaki-kokoro"]
    rewired = {clip["id"]: clip for clip in json.loads(path.read_text(encoding="utf-8"))["clips"]}["lib-kokoro"]["cube"]
    assert rewired == {clip["id"]: clip for clip in json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))["clips"]}["lib-kokoro"]["cube"]
    repaired = {clip["id"]: clip for clip in json.loads(path.read_text(encoding="utf-8"))["clips"]}
    misaki = repaired["lib-misaki-kokoro"]["cube"]
    doc = json.loads((LIBRARY / "library_genaid_full_misaki_kokoro_cube3d.json").read_text(encoding="utf-8"))
    assert misaki["cube_revision"] == doc["cube_revision"] == 3
    assert misaki["note"].startswith("Inverse-HDR cube rev 3 over the full 139.375 s WAV")
    assert "bitdot" not in misaki["note"]
    assert json.dumps(repaired["lib-kokoro-onnx"]["cube"]) == legacy_before


def test_generated_cube_titles_name_the_clip_not_the_point_style():
    for _stem, cube, _engine, label in GENERATED:
        doc = json.loads((LIBRARY / cube).read_text(encoding="utf-8"))
        assert doc["title"] == f"Inverse-HDR cube \u2014 {label}"
