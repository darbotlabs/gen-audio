"""Prompt-to-script rules: user words only, no example cast file."""

from __future__ import annotations

from gen_audio.prompt_script import PromptError, prepare_prompt, voice_for


def test_empty_prompt_is_rejected():
    try:
        prepare_prompt("  \n", ["alice"], "kokoro_onnx", 6)
    except PromptError as exc:
        assert "empty prompt" in str(exc)
    else:
        raise AssertionError("empty prompt was accepted")


def test_two_personas_split_user_words_and_do_not_open_the_example_cast():
    prepared = prepare_prompt(
        "Hello from the user. Frank heard it.",
        ["alice", "frank"],
        "kokoro_onnx",
        6,
    )
    assert prepared.cast["source"] == "persona-voice-map"
    assert prepared.cast["speakers"]["1"]["voice"] == "af_heart"
    assert prepared.cast["speakers"]["2"]["voice"] == "am_michael"
    assert "podcast_script_sample" not in prepared.script_text
    spoken = prepared.spoken_text.split()
    prompt_words = "Hello from the user. Frank heard it.".split()
    assert spoken == prompt_words
    assert "Speaker 1 (Alice):" in prepared.script_text
    assert "Speaker 2 (Frank):" in prepared.script_text


def test_voice_ids_come_from_the_persona_map():
    assert voice_for("kokoro_onnx", "alice") == "af_heart"
    assert voice_for("kokoro_onnx", "frank") == "am_michael"
    assert voice_for("pocket_tts", "alice") == "alba"


def test_authored_speaker_script_keeps_the_user_text():
    prompt = "Speaker 1 (Alice): Alpha beta.\nSpeaker 2 (Frank): Gamma.\n"
    prepared = prepare_prompt(prompt, ["alice", "frank"], "kokoro_onnx", 8)
    assert "Alpha beta." in prepared.script_text
    assert "Gamma." in prepared.script_text
    assert prepared.spoken_text == "Alpha beta. Gamma."
