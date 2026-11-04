"""Voice and G2P documents: the common envelope, then the framework's params."""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping

from gen_audio.voices.errors import VoiceError
from gen_audio.voices.fields import (
    check_framework_id,
    check_https,
    check_lang,
    check_rel_path,
    check_tag,
    check_voice_id,
    optional_str,
    reject_unknown,
    require_object,
    require_str,
)

SCHEMA_VERSION = "1.0.0"
_GENDERS = {"female", "male", "neutral", "unspecified"}
_FORMATS = {"pt", "npz", "onnx", "wav", "bin", "json"}
_SHA256_RE = re.compile(r"^[0-9a-f]{64}\Z")
_LFS_SIZE_RE = re.compile(r"^[0-9]+(?:\.[0-9]+)? (?:KB|MB)\Z")
_VOICE_KEYS = {
    "schema_version",
    "id",
    "display_name",
    "language",
    "gender",
    "framework",
    "assets",
    "source",
    "tags",
    "g2p",
    "params",
    "duplicates",
    "file_prefix",
    "framework_lang_code",
    "reference_audio",
    "access",
}
_ASSET_KEYS = {"role", "path", "format", "member", "sha256", "size"}
_DUPLICATE_KEYS = {"path", "sha256", "size"}
_SOURCE_KEYS = {"name", "url", "license", "attribution"}


@dataclass(frozen=True)
class AssetRef:
    role: str
    path: str
    format: str
    member: str | None = None
    sha256: str | None = None
    size: str | None = None


@dataclass(frozen=True)
class DuplicateRef:
    """Another library path whose LFS oid matches the voice asset."""

    path: str
    sha256: str
    size: str


@dataclass(frozen=True)
class VoiceSource:
    name: str
    url: str
    license: str
    attribution: str | None = None


@dataclass(frozen=True)
class VoiceDescriptor:
    """One voice. ``params`` is the framework object, already checked."""

    schema_version: str
    id: str
    display_name: str
    language: str
    gender: str
    framework: str
    assets: tuple[AssetRef, ...]
    source: VoiceSource
    tags: tuple[str, ...]
    g2p: str | None
    params: Mapping[str, Any]
    path: Path | None = None
    duplicates: tuple[DuplicateRef, ...] = ()
    file_prefix: str | None = None
    reference_audio: Mapping[str, Any] | None = None
    access: Mapping[str, Any] | None = None

    @property
    def local_id(self) -> str:
        return self.id.split(".", 1)[1]


@dataclass(frozen=True)
class G2pConfig:
    """A Misaki (or espeak) phonemizer config. Not a waveform voice."""

    schema_version: str
    id: str
    framework: str
    language: str
    backend: str
    variant: str | None
    british: bool | None
    transformer: bool | None
    espeak_fallback: bool | None
    espeak_language: str | None
    lexicon: str | None
    custom_entries: Mapping[str, str]
    unk: str | None
    framework_lang_code: str | None = None
    path: Path | None = None


def repo_root() -> Path:
    """Checkout root. Same layout the other CLIs use: this file lives in ``src/gen_audio/voices``."""
    return Path(__file__).resolve().parents[3]


def _checkout_root() -> Path | None:
    root = Path(__file__).resolve().parents[3]
    if (root / "pyproject.toml").is_file() and (root / "schemas" / "voices" / "registry.v1.json").is_file():
        return root
    return None


def default_catalog_path() -> Path:
    """Example catalog in a checkout, otherwise the copy installed with the package."""
    checkout = _checkout_root()
    if checkout is not None:
        return checkout / "voices" / "catalog.example.json"
    bundled = Path(__file__).resolve().parent / "data" / "catalog.example.json"
    if bundled.is_file():
        return bundled
    raise VoiceError("voice catalog is not installed", code="missing_schemas")


def g2p_config_path(config_id: str) -> Path:
    """One shipped Misaki config, from the checkout or the installed package data."""
    name = f"{config_id}.json"
    checkout = _checkout_root()
    if checkout is not None:
        return checkout / "voices" / "g2p" / name
    bundled = Path(__file__).resolve().parent / "data" / "g2p" / name
    if bundled.is_file():
        return bundled
    raise VoiceError(f"g2p config {config_id} is not installed", code="missing_schemas")


def schema_dir() -> Path:
    """Schema directory in a checkout, otherwise the copy installed with the package."""
    checkout = _checkout_root()
    if checkout is not None:
        return checkout / "schemas" / "voices"
    bundled = Path(__file__).resolve().parent / "data" / "schemas"
    if (bundled / "registry.v1.json").is_file():
        return bundled
    raise VoiceError("voice schemas are not installed", code="missing_schemas")


def read_json(path: Path) -> dict:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise VoiceError(f"{path} is not valid JSON") from exc
    if not isinstance(payload, dict):
        raise VoiceError(f"{path} root must be an object")
    return payload


def validate_voice(payload: dict, *, path: Path | None = None) -> VoiceDescriptor:
    """Validate a voice descriptor and its framework params."""
    label = str(path) if path is not None else "voice"
    body = require_object(payload, label)
    reject_unknown(body, _VOICE_KEYS, label)
    version = require_str(body, "schema_version", label=label, min_len=5, max_len=16)
    if version != SCHEMA_VERSION:
        raise VoiceError(f"{label} schema_version must be {SCHEMA_VERSION}")
    voice_id = check_voice_id(require_str(body, "id", label=label, min_len=3, max_len=96, pattern=None), label=f"{label} id")
    # require_str already returned the id; re-check with the voice-id pattern via check_voice_id.
    framework = check_framework_id(require_str(body, "framework", label=label, min_len=1, max_len=32), label=f"{label} framework")
    prefix, _, _local = voice_id.partition(".")
    if prefix != framework:
        raise VoiceError(f"{label} id {voice_id} must start with {framework}.")
    display = require_str(body, "display_name", label=label, min_len=1, max_len=80)
    language = check_lang(require_str(body, "language", label=label, min_len=2, max_len=12), label=f"{label} language")
    gender = require_str(body, "gender", label=label, min_len=4, max_len=16)
    if gender not in _GENDERS:
        raise VoiceError(f"{label} gender must be female, male, neutral, or unspecified")
    assets = _assets(body.get("assets"), label)
    duplicates = _duplicates(body.get("duplicates"), assets, label)
    source = _source(body.get("source"), label)
    tags = _tags(body.get("tags"), label)
    g2p = optional_str(body, "g2p", label=label, min_len=3, max_len=80)
    if g2p is not None:
        check_voice_id(g2p, label=f"{label} g2p")
        if not g2p.startswith("misaki."):
            raise VoiceError(f"{label} g2p must be a misaki config id")
    if framework in {"kokoro", "kokoro_onnx"} and g2p is None:
        raise VoiceError(f"{label} framework {framework} requires a g2p config")
    if framework not in {"kokoro", "kokoro_onnx"} and g2p is not None:
        raise VoiceError(f"{label} framework {framework} does not take a g2p config")
    params_raw = body.get("params")
    if not isinstance(params_raw, dict):
        raise VoiceError(f"{label} params must be an object")
    from gen_audio.voices.registry import get_framework

    spec = get_framework(framework)
    if spec.kind == "g2p":
        raise VoiceError(f"{label} framework {framework} is a G2P config, not a voice")
    normalized = spec.validate_params(params_raw, language=language, gender=gender, assets=assets)
    file_prefix = optional_str(body, "file_prefix", label=label, min_len=2, max_len=3)
    reference_audio = _reference_audio(body.get("reference_audio"), label)
    access = _access(body.get("access"), label)
    if framework in {"vibevoice", "livevoice"}:
        if file_prefix is None:
            raise VoiceError(f"{label} needs file_prefix", code="bad_lang")
        if file_prefix != normalized.get("file_prefix"):
            raise VoiceError(f"{label} file_prefix does not match params", code="bad_lang")
    if framework == "vibevoice" and reference_audio is None:
        raise VoiceError(f"{label} needs reference_audio", code="bad_reference")
    if framework == "personaplex":
        if access is None or access.get("gated") is not True:
            raise VoiceError(f"{label} PersonaPlex access must be gated", code="bad_access")
    return VoiceDescriptor(
        schema_version=version,
        id=voice_id,
        display_name=display,
        language=language,
        gender=gender,
        framework=framework,
        assets=assets,
        source=source,
        tags=tags,
        g2p=g2p,
        params=MappingProxyType(normalized),
        path=path,
        duplicates=duplicates,
        file_prefix=file_prefix,
        reference_audio=reference_audio,
        access=access,
    )


def _reference_audio(value: Any, label: str) -> Mapping[str, Any] | None:
    if value is None:
        return None
    body = require_object(value, f"{label} reference_audio")
    reject_unknown(body, {"sample_rate", "channels", "duration_s"}, f"{label} reference_audio")
    rate = body.get("sample_rate")
    channels = body.get("channels")
    if isinstance(rate, bool) or not isinstance(rate, int) or not 8000 <= rate <= 48000:
        raise VoiceError(f"{label} reference sample_rate must be 8000..48000", code="bad_reference")
    if isinstance(channels, bool) or not isinstance(channels, int) or channels not in {1, 2}:
        raise VoiceError(f"{label} reference channels must be 1 or 2", code="bad_reference")
    out: dict[str, Any] = {"sample_rate": rate, "channels": channels}
    if "duration_s" in body:
        duration = body["duration_s"]
        if isinstance(duration, bool) or not isinstance(duration, (int, float)) or duration <= 0:
            raise VoiceError(f"{label} reference duration_s must be positive", code="bad_reference")
        out["duration_s"] = float(duration)
    return MappingProxyType(out)


def _access(value: Any, label: str) -> Mapping[str, Any] | None:
    if value is None:
        return None
    body = require_object(value, f"{label} access")
    reject_unknown(body, {"gated", "provider", "repo", "requires_acceptance"}, f"{label} access")
    gated = body.get("gated")
    acceptance = body.get("requires_acceptance")
    if not isinstance(gated, bool) or not isinstance(acceptance, bool):
        raise VoiceError(f"{label} access.gated and requires_acceptance must be booleans", code="bad_access")
    provider = require_str(body, "provider", label=f"{label} access", min_len=1, max_len=40)
    repo = require_str(body, "repo", label=f"{label} access", min_len=1, max_len=120)
    return MappingProxyType(
        {"gated": gated, "provider": provider, "repo": repo, "requires_acceptance": acceptance}
    )


def load_voice(path: Path | str) -> VoiceDescriptor:
    source = Path(path)
    return validate_voice(read_json(source), path=source)


def validate_g2p(payload: dict, *, path: Path | None = None) -> G2pConfig:
    from gen_audio.voices.builtins import validate_g2p_document

    return validate_g2p_document(payload, path=path)


def load_g2p(path: Path | str) -> G2pConfig:
    source = Path(path)
    return validate_g2p(read_json(source), path=source)


def _assets(value: Any, label: str) -> tuple[AssetRef, ...]:
    if not isinstance(value, list) or not value or len(value) > 16:
        raise VoiceError(f"{label} assets must be 1 to 16 objects")
    found: list[AssetRef] = []
    seen: set[str] = set()
    for index, raw in enumerate(value):
        item = require_object(raw, f"{label} asset {index}")
        reject_unknown(item, _ASSET_KEYS, f"{label} asset {index}")
        role = check_framework_id(require_str(item, "role", label=f"{label} asset {index}", min_len=1, max_len=32), label="asset role")
        rel = check_rel_path(require_str(item, "path", label=f"{label} asset {index}", min_len=1, max_len=220), label=f"{label} asset path")
        fmt = require_str(item, "format", label=f"{label} asset {index}", min_len=2, max_len=8)
        if fmt not in _FORMATS:
            raise VoiceError(f"{label} asset format {fmt!r} is not supported")
        member = optional_str(item, "member", label=f"{label} asset {index}", min_len=1, max_len=80)
        sha256 = _optional_sha256(item, label=f"{label} asset {index}")
        size = _optional_lfs_size(item, label=f"{label} asset {index}")
        if rel in seen:
            raise VoiceError(f"{label} repeats asset path {rel}")
        seen.add(rel)
        found.append(AssetRef(role=role, path=rel, format=fmt, member=member, sha256=sha256, size=size))
    return tuple(found)


def _duplicates(value: Any, assets: tuple[AssetRef, ...], label: str) -> tuple[DuplicateRef, ...]:
    if value is None:
        return ()
    if not isinstance(value, list) or len(value) > 8:
        raise VoiceError(f"{label} duplicates must be an array of at most 8")
    asset_sha = {asset.sha256 for asset in assets if asset.sha256}
    found: list[DuplicateRef] = []
    seen: set[str] = {asset.path for asset in assets}
    for index, raw in enumerate(value):
        item = require_object(raw, f"{label} duplicate {index}")
        reject_unknown(item, _DUPLICATE_KEYS, f"{label} duplicate {index}")
        rel = check_rel_path(
            require_str(item, "path", label=f"{label} duplicate {index}", min_len=1, max_len=220),
            label=f"{label} duplicate path",
        )
        sha256 = _require_sha256(item, label=f"{label} duplicate {index}")
        size = _require_lfs_size(item, label=f"{label} duplicate {index}")
        if rel in seen:
            raise VoiceError(f"{label} duplicate path repeats {rel}")
        if asset_sha and sha256 not in asset_sha:
            raise VoiceError(f"{label} duplicate {rel} sha256 does not match the voice asset")
        seen.add(rel)
        found.append(DuplicateRef(path=rel, sha256=sha256, size=size))
    return tuple(found)


def _optional_sha256(payload: dict, *, label: str) -> str | None:
    if "sha256" not in payload:
        return None
    return _require_sha256(payload, label=label)


def _require_sha256(payload: dict, *, label: str) -> str:
    value = require_str(payload, "sha256", label=label, min_len=64, max_len=64)
    if _SHA256_RE.fullmatch(value) is None:
        raise VoiceError(f"{label} sha256 must be 64 lowercase hex characters")
    return value


def _optional_lfs_size(payload: dict, *, label: str) -> str | None:
    if "size" not in payload:
        return None
    return _require_lfs_size(payload, label=label)


def _require_lfs_size(payload: dict, *, label: str) -> str:
    value = require_str(payload, "size", label=label, min_len=4, max_len=16)
    if _LFS_SIZE_RE.fullmatch(value) is None:
        raise VoiceError(f"{label} size must be a git lfs ls-files display size such as 297 KB")
    return value


def _source(value: Any, label: str) -> VoiceSource:
    body = require_object(value, f"{label} source")
    reject_unknown(body, _SOURCE_KEYS, f"{label} source")
    return VoiceSource(
        name=require_str(body, "name", label=f"{label} source", min_len=1, max_len=120),
        url=check_https(require_str(body, "url", label=f"{label} source", min_len=12, max_len=300), label=f"{label} source url"),
        license=require_str(body, "license", label=f"{label} source", min_len=1, max_len=80),
        attribution=optional_str(body, "attribution", label=f"{label} source", min_len=1, max_len=400),
    )


def _tags(value: Any, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or len(value) > 16:
        raise VoiceError(f"{label} tags must be an array of at most 16")
    found: list[str] = []
    for raw in value:
        tag = check_tag(raw)
        if tag in found:
            raise VoiceError(f"{label} repeats tag {tag}")
        found.append(tag)
    return tuple(found)
