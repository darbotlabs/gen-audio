"""First-class synthesizer adapters.

Each engine either writes a real WAV or raises ``EngineRefusal`` naming what
is missing. Nothing here writes a substitute tone.
"""

from __future__ import annotations

import importlib.util
import os
import shutil
from dataclasses import dataclass
from pathlib import Path

import numpy as np

from gen_audio.audio_io import write_wav
from gen_audio.cast import CastMap, Turn, resolve_turn


class EngineRefusal(RuntimeError):
    """The engine will not run. ``missing`` lists the exact gaps."""

    def __init__(self, engine: str, missing: list[str]):
        self.engine = engine
        self.missing = [item for item in missing if item]
        if not self.missing:
            self.missing = [f"{engine} refused without a reason"]
        detail = "; ".join(self.missing)
        super().__init__(f"{engine} refusal: {detail}. No speech was invented.")


@dataclass(frozen=True)
class Probe:
    engine: str
    available: bool
    missing: list[str]

    def as_dict(self) -> dict:
        return {
            "engine": self.engine,
            "available": self.available,
            "missing": list(self.missing),
            "weightsBundled": False,
        }


def probe(engine: str) -> Probe:
    """Report whether this process can synthesize with ``engine``."""
    missing = _missing(engine)
    return Probe(engine=engine, available=not missing, missing=missing)


def synthesize(engine: str, turns: list[Turn], cast: CastMap, output: Path) -> dict:
    """Render ``turns`` to ``output``. Raises ``EngineRefusal`` when the engine cannot run."""
    found = probe(engine)
    if not found.available:
        raise EngineRefusal(engine, found.missing)
    if cast.engine != engine:
        raise EngineRefusal(engine, [f"cast map engine is {cast.engine}, not {engine}"])
    if engine == "kokoro_onnx":
        return _synth_kokoro(turns, cast, output)
    if engine in {"pocket_tts", "vibevoice", "magpie"}:
        raise EngineRefusal(
            engine,
            [f"{engine} is experimental and is not enabled until a measured run proves it"],
        )
    raise EngineRefusal(engine, [f"no adapter is registered for {engine}"])


def _missing(engine: str) -> list[str]:
    if engine == "kokoro_onnx":
        return _missing_kokoro()
    if engine == "pocket_tts":
        return _missing_pocket()
    if engine == "vibevoice":
        return _missing_vibevoice()
    if engine == "magpie":
        return _missing_magpie()
    if engine == "misaki":
        return ["misaki is a grapheme-to-phoneme library and does not emit a waveform"]
    if engine == "kokoro_dayour":
        return ["the dayour/kokoro torch runtime is not vendored in this package"]
    return [f"unknown engine {engine}"]


def _module_present(name: str) -> bool:
    try:
        return importlib.util.find_spec(name) is not None
    except (ModuleNotFoundError, ValueError):
        return False


def _missing_kokoro() -> list[str]:
    missing: list[str] = []
    if not _module_present("kokoro_onnx"):
        missing.append("kokoro-onnx package is not installed (pip install 'gen-audio[kokoro]')")
    model = os.environ.get("GEN_AUDIO_KOKORO_MODEL", "").strip()
    voices = os.environ.get("GEN_AUDIO_KOKORO_VOICES", "").strip()
    if not model:
        missing.append("GEN_AUDIO_KOKORO_MODEL is unset")
    elif not Path(model).is_file():
        missing.append("GEN_AUDIO_KOKORO_MODEL file is missing")
    if not voices:
        missing.append("GEN_AUDIO_KOKORO_VOICES is unset")
    elif not Path(voices).is_file():
        missing.append("GEN_AUDIO_KOKORO_VOICES file is missing")
    return missing


def _missing_pocket() -> list[str]:
    missing: list[str] = []
    if not _module_present("pocket_tts"):
        missing.append("pocket-tts package is not installed")
    if shutil.which("ffmpeg") is None:
        # pocket-tts audio prompts are wav; ffmpeg is not required to import.
        pass
    return missing


def _cuda_available() -> bool:
    if not _module_present("torch"):
        return False
    import torch

    return bool(torch.cuda.is_available())


def _missing_vibevoice() -> list[str]:
    missing: list[str] = []
    if not _module_present("torch"):
        missing.append("PyTorch is not installed")
    elif not _cuda_available():
        missing.append("CUDA GPU is not available (torch.cuda.is_available() is false)")
    if not _module_present("vibevoice"):
        missing.append("vibevoice package is not installed")
    model = os.environ.get("GEN_AUDIO_VIBEVOICE_MODEL", "").strip()
    if not model:
        missing.append("GEN_AUDIO_VIBEVOICE_MODEL is unset (expected a local VibeVoice-1.5B directory)")
    elif not Path(model).exists():
        missing.append("VibeVoice-1.5B weights are not present at GEN_AUDIO_VIBEVOICE_MODEL")
    return missing


def _missing_magpie() -> list[str]:
    missing: list[str] = []
    if not _module_present("torch"):
        missing.append("PyTorch is not installed")
    elif not _cuda_available():
        missing.append("CUDA GPU is not available (torch.cuda.is_available() is false)")
    nemo = _module_present("nemo") or _module_present("nemo.collections")
    magpie_pkg = _module_present("magpie_tts")
    if not nemo and not magpie_pkg:
        missing.append("Magpie TTS runtime is not installed (neither magpie_tts nor NeMo TTS)")
    model = os.environ.get("GEN_AUDIO_MAGPIE_MODEL", "").strip()
    if not model:
        missing.append("GEN_AUDIO_MAGPIE_MODEL is unset (expected local Magpie TTS weights)")
    elif not Path(model).exists():
        missing.append("Magpie weights are not present at GEN_AUDIO_MAGPIE_MODEL")
    if not Path("/dev/nvidia0").exists() and not _cuda_available():
        if "CUDA GPU is not available (torch.cuda.is_available() is false)" not in missing and not _module_present("torch"):
            missing.append("no NVIDIA GPU device node (/dev/nvidia0) is present on this host")
    return missing


def _synth_kokoro(turns: list[Turn], cast: CastMap, output: Path) -> dict:
    from gen_audio.synth_kokoro_onnx import KokoroOnnxSynthesizer, resolve_model_paths

    model_path, voices_path = resolve_model_paths(None, None)
    synth = KokoroOnnxSynthesizer(model_path, voices_path)
    result = synth.synthesize_turns(turns, cast, gap_s=0.15, improve_publish=False)
    write_wav(output, result.audio, result.sample_rate, subtype="FLOAT")
    return {
        "engine": "kokoro_onnx",
        "sample_rate": result.sample_rate,
        "samples": int(result.audio.size),
        "turns": len(result.turns),
    }


def _synth_pocket(turns: list[Turn], cast: CastMap, output: Path) -> dict:
    try:
        from pocket_tts import TTSModel
    except ImportError as exc:
        raise EngineRefusal("pocket_tts", ["pocket-tts package is not installed"]) from exc
    try:
        model = TTSModel.load_model()
    except Exception as exc:
        raise EngineRefusal("pocket_tts", [f"Pocket TTS model failed to load: {exc}"]) from exc
    pieces: list[np.ndarray] = []
    sample_rate = int(getattr(model, "sample_rate", 24000))
    for turn in turns:
        voice = resolve_turn(turn, cast)
        state = _pocket_voice_state(model, voice.voice)
        audio = model.generate_audio(state, turn.text)
        samples = _as_float_mono(audio)
        pieces.append(samples)
        gap = int(0.15 * sample_rate)
        if gap:
            pieces.append(np.zeros(gap, dtype=np.float32))
    if pieces and pieces[-1].size and np.all(pieces[-1] == 0):
        pieces.pop()
    if not pieces:
        raise EngineRefusal("pocket_tts", ["Pocket TTS produced no samples"])
    joined = np.concatenate(pieces).astype(np.float32)
    write_wav(output, joined, sample_rate, subtype="FLOAT")
    return {"engine": "pocket_tts", "sample_rate": sample_rate, "samples": int(joined.size), "turns": len(turns)}


def _pocket_voice_state(model, voice: str):
    """Resolve a Pocket TTS voice. Named presets are tried before a wav prompt path."""
    preset_dirs = []
    voices_root = os.environ.get("GEN_AUDIO_POCKET_VOICES", "").strip()
    if voices_root:
        preset_dirs.append(Path(voices_root))
    # Built-in voice names used by kyutai/pocket-tts. A missing preset is a refusal.
    try:
        if hasattr(model, "get_state_for_audio_prompt"):
            candidate = voice
            for root in preset_dirs:
                wav = root / f"{voice}.wav"
                if wav.is_file():
                    candidate = str(wav)
                    break
            return model.get_state_for_audio_prompt(candidate)
    except Exception as exc:
        raise EngineRefusal(
            "pocket_tts",
            [f"Pocket TTS voice {voice!r} could not be loaded: {exc}"],
        ) from exc
    raise EngineRefusal("pocket_tts", ["TTSModel.get_state_for_audio_prompt is missing"])


def _synth_vibevoice(turns: list[Turn], cast: CastMap, output: Path) -> dict:
    """Run VibeVoice-1.5B when CUDA, the package, and local weights are present."""
    import torch

    model_dir = os.environ.get("GEN_AUDIO_VIBEVOICE_MODEL", "").strip()
    if not torch.cuda.is_available():
        raise EngineRefusal("vibevoice", ["CUDA GPU is not available (torch.cuda.is_available() is false)"])
    try:
        from vibevoice.modular.modeling_vibevoice_inference import (
            VibeVoiceForConditionalGenerationInference,
        )
        from vibevoice.processor.vibevoice_processor import VibeVoiceProcessor
    except ImportError as exc:
        raise EngineRefusal("vibevoice", [f"vibevoice import failed: {exc}"]) from exc
    try:
        processor = VibeVoiceProcessor.from_pretrained(model_dir)
        model = VibeVoiceForConditionalGenerationInference.from_pretrained(
            model_dir,
            torch_dtype=torch.float16,
        ).to("cuda")
        script = "\n".join(f"Speaker {turn.speaker_id}: {turn.text}" for turn in turns)
        inputs = processor(text=script, return_tensors="pt")
        moved = {key: value.to("cuda") if hasattr(value, "to") else value for key, value in inputs.items()}
        with torch.no_grad():
            generated = model.generate(**moved)
        speech = generated.speech_outputs[0]
        audio = _as_float_mono(speech)
        rate = int(getattr(model, "sampling_rate", 24000) or 24000)
        write_wav(output, audio, rate, subtype="FLOAT")
    except EngineRefusal:
        raise
    except Exception as exc:
        raise EngineRefusal("vibevoice", [f"VibeVoice-1.5B failed to synthesize: {exc}"]) from exc
    return {"engine": "vibevoice", "sample_rate": rate, "samples": int(audio.size), "turns": len(turns)}


def _synth_magpie(turns: list[Turn], cast: CastMap, output: Path) -> dict:
    """Run Magpie TTS when NeMo or magpie_tts, CUDA, and weights are present."""
    import torch

    model_dir = os.environ.get("GEN_AUDIO_MAGPIE_MODEL", "").strip()
    if not torch.cuda.is_available():
        raise EngineRefusal("magpie", ["CUDA GPU is not available (torch.cuda.is_available() is false)"])
    try:
        if _module_present("magpie_tts"):
            import magpie_tts

            model = magpie_tts.load(model_dir)
            pieces = []
            rate = 24000
            for turn in turns:
                voice = resolve_turn(turn, cast)
                audio, rate = model.synthesize(turn.text, voice=voice.voice)
                pieces.append(_as_float_mono(audio))
            joined = np.concatenate(pieces)
            write_wav(output, joined, int(rate), subtype="FLOAT")
            return {"engine": "magpie", "sample_rate": int(rate), "samples": int(joined.size), "turns": len(turns)}
        from nemo.collections.tts.models import MagpieTTSModel

        model = MagpieTTSModel.restore_from(model_dir).cuda()
        pieces = []
        rate = int(getattr(model, "sample_rate", 22050))
        for turn in turns:
            voice = resolve_turn(turn, cast)
            audio, rate = model.do_tts(turn.text, speaker=voice.voice)
            pieces.append(_as_float_mono(audio))
        joined = np.concatenate(pieces)
        write_wav(output, joined, int(rate), subtype="FLOAT")
        return {"engine": "magpie", "sample_rate": int(rate), "samples": int(joined.size), "turns": len(turns)}
    except EngineRefusal:
        raise
    except Exception as exc:
        raise EngineRefusal("magpie", [f"Magpie TTS failed to synthesize: {exc}"]) from exc


def _as_float_mono(audio) -> np.ndarray:
    if hasattr(audio, "detach"):
        audio = audio.detach().cpu().numpy()
    values = np.asarray(audio, dtype=np.float32).reshape(-1)
    if values.size == 0:
        raise EngineRefusal("pocket_tts", ["Pocket TTS returned an empty buffer"])
    return np.nan_to_num(values, nan=0.0, posinf=0.0, neginf=0.0)
