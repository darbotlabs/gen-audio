"""Identity-integer rounding, the Python leg of ASSET_OBJECT_MODEL.md "rounding (B1)".

Same results as Rust ``asset::{ms_from_frames, round_half_up,
bin_frames_inferred}`` and TS ``msFromFrames`` / ``roundHalfUp`` /
``binFramesInferred``; ``schemas/asset-object/vectors/v1.json`` (rounding) is
the shared table.
"""

from __future__ import annotations

import math


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
