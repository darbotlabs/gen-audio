"""Serial revision sketch driven by inv-HDR, BW95, and clip fraction.

This is a heuristic, not a mastering standard and not a model of hearing.
Each step remeasures the three values and applies at most one operation that
actually changes the samples, then measures again.

Definitions used here:

* ``inv_hdr`` is ``rms / peak`` (the inverse of the linear crest factor).
  ``hdr_db`` is also reported as ``20 log10(peak / rms)`` when both are nonzero.
* ``bw95_hz`` is the width, in hertz, between the 2.5 and 97.5 percentiles of
  Hann-weighted rFFT power. That is the band holding the central 95% of power.
* ``clip_frac`` is the fraction of samples with absolute value at or above 0.99.

The publish chain in ``improve.py`` is separate. This module does not claim
to undo clipping distortion; ``reduce_clip`` only scales or limits the peak.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass

import numpy as np
from scipy import signal

from gen_audio.audio_io import as_mono
from gen_audio.improve import PEAK_TARGET

CLIP_LEVEL = 0.99
CLIP_FRAC_MAX = 1e-4
INV_HDR_DENSE = 0.45
INV_HDR_SPARSE = 0.08
BW95_NARROW_HZ = 2500.0
DEFAULT_MAX_STEPS = 4


@dataclass(frozen=True)
class CubeThresholds:
    """Knobs for the sketch. Defaults are starting points, not a spec."""

    clip_level: float = CLIP_LEVEL
    clip_frac_max: float = CLIP_FRAC_MAX
    inv_hdr_dense: float = INV_HDR_DENSE
    inv_hdr_sparse: float = INV_HDR_SPARSE
    bw95_narrow_hz: float = BW95_NARROW_HZ
    peak_target: float = PEAK_TARGET


@dataclass(frozen=True)
class CubeMetrics:
    """One measurement of the inverse-HDR cube."""

    inv_hdr: float
    hdr_db: float
    bw95_hz: float
    clip_frac: float
    peak: float
    rms: float
    n: int
    sample_rate: int

    def point(self) -> tuple[float, float, float]:
        """Return ``(inv_hdr, bw95 / nyquist, clip_frac)`` for a unit-cube plot."""
        nyquist = self.sample_rate / 2.0 if self.sample_rate else 0.0
        bandwidth = 0.0 if nyquist <= 0.0 else min(1.0, self.bw95_hz / nyquist)
        return (self.inv_hdr, bandwidth, self.clip_frac)

    def to_dict(self) -> dict[str, float | int]:
        return asdict(self)


@dataclass(frozen=True)
class RevisionResult:
    """Audio after the serial pass, with the ops that changed samples."""

    audio: np.ndarray
    sample_rate: int
    before: CubeMetrics
    after: CubeMetrics
    ops: list[str]


def measure(
    audio: np.ndarray,
    sample_rate: int,
    thresholds: CubeThresholds | None = None,
) -> CubeMetrics:
    """Measure inv-HDR, BW95, and clip fraction."""
    limits = thresholds or CubeThresholds()
    values = as_mono(audio)
    rate = int(sample_rate)
    count = int(values.size)
    if count == 0 or rate <= 0:
        return CubeMetrics(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, count, rate)
    peak = float(np.max(np.abs(values)))
    rms = float(np.sqrt(np.mean(values * values)))
    if peak < 1e-12:
        inv_hdr = 0.0
        hdr_db = 0.0
    else:
        inv_hdr = float(rms / peak)
        hdr_db = float(20.0 * np.log10(peak / rms)) if rms > 1e-12 else 0.0
    clip_frac = float(np.mean(np.abs(values) >= limits.clip_level))
    return CubeMetrics(
        inv_hdr=inv_hdr,
        hdr_db=hdr_db,
        bw95_hz=bandwidth_95(values, rate),
        clip_frac=clip_frac,
        peak=peak,
        rms=rms,
        n=count,
        sample_rate=rate,
    )


def bandwidth_95(audio: np.ndarray, sample_rate: int) -> float:
    """Hertz between the 2.5 and 97.5 percentiles of rFFT power."""
    values = as_mono(audio)
    if values.size < 16 or int(sample_rate) <= 0:
        return 0.0
    window = np.hanning(values.size)
    power = np.abs(np.fft.rfft(values * window)) ** 2
    total = float(power.sum())
    if total <= 0.0:
        return 0.0
    cumulative = np.cumsum(power) / total
    freqs = np.fft.rfftfreq(values.size, d=1.0 / float(sample_rate))
    low_index = int(np.searchsorted(cumulative, 0.025, side="left"))
    high_index = int(np.searchsorted(cumulative, 0.975, side="left"))
    low_index = min(low_index, freqs.size - 1)
    high_index = min(high_index, freqs.size - 1)
    return float(max(0.0, freqs[high_index] - freqs[low_index]))


def decide_ops(metrics: CubeMetrics, thresholds: CubeThresholds | None = None) -> list[str]:
    """List candidate ops in priority order. The reviser applies them one at a time."""
    limits = thresholds or CubeThresholds()
    ops: list[str] = []
    if metrics.clip_frac > limits.clip_frac_max:
        ops.append("reduce_clip")
    if metrics.inv_hdr > limits.inv_hdr_dense:
        ops.append("expand_crest")
    elif metrics.inv_hdr < limits.inv_hdr_sparse and metrics.peak > 0.0:
        ops.append("lift_body")
    if metrics.bw95_hz < limits.bw95_narrow_hz and metrics.n >= 16:
        ops.append("widen_band")
    return ops


def revise(
    audio: np.ndarray,
    sample_rate: int,
    *,
    thresholds: CubeThresholds | None = None,
    max_steps: int = DEFAULT_MAX_STEPS,
) -> RevisionResult:
    """Apply ops serially, remeasuring after every op that changes the audio."""
    if int(max_steps) < 0:
        raise ValueError("max_steps must be >= 0")
    rate = int(sample_rate)
    current = as_mono(audio).astype(np.float32)
    if current.size == 0:
        raise ValueError("audio is empty")
    limits = thresholds or CubeThresholds()
    before = measure(current, rate, limits)
    applied: list[str] = []
    for _ in range(int(max_steps)):
        moved = False
        for op_name in decide_ops(measure(current, rate, limits), limits):
            updated = apply_op(op_name, current, rate, limits)
            if np.allclose(updated, current, rtol=0.0, atol=1e-7):
                continue
            current = np.ascontiguousarray(updated, dtype=np.float32)
            applied.append(op_name)
            moved = True
            break
        if not moved:
            break
    after = measure(current, rate, limits)
    return RevisionResult(
        audio=current,
        sample_rate=rate,
        before=before,
        after=after,
        ops=applied,
    )


def apply_op(
    name: str,
    audio: np.ndarray,
    sample_rate: int,
    thresholds: CubeThresholds | None = None,
) -> np.ndarray:
    """Apply one named op. Unknown names raise ``ValueError``."""
    limits = thresholds or CubeThresholds()
    values = as_mono(audio)
    if name == "reduce_clip":
        return reduce_clip(values, limits.peak_target)
    if name == "expand_crest":
        return expand_crest(values)
    if name == "lift_body":
        return lift_body(values)
    if name == "widen_band":
        return widen_band(values, int(sample_rate))
    raise ValueError(f"unknown cube op {name!r}")


def reduce_clip(audio: np.ndarray, target: float = PEAK_TARGET) -> np.ndarray:
    """Bring the peak down to ``target``. This does not reconstruct clipped peaks."""
    values = as_mono(audio)
    peak = float(np.max(np.abs(values))) if values.size else 0.0
    if peak < 1e-12:
        return values
    if peak > target:
        values = values * (float(target) / peak)
    return np.clip(values, -float(target), float(target))


def expand_crest(audio: np.ndarray, *, knee_ratio: float = 0.35, quiet_gain: float = 0.55) -> np.ndarray:
    """Attenuate samples under a fraction of the peak so crest can increase.

    A signal that is already flat at the peak is unchanged, because nothing
    sits under the knee.
    """
    values = as_mono(audio)
    peak = float(np.max(np.abs(values))) if values.size else 0.0
    if peak < 1e-12:
        return values
    quiet = np.abs(values) < (knee_ratio * peak)
    expanded = values.copy()
    expanded[quiet] *= quiet_gain
    return expanded


def lift_body(audio: np.ndarray, *, power: float = 0.75) -> np.ndarray:
    """Raise quieter magnitudes with a power curve while keeping the same peak.

    Samples at 0 and at the peak stay put. A lone impulse in digital silence
    therefore does not move; a low-level body under a louder peak does.
    """
    values = as_mono(audio)
    peak = float(np.max(np.abs(values))) if values.size else 0.0
    if peak < 1e-12:
        return values
    magnitude = np.abs(values) / peak
    lifted = np.sign(values) * np.power(magnitude, power) * peak
    return lifted


def widen_band(audio: np.ndarray, sample_rate: int, *, mix: float = 0.35, shelf_hz: float = 1800.0) -> np.ndarray:
    """Blend in a high-passed copy, then restore the original peak.

    A linear filter cannot add frequencies a pure tone does not have. The
    blend only changes bandwidth when some energy is already present above
    the shelf and is being masked by a louder low band.
    """
    values = as_mono(audio)
    rate = int(sample_rate)
    if values.size < 32 or rate < 8_000:
        return values
    nyquist = rate / 2.0
    if shelf_hz <= 0.0 or shelf_hz >= nyquist * 0.9:
        return values
    sos = signal.butter(2, float(shelf_hz), btype="highpass", fs=rate, output="sos")
    try:
        high = signal.sosfiltfilt(sos, values)
    except ValueError:
        return values
    mixed = values + (float(mix) * high)
    before_peak = float(np.max(np.abs(values)))
    after_peak = float(np.max(np.abs(mixed)))
    if before_peak > 0.0 and after_peak > 1e-12:
        mixed = mixed * (before_peak / after_peak)
    return mixed


def write_cube_plot(path, before: CubeMetrics, after: CubeMetrics) -> None:
    """Scatter the before and after points on the inverse-HDR cube axes."""
    from pathlib import Path

    from matplotlib.backends.backend_agg import FigureCanvasAgg
    from matplotlib.figure import Figure
    from mpl_toolkits.mplot3d import Axes3D  # noqa: F401  (registers the 3d projection)

    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    figure = Figure(figsize=(6.4, 5.2))
    FigureCanvasAgg(figure)
    axes = figure.add_subplot(111, projection="3d")
    start = before.point()
    end = after.point()
    axes.scatter([start[0]], [start[1]], [start[2]], s=40, label="before")
    axes.scatter([end[0]], [end[1]], [end[2]], s=40, label="after")
    axes.plot([start[0], end[0]], [start[1], end[1]], [start[2], end[2]])
    axes.set_xlabel("inv-HDR (rms/peak)")
    axes.set_ylabel("BW95 / Nyquist")
    axes.set_zlabel("clip fraction")
    axes.set_title("Inverse-HDR cube (sketch)")
    axes.legend(loc="upper left")
    figure.tight_layout()
    figure.savefig(destination, dpi=120)
