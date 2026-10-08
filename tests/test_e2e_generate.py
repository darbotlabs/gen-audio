"""Headless generate: prompt, two personas, kokoro-onnx, duration, then analysis."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from gen_audio.assets import sha256_file


def test_e2e_kokoro_prompt_pipeline(tmp_path):
    model = os.environ.get("GEN_AUDIO_KOKORO_MODEL", "")
    voices = os.environ.get("GEN_AUDIO_KOKORO_VOICES", "")
    if not model or not voices or not Path(model).is_file() or not Path(voices).is_file():
        pytest.skip("kokoro-onnx weights are not configured")
    pytest.importorskip("kokoro_onnx")
    pytest.importorskip("faster_whisper")
    from gen_audio.cli.generate import main

    prompt = tmp_path / "scripts" / "prompt.txt"
    prompt.parent.mkdir()
    prompt.write_text("Hello from Alice. Frank heard the clip.\n", encoding="utf-8")
    code = main(
        [
            "--prompt-file",
            str(prompt),
            "--personas",
            "alice,frank",
            "--engine",
            "kokoro_onnx",
            "--duration-s",
            "6",
            "--out-dir",
            str(tmp_path),
        ]
    )
    assert code == 0
    # generate.main prints JSON; re-read the manifest files it wrote.
    fitted = tmp_path / "fitted.wav"
    assert fitted.is_file()
    spec = json.loads((tmp_path / "spectrogram.json").read_text(encoding="utf-8"))
    cube = json.loads((tmp_path / "cube.json").read_text(encoding="utf-8"))
    compare = json.loads((tmp_path / "compare.json").read_text(encoding="utf-8"))
    assert spec["duration_s"] == cube["duration_s"]
    assert compare["measuredHere"] is True
    assert isinstance(compare["wer"], float)
    assert compare["asr"] == "faster-whisper tiny.en"
    assert sha256_file(fitted)
    script = (tmp_path / "scripts" / "script.txt").read_text(encoding="utf-8")
    assert "podcast_script_sample" not in script
    cast = json.loads((tmp_path / "cast.json").read_text(encoding="utf-8"))
    assert cast["source"] == "persona-voice-map"
    assert cast["speakers"]["1"]["voice"] == "af_heart"
    assert cast["speakers"]["2"]["voice"] == "am_michael"
    assert (tmp_path / "clip.mp4").is_file()
