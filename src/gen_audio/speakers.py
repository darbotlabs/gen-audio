"""Speaker timeline taken from concatenation sample cursors.

Each script line is one buffer. A gap is the silence array actually inserted
between buffers. Segment bounds are those cursors divided by the sample rate.
Nothing here estimates a duration from text length.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from gen_audio.identity import round_half_up

MAX_SPEAKERS = 8
SILENCE_IDX = 255
UNRESOLVED = "unresolved"


class SpeakerError(ValueError):
    """The speaker timeline cannot be recorded."""


@dataclass(frozen=True)
class SpeechSegment:
    """One speaker's samples in a single buffer, ``[start_sample, end_sample)``."""

    speaker_idx: int
    start_sample: int
    end_sample: int


@dataclass(frozen=True)
class SpeechTimeline:
    """Roster plus segments whose sample offsets use ``sample_rate``."""

    speakers: list[dict]
    segments: list[SpeechSegment]
    sample_rate: int


def concatenate_turns(
    pieces: list[tuple[int, np.ndarray]],
    sample_rate: int,
    gap: np.ndarray,
) -> tuple[np.ndarray, list[SpeechSegment]]:
    """Concatenate ``(speaker_idx, samples)`` pieces, inserting ``gap`` between them.

    ``gap`` is the buffer that is appended, including a zero-length array when
    turns are abutted. The cursor advances by ``len(gap)`` and by each piece.
    Speaker indices are 0..7. A ninth distinct speaker, or any index outside
    that range, is refused.
    """
    rate = int(sample_rate)
    if rate <= 0:
        raise SpeakerError("sample rate must be positive")
    silence = np.asarray(gap, dtype=np.float32).reshape(-1)
    seen: list[int] = []
    chunks: list[np.ndarray] = []
    segments: list[SpeechSegment] = []
    cursor = 0
    started = False
    for raw_idx, raw_audio in pieces:
        idx = int(raw_idx)
        if idx < 0 or idx > 7:
            raise SpeakerError(f"speaker idx {idx} is outside 0..7")
        if idx not in seen:
            if len(seen) >= MAX_SPEAKERS:
                raise SpeakerError("at most 8 speakers")
            seen.append(idx)
        audio = np.asarray(raw_audio, dtype=np.float32).reshape(-1)
        if started and silence.size:
            chunks.append(silence)
            cursor += int(silence.size)
        if audio.size == 0:
            continue
        start = cursor
        chunks.append(audio)
        cursor += int(audio.size)
        segments.append(SpeechSegment(idx, start, cursor))
        started = True
    if not chunks:
        raise SpeakerError("no audio to concatenate")
    return np.concatenate(chunks).astype(np.float32), segments


def roster(turns: list, cast) -> tuple[list[dict], dict[str, int]]:
    """Assign idx 0..7 in first-appearance order. Refuses a ninth speaker."""
    speakers: list[dict] = []
    index_of: dict[str, int] = {}
    engine = str(getattr(cast, "engine", "") or "").strip()
    if not engine:
        raise SpeakerError("speaker engine must be non-empty")
    for turn in turns:
        speaker_id = str(turn.speaker_id)
        if speaker_id in index_of:
            continue
        if len(speakers) >= MAX_SPEAKERS:
            raise SpeakerError("at most 8 speakers")
        assignment = cast.speakers[speaker_id]
        persona = str(turn.script_name or assignment.name).strip()
        voice = str(assignment.voice).strip()
        if not persona or not voice:
            raise SpeakerError("speaker persona and voice must be non-empty")
        idx = len(speakers)
        index_of[speaker_id] = idx
        speakers.append({"idx": idx, "persona": persona, "voice": voice, "engine": engine})
    if not speakers:
        raise SpeakerError("no speakers")
    return speakers, index_of


def remap_through_improve(
    segments: list[SpeechSegment],
    *,
    trim_start: int,
    trimmed_len: int,
    published_len: int,
) -> list[SpeechSegment]:
    """Map raw cursors onto the published buffer.

    Leading trim drops ``[0, trim_start)``. High-pass and peak-normalize keep
    the trimmed length. Resample scales by the actual published length over
    the actual trimmed length.
    """
    trimmed = int(trimmed_len)
    published = int(published_len)
    if trimmed <= 0:
        raise SpeakerError("trimmed length must be positive")
    if published < 0:
        raise SpeakerError("published length must be non-negative")
    shifted: list[SpeechSegment] = []
    origin = int(trim_start)
    for seg in segments:
        start = int(seg.start_sample) - origin
        end = int(seg.end_sample) - origin
        if end <= 0 or start >= trimmed:
            continue
        start = max(0, start)
        end = min(trimmed, end)
        if end <= start:
            continue
        shifted.append(SpeechSegment(int(seg.speaker_idx), start, end))
    if published == trimmed:
        return shifted
    return scale_segments(shifted, trimmed, published)


def scale_segments(segments: list[SpeechSegment], src_len: int, dst_len: int) -> list[SpeechSegment]:
    """Scale cursors by ``dst_len / src_len`` using integer arithmetic."""
    src = int(src_len)
    dst = int(dst_len)
    if src <= 0 or dst < 0:
        raise SpeakerError("segment scale lengths must be positive")
    out: list[SpeechSegment] = []
    for seg in segments:
        start = (int(seg.start_sample) * dst) // src
        end = (int(seg.end_sample) * dst) // src
        if end <= start and int(seg.end_sample) > int(seg.start_sample) and start < dst:
            end = start + 1
        end = min(end, dst)
        if end > start:
            out.append(SpeechSegment(int(seg.speaker_idx), start, end))
    return out


def published_segments(segments: list[SpeechSegment], sample_rate: int) -> list[dict]:
    """Seconds are ``sample / rate`` of the cursors. No other fields."""
    rate = int(sample_rate)
    if rate <= 0:
        raise SpeakerError("sample rate must be positive")
    return [
        {
            "speaker_idx": int(seg.speaker_idx),
            "start_s": int(seg.start_sample) / float(rate),
            "end_s": int(seg.end_sample) / float(rate),
        }
        for seg in segments
    ]


def speaker_idx_bins(
    segments: list[SpeechSegment],
    *,
    n_bins: int,
    bin_seconds: float,
    sample_rate: int,
    n_samples: int,
) -> list[int]:
    """One speaker per cube time bin. 255 when silence or a gap has the majority.

    Bin edges are ``round_half_up(bin_index * bin_seconds * sample_rate)``,
    clamped to the buffer. Synthesized output does not overlap, so a bin that
    crosses a boundary takes the speaker with more samples in that bin.
    """
    count = int(n_bins)
    if count < 0:
        raise SpeakerError("time bin count must be non-negative")
    rate = int(sample_rate)
    total = int(n_samples)
    if rate <= 0 or total < 0:
        raise SpeakerError("sample rate and length must be non-negative")
    edges = [_bin_edge(index, bin_seconds, rate, total) for index in range(count + 1)]
    bins: list[int] = []
    for index in range(count):
        start = edges[index]
        end = edges[index + 1]
        span = end - start
        if span <= 0:
            bins.append(SILENCE_IDX)
            continue
        counts: dict[int, int] = {}
        for seg in segments:
            overlap = max(0, min(int(seg.end_sample), end) - max(int(seg.start_sample), start))
            if overlap:
                counts[int(seg.speaker_idx)] = counts.get(int(seg.speaker_idx), 0) + overlap
        spoken = sum(counts.values())
        silence = span - spoken
        if not counts or max(counts.values()) <= silence:
            bins.append(SILENCE_IDX)
            continue
        winner = min(((-samples, idx) for idx, samples in counts.items()))[1]
        bins.append(int(winner))
    return bins


def timeline_from_dict(payload: dict) -> SpeechTimeline:
    """Rebuild a timeline from the synth result. Offsets stay the recorded samples."""
    rate = int(payload["sample_rate"])
    speakers = payload.get("speakers")
    if not isinstance(speakers, list) or not speakers:
        raise SpeakerError("speech timeline needs a speaker roster")
    if len(speakers) > MAX_SPEAKERS:
        raise SpeakerError("at most 8 speakers")
    segments: list[SpeechSegment] = []
    for item in payload.get("segments") or []:
        segments.append(
            SpeechSegment(
                int(item["speaker_idx"]),
                int(item["start_sample"]),
                int(item["end_sample"]),
            )
        )
    return SpeechTimeline(speakers=list(speakers), segments=segments, sample_rate=rate)


def _bin_edge(index: int, bin_seconds: float, sample_rate: int, n_samples: int) -> int:
    sample = round_half_up(float(index) * float(bin_seconds) * float(sample_rate))
    if sample < 0:
        return 0
    if sample > n_samples:
        return n_samples
    return sample
