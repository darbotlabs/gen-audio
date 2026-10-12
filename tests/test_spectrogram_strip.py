"""Flat 2D spectrogram strip used by audio-clip livetiles (gen_audio.spectrogram_strip)
and the shared identity-integer rounding (gen_audio.identity)."""

from __future__ import annotations

import json
import struct
import zlib
from pathlib import Path

import numpy as np
import pytest

from gen_audio import identity
from gen_audio import spectrogram_strip as module
from gen_audio.audio_io import read_wav
from gen_audio.cli import spectrogram_strip as cli

ROOT = Path(__file__).resolve().parents[1]
VECTORS = ROOT / "schemas/asset-object/vectors/v1.json"


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


def test_tone_lands_in_its_band_and_sidecar_is_integer_identity(tmp_path):
    rate = 24_000
    t = np.arange(rate * 2) / rate
    for float32 in (False, True):
        wav = tmp_path / ("tone_f32.wav" if float32 else "tone.wav")
        _write_wav(wav, 0.5 * np.sin(2 * np.pi * 1000 * t), rate, float32)
        out = tmp_path / "out"
        assert cli.main([str(wav), "--clip-id", "tone", "--out-dir", str(out)]) == 0
        stem = wav.stem
        sidecar = json.loads((out / f"library_{stem}_spec2d.json").read_text())
        fields = sidecar["fields"]
        assert all(isinstance(value, int) for key, value in fields.items() if key != "colormap")
        assert fields["width_px"] == 20 and fields["height_px"] == 64
        assert fields["covers_ms"] <= fields["duration_ms"] == 2000
        assert _png_size(out / sidecar["path"]) == (20, 64)

    samples, got_rate = read_wav(tmp_path / "tone.wav")
    scaled, columns = module.strip(samples, got_rate, 2048, 2400, 64, 60.0, 8000.0, -80)
    loudest_row = int(np.argmax(scaled[:, columns // 2]))
    edges = np.geomspace(60.0, 8000.0, 65)
    band = 63 - loudest_row  # rows are flipped: high frequencies at the top
    assert edges[band] <= 1000.0 <= edges[band + 1]


def test_png_writer_round_trips_pixels(tmp_path):
    rgb = np.zeros((2, 3, 3), dtype=np.uint8)
    rgb[0, 0] = (255, 0, 0)
    path = tmp_path / "x.png"
    module.write_png(path, rgb)
    raw = path.read_bytes()
    idat = raw.index(b"IDAT")
    length = struct.unpack(">I", raw[idat - 4 : idat])[0]
    pixels = zlib.decompress(raw[idat + 4 : idat + 4 + length])
    assert pixels[:4] == b"\x00\xff\x00\x00"


def test_rounding_matches_every_shared_vector():
    by_op: dict[str, int] = {}
    for row in json.loads(VECTORS.read_text(encoding="utf-8"))["rounding"]:
        op = row["op"]
        by_op[op] = by_op.get(op, 0) + 1
        if op == "ms_from_frames":
            got = identity.ms_from_frames(row["frames"], row["rate"])
        elif op == "round_half_up":
            got = identity.round_half_up(row["value"] * row["scale"])
        elif op == "bin_frames_inferred":
            got = identity.bin_frames_inferred(row["duration_s"], row["sample_rate_hz"], row["time_bins"])
        else:
            raise AssertionError(f"unknown rounding op {op}")
        assert got == row["expect"], row
    assert by_op == {"ms_from_frames": 7, "round_half_up": 8, "bin_frames_inferred": 1}, by_op
    assert identity.ms_from_frames(24008, 16000) == 1501  # exact .5 tie rounds up


def test_bin_frames_near_tie_uses_the_binary64_two_step():
    # B1': fl(157.134 * 24000) = 3771215.9999999995, / 96 = 39283.49999999999.
    assert 157.134 * 24000 / 96 < 39283.5
    assert identity.bin_frames_inferred(157.134, 24000, 96) == 39283
    assert (3_771_216 * 2 + 96) // (2 * 96) == 39284  # exact rational math would round up


def test_round_half_up_rejects_what_rust_rejects():
    assert identity.round_half_up(0.5) == 1 and identity.round_half_up(2.5) == 3
    assert identity.round_half_up(0.49999999999999994) == 0
    for bad in (-0.5, float("nan"), float("inf")):
        with pytest.raises(ValueError, match="bad_rounding_input"):
            identity.round_half_up(bad)
    with pytest.raises(ValueError, match="integer_out_of_range"):
        identity.round_half_up(2.0**53)


def _png_pixels(path: Path) -> tuple[bytes, bytes]:
    """IHDR and decompressed scanlines. zlib builds differ (CPython 3.14 on
    Windows ships zlib-ng), so compressed bytes are not comparable; pixels are."""
    raw = path.read_bytes()
    idat = raw.index(b"IDAT")
    length = struct.unpack(">I", raw[idat - 4 : idat])[0]
    return raw[8:33], zlib.decompress(raw[idat + 4 : idat + 4 + length])


@pytest.mark.parametrize("stem", ["bitdot_braille_vibevoice", "genaid_full_misaki_kokoro"])
def test_committed_strips_reproduce_pixel_for_pixel(tmp_path, stem):
    library = ROOT / "apps/desktop/public/library"
    wav = library / f"{stem}.wav"
    if not wav.exists():
        pytest.skip(f"{wav.name} is gitignored and not staged here")
    summary = module.write_strip(wav, "x", tmp_path)
    assert _png_pixels(tmp_path / summary["png"]) == _png_pixels(library / summary["png"])
    committed = json.loads((library / summary["png"].replace(".png", ".json")).read_text(encoding="utf-8"))
    regenerated = json.loads((tmp_path / summary["png"].replace(".png", ".json")).read_text(encoding="utf-8"))
    assert regenerated["fields"] == committed["fields"]


@pytest.mark.parametrize("kwargs", [
    {"hop": 0}, {"n_fft": 1_000_000}, {"n_bands": 1000},
    {"f_min": float("nan")}, {"f_max": float("inf")}, {"db_floor": 0},
    {"hop": 1, "n_fft": 16384},
])
def test_strip_rejects_invalid_or_excessive_allocations(kwargs):
    params = dict(n_fft=2048, hop=2400, n_bands=64, f_min=60.0, f_max=8000.0, db_floor=-80)
    params.update(kwargs)
    with pytest.raises(ValueError):
        module.strip(np.ones(24000), 24000, **params)


def test_strip_refuses_nonfinite_input_and_short_audio():
    for samples in (np.array([float("nan")]), np.ones(2)):
        with pytest.raises(ValueError):
            module.strip(samples, 24000, 2048, 2400, 64, 60.0, 8000.0, -80)


def test_strip_publication_collision_preserves_existing_pair(tmp_path):
    wav = tmp_path / "tone.wav"
    _write_wav(wav, np.sin(np.arange(24000) / 10), 24000, True)
    out = tmp_path / "out"
    summary = module.write_strip(wav, "tone", out)
    png = out / summary["png"]
    sidecar = png.with_suffix(".json")
    before = (png.read_bytes(), sidecar.read_bytes())
    with pytest.raises(FileExistsError):
        module.write_strip(wav, "different-id", out)
    assert (png.read_bytes(), sidecar.read_bytes()) == before
    assert not list(out.glob("*.partial"))


def test_strip_publication_failure_rolls_back_png(tmp_path, monkeypatch):
    from gen_audio import artifacts

    wav = tmp_path / "tone.wav"
    _write_wav(wav, np.sin(np.arange(24000) / 10), 24000, True)
    original = artifacts.os.link

    def fail_sidecar(source, destination):
        if Path(destination).suffix == ".json":
            raise OSError("sidecar publication failed")
        original(source, destination)

    monkeypatch.setattr(artifacts.os, "link", fail_sidecar)
    out = tmp_path / "out"
    with pytest.raises(OSError, match="sidecar publication"):
        module.write_strip(wav, "tone", out)
    assert list(out.iterdir()) == []
