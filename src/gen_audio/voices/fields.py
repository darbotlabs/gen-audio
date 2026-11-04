"""Small checkers shared by the voice schemas' Python validators.

JSON Schema in ``schemas/voices/`` is the document contract. These helpers
enforce the same shape in the loader, plus the cross-field rules schema
cannot express (id prefix, weight sums, pack gender, path escape).
"""

from __future__ import annotations

import math
import re
from typing import Any

from gen_audio.voices.errors import VoiceError

_FRAMEWORK_RE = re.compile(r"^[a-z][a-z0-9_]{0,31}\Z")
_VOICE_ID_RE = re.compile(r"^[a-z][a-z0-9_]{0,31}\.[A-Za-z0-9][A-Za-z0-9_.-]{0,63}\Z")
_LANG_RE = re.compile(r"^[a-z]{2,3}(-[a-z0-9]{2,8})?\Z")
_TAG_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,31}\Z")
_PATH_RE = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_./+-]{0,219}\Z")
_SLUG_RE = re.compile(r"^[a-z][a-z0-9_]{0,31}\Z")
_HTTPS_RE = re.compile(r"^https://\S+\Z")


def reject_unknown(payload: dict, allowed: set[str], label: str) -> None:
    extra = sorted(set(payload) - allowed)
    if extra:
        raise VoiceError(f"{label} has unknown fields: {', '.join(extra)}")


def require_object(value: Any, label: str) -> dict:
    if not isinstance(value, dict):
        raise VoiceError(f"{label} must be an object")
    return value


def require_str(
    payload: dict,
    key: str,
    *,
    label: str,
    pattern: re.Pattern[str] | None = None,
    min_len: int = 1,
    max_len: int = 80,
) -> str:
    value = payload.get(key)
    if not isinstance(value, str):
        raise VoiceError(f"{label} field {key} must be a string")
    text = value.strip()
    if text != value:
        raise VoiceError(f"{label} field {key} must not have surrounding space")
    if len(text) < min_len or len(text) > max_len:
        raise VoiceError(f"{label} field {key} length must be {min_len}..{max_len}")
    if pattern is not None and pattern.fullmatch(text) is None:
        raise VoiceError(f"{label} field {key} has the wrong form")
    return text


def optional_str(
    payload: dict,
    key: str,
    *,
    label: str,
    pattern: re.Pattern[str] | None = None,
    min_len: int = 1,
    max_len: int = 80,
) -> str | None:
    if key not in payload:
        return None
    return require_str(payload, key, label=label, pattern=pattern, min_len=min_len, max_len=max_len)


def require_bool(payload: dict, key: str, *, label: str) -> bool:
    value = payload.get(key)
    if not isinstance(value, bool):
        raise VoiceError(f"{label} field {key} must be a boolean")
    return value


def optional_bool(payload: dict, key: str, *, label: str) -> bool | None:
    if key not in payload:
        return None
    return require_bool(payload, key, label=label)


def require_number(
    payload: dict,
    key: str,
    *,
    label: str,
    minimum: float | None = None,
    maximum: float | None = None,
    exclusive_minimum: float | None = None,
) -> float:
    value = payload.get(key)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise VoiceError(f"{label} field {key} must be a number")
    number = float(value)
    if not math.isfinite(number):
        raise VoiceError(f"{label} field {key} must be finite")
    if minimum is not None and number < minimum:
        raise VoiceError(f"{label} field {key} must be >= {minimum}")
    if maximum is not None and number > maximum:
        raise VoiceError(f"{label} field {key} must be <= {maximum}")
    if exclusive_minimum is not None and number <= exclusive_minimum:
        raise VoiceError(f"{label} field {key} must be > {exclusive_minimum}")
    return number


def require_int(payload: dict, key: str, *, label: str, minimum: int, maximum: int) -> int:
    value = payload.get(key)
    if isinstance(value, bool) or not isinstance(value, int):
        raise VoiceError(f"{label} field {key} must be an integer")
    if value < minimum or value > maximum:
        raise VoiceError(f"{label} field {key} must be {minimum}..{maximum}")
    return value


def check_rel_path(path: str, *, label: str) -> str:
    if not isinstance(path, str) or _PATH_RE.fullmatch(path) is None:
        raise VoiceError(f"{label} must be a relative library path")
    parts = path.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        raise VoiceError(f"{label} must stay inside the voice library")
    return path


def check_slug(value: str, *, label: str) -> str:
    if not isinstance(value, str) or _SLUG_RE.fullmatch(value) is None:
        raise VoiceError(f"{label} must be a lowercase slug")
    return value


def check_lang(value: str, *, label: str) -> str:
    if not isinstance(value, str) or _LANG_RE.fullmatch(value) is None:
        raise VoiceError(f"{label} must be a language tag")
    return value


def check_voice_id(value: str, *, label: str) -> str:
    if not isinstance(value, str) or _VOICE_ID_RE.fullmatch(value) is None:
        raise VoiceError(f"{label} must be framework.local-id")
    return value


def check_framework_id(value: str, *, label: str) -> str:
    if not isinstance(value, str) or _FRAMEWORK_RE.fullmatch(value) is None:
        raise VoiceError(f"{label} must be a framework id")
    return value


def check_tag(value: str) -> str:
    if not isinstance(value, str) or _TAG_RE.fullmatch(value) is None:
        raise VoiceError(f"tag {value!r} has the wrong form")
    return value


def check_https(value: str, *, label: str) -> str:
    if not isinstance(value, str) or _HTTPS_RE.fullmatch(value) is None or len(value) > 300:
        raise VoiceError(f"{label} must be an https URL")
    return value
