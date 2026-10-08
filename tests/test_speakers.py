"""Speaker segments come from concatenation cursors, not a duration estimate."""

from __future__ import annotations

import json

import numpy as np
import pytest

from gen_audio.asr_wer import AsrError
from gen_audio.assets import asset_object
from gen_audio.audio_io import write_wav
from gen_audio.improve import improve
from gen_audio.pipeline import run_pipeline
from gen_audio.speakers import (
    SpeechSegment,
    SpeechTimeline,
    SpeakerError,
    concatenate_turns,
    published_segments,
    remap_through_improve,
    roster,
    speaker_idx_bins,
)


def _piece(idx: int, n: int) -> tuple[int, np.ndarray]:
    return idx, np.full(n, 0.2, dtype=np.float32)


def test_two_speaker_boundaries_match_sample_offsets():
    rate = 16000
    gap = np.zeros(160, dtype=np.float32)
    audio, segments = concatenate_turns([_piece(0, 1000), _piece(1, 500)], rate, gap)
    assert audio.size == 1000 + gap.size + 500
    assert [(seg.speaker_idx, seg.start_sample, seg.end_sample) for seg in segments] == [
        (0, 0, 1000),
        (1, 1000 + int(gap.size), 1000 + int(gap.size) + 500),
    ]
    published = published_segments(segments, rate)
    assert published == [
        {"speaker_idx": 0, "start_s": 0 / rate, "end_s": 1000 / rate},
        {"speaker_idx": 1, "start_s": 1160 / rate, "end_s": 1660 / rate},
    ]
    for seg, fact in zip(segments, published, strict=True):
        assert fact["start_s"] == seg.start_sample / rate
        assert fact["end_s"] == seg.end_sample / rate


def test_eight_speaker_boundaries_match_sample_offsets():
    rate = 8000
    lengths = [100, 200, 300, 400, 500, 600, 700, 800]
    gap = np.zeros(50, dtype=np.float32)
    audio, segments = concatenate_turns([_piece(idx, n) for idx, n in enumerate(lengths)], rate, gap)
    cursor = 0
    published = published_segments(segments, rate)
    assert len(segments) == 8
    for idx, length in enumerate(lengths):
        if idx:
            cursor += int(gap.size)
        assert segments[idx].speaker_idx == idx
        assert segments[idx].start_sample == cursor
        assert segments[idx].end_sample == cursor + length
        assert published[idx]["start_s"] == cursor / rate
        assert published[idx]["end_s"] == (cursor + length) / rate
        cursor += length
    assert audio.size == cursor


def test_nine_speakers_are_refused():
    pieces = [(idx, np.ones(8, dtype=np.float32)) for idx in range(9)]
    with pytest.raises(SpeakerError, match="outside 0..7|at most 8"):
        concatenate_turns(pieces, 8000, np.zeros(0, dtype=np.float32))

    class _Assignment:
        name = "Persona"
        voice = "af_heart"

    class _Turn:
        def __init__(self, speaker_id: int) -> None:
            self.speaker_id = str(speaker_id)
            self.script_name = f"Speaker {speaker_id}"

    class _Cast:
        engine = "kokoro_onnx"
        speakers = {str(idx): _Assignment() for idx in range(9)}

    with pytest.raises(SpeakerError, match="at most 8"):
        roster([_Turn(idx) for idx in range(9)], _Cast())


def test_speaker_bins_mark_gaps_with_255():
    rate = 1000
    segments = [SpeechSegment(0, 0, 1000), SpeechSegment(1, 2000, 3000)]
    bins = speaker_idx_bins(segments, n_bins=3, bin_seconds=1.0, sample_rate=rate, n_samples=3000)
    assert bins == [0, 255, 1]
    split = speaker_idx_bins(
        [SpeechSegment(0, 0, 400), SpeechSegment(1, 400, 1000)],
        n_bins=1,
        bin_seconds=1.0,
        sample_rate=rate,
        n_samples=1000,
    )
    assert split == [1]
    mostly_silent = speaker_idx_bins(
        [SpeechSegment(0, 0, 400)],
        n_bins=1,
        bin_seconds=1.0,
        sample_rate=rate,
        n_samples=1000,
    )
    assert mostly_silent == [255]


def test_pipeline_cube_uses_remapped_sample_offsets(tmp_path, monkeypatch):
    rate = 24000
    gap = np.zeros(240, dtype=np.float32)
    speech, segments = concatenate_turns([_piece(0, 2400), _piece(1, 4800)], rate, gap)
    lead = np.zeros(1200, dtype=np.float32)
    tail = np.zeros(1200, dtype=np.float32)
    raw = np.concatenate([lead, speech, tail])
    shifted = [
        SpeechSegment(seg.speaker_idx, seg.start_sample + int(lead.size), seg.end_sample + int(lead.size))
        for seg in segments
    ]
    improved = improve(raw, rate)
    expected = remap_through_improve(
        shifted,
        trim_start=improved.trim_start,
        trimmed_len=improved.trimmed_samples,
        published_len=int(improved.audio.size),
    )
    raw_path = tmp_path / "raw.wav"
    write_wav(raw_path, raw, rate, subtype="FLOAT")
    script = tmp_path / "script.txt"
    script.write_text("Speaker 1: one\nSpeaker 2: two\n", encoding="utf-8")

    def boom(_path):
        raise AsrError("faster-whisper is not installed")

    monkeypatch.setattr("gen_audio.pipeline.transcribe", boom)
    manifest = run_pipeline(
        raw_path,
        out_dir=tmp_path,
        engine="tone",
        reference_text="one two",
        duration_target_s=improved.output_seconds,
        script_asset=asset_object(script, kind="script", derived_from=[]),
        speech=SpeechTimeline(
            speakers=[
                {"idx": 0, "persona": "Alice", "voice": "af_heart", "engine": "tone"},
                {"idx": 1, "persona": "Frank", "voice": "am_michael", "engine": "tone"},
            ],
            segments=shifted,
            sample_rate=rate,
        ),
    )
    document = json.loads((tmp_path / "cube.json").read_text(encoding="utf-8"))
    assert document["speakers"] == [
        {"idx": 0, "persona": "Alice", "voice": "af_heart", "engine": "tone"},
        {"idx": 1, "persona": "Frank", "voice": "am_michael", "engine": "tone"},
    ]
    assert document["segments"] == published_segments(expected, improved.sample_rate)
    for seg, fact in zip(expected, document["segments"], strict=True):
        assert fact["start_s"] == seg.start_sample / improved.sample_rate
        assert fact["end_s"] == seg.end_sample / improved.sample_rate
    assert len(document["speaker_idx"]) == document["cube_shape_f_t"][1]
    assert set(document["speaker_idx"]) <= {0, 1, 255}
    assert 255 in document["speaker_idx"]
