"""Direct API adapter for the pinned community VibeVoice implementation."""

from __future__ import annotations

import hashlib
import math
from copy import deepcopy
from pathlib import Path

from .checkpoints import current_headers, validate_meta_model, vibevoice_architecture
from .contracts import GIB, MAX_INPUT_TOKENS, MAX_REFERENCE_SECONDS, Request, WorkerError
from .preflight import normalized_script
from .runtime import check_deadline, checked_audio, read_pcm
from .execution import recheck_memory, attention_context


def load(request: Request, report: dict, deadline: float):
    import torch
    from accelerate import init_empty_weights
    try:
        from transformers.initialization import no_init_weights
    except ImportError:  # Original CPU runtime before Transformers 5.
        from transformers.modeling_utils import no_init_weights
    from vibevoice.modular.configuration_vibevoice import VibeVoiceConfig
    from vibevoice.modular.modular_vibevoice_text_tokenizer import VibeVoiceTextTokenizerFast
    from vibevoice.modular.modeling_vibevoice_inference import VibeVoiceForConditionalGenerationInference
    from vibevoice.processor.vibevoice_processor import VibeVoiceProcessor
    from vibevoice.processor.vibevoice_tokenizer_processor import VibeVoiceTokenizerProcessor

    torch.manual_seed(request.seed)
    torch.set_num_threads(min(8, torch.get_num_threads()))
    assets = report["assets"]
    tokenizer = VibeVoiceTextTokenizerFast.from_pretrained(
        assets["tokenizer"], local_files_only=True, trust_remote_code=False,
    )
    vocab_size = assets["config"]["decoder_config"]["vocab_size"]
    for token in (tokenizer.speech_start_id, tokenizer.speech_end_id,
                  tokenizer.speech_diffusion_id, tokenizer.eos_token_id, tokenizer.pad_id):
        if not isinstance(token, int) or not 0 <= token < vocab_size:
            raise WorkerError("incompatible_tokenizer", "Local tokenizer's special tokens do not fit the model vocabulary")
    if len(tokenizer) > vocab_size:
        raise WorkerError("incompatible_tokenizer", "Local tokenizer exceeds the model embedding vocabulary")
    config = assets["processor_config"]
    audio_config = config.get("audio_processor", {})
    audio_processor = VibeVoiceTokenizerProcessor(
        sampling_rate=24000, normalize_audio=audio_config.get("normalize_audio", True),
        target_dB_FS=audio_config.get("target_dB_FS", -25), eps=audio_config.get("eps", 1e-6),
    )
    # Do not call VibeVoiceProcessor.from_pretrained: this revision ignores the
    # tokenizer subdirectory and falls back to a Hub Qwen ID.
    processor = VibeVoiceProcessor(
        tokenizer=tokenizer, audio_processor=audio_processor,
        speech_tok_compress_ratio=3200, db_normalize=config.get("db_normalize", True),
    )
    check_deadline(deadline)
    recheck_memory(report)
    execution = report["execution"]
    dtype = getattr(torch, execution["precision"])
    device = execution["torch_device"]
    architecture = vibevoice_architecture(assets["config"])
    config = VibeVoiceConfig.from_dict(deepcopy(assets["config"]))
    # Automatic precision follows the checkpoint; explicit user policies can
    # still request a different compute dtype without rewriting config.json.
    if execution["requested"]["precision"] != "auto" or execution.get("precision_source") != "model_config":
        config.torch_dtype = dtype
        for name in ("decoder_config", "acoustic_tokenizer_config", "semantic_tokenizer_config", "diffusion_head_config"):
            getattr(config, name).torch_dtype = dtype
    # Keep the tiny scheduler/codec buffers on CPU: this pinned codec reads
    # scalars from linspace during construction. Only parameters must be meta.
    with no_init_weights(), init_empty_weights(include_buffers=False):
        empty_model = VibeVoiceForConditionalGenerationInference(config)
    empty_model.tie_weights()
    headers = current_headers(assets["weights"])
    expected_bytes = validate_meta_model(empty_model, headers, architecture["aliases"])
    del empty_model
    check_deadline(deadline)
    recheck_memory(report, minimum_cpu_bytes=(expected_bytes + sum(header["bytes"] for header in headers) + 6 * GIB)
                   if execution["device"] == "cpu" and execution["precision"] == "float32" else 0)
    attention_options = {"attn_implementation": "sdpa"} if execution.get("attention", "auto") != "auto" else {}
    model, loading = VibeVoiceForConditionalGenerationInference.from_pretrained(
        str(request.model_dir.resolve()), torch_dtype=dtype, device_map=device,
        **attention_options, local_files_only=True, trust_remote_code=False,
        use_safetensors=True, weights_only=True, low_cpu_mem_usage=True,
        output_loading_info=True, config=config,
    )
    if any(loading.get(key) for key in ("missing_keys", "unexpected_keys", "mismatched_keys", "error_msgs")):
        raise WorkerError("invalid_weights", "Checkpoint did not load completely")
    model.eval()
    from .attention import configure
    configure(model, execution)
    if any(parameter.is_meta for parameter in model.parameters()):
        raise WorkerError("invalid_weights", "VibeVoice left unloaded model parameters")
    return {"model": model, "processor": processor, "tokenizer": tokenizer, "execution": execution}


def generate(request: Request, report: dict, deadline: float, *, resident=None):
    import torch
    torch.manual_seed(request.seed)
    runtime = resident if resident is not None else load(request, report, deadline)
    model, processor, tokenizer = runtime["model"], runtime["processor"], runtime["tokenizer"]
    assets = report["assets"]
    vocab_size = assets["config"]["decoder_config"]["vocab_size"]
    script = normalized_script(request.text, len(request.voice_refs))
    refs = [read_pcm(path, seconds=MAX_REFERENCE_SECONDS) for path in request.voice_refs]
    inputs = processor(text=[script], voice_samples=[refs], padding=True,
                       return_tensors="pt", return_attention_mask=True)
    count = int(inputs["input_ids"].shape[-1])
    token_cap = min(request.max_new_tokens, math.floor(request.max_seconds * 24000 / 3200))
    if count > MAX_INPUT_TOKENS or count + token_cap > assets["config"]["decoder_config"]["max_position_embeddings"]:
        raise WorkerError("resource_limit", "Tokenized request exceeds input/context token limit; text was not truncated")
    if inputs["input_ids"].min().item() < 0 or inputs["input_ids"].max().item() >= vocab_size:
        raise WorkerError("incompatible_tokenizer", "Tokenized input contains out-of-vocabulary IDs")
    for name, value in inputs.items():
        if torch.is_tensor(value):
            if getattr(value, "is_floating_point", lambda: False)():
                inputs[name] = value.to(device=report["device"], dtype=getattr(torch, report["dtype"]))
            else:
                inputs[name] = value.to(report["device"])

    def stop_check():
        check_deadline(deadline)
        return False

    with torch.inference_mode(), attention_context(report["execution"]):
        outputs = model.generate(
            **inputs, max_new_tokens=token_cap, tokenizer=tokenizer,
            is_prefill=True,
            verbose=False, show_progress_bar=False, stop_check_fn=stop_check,
            max_length_times=token_cap,
        )
    check_deadline(deadline)
    if not outputs.speech_outputs or outputs.speech_outputs[0] is None:
        raise WorkerError("no_generated_speech", "VibeVoice returned no decoded speech")
    audio = checked_audio(outputs.speech_outputs[0], max_seconds=request.max_seconds)
    generated_tokens = int(outputs.sequences.shape[-1]) - count
    return audio, {
        "input_text_sha256": hashlib.sha256(script.encode("utf-8")).hexdigest(),
        "voice_references": [str(path.resolve()) for path in request.voice_refs],
        "tokenizer": str(Path(assets["tokenizer"]).resolve()),
        "input_tokens": count, "generated_tokens": generated_tokens,
        "effective_max_new_tokens": token_cap, "limit_reached": generated_tokens >= token_cap,
        "attention": model.config.decoder_config._attn_implementation,
        "attention_policy": report["execution"].get("attention", "auto"),
        "ddpm_inference_steps": assets["config"]["diffusion_head_config"]["ddpm_num_inference_steps"],
        "generation_settings_source": "model_config_and_runtime_defaults",
    }
