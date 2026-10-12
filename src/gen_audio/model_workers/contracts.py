"""Small, standard-library-only worker protocol."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

VIBEVOICE_REVISION = "952326ddb264062466a888cf32a5b2f4e803e16e"
PERSONAPLEX_REVISION = "3428dfd95309a7f3c84fd93259ded0f810d1ff91"
GIB = 1024**3
SAMPLE_RATE = 24000
MAX_TEXT_BYTES = 32768
MAX_TEXT_CHARS = 8192
MAX_SECONDS = 60.0
MAX_REFERENCE_SECONDS = 30.0
MAX_NEW_TOKENS = 1024
MAX_INPUT_TOKENS = 4096
MAX_JSON_BYTES = 16 * 1024**2


class WorkerError(ValueError):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


@dataclass(frozen=True)
class Request:
    engine: str
    model_dir: Path
    output: Path | None = None
    text: str | None = None
    input_audio: Path | None = None
    persona: str | None = None
    voice_refs: tuple[Path, ...] = ()
    tokenizer: Path | None = None
    device: str = "cpu"
    max_new_tokens: int = 256
    max_seconds: float = 30.0
    timeout_seconds: float = 600.0
    seed: int = 0
    agent_names: tuple[str, ...] = ()
    execution: dict | None = None


def runtime_spec(engine: str) -> dict:
    if engine == "vibevoice":
        return {
            "distribution": "vibevoice",
            "repository": "https://github.com/vibevoice-community/VibeVoice",
            "revision": VIBEVOICE_REVISION,
            "requirements": "runtimes/requirements-vibevoice.txt",
            "capability": "script_tts",
            "versions": {
                "vibevoice": "0.1.0", "torch": "2.6.0",
                "transformers": "4.51.3", "accelerate": "1.6.0",
                "numpy": "2.1.3", "safetensors": "0.5.3",
                "datasets": "3.5.0", "gradio": "5.50.0", "peft": "0.15.2",
                "diffusers": "0.32.2", "librosa": "0.11.0", "numba": "0.61.2",
                "scipy": "1.15.2", "soundfile": "0.13.1",
            },
        }
    if engine == "personaplex":
        return {
            "distribution": "moshi-personaplex",
            "repository": "https://github.com/NVIDIA/personaplex",
            "revision": PERSONAPLEX_REVISION,
            "requirements": "runtimes/requirements-personaplex.txt",
            "capability": "speech_to_speech",
            "versions": {
                "moshi-personaplex": "0.1.0", "torch": "2.4.1",
                "numpy": "1.26.4", "safetensors": "0.4.5",
                "sentencepiece": "0.2.0", "sphn": "0.1.4",
                "pyloudnorm": "0.1.1", "huggingface-hub": "0.24.7",
                "scipy": "1.15.2",
            },
        }
    raise WorkerError("unsupported_engine", f"Unknown engine: {engine}")
