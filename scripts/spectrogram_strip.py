#!/usr/bin/env python3
"""Write a flat 2D spectrogram strip PNG (+ sidecar JSON) for a library WAV.

The strip is what the desktop livetile autoplays under its playhead: one
column per `hop` samples (10 columns per second at 24 kHz by default), log
spaced frequency bands from low (bottom) to high (top), dB magnitude mapped
through a fixed colormap. Integers only in the identity fields of the
sidecar (asset object model v1); floats stay in `params`.

Needs numpy. No matplotlib: the PNG is written with zlib.
"""

from __future__ import annotations

import argparse
import json
import struct
import zlib
from pathlib import Path

import numpy as np

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


def read_wav(path: Path) -> tuple[np.ndarray, int]:
    """Minimal RIFF reader: PCM 16-bit (format 1) or IEEE float32 (format 3)."""
    data = path.read_bytes()
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise SystemExit(f"{path}: not a RIFF/WAVE file")
    offset, fmt, payload = 12, None, None
    while offset + 8 <= len(data):
        tag, size = data[offset : offset + 4], struct.unpack("<I", data[offset + 4 : offset + 8])[0]
        body = data[offset + 8 : offset + 8 + size]
        if tag == b"fmt ":
            fmt = struct.unpack("<HHIIHH", body[:16])
        elif tag == b"data":
            payload = body
        offset += 8 + size + (size & 1)
    if fmt is None or payload is None:
        raise SystemExit(f"{path}: missing fmt or data chunk")
    format_tag, channels, rate, _, _, bits = fmt
    if format_tag == 1 and bits == 16:
        samples = np.frombuffer(payload, dtype="<i2").astype(np.float64) / 32768.0
    elif format_tag == 3 and bits == 32:
        samples = np.frombuffer(payload, dtype="<f4").astype(np.float64)
    else:
        raise SystemExit(f"{path}: unsupported format {format_tag}/{bits}-bit")
    if channels > 1:
        samples = samples[: len(samples) // channels * channels].reshape(-1, channels).mean(axis=1)
    return samples, rate


def strip(samples: np.ndarray, rate: int, n_fft: int, hop: int, n_bands: int, f_min: float, f_max: float, db_floor: int):
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wav", type=Path)
    parser.add_argument("--clip-id", required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--n-fft", type=int, default=2048)
    parser.add_argument("--columns-per-second", type=int, default=10)
    parser.add_argument("--bands", type=int, default=64)
    parser.add_argument("--f-min", type=float, default=60.0)
    parser.add_argument("--f-max", type=float, default=8000.0)
    parser.add_argument("--db-floor", type=int, default=-80)
    args = parser.parse_args()

    samples, rate = read_wav(args.wav)
    hop = rate // args.columns_per_second
    scaled, columns = strip(samples, rate, args.n_fft, hop, args.bands, args.f_min, args.f_max, args.db_floor)
    stem = args.wav.stem
    png_name = f"library_{stem.removeprefix('library_')}_spec2d.png"
    args.out_dir.mkdir(parents=True, exist_ok=True)
    write_png(args.out_dir / png_name, colormap(scaled))
    duration_ms = (len(samples) * 1000 + rate // 2) // rate
    covers_ms = columns * hop * 1000 // rate
    sidecar = {
        "clip_id": args.clip_id,
        "path": png_name,
        "fields": {
            "sample_rate_hz": rate,
            "n_fft": args.n_fft,
            "hop_frames": hop,
            "n_bands": args.bands,
            "width_px": columns,
            "height_px": args.bands,
            "duration_ms": duration_ms,
            "covers_ms": covers_ms,
            "db_floor": args.db_floor,
            "colormap": "ga-ember",
        },
        "params": {
            "f_min_hz": args.f_min,
            "f_max_hz": min(args.f_max, rate / 2),
            "band_spacing": "geometric",
            "seconds_per_px": hop / rate,
            "window": "hann",
        },
    }
    (args.out_dir / png_name.replace(".png", ".json")).write_text(json.dumps(sidecar, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"png": png_name, "columns": columns, "duration_ms": duration_ms, "covers_ms": covers_ms}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
