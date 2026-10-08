"""Pipeline duration lock, WER math, and a local-ASR compare on a real tone."""

from __future__ import annotations

import numpy as np
import pytest

from gen_audio.asr_wer import AsrError, word_error_rate
from gen_audio.assets import asset_object, sha256_file
from gen_audio.audio_io import write_wav
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
    assert document["cube_covers_s"] == duration
    names = {point["layer"] for point in document["points_preview"]}
    assert names == {"signal", "tonality", "confidence", "quality"}


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
    assert manifest["compare"]["measuredHere"] is True
    assert "wer" in manifest["compare"]
    assert manifest["compare"]["asr"] == "faster-whisper tiny.en"
    assert manifest["sha256"] == sha256_file(tmp_path / "fitted.wav")
    assert manifest["assets"]["wav"]["derived_from"] == [manifest["assets"]["raw"]["uid"]]
    assert (tmp_path / "clip.mp4").is_file()
