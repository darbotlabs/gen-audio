"""Bounded, data-free checkpoint checks for the pinned runtime architectures."""

from __future__ import annotations

import math

from .contracts import WorkerError


_METADATA = {
    "_name_or_path", "architectures", "model_type", "torch_dtype",
    "transformers_version", "_attn_implementation", "_attn_implementation_autoset",
}
_EMBEDDING_ALIASES = ("model.language_model.embed_tokens.weight", "lm_head.weight")


def _supported(config, keys):
    if not isinstance(config, dict) or set(config) - keys - _METADATA:
        raise WorkerError("unsupported_model", "Unsupported architecture configuration fields")
    if config.get("torch_dtype") not in (None, "float32", "float16", "bfloat16"):
        raise WorkerError("unsupported_model", "Unsupported architecture dtype")


def _integer(config, key, maximum, minimum=1):
    value = config.get(key)
    if type(value) is not int or not minimum <= value <= maximum:
        raise WorkerError("unsupported_model", f"Invalid architecture dimension: {key}")
    return value


def _fixed(config, values):
    for key, expected in values.items():
        value = config.get(key, expected)
        if (type(value) is not type(expected) or value != expected
                or isinstance(expected, list) and any(type(item) is not type(want)
                                                       for item, want in zip(value, expected))):
            raise WorkerError("unsupported_model", f"Unsupported architecture setting: {key}")


def _codec_shapes(config, prefix, *, acoustic):
    _supported(config, {
        "channels", "corpus_normalize", "causal", "vae_dim", "fix_std", "std_dist_type",
        "mixer_layer", "conv_norm", "pad_mode", "disable_last_norm", "layernorm",
        "layernorm_eps", "layernorm_elementwise_affine", "conv_bias",
        "layer_scale_init_value", "weight_init_value", "encoder_n_filters",
        "encoder_ratios", "encoder_depths",
    } | ({"decoder_n_filters", "decoder_ratios", "decoder_depths"} if acoustic else set()))
    _fixed(config, {
        "model_type": f"vibevoice_{'acoustic' if acoustic else 'semantic'}_tokenizer",
        "channels": 1, "causal": True, "mixer_layer": "depthwise_conv",
        "conv_norm": "none", "pad_mode": "constant", "disable_last_norm": True,
        "layernorm": "RMSNorm", "layernorm_elementwise_affine": True, "conv_bias": True,
        "encoder_ratios": [8, 5, 5, 4, 2, 2],
    })
    scale = config.get("layer_scale_init_value", 1e-6)
    if type(scale) not in (float, int) or not math.isfinite(scale) or not 0 < scale <= 1:
        raise WorkerError("unsupported_model", "Unsupported codec layer scaling")
    latent = _integer(config, "vae_dim", 256)
    filters = _integer(config, "encoder_n_filters", 64)
    depths = config.get("encoder_depths")
    if not isinstance(depths, str) or len(depths) > 32:
        raise WorkerError("unsupported_model", "Expected explicit codec encoder depths")
    try:
        depths = [int(value) for value in depths.split("-")]
    except ValueError as exc:
        raise WorkerError("unsupported_model", "Invalid codec encoder depths") from exc
    if len(depths) != 7 or any(not 1 <= value <= 16 for value in depths):
        raise WorkerError("unsupported_model", "Unsupported codec encoder depths")
    shapes = {}

    def conv(name, out_dim, in_dim, kernel, *, transpose=False):
        suffix = "convtr.convtr" if transpose else "conv.conv"
        shapes[f"{name}.{suffix}.weight"] = (
            (in_dim, out_dim, kernel) if transpose else (out_dim, in_dim, kernel)
        )
        shapes[f"{name}.{suffix}.bias"] = (out_dim,)

    def stages(name, widths, layer_depths):
        for stage, (width, depth) in enumerate(zip(widths, layer_depths)):
            for layer in range(depth):
                base = f"{name}.stages.{stage}.{layer}"
                for suffix in ("norm.weight", "ffn_norm.weight", "gamma", "ffn_gamma"):
                    shapes[f"{base}.{suffix}"] = (width,)
                conv(f"{base}.mixer.conv", width, 1, 7)
                shapes[f"{base}.ffn.linear1.weight"] = (4 * width, width)
                shapes[f"{base}.ffn.linear1.bias"] = (4 * width,)
                shapes[f"{base}.ffn.linear2.weight"] = (width, 4 * width)
                shapes[f"{base}.ffn.linear2.bias"] = (width,)

    widths = [filters * 2**index for index in range(7)]
    encoder = f"{prefix}.encoder"
    conv(f"{encoder}.downsample_layers.0.0", filters, 1, 7)
    for index, ratio in enumerate((2, 2, 4, 5, 5, 8)):
        conv(f"{encoder}.downsample_layers.{index + 1}.0", widths[index + 1], widths[index], ratio * 2)
    stages(encoder, widths, depths)
    conv(f"{encoder}.head", latent, widths[-1], 7)
    if acoustic:
        _fixed(config, {"decoder_depths": None})
        ratios = config.get("decoder_ratios")
        if ratios is not None and ratios != [8, 5, 5, 4, 2, 2]:
            raise WorkerError("unsupported_model", "Unsupported codec decoder ratios")
        filters = _integer(config, "decoder_n_filters", 64)
        widths = [filters * 2**index for index in reversed(range(7))]
        decoder = f"{prefix}.decoder"
        conv(f"{decoder}.upsample_layers.0.0", widths[0], latent, 7)
        for index, ratio in enumerate((8, 5, 5, 4, 2, 2)):
            conv(f"{decoder}.upsample_layers.{index + 1}.0", widths[index + 1],
                 widths[index], ratio * 2, transpose=True)
        stages(decoder, widths, list(reversed(depths)))
        conv(f"{decoder}.head", 1, widths[-1], 7)
    return shapes, latent


def vibevoice_architecture(config):
    """Static schema for the supported Qwen2/depthwise-codec/DDPM topology.

    This is an early fail-closed gate, not a substitute for the runtime's meta
    state_dict check. Count both embeddings even when tied: the constructor
    initially creates both, and counting aliases conservatively costs no RAM.
    """
    _supported(config, {
        "decoder_config", "acoustic_tokenizer_config", "semantic_tokenizer_config",
        "diffusion_head_config", "acoustic_vae_dim", "acostic_vae_dim",
        "semantic_vae_dim", "tie_word_embeddings",
    })
    _fixed(config, {"model_type": "vibevoice"})
    decoder = config.get("decoder_config")
    _supported(decoder, {
        "vocab_size", "max_position_embeddings", "hidden_size", "intermediate_size",
        "num_hidden_layers", "num_attention_heads", "num_key_value_heads",
        "head_dim", "attention_dropout", "hidden_act", "initializer_range",
        "max_window_layers", "rms_norm_eps", "rope_scaling", "rope_theta",
        "sliding_window", "tie_word_embeddings", "use_cache", "use_mrope", "use_sliding_window",
    })
    _fixed(decoder, {"model_type": "qwen2", "hidden_act": "silu",
                     "rope_scaling": None, "use_sliding_window": False, "use_mrope": False})
    width = _integer(decoder, "hidden_size", 8192)
    ffn = _integer(decoder, "intermediate_size", 65536)
    layers = _integer(decoder, "num_hidden_layers", 80)
    heads = _integer(decoder, "num_attention_heads", 64)
    kv_heads = _integer(decoder, "num_key_value_heads", heads)
    vocab = _integer(decoder, "vocab_size", 200000)
    _integer(decoder, "max_position_embeddings", 131072)
    if width % heads or heads % kv_heads or (width // heads) % 2:
        raise WorkerError("unsupported_model", "Incompatible Qwen2 attention dimensions")
    _fixed(decoder, {"head_dim": width // heads})
    for section in (config, decoder):
        if type(section.get("tie_word_embeddings", True)) is not bool:
            raise WorkerError("unsupported_model", "Invalid embedding tying configuration")
    kv = width // heads * kv_heads
    shapes = {
        _EMBEDDING_ALIASES[0]: (vocab, width), _EMBEDDING_ALIASES[1]: (vocab, width),
        "model.language_model.norm.weight": (width,),
        "model.speech_scaling_factor": (), "model.speech_bias_factor": (),
    }
    for index in range(layers):
        base = f"model.language_model.layers.{index}"
        for name, out_dim in (("q_proj", width), ("k_proj", kv), ("v_proj", kv)):
            shapes[f"{base}.self_attn.{name}.weight"] = (out_dim, width)
            shapes[f"{base}.self_attn.{name}.bias"] = (out_dim,)
        shapes[f"{base}.self_attn.o_proj.weight"] = (width, width)
        for name, shape in (("gate_proj", (ffn, width)), ("up_proj", (ffn, width)),
                            ("down_proj", (width, ffn))):
            shapes[f"{base}.mlp.{name}.weight"] = shape
        for name in ("input_layernorm", "post_attention_layernorm"):
            shapes[f"{base}.{name}.weight"] = (width,)
    for kind in ("acoustic", "semantic"):
        codec, latent = _codec_shapes(config.get(f"{kind}_tokenizer_config"),
                                      f"model.{kind}_tokenizer", acoustic=kind == "acoustic")
        _fixed(config, {f"{kind}_vae_dim": latent})
        shapes.update(codec)
        base = f"model.{kind}_connector"
        shapes.update({
            f"{base}.fc1.weight": (width, latent), f"{base}.fc1.bias": (width,),
            f"{base}.norm.weight": (width,), f"{base}.fc2.weight": (width, width),
            f"{base}.fc2.bias": (width,),
        })
    diffusion = config.get("diffusion_head_config")
    _supported(diffusion, {
        "hidden_size", "head_layers", "head_ffn_ratio", "rms_norm_eps", "latent_size",
        "speech_vae_dim", "prediction_type", "diffusion_type", "ddpm_num_steps",
        "ddpm_num_inference_steps", "ddpm_beta_schedule", "ddpm_batch_mul",
    })
    latent = config["acoustic_tokenizer_config"]["vae_dim"]
    _integer(diffusion, "hidden_size", 8192)
    _integer(diffusion, "latent_size", 256)
    _fixed(diffusion, {
        "model_type": "vibevoice_diffusion_head", "hidden_size": width,
        "latent_size": latent, "diffusion_type": "ddpm", "prediction_type": "v_prediction",
        "ddpm_beta_schedule": "cosine",
    })
    depth = _integer(diffusion, "head_layers", 16)
    _integer(diffusion, "ddpm_num_steps", 1000)
    _integer(diffusion, "ddpm_num_inference_steps", 1000)
    ratio = diffusion.get("head_ffn_ratio")
    if type(ratio) not in (int, float) or not math.isfinite(ratio) or not 1 <= ratio <= 8:
        raise WorkerError("unsupported_model", "Invalid diffusion FFN ratio")
    ffn = int(width * ratio)
    base = "model.prediction_head"
    shapes.update({
        f"{base}.noisy_images_proj.weight": (width, latent),
        f"{base}.cond_proj.weight": (width, width),
        f"{base}.t_embedder.mlp.0.weight": (width, 256),
        f"{base}.t_embedder.mlp.2.weight": (width, width),
        f"{base}.final_layer.linear.weight": (latent, width),
        f"{base}.final_layer.adaLN_modulation.1.weight": (2 * width, width),
    })
    for index in range(depth):
        layer = f"{base}.layers.{index}"
        shapes.update({
            f"{layer}.ffn.gate_proj.weight": (ffn, width),
            f"{layer}.ffn.up_proj.weight": (ffn, width),
            f"{layer}.ffn.down_proj.weight": (width, ffn),
            f"{layer}.norm.weight": (width,),
            f"{layer}.adaLN_modulation.1.weight": (3 * width, width),
        })
    aliases = (_EMBEDDING_ALIASES,) if config.get("tie_word_embeddings", True) else ()
    return {"shapes": shapes, "aliases": aliases, "fp32_bytes": shape_bytes(shapes),
            "kv_bytes_per_token": 4 * layers * kv * 4}


def shape_bytes(shapes):
    return sum(math.prod(shape) * 4 for shape in shapes.values())


def validate_shapes(tensors, expected, aliases=()):
    missing = set(expected) - set(tensors)
    for group in aliases:
        if any(name in tensors for name in group):
            missing.difference_update(group)
    unexpected = set(tensors) - set(expected)
    mismatched = [name for name in set(tensors) & set(expected)
                  if tuple(tensors[name]["shape"]) != tuple(expected[name])]
    if missing or unexpected or mismatched:
        raise WorkerError(
            "invalid_weights",
            f"Checkpoint architecture mismatch: {len(missing)} missing, "
            f"{len(unexpected)} unexpected, {len(mismatched)} wrong-shaped tensors",
        )


def combine_headers(headers):
    tensors = {}
    for header in headers:
        if set(tensors) & set(header["tensors"]):
            raise WorkerError("invalid_weights", "Duplicate checkpoint tensors across shards")
        tensors.update(header["tensors"])
    return tensors


def current_headers(weights):
    from pathlib import Path
    from .preflight import safetensors_header

    headers = [safetensors_header(Path(weight["path"])) for weight in weights]
    if any(header["bytes"] != weight["bytes"] or header["fp32_bytes"] != weight["fp32_bytes"]
           for header, weight in zip(headers, weights)):
        raise WorkerError("invalid_weights", "Checkpoint metadata changed after preflight")
    return headers


def validate_meta_model(model, headers, aliases=()):
    if not all(parameter.is_meta for parameter in model.parameters()):
        raise WorkerError("unsupported_model", "Architecture inspection requires meta parameters")
    expected = {name: tuple(tensor.shape) for name, tensor in model.state_dict().items()}
    if not expected:
        raise WorkerError("unsupported_model", "Runtime supplied an empty architecture")
    validate_shapes(combine_headers(headers), expected, aliases)
    return shape_bytes(expected)
