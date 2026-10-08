"""Four-layer Library cube (gen_audio.cube_layers, cube_revision.py layers)."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np
import pytest

from gen_audio.audio_io import read_wav, write_wav
from gen_audio.cli.cube import main as cube_main
from gen_audio.cube_layers import (
    LAYER_METHOD_LABELS,
    LAYER_NAMES,
    CubeParams,
    compute_layers,
    cube_json_name,
    cube_png_name,
    library_cube,
    pipeline_r2_stft,
)
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


# (wav stem, cube JSON, engine, cube_revision, layer_method) for cubes this generator owns.
GENERATED = [
    ("bitdot_braille_vibevoice", "library_bitdot_braille_vibevoice_cube3d.json", "vibevoice", 2, "library_r3"),
    ("genaid_full_misaki_kokoro", "library_genaid_full_misaki_kokoro_cube3d.json", "misaki_kokoro", 3, "library_r3"),
    # Compare-mode cubes: PR #4's formulas on the same WAVs (cube_revision.py layers --method pipeline_r2).
    ("bitdot_braille_vibevoice", "library_bitdot_braille_vibevoice_pipeline_r2_cube3d.json", "vibevoice", 2, "pipeline_r2"),
    ("genaid_full_misaki_kokoro", "library_genaid_full_misaki_kokoro_pipeline_r2_cube3d.json", "misaki_kokoro", 2, "pipeline_r2"),
]
# Every Library cube JSON and its WAV (the two legacy cubes came from the retired generator).
LIBRARY_CUBES = [(stem, cube) for stem, cube, _engine, _rev, _method in GENERATED] + [
    ("library_kokoro_onnx", "library_kokoro_onnx_cube3d.json"),
    ("library_cube_explainer_kokoro_onnx", "library_cube_explainer_kokoro_onnx_cube3d.json"),
]


def _need_wav(stem: str) -> Path:
    wav = LIBRARY / f"{stem}.wav"
    if not wav.exists():
        pytest.skip(f"{wav.name} is gitignored and not staged here (present on SMAX and the box)")
    return wav


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


@pytest.mark.parametrize(("stem", "cube", "engine", "revision", "method"), GENERATED)
def test_reproduces_shipped_cube_byte_for_byte(stem, cube, engine, revision, method):
    wav = _need_wav(stem)
    audio, sr = read_wav(wav)
    sha = _sha256(wav) if method != "library_r3" else None
    doc, _ = library_cube(audio, sr, stem=stem, engine=engine, revision=revision, method=method, source_sha256=sha)
    assert cube == cube_json_name(stem, method)
    assert json.dumps(doc, ensure_ascii=False) == (LIBRARY / cube).read_text(encoding="utf-8")


@pytest.mark.parametrize(("stem", "cube"), LIBRARY_CUBES)
def test_library_cube_inv_hdr_is_rms_over_peak_of_its_wav(stem, cube):
    """inv_hdr in every Library cube JSON is rms/peak of its WAV (cube_revision.measure)."""
    audio, sr = read_wav(_need_wav(stem))
    doc = json.loads((LIBRARY / cube).read_text(encoding="utf-8"))
    assert doc["inv_hdr"] == pytest.approx(measure(audio, sr).inv_hdr, abs=1e-6), cube
    assert doc["duration_s"] == pytest.approx(len(audio) / sr, abs=1e-9)


# ---- layer_method pipeline_r2: PR #4's formulas as a comparison variant ----
#
# Golden layer stats from PR #4's own code: darbotlabs/gen-audio@204d0de,
# src/gen_audio/pipeline.py cube_document(audio, sr, ...)["layers"], run on
# the same samples (box, 2026-10-07). [mean, std, p50, p90, active_frac].
PR4_204D0DE_STATS = {
    "speechish": (
        [102, 8],
        {
            "signal": [0.07769936901840238, 0.14810095425377648, 0.041461231175063576, 0.047991161636554605, 0.058823529411764705],
            "tonality": [0.36317973976193213, 0.0023611153035942673, 0.36355081156938784, 0.3659553289829641, 1.0],
            "confidence": [0.5194238779828132, 0.11192286081505017, 0.4888717837944977, 0.5254118540396466, 1.0],
            "quality": [0.2204395543901673, 0.07406380961061054, 0.20243245185227643, 0.20597433867168702, 0.8762254901960784],
        },
    ),
    "genaid_full_misaki_kokoro": (
        [102, 395],
        {
            "signal": [0.01915429562396228, 0.04637428166034303, 0.006928069032084228, 0.03700582743614974, 0.014743112434847357],
            "tonality": [0.49622658792574514, 0.22886384344584249, 0.5232336820977965, 0.7707203559615271, 0.8992057582526681],
            "confidence": [0.35640301962363075, 0.25952038028872904, 0.33147399189118054, 0.7259097598796155, 0.65951849094068],
            "quality": [0.2576904417748537, 0.11823910305681262, 0.26980911187877454, 0.3936589317344863, 0.6952097294614048],
        },
    ),
    "bitdot_braille_vibevoice": (
        [102, 701],
        {
            "signal": [0.02465064335400211, 0.0634165912020089, 0.0024562200045814843, 0.06294800628446252, 0.026558697658806748],
            "tonality": [0.5794847820936125, 0.4385635089161264, 0.9022570494706805, 0.9462422735371282, 0.6398562277978238],
            "confidence": [0.29731470571381385, 0.32545877830136405, 0.1518264360748096, 0.8210295239642693, 0.4671337864675114],
            "quality": [0.30206771272380734, 0.2310318916501738, 0.45854856963934976, 0.49756936469548024, 0.639073032922156],
        },
    ),
}


def _assert_pr4_stats(doc: dict, key: str) -> None:
    shape, golden = PR4_204D0DE_STATS[key]
    assert doc["cube_shape_f_t"] == shape
    for name in LAYER_NAMES:
        got = doc["layers"][name]
        assert got["shape"] == shape
        assert [got[k] for k in ("mean", "std", "p50", "p90", "active_frac")] == pytest.approx(golden[name], rel=0, abs=1e-12), name


def test_pipeline_r2_reproduces_pr4_cube_document_layer_stats():
    """WAV-free: the port matches PR #4's cube_document on the same samples."""
    audio = _speechish()
    doc, _ = library_cube(audio, 24000, stem="tone", engine="fixture", revision=2, method="pipeline_r2", source_sha256="a" * 64)
    _assert_pr4_stats(doc, "speechish")
    # PR #4 geometry: 1 + n // hop frames, 5 x 33 blocks, bin 33 * 256 / sr.
    assert doc["stft_frames"] == 1 + len(audio) // 256
    assert doc["downsample_sf_st"] == [5, 33] and doc["bin_seconds"] == 33 * 256 / 24000
    assert doc["inv_hdr"] == measure(audio, 24000).inv_hdr


@pytest.mark.parametrize("stem", ["genaid_full_misaki_kokoro", "bitdot_braille_vibevoice"])
def test_pipeline_r2_reproduces_pr4_stats_on_library_wavs(stem):
    wav = _need_wav(stem)
    audio, sr = read_wav(wav)
    doc, _ = library_cube(audio, sr, stem=stem, engine="e", revision=2, method="pipeline_r2", source_sha256=_sha256(wav))
    _assert_pr4_stats(doc, stem)


def test_pipeline_r2_formulas():
    """signal = grid / max, tonality = (1 - frame flatness) where signal > 1e-4,
    confidence = grid / (grid + p70 + eps), quality = (signal + tonality) / 2."""
    audio = _speechish(2.0)
    mag = pipeline_r2_stft(audio)
    layers = compute_layers(mag, 24000, 1024, method="pipeline_r2")
    nf, nt = mag.shape[0] // 5, mag.shape[1] // 33
    grid = mag[: nf * 5, : nt * 33].reshape(nf, 5, nt, 33).mean(axis=(1, 3))
    np.testing.assert_array_equal(layers["signal"], grid / grid.max())
    assert layers["signal"].max() == 1.0
    np.testing.assert_allclose(layers["confidence"], grid / (grid + np.percentile(grid, 70) + 1e-12), rtol=0, atol=0)
    np.testing.assert_allclose(layers["quality"], 0.5 * layers["signal"] + 0.5 * layers["tonality"], rtol=0, atol=0)
    # Tonality is one value per frame (column), masked where the signal is ~0.
    lit = layers["signal"] > 1e-4
    for column in range(nt):
        values = np.unique(layers["tonality"][lit[:, column], column])
        assert len(values) <= 1


def test_layer_method_default_and_guards():
    audio = _speechish(1.5)
    default, _ = library_cube(audio, 24000, stem="a", engine="e")
    explicit, _ = library_cube(audio, 24000, stem="a", engine="e", method="library_r3")
    assert json.dumps(default) == json.dumps(explicit)
    # library_r3 JSON keeps its shipped key set (no layer_method / source_sha256).
    assert "layer_method" not in default and "source_sha256" not in default and "layer_score" in default
    with pytest.raises(ValueError, match="unknown layer_method"):
        library_cube(audio, 24000, stem="a", engine="e", method="pipeline_r3")
    with pytest.raises(ValueError, match="source_sha256"):
        library_cube(audio, 24000, stem="a", engine="e", method="pipeline_r2")
    with pytest.raises(ValueError, match="too short"):
        library_cube(audio[:2000], 24000, stem="a", engine="e", method="pipeline_r2", source_sha256="b" * 64)
    doc, _ = library_cube(audio, 24000, stem="a", engine="e", revision=2, method="pipeline_r2", source_sha256="b" * 64)
    assert doc["layer_method"] == "pipeline_r2"
    assert doc["layer_method_label"] == LAYER_METHOD_LABELS["pipeline_r2"] == "Pipeline formulas, rev 2 (PR #4)"
    assert doc["source_sha256"] == "b" * 64 and doc["compare_to"] == "/library/library_a_cube3d.json"
    assert doc["pngUrl"] == "/library/a_pipeline_r2_cube3d.png" and cube_png_name("a", "pipeline_r2") == "a_pipeline_r2_cube3d.png"
    assert doc["layer_method_source"].startswith("darbotlabs/gen-audio@204d0de")


def test_layers_cli_method_flag_records_method_and_wav_sha(tmp_path):
    wav = write_wav(tmp_path / "tone.wav", _speechish(2.0), 24000)
    out, png = tmp_path / "cube.json", tmp_path / "cube.png"
    argv = ["layers", str(wav), str(out), "--stem", "tone", "--engine", "fixture", "--revision", "2", "--method", "pipeline_r2", "--png", str(png)]
    assert cube_main(argv) == 0
    doc = json.loads(out.read_text(encoding="utf-8"))
    assert doc["layer_method"] == "pipeline_r2" and doc["cube_revision"] == 2
    assert doc["source_sha256"] == _sha256(wav)
    assert png.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"
    with pytest.raises(SystemExit):
        cube_main(["layers", str(wav), str(out), "--stem", "tone", "--engine", "fixture", "--method", "nope"])


def test_compare_cubes_are_declared_hashed_and_enveloped():
    """manifest cube.compare[] -> committed JSON/PNG -> a real cube_ihdr envelope (B2 roles) of the same WAV."""
    manifest = json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))
    assets = json.loads((LIBRARY / "assets.json").read_text(encoding="utf-8"))["assets"]
    declared = 0
    for clip in manifest["clips"]:
        for entry in (clip.get("cube") or {}).get("compare", []):
            declared += 1
            json_name = entry["jsonUrl"].removeprefix("/library/")
            png_name = entry["pngUrl"].removeprefix("/library/")
            doc = json.loads((LIBRARY / json_name).read_text(encoding="utf-8"))
            assert doc["layer_method"] == entry["layer_method"] == "pipeline_r2"
            assert (LIBRARY / png_name).read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"
            [envelope] = [a for a in assets if a.get("legacy_id") == f"{clip['id']}.cube.pipeline_r2"]
            assert envelope["kind"] == "cube_ihdr" and envelope["honesty"]["fixture"] is False
            assert "library_cube" in envelope["honesty"]["claims"]
            media = {m["role"]: m for m in envelope["media"]}
            assert set(media) == {"cube_json", "cube_png"}
            assert media["cube_json"]["sha256"] == _sha256(LIBRARY / json_name)
            assert media["cube_png"]["sha256"] == _sha256(LIBRARY / png_name)
            [clip_env] = [a for a in assets if a["kind"] == "audio_clip" and a.get("legacy_id") == clip["id"]]
            wav_sha = next(m["sha256"] for m in clip_env["media"] if m["role"] == "wav")
            assert envelope["src"] == [clip_env["uid"]]
            assert envelope["fields"]["source_sha256"] == doc["source_sha256"] == wav_sha
            assert envelope["provenance"]["params"]["layer_method"] == "pipeline_r2"
            [own] = [a for a in assets if a.get("legacy_id") == f"{clip['id']}.cube"]
            assert envelope["provenance"]["params"]["compare_to"] == own["uid"]
    assert declared == 2
