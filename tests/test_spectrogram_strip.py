"""Flat 2D spectrogram strip used by audio-clip livetiles (scripts/spectrogram_strip.py)."""

from __future__ import annotations

import importlib.util
import json
import struct
import sys
import zlib
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]


def _load():
    spec = importlib.util.spec_from_file_location("spectrogram_strip", ROOT / "scripts" / "spectrogram_strip.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def _write_wav(path: Path, samples: np.ndarray, rate: int, float32: bool) -> None:
    if float32:
        data = samples.astype("<f4").tobytes()
        fmt = struct.pack("<HHIIHH", 3, 1, rate, rate * 4, 4, 32)
    else:
        data = (np.clip(samples, -1, 1) * 32767).astype("<i2").tobytes()
        fmt = struct.pack("<HHIIHH", 1, 1, rate, rate * 2, 2, 16)
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data
    path.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)


def _png_size(path: Path) -> tuple[int, int]:
    raw = path.read_bytes()
    assert raw[:8] == b"\x89PNG\r\n\x1a\n"
    width, height = struct.unpack(">II", raw[16:24])
    return width, height


def test_tone_lands_in_its_band_and_sidecar_is_integer_identity(tmp_path, monkeypatch):
    module = _load()
    rate = 24_000
    t = np.arange(rate * 2) / rate
    for float32 in (False, True):
        wav = tmp_path / ("tone_f32.wav" if float32 else "tone.wav")
        _write_wav(wav, 0.5 * np.sin(2 * np.pi * 1000 * t), rate, float32)
        out = tmp_path / "out"
        monkeypatch.setattr(sys, "argv", ["spectrogram_strip.py", str(wav), "--clip-id", "tone", "--out-dir", str(out)])
        assert module.main() == 0
        stem = wav.stem
        sidecar = json.loads((out / f"library_{stem}_spec2d.json").read_text())
        fields = sidecar["fields"]
        assert all(isinstance(value, int) for key, value in fields.items() if key != "colormap")
        assert fields["width_px"] == 20 and fields["height_px"] == 64
        assert fields["covers_ms"] <= fields["duration_ms"] == 2000
        assert _png_size(out / sidecar["path"]) == (20, 64)

    samples, got_rate = module.read_wav(tmp_path / "tone.wav")
    scaled, columns = module.strip(samples, got_rate, 2048, 2400, 64, 60.0, 8000.0, -80)
    loudest_row = int(np.argmax(scaled[:, columns // 2]))
    edges = np.geomspace(60.0, 8000.0, 65)
    band = 63 - loudest_row  # rows are flipped: high frequencies at the top
    assert edges[band] <= 1000.0 <= edges[band + 1]


def test_png_writer_round_trips_pixels(tmp_path):
    module = _load()
    rgb = np.zeros((2, 3, 3), dtype=np.uint8)
    rgb[0, 0] = (255, 0, 0)
    path = tmp_path / "x.png"
    module.write_png(path, rgb)
    raw = path.read_bytes()
    idat = raw.index(b"IDAT")
    length = struct.unpack(">I", raw[idat - 4 : idat])[0]
    pixels = zlib.decompress(raw[idat + 4 : idat + 4 + length])
    assert pixels[:4] == b"\x00\xff\x00\x00"
