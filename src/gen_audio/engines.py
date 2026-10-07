"""Named engines for compare notes.

Only ``kokoro_onnx`` has a synthesizer in this package. The other rows are
here so a score file can label WAVs that were rendered somewhere else. This
module does not download weights and does not rank engines.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class EngineSpec:
    """One compare-list entry."""

    id: str
    kind: str
    status: str
    summary: str


ENGINES: dict[str, EngineSpec] = {
    "kokoro_onnx": EngineSpec(
        id="kokoro_onnx",
        kind="synth",
        status="implemented",
        summary=(
            "Multi-turn WAV synthesis through the kokoro-onnx package. "
            "Requires local model and voice files via CLI flags or "
            "GEN_AUDIO_KOKORO_MODEL and GEN_AUDIO_KOKORO_VOICES."
        ),
    ),
    "kokoro_dayour": EngineSpec(
        id="kokoro_dayour",
        kind="synth",
        status="external",
        summary=(
            "Compare slot for the dayour Kokoro runtime. This repository does "
            "not vendor that runtime, its weights, or a client for it."
        ),
    ),
    "misaki": EngineSpec(
        id="misaki",
        kind="g2p",
        status="library",
        summary=(
            "hexgrad/misaki is the grapheme-to-phoneme library Kokoro uses. "
            "It is not a waveform synthesizer. kokoro-onnx phonemizes inside "
            "its own package; this repo does not call misaki directly."
        ),
    ),
    "vibevoice": EngineSpec(
        id="vibevoice",
        kind="synth",
        status="external",
        summary=(
            "Compare slot for VibeVoice long-form multi-speaker synthesis. "
            "No adapter and no weights are included."
        ),
    ),
    "magpie": EngineSpec(
        id="magpie",
        kind="synth",
        status="external",
        summary=(
            "Compare slot for Magpie TTS. No adapter and no weights are included."
        ),
    ),
    "pocket_tts": EngineSpec(
        id="pocket_tts",
        kind="synth",
        status="external",
        summary=(
            "Compare slot for Kyutai Pocket TTS, used as the CPU stand-in when "
            "a Magpie render is not available. No adapter and no weights are included."
        ),
    ),
}


def list_engines() -> list[EngineSpec]:
    """Return the compare list in a stable id order."""
    return [ENGINES[key] for key in sorted(ENGINES)]


def get_engine(engine_id: str) -> EngineSpec:
    """Return one spec or raise ``KeyError``."""
    try:
        return ENGINES[engine_id]
    except KeyError:
        known = ", ".join(sorted(ENGINES))
        raise KeyError(f"unknown engine {engine_id!r}; known ids: {known}") from None
