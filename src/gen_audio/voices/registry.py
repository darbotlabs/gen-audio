"""Framework registry.

Built-in rows come from ``schemas/voices/registry.v1.json``. ``register`` adds
another framework for this process: a params checker and a ``prepare`` function.
The JSON Schema for that framework's ``params`` object is the other half of the plug-in.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Callable

from gen_audio.voices.errors import VoiceError

ParamChecker = Callable[..., dict[str, Any]]
Preparer = Callable[..., Any]


@dataclass(frozen=True)
class FrameworkSpec:
    id: str
    kind: str
    engine_id: str | None
    summary: str
    wired: bool
    params_schema: str
    validate_params: ParamChecker
    prepare: Preparer
    speaker_limit: int | None = None


_SPECS: dict[str, FrameworkSpec] = {}
_BUILTIN_IDS: set[str] = set()
_LOADED = False


def registry_document() -> dict:
    from gen_audio.voices.model import schema_dir

    path = schema_dir() / "registry.v1.json"
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise VoiceError("voice framework registry is not an object")
    return payload


def register(spec: FrameworkSpec, *, builtin: bool = False) -> None:
    if spec.id in _SPECS:
        raise VoiceError(f"framework {spec.id} is already registered")
    _SPECS[spec.id] = spec
    if builtin:
        _BUILTIN_IDS.add(spec.id)


def unregister(framework_id: str) -> None:
    """Drop a framework added with ``register``. Built-ins stay."""
    if framework_id in _BUILTIN_IDS:
        raise VoiceError(f"built-in framework {framework_id} cannot be unregistered")
    if framework_id not in _SPECS:
        raise VoiceError(f"unknown framework {framework_id}")
    del _SPECS[framework_id]


def ensure_builtins() -> None:
    global _LOADED
    if _LOADED:
        return
    from gen_audio.voices.builtins import register_builtins

    register_builtins()
    rows = registry_document()["frameworks"]
    declared = [row["id"] for row in rows]
    missing = [spec_id for spec_id in declared if spec_id not in _BUILTIN_IDS]
    if missing:
        raise VoiceError(f"registry lists frameworks with no adapter: {', '.join(missing)}")
    extra = sorted(_BUILTIN_IDS - set(declared))
    if extra:
        raise VoiceError(f"adapters are not listed in the framework registry: {', '.join(extra)}")
    for row in rows:
        spec = _SPECS[row["id"]]
        engine = row["engine_id"]
        if spec.kind != row["kind"] or spec.wired != row["wired"] or spec.engine_id != engine:
            raise VoiceError(f"framework {row['id']} does not match the registry document")
        if spec.params_schema != row["params_schema"] or spec.summary != row["summary"]:
            raise VoiceError(f"framework {row['id']} schema or summary does not match the registry document")
    _LOADED = True


def get_framework(framework_id: str) -> FrameworkSpec:
    ensure_builtins()
    try:
        return _SPECS[framework_id]
    except KeyError:
        known = ", ".join(sorted(_SPECS))
        raise VoiceError(f"unknown framework {framework_id}; known: {known}") from None


def list_frameworks() -> list[FrameworkSpec]:
    ensure_builtins()
    return [_SPECS[key] for key in sorted(_SPECS)]
