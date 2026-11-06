"""Named engines for compare notes.

``kokoro_onnx`` is the compare-list engine this package runs without optional
weights. ``kokoro_dayour`` and ``vibevoice`` have adapters that refuse until
the local runtime and weights exist. The other rows label WAVs rendered
somewhere else. This module does not download weights and does not rank engines.
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
            "dayour/kokoro KPipeline on CPU. Misaki is assigned to pipeline.g2p. "
            "Weights are not vendored; fetch hexgrad/Kokoro-82M only when "
            "GEN_AUDIO_ALLOW_HF_DOWNLOAD=1, otherwise use a local .pt pack."
        ),
    ),
    "misaki": EngineSpec(
        id="misaki",
        kind="g2p",
        status="library",
        summary=(
            "Misaki grapheme-to-phoneme for Kokoro. custom_entries merge into "
            "lexicon.golds. Kokoro receives those phonemes. Misaki does not "
            "emit a waveform."
        ),
    ),
    "vibevoice": EngineSpec(
        id="vibevoice",
        kind="synth",
        status="external",
        summary=(
            "VibeVoice 1.5B adapter. Calls the processor with voice_samples and "
            "generate with cfg_scale, DDPM steps, and the tokenizer. 7B is "
            "GPU-only. Weights are not bundled."
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
