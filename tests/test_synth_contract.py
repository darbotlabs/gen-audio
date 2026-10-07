from __future__ import annotations

from pathlib import Path

import pytest

from gen_audio.cast import CastMap, Turn, VoiceAssignment
from gen_audio.synth_kokoro_onnx import KokoroOnnxSynthesizer, resolve_model_paths


def test_missing_env_explains_that_weights_are_not_downloaded(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.delenv("GEN_AUDIO_KOKORO_MODEL", raising=False)
    monkeypatch.delenv("GEN_AUDIO_KOKORO_VOICES", raising=False)
    with pytest.raises(RuntimeError, match="does not download"):
        resolve_model_paths()


def test_env_paths_are_used(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    model = tmp_path / "model.onnx"
    voices = tmp_path / "voices.bin"
    monkeypatch.setenv("GEN_AUDIO_KOKORO_MODEL", str(model))
    monkeypatch.setenv("GEN_AUDIO_KOKORO_VOICES", str(voices))
    resolved_model, resolved_voices = resolve_model_paths()
    assert resolved_model == model
    assert resolved_voices == voices


def test_missing_model_file_raises_before_import(tmp_path: Path):
    with pytest.raises(FileNotFoundError, match="model file"):
        KokoroOnnxSynthesizer(tmp_path / "missing.onnx", tmp_path / "missing.bin")


def test_non_kokoro_cast_is_rejected():
    synth = KokoroOnnxSynthesizer.__new__(KokoroOnnxSynthesizer)
    cast = CastMap(
        engine="vibevoice",
        default_lang="en-us",
        default_speed=1.0,
        speakers={
            "1": VoiceAssignment("1", "Alice", "af_heart", "en-us", 1.0),
        },
    )
    turns = [Turn("1", "Hello.", None, 1)]
    with pytest.raises(ValueError, match="kokoro_onnx"):
        synth.synthesize_turns(turns, cast)
