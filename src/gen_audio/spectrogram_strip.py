"""Flat 2D spectrogram strip (PNG + sidecar JSON) for a library WAV.

The strip is what the desktop livetile autoplays under its playhead: one
column per ``hop`` samples (10 columns per second at 24 kHz by default), log
spaced frequency bands from low (bottom) to high (top), dB magnitude mapped
through a fixed colormap. Identity fields in the sidecar are integers (asset
object model v1); floats stay in ``params``. WAV input goes through
``gen_audio.identity.read_wav``; the PNG is written with zlib (no matplotlib).
"""

from __future__ import annotations

import json
import struct
import zlib
from dataclasses import dataclass
from pathlib import Path

import numpy as np

from gen_audio.identity import ms_from_frames, read_wav

# Fixed 5-anchor colormap ("ga-ember"): near-black -> indigo -> magenta -> orange -> pale yellow.
ANCHORS = np.array(
    [
        [8, 9, 18],
        [52, 28, 110],
        [170, 46, 120],
        [246, 122, 44],
        [252, 236, 164],
    ],
    dtype=np.float64,
)


@dataclass(frozen=True)
class StripParams:
    n_fft: int = 2048
    columns_per_second: int = 10
    bands: int = 64
    f_min: float = 60.0
    f_max: float = 8000.0
    db_floor: int = -80


def colormap(values: np.ndarray) -> np.ndarray:
    pos = np.clip(values, 0.0, 1.0) * (len(ANCHORS) - 1)
    low = np.floor(pos).astype(int)
    high = np.minimum(low + 1, len(ANCHORS) - 1)
    frac = (pos - low)[..., None]
    rgb = ANCHORS[low] * (1.0 - frac) + ANCHORS[high] * frac
    return np.round(rgb).astype(np.uint8)


def write_png(path: Path, rgb: np.ndarray) -> None:
    height, width, _ = rgb.shape
    raw = b"".join(b"\x00" + rgb[row].tobytes() for row in range(height))

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def strip(samples: np.ndarray, rate: int, n_fft: int, hop: int, n_bands: int, f_min: float, f_max: float, db_floor: int):
    """Scaled 0..1 band magnitudes, rows flipped (high frequencies on top), and the column count."""
    samples = np.asarray(samples, dtype=np.float64)
    columns = len(samples) // hop
    window = np.hanning(n_fft)
    padded = np.concatenate([np.zeros(n_fft // 2), samples, np.zeros(n_fft)])
    freqs = np.fft.rfftfreq(n_fft, 1.0 / rate)
    edges = np.geomspace(f_min, min(f_max, rate / 2), n_bands + 1)
    band_of = np.digitize(freqs, edges) - 1
    # Bands narrower than one FFT bin (low end of the geometric scale) take the
    # bin nearest their center instead of staying empty.
    masks = []
    for band in range(n_bands):
        mask = band_of == band
        if not mask.any():
            center_hz = np.sqrt(edges[band] * edges[band + 1])
            mask = np.zeros_like(mask)
            mask[int(np.argmin(np.abs(freqs - center_hz)))] = True
        masks.append(mask)
    mags = np.zeros((n_bands, columns))
    for col in range(columns):
        center = col * hop + hop // 2
        frame = padded[center : center + n_fft] * window
        power = np.abs(np.fft.rfft(frame)) ** 2
        for band, mask in enumerate(masks):
            mags[band, col] = power[mask].mean()
    db = 10.0 * np.log10(mags + 1e-12)
    db -= db.max()
    scaled = (np.clip(db, db_floor, 0.0) - db_floor) / -db_floor
    return scaled[::-1, :], columns


def write_strip(wav: Path, clip_id: str, out_dir: Path, params: StripParams = StripParams()) -> dict:
    """Write ``library_<stem>_spec2d.png`` and its sidecar JSON; return a short summary."""
    samples, rate = read_wav(wav)
    hop = rate // params.columns_per_second
    scaled, columns = strip(samples, rate, params.n_fft, hop, params.bands, params.f_min, params.f_max, params.db_floor)
    png_name = f"library_{Path(wav).stem.removeprefix('library_')}_spec2d.png"
    out_dir.mkdir(parents=True, exist_ok=True)
    write_png(out_dir / png_name, colormap(scaled))
    duration_ms = ms_from_frames(len(samples), rate)
    covers_ms = ms_from_frames(columns * hop, rate)
    sidecar = {
        "clip_id": clip_id,
        "path": png_name,
        "fields": {
            "sample_rate_hz": rate,
            "n_fft": params.n_fft,
            "hop_frames": hop,
            "n_bands": params.bands,
            "width_px": columns,
            "height_px": params.bands,
            "duration_ms": duration_ms,
            "covers_ms": covers_ms,
            "db_floor": params.db_floor,
            "colormap": "ga-ember",
        },
        "params": {
            "f_min_hz": params.f_min,
            "f_max_hz": min(params.f_max, rate / 2),
            "band_spacing": "geometric",
            "seconds_per_px": hop / rate,
            "window": "hann",
        },
    }
    (out_dir / png_name.replace(".png", ".json")).write_text(json.dumps(sidecar, indent=2) + "\n", encoding="utf-8")
    return {"png": png_name, "columns": columns, "duration_ms": duration_ms, "covers_ms": covers_ms}
