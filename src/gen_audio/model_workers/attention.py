"""Registered SDPA adapter for fused kernels without native grouped-query support."""

def expanded_sdpa(module, query, key, value, attention_mask, dropout=0.0,
                  scaling=None, is_causal=None, **kwargs):
    import torch
    groups = query.shape[1] // key.shape[1]
    if groups > 1:
        key = key.repeat_interleave(groups, dim=1)
        value = value.repeat_interleave(groups, dim=1)
    causal = getattr(module, "is_causal", True) if is_causal is None else is_causal
    causal = bool(causal and query.shape[2] > 1 and attention_mask is None)
    # Retain the exact causal/padding/CFG mask produced by the registered mask
    # interface. Kernel selection remains enforced by the execution context.
    output = torch.nn.functional.scaled_dot_product_attention(
        query, key, value, attn_mask=attention_mask, dropout_p=dropout,
        scale=scaling, is_causal=causal,
    )
    return output.transpose(1, 2).contiguous(), None


def configure(model, execution):
    if execution.get("attention") != "efficient":
        return
    from transformers import AttentionInterface, AttentionMaskInterface
    from transformers.masking_utils import sdpa_mask
    name = "gen_audio_expanded_sdpa"
    AttentionInterface.register(name, expanded_sdpa)
    AttentionMaskInterface.register(name, sdpa_mask)
    model.model.language_model.set_attn_implementation(name)
