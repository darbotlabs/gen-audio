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
    """Render ``turns`` to ``output``. Raises ``EngineRefusal`` when the engine cannot run.

    A successful adapter returns ``engine``, ``sample_rate``, ``samples``, and
    ``turns``. When it recorded per-line sample cursors it also returns
    ``speech``:

    - ``speakers``: ``[{idx, persona, voice, engine}]``, ``idx`` 0..7 in
      first-appearance order, at most 8.
    - ``segments``: ``[{speaker_idx, start_sample, end_sample}]``, half-open
      offsets into the written buffer.
    - ``sample_rate``: the rate of that buffer.

    Omit ``speech`` when the engine cannot name those offsets. The pipeline
    then stores speakers as ``"unresolved"`` and does not guess.
    """
    found = probe(engine)
    if not found.available:
        raise EngineRefusal(engine, found.missing)
    if cast.engine != engine:
        raise EngineRefusal(engine, [f"cast map engine is {cast.engine}, not {engine}"])
    if engine == "kokoro_onnx":
        return _synth_kokoro(turns, cast, output)
    if engine == "kokoro_dayour":
        return _synth_kokoro_torch(turns, cast, output)
    if engine == "vibevoice":
        return _synth_vibevoice(turns, cast, output)
    if engine in {"pocket_tts", "magpie"}:
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
        return _missing_kokoro_torch()
    return [f"unknown engine {engine}"]


def _module_present(name: str) -> bool:
    try:
        return importlib.util.find_spec(name) is not None
    except (ModuleNotFoundError, ValueError):
        return False


_KOKORO_LANG_CODE = {
    "en-us": "a",
    "en-gb": "b",
    "es": "e",
    "fr-fr": "f",
    "hi": "h",
    "it": "i",
    "ja": "j",
    "pt-br": "p",
    "zh": "z",
}
_KOKORO_G2P = {
    "en-us": "misaki.en-us",
    "en-gb": "misaki.en-gb",
    "es": "misaki.es",
    "ja": "misaki.ja",
    "zh": "misaki.zh",
}


def _missing_kokoro_torch() -> list[str]:
    from gen_audio.voices.runtime import allow_hf_download

    missing: list[str] = []
    if not _module_present("kokoro"):
        missing.append("kokoro package is not installed")
    if not _module_present("misaki"):
        missing.append("misaki is not installed; kokoro will not invent phonemes")
    if not allow_hf_download() and not os.environ.get("GENAID_VOICES_DIR", "").strip():
        missing.append(
            "Kokoro .pt packs are not bundled; GEN_AUDIO_ALLOW_HF_DOWNLOAD=1 is required before KPipeline can fetch hexgrad/Kokoro-82M"
        )
    return missing


def _synth_kokoro_torch(turns: list[Turn], cast: CastMap, output: Path) -> dict:
    """Run dayour/kokoro ``KPipeline`` on CPU. Misaki is assigned to ``pipeline.g2p``."""
    from gen_audio.voices.errors import VoiceError
    from gen_audio.voices.model import g2p_config_path, load_g2p
    from gen_audio.voices.runtime import run_kokoro_pipeline

    library = os.environ.get("GENAID_VOICES_DIR", "").strip()
    pieces: list[np.ndarray] = []
    sample_rate = 24000
    try:
        for turn in turns:
            assignment = resolve_turn(turn, cast)
            lang = assignment.lang or cast.default_lang
            code = _KOKORO_LANG_CODE.get(lang)
            g2p_id = _KOKORO_G2P.get(lang)
            if code is None or g2p_id is None:
                raise EngineRefusal("kokoro_dayour", [f"no Misaki config for language {lang}"])
            g2p = load_g2p(g2p_config_path(g2p_id))
            voice_arg: object = assignment.voice
            if library:
                pack = Path(library) / "kokoro" / "voices" / f"{assignment.voice}.pt"
                if pack.is_file():
                    voice_arg = str(pack)
            audio, sample_rate = run_kokoro_pipeline(
                turn.text,
                lang_code=code,
                voice=voice_arg,
                g2p=g2p,
                speed=assignment.speed,
            )
            pieces.append(audio)
            gap = int(0.15 * sample_rate)
            if gap:
                pieces.append(np.zeros(gap, dtype=np.float32))
    except VoiceError as exc:
        raise EngineRefusal("kokoro_dayour", [str(exc)]) from exc
    if pieces and pieces[-1].size and np.all(pieces[-1] == 0):
        pieces.pop()
    if not pieces:
        raise EngineRefusal("kokoro_dayour", ["KPipeline produced no samples"])
    joined = np.concatenate(pieces).astype(np.float32)
    write_wav(output, joined, sample_rate, subtype="FLOAT")
    return {
        "engine": "kokoro_dayour",
        "sample_rate": sample_rate,
        "samples": int(joined.size),
        "turns": len(turns),
    }


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


def _vibevoice_device() -> str:
    """Torch device for VibeVoice weights. Default ``cpu``.

    ``GEN_AUDIO_VIBEVOICE_DEVICE`` overrides it (``cpu``, ``cuda``, ``cuda:0``).
    A missing CUDA device is a refusal only when this value asks for CUDA.
    """
    raw = os.environ.get("GEN_AUDIO_VIBEVOICE_DEVICE", "cpu").strip()
    if not raw or raw.lower() == "cpu":
        return "cpu"
    return raw


def _vibevoice_dtype_name(device: str) -> str:
    """``float32`` on CPU unless ``GEN_AUDIO_VIBEVOICE_DTYPE`` is set.

    A CUDA device keeps ``float16`` when the variable is unset. Accepted names
    are ``float32``/``fp32``, ``float16``/``fp16``, and ``bfloat16``/``bf16``.
    """
    configured = os.environ.get("GEN_AUDIO_VIBEVOICE_DTYPE", "").strip().lower()
    if configured:
        return configured
    if device.lower().startswith("cpu"):
        return "float32"
    return "float16"


def _vibevoice_torch_dtype(torch, name: str):
    table = {
        "float32": torch.float32,
        "fp32": torch.float32,
        "float16": torch.float16,
        "fp16": torch.float16,
        "bfloat16": torch.bfloat16,
        "bf16": torch.bfloat16,
    }
    dtype = table.get(name)
    if dtype is None:
        raise EngineRefusal(
            "vibevoice",
            [f"GEN_AUDIO_VIBEVOICE_DTYPE {name!r} is not float32, float16, or bfloat16"],
        )
    return dtype


def _missing_vibevoice() -> list[str]:
    missing: list[str] = []
    if not _module_present("torch"):
        missing.append("PyTorch is not installed")
    if not _module_present("vibevoice"):
        missing.append("vibevoice package is not installed")
    model = os.environ.get("GEN_AUDIO_VIBEVOICE_MODEL", "").strip()
    if not model:
        missing.append("GEN_AUDIO_VIBEVOICE_MODEL is unset (expected a local VibeVoice-1.5B directory)")
    elif not Path(model).exists():
        missing.append("VibeVoice-1.5B weights are not present at GEN_AUDIO_VIBEVOICE_MODEL")
    device = _vibevoice_device()
    if device.lower().startswith("cuda") and _module_present("torch") and not _cuda_available():
        missing.append(f"GEN_AUDIO_VIBEVOICE_DEVICE is {device} and CUDA is not available")
    if os.environ.get("GEN_AUDIO_VIBEVOICE_SIZE", "1.5B").strip() == "7B" and not _cuda_available():
        missing.append("VibeVoice 7B is GPU-only")
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
        "speech": {
            "speakers": result.speakers,
            "segments": [
                {
                    "speaker_idx": segment.speaker_idx,
                    "start_sample": segment.start_sample,
                    "end_sample": segment.end_sample,
                }
                for segment in result.segments
            ],
            "sample_rate": result.sample_rate,
        },
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
        samples = _as_float_mono("pocket_tts", audio)
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


def vibevoice_profile(size: str | None = None) -> dict:
    """Render profile for 1.5B (CPU) or 7B (GPU-only). Identity is a model_ref, not one HF id."""
    chosen = (size or os.environ.get("GEN_AUDIO_VIBEVOICE_SIZE", "1.5B")).strip()
    if chosen == "7B":
        return {
            "id": "7B",
            "local_env": "GEN_AUDIO_VIBEVOICE_7B_MODEL",
            "upstream": "vibevoice/VibeVoice-7B",
            "device_policy": "gpu",
            "cfg_scale": 1.3,
            "diffusion_steps": 20,
            "max_speakers": 4,
            "output_sample_rate": 24000,
            "max_position_embeddings": 32768,
        }
    if chosen != "1.5B":
        raise EngineRefusal("vibevoice", [f"unknown VibeVoice size {chosen}"])
    return {
        "id": "1.5B",
        "local_env": "GEN_AUDIO_VIBEVOICE_MODEL",
        "upstream": "microsoft/VibeVoice-1.5B",
        "device_policy": "cpu",
        "cfg_scale": 1.3,
        "diffusion_steps": 20,
        "max_speakers": 4,
        "output_sample_rate": 24000,
        "max_position_embeddings": 65536,
    }


def vibevoice_call_plan(profile: dict, script: str, wavs: list) -> dict:
    """The processor and generate arguments VibeVoice 1.5B/7B actually require.

    ``voice_samples`` is one list per script, speakers in first-appearance order.
    ``set_ddpm_inference_steps`` is called before ``generate``. ``tokenizer`` is
    the processor tokenizer. 7B is refused on CPU before this plan is used.
    """
    if profile["device_policy"] == "gpu" and not _cuda_available():
        raise EngineRefusal("vibevoice", ["VibeVoice 7B is GPU-only"])
    if len(wavs) > int(profile["max_speakers"]):
        raise EngineRefusal("vibevoice", [f"VibeVoice supports at most {profile['max_speakers']} speakers"])
    return {
        "processor": {
            "text": [script],
            "voice_samples": [wavs],
            "padding": True,
            "return_tensors": "pt",
            "return_attention_mask": True,
        },
        "diffusion_steps": int(profile["diffusion_steps"]),
        "generate": {
            "cfg_scale": float(profile["cfg_scale"]),
            "tokenizer_from": "processor.tokenizer",
            "generation_config": {"do_sample": False},
        },
    }


def run_vibevoice_generate(model, processor, inputs: dict, steps: int, cfg_scale: float):
    """Call the inference model the way the 1.5B path is specified."""
    model.set_ddpm_inference_steps(int(steps))
    return model.generate(
        **inputs,
        cfg_scale=float(cfg_scale),
        tokenizer=processor.tokenizer,
        generation_config={"do_sample": False},
    )


def _synth_vibevoice(turns: list[Turn], cast: CastMap, output: Path) -> dict:
    """Run VibeVoice when local weights load. 1.5B may use CPU. 7B refuses without a GPU."""
    import torch

    profile = vibevoice_profile()
    model_dir = os.environ.get(profile["local_env"], "").strip()
    if profile["device_policy"] == "gpu" and not _cuda_available():
        raise EngineRefusal("vibevoice", ["VibeVoice 7B is GPU-only"])
    device = "cpu" if profile["device_policy"] == "cpu" else _vibevoice_device()
    try:
        from vibevoice.modular.modeling_vibevoice_inference import (
            VibeVoiceForConditionalGenerationInference,
        )
        from vibevoice.processor.vibevoice_processor import VibeVoiceProcessor
    except ImportError as exc:
        raise EngineRefusal("vibevoice", [f"vibevoice import failed: {exc}"]) from exc
    try:
        processor = VibeVoiceProcessor.from_pretrained(model_dir)
        dtype_name = _vibevoice_dtype_name(device)
        load_kwargs = {
            "torch_dtype": _vibevoice_torch_dtype(torch, dtype_name),
            "attn_implementation": "sdpa",
        }
        if device == "cpu":
            load_kwargs["device_map"] = "cpu"
        model = VibeVoiceForConditionalGenerationInference.from_pretrained(model_dir, **load_kwargs)
        if device != "cpu":
            model = model.to(device)
        script = "\n".join(f"Speaker {turn.speaker_id}: {turn.text}" for turn in turns)
        wavs = []
        seen: set[str] = set()
        for turn in turns:
            if turn.speaker_id in seen:
                continue
            seen.add(turn.speaker_id)
            wavs.append(_load_vibevoice_prompt(turn, cast))
        plan = vibevoice_call_plan(profile, script, wavs)
        inputs = processor(**plan["processor"])
        moved = {key: value.to(device) if hasattr(value, "to") else value for key, value in inputs.items()}
        with torch.no_grad():
            generated = run_vibevoice_generate(
                model,
                processor,
                moved,
                plan["diffusion_steps"],
                plan["generate"]["cfg_scale"],
            )
        speech = generated.speech_outputs[0]
        audio = _as_float_mono("vibevoice", speech)
        rate = int(profile["output_sample_rate"])
        write_wav(output, audio, rate, subtype="FLOAT")
    except EngineRefusal:
        raise
    except Exception as exc:
        raise EngineRefusal("vibevoice", [f"VibeVoice {profile['id']} failed to synthesize: {exc}"]) from exc
    return {"engine": "vibevoice", "sample_rate": rate, "samples": int(audio.size), "turns": len(turns)}


def _load_vibevoice_prompt(turn: Turn, cast: CastMap):
    """Load one reference WAV. The cast voice is a file stem or a path."""
    from gen_audio.audio_io import read_wav

    assignment = resolve_turn(turn, cast)
    candidate = Path(assignment.voice)
    if not candidate.is_file():
        library = os.environ.get("GENAID_VOICES_DIR", "").strip()
        if library:
            candidate = Path(library) / "_hf_vibevoice_samples" / "voices" / f"{assignment.voice}.wav"
    if not candidate.is_file():
        raise EngineRefusal("vibevoice", [f"reference wav for {assignment.voice} was not found"])
    audio, _rate = read_wav(candidate)
    return audio


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
                pieces.append(_as_float_mono("magpie", audio))
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
            pieces.append(_as_float_mono("magpie", audio))
        joined = np.concatenate(pieces)
        write_wav(output, joined, int(rate), subtype="FLOAT")
        return {"engine": "magpie", "sample_rate": int(rate), "samples": int(joined.size), "turns": len(turns)}
    except EngineRefusal:
        raise
    except Exception as exc:
        raise EngineRefusal("magpie", [f"Magpie TTS failed to synthesize: {exc}"]) from exc


def _as_float_mono(engine: str, audio) -> np.ndarray:
    if hasattr(audio, "detach"):
        audio = audio.detach().cpu().numpy()
    values = np.asarray(audio, dtype=np.float32).reshape(-1)
    if values.size == 0:
        raise EngineRefusal(engine, [f"{engine} returned an empty buffer"])
    return np.nan_to_num(values, nan=0.0, posinf=0.0, neginf=0.0)
