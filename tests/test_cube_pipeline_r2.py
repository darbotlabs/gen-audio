"""Layer method pipeline_r2 (gen_audio.cube_pipeline_r2): PR #4's formulas, comparison only.

Also guards the module split: pipeline_r2 lives in its own module and imports
the shared cube math from gen_audio.cube_layers rather than copying it.
"""

from __future__ import annotations

import ast
import hashlib
import json
from pathlib import Path

import numpy as np
import pytest

from gen_audio import cube_layers, cube_pipeline_r2
from gen_audio.audio_io import read_wav, write_wav
from gen_audio.cli.cube import LAYER_METHODS
from gen_audio.cli.cube import main as cube_main
from gen_audio.cube_layers import LAYER_NAMES
from gen_audio.cube_pipeline_r2 import compute_layers, cube_json_name, cube_png_name, grid, pipeline_r2_cube, stft
from gen_audio.cube_revision import measure
from test_cube_layers import LIBRARY, _need_wav, _speechish

R2_MODULE = Path(cube_pipeline_r2.__file__)

# (wav stem, engine, cube_revision) of the committed compare cubes (cube_revision.py layers --method pipeline_r2).
COMPARE = [
    ("bitdot_braille_vibevoice", "vibevoice", 2),
    ("genaid_full_misaki_kokoro", "misaki_kokoro", 2),
]


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


# ---- the split: shared math is imported, never copied ----


def test_r2_module_defines_no_copy_of_the_shared_helpers():
    """A copy of preview_points or stft_mag would fork the math silently."""
    tree = ast.parse(R2_MODULE.read_text(encoding="utf-8"))
    defined = {node.name for node in ast.walk(tree) if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))}
    assigned = {t.id for node in ast.walk(tree) if isinstance(node, ast.Assign) for t in node.targets if isinstance(t, ast.Name)}
    for name in ("preview_points", "stft_mag", "layers_to_points", "layer_score", "write_cube_png"):
        assert name not in defined | assigned, f"cube_pipeline_r2 defines its own {name}; import it from gen_audio.cube_layers"
    imported = {
        alias.name
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.module == "gen_audio.cube_layers"
        for alias in node.names
    }
    assert {"preview_points", "layers_to_points", "layer_score"} <= imported
    # The same objects, not look-alikes.
    assert cube_pipeline_r2.preview_points is cube_layers.preview_points
    assert cube_pipeline_r2.layers_to_points is cube_layers.layers_to_points
    assert cube_pipeline_r2.layer_score is cube_layers.layer_score


def test_cube_layers_has_no_pipeline_r2_code():
    """library_r3's module carries no r2 formula, so an r2 edit cannot move a Library cube's identity."""
    source = Path(cube_layers.__file__).read_text(encoding="utf-8")
    assert "pipeline_r2" not in source and "PIPELINE_R2" not in source


def test_r2_stft_is_pr4s_formula_not_stft_mag():
    """r2's STFT is PR #4's _stft_magnitude (zero pad, 1 + n // hop frames), a different formula from stft_mag."""
    audio = _speechish(1.0)
    r2 = stft(audio)
    assert r2.shape[1] == 1 + len(audio) // 256
    assert r2.shape != cube_layers.stft_mag(audio).shape or not np.array_equal(r2, cube_layers.stft_mag(audio))


def test_cli_dispatches_by_method_to_the_owning_module(tmp_path):
    assert set(LAYER_METHODS) == {"library_r3", "pipeline_r2"}
    wav = write_wav(tmp_path / "tone.wav", _speechish(2.0), 24000)
    out = tmp_path / "r2.json"
    assert cube_main(["layers", str(wav), str(out), "--stem", "tone", "--engine", "fixture", "--revision", "2", "--method", "pipeline_r2"]) == 0
    audio, sr = read_wav(wav)
    expected, _ = pipeline_r2_cube(audio, sr, stem="tone", engine="fixture", source_sha256=_sha256(wav), revision=2)
    assert json.loads(out.read_text(encoding="utf-8")) == json.loads(json.dumps(expected))


# ---- committed compare cubes ----


@pytest.mark.parametrize(("stem", "engine", "revision"), COMPARE)
def test_reproduces_shipped_compare_cube_byte_for_byte(stem, engine, revision):
    """The regen-diff gate for compare cubes: same WAV + module bytes -> same bytes."""
    wav = _need_wav(stem)
    audio, sr = read_wav(wav)
    doc, _ = pipeline_r2_cube(audio, sr, stem=stem, engine=engine, source_sha256=_sha256(wav), revision=revision)
    assert json.dumps(doc, ensure_ascii=False) == (LIBRARY / cube_json_name(stem)).read_text(encoding="utf-8"), (
        "regenerate: python scripts/cube_revision.py layers ... --method pipeline_r2"
    )


@pytest.mark.parametrize(("stem", "engine", "revision"), COMPARE)
def test_compare_cube_inv_hdr_is_rms_over_peak_of_its_wav(stem, engine, revision):
    audio, sr = read_wav(_need_wav(stem))
    doc = json.loads((LIBRARY / cube_json_name(stem)).read_text(encoding="utf-8"))
    assert doc["inv_hdr"] == pytest.approx(measure(audio, sr).inv_hdr, abs=1e-6)
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
    doc, _ = pipeline_r2_cube(audio, 24000, stem="tone", engine="fixture", source_sha256="a" * 64)
    _assert_pr4_stats(doc, "speechish")
    # PR #4 geometry: 1 + n // hop frames, 5 x 33 blocks, bin 33 * 256 / sr.
    assert doc["stft_frames"] == 1 + len(audio) // 256
    assert doc["downsample_sf_st"] == [5, 33] and doc["bin_seconds"] == 33 * 256 / 24000
    assert doc["inv_hdr"] == measure(audio, 24000).inv_hdr


@pytest.mark.parametrize("stem", ["genaid_full_misaki_kokoro", "bitdot_braille_vibevoice"])
def test_pipeline_r2_reproduces_pr4_stats_on_library_wavs(stem):
    wav = _need_wav(stem)
    audio, sr = read_wav(wav)
    doc, _ = pipeline_r2_cube(audio, sr, stem=stem, engine="e", source_sha256=_sha256(wav))
    _assert_pr4_stats(doc, stem)


def test_pipeline_r2_formulas():
    """signal = grid / max, tonality = (1 - frame flatness) where signal > 1e-4,
    confidence = grid / (grid + p70 + eps), quality = (signal + tonality) / 2."""
    audio = _speechish(2.0)
    mag = stft(audio)
    layers = compute_layers(grid(mag))
    nf, nt = mag.shape[0] // 5, mag.shape[1] // 33
    cells = mag[: nf * 5, : nt * 33].reshape(nf, 5, nt, 33).mean(axis=(1, 3))
    np.testing.assert_array_equal(layers["signal"], cells / cells.max())
    assert layers["signal"].max() == 1.0
    np.testing.assert_allclose(layers["confidence"], cells / (cells + np.percentile(cells, 70) + 1e-12), rtol=0, atol=0)
    np.testing.assert_allclose(layers["quality"], 0.5 * layers["signal"] + 0.5 * layers["tonality"], rtol=0, atol=0)
    # Tonality is one value per frame (column), masked where the signal is ~0.
    lit = layers["signal"] > 1e-4
    for column in range(nt):
        values = np.unique(layers["tonality"][lit[:, column], column])
        assert len(values) <= 1


def test_pipeline_r2_cube_guards_and_keys():
    audio = _speechish(1.5)
    with pytest.raises(ValueError, match="source_sha256"):
        pipeline_r2_cube(audio, 24000, stem="a", engine="e", source_sha256="")
    with pytest.raises(ValueError, match="source_sha256"):
        pipeline_r2_cube(audio, 24000, stem="a", engine="e", source_sha256="B" * 64)
    with pytest.raises(ValueError, match="too short"):
        pipeline_r2_cube(audio[:2000], 24000, stem="a", engine="e", source_sha256="b" * 64)
    doc, _ = pipeline_r2_cube(audio, 24000, stem="a", engine="e", source_sha256="b" * 64)
    assert doc["layer_method"] == cube_pipeline_r2.LAYER_METHOD == "pipeline_r2"
    assert doc["layer_method_label"] == cube_pipeline_r2.LAYER_METHOD_LABEL == "Pipeline formulas, rev 2 (PR #4)"
    assert doc["source_sha256"] == "b" * 64 and doc["compare_to"] == "/library/library_a_cube3d.json"
    assert doc["pngUrl"] == "/library/a_pipeline_r2_cube3d.png" and cube_png_name("a") == "a_pipeline_r2_cube3d.png"
    assert cube_json_name("a") == "library_a_pipeline_r2_cube3d.json"
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
