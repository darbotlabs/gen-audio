"""Engine probes name the missing piece and do not write audio."""

from gen_audio.adapters import EngineRefusal, probe, synthesize
from gen_audio.cast import CastMap, Turn


def test_kokoro_refusal_names_missing_model_env(monkeypatch):
    monkeypatch.delenv("GEN_AUDIO_KOKORO_MODEL", raising=False)
    monkeypatch.delenv("GEN_AUDIO_KOKORO_VOICES", raising=False)
    found = probe("kokoro_onnx")
    assert found.available is False
    blob = " ".join(found.missing)
    assert "GEN_AUDIO_KOKORO_MODEL is unset" in blob
    assert "GEN_AUDIO_KOKORO_VOICES is unset" in blob


def test_vibevoice_default_device_does_not_require_cuda(monkeypatch):
    monkeypatch.delenv("GEN_AUDIO_VIBEVOICE_MODEL", raising=False)
    monkeypatch.delenv("GEN_AUDIO_VIBEVOICE_DEVICE", raising=False)
    found = probe("vibevoice")
    assert found.available is False
    blob = " ".join(found.missing)
    assert "GEN_AUDIO_VIBEVOICE_MODEL is unset" in blob
    assert "CUDA" not in blob


def test_vibevoice_dtype_follows_the_device(monkeypatch):
    from gen_audio.adapters import _vibevoice_dtype_name

    monkeypatch.delenv("GEN_AUDIO_VIBEVOICE_DTYPE", raising=False)
    assert _vibevoice_dtype_name("cpu") == "float32"
    assert _vibevoice_dtype_name("cuda") == "float16"
    assert _vibevoice_dtype_name("cuda:0") == "float16"
    monkeypatch.setenv("GEN_AUDIO_VIBEVOICE_DTYPE", "float16")
    assert _vibevoice_dtype_name("cpu") == "float16"
    monkeypatch.setenv("GEN_AUDIO_VIBEVOICE_DTYPE", "fp32")
    assert _vibevoice_dtype_name("cuda:0") == "fp32"


def test_vibevoice_cuda_device_refuses_when_cuda_is_absent(monkeypatch):
    monkeypatch.setenv("GEN_AUDIO_VIBEVOICE_DEVICE", "cuda")
    monkeypatch.setenv("GEN_AUDIO_VIBEVOICE_MODEL", "/tmp/gen-audio-vibevoice-missing")
    monkeypatch.setattr(
        "gen_audio.adapters._module_present",
        lambda name: name in {"torch", "vibevoice"},
    )
    monkeypatch.setattr("gen_audio.adapters._cuda_available", lambda: False)
    found = probe("vibevoice")
    blob = " ".join(found.missing)
    assert "GEN_AUDIO_VIBEVOICE_DEVICE is cuda and CUDA is not available" in blob


def test_magpie_refusal_names_runtime_and_weights(monkeypatch):
    monkeypatch.delenv("GEN_AUDIO_MAGPIE_MODEL", raising=False)
    found = probe("magpie")
    assert found.available is False
    blob = " ".join(found.missing)
    assert "GEN_AUDIO_MAGPIE_MODEL is unset" in blob
    assert "Magpie" in blob or "magpie" in blob or "CUDA" in blob or "PyTorch" in blob


def test_synthesize_does_not_write_audio_when_refused(tmp_path, monkeypatch):
    monkeypatch.delenv("GEN_AUDIO_KOKORO_MODEL", raising=False)
    monkeypatch.delenv("GEN_AUDIO_KOKORO_VOICES", raising=False)
    output = tmp_path / "nope.wav"
    turns = [Turn(speaker_id="1", text="Hello.", script_name="Alice", line=1)]
    cast = CastMap(engine="kokoro_onnx", default_lang="en-us", default_speed=1.0, speakers={})
    try:
        synthesize("kokoro_onnx", turns, cast, output)
    except EngineRefusal as exc:
        assert "No speech was invented" in str(exc)
    else:
        raise AssertionError("synth wrote a refusal path as success")
    assert not output.exists()
