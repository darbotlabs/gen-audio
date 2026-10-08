"""Pipeline duration lock, WER math, and a local-ASR compare on a real tone."""

from __future__ import annotations

import json

import numpy as np
import pytest

from gen_audio.asr_wer import AsrError, word_error_rate
from gen_audio.assets import asset_object, sha256_file
from gen_audio.audio_io import write_wav
from gen_audio.cube_layers import library_cube
from gen_audio.cube_revision import measure
from gen_audio.identity import mint
from gen_audio.pipeline import cube_document, run_pipeline


def test_wer_counts_edits():
    assert word_error_rate("the cat sat", "the cat sat") == 0
    assert word_error_rate("the cat sat", "the dog sat") == pytest.approx(1 / 3)


def test_cube_duration_matches_the_buffer():
    rate = 24000
    audio = (0.2 * np.sin(2 * np.pi * 220 * np.arange(rate) / rate)).astype(np.float32)
    duration = audio.size / rate
    document = cube_document(audio, rate, duration_s=duration, engine="tone", derived_from=["asset-src"])
    assert document["duration_s"] == duration
    assert document["cube_revision"] == 3
    assert "layer_method" not in document
    assert document["provenance"]["module"] == "gen_audio.cube_layers"
    assert document["provenance"]["generator"] == "gen_audio.cube_layers.library_cube"
    assert document["provenance"]["layer_method"] == "library_r3"
    assert document["provenance"]["params"]["n_fft"] == 1024
    assert document["inv_hdr"] == measure(audio, rate).inv_hdr
    assert 0.0 < document["layer_score"] < 1.0
    assert document["cube_shape_f_t"][0] == (1024 // 2 + 1) // 5
    # 1 s at 24 kHz is 94 STFT frames, so the shared downsample uses stride 1.
    assert document["downsample_sf_st"] == [5, 1]
    assert document["cube_shape_f_t"][1] == 94
    assert document["bin_seconds"] == pytest.approx(256 / rate)
    assert document["cube_covers_s"] == pytest.approx(document["cube_shape_f_t"][1] * document["bin_seconds"])
    assert document["speakers"] == "unresolved"
    assert "segments" not in document
    assert "speaker_idx" not in document
    names = {point["layer"] for point in document["points_preview"]}
    assert names == {"signal", "tonality", "confidence", "quality"}


def test_generated_cube_layers_match_library_cube():
    rate = 24000
    rng = np.random.default_rng(1)
    samples = np.arange(rate * 2)
    audio = (0.2 * np.sin(2 * np.pi * 220 * samples / rate)).astype(np.float64)
    audio += rng.normal(0, 0.01, audio.size)
    duration = audio.size / rate
    generated = cube_document(audio, rate, duration_s=duration, engine="tone", derived_from=["asset-src"])
    library, _cloud = library_cube(audio, rate, stem="tone", engine="tone", revision=3)
    assert generated["layers"] == library["layers"]
    assert generated["layer_score"] == library["layer_score"]
    assert generated["inv_hdr"] == library["inv_hdr"]
    assert generated["points_preview"] == library["points_preview"]
    assert generated["cube_shape_f_t"] == library["cube_shape_f_t"]
    assert generated["downsample_sf_st"] == library["downsample_sf_st"]
    assert generated["bin_seconds"] == library["bin_seconds"]
    assert generated["cube_covers_s"] == library["cube_covers_s"]
    assert generated["stft_frames"] == library["stft_frames"]
    assert generated["n_points"] == library["n_points"]
    assert generated["n_points_source"] == library["n_points_source"]
    assert generated["n_points_source_per_layer"] == library["n_points_source_per_layer"]


def test_library_cube_geometry_matches_the_misaki_clip():
    doc, _cloud = library_cube(np.zeros(3_345_000, dtype=np.float64), 24_000, stem="misaki", engine="misaki_kokoro", revision=3)
    assert doc["stft_frames"] == 13067
    assert doc["cube_shape_f_t"] == [102, 395]
    assert doc["bin_seconds"] == pytest.approx(0.352)
    assert doc["cube_covers_s"] == pytest.approx(139.04)


def test_media_a_uid_matches_the_v1_golden_vector():
    uid = mint(
        "audio_clip",
        {"duration_ms": 1000, "engine": "kokoro_onnx", "sample_rate_hz": 24000},
        [{"role": "wav", "sha256": "61b8ca26b3f4759ce643738e2afbc80f7bf2a5eb554c5fedbfbdc2d9bfad3d38"}],
        [],
    )
    assert uid == "ga:audio_clip:n2v3xarf6qy5l77hoje3wnf76u"


def test_pipeline_on_a_tone_locks_duration_and_measures_wer(tmp_path):
    pytest.importorskip("faster_whisper")
    rate = 24000
    seconds = 1.0
    audio = (0.4 * np.sin(2 * np.pi * 180 * np.arange(int(rate * seconds)) / rate)).astype(np.float32)
    raw = tmp_path / "raw.wav"
    write_wav(raw, audio, rate, subtype="FLOAT")
    script = tmp_path / "script.txt"
    script.write_text("Speaker 1: hello\n", encoding="utf-8")
    script_asset = asset_object(script, kind="script", derived_from=[])
    try:
        manifest = run_pipeline(
            raw,
            out_dir=tmp_path,
            engine="tone",
            reference_text="hello",
            duration_target_s=1.2,
            script_asset=script_asset,
        )
    except AsrError as exc:
        pytest.skip(str(exc))
    duration = manifest["duration_s"]
    assert abs(duration - 1.2) < 0.05
    spec = (tmp_path / "spectrogram.json").read_text(encoding="utf-8")
    cube = (tmp_path / "cube.json").read_text(encoding="utf-8")
    assert f'"duration_s": {duration}' in spec or str(duration) in spec
    spec_doc = json.loads(spec)
    cube_doc = json.loads(cube)
    assert spec_doc["duration_s"] == duration
    assert cube_doc["duration_s"] == duration
    assert manifest["durationHonoured"] is False
    assert manifest["speech_s"] == pytest.approx(1.0, abs=0.05)
    assert manifest["compare"]["measuredHere"] is True
    assert "wer" in manifest["compare"]
    assert manifest["compare"]["asr"] == "faster-whisper tiny.en"
    assert manifest["sha256"] == sha256_file(tmp_path / "fitted.wav")
    assert manifest["assets"]["wav"]["derived_from"] == [manifest["assets"]["raw"]["uid"]]
    assert (tmp_path / "clip.mp4").is_file()


def test_asr_failure_keeps_the_synth(tmp_path, monkeypatch):
    rate = 24000
    audio = (0.4 * np.sin(2 * np.pi * 180 * np.arange(rate) / rate)).astype(np.float32)
    raw = tmp_path / "raw.wav"
    write_wav(raw, audio, rate, subtype="FLOAT")
    script = tmp_path / "script.txt"
    script.write_text("Speaker 1: hello\n", encoding="utf-8")
    script_asset = asset_object(script, kind="script", derived_from=[])

    def boom(_path):
        raise AsrError("faster-whisper is not installed")

    monkeypatch.setattr("gen_audio.pipeline.transcribe", boom)
    manifest = run_pipeline(
        raw,
        out_dir=tmp_path,
        engine="tone",
        reference_text="hello",
        duration_target_s=1.2,
        script_asset=script_asset,
    )
    assert manifest["synthesizedSpeech"] is True
    assert manifest["assets"]["wav"]["uid"].startswith("ga:audio_clip:")
    assert (tmp_path / "fitted.wav").is_file()
    assert manifest["compare"]["measuredHere"] is False
    assert "unavailable" in manifest["compare"]
    assert manifest["durationHonoured"] is False
    cube = manifest["assets"]["cube"]
    png = manifest["assets"]["cubePng"]
    assert cube["uid"].startswith("ga:cube_ihdr:")
    assert png["uid"] == cube["uid"]
    assert {item["role"] for item in cube["media"]} == {"cube_json", "cube_png"}
    assert cube["fields"]["cube_revision"] == 3
    again = mint(
        "cube_ihdr",
        cube["fields"],
        cube["media"],
        [item for item in cube["derived_from"] if str(item).startswith("ga:")],
    )
    assert again == cube["uid"]
    assert json.loads((tmp_path / "cube.json").read_text(encoding="utf-8"))["cube_revision"] == 3
