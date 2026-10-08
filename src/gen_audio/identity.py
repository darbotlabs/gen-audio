"""Asset object model v1 identity: rounding (B1) and uid minting.

Rounding matches Rust ``asset::{ms_from_frames, round_half_up, bin_frames_inferred}``
and ``schemas/asset-object/vectors/v1.json``. Uids use the same preimage as the
Rust minter:

preimage = UTF-8("ga-asset-v1") || 0x00 || JCS(identity)
uid = "ga:{kind}:" + base32(SHA-256(preimage)[:16])   RFC 4648, lowercase, no padding

``read_wav`` is the single reader in :mod:`gen_audio.audio_io`, re-exported
here so callers share one function with ``round_half_up`` and ``ms_from_frames``.
"""

from __future__ import annotations

import hashlib
import math
from typing import Any

from gen_audio.audio_io import read_wav as read_wav

DOMAIN_TAG = b"ga-asset-v1"
SCHEMA_MAJOR = 1
_ALPHABET = "abcdefghijklmnopqrstuvwxyz234567"


def ms_from_frames(frames: int, rate: int) -> int:
    """Whole ms from a frame count, round half up in exact integers."""
    return (frames * 1000 + rate // 2) // rate if rate > 0 else 0


def round_half_up(value: float) -> int:
    """Round a non-negative binary64 half up, as Rust ``round_half_up``.

    ``floor(x)`` plus one when the fraction is at least 0.5 (the subtraction
    is exact in binary64). Negative or non-finite input is
    ``bad_rounding_input``; a result above 2**53-1 is ``integer_out_of_range``.
    """
    if not math.isfinite(value) or value < 0:
        raise ValueError(f"bad_rounding_input: {value!r} must be finite and >= 0")
    whole = math.floor(value)
    rounded = int(whole) + (1 if value - whole >= 0.5 else 0)
    if rounded > 2**53 - 1:
        raise ValueError(f"integer_out_of_range: {value!r} rounds outside 2^53-1")
    return rounded


def bin_frames_inferred(duration_s: float, sample_rate_hz: int, time_bins: int) -> int:
    """Inferred cube bin_frames: ``round_half_up(fl(fl(duration_s * sr) / time_bins))``.

    Exactly two binary64 operations, multiply first, from the cube JSON's float
    ``duration_s`` (never frames). 157.134 s, 24 kHz, 96 bins gives 39283.
    """
    product = float(duration_s) * float(sample_rate_hz)
    return round_half_up(product / float(max(1, time_bins)))


def canonicalize(value: Any) -> str:
    """RFC 8785 for the identity subset: sorted keys, no whitespace, raw UTF-8."""
    return _write(value)


def _write(value: Any) -> str:
    if value is None:
        raise ValueError("null is not allowed in an identity")
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int) and not isinstance(value, bool):
        return str(value)
    if isinstance(value, float):
        raise ValueError("identity fields are integers")
    if isinstance(value, str):
        return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'
    if isinstance(value, list):
        return "[" + ",".join(_write(item) for item in value) + "]"
    if isinstance(value, dict):
        parts = []
        for key in sorted(value):
            if not isinstance(key, str):
                raise ValueError("identity keys are strings")
            parts.append(_write(key) + ":" + _write(value[key]))
        return "{" + ",".join(parts) + "}"
    raise ValueError(f"unsupported identity value {type(value).__name__}")


def base32_lower(data: bytes) -> str:
    """RFC 4648 base32, lowercase, no padding. Leftover bits are zero-filled."""
    out: list[str] = []
    buffer = 0
    bits = 0
    for byte in data:
        buffer = (buffer << 8) | byte
        bits += 8
        while bits >= 5:
            bits -= 5
            out.append(_ALPHABET[(buffer >> bits) & 31])
    if bits:
        out.append(_ALPHABET[(buffer << (5 - bits)) & 31])
    return "".join(out)


def mint(kind: str, fields: dict, media: list[dict], src: list[str]) -> str:
    """Return the ``ga:`` uid for one identity."""
    media_sorted = sorted(
        ({"role": item["role"], "sha256": item["sha256"]} for item in media),
        key=lambda item: (item["role"], item["sha256"]),
    )
    identity = {
        "kind": kind,
        "schema_major": SCHEMA_MAJOR,
        "fields": fields,
        "media": media_sorted,
        "src": sorted(src),
    }
    canonical = canonicalize(identity)
    preimage = DOMAIN_TAG + b"\x00" + canonical.encode("utf-8")
    digest = hashlib.sha256(preimage).digest()
    return f"ga:{kind}:{base32_lower(digest[:16])}"
