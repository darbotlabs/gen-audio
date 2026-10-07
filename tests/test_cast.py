from __future__ import annotations

from pathlib import Path

import pytest

from gen_audio.cast import load_cast_map, parse_script, read_script, resolve_turn

ROOT = Path(__file__).resolve().parents[1]
SAMPLE = ROOT / "examples" / "podcast_script_sample.txt"
CAST = ROOT / "voices" / "cast_map.example.json"


def test_sample_script_is_labeled_and_resolves_alice_frank():
    text = SAMPLE.read_text(encoding="utf-8")
    lowered = text.lower()
    assert "sample script" in lowered
    assert "not a news broadcast" in lowered
    turns = read_script(SAMPLE)
    assert len(turns) >= 4
    assert {turn.speaker_id for turn in turns} == {"1", "2"}
    cast = load_cast_map(CAST)
    assert cast.engine == "kokoro_onnx"
    first = resolve_turn(turns[0], cast)
    second = resolve_turn(turns[1], cast)
    assert first.name == "Alice"
    assert first.voice == "af_heart"
    assert second.name == "Frank"
    assert second.voice == "am_michael"


def test_block_turn_joins_lines():
    turns = parse_script(
        "Speaker 1 (Alice):\nHello there\nfriend.\n\nSpeaker 2: Noted.\n"
    )
    assert len(turns) == 2
    assert turns[0].text == "Hello there friend."
    assert turns[0].script_name == "Alice"
    assert turns[1].text == "Noted."


def test_text_before_a_speaker_is_an_error():
    with pytest.raises(ValueError, match="line 1"):
        parse_script("Hello\nSpeaker 1: Hi\n")


def test_empty_turn_is_an_error():
    with pytest.raises(ValueError, match="no text"):
        parse_script("Speaker 1:\n\nSpeaker 2: Hi\n")


def test_unknown_speaker_lists_known_ids():
    turns = parse_script("Speaker 3: Someone else.\n")
    cast = load_cast_map(CAST)
    with pytest.raises(KeyError, match="known speakers: 1, 2"):
        resolve_turn(turns[0], cast)


def test_speed_outside_kokoro_range_is_rejected(tmp_path: Path):
    path = tmp_path / "cast.json"
    path.write_text(
        '{"engine": "kokoro_onnx", "speakers": {"1": {"name": "A", "voice": "af_heart", "speed": 3}}}\n',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="0.5"):
        load_cast_map(path)
