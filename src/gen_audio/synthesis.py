"""Engine boundary and bounded multi-turn assembly; no backend is imported here."""

from __future__ import annotations

import hashlib
import math
import time
from dataclasses import dataclass, field
from typing import Protocol

import numpy as np

from gen_audio.audio_io import as_mono, validate_frame_count, validate_sample_rate
from gen_audio.audio_metadata import agent_metadata
from gen_audio.cast import CastMap, Turn, VoiceAssignment, resolve_turn, validate_turns
from gen_audio.guards import max_audio_duration, max_generation_seconds
from gen_audio.improve import improve, resample_audio
from gen_audio.speakers import SpeechSegment, published_segments, remap_through_improve, roster

DEFAULT_TURN_GAP_S = 0.35


class EngineAdapter(Protocol):
    """A local backend; other engines need real adapters before they can be used."""

    engine_id: str
    provenance: dict

    def validate_assignment(self, assignment: VoiceAssignment) -> None: ...

    def generate(self, text: str, assignment: VoiceAssignment) -> tuple[np.ndarray, int]: ...


@dataclass(frozen=True)
class SynthTurn:
    speaker_id: str
    name: str
    voice: str
    lang: str
    speed: float
    text: str
    samples: int
    sample_rate: int
    source_samples: int
    source_sample_rate: int
    start_sample: int
    gap_before_samples: int


@dataclass(frozen=True)
class SynthResult:
    audio: np.ndarray
    sample_rate: int
    turns: list[SynthTurn] = field(default_factory=list)
    gap_s: float = DEFAULT_TURN_GAP_S
    improved: bool = False
    engine: str = "kokoro_onnx"
    provenance: dict = field(default_factory=dict)
    pre_improve_samples: int = 0
    pre_improve_sample_rate: int = 0
    speakers: list[dict] = field(default_factory=list)
    segments: list[SpeechSegment] = field(default_factory=list)

    def manifest(self, *, include_text: bool = False) -> dict:
        """Raw turn receipts use pre-improve cursors; segments use published audio."""
        validate_frame_count(self.audio.size, self.sample_rate)
        turns = []
        for turn in self.turns:
            record = {
                "speaker_id": turn.speaker_id,
                "name": turn.name,
                "voice": turn.voice,
                "lang": turn.lang,
                "speed": turn.speed,
                "text_chars": len(turn.text),
                "text_sha256": hashlib.sha256(turn.text.encode("utf-8")).hexdigest(),
                "samples": turn.samples,
                "sample_rate": turn.sample_rate,
                "source_samples": turn.source_samples,
                "source_sample_rate": turn.source_sample_rate,
                "start_sample": turn.start_sample,
                "gap_before_samples": turn.gap_before_samples,
            }
            if include_text:
                record["text"] = turn.text
            turns.append(record)
        return {
            "schema_version": "1.0",
            "synthesizedSpeech": True,
            "engine": self.engine,
            "sample_rate": self.sample_rate,
            "samples": int(self.audio.size),
            "duration_s": self.audio.size / self.sample_rate,
            "seconds": self.audio.size / self.sample_rate,
            "gap_s": self.gap_s,
            "gap_samples": sum(turn.gap_before_samples for turn in self.turns),
            "improved": self.improved,
            "turn_count": len(turns),
            "speaker_count": len({turn.speaker_id for turn in self.turns}),
            "text_included": include_text,
            "turn_timing_basis": "pre_improve",
            "pre_improve_samples": self.pre_improve_samples,
            "pre_improve_sample_rate": self.pre_improve_sample_rate,
            "speakers": list(self.speakers),
            "metadata": agent_metadata(list(dict.fromkeys(turn.name for turn in self.turns))),
            "segments": published_segments(self.segments, self.sample_rate),
            "provenance": self.provenance,
            "turns": turns,
        }


def validate_request(
    turns: list[Turn], cast: CastMap, engine_id: str, gap_s: float, max_duration_s: float | None
) -> tuple[list[VoiceAssignment], float]:
    if cast.engine != engine_id:
        raise ValueError(f"cast map engine must be {engine_id!r}, got {cast.engine!r}")
    validate_turns(turns)
    if isinstance(gap_s, bool) or not math.isfinite(gap_s) or gap_s < 0:
        raise ValueError("gap_s must be finite and >= 0")
    duration_cap = max_audio_duration(max_duration_s)
    if gap_s > duration_cap or (len(turns) - 1) * gap_s >= duration_cap:
        raise ValueError("turn gaps exceed the audio duration limit")
    assignments = [resolve_turn(turn, cast) for turn in turns]
    roster(turns, cast)
    return assignments, duration_cap


def validate_generated_audio(audio: np.ndarray, rate: int, duration_cap: float) -> np.ndarray:
    validate_sample_rate(rate)
    if not isinstance(audio, np.ndarray) or audio.ndim != 1:
        raise ValueError("engine must return a mono numpy sample array")
    validate_frame_count(audio.size, rate, max_duration=duration_cap)
    values = as_mono(audio).astype(np.float32)
    if not np.any(values):
        raise ValueError("engine produced only silence")
    return values


def synthesize_turns(
    adapter: EngineAdapter,
    turns: list[Turn],
    cast: CastMap,
    *,
    gap_s: float = DEFAULT_TURN_GAP_S,
    improve_publish: bool = False,
    max_duration_s: float | None = None,
    generation_timeout_s: float | None = None,
    deadline: float | None = None,
) -> SynthResult:
    assignments, duration_cap = validate_request(turns, cast, adapter.engine_id, gap_s, max_duration_s)
    speakers, index_of = roster(turns, cast)
    timeout = max_generation_seconds(generation_timeout_s)
    deadline = min(deadline, time.monotonic() + timeout) if deadline is not None else time.monotonic() + timeout

    def check_deadline() -> None:
        if time.monotonic() >= deadline:
            raise TimeoutError("speech generation exceeded its deadline")

    for assignment in assignments:
        adapter.validate_assignment(assignment)
    pieces: list[np.ndarray] = []
    rendered: list[SynthTurn] = []
    segments: list[SpeechSegment] = []
    sample_rate: int | None = None
    total = 0
    for index, (turn, assignment) in enumerate(zip(turns, assignments, strict=True)):
        check_deadline()
        raw_audio, raw_rate = adapter.generate(turn.text, assignment)
        check_deadline()
        rate = validate_sample_rate(raw_rate)
        audio = validate_generated_audio(raw_audio, rate, duration_cap)
        source_samples = int(audio.size)
        if sample_rate is None:
            sample_rate = rate
        expected = (audio.size * sample_rate + rate - 1) // rate
        gap_samples = int(round(sample_rate * gap_s)) if index else 0
        # Check the allocation size *before* resampling or allocating silence.
        validate_frame_count(total + gap_samples + expected, sample_rate, max_duration=duration_cap)
        if rate != sample_rate:
            audio = resample_audio(audio, rate, sample_rate).astype(np.float32)
            audio = validate_generated_audio(audio, sample_rate, duration_cap)
        if gap_samples:
            pieces.append(np.zeros(gap_samples, dtype=np.float32))
        pieces.append(audio)
        segments.append(SpeechSegment(index_of[turn.speaker_id], total + gap_samples, total + gap_samples + int(audio.size)))
        rendered.append(SynthTurn(
            speaker_id=turn.speaker_id, name=assignment.name, voice=assignment.voice,
            lang=assignment.lang, speed=assignment.speed, text=turn.text,
            samples=int(audio.size), sample_rate=sample_rate, source_samples=source_samples,
            source_sample_rate=rate, start_sample=total + gap_samples,
            gap_before_samples=gap_samples,
        ))
        total += gap_samples + audio.size
    mixed = np.concatenate(pieces)
    assembly_rate = sample_rate
    if improve_publish:
        published = improve(mixed, sample_rate)
        segments = remap_through_improve(
            segments, trim_start=published.trim_start,
            trimmed_len=published.trimmed_samples, published_len=int(published.audio.size),
        )
        mixed, sample_rate = published.audio, published.sample_rate
    check_deadline()
    mixed = validate_generated_audio(mixed, sample_rate, duration_cap)
    return SynthResult(
        audio=mixed, sample_rate=sample_rate, turns=rendered, gap_s=float(gap_s),
        improved=improve_publish, engine=adapter.engine_id, provenance=adapter.provenance,
        pre_improve_samples=int(total), pre_improve_sample_rate=assembly_rate,
        speakers=speakers, segments=segments,
    )
