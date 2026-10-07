"""Multi-turn WAV synthesis through kokoro-onnx.

The optional extra is imported only when a synthesizer is constructed.
Model files stay on disk; this module does not download them.

Expected files from the kokoro-onnx v1 setup are typically ``kokoro-v1.0.onnx``
and ``voices-v1.0.bin``. Point at whatever paths you actually have:

* ``--model`` / ``--voices``
* or ``GEN_AUDIO_KOKORO_MODEL`` and ``GEN_AUDIO_KOKORO_VOICES``

The cast map engine field must be ``kokoro_onnx``. Example voices in
``voices/cast_map.example.json`` are ``af_heart`` (Alice) and ``am_michael``
(Frank). kokoro-onnx rejects a voice id that is not in the loaded voice pack.
"""

from __future__ import annotations

import os
import threading
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from gen_audio.cast import CastMap, Turn, resolve_turn
from gen_audio.guards import (
    local_files_only_enabled,
    resolve_model_directory,
    verify_model_checksums,
)
from gen_audio.improve import improve, resample_audio

# One generate at a time. Unload and disk IO stay off this lock's caller thread
# by running through ``run_blocking`` in ``gen_audio.gateway``.
_MODEL_LOCK = threading.Lock()

DEFAULT_TURN_GAP_S = 0.35


@dataclass(frozen=True)
class SynthTurn:
    """One synthesized turn, before the shared gap that follows it."""

    speaker_id: str
    name: str
    voice: str
    lang: str
    speed: float
    text: str
    samples: int
    sample_rate: int


@dataclass(frozen=True)
class SynthResult:
    """Concatenated mono render plus a small manifest."""

    audio: np.ndarray
    sample_rate: int
    turns: list[SynthTurn] = field(default_factory=list)
    gap_s: float = DEFAULT_TURN_GAP_S
    improved: bool = False

    def manifest(self) -> dict:
        return {
            "engine": "kokoro_onnx",
            "sample_rate": self.sample_rate,
            "samples": int(self.audio.size),
            "seconds": (float(self.audio.size) / float(self.sample_rate)) if self.sample_rate else 0.0,
            "gap_s": self.gap_s,
            "improved": self.improved,
            "turns": [
                {
                    "speaker_id": turn.speaker_id,
                    "name": turn.name,
                    "voice": turn.voice,
                    "lang": turn.lang,
                    "speed": turn.speed,
                    "text": turn.text,
                    "samples": turn.samples,
                    "sample_rate": turn.sample_rate,
                }
                for turn in self.turns
            ],
        }


def resolve_model_paths(
    model: str | Path | None = None,
    voices: str | Path | None = None,
) -> tuple[Path, Path]:
    """Resolve model and voice-pack paths from arguments or the environment."""
    model_raw = model or os.environ.get("GEN_AUDIO_KOKORO_MODEL")
    voices_raw = voices or os.environ.get("GEN_AUDIO_KOKORO_VOICES")
    if not model_raw or not voices_raw:
        raise RuntimeError(
            "kokoro-onnx model paths are missing. Pass --model and --voices, or set "
            "GEN_AUDIO_KOKORO_MODEL and GEN_AUDIO_KOKORO_VOICES. This package does not download weights."
        )
    model_path = Path(model_raw)
    voices_path = Path(voices_raw)
    if local_files_only_enabled():
        for raw in (str(model_path), str(voices_path)):
            lowered = raw.replace("\\", "/").lower()
            if "://" in lowered or lowered.startswith("file:"):
                raise ValueError("GENAID_LOCAL_FILES_ONLY rejects non-local model paths")
    base = os.environ.get("GENAID_MODEL_BASE", "").strip()
    if base:
        model_dir = model_path if model_path.is_dir() else model_path.parent
        confined = Path(resolve_model_directory(str(model_dir), base))
        if not model_path.is_dir():
            model_path = confined / model_path.name
        verify_model_checksums(str(confined))
    return model_path, voices_path


class KokoroOnnxSynthesizer:
    """Lazy wrapper around ``kokoro_onnx.Kokoro``."""

    def __init__(self, model_path: str | Path, voices_path: str | Path) -> None:
        model = Path(model_path)
        voices = Path(voices_path)
        if not model.is_file():
            raise FileNotFoundError(f"kokoro-onnx model file not found: {model}")
        if not voices.is_file():
            raise FileNotFoundError(f"kokoro-onnx voices file not found: {voices}")
        try:
            from kokoro_onnx import Kokoro
        except ImportError as exc:
            raise ImportError(
                "kokoro-onnx is not installed. Install the optional extra: "
                "pip install 'gen-audio[kokoro]'"
            ) from exc
        self._kokoro = Kokoro(str(model), str(voices))

    def synthesize_turns(
        self,
        turns: list[Turn],
        cast: CastMap,
        *,
        gap_s: float = DEFAULT_TURN_GAP_S,
        improve_publish: bool = False,
    ) -> SynthResult:
        """Synthesize each turn and concatenate with silence between turns.

        ``improve_publish`` runs the 24 kHz publish chain on the concatenation.
        It is off by default so a before/after spectrogram can use the raw render.
        """
        if cast.engine != "kokoro_onnx":
            raise ValueError(
                f"cast map engine is {cast.engine!r}; synth_kokoro_onnx requires 'kokoro_onnx'"
            )
        if gap_s < 0.0:
            raise ValueError("gap_s must be >= 0")
        if not turns:
            raise ValueError("no turns to synthesize")

        with _MODEL_LOCK:
            return self._synthesize_locked(turns, cast, gap_s=gap_s, improve_publish=improve_publish)

    def _synthesize_locked(
        self,
        turns: list[Turn],
        cast: CastMap,
        *,
        gap_s: float,
        improve_publish: bool,
    ) -> SynthResult:
        pieces: list[np.ndarray] = []
        rendered: list[SynthTurn] = []
        sample_rate: int | None = None
        for index, turn in enumerate(turns):
            assignment = resolve_turn(turn, cast)
            try:
                raw_audio, raw_rate = self._kokoro.create(
                    turn.text,
                    voice=assignment.voice,
                    speed=assignment.speed,
                    lang=assignment.lang,
                )
            except AssertionError as exc:
                raise ValueError(
                    f"kokoro-onnx rejected speaker {turn.speaker_id} voice {assignment.voice!r}: {exc}"
                ) from exc
            audio = np.ascontiguousarray(np.asarray(raw_audio, dtype=np.float32))
            rate = int(raw_rate)
            if sample_rate is None:
                sample_rate = rate
            elif rate != sample_rate:
                audio = resample_audio(audio, rate, sample_rate).astype(np.float32)
                rate = sample_rate
            if audio.ndim != 1:
                raise ValueError(f"expected mono samples from kokoro-onnx, got shape {audio.shape}")
            if index and gap_s > 0.0:
                pieces.append(np.zeros(int(round(sample_rate * gap_s)), dtype=np.float32))
            pieces.append(audio)
            rendered.append(
                SynthTurn(
                    speaker_id=turn.speaker_id,
                    name=assignment.name,
                    voice=assignment.voice,
                    lang=assignment.lang,
                    speed=assignment.speed,
                    text=turn.text,
                    samples=int(audio.size),
                    sample_rate=rate,
                )
            )
        if sample_rate is None or not pieces:
            raise ValueError("kokoro-onnx produced no audio")
        mixed = np.concatenate(pieces).astype(np.float32)
        improved = False
        if improve_publish:
            published = improve(mixed, sample_rate)
            mixed = published.audio
            sample_rate = published.sample_rate
            improved = True
        return SynthResult(
            audio=mixed,
            sample_rate=sample_rate,
            turns=rendered,
            gap_s=float(gap_s),
            improved=improved,
        )
