"""Fixed publish chain: trim silence, high-pass, peak-normalize, resample.

The stage order is part of the contract:

1. Trim leading and trailing silence (internal pauses stay).
2. Mild zero-phase high-pass around 70 Hz.
3. Peak-normalize to about 0.89 linear (about -1.0 dBFS).
4. Resample to 24 kHz.

Resampling is last, so the stored peak can sit slightly off 0.89. PCM_16
quantization can move it again. See ``docs/ARCHITECTURE.md``.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

import numpy as np
from scipy import signal

from gen_audio.audio_io import as_mono

PUBLISH_SAMPLE_RATE = 24_000
HIGHPASS_HZ = 70.0
PEAK_TARGET = 0.89
TRIM_THRESHOLD_DB = -40.0
TRIM_PAD_MS = 30.0
HIGHPASS_ORDER = 2


@dataclass(frozen=True)
class ImproveResult:
    """Mono float32 audio at the publish sample rate, plus stage notes."""

    audio: np.ndarray
    sample_rate: int
    input_sample_rate: int
    input_seconds: float
    output_seconds: float
    peak: float
    trim_start: int = 0
    trimmed_samples: int = 0
    stages: dict[str, float] = field(default_factory=dict)


def trim_bounds(
    audio: np.ndarray,
    sample_rate: int,
    *,
    threshold_db: float = TRIM_THRESHOLD_DB,
    pad_ms: float = TRIM_PAD_MS,
) -> tuple[int, int]:
    """Sample span kept by :func:`trim_silence`, as ``[start, end)``.

    ``start`` is the first sample that survives the cut, so a later stage can
    remap speaker cursors by subtracting it. An all-silent buffer returns
    ``(0, 0)``.
    """
    values = as_mono(audio)
    if values.size == 0:
        return 0, 0
    peak = float(np.max(np.abs(values)))
    if peak == 0.0:
        return 0, 0
    threshold = peak * (10.0 ** (float(threshold_db) / 20.0))
    loud = np.abs(values) >= threshold
    if not np.any(loud):
        return 0, 0
    indices = np.flatnonzero(loud)
    pad = int(round(sample_rate * (float(pad_ms) / 1000.0)))
    start = max(0, int(indices[0]) - pad)
    end = min(values.size, int(indices[-1]) + 1 + pad)
    return start, end


def trim_silence(
    audio: np.ndarray,
    sample_rate: int,
    *,
    threshold_db: float = TRIM_THRESHOLD_DB,
    pad_ms: float = TRIM_PAD_MS,
) -> np.ndarray:
    """Drop leading and trailing samples quieter than ``threshold_db`` below the peak.

    The threshold is relative to this file's own peak, so a quiet take is not
    discarded just because it never approaches full scale. Internal pauses are
    kept. ``pad_ms`` samples are restored on each cut edge when they exist.
    """
    values = as_mono(audio)
    start, end = trim_bounds(values, sample_rate, threshold_db=threshold_db, pad_ms=pad_ms)
    return values[start:end]


def highpass(
    audio: np.ndarray,
    sample_rate: int,
    *,
    cutoff_hz: float = HIGHPASS_HZ,
    order: int = HIGHPASS_ORDER,
) -> np.ndarray:
    """Zero-phase Butterworth high-pass.

    ``sosfiltfilt`` applies the section forward and back, so the effective
    rolloff is steeper than ``order`` alone. The cutoff is about 70 Hz by
    default: low enough to leave speech fundamentals, high enough to ease
    rumble before the peak stage measures headroom.
    """
    values = as_mono(audio)
    cutoff = float(cutoff_hz)
    nyquist = float(sample_rate) / 2.0
    if cutoff <= 0.0 or cutoff >= nyquist * 0.95:
        raise ValueError(f"high-pass cutoff {cutoff} Hz is outside the usable band at {sample_rate} Hz")
    sos = signal.butter(int(order), cutoff, btype="highpass", fs=int(sample_rate), output="sos")
    try:
        filtered = signal.sosfiltfilt(sos, values)
    except ValueError as exc:
        raise ValueError(f"audio is too short to high-pass at {sample_rate} Hz ({values.size} samples)") from exc
    return np.ascontiguousarray(filtered, dtype=np.float64)


def peak_normalize(audio: np.ndarray, target: float = PEAK_TARGET) -> np.ndarray:
    """Scale so the peak absolute sample equals ``target``.

    Silence is returned unchanged. ``target`` is linear amplitude, not dBFS.
    0.89 linear is about -1.0 dBFS.
    """
    if target <= 0.0 or target > 1.0:
        raise ValueError(f"peak target must be in (0, 1], got {target}")
    values = as_mono(audio)
    peak = float(np.max(np.abs(values))) if values.size else 0.0
    if peak < 1e-12:
        return values
    return values * (float(target) / peak)


def resample_audio(audio: np.ndarray, orig_sr: int, target_sr: int = PUBLISH_SAMPLE_RATE) -> np.ndarray:
    """Resample with ``scipy.signal.resample_poly``."""
    values = as_mono(audio)
    orig = int(orig_sr)
    target = int(target_sr)
    if orig <= 0 or target <= 0:
        raise ValueError("sample rates must be positive")
    if orig == target or values.size == 0:
        return values
    divisor = math.gcd(orig, target)
    up = target // divisor
    down = orig // divisor
    converted = signal.resample_poly(values, up, down)
    return np.ascontiguousarray(converted, dtype=np.float64)


def improve(
    audio: np.ndarray,
    sample_rate: int,
    *,
    hp_hz: float = HIGHPASS_HZ,
    peak_target: float = PEAK_TARGET,
    target_sr: int = PUBLISH_SAMPLE_RATE,
    threshold_db: float = TRIM_THRESHOLD_DB,
    pad_ms: float = TRIM_PAD_MS,
) -> ImproveResult:
    """Run the publish chain and return 24 kHz mono float32 audio by default."""
    source_rate = int(sample_rate)
    if source_rate <= 0:
        raise ValueError("sample rate must be positive")
    original = as_mono(audio)
    if original.size == 0:
        raise ValueError("audio is empty")
    input_seconds = float(original.size) / float(source_rate)

    trim_start, trim_end = trim_bounds(original, source_rate, threshold_db=threshold_db, pad_ms=pad_ms)
    trimmed = original[trim_start:trim_end]
    if trimmed.size == 0:
        raise ValueError("audio is silence at the trim threshold; nothing to publish")
    filtered = highpass(trimmed, source_rate, cutoff_hz=hp_hz)
    normalized = peak_normalize(filtered, target=peak_target)
    published = resample_audio(normalized, source_rate, target_sr)
    final = np.ascontiguousarray(published, dtype=np.float32)
    output_seconds = float(final.size) / float(target_sr) if target_sr else 0.0
    peak = float(np.max(np.abs(final))) if final.size else 0.0
    return ImproveResult(
        audio=final,
        sample_rate=int(target_sr),
        input_sample_rate=source_rate,
        input_seconds=input_seconds,
        output_seconds=output_seconds,
        peak=peak,
        trim_start=int(trim_start),
        trimmed_samples=int(trimmed.size),
        stages={
            "trim_threshold_db": float(threshold_db),
            "trim_pad_ms": float(pad_ms),
            "highpass_hz": float(hp_hz),
            "peak_target": float(peak_target),
            "publish_hz": float(target_sr),
            "trimmed_samples": float(trimmed.size),
        },
    )
