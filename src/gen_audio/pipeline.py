"""Analysis pipeline: improve, 2D spectrogram, inverse-HDR cube, compare.

Every step writes an asset object. Spectrogram and cube ``duration_s`` are the
fitted WAV's sample count divided by its sample rate.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
from scipy import signal

from gen_audio.asr_wer import AsrError, transcribe, word_error_rate
from gen_audio.assets import asset_object
from gen_audio.audio_io import read_wav, write_wav
from gen_audio.cube_revision import bandwidth_95, measure
from gen_audio.improve import improve
from gen_audio.spectrogram import write_spectrogram
from gen_audio.video_render import VideoError, render_clip_mp4

LAYERS = ("signal", "tonality", "confidence", "quality")


def duration_seconds(audio: np.ndarray, sample_rate: int) -> float:
    """Exact duration of a buffer. Callers store this value unchanged."""
    if int(sample_rate) <= 0:
        raise ValueError("sample rate must be positive")
    return float(audio.size) / float(sample_rate)


def fit_duration(audio: np.ndarray, sample_rate: int, target_s: float) -> tuple[np.ndarray, bool]:
    """Pad with silence up to ``target_s``. Longer speech is kept, not trimmed."""
    target = int(round(float(target_s) * int(sample_rate)))
    if audio.size < target:
        pad = np.zeros(target - audio.size, dtype=np.float32)
        return np.concatenate([audio.astype(np.float32), pad]), True
    return audio.astype(np.float32), audio.size <= target + int(sample_rate * 0.05)


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
    raw_asset = asset_object(raw_wav, kind="wav", derived_from=[script_asset["uid"]], duration_s=duration_seconds(raw_audio, raw_sr))

    improved = improve(raw_audio, raw_sr)
    fitted, honoured = fit_duration(improved.audio, improved.sample_rate, duration_target_s)
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
    )

    spec_png = out_dir / "spectrogram.png"
    write_spectrogram(spec_png, fitted, fitted_sr, title=f"{engine} 2D spectrogram")
    spec_json_path = out_dir / "spectrogram.json"
    spec_asset = asset_object(spec_png, kind="spectrogram_2d", derived_from=[wav_asset["uid"]], duration_s=duration_s)
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

    cube_doc = cube_document(fitted, fitted_sr, duration_s=duration_s, engine=engine, derived_from=[wav_asset["uid"]])
    cube_json_path = out_dir / "cube.json"
    cube_png = out_dir / "cube.png"
    _write_cube_png(cube_png, cube_doc)
    cube_png_asset = asset_object(cube_png, kind="cube_png", derived_from=[wav_asset["uid"]], duration_s=duration_s)
    cube_doc["png"] = cube_png.name
    cube_doc["png_sha256"] = cube_png_asset["sha256"]
    cube_json_path.write_text(json.dumps(cube_doc, indent=2) + "\n", encoding="utf-8")
    cube_asset = asset_object(
        cube_json_path,
        kind="cube",
        derived_from=[wav_asset["uid"], cube_png_asset["uid"]],
        duration_s=duration_s,
    )

    compare_doc = compare_clip(
        fitted_path,
        engine=engine,
        reference_text=reference_text,
        duration_s=duration_s,
        derived_from=[wav_asset["uid"]],
    )
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
        "ok": video_asset is not None and "wer" in compare_doc,
        "synthesizedSpeech": True,
        "sha256": wav_asset["sha256"],
        "duration_s": duration_s,
        "durationTarget_s": float(duration_target_s),
        "durationHonoured": bool(honoured) and abs(duration_s - float(duration_target_s)) < 0.05,
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


def cube_document(
    audio: np.ndarray,
    sample_rate: int,
    *,
    duration_s: float,
    engine: str,
    derived_from: list[str],
) -> dict:
    """Inverse-HDR cube with signal, tonality, confidence, and quality layers."""
    values = np.asarray(audio, dtype=np.float64).reshape(-1)
    metrics = measure(values, sample_rate)
    n_fft = 1024 if values.size >= 1024 else 256
    hop = n_fft // 2
    if values.size < n_fft:
        raise ValueError("audio is too short for a cube")
    _freq, _time, spectrum = signal.stft(
        values,
        fs=int(sample_rate),
        window="hann",
        nperseg=n_fft,
        noverlap=n_fft - hop,
        boundary=None,
    )
    magnitude = np.abs(spectrum)
    freq_bins, time_bins = 32, min(48, max(8, magnitude.shape[1]))
    grid = _block_mean(magnitude, freq_bins, time_bins)
    signal_layer = _unit(grid)
    flatness = _frame_flatness(grid)
    tonality = np.broadcast_to(1.0 - flatness, grid.shape).copy()
    confidence = grid / (grid + np.percentile(grid, 70) + 1e-12)
    quality = signal_layer * (0.5 + 0.5 * tonality)
    layers = {
        "signal": _layer_stats(signal_layer),
        "tonality": _layer_stats(tonality),
        "confidence": _layer_stats(confidence),
        "quality": _layer_stats(quality),
    }
    matrices = {
        "signal": signal_layer,
        "tonality": tonality,
        "confidence": confidence,
        "quality": quality,
    }
    z_of = {"signal": 0.0, "tonality": 1.0 / 3.0, "confidence": 2.0 / 3.0, "quality": 1.0}
    points: list[dict] = []
    for name in LAYERS:
        points.extend(_preview_points(matrices[name], name, z_of[name]))
    return {
        "engine": engine,
        "duration_s": float(duration_s),
        "sample_rate": int(sample_rate),
        "inv_hdr": metrics.inv_hdr,
        "bw95_hz": metrics.bw95_hz,
        "n_fft": n_fft,
        "hop": hop,
        "bin_seconds": float(duration_s) / float(time_bins),
        "cube_shape_f_t": [freq_bins, time_bins],
        "cube_covers_s": float(duration_s),
        "cube_revision": 3,
        "layers": layers,
        "points_preview": points,
        "derived_from": list(derived_from),
    }


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


def _block_mean(matrix: np.ndarray, freq_bins: int, time_bins: int) -> np.ndarray:
    out = np.zeros((freq_bins, time_bins), dtype=np.float64)
    freq_edges = np.linspace(0, matrix.shape[0], freq_bins + 1).astype(int)
    time_edges = np.linspace(0, matrix.shape[1], time_bins + 1).astype(int)
    for i in range(freq_bins):
        for j in range(time_bins):
            block = matrix[freq_edges[i] : max(freq_edges[i + 1], freq_edges[i] + 1), time_edges[j] : max(time_edges[j + 1], time_edges[j] + 1)]
            out[i, j] = float(np.mean(block)) if block.size else 0.0
    return out


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
