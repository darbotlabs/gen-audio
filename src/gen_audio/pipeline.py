"""Analysis pipeline: improve, 2D spectrogram, inverse-HDR cube, compare.

Every step writes an asset object. Spectrogram and cube ``duration_s`` are the
fitted WAV's sample count divided by its sample rate.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

from gen_audio.asr_wer import AsrError, transcribe, word_error_rate
from gen_audio.assets import asset_object
from gen_audio.audio_io import read_wav, write_wav
from gen_audio.cube_revision import bandwidth_95, measure
from gen_audio.identity import ms_from_frames, round_half_up
from gen_audio.improve import improve
from gen_audio.spectrogram_strip import colormap, strip, write_png
from gen_audio.video_render import VideoError, render_clip_mp4

LAYERS = ("signal", "tonality", "confidence", "quality")


def duration_seconds(audio: np.ndarray, sample_rate: int) -> float:
    """Exact duration of a buffer. Callers store this value unchanged."""
    if int(sample_rate) <= 0:
        raise ValueError("sample rate must be positive")
    return float(audio.size) / float(sample_rate)


def fit_duration(audio: np.ndarray, sample_rate: int, target_s: float) -> tuple[np.ndarray, bool]:
    """Pad with silence up to ``target_s``. The bool is True when silence was added.

    Longer speech is kept, not trimmed. Padding never counts as an honoured duration.
    """
    target = int(round(float(target_s) * int(sample_rate)))
    if audio.size < target:
        pad = np.zeros(target - audio.size, dtype=np.float32)
        return np.concatenate([audio.astype(np.float32), pad]), True
    return audio.astype(np.float32), False


def run_pipeline(
    raw_wav: Path,
    *,
    out_dir: Path,
    engine: str,
    reference_text: str,
    duration_target_s: float,
    script_asset: dict,
) -> dict:
    """Run improve → spectrogram → cube → compare → video on one WAV."""
    out_dir.mkdir(parents=True, exist_ok=True)
    raw_audio, raw_sr = read_wav(raw_wav)
    raw_asset = asset_object(
        raw_wav,
        kind="wav",
        derived_from=[script_asset["uid"]],
        duration_s=duration_seconds(raw_audio, raw_sr),
        engine=engine,
        sample_rate=raw_sr,
        n_frames=int(raw_audio.size),
    )

    improved = improve(raw_audio, raw_sr)
    speech_s = duration_seconds(improved.audio, improved.sample_rate)
    fitted, padded = fit_duration(improved.audio, improved.sample_rate, duration_target_s)
    duration_s = duration_seconds(fitted, improved.sample_rate)
    fitted_path = out_dir / "fitted.wav"
    write_wav(fitted_path, fitted, improved.sample_rate, subtype="PCM_16")
    # Re-read so the stored duration matches the file on disk, including PCM rounding.
    fitted, fitted_sr = read_wav(fitted_path)
    duration_s = duration_seconds(fitted, fitted_sr)
    wav_asset = asset_object(
        fitted_path,
        kind="wav",
        derived_from=[raw_asset["uid"]],
        duration_s=duration_s,
        engine=engine,
        sample_rate=fitted_sr,
        n_frames=int(fitted.size),
    )

    spec_png = out_dir / "spectrogram.png"
    hop = max(fitted_sr // 10, 1)
    n_fft = 2048 if fitted.size >= 2048 else 256
    scaled, columns = strip(np.asarray(fitted, dtype=np.float64), fitted_sr, n_fft, hop, 64, 60.0, 8000.0, -80)
    write_png(spec_png, colormap(scaled))
    spec_json_path = out_dir / "spectrogram.json"
    duration_ms = ms_from_frames(int(fitted.size), int(fitted_sr))
    covers_ms = ms_from_frames(int(columns) * int(hop), int(fitted_sr))
    spec_fields = {
        "source_sha256": wav_asset["sha256"],
        "sample_rate_hz": int(fitted_sr),
        "n_fft": int(n_fft),
        "hop_frames": int(hop),
        "n_bands": 64,
        "width_px": int(columns),
        "height_px": 64,
        "duration_ms": duration_ms,
        "covers_ms": covers_ms,
        "db_floor": -80,
        "colormap": "ga-ember",
    }
    spec_asset = asset_object(
        spec_png,
        kind="spectrogram_2d",
        derived_from=[wav_asset["uid"]],
        duration_s=duration_s,
        fields=spec_fields,
    )
    spec_doc = {
        **spec_asset,
        "duration_s": duration_s,
        "sample_rate": fitted_sr,
        "png": spec_png.name,
    }
    spec_json_path.write_text(json.dumps(spec_doc, indent=2) + "\n", encoding="utf-8")
    spec_json_asset = asset_object(
        spec_json_path,
        kind="spectrogram_json",
        derived_from=[wav_asset["uid"], spec_asset["uid"]],
        duration_s=duration_s,
    )

    cube_doc = cube_document(
        fitted,
        fitted_sr,
        duration_s=duration_s,
        engine=engine,
        derived_from=[wav_asset["uid"]],
        source_sha256=wav_asset["sha256"],
    )
    cube_json_path = out_dir / "cube.json"
    cube_png = out_dir / "cube.png"
    _write_cube_png(cube_png, cube_doc)
    cube_png_asset = asset_object(cube_png, kind="cube_png", derived_from=[wav_asset["uid"]], duration_s=duration_s)
    cube_doc["png"] = cube_png.name
    cube_doc["png_sha256"] = cube_png_asset["sha256"]
    cube_json_path.write_text(json.dumps(cube_doc, indent=2) + "\n", encoding="utf-8")
    cube_fields = {
        "source_sha256": wav_asset["sha256"],
        "sample_rate_hz": int(fitted_sr),
        "bin_frames": round_half_up(float(cube_doc["bin_seconds"]) * float(fitted_sr)),
        "time_bins": int(cube_doc["cube_shape_f_t"][1]),
        "freq_bins": int(cube_doc["cube_shape_f_t"][0]),
        "duration_ms": round_half_up(float(duration_s) * 1000.0),
        "covers_ms": round_half_up(float(cube_doc["cube_covers_s"]) * 1000.0),
        "inv_hdr_ppm": round_half_up(float(cube_doc["inv_hdr"]) * 1_000_000.0),
        "cube_revision": 2,
        "n_points": int(cube_doc["n_points"]),
        "n_fft": int(cube_doc["n_fft"]),
        "hop_frames": int(cube_doc["hop"]),
    }
    cube_asset = asset_object(
        cube_json_path,
        kind="cube",
        derived_from=[wav_asset["uid"]],
        duration_s=duration_s,
        fields=cube_fields,
    )

    try:
        compare_doc = compare_clip(
            fitted_path,
            engine=engine,
            reference_text=reference_text,
            duration_s=duration_s,
            derived_from=[wav_asset["uid"]],
        )
    except AsrError as exc:
        compare_doc = {
            "measuredHere": False,
            "unavailable": str(exc),
            "engine": engine,
            "duration_s": float(duration_s),
            "derived_from": [wav_asset["uid"]],
        }
    compare_path = out_dir / "compare.json"
    compare_path.write_text(json.dumps(compare_doc, indent=2) + "\n", encoding="utf-8")
    compare_asset = asset_object(
        compare_path,
        kind="compare",
        derived_from=[wav_asset["uid"]],
        duration_s=duration_s,
    )

    video_path = out_dir / "clip.mp4"
    video_error = ""
    video_asset = None
    try:
        render_clip_mp4(fitted_path, spec_png, cube_png, video_path)
        video_asset = asset_object(
            video_path,
            kind="video",
            derived_from=[wav_asset["uid"], spec_asset["uid"], cube_png_asset["uid"]],
            duration_s=duration_s,
        )
    except VideoError as exc:
        video_error = str(exc)

    if abs(float(spec_doc["duration_s"]) - duration_s) > 1e-9:
        raise RuntimeError("spectrogram duration does not match the source clip")
    if abs(float(cube_doc["duration_s"]) - duration_s) > 1e-9:
        raise RuntimeError("cube duration does not match the source clip")

    return {
        "ok": True,
        "synthesizedSpeech": True,
        "sha256": wav_asset["sha256"],
        "duration_s": duration_s,
        "speech_s": speech_s,
        "durationTarget_s": float(duration_target_s),
        "durationHonoured": (not padded) and abs(duration_s - float(duration_target_s)) < 0.05,
        "sample_rate": fitted_sr,
        "engine": engine,
        "assets": {
            "script": script_asset,
            "raw": raw_asset,
            "wav": wav_asset,
            "spectrogram": spec_asset,
            "spectrogramJson": spec_json_asset,
            "cube": cube_asset,
            "cubePng": cube_png_asset,
            "compare": compare_asset,
            "video": video_asset,
        },
        "compare": compare_doc,
        "videoError": video_error,
        "files": {
            "wav": fitted_path.name,
            "spectrogramPng": spec_png.name,
            "spectrogramJson": spec_json_path.name,
            "cubeJson": cube_json_path.name,
            "cubePng": cube_png.name,
            "compare": compare_path.name,
            "video": video_path.name if video_asset else "",
        },
    }


# Library cube revision 2 (scripts/cube_spectrogram_3d.py geometry).
# freq bins = (n_fft/2+1)//5, time bins = stft_frames//33,
# bin_seconds = 33*hop/sample_rate, covers = time_bins * bin_seconds.
# inv_hdr is rms/peak of these samples via cube_revision.measure.
CUBE_N_FFT = 1024
CUBE_HOP = 256
CUBE_DOWNSAMPLE = (5, 33)


def cube_geometry(n_samples: int, sample_rate: int) -> dict:
    """Revision-2 binning. ``cube_covers_s`` can be shorter than ``duration_s``."""
    hop = CUBE_HOP
    sf, st = CUBE_DOWNSAMPLE
    stft_frames = 1 + int(n_samples) // hop
    freq_bins = (CUBE_N_FFT // 2 + 1) // sf
    time_bins = stft_frames // st
    bin_seconds = (st * hop) / float(sample_rate)
    return {
        "n_fft": CUBE_N_FFT,
        "hop": hop,
        "downsample_sf_st": [sf, st],
        "stft_frames": stft_frames,
        "freq_bins": freq_bins,
        "time_bins": time_bins,
        "bin_seconds": bin_seconds,
        "cube_covers_s": time_bins * bin_seconds,
        "cube_shape_f_t": [freq_bins, time_bins],
    }


def cube_document(
    audio: np.ndarray,
    sample_rate: int,
    *,
    duration_s: float,
    engine: str,
    derived_from: list[str],
    source_sha256: str = "",
) -> dict:
    """Inverse-HDR bitdot cube, revision 2, on the samples that were passed in."""
    values = np.asarray(audio, dtype=np.float64).reshape(-1)
    if values.size < CUBE_N_FFT:
        raise ValueError("audio is too short for a cube")
    metrics = measure(values, sample_rate)
    geo = cube_geometry(int(values.size), int(sample_rate))
    if geo["time_bins"] < 1:
        raise ValueError("audio is too short for a revision-2 cube")
    magnitude = _stft_magnitude(values, CUBE_N_FFT, CUBE_HOP)
    sf, st = CUBE_DOWNSAMPLE
    freq_bins, time_bins = geo["freq_bins"], geo["time_bins"]
    cropped = magnitude[: freq_bins * sf, : time_bins * st]
    grid = cropped.reshape(freq_bins, sf, time_bins, st).mean(axis=(1, 3))
    signal_layer = _unit(grid)
    flatness = _frame_flatness(grid)
    tonality = (1.0 - flatness) * (signal_layer > 1e-4)
    floor = float(np.percentile(grid, 70))
    confidence = np.clip(grid / (grid + floor + 1e-12), 0.0, 1.0)
    quality = 0.5 * signal_layer + 0.5 * np.clip(tonality, 0.0, 1.0)
    matrices = {
        "signal": signal_layer,
        "tonality": np.clip(tonality, 0.0, 1.0),
        "confidence": confidence,
        "quality": np.clip(quality, 0.0, 1.0),
    }
    layers = {name: _layer_stats(matrices[name]) for name in LAYERS}
    z_of = {"signal": 0.0, "tonality": 1.0 / 3.0, "confidence": 2.0 / 3.0, "quality": 1.0}
    points: list[dict] = []
    for name in LAYERS:
        points.extend(_preview_points(matrices[name], name, z_of[name], limit=900))
    source_cells = int(np.count_nonzero(signal_layer > 1e-6))
    return {
        "engine": engine,
        "duration_s": float(duration_s),
        "sample_rate": int(sample_rate),
        "inv_hdr": metrics.inv_hdr,
        "bw95_hz": metrics.bw95_hz,
        "n_fft": CUBE_N_FFT,
        "hop": CUBE_HOP,
        "downsample_sf_st": [sf, st],
        "stft_frames": geo["stft_frames"],
        "bin_seconds": geo["bin_seconds"],
        "cube_shape_f_t": [freq_bins, time_bins],
        "cube_covers_s": geo["cube_covers_s"],
        "cube_revision": 2,
        "n_points": len(points),
        "n_points_source": source_cells * len(LAYERS),
        "source_sha256": source_sha256,
        "axes": {"x": "time bin", "y": "freq bin", "z": "layer (+value)"},
        "legend": {"signal": "blue", "tonality": "green", "confidence": "orange", "quality": "pink"},
        "layers": layers,
        "points_preview": points,
        "derived_from": list(derived_from),
    }


def _stft_magnitude(samples: np.ndarray, n_fft: int, hop: int) -> np.ndarray:
    """Centered Hann STFT. Frame count is ``1 + n_samples // hop``."""
    frames = 1 + int(samples.size) // hop
    padded = np.pad(samples, (n_fft // 2, n_fft // 2 + hop))
    window = np.hanning(n_fft)
    shape = (frames, n_fft)
    strides = (padded.strides[0] * hop, padded.strides[0])
    view = np.lib.stride_tricks.as_strided(padded, shape=shape, strides=strides)
    spectrum = np.fft.rfft(view * window, axis=1)
    return np.abs(spectrum).T


def compare_clip(
    wav_path: Path,
    *,
    engine: str,
    reference_text: str,
    duration_s: float,
    derived_from: list[str],
) -> dict:
    """WER from local ASR plus bandwidth. Raises ``AsrError`` instead of inventing a score."""
    audio, sample_rate = read_wav(wav_path)
    try:
        hypothesis = transcribe(wav_path)
    except AsrError:
        raise
    wer = word_error_rate(reference_text, hypothesis)
    bw95 = bandwidth_95(np.asarray(audio, dtype=np.float64), sample_rate)
    return {
        "measuredHere": True,
        "measuredAt": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "engine": engine,
        "metric": "wer",
        "wer": wer,
        "bw95_hz": bw95,
        "hypothesis": hypothesis,
        "reference": reference_text,
        "asr": "faster-whisper tiny.en",
        "duration_s": float(duration_s),
        "derived_from": list(derived_from),
    }


def _frame_flatness(grid: np.ndarray) -> np.ndarray:
    power = np.maximum(grid, 1e-12)
    geometric = np.exp(np.mean(np.log(power), axis=0))
    arithmetic = np.mean(power, axis=0)
    return np.clip(geometric / np.maximum(arithmetic, 1e-12), 0.0, 1.0)


def _unit(grid: np.ndarray) -> np.ndarray:
    peak = float(np.max(grid)) if grid.size else 0.0
    if peak <= 1e-12:
        return np.zeros_like(grid)
    return grid / peak


def _layer_stats(grid: np.ndarray) -> dict:
    flat = grid.reshape(-1)
    return {
        "mean": float(np.mean(flat)),
        "std": float(np.std(flat)),
        "p50": float(np.percentile(flat, 50)),
        "p90": float(np.percentile(flat, 90)),
        "active_frac": float(np.mean(flat > 0.2)),
        "shape": [int(grid.shape[0]), int(grid.shape[1])],
    }


def _preview_points(grid: np.ndarray, layer: str, z: float, limit: int = 80) -> list[dict]:
    freq_bins, time_bins = grid.shape
    coords = [(i, j, float(grid[i, j])) for i in range(freq_bins) for j in range(time_bins)]
    coords.sort(key=lambda item: item[2], reverse=True)
    points = []
    for freq_index, time_index, value in coords[:limit]:
        points.append(
            {
                "t": 0.0 if time_bins <= 1 else time_index / float(time_bins - 1),
                "f": 0.0 if freq_bins <= 1 else freq_index / float(freq_bins - 1),
                "z": z,
                "v": value,
                "layer": layer,
            }
        )
    return points


def _write_cube_png(path: Path, document: dict) -> None:
    from matplotlib.backends.backend_agg import FigureCanvasAgg
    from matplotlib.figure import Figure

    figure = Figure(figsize=(4.2, 3.2))
    FigureCanvasAgg(figure)
    axes = figure.add_subplot(1, 1, 1, projection="3d")
    colors = {"signal": "#7eb6ff", "tonality": "#f0c674", "confidence": "#b5e48c", "quality": "#e5989b"}
    for point in document["points_preview"]:
        axes.scatter(point["t"], point["f"], point["z"], s=8, c=colors.get(point["layer"], "#cccccc"))
    axes.set_xlim(0, 1)
    axes.set_ylim(0, 1)
    axes.set_zlim(0, 1)
    axes.set_title(f"inverse-HDR cube {document['duration_s']:.2f}s")
    figure.tight_layout()
    path.parent.mkdir(parents=True, exist_ok=True)
    figure.savefig(path, dpi=110)
