"""Direct local PersonaPlex streaming API, never moshi.offline's hidden Hub call.

Raw PCM voice references deliberately bypass the unrestricted pickle loader in
LMGen.load_voice_prompt_embeddings. No .pt path is ever passed to upstream code.
"""

from __future__ import annotations

import hashlib
import math

from .checkpoints import current_headers, validate_meta_model
from .contracts import GIB, MAX_INPUT_TOKENS, MAX_REFERENCE_SECONDS, Request, SAMPLE_RATE, WorkerError
from .runtime import check_deadline, checked_audio, read_pcm
from .execution import recheck_memory, attention_context


class PrecisionCodec:
    """Keep FP32 codec attention usable with a half-precision fused LM policy."""

    def __init__(self, model, execution):
        self.model = model
        policy = execution["components"]["audio_codec"]
        self.execution = {**execution, "precision": policy["weight_precision"], "attention": policy["attention_policy"]}

    def __getattr__(self, name):
        return getattr(self.model, name)

    def encode(self, *args, **kwargs):
        with attention_context(self.execution):
            return self.model.encode(*args, **kwargs)

    def decode(self, *args, **kwargs):
        with attention_context(self.execution):
            return self.model.decode(*args, **kwargs)


def load(request: Request, report: dict, deadline: float):
    import sentencepiece
    import torch
    from moshi.models import loaders
    tokenizer = sentencepiece.SentencePieceProcessor(report["assets"]["tokenizer"])
    root = request.model_dir.resolve()
    check_deadline(deadline)
    recheck_memory(report)
    execution = report["execution"]
    device, dtype = execution["torch_device"], getattr(torch, execution["precision"])
    # The pinned filename=None API constructs only the architecture. Passing
    # meta explicitly avoids both loading and the default depformer copying.
    lm = loaders.get_moshi_lm(None, copy_missing_weights=False, device="meta",
                             dtype=dtype, cpu_offload=False)
    headers = current_headers(report["assets"]["weights"])
    lm_path = str((root / "model.safetensors").resolve())
    lm_headers = [header for header in headers if header["path"] == lm_path]
    expected_bytes = validate_meta_model(lm, lm_headers)
    check_deadline(deadline)
    codec_bytes = sum(header["fp32_bytes"] for header in headers if header not in lm_headers)
    recheck_memory(report, minimum_cpu_bytes=(expected_bytes + codec_bytes + sum(header["bytes"] for header in headers) + 6 * GIB)
                   if execution["device"] == "cpu" and execution["precision"] == "float32" else 0)
    mimi_path = str(root / "tokenizer-e351c8d8-checkpoint125.safetensors")
    # get_mimi uses safetensors.load_model's strict=True default.
    mimi = loaders.get_mimi(mimi_path, device=device)
    # Stage one tensor at a time, honoring the full device index. The
    # upstream loader materializes a full checkpoint before conversion and uses
    # strict=False; our validated architecture must load every key exactly.
    from safetensors import safe_open
    state = {}
    # Modern safetensors' positioned reads avoid Windows/ROCm access violations
    # when mapping this monolithic 16.7 GB checkpoint. Original CPU packages
    # retain their supported mmap API; no whole-checkpoint byte copy is needed.
    storage = {"backend": "pread"} if report["runtime"].get("profile") == "personaplex-modern" else {}
    with safe_open(lm_path, framework="pt", device="cpu", **storage) as checkpoint:
        for name in checkpoint.keys():
            check_deadline(deadline)
            state[name] = checkpoint.get_tensor(name).to(device=device, dtype=dtype)
    lm.load_state_dict(state, strict=True, assign=True)
    del state
    lm.eval()
    if any(parameter.is_meta for parameter in lm.parameters()):
        raise WorkerError("invalid_weights", "PersonaPlex checkpoint left unloaded model parameters")
    for name, model in (("language_model", lm), ("audio_codec", mimi)):
        actual = {str(parameter.dtype).removeprefix("torch.") for parameter in model.parameters() if parameter.is_floating_point()}
        expected = execution["components"][name]["weight_precision"]
        if actual != {expected}:
            raise WorkerError("unsupported_precision", f"{name} weight precision differs from its execution contract")
    if mimi.sample_rate != SAMPLE_RATE or mimi.frame_rate != 12.5:
        raise WorkerError("unsupported_model", "Unexpected PersonaPlex codec sample/frame rate")
    frame_size = int(mimi.sample_rate / mimi.frame_rate)
    return {"lm": lm, "mimi": PrecisionCodec(mimi, execution), "tokenizer": tokenizer, "frame_size": frame_size, "execution": execution}


def generate(request: Request, report: dict, deadline: float, *, resident=None):
    import numpy as np
    import torch
    from moshi.models import LMGen
    from moshi.models.lm import normalize_audio

    torch.manual_seed(request.seed)
    torch.set_num_threads(min(8, torch.get_num_threads()))
    user_audio = read_pcm(request.input_audio, seconds=request.max_seconds, allow_silence=True)
    voice_audio = read_pcm(request.voice_refs[0], seconds=MAX_REFERENCE_SECONDS)
    voice_audio = checked_audio(normalize_audio(voice_audio, SAMPLE_RATE, -24.0),
                                max_seconds=MAX_REFERENCE_SECONDS)
    runtime = resident if resident is not None else load(request, report, deadline)
    lm, mimi = runtime["lm"], runtime["mimi"]
    tokenizer, frame_size = runtime["tokenizer"], runtime["frame_size"]
    persona = request.persona.strip()
    if not (persona.startswith("<system>") and persona.endswith("<system>")):
        persona = f"<system> {persona} <system>"
    text_tokens = tokenizer.encode(persona)
    if not text_tokens or len(text_tokens) > MAX_INPUT_TOKENS:
        raise WorkerError("resource_limit", "Persona token count must be 1..4096")
    steps = math.ceil(user_audio.size / frame_size)
    if steps > request.max_new_tokens:
        raise WorkerError("resource_limit", "Conversation exceeds the stream-step limit")

    class BoundedLMGen(LMGen):
        def step(self, *args, **kwargs):
            check_deadline(deadline)
            return super().step(*args, **kwargs)

    generator = BoundedLMGen(
        lm, audio_silence_frame_cnt=int(0.5 * mimi.frame_rate),
        sample_rate=SAMPLE_RATE, device=report["device"], frame_rate=mimi.frame_rate,
        save_voice_prompt_embeddings=False, use_sampling=False,
        temp=0.8, temp_text=0.7, top_k=250, top_k_text=25,
    )
    frames, generated_text = [], []
    total = 0
    # Moshi's lazy compiled helpers require Triton, which is not supplied by
    # the Windows ROCm runtime. Eager tensor operations and HIP graphs remain
    # accelerated; this supported context avoids global monkey-patching.
    from moshi.utils.compile import no_compile
    with torch.inference_mode(), no_compile(), attention_context(report["execution"]), mimi.streaming(1), generator.streaming(1):
        for _ in range(4):
            check_deadline(deadline)
            chunk = torch.zeros(1, 1, frame_size, dtype=torch.float32, device=report["device"])
            codes = mimi.encode(chunk)
            for index in range(codes.shape[-1]):
                tokens = generator.step(codes[:, :, index:index + 1])
                if tokens is not None:
                    mimi.decode(tokens[:, 1:9])
        # These are the same fields populated by the reviewed load_voice_prompt,
        # after normalization; using arrays keeps all I/O bounded and local.
        generator.voice_prompt_audio = voice_audio[None, :]
        generator.voice_prompt_cache = None
        generator.voice_prompt_embeddings = None
        generator.text_prompt_tokens = text_tokens
        mimi.reset_streaming()
        generator.reset_streaming()
        generator.step_system_prompts(mimi)
        mimi.reset_streaming()
        for offset in range(0, user_audio.size, frame_size):
            check_deadline(deadline)
            chunk = user_audio[offset:offset + frame_size]
            if chunk.size < frame_size:
                chunk = np.pad(chunk, (0, frame_size - chunk.size))
            codes = mimi.encode(torch.as_tensor(chunk, dtype=torch.float32, device=report["device"])[None, None, :])
            for index in range(codes.shape[-1]):
                tokens = generator.step(codes[:, :, index:index + 1])
                if tokens is None:
                    continue
                pcm = mimi.decode(tokens[:, 1:9]).detach().float().cpu().numpy()[0, 0]
                pcm = checked_audio(pcm, max_seconds=frame_size / SAMPLE_RATE, allow_silence=True)
                remaining = min(user_audio.size, int(request.max_seconds * SAMPLE_RATE)) - total
                if remaining <= 0:
                    raise WorkerError("resource_limit", "PersonaPlex generated more frames than the input duration")
                pcm = pcm[:remaining]
                frames.append(pcm)
                total += pcm.size
                text_token = tokens[0, 0, 0].item()
                if text_token not in (0, 1, 2, 3):
                    generated_text.append(tokenizer.id_to_piece(text_token).replace("▁", " "))
    check_deadline(deadline)
    if not frames:
        raise WorkerError("no_generated_speech", "PersonaPlex returned no decoded speech frames")
    audio = checked_audio(np.concatenate(frames), max_seconds=request.max_seconds)
    return audio, {
        "input_audio": str(request.input_audio.resolve()),
        "tokenizer": report["assets"]["tokenizer"],
        "persona_sha256": hashlib.sha256(persona.encode("utf-8")).hexdigest(),
        "voice_references": [str(request.voice_refs[0].resolve())],
        "voice_format": "raw_pcm_wav", "stream_steps": steps,
        "codec_precision": "float32", "codec_attention": runtime["mimi"].execution["attention"],
        "generated_text": "".join(generated_text),
        "generated_text_semantics": "model_response_not_input_transcription",
        "limit_reached": False,
    }
