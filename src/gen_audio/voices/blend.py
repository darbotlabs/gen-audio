"""Weighted mix of equal-shaped style vectors.

Kokoro's PyTorch voicepacks and the kokoro-onnx ``voices-*.bin`` npz both
blend by a weighted sum. Weights must already sum to 1. This function does
not open a ``.pt`` file and does not import torch.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np

from gen_audio.voices.errors import VoiceError


def blend_vectors(vectors: dict[str, np.ndarray], parts: list[tuple[str, float]]) -> np.ndarray:
    """Return ``sum(weight * vector)`` for ``parts``.

    Every vector must share a shape. A missing name or an empty mix is an error.
    """
    if len(parts) < 2:
        raise VoiceError("a blend needs at least two voices")
    mixed: np.ndarray | None = None
    for voice_id, weight in parts:
        if voice_id not in vectors:
            known = ", ".join(sorted(vectors))
            raise VoiceError(f"blend voice {voice_id} is not in the style pack; known: {known}")
        vector = np.asarray(vectors[voice_id], dtype=np.float32)
        if vector.size == 0 or not np.isfinite(vector).all():
            raise VoiceError(f"blend voice {voice_id} has an empty or non-finite style vector")
        if mixed is None:
            mixed = np.zeros_like(vector, dtype=np.float32)
        elif vector.shape != mixed.shape:
            raise VoiceError(
                f"blend voice {voice_id} shape {vector.shape} does not match {mixed.shape}"
            )
        mixed = mixed + vector * np.float32(weight)
    assert mixed is not None
    return mixed


def load_style_pack(path: Path | str) -> dict[str, np.ndarray]:
    """Load a kokoro-onnx voices file. The ``.bin`` is an npz of named arrays."""
    source = Path(path)
    if not source.is_file():
        raise VoiceError(f"style pack not found: {source}")
    try:
        with np.load(source) as pack:
            names = list(pack.files)
            return {name: np.asarray(pack[name], dtype=np.float32) for name in names}
    except Exception as exc:
        raise VoiceError(f"style pack {source} is not a numpy npz of voice vectors") from exc


def blend_from_pack(path: Path | str, parts: list[tuple[str, float]]) -> np.ndarray:
    """Blend named vectors from a kokoro-onnx voices file."""
    return blend_vectors(load_style_pack(path), parts)
