"""Four-layer inverse-HDR "bitdot" cube for Library clips.

This is the generator behind every shipped Library cube JSON. It replaces
the one-off ``make_library_cube.py`` and ``make_optimus_*`` generators, which
each kept their own WAV reader, STFT and layer formulas.

Revision 3 (``LIBRARY_REVISION``, ``LAYER_METHOD`` ``library_r3``) is the one
method for all Library clips: the same ``LIBRARY_PARAMS`` for every WAV, no
per-clip overrides. Each cube JSON records ``source_sha256`` (sha256 of the
WAV bytes) and ``provenance``: ``generator`` (this file's repo path),
``generator_sha256`` (sha256 of this file's bytes with CRLF normalized to LF,
:func:`generator_sha256`), ``layer_method`` and ``params``. Those bytes are
the cube asset's identity (its uid hashes the cube JSON), so identity comes
from what the generator says, not from git history: a commit that changes
this file needs a regen, and a rebase, squash or CRLF checkout does not
(tests/test_cube_layers.py fails with "regenerate cubes" on a mismatch, and
build_assets checks it too). No commit SHA is written here. The commit that
holds these bytes is information only; ``cube_revision.py manifest
--record-generator-commit`` writes it to the manifest cube block after the
regen is committed, and build_assets copies it into the cube envelope's
unhashed provenance (docs/ASSET_OBJECT_MODEL.md, "Cube identity").

Method:

- STFT: Hann window, ``n_fft`` 1024, ``hop`` 256, reflect padding.
- Layers, each in [0, 1] and the shape of the magnitude spectrogram:
  ``signal`` (percentile-stretched log magnitude, power 0.65),
  ``tonality`` (1 - spectral flatness blended with local peakiness),
  ``confidence`` (log SNR over a 20th-percentile noise floor) and
  ``quality`` (formant/clarity/mud balance weighted by speech band).
- Block-average downsample to at most ``max_f`` x ``max_t`` bins.
- Library preview: the ``per_layer`` strongest points per layer above
  ``thresh``, t and f normalized by bin / (n - 1), z = layer index / 3,
  v min-max scaled within the selected points.
- ``inv_hdr`` is the repo's one definition, ``rms / peak`` of the WAV
  (``gen_audio.cube_revision.measure``; ARCHITECTURE.md, cube_revision.py).
  The cube JSON's ``inv_hdr_ppm`` identity field derives from it.
- ``layer_score`` = 0.35 signal + 0.25 tonality + 0.20 confidence + 0.20
  quality (layer means over the full-resolution layers). Revisions 1 and 2
  stored this composite under ``inv_hdr``; it is not rms/peak, so it has its
  own key.

Scrub mapping: ``bin_seconds`` = ``downsample_sf_st[1] * hop / sample_rate``;
the cube covers ``cube_shape_f_t[1] * bin_seconds`` seconds of the WAV.

``pipeline_r2`` is an explicit comparison method, not the current Generate
pipeline default. It ports PR #4's zero-padded STFT, fixed 5 x 33 block
averaging before the layer formulas, grid/max signal, spectral-flatness
tonality, global p70 confidence floor, and 0.5 signal + 0.5 tonality quality.
Both methods share preview selection and the rms/peak inverse-HDR metric.
Comparison documents record the original formula source and the same WAV's
sha256; changing this generator requires regenerating both methods.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass
from pathlib import Path

import numpy as np

from gen_audio.audio_io import as_mono, validate_frame_count, validate_sample_rate
from gen_audio.cube_revision import measure
from gen_audio.library_manifest import normalized_sha256

LAYER_NAMES = ("signal", "tonality", "confidence", "quality")
LAYER_COLORS = (
    (0.15, 0.55, 1.0),
    (0.2, 0.9, 0.35),
    (1.0, 0.75, 0.1),
    (0.95, 0.25, 0.55),
)
LEGEND = {"signal": "blue", "tonality": "green", "confidence": "orange", "quality": "pink"}

LIBRARY_R3 = "library_r3"
PIPELINE_R2 = "pipeline_r2"
LAYER_METHODS = (LIBRARY_R3, PIPELINE_R2)
DEFAULT_LAYER_METHOD = LIBRARY_R3
LAYER_METHOD_LABELS = {
    LIBRARY_R3: "Library formulas, rev 3",
    PIPELINE_R2: "Pipeline formulas, rev 2 (PR #4)",
}
PIPELINE_R2_SOURCE = (
    "darbotlabs/gen-audio@204d0de45766ad31af89d8f5aee6c1a014010834 src/gen_audio/pipeline.py "
    "cube_document L275-L337, _stft_magnitude L340-L349, _frame_flatness L383-L387, _unit L390-L394"
)
PIPELINE_R2_DOWNSAMPLE = (5, 33)


def _check_method(method: str) -> str:
    if method not in LAYER_METHODS:
        raise ValueError(f"unknown layer_method {method!r}; expected one of {', '.join(LAYER_METHODS)}")
    return method


@dataclass(frozen=True)
class CubeParams:
    n_fft: int = 1024
    hop: int = 256
    max_f: int = 128
    max_t: int = 400
    thresh: float = 0.12
    per_layer: int = 900


# The one Library cube method. Every shipped cube uses exactly these.
LIBRARY_REVISION = 3
LAYER_METHOD = LIBRARY_R3
LIBRARY_PARAMS = CubeParams()
GENERATOR_PATH = "src/gen_audio/cube_layers.py"
REPO_ROOT = Path(__file__).resolve().parents[2]


def generator_sha256(repo: Path | str = REPO_ROOT) -> str:
    """sha256 of this generator's bytes with CRLF normalized to LF.

    A Windows checkout with core.autocrlf=true hashes the same as a Linux one.
    """
    return normalized_sha256((Path(repo) / GENERATOR_PATH).read_bytes())


def check_generator(doc: dict, repo: Path | str = REPO_ROOT) -> None:
    """Raise ValueError unless a cube JSON was made by this generator's bytes."""
    recorded = (doc.get("provenance") or {}).get("generator_sha256")
    current = generator_sha256(repo)
    if recorded != current:
        raise ValueError(
            f"cube {doc.get('wavUrl')} was made by {GENERATOR_PATH} with sha256 {recorded}, "
            f"but the file now hashes to {current} (CRLF->LF); regenerate cubes: "
            "python scripts/cube_revision.py layers ... for each Library WAV"
        )


def stft_mag(audio: np.ndarray, n_fft: int = 1024, hop: int = 256) -> np.ndarray:
    """Magnitude STFT, shape (freq, time), Hann window, reflect padding."""
    y = np.asarray(audio, dtype=np.float64)
    win = np.hanning(n_fft)
    pad = n_fft // 2
    yp = np.pad(y, (pad, pad), mode="reflect")
    n_frames = 1 + (len(yp) - n_fft) // hop
    if n_frames < 1:
        n_frames = 1
        yp = np.pad(yp, (0, max(0, n_fft - len(yp))))
    frames = np.lib.stride_tricks.as_strided(
        yp,
        shape=(n_frames, n_fft),
        strides=(yp.strides[0] * hop, yp.strides[0]),
        writeable=False,
    ).copy()
    frames *= win
    return np.abs(np.fft.rfft(frames, axis=1)).T


def pipeline_r2_stft(samples: np.ndarray, n_fft: int = 1024, hop: int = 256) -> np.ndarray:
    """PR #4 STFT: zero pad, Hann window and ``1 + n // hop`` frames."""
    values = np.asarray(samples, dtype=np.float64).reshape(-1)
    if type(n_fft) is not int or not 16 <= n_fft <= 16384 or type(hop) is not int or hop < 1:
        raise ValueError("invalid pipeline_r2 STFT dimensions")
    frames = 1 + int(values.size) // hop
    if frames * n_fft > 64_000_000:
        raise ValueError("pipeline_r2 STFT exceeds the work limit")
    padded = np.pad(values, (n_fft // 2, n_fft // 2 + hop))
    window = np.hanning(n_fft)
    strides = (padded.strides[0] * hop, padded.strides[0])
    view = np.lib.stride_tricks.as_strided(padded, shape=(frames, n_fft), strides=strides, writeable=False)
    return np.abs(np.fft.rfft(view * window, axis=1)).T


def pipeline_r2_grid(mag: np.ndarray) -> np.ndarray:
    """Crop and block-average the PR #4 magnitude into complete 5 x 33 blocks."""
    sf, st = PIPELINE_R2_DOWNSAMPLE
    freq_bins, time_bins = mag.shape[0] // sf, mag.shape[1] // st
    if freq_bins < 1 or time_bins < 1:
        raise ValueError("audio is too short for a pipeline_r2 cube")
    cropped = mag[:freq_bins * sf, :time_bins * st]
    return cropped.reshape(freq_bins, sf, time_bins, st).mean(axis=(1, 3))


def _pipeline_r2_layers(grid: np.ndarray) -> dict[str, np.ndarray]:
    peak = float(np.max(grid)) if grid.size else 0.0
    signal = np.zeros_like(grid) if peak <= 1e-12 else grid / peak
    power = np.maximum(grid, 1e-12)
    flatness = np.clip(np.exp(np.mean(np.log(power), axis=0)) / np.maximum(np.mean(power, axis=0), 1e-12), 0.0, 1.0)
    tonality = (1.0 - flatness) * (signal > 1e-4)
    floor = float(np.percentile(grid, 70))
    confidence = np.clip(grid / (grid + floor + 1e-12), 0.0, 1.0)
    quality = 0.5 * signal + 0.5 * np.clip(tonality, 0.0, 1.0)
    return {
        "signal": signal,
        "tonality": np.clip(tonality, 0.0, 1.0),
        "confidence": confidence,
        "quality": np.clip(quality, 0.0, 1.0),
    }


def compute_layers(
    mag: np.ndarray, sample_rate: int, n_fft: int, method: str = DEFAULT_LAYER_METHOD,
) -> dict[str, np.ndarray]:
    """Library layers at full resolution, or PR #4 float64 layers on its grid."""
    if _check_method(method) == PIPELINE_R2:
        return _pipeline_r2_layers(pipeline_r2_grid(mag))
    eps = 1e-12
    logm = np.log1p(mag)
    lo, hi = np.percentile(logm, [5, 99.5])
    signal = np.clip((logm - lo) / max(hi - lo, eps), 0, 1)
    signal = np.power(signal, 0.65)

    power = mag**2 + eps
    geo = np.exp(np.mean(np.log(power), axis=0))
    arith = np.mean(power, axis=0)
    tonality_t = 1.0 - np.clip(geo / (arith + eps), 0, 1)
    kernel = 5
    half = kernel // 2
    padded = np.pad(mag, ((half, half), (0, 0)), mode="edge")
    local = np.stack([padded[i : i + mag.shape[0]] for i in range(kernel)], axis=0).mean(axis=0)
    peakiness = np.clip((mag / (local + eps) - 1.0) / 3.0, 0, 1)
    tonality = np.clip(0.55 * tonality_t[None, :] + 0.45 * peakiness, 0, 1)
    tonality *= (signal > 0.08).astype(np.float64)

    noise_floor = np.percentile(mag, 20, axis=1, keepdims=True) + eps
    confidence = np.clip(np.log1p(mag / noise_floor) / np.log1p(40.0), 0, 1)
    confidence *= (signal > 0.05).astype(np.float64)

    freqs = np.fft.rfftfreq(n_fft, d=1.0 / sample_rate)
    mid = ((freqs >= 300) & (freqs <= 3400)).astype(np.float64)[:, None]
    hi_band = ((freqs >= 4000) & (freqs <= 8000)).astype(np.float64)[:, None]
    mud = ((freqs > 0) & (freqs <= 200)).astype(np.float64)[:, None]
    mid_e = (mag * mid).sum(axis=0) + eps
    hi_e = (mag * hi_band).sum(axis=0) + eps
    mud_e = (mag * mud).sum(axis=0) + eps
    clarity = hi_e / (mid_e + hi_e)
    muddiness = mud_e / (mid_e + mud_e)
    q_t = np.clip(0.6 * (1.0 - muddiness) + 0.4 * clarity, 0, 1)
    speech_band = ((freqs >= 200) & (freqs <= 5000)).astype(np.float64)[:, None]
    band_w = (mag * speech_band) / ((mag * speech_band).max(axis=0, keepdims=True) + eps)
    quality = np.clip(0.5 * q_t[None, :] + 0.5 * band_w * signal, 0, 1)

    return {
        "signal": signal.astype(np.float32),
        "tonality": tonality.astype(np.float32),
        "confidence": confidence.astype(np.float32),
        "quality": quality.astype(np.float32),
    }


def downsample_cube(layers: dict[str, np.ndarray], max_f: int, max_t: int) -> tuple[dict[str, np.ndarray], int, int]:
    """Block-average every layer to at most max_f x max_t; returns (layers, sf, st)."""
    f, t = next(iter(layers.values())).shape
    sf = max(1, int(np.ceil(f / max_f)))
    st = max(1, int(np.ceil(t / max_t)))
    nf, nt = f // sf, t // st
    out = {k: v[: nf * sf, : nt * st].reshape(nf, sf, nt, st).mean(axis=(1, 3)) for k, v in layers.items()}
    return out, sf, st


def layers_to_points(layers: dict[str, np.ndarray], thresh: float = 0.12):
    """Point cloud over the downsampled layers: (t, f, z, rgba, value, layer_id) arrays."""
    xs, ys, zs, cs, vs, ls = [], [], [], [], [], []
    for li, name in enumerate(LAYER_NAMES):
        mat = layers[name]
        f_idx, t_idx = np.where(mat >= thresh)
        if len(f_idx) == 0:
            continue
        vals = mat[f_idx, t_idx]
        xs.append(t_idx.astype(np.float32))
        ys.append(f_idx.astype(np.float32))
        zs.append(li + vals * 0.85)
        rgba = np.zeros((len(vals), 4), dtype=np.float32)
        rgba[:, :3] = np.array(LAYER_COLORS[li], dtype=np.float32)
        rgba[:, 3] = 0.25 + 0.75 * vals
        cs.append(rgba)
        vs.append(vals.astype(np.float32))
        ls.append(np.full(len(vals), li, dtype=np.int32))
    if not xs:
        empty = np.zeros(0, np.float32)
        return empty, empty, empty, np.zeros((0, 4), np.float32), empty, np.zeros(0, np.int32)
    return (
        np.concatenate(xs),
        np.concatenate(ys),
        np.concatenate(zs),
        np.concatenate(cs),
        np.concatenate(vs),
        np.concatenate(ls),
    )


def preview_points(cloud: tuple, shape_f_t: tuple[int, int], per_layer: int) -> tuple[list[dict], dict[str, int]]:
    """Library preview selection over a :func:`layers_to_points` cloud.

    Per layer: the ``per_layer`` strongest points (stable order), t and f
    normalized by bin / (n - 1) of ``shape_f_t``, z = layer index / 3, v
    min-max scaled within the selected points. Returns (points, counts) where
    counts is the source point count per layer. Public so every layer method
    shares one selection (import it; a copy would fork the preview silently).
    """
    x, yy, _z, _rgba, vals, lids = cloud
    nf, nt = shape_f_t
    points, counts = [], {}
    for li, name in enumerate(LAYER_NAMES):
        mask = lids == li
        tx, fy, vv = x[mask], yy[mask], vals[mask]
        counts[name] = int(mask.sum())
        order = np.argsort(-vv, kind="stable")[:per_layer]
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
    return points, counts


def layer_score(layers: dict[str, np.ndarray]) -> float:
    """Weighted mean of the four layers (informational; not inv_hdr)."""
    return float(
        0.35 * layers["signal"].mean()
        + 0.25 * layers["tonality"].mean()
        + 0.20 * layers["confidence"].mean()
        + 0.20 * layers["quality"].mean()
    )


def cube_json_name(stem: str, method: str = DEFAULT_LAYER_METHOD) -> str:
    base = f"library_{stem.removeprefix('library_')}"
    return f"{base}_cube3d.json" if _check_method(method) == LIBRARY_R3 else f"{base}_{method}_cube3d.json"


def cube_png_name(stem: str, method: str = DEFAULT_LAYER_METHOD) -> str:
    return f"{stem}_cube3d.png" if _check_method(method) == LIBRARY_R3 else f"{stem}_{method}_cube3d.png"


def library_cube(
    audio: np.ndarray,
    sample_rate: int,
    *,
    stem: str,
    engine: str,
    source_sha256: str,
    label: str | None = None,
    method: str = DEFAULT_LAYER_METHOD,
) -> tuple[dict, tuple]:
    """Build the rev 3 Library cube document. Returns (doc, point_cloud) where
    point_cloud feeds :func:`write_cube_png`. ``source_sha256`` is the sha256
    of the WAV file and ``label`` names the clip in the title (default: the
    stem). ``method='pipeline_r2'`` selects the archived comparison formulas
    instead. ``provenance.generator_sha256`` is :func:`generator_sha256`."""
    _check_method(method)
    params = LIBRARY_PARAMS
    if not isinstance(source_sha256, str) or len(source_sha256) != 64 or any(c not in "0123456789abcdef" for c in source_sha256):
        raise ValueError("source_sha256 must be 64 lowercase hex digits")
    y = np.asarray(audio, dtype=np.float64)
    if y.ndim != 1 or len(y) == 0:
        raise ValueError("library_cube expects non-empty mono audio")
    sample_rate = validate_sample_rate(sample_rate)
    y = as_mono(y)
    validate_frame_count(y.size, sample_rate)
    if method == PIPELINE_R2:
        mag = pipeline_r2_stft(y, n_fft=params.n_fft, hop=params.hop)
        full = compute_layers(mag, sample_rate, params.n_fft, method=method)
        layers = full
        sf, st = PIPELINE_R2_DOWNSAMPLE
    else:
        mag = stft_mag(y, n_fft=params.n_fft, hop=params.hop)
        full = compute_layers(mag, sample_rate, params.n_fft)
        layers, sf, st = downsample_cube(full, max_f=params.max_f, max_t=params.max_t)
    cloud = layers_to_points(layers, thresh=params.thresh)
    x, yy, _z, _rgba, vals, lids = cloud
    nf, nt = next(iter(layers.values())).shape
    bin_s = st * params.hop / sample_rate
    points, counts = preview_points(cloud, (nf, nt), params.per_layer)
    stats = {}
    for name in LAYER_NAMES:
        mat = full[name]
        if method == PIPELINE_R2:
            mat = mat.reshape(-1)
        stats[name] = {
            "mean": float(mat.mean()),
            "std": float(mat.std()),
            "p50": float(np.percentile(mat, 50)),
            "p90": float(np.percentile(mat, 90)),
            "active_frac": float((mat > 0.2).mean()),
            "shape": [int(nf), int(nt)],
        }
    doc = {
        "source_wav": f"artifacts/library/{stem}.wav",
        "source_sha256": source_sha256,
        "engine": engine,
        "title": f"Inverse-HDR cube \u2014 {label or stem}",
        "cube_revision": LIBRARY_REVISION if method == LIBRARY_R3 else 2,
        "sample_rate": int(sample_rate),
        "duration_s": float(len(y) / sample_rate),
        "n_fft": params.n_fft,
        "hop": params.hop,
        "inv_hdr": measure(y, sample_rate).inv_hdr,
        "layer_score": layer_score(full),
        "n_points": len(points),
        "n_points_source": int(len(x)),
        "n_points_source_per_layer": counts,
        "cube_shape_f_t": [int(nf), int(nt)],
        "downsample_sf_st": [int(sf), int(st)],
        "bin_seconds": bin_s,
        "cube_covers_s": nt * bin_s,
        "stft_frames": int(mag.shape[1]),
        "axes": {"x": "time bin", "y": "freq bin", "z": "layer (+value)"},
        "layers": stats,
        "points_preview": points,
        "pngUrl": f"/library/{cube_png_name(stem, method)}",
        "wavUrl": f"/library/{stem}.wav",
        "legend": dict(LEGEND),
        "provenance": {
            "generator": GENERATOR_PATH,
            "generator_sha256": generator_sha256(),
            "layer_method": method,
            "params": asdict(params),
        },
    }
    if method == PIPELINE_R2:
        doc["title"] += f" \u2014 {LAYER_METHOD_LABELS[method]}"
        doc.update({
            "layer_method": method,
            "layer_method_label": LAYER_METHOD_LABELS[method],
            "layer_method_source": PIPELINE_R2_SOURCE,
            "layer_stats_on": "r2 grid: 5 x 33 block average of the STFT magnitude, before the layer formulas",
            "compare_to": f"/library/{cube_json_name(stem)}",
        })
        doc["provenance"]["params"] = {
            "n_fft": params.n_fft, "hop": params.hop,
            "downsample_sf_st": list(PIPELINE_R2_DOWNSAMPLE),
            "thresh": params.thresh, "per_layer": params.per_layer,
        }
    return doc, cloud


def write_cube_png(path: Path | str, cloud: tuple, title: str, max_points: int = 80000) -> Path:
    """Static 3D scatter of the cube point cloud (matplotlib, Agg)."""
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    x, y, z, rgba, _vals, _lids = cloud
    n = len(x)
    if n > max_points:
        idx = np.random.default_rng(42).choice(n, size=max_points, replace=False)
        x, y, z, rgba = x[idx], y[idx], z[idx], rgba[idx]
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    fig = plt.figure(figsize=(12, 9), dpi=140)
    ax = fig.add_subplot(111, projection="3d")
    ax.scatter(x, y, z, c=rgba, s=2.0, linewidths=0, depthshade=True)
    ax.set_xlabel("time bin")
    ax.set_ylabel("freq bin")
    ax.set_zlabel("layer (+ value)")
    ax.set_zticks([0, 1, 2, 3])
    ax.set_zticklabels(list(LAYER_NAMES))
    ax.set_title(title)
    for i, name in enumerate(LAYER_NAMES):
        ax.scatter([], [], [], c=[LAYER_COLORS[i]], label=name, s=30)
    ax.legend(loc="upper left", fontsize=8)
    ax.view_init(elev=22, azim=-58)
    fig.tight_layout()
    fig.savefig(out, bbox_inches="tight")
    plt.close(fig)
    return out
