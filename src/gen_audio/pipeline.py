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
from gen_audio.cube_layers import (
    LAYER_NAMES,
    CubeParams,
    compute_layers,
    downsample_cube,
    layer_score,
    layers_to_points,
    stft_mag,
)
from gen_audio.cube_revision import bandwidth_95, measure
from gen_audio.identity import ms_from_frames, round_half_up
from gen_audio.improve import improve
from gen_audio.speakers import (
    UNRESOLVED,
    SpeechTimeline,
    published_segments,
    remap_through_improve,
    scale_segments,
    speaker_idx_bins,
)
from gen_audio.spectrogram_strip import colormap, strip, write_png
from gen_audio.video_render import VideoError, render_clip_mp4


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
    speech: SpeechTimeline | None = None,
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
    speech_timeline = _timeline_after_improve(speech, raw_sr, improved)
    speech_s = duration_seconds(improved.audio, improved.sample_rate)
    fitted, padded = fit_duration(improved.audio, improved.sample_rate, duration_target_s)
    duration_s = duration_seconds(fitted, improved.sample_rate)
    fitted_path = out_dir / "fitted.wav"
    write_wav(fitted_path, fitted, improved.sample_rate, subtype="PCM_16")
    # Re-read so the stored duration matches the file on disk, including PCM rounding.
    written_len = int(fitted.size)
    fitted, fitted_sr = read_wav(fitted_path)
    duration_s = duration_seconds(fitted, fitted_sr)
    if speech_timeline is not None:
        segments = speech_timeline.segments
        if int(fitted.size) != written_len:
            segments = scale_segments(segments, written_len, int(fitted.size))
        speech_timeline = SpeechTimeline(
            speakers=speech_timeline.speakers,
            segments=segments,
            sample_rate=int(fitted_sr),
        )
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
        speech=speech_timeline,
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
        "covers_ms": min(
            round_half_up(float(cube_doc["cube_covers_s"]) * 1000.0),
            round_half_up(float(duration_s) * 1000.0),
        ),
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


# Closed form of the long-clip downsample (max 128 x 400 at n_fft 1024 / hop 256).
# A 3_345_000-sample 24 kHz clip lands on sf=5, st=33. Short clips use a smaller
# time stride; cube_document asks cube_layers.downsample_cube for the real pair.
CUBE_N_FFT = 1024
CUBE_HOP = 256
CUBE_DOWNSAMPLE = (5, 33)


def cube_geometry(n_samples: int, sample_rate: int) -> dict:
    """Fixed (5, 33) binning used by the shipped long Library clips."""
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
    speech: SpeechTimeline | None = None,
) -> dict:
    """Inverse-HDR bitdot cube on the samples that were passed in.

    Signal, tonality, confidence, quality, the downsample, and ``layer_score``
    come from :mod:`gen_audio.cube_layers`. ``inv_hdr`` stays
    :func:`gen_audio.cube_revision.measure` (rms/peak).
    """
    values = np.asarray(audio, dtype=np.float64).reshape(-1)
    rate = int(sample_rate)
    if values.size == 0 or rate <= 0:
        raise ValueError("audio is too short for a cube")
    params = CubeParams()
    metrics = measure(values, rate)
    magnitude = stft_mag(values, n_fft=params.n_fft, hop=params.hop)
    full = compute_layers(magnitude, rate, params.n_fft)
    down, sf, st = downsample_cube(full, max_f=params.max_f, max_t=params.max_t)
    cloud = layers_to_points(down, thresh=params.thresh)
    x, yy, _z, _rgba, vals, lids = cloud
    nf, nt = next(iter(down.values())).shape
    if nf < 1 or nt < 1:
        raise ValueError("audio is too short for a cube")
    bin_s = st * params.hop / rate
    points, stats, counts = [], {}, {}
    for li, name in enumerate(LAYER_NAMES):
        mask = lids == li
        tx, fy, vv = x[mask], yy[mask], vals[mask]
        counts[name] = int(mask.sum())
        order = np.argsort(-vv, kind="stable")[: params.per_layer]
        if len(order):
            sel = vv[order]
            lo, hi = float(sel.min()), float(sel.max())
        for i in order:
            v = 1.0 if hi <= lo else (float(vv[i]) - lo) / (hi - lo)
            points.append(
                {
                    "t": round(float(tx[i]) / max(nt - 1, 1), 5),
                    "f": round(float(fy[i]) / max(nf - 1, 1), 5),
                    "z": li / 3.0,
                    "v": round(v, 5),
                    "layer": name,
                }
            )
        mat = full[name]
        stats[name] = {
            "mean": float(mat.mean()),
            "std": float(mat.std()),
            "p50": float(np.percentile(mat, 50)),
            "p90": float(np.percentile(mat, 90)),
            "active_frac": float((mat > 0.2).mean()),
            "shape": [int(nf), int(nt)],
        }
    document = {
        "engine": engine,
        "duration_s": float(duration_s),
        "sample_rate": rate,
        "inv_hdr": metrics.inv_hdr,
        "layer_score": layer_score(full),
        "bw95_hz": metrics.bw95_hz,
        "n_fft": params.n_fft,
        "hop": params.hop,
        "downsample_sf_st": [int(sf), int(st)],
        "stft_frames": int(magnitude.shape[1]),
        "bin_seconds": bin_s,
        "cube_shape_f_t": [int(nf), int(nt)],
        "cube_covers_s": nt * bin_s,
        "cube_revision": 2,
        "n_points": len(points),
        "n_points_source": int(len(x)),
        "n_points_source_per_layer": counts,
        "source_sha256": source_sha256,
        "axes": {"x": "time bin", "y": "freq bin", "z": "layer (+value)"},
        "legend": {"signal": "blue", "tonality": "green", "confidence": "orange", "quality": "pink"},
        "layers": stats,
        "points_preview": points,
        "derived_from": list(derived_from),
    }
    document.update(_speech_facts(speech, rate, n_bins=int(nt), bin_seconds=bin_s, n_samples=int(values.size)))
    return document


def _timeline_after_improve(speech: SpeechTimeline | None, raw_sr: int, improved) -> SpeechTimeline | None:
    """Move concatenation cursors through the publish chain. Pad-at-end comes later."""
    if speech is None:
        return None
    if int(speech.sample_rate) != int(raw_sr):
        raise ValueError("speech timeline sample rate does not match the raw wav")
    remapped = remap_through_improve(
        speech.segments,
        trim_start=int(improved.trim_start),
        trimmed_len=int(improved.trimmed_samples),
        published_len=int(improved.audio.size),
    )
    return SpeechTimeline(speakers=list(speech.speakers), segments=remapped, sample_rate=int(improved.sample_rate))


def _speech_facts(
    speech: SpeechTimeline | None,
    sample_rate: int,
    *,
    n_bins: int,
    bin_seconds: float,
    n_samples: int,
) -> dict:
    """Speakers are unresolved unless a per-line timeline was recorded."""
    if speech is None:
        return {"speakers": UNRESOLVED}
    if int(speech.sample_rate) != int(sample_rate):
        raise ValueError("speech timeline sample rate does not match the cube wav")
    return {
        "speakers": list(speech.speakers),
        "segments": published_segments(speech.segments, sample_rate),
        "speaker_idx": speaker_idx_bins(
            speech.segments,
            n_bins=n_bins,
            bin_seconds=bin_seconds,
            sample_rate=sample_rate,
            n_samples=n_samples,
        ),
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
