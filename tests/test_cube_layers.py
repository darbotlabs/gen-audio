"""Four-layer Library cube (gen_audio.cube_layers, cube_revision.py layers)."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest

from gen_audio.audio_io import read_wav, write_wav
from gen_audio.cli.cube import main as cube_main
from gen_audio.cube_layers import LAYER_NAMES, CubeParams, library_cube
from gen_audio.cube_revision import measure

LIBRARY = Path(__file__).resolve().parents[1] / "apps" / "desktop" / "public" / "library"


def _speechish(seconds: float = 3.0, sr: int = 24000) -> np.ndarray:
    t = np.arange(int(seconds * sr)) / sr
    env = 0.5 + 0.5 * np.sin(2 * np.pi * 3 * t)
    tone = sum(np.sin(2 * np.pi * f * t) / k for k, f in enumerate((220, 440, 880, 1760, 3520), start=1))
    noise = np.random.default_rng(0).normal(0, 0.01, len(t))
    return (0.3 * env * tone / 2.3 + noise).astype(np.float32)


def test_library_cube_shape_and_scrub_mapping():
    audio, sr = _speechish(), 24000
    doc, _cloud = library_cube(audio, sr, stem="tone", engine="fixture", revision=3)
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
    first, _ = library_cube(audio, 24000, stem="a", engine="e")
    second, _ = library_cube(audio.copy(), 24000, stem="a", engine="e")
    assert json.dumps(first) == json.dumps(second)


def test_library_cube_rejects_empty_audio():
    with pytest.raises(ValueError):
        library_cube(np.zeros(0, np.float32), 24000, stem="x", engine="e")


def test_layers_cli_writes_json_and_png(tmp_path):
    wav = write_wav(tmp_path / "tone.wav", _speechish(2.0), 24000)
    out, png = tmp_path / "cube.json", tmp_path / "cube.png"
    assert cube_main(["layers", str(wav), str(out), "--stem", "tone", "--engine", "fixture", "--png", str(png)]) == 0
    doc = json.loads(out.read_text(encoding="utf-8"))
    assert doc["engine"] == "fixture" and doc["cube_revision"] == 1
    assert png.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"


# (wav stem, cube JSON, engine, cube_revision) for cubes this generator owns.
GENERATED = [
    ("bitdot_braille_vibevoice", "library_bitdot_braille_vibevoice_cube3d.json", "vibevoice", 2),
    ("genaid_full_misaki_kokoro", "library_genaid_full_misaki_kokoro_cube3d.json", "misaki_kokoro", 3),
]
# Every Library cube JSON and its WAV (the two legacy cubes came from the retired generator).
LIBRARY_CUBES = [(stem, cube) for stem, cube, _engine, _rev in GENERATED] + [
    ("library_kokoro_onnx", "library_kokoro_onnx_cube3d.json"),
    ("library_cube_explainer_kokoro_onnx", "library_cube_explainer_kokoro_onnx_cube3d.json"),
]


def _need_wav(stem: str) -> Path:
    wav = LIBRARY / f"{stem}.wav"
    if not wav.exists():
        pytest.skip(f"{wav.name} is gitignored and not staged here (present on SMAX and the box)")
    return wav


@pytest.mark.parametrize(("stem", "cube", "engine", "revision"), GENERATED)
def test_reproduces_shipped_cube_byte_for_byte(stem, cube, engine, revision):
    audio, sr = read_wav(_need_wav(stem))
    doc, _ = library_cube(audio, sr, stem=stem, engine=engine, revision=revision)
    assert json.dumps(doc, ensure_ascii=False) == (LIBRARY / cube).read_text(encoding="utf-8")


@pytest.mark.parametrize(("stem", "cube"), LIBRARY_CUBES)
def test_library_cube_inv_hdr_is_rms_over_peak_of_its_wav(stem, cube):
    """inv_hdr in every Library cube JSON is rms/peak of its WAV (cube_revision.measure)."""
    audio, sr = read_wav(_need_wav(stem))
    doc = json.loads((LIBRARY / cube).read_text(encoding="utf-8"))
    assert doc["inv_hdr"] == pytest.approx(measure(audio, sr).inv_hdr, abs=1e-6), cube
    assert doc["duration_s"] == pytest.approx(len(audio) / sr, abs=1e-9)
