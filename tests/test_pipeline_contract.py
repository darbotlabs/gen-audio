"""Pipeline duration lock, WER math, and a local-ASR compare on a real tone."""

from __future__ import annotations

import numpy as np
import pytest

from gen_audio.asr_wer import AsrError, word_error_rate
from gen_audio.assets import asset_object, sha256_file
from gen_audio.audio_io import write_wav
from gen_audio.cube_revision import measure
from gen_audio.identity import mint
from gen_audio.pipeline import cube_document, cube_geometry, run_pipeline


def test_wer_counts_edits():
    assert word_error_rate("the cat sat", "the cat sat") == 0
    assert word_error_rate("the cat sat", "the dog sat") == pytest.approx(1 / 3)


def test_cube_duration_matches_the_buffer():
    rate = 24000
    audio = (0.2 * np.sin(2 * np.pi * 220 * np.arange(rate) / rate)).astype(np.float32)
    duration = audio.size / rate
    document = cube_document(audio, rate, duration_s=duration, engine="tone", derived_from=["asset-src"])
    assert document["duration_s"] == duration
    assert document["cube_revision"] == 2
    assert document["inv_hdr"] == measure(audio, rate).inv_hdr
    assert 0.0 < document["layer_score"] < 1.0
    assert document["cube_shape_f_t"][0] == (1024 // 2 + 1) // 5
    assert document["bin_seconds"] == pytest.approx(33 * 256 / rate)
    assert document["cube_covers_s"] == pytest.approx(document["cube_shape_f_t"][1] * document["bin_seconds"])
    assert document["cube_covers_s"] < duration
    names = {point["layer"] for point in document["points_preview"]}
    assert names == {"signal", "tonality", "confidence", "quality"}


def test_library_cube_geometry_matches_the_misaki_clip():
    geo = cube_geometry(3_345_000, 24_000)
    assert geo["stft_frames"] == 13067
    assert geo["freq_bins"] == 102
    assert geo["time_bins"] == 395
    assert geo["bin_seconds"] == pytest.approx(0.352)
    assert geo["cube_covers_s"] == pytest.approx(139.04)
    assert geo["cube_shape_f_t"] == [102, 395]


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
    import json

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
