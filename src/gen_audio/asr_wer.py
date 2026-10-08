"""Local ASR and word error rate.

The recognizer is faster-whisper on CPU. A missing install is an error, not a
stand-in transcript.
"""

from __future__ import annotations

import re
from pathlib import Path

_WORD = re.compile(r"[a-z0-9']+")


class AsrError(RuntimeError):
    """ASR could not run. The message names what is missing."""


def normalize_words(text: str) -> list[str]:
    """Lowercase word tokens. Punctuation is not part of the WER alignment."""
    return _WORD.findall(text.lower())


def word_error_rate(reference: str, hypothesis: str) -> float:
    """Word error rate in ``[0, inf)``. An empty reference with no hypothesis is 0."""
    ref = normalize_words(reference)
    hyp = normalize_words(hypothesis)
    if not ref:
        return 0.0 if not hyp else 1.0
    return _levenshtein(ref, hyp) / float(len(ref))


def transcribe(path: Path | str) -> str:
    """Transcribe a WAV with faster-whisper tiny.en on CPU."""
    wav = Path(path)
    if not wav.is_file():
        raise AsrError(f"ASR input is missing: {wav.name}")
    try:
        from faster_whisper import WhisperModel
    except ImportError as exc:
        raise AsrError("faster-whisper is not installed") from exc
    try:
        model = WhisperModel("tiny.en", device="cpu", compute_type="int8")
        segments, _info = model.transcribe(str(wav), language="en", vad_filter=True)
        text = " ".join(segment.text.strip() for segment in segments).strip()
    except Exception as exc:
        raise AsrError(f"faster-whisper failed: {exc}") from exc
    return text


def _levenshtein(ref: list[str], hyp: list[str]) -> int:
    rows = len(ref) + 1
    cols = len(hyp) + 1
    previous = list(range(cols))
    for i in range(1, rows):
        current = [i]
        for j in range(1, cols):
            cost = 0 if ref[i - 1] == hyp[j - 1] else 1
            current.append(min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + cost))
        previous = current
    return previous[-1]
