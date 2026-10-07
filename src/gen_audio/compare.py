"""Score existing WAV files with the cube metrics.

Pass renders you already have (kokoro-onnx output, or a WAV from an external
engine). This does not synthesize, and it does not pick a winner.
"""

from __future__ import annotations

from pathlib import Path

from gen_audio.audio_io import read_wav
from gen_audio.cube_revision import measure
from gen_audio.engines import ENGINES


def score_wav(engine_id: str, path: Path | str) -> dict:
    """Measure one file and attach its engine label.

    Unregistered ids are allowed so a local render can be scored before an
    adapter exists. ``registered`` is false in that case.
    """
    engine = engine_id.strip()
    if not engine:
        raise ValueError("engine id is empty")
    audio_path = Path(path)
    audio, sample_rate = read_wav(audio_path)
    metrics = measure(audio, sample_rate)
    spec = ENGINES.get(engine)
    seconds = (audio.size / sample_rate) if sample_rate else 0.0
    return {
        "engine": engine,
        "registered": spec is not None,
        "engine_kind": None if spec is None else spec.kind,
        "engine_status": None if spec is None else spec.status,
        "path": str(audio_path),
        "sample_rate": sample_rate,
        "seconds": seconds,
        "metrics": metrics.to_dict(),
    }


def score_files(items: list[tuple[str, Path | str]]) -> list[dict]:
    """Score ``(engine_id, path)`` pairs in order."""
    return [score_wav(engine_id, path) for engine_id, path in items]
