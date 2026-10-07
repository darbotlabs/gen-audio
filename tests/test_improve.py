from __future__ import annotations

import numpy as np

from gen_audio.audio_io import read_wav, write_wav
from gen_audio.improve import (
    PEAK_TARGET,
    PUBLISH_SAMPLE_RATE,
    highpass,
    improve,
    peak_normalize,
    resample_audio,
    trim_silence,
)


def _tone(seconds: float, freq: float, sample_rate: int, amplitude: float = 0.5) -> np.ndarray:
    count = int(round(seconds * sample_rate))
    time = np.arange(count) / sample_rate
    return (amplitude * np.sin(2 * np.pi * freq * time)).astype(np.float32)


def test_highpass_attenuates_30_hz():
    sample_rate = 16_000
    rumble = _tone(1.0, 30.0, sample_rate, amplitude=0.8)
    before = float(np.sqrt(np.mean(rumble.astype(np.float64) ** 2)))
    after = highpass(rumble, sample_rate, cutoff_hz=70.0)
    after_rms = float(np.sqrt(np.mean(after**2)))
    assert after_rms < before * 0.5


def test_peak_normalize_hits_target():
    audio = _tone(0.25, 440.0, 16_000, amplitude=0.2)
    normalized = peak_normalize(audio, PEAK_TARGET)
    assert np.isclose(float(np.max(np.abs(normalized))), PEAK_TARGET, atol=1e-6)


def test_trim_keeps_internal_pause_and_drops_edges():
    sample_rate = 16_000
    tone = _tone(0.2, 440.0, sample_rate, amplitude=0.5)
    pause = np.zeros(int(0.2 * sample_rate), dtype=np.float32)
    edge = np.zeros(int(0.4 * sample_rate), dtype=np.float32)
    audio = np.concatenate([edge, tone, pause, tone, edge])
    trimmed = trim_silence(audio, sample_rate)
    trimmed_seconds = trimmed.size / sample_rate
    assert 0.55 < trimmed_seconds < 0.85
    middle = trimmed[trimmed.size // 2 - 200 : trimmed.size // 2 + 200]
    assert float(np.max(np.abs(middle))) < 1e-6


def test_improve_publishes_24k_and_shortens_padded_take(tmp_path):
    sample_rate = 16_000
    tone = _tone(0.4, 440.0, sample_rate, amplitude=0.5)
    audio = np.concatenate(
        [
            np.zeros(int(0.5 * sample_rate), dtype=np.float32),
            tone,
            np.zeros(int(0.5 * sample_rate), dtype=np.float32),
        ]
    )
    result = improve(audio, sample_rate)
    assert result.sample_rate == PUBLISH_SAMPLE_RATE
    assert result.output_seconds < 0.7
    assert result.output_seconds > 0.35
    assert 0.5 < result.peak <= 1.0
    assert np.isfinite(result.audio).all()

    destination = write_wav(tmp_path / "out.wav", result.audio, result.sample_rate)
    loaded, loaded_rate = read_wav(destination)
    assert loaded_rate == PUBLISH_SAMPLE_RATE
    assert loaded.size == result.audio.size


def test_resample_16k_to_24k_length_ratio():
    audio = _tone(0.5, 440.0, 16_000)
    converted = resample_audio(audio, 16_000, 24_000)
    expected = audio.size * 3 / 2
    assert abs(converted.size - expected) <= 16


def test_integer_pcm_is_scaled_before_normalize():
    pcm = (np.array([0, 16384, -32768, 1000], dtype=np.int16))
    normalized = peak_normalize(pcm, PEAK_TARGET)
    assert np.isclose(float(np.max(np.abs(normalized))), PEAK_TARGET, atol=1e-5)


def test_all_silence_is_refused():
    import pytest

    with pytest.raises(ValueError, match="silence"):
        improve(np.zeros(16_000, dtype=np.float32), 16_000)
