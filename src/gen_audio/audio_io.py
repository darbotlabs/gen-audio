"""WAV read and write helpers. Arrays in memory are mono float32."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import soundfile as sf


def as_mono(audio: np.ndarray) -> np.ndarray:
    """Return a 1-D float64 copy in about [-1, 1].

    Stereo input is averaged to mono. Integer PCM is scaled by the dtype's
    full-scale value. Float input is not rescaled; pass samples that are
    already in amplitude units, which is what ``read_wav`` returns.
    """
    values = np.asarray(audio)
    if np.issubdtype(values.dtype, np.integer):
        info = np.iinfo(values.dtype)
        scale = float(max(abs(int(info.min)), int(info.max)))
        values = values.astype(np.float64) / scale
    if values.ndim == 2:
        if values.shape[1] == 1:
            values = values[:, 0]
        elif values.shape[0] == 1:
            values = values[0]
        else:
            values = values.mean(axis=1)
    if values.ndim != 1:
        raise ValueError(f"expected mono or stereo audio, got shape {audio.shape}")
    mono = np.ascontiguousarray(values, dtype=np.float64)
    if not np.isfinite(mono).all():
        raise ValueError("audio contains NaN or Inf")
    return mono


def read_wav(path: Path | str) -> tuple[np.ndarray, int]:
    """Read a WAV file as mono float32 plus its sample rate."""
    data, sample_rate = sf.read(Path(path), always_2d=True, dtype="float32")
    mono = as_mono(data).astype(np.float32)
    if int(sample_rate) <= 0:
        raise ValueError(f"invalid sample rate {sample_rate} in {path}")
    return mono, int(sample_rate)


def write_wav(
    path: Path | str,
    audio: np.ndarray,
    sample_rate: int,
    *,
    subtype: str = "PCM_16",
) -> Path:
    """Write mono audio. Parent directories are created.

    PCM_16 is the default interchange format. Values outside [-1, 1] are
    clipped to full scale on the way into 16-bit, and quantization can move a
    sample by about one step, so a 0.89 peak target may read back a hair off.
    Pass ``subtype="FLOAT"`` to store float32 instead. The kokoro-onnx CLI
    does that for a raw render, before the publish chain has limited the peak.
    """
    if int(sample_rate) <= 0:
        raise ValueError("sample rate must be positive")
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    mono = as_mono(audio).astype(np.float32)
    sf.write(destination, mono, int(sample_rate), subtype=subtype)
    return destination
