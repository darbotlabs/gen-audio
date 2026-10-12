"""WAV read and write helpers. Arrays in memory are mono float32."""

from __future__ import annotations

from pathlib import Path
from numbers import Integral
import stat

import numpy as np
import soundfile as sf

from gen_audio.guards import (
    RequestLimitError, max_audio_duration, max_audio_file_bytes, max_audio_samples,
)

HARD_MAX_SAMPLE_RATE = 192_000
HARD_MAX_CHANNELS = 8
BLOCK_FRAMES = 65_536


def validate_sample_rate(sample_rate: int) -> int:
    """Require a true integer in 1..192000, not bool, NaN, or a truncated float."""
    if isinstance(sample_rate, (bool, np.bool_)) or not isinstance(sample_rate, Integral):
        raise ValueError("sample rate must be a positive integer")
    if not 0 < sample_rate <= HARD_MAX_SAMPLE_RATE:
        raise ValueError(f"sample rate must be in 1..{HARD_MAX_SAMPLE_RATE}")
    return int(sample_rate)


def validate_frame_count(frames: int, sample_rate: int, *, max_duration: float | None = None) -> None:
    rate = validate_sample_rate(sample_rate)
    if isinstance(frames, (bool, np.bool_)) or not isinstance(frames, Integral) or frames <= 0:
        raise ValueError("audio is empty")
    if frames > max_audio_samples() or frames / rate > max_audio_duration(max_duration):
        raise RequestLimitError("audio exceeds sample or duration limit")


def validate_wav_file(handle: sf.SoundFile, *, max_duration: float | None = None) -> None:
    """Validate metadata before decoding; actual blocks must still be finite."""
    if handle.format not in {"WAV", "WAVEX", "RF64"}:
        raise ValueError("input must be a WAV file")
    if not 1 <= handle.channels <= HARD_MAX_CHANNELS:
        raise ValueError(f"audio must have 1..{HARD_MAX_CHANNELS} channels")
    validate_frame_count(handle.frames, handle.samplerate, max_duration=max_duration)
    if handle.frames * handle.channels > max_audio_samples():
        raise RequestLimitError("decoded audio exceeds sample limit")


def check_audio_file_size(path: Path | str) -> None:
    info = Path(path).stat()
    if not stat.S_ISREG(info.st_mode):
        raise ValueError("audio input must be a regular file")
    if info.st_size > max_audio_file_bytes():
        raise RequestLimitError("audio file exceeds byte limit")


def as_mono(audio: np.ndarray) -> np.ndarray:
    """Return a 1-D float64 copy in about [-1, 1].

    Stereo input is averaged to mono. Integer PCM is scaled by the dtype's
    full-scale value. Float input is not rescaled; pass samples that are
    already in amplitude units, which is what ``read_wav`` returns.
    """
    values = np.asarray(audio)
    if values.dtype.kind not in "iuf":
        raise ValueError("audio must contain real numeric samples")
    if values.size > max_audio_samples():
        raise RequestLimitError("audio exceeds sample limit")
    if values.ndim == 2 and not 1 <= values.shape[1] <= HARD_MAX_CHANNELS:
        raise ValueError(f"audio must have 1..{HARD_MAX_CHANNELS} channels")
    if np.issubdtype(values.dtype, np.integer):
        info = np.iinfo(values.dtype)
        scale = float(max(abs(int(info.min)), int(info.max)))
        values = values.astype(np.float64) / scale
    if not np.isfinite(values).all():
        raise ValueError("audio contains NaN or Inf")
    if values.ndim == 2:
        if values.shape[1] == 1:
            values = values[:, 0]
        else:
            values = values.mean(axis=1, dtype=np.float64)
    if values.ndim != 1:
        raise ValueError(f"expected mono or stereo audio, got shape {values.shape}")
    mono = np.ascontiguousarray(values, dtype=np.float64)
    if not np.isfinite(mono).all():
        raise ValueError("audio contains NaN or Inf")
    if mono.size and np.max(np.abs(mono)) > np.finfo(np.float32).max:
        raise ValueError("audio cannot be represented as finite float32")
    return mono


def read_wav(path: Path | str) -> tuple[np.ndarray, int]:
    """Read bounded WAV frames; inspect metadata before allocating the mono result."""
    check_audio_file_size(path)
    with sf.SoundFile(path) as source:
        validate_wav_file(source)
        mono = np.empty(source.frames, dtype=np.float32)
        offset = 0
        for data in source.blocks(blocksize=BLOCK_FRAMES, dtype="float32", always_2d=True):
            block = as_mono(data)
            mono[offset:offset + block.size] = block
            offset += block.size
        if offset != mono.size:
            raise ValueError("WAV frame count changed during read")
        return mono, source.samplerate


def write_wav(
    path: Path | str,
    audio: np.ndarray,
    sample_rate: int,
    *,
    subtype: str = "PCM_16",
    agent_names: list[str] | tuple[str, ...] = (),
) -> Path:
    """Write mono audio. Parent directories are created.

    PCM_16 is the default interchange format. Values outside [-1, 1] are
    clipped to full scale on the way into 16-bit, and quantization can move a
    sample by about one step, so a 0.89 peak target may read back a hair off.
    Pass ``subtype="FLOAT"`` to store float32 instead. The kokoro-onnx CLI
    does that for a raw render, before the publish chain has limited the peak.
    """
    rate = validate_sample_rate(sample_rate)
    mono = as_mono(audio).astype(np.float32)
    validate_frame_count(mono.size, rate)
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    from gen_audio.artifacts import staged_files
    with staged_files([destination], replace=True) as staged:
        sf.write(staged[0], mono, rate, subtype=subtype, format="WAV")
        if agent_names:
            from gen_audio.audio_metadata import tag_wav
            tag_wav(staged[0], agent_names)
        if staged[0].stat().st_size > max_audio_file_bytes():
            raise RequestLimitError("encoded audio exceeds byte limit")
    return destination
