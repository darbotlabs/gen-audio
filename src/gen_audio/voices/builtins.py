"""Built-in voice frameworks. Each one validates ``params`` and builds a plan.

A new framework calls ``register`` with the same shape. It also needs a JSON
Schema for ``params`` and a row in ``schemas/voices/registry.v1.json`` when it
ships with this package. Process-local experiments can ``register`` without
that row; ``unregister`` removes them.
"""

from __future__ import annotations

import importlib.util
import os
import re
from pathlib import Path
from types import MappingProxyType
from typing import Any

from gen_audio.voices.errors import VoiceError
from gen_audio.voices.fields import (
    check_lang,
    check_rel_path,
    optional_bool,
    optional_str,
    reject_unknown,
    require_bool,
    require_int,
    require_number,
    require_object,
    require_str,
)
from gen_audio.voices.model import SCHEMA_VERSION, AssetRef, G2pConfig
from gen_audio.voices.plan import SynthesisPlan
from gen_audio.voices.registry import FrameworkSpec, register

_PACK_RE = re.compile(r"^[abefhijpz][fm]_[a-z0-9]{1,32}\Z")
_STEM_RE = re.compile(r"^(?P<lang>[a-z]{2,3})-(?P<name>[A-Za-z0-9]+)_(?P<gender>man|woman|neutral)(?P<bgm>_bgm)?\Z")
_LIVE_RE = re.compile(r"^(?P<lang>[a-z]{2,3})-(?P<name>[A-Za-z0-9]+)_(?P<gender>man|woman|neutral)\Z")
_PERSONA_RE = re.compile(r"^(NATF[0-3]|NATM[0-3]|VARF[0-4]|VARM[0-4])\Z")
_KOKORO_LANG = {
    "a": "en-us",
    "b": "en-gb",
    "e": "es",
    "f": "fr-fr",
    "h": "hi",
    "i": "it",
    "j": "ja",
    "p": "pt-br",
    "z": "zh",
}
_GENDER_LETTER = {"f": "female", "m": "male"}
_STEM_GENDER = {"woman": "female", "man": "male", "neutral": "neutral"}
_VIBEVOICE_MODELS = {"1.5B": "microsoft/VibeVoice-1.5B", "7B": "microsoft/VibeVoice-Large"}
_G2P_KEYS = {
    "schema_version",
    "id",
    "framework",
    "language",
    "variant",
    "backend",
    "british",
    "transformer",
    "espeak_fallback",
    "espeak_language",
    "lexicon",
    "custom_entries",
    "unk",
}
_BACKENDS = {"misaki-en", "misaki-ja", "misaki-zh", "espeak"}
_REGISTERED = False


def register_builtins() -> None:
    global _REGISTERED
    if _REGISTERED:
        return
    rows = {row["id"]: row for row in _registry_rows()}
    _add(rows, "kokoro", validate_kokoro_params, prepare_kokoro, speaker_limit=8)
    _add(rows, "kokoro_onnx", validate_kokoro_onnx_params, prepare_kokoro_onnx, speaker_limit=8)
    _add(rows, "misaki", validate_misaki_voice_params, prepare_misaki, speaker_limit=None)
    _add(rows, "vibevoice", validate_vibevoice_params, prepare_vibevoice, speaker_limit=4)
    _add(rows, "livevoice", validate_livevoice_params, prepare_livevoice, speaker_limit=1)
    _add(rows, "personaplex", validate_personaplex_params, prepare_personaplex, speaker_limit=1)
    _REGISTERED = True


def _registry_rows() -> list[dict]:
    from gen_audio.voices.registry import registry_document

    return list(registry_document()["frameworks"])


def _add(rows: dict[str, dict], framework_id: str, checker, preparer, *, speaker_limit: int | None) -> None:
    row = rows[framework_id]
    register(
        FrameworkSpec(
            id=framework_id,
            kind=row["kind"],
            engine_id=row["engine_id"],
            summary=row["summary"],
            wired=bool(row["wired"]),
            params_schema=row["params_schema"],
            validate_params=checker,
            prepare=preparer,
            speaker_limit=speaker_limit,
        ),
        builtin=True,
    )


def validate_kokoro_params(params: dict, *, language: str, gender: str, assets: tuple[AssetRef, ...]) -> dict:
    return _kokoro_like(
        params,
        language=language,
        gender=gender,
        assets=assets,
        label="kokoro",
        extra_keys=set(),
        asset_role="voicepack",
        asset_format="pt",
    )


def validate_kokoro_onnx_params(params: dict, *, language: str, gender: str, assets: tuple[AssetRef, ...]) -> dict:
    body = require_object(params, "kokoro_onnx params")
    allowed = {"voice_id", "lang", "speed", "voices_file", "model_file", "blend"}
    reject_unknown(body, allowed, "kokoro_onnx params")
    if ("voice_id" in body) == ("blend" in body):
        raise VoiceError("kokoro_onnx params need voice_id or blend, not both")
    lang = check_lang(require_str(body, "lang", label="kokoro_onnx params", min_len=2, max_len=12), label="kokoro_onnx lang")
    if lang != language:
        raise VoiceError(f"kokoro_onnx lang {lang} does not match the voice language {language}")
    if lang not in set(_KOKORO_LANG.values()):
        raise VoiceError(f"kokoro_onnx lang {lang} is not a Kokoro language")
    speed = require_number(body, "speed", label="kokoro_onnx params", minimum=0.5, maximum=2.0)
    voices_file = require_str(body, "voices_file", label="kokoro_onnx params", min_len=8, max_len=64)
    model_file = require_str(body, "model_file", label="kokoro_onnx params", min_len=8, max_len=64)
    if not re.fullmatch(r"voices-v[0-9]+\.[0-9]+(-[a-z0-9]+)?\.bin", voices_file):
        raise VoiceError("kokoro_onnx voices_file must look like voices-v1.0.bin")
    if not re.fullmatch(r"kokoro-v[0-9]+\.[0-9]+(\.[a-z0-9]+)?\.onnx", model_file):
        raise VoiceError("kokoro_onnx model_file must look like kokoro-v1.0.onnx")
    voice_ids = _voice_selection(body, "kokoro_onnx params")
    _kokoro_voices_agree(voice_ids, lang, gender, label="kokoro_onnx")
    _require_assets(
        assets,
        role="style_pack",
        fmt="npz",
        label="kokoro_onnx",
        expect_members=None if "blend" in body else voice_ids,
    )
    packs = [asset for asset in assets if asset.role == "style_pack"]
    if len(packs) != 1 or Path(packs[0].path).name != voices_file:
        raise VoiceError("kokoro_onnx needs one style_pack asset whose file name is voices_file")
    if "blend" in body and packs[0].member is not None:
        raise VoiceError("a kokoro_onnx blend keeps the member off the shared style pack")
    if "voice_id" in body and packs[0].member not in {None, body["voice_id"]}:
        raise VoiceError("kokoro_onnx style_pack member must match voice_id")
    models = [asset for asset in assets if asset.role == "onnx_model"]
    if len(models) != 1 or models[0].format != "onnx" or Path(models[0].path).name != model_file:
        raise VoiceError("kokoro_onnx needs one onnx_model asset whose file name is model_file")
    used = {asset.role for asset in assets}
    if used - {"style_pack", "onnx_model"}:
        raise VoiceError("kokoro_onnx assets must be the style pack and the ONNX model")
    normalized = {
        "lang": lang,
        "speed": speed,
        "voices_file": voices_file,
        "model_file": model_file,
    }
    if "voice_id" in body:
        normalized["voice_id"] = body["voice_id"]
    else:
        normalized["blend"] = _blend_payload(body["blend"])
    return normalized


def validate_misaki_voice_params(params: dict, *, language: str, gender: str, assets: tuple[AssetRef, ...]) -> dict:
    del language, gender, assets
    if params:
        raise VoiceError("misaki is a G2P config, not a voice; params must be empty")
    return {}


def validate_vibevoice_params(params: dict, *, language: str, gender: str, assets: tuple[AssetRef, ...]) -> dict:
    body = require_object(params, "vibevoice params")
    allowed = {
        "model_size",
        "model_id",
        "file_stem",
        "cfg_scale",
        "diffusion_steps",
        "sample_rate",
        "max_speakers",
        "background_music",
    }
    reject_unknown(body, allowed, "vibevoice params")
    stem = require_str(body, "file_stem", label="vibevoice params", min_len=8, max_len=80)
    match = _STEM_RE.fullmatch(stem)
    if match is None:
        raise VoiceError("vibevoice file_stem must look like en-Alice_woman or zh-Bowen_man_bgm")
    if match.group("lang") != language:
        raise VoiceError(f"vibevoice file_stem language {match.group('lang')} does not match {language}")
    stem_gender = _STEM_GENDER[match.group("gender")]
    if stem_gender != gender:
        raise VoiceError(f"vibevoice file_stem gender does not match {gender}")
    background = require_bool(body, "background_music", label="vibevoice params")
    if background != bool(match.group("bgm")):
        raise VoiceError("vibevoice background_music must follow the _bgm suffix")
    rate = require_int(body, "sample_rate", label="vibevoice params", minimum=24000, maximum=24000)
    speakers = require_int(body, "max_speakers", label="vibevoice params", minimum=4, maximum=4)
    _one_asset(assets, role="reference_wav", fmt="wav", suffix=f"{stem}.wav", label="vibevoice")
    normalized: dict[str, Any] = {
        "file_stem": stem,
        "sample_rate": rate,
        "max_speakers": speakers,
        "background_music": background,
    }
    if "model_size" in body or "model_id" in body:
        model_size = require_str(body, "model_size", label="vibevoice params", min_len=2, max_len=8)
        model_id = require_str(body, "model_id", label="vibevoice params", min_len=8, max_len=80)
        if _VIBEVOICE_MODELS.get(model_size) != model_id:
            raise VoiceError("vibevoice model_size and model_id do not match")
        normalized["model_size"] = model_size
        normalized["model_id"] = model_id
    if "cfg_scale" in body:
        normalized["cfg_scale"] = require_number(body, "cfg_scale", label="vibevoice params", minimum=0.5, maximum=5.0)
    if "diffusion_steps" in body:
        normalized["diffusion_steps"] = require_int(
            body, "diffusion_steps", label="vibevoice params", minimum=1, maximum=100
        )
    return normalized


def validate_livevoice_params(params: dict, *, language: str, gender: str, assets: tuple[AssetRef, ...]) -> dict:
    body = require_object(params, "livevoice params")
    allowed = {
        "model_id",
        "model_size",
        "file_stem",
        "preset_kind",
        "single_speaker",
        "experimental",
        "sample_rate",
    }
    reject_unknown(body, allowed, "livevoice params")
    model_id = require_str(body, "model_id", label="livevoice params", min_len=8, max_len=80)
    if model_id != "microsoft/VibeVoice-Realtime-0.5B":
        raise VoiceError("livevoice model_id must be microsoft/VibeVoice-Realtime-0.5B")
    model_size = require_str(body, "model_size", label="livevoice params", min_len=3, max_len=8)
    if model_size != "0.5B":
        raise VoiceError("livevoice model_size must be 0.5B")
    stem = require_str(body, "file_stem", label="livevoice params", min_len=8, max_len=80)
    match = _LIVE_RE.fullmatch(stem)
    if match is None:
        raise VoiceError("livevoice file_stem must look like en-Carter_man or de-Spk0_man")
    if match.group("lang") != language:
        raise VoiceError(f"livevoice file_stem language {match.group('lang')} does not match {language}")
    if _STEM_GENDER[match.group("gender")] != gender:
        raise VoiceError(f"livevoice file_stem gender does not match {gender}")
    preset = require_str(body, "preset_kind", label="livevoice params", min_len=3, max_len=32)
    if preset != "kv_cache":
        raise VoiceError("livevoice preset_kind must be kv_cache")
    single = require_bool(body, "single_speaker", label="livevoice params")
    if single is not True:
        raise VoiceError("livevoice is a single-speaker streaming preset")
    experimental = require_bool(body, "experimental", label="livevoice params")
    rate = require_int(body, "sample_rate", label="livevoice params", minimum=24000, maximum=24000)
    asset = _one_asset(assets, role="voice_embedding", fmt="pt", suffix=f"{stem}.pt", label="livevoice")
    experimental_path = "/experimental_voices/" in asset.path.replace("\\", "/")
    if experimental != experimental_path:
        raise VoiceError("livevoice experimental must follow experimental_voices/ in the asset path")
    return {
        "model_id": model_id,
        "model_size": model_size,
        "file_stem": stem,
        "preset_kind": preset,
        "single_speaker": True,
        "experimental": experimental,
        "sample_rate": rate,
    }


def validate_personaplex_params(params: dict, *, language: str, gender: str, assets: tuple[AssetRef, ...]) -> dict:
    body = require_object(params, "personaplex params")
    allowed = {
        "model_id",
        "gated",
        "voice_code",
        "voice_family",
        "voice_prompt",
        "text_prompt",
        "text_prompt_role",
        "sample_rate",
        "seed",
        "cpu_offload",
    }
    reject_unknown(body, allowed, "personaplex params")
    if language != "en":
        raise VoiceError("personaplex voices in this schema are the English stock prompts")
    model_id = require_str(body, "model_id", label="personaplex params", min_len=8, max_len=80)
    if model_id != "nvidia/personaplex-7b-v1":
        raise VoiceError("personaplex model_id must be nvidia/personaplex-7b-v1")
    gated = require_bool(body, "gated", label="personaplex params")
    if gated is not True:
        raise VoiceError("personaplex weights are gated")
    code = require_str(body, "voice_code", label="personaplex params", min_len=5, max_len=5)
    if _PERSONA_RE.fullmatch(code) is None:
        raise VoiceError("personaplex voice_code must be NATF0-3, NATM0-3, VARF0-4, or VARM0-4")
    family = require_str(body, "voice_family", label="personaplex params", min_len=7, max_len=8)
    expected_family = "natural" if code.startswith("NAT") else "variety"
    if family != expected_family:
        raise VoiceError(f"personaplex voice_family for {code} must be {expected_family}")
    letter = code[3]
    expected_gender = "female" if letter == "F" else "male"
    if gender != expected_gender:
        raise VoiceError(f"personaplex voice_code {code} does not match gender {gender}")
    prompt = require_str(body, "voice_prompt", label="personaplex params", min_len=6, max_len=16)
    if prompt != f"{code}.pt":
        raise VoiceError("personaplex voice_prompt must be the voice code plus .pt")
    text = require_str(body, "text_prompt", label="personaplex params", min_len=8, max_len=2000)
    role = optional_str(body, "text_prompt_role", label="personaplex params", min_len=6, max_len=20)
    if role is not None and role not in {"assistant", "customer_service", "casual"}:
        raise VoiceError("personaplex text_prompt_role must be assistant, customer_service, or casual")
    rate = require_int(body, "sample_rate", label="personaplex params", minimum=24000, maximum=24000)
    cpu = optional_bool(body, "cpu_offload", label="personaplex params")
    seed = None
    if "seed" in body:
        seed = require_int(body, "seed", label="personaplex params", minimum=0, maximum=2_147_483_647)
    _one_asset(assets, role="voice_prompt", fmt="pt", suffix=prompt, label="personaplex")
    normalized: dict[str, Any] = {
        "model_id": model_id,
        "gated": True,
        "voice_code": code,
        "voice_family": family,
        "voice_prompt": prompt,
        "text_prompt": text,
        "sample_rate": rate,
    }
    if role is not None:
        normalized["text_prompt_role"] = role
    if cpu is not None:
        normalized["cpu_offload"] = cpu
    if seed is not None:
        normalized["seed"] = seed
    return normalized


def validate_g2p_document(payload: dict, *, path: Path | None = None) -> G2pConfig:
    label = str(path) if path is not None else "g2p"
    body = require_object(payload, label)
    reject_unknown(body, _G2P_KEYS, label)
    version = require_str(body, "schema_version", label=label, min_len=5, max_len=16)
    if version != SCHEMA_VERSION:
        raise VoiceError(f"{label} schema_version must be {SCHEMA_VERSION}")
    g2p_id = require_str(body, "id", label=label, min_len=3, max_len=80)
    if not g2p_id.startswith("misaki.") or g2p_id == "misaki.":
        raise VoiceError(f"{label} id must start with misaki.")
    framework = require_str(body, "framework", label=label, min_len=6, max_len=6)
    if framework != "misaki":
        raise VoiceError(f"{label} framework must be misaki")
    language = check_lang(require_str(body, "language", label=label, min_len=2, max_len=12), label=f"{label} language")
    backend = require_str(body, "backend", label=label, min_len=6, max_len=16)
    if backend not in _BACKENDS:
        raise VoiceError(f"{label} backend is not a Misaki or espeak backend")
    variant = optional_str(body, "variant", label=label, min_len=2, max_len=12)
    if variant is not None:
        check_lang(variant, label=f"{label} variant")
    british = optional_bool(body, "british", label=label)
    transformer = optional_bool(body, "transformer", label=label)
    espeak_fallback = optional_bool(body, "espeak_fallback", label=label)
    espeak_language = optional_str(body, "espeak_language", label=label, min_len=2, max_len=12)
    if espeak_language is not None:
        check_lang(espeak_language, label=f"{label} espeak_language")
    lexicon = optional_str(body, "lexicon", label=label, min_len=1, max_len=220)
    if lexicon is not None:
        check_rel_path(lexicon, label=f"{label} lexicon")
    entries = _custom_entries(body.get("custom_entries"), label) if "custom_entries" in body else {}
    unk = optional_str(body, "unk", label=label, min_len=0, max_len=4)
    _check_g2p_backend(
        backend,
        language=language,
        variant=variant,
        british=british,
        transformer=transformer,
        espeak_fallback=espeak_fallback,
        espeak_language=espeak_language,
        label=label,
    )
    return G2pConfig(
        schema_version=version,
        id=g2p_id,
        framework="misaki",
        language=language,
        backend=backend,
        variant=variant,
        british=british,
        transformer=transformer,
        espeak_fallback=espeak_fallback,
        espeak_language=espeak_language,
        lexicon=lexicon,
        custom_entries=MappingProxyType(entries),
        unk=unk,
        path=path,
    )


def prepare_kokoro(voice, library: Path | None = None) -> SynthesisPlan:
    missing = ["the dayour/kokoro torch runtime is not vendored, so .pt voicepacks are not loaded"]
    files, absent = _existing_assets(voice, library)
    missing.extend(absent)
    return _plan(voice, "kokoro", missing, files, executable=False)


def prepare_kokoro_onnx(voice, library: Path | None = None) -> SynthesisPlan:
    files, missing = _kokoro_onnx_files(voice, library)
    if importlib.util.find_spec("kokoro_onnx") is None:
        missing.append("kokoro-onnx package is not installed (pip install 'gen-audio[kokoro]')")
    return _plan(voice, "kokoro_onnx", missing, files, executable=not missing)


def prepare_misaki(config, library: Path | None = None) -> SynthesisPlan:
    if not isinstance(config, G2pConfig):
        raise VoiceError("misaki prepare expects a G2P config")
    missing = ["misaki is a grapheme-to-phoneme library and does not emit a waveform"]
    files: dict[str, str] = {}
    if config.lexicon:
        if library is None:
            missing.append("lexicon is set and GENAID_VOICES_DIR is unset")
        else:
            path = _resolve_under(library, config.lexicon)
            files["lexicon"] = str(path)
            if not path.is_file():
                missing.append(f"lexicon file is missing: {config.lexicon}")
    return SynthesisPlan(
        framework="misaki",
        voice_id=config.id,
        engine_id="misaki",
        kind="g2p",
        wired=False,
        executable=False,
        missing=tuple(missing),
        params=MappingProxyType(
            {
                "backend": config.backend,
                "language": config.language,
                "variant": config.variant,
                "british": config.british,
                "espeak_fallback": config.espeak_fallback,
            }
        ),
        resolved_files=MappingProxyType(files),
    )


def prepare_vibevoice(voice, library: Path | None = None) -> SynthesisPlan:
    missing = [
        "VibeVoice weights are not bundled",
        "GEN_AUDIO_VIBEVOICE_MODEL is unset"
        if not os.environ.get("GEN_AUDIO_VIBEVOICE_MODEL", "").strip()
        else "the voice plan does not load VibeVoice weights",
    ]
    files, absent = _existing_assets(voice, library)
    if library is None:
        missing.append("reference wav is unresolved until GENAID_VOICES_DIR or a library path is set")
    missing.extend(absent)
    return _plan(voice, "vibevoice", missing, files, executable=False)


def prepare_livevoice(voice, library: Path | None = None) -> SynthesisPlan:
    missing = [
        "VibeVoice-Realtime-0.5B weights are not bundled",
        "livevoice is one streaming speaker; this package does not run that runtime",
    ]
    files, absent = _existing_assets(voice, library)
    if library is None:
        missing.append("voice preset is unresolved until GENAID_VOICES_DIR or a library path is set")
    missing.extend(absent)
    return _plan(voice, "livevoice", missing, files, executable=False)


def prepare_personaplex(voice, library: Path | None = None) -> SynthesisPlan:
    missing = [
        "nvidia/personaplex-7b-v1 is gated and its weights are not bundled",
        "no PersonaPlex runtime is registered in this package",
    ]
    files, absent = _existing_assets(voice, library)
    missing.extend(absent)
    return _plan(voice, "personaplex", missing, files, executable=False)


def mock_offline_request(plan: SynthesisPlan) -> dict[str, Any]:
    """The ``moshi.offline`` arguments a PersonaPlex run would use. Weights stay unloaded."""
    if plan.framework != "personaplex":
        raise VoiceError("the offline mock is only defined for personaplex")
    return {
        "module": "moshi.offline",
        "voice_prompt": plan.params["voice_prompt"],
        "text_prompt": plan.params["text_prompt"],
        "text_prompt_role": plan.params.get("text_prompt_role"),
        "sample_rate": plan.params["sample_rate"],
        "seed": plan.params.get("seed"),
        "cpu_offload": bool(plan.params.get("cpu_offload", False)),
        "weights_loaded": False,
    }


def _kokoro_like(
    params: dict,
    *,
    language: str,
    gender: str,
    assets: tuple[AssetRef, ...],
    label: str,
    extra_keys: set[str],
    asset_role: str,
    asset_format: str,
) -> dict:
    body = require_object(params, f"{label} params")
    allowed = {"voice_id", "lang_code", "lang", "speed", "blend"} | extra_keys
    reject_unknown(body, allowed, f"{label} params")
    if ("voice_id" in body) == ("blend" in body):
        raise VoiceError(f"{label} params need voice_id or blend, not both")
    lang_code = require_str(body, "lang_code", label=f"{label} params", min_len=1, max_len=1)
    if lang_code not in _KOKORO_LANG:
        raise VoiceError(f"{label} lang_code must be a Kokoro pipeline code")
    lang = check_lang(require_str(body, "lang", label=f"{label} params", min_len=2, max_len=12), label=f"{label} lang")
    if _KOKORO_LANG[lang_code] != lang or lang != language:
        raise VoiceError(f"{label} lang_code {lang_code} does not match language {language}")
    speed = require_number(body, "speed", label=f"{label} params", minimum=0.5, maximum=2.0)
    voice_ids = _voice_selection(body, f"{label} params")
    for voice_id in voice_ids:
        if voice_id[0] != lang_code:
            raise VoiceError(f"{label} voice {voice_id} does not use lang_code {lang_code}")
    _gender_agrees(voice_ids, gender, label=label)
    members = _require_assets(assets, role=asset_role, fmt=asset_format, label=label, expect_members=voice_ids)
    for asset in assets:
        if asset.role != asset_role:
            raise VoiceError(f"{label} assets must be {asset_role} files")
        if Path(asset.path).name != f"{asset.member}.pt":
            raise VoiceError(f"{label} asset path must name the pack id")
    del members
    normalized: dict[str, Any] = {"lang_code": lang_code, "lang": lang, "speed": speed}
    if "voice_id" in body:
        normalized["voice_id"] = body["voice_id"]
    else:
        normalized["blend"] = _blend_payload(body["blend"])
    return normalized


def _voice_selection(body: dict, label: str) -> list[str]:
    if "voice_id" in body:
        voice_id = require_str(body, "voice_id", label=label, min_len=4, max_len=40)
        if _PACK_RE.fullmatch(voice_id) is None:
            raise VoiceError(f"{label} voice_id is not a Kokoro pack id")
        return [voice_id]
    blend = body.get("blend")
    if not isinstance(blend, list) or not 2 <= len(blend) <= 8:
        raise VoiceError(f"{label} blend must list 2 to 8 voices")
    found: list[str] = []
    weights: list[float] = []
    for index, raw in enumerate(blend):
        part = require_object(raw, f"{label} blend {index}")
        reject_unknown(part, {"voice_id", "weight"}, f"{label} blend {index}")
        voice_id = require_str(part, "voice_id", label=f"{label} blend {index}", min_len=4, max_len=40)
        if _PACK_RE.fullmatch(voice_id) is None:
            raise VoiceError(f"{label} blend voice is not a Kokoro pack id")
        if voice_id in found:
            raise VoiceError(f"{label} blend repeats {voice_id}")
        weight = require_number(part, "weight", label=f"{label} blend {index}", exclusive_minimum=0, maximum=1)
        found.append(voice_id)
        weights.append(weight)
    if abs(sum(weights) - 1.0) > 1e-6:
        raise VoiceError(f"{label} blend weights must sum to 1")
    return found


def _blend_payload(blend: list) -> list[dict[str, Any]]:
    return [{"voice_id": part["voice_id"], "weight": float(part["weight"])} for part in blend]


def _kokoro_voices_agree(voice_ids: list[str], lang: str, gender: str, *, label: str) -> None:
    codes = {voice_id[0] for voice_id in voice_ids}
    if len(codes) != 1:
        raise VoiceError(f"{label} blend must stay inside one language")
    code = codes.pop()
    if _KOKORO_LANG[code] != lang:
        raise VoiceError(f"{label} pack language does not match {lang}")
    _gender_agrees(voice_ids, gender, label=label)


def _gender_agrees(voice_ids: list[str], gender: str, *, label: str) -> None:
    letters = {voice_id[1] for voice_id in voice_ids}
    if len(letters) == 1:
        expected = _GENDER_LETTER[letters.pop()]
        if gender != expected:
            raise VoiceError(f"{label} gender must be {expected}")
        return
    if gender != "unspecified":
        raise VoiceError(f"{label} mixed-gender blend must set gender to unspecified")


def _require_assets(
    assets: tuple[AssetRef, ...],
    *,
    role: str,
    fmt: str,
    label: str,
    expect_members: list[str] | None,
) -> list[str]:
    chosen = [asset for asset in assets if asset.role == role and asset.format == fmt]
    if expect_members is None:
        if not chosen:
            raise VoiceError(f"{label} needs a {role} asset")
        return [asset.member for asset in chosen if asset.member]
    members = [asset.member for asset in chosen]
    if any(member is None for member in members):
        raise VoiceError(f"{label} {role} assets must name their pack id in member")
    if sorted(member or "" for member in members) != sorted(expect_members):
        raise VoiceError(f"{label} assets must list each blended pack once")
    return [member for member in members if member]


def _one_asset(assets: tuple[AssetRef, ...], *, role: str, fmt: str, suffix: str, label: str) -> AssetRef:
    chosen = [asset for asset in assets if asset.role == role]
    if len(chosen) != 1 or len(assets) != 1:
        raise VoiceError(f"{label} needs exactly one {role} asset")
    asset = chosen[0]
    if asset.format != fmt or Path(asset.path).name != suffix:
        raise VoiceError(f"{label} asset must be a {fmt} file named {suffix}")
    return asset


def _check_g2p_backend(
    backend: str,
    *,
    language: str,
    variant: str | None,
    british: bool | None,
    transformer: bool | None,
    espeak_fallback: bool | None,
    espeak_language: str | None,
    label: str,
) -> None:
    if backend == "misaki-en":
        if language != "en" or variant not in {"en-us", "en-gb"}:
            raise VoiceError(f"{label} English config needs language en and variant en-us or en-gb")
        if british is None or transformer is None or espeak_fallback is None:
            raise VoiceError(f"{label} English config needs british, transformer, and espeak_fallback")
        if british != (variant == "en-gb"):
            raise VoiceError(f"{label} british must follow the en-gb variant")
        if espeak_fallback and espeak_language != variant:
            raise VoiceError(f"{label} espeak_language must match the English variant")
        if not espeak_fallback and espeak_language is not None:
            raise VoiceError(f"{label} espeak_language is only set when espeak_fallback is true")
        return
    if british or transformer:
        raise VoiceError(f"{label} british and transformer apply to English Misaki")
    if backend == "misaki-ja" and language != "ja":
        raise VoiceError(f"{label} misaki-ja language must be ja")
    if backend == "misaki-zh" and language != "zh":
        raise VoiceError(f"{label} misaki-zh language must be zh")
    if backend == "espeak":
        if espeak_fallback is not True or espeak_language is None:
            raise VoiceError(f"{label} espeak backend needs espeak_fallback and espeak_language")
        if espeak_language != language and not espeak_language.startswith(language + "-"):
            raise VoiceError(f"{label} espeak_language must match language {language}")


def _custom_entries(value: Any, label: str) -> dict[str, str]:
    if not isinstance(value, dict) or len(value) > 32:
        raise VoiceError(f"{label} custom_entries must be an object of at most 32 entries")
    found: dict[str, str] = {}
    for key, phonemes in value.items():
        if not isinstance(key, str) or not key.strip() or key != key.strip() or len(key) > 48:
            raise VoiceError(f"{label} custom lexicon key is invalid")
        if any(ord(char) < 32 for char in key):
            raise VoiceError(f"{label} custom lexicon key has a control character")
        if not isinstance(phonemes, str) or not phonemes or len(phonemes) > 80:
            raise VoiceError(f"{label} custom lexicon value must be a short phoneme string")
        found[key] = phonemes
    return found


def _plan(voice, framework_id: str, missing: list[str], files: dict[str, str], *, executable: bool) -> SynthesisPlan:
    from gen_audio.voices.registry import get_framework

    spec = get_framework(framework_id)
    return SynthesisPlan(
        framework=framework_id,
        voice_id=voice.id,
        engine_id=spec.engine_id,
        kind=spec.kind,
        wired=spec.wired,
        executable=executable and spec.wired,
        missing=tuple(missing),
        params=voice.params,
        resolved_files=MappingProxyType(files),
    )


def _kokoro_onnx_files(voice, library: Path | None) -> tuple[dict[str, str], list[str]]:
    missing: list[str] = []
    files: dict[str, str] = {}
    model_env = os.environ.get("GEN_AUDIO_KOKORO_MODEL", "").strip()
    voices_env = os.environ.get("GEN_AUDIO_KOKORO_VOICES", "").strip()
    model_asset = next(asset.path for asset in voice.assets if asset.role == "onnx_model")
    voices_asset = next(asset.path for asset in voice.assets if asset.role == "style_pack")
    model_path = _env_or_library(model_env, model_asset, library, "GEN_AUDIO_KOKORO_MODEL", missing, "model")
    voices_path = _env_or_library(voices_env, voices_asset, library, "GEN_AUDIO_KOKORO_VOICES", missing, "voices")
    if model_path is not None:
        files["model"] = str(model_path)
    if voices_path is not None:
        files["voices"] = str(voices_path)
    return files, missing


def _env_or_library(
    env_value: str,
    relative: str,
    library: Path | None,
    env_name: str,
    missing: list[str],
    role: str,
) -> Path | None:
    if env_value:
        path = Path(env_value)
        if not path.is_file():
            missing.append(f"{env_name} file is missing")
            return path
        return path
    if library is None:
        missing.append(f"{env_name} is unset and no voice library was given for the {role} file")
        return None
    path = _resolve_under(library, relative)
    if not path.is_file():
        missing.append(f"{role} file is missing: {relative}")
    return path


def _existing_assets(voice, library: Path | None) -> tuple[dict[str, str], list[str]]:
    if library is None:
        return {}, []
    files: dict[str, str] = {}
    missing: list[str] = []
    for asset in voice.assets:
        path = _resolve_under(library, asset.path)
        files[asset.role] = str(path)
        if not path.is_file():
            missing.append(f"asset file is missing: {asset.path}")
    return files, missing


def _resolve_under(library: Path, relative: str) -> Path:
    root = library.resolve()
    candidate = (root / relative).resolve()
    if candidate != root and root not in candidate.parents:
        raise VoiceError(f"asset path escapes the voice library: {relative}")
    return candidate
