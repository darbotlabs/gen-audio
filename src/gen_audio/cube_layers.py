"""Four-layer inverse-HDR "bitdot" cube for Library clips.

This is the generator behind the shipped Library cube JSON files that carry
``cube_revision``, ``cube_shape_f_t``, ``downsample_sf_st`` and
``bin_seconds`` (misaki rev 2, bitdot rev 1). It replaces the one-off
``make_library_cube.py`` and ``make_optimus_*`` generators, which each kept
their own WAV reader, STFT and layer formulas.

Method (unchanged from the generator that produced the shipped cubes):

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
  quality (layer means over the full-resolution layers). Earlier revisions
  of this generator (misaki rev 2, bitdot rev 1) stored this composite
  under ``inv_hdr``; it is not rms/peak, so it now has its own key.

Scrub mapping: ``bin_seconds`` = ``downsample_sf_st[1] * hop / sample_rate``;
the cube covers ``cube_shape_f_t[1] * bin_seconds`` seconds of the WAV.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import numpy as np

from gen_audio.cube_revision import measure

LAYER_NAMES = ("signal", "tonality", "confidence", "quality")
LAYER_COLORS = (
    (0.15, 0.55, 1.0),
    (0.2, 0.9, 0.35),
    (1.0, 0.75, 0.1),
    (0.95, 0.25, 0.55),
)
LEGEND = {"signal": "blue", "tonality": "green", "confidence": "orange", "quality": "pink"}


@dataclass(frozen=True)
class CubeParams:
    n_fft: int = 1024
    hop: int = 256
    max_f: int = 128
    max_t: int = 400
    thresh: float = 0.12
    per_layer: int = 900


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


def compute_layers(mag: np.ndarray, sample_rate: int, n_fft: int) -> dict[str, np.ndarray]:
    """The four honesty layers at full STFT resolution, float32 in [0, 1]."""
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


def layer_score(layers: dict[str, np.ndarray]) -> float:
    """Weighted mean of the four layers (informational; not inv_hdr)."""
    return float(
        0.35 * layers["signal"].mean()
        + 0.25 * layers["tonality"].mean()
        + 0.20 * layers["confidence"].mean()
        + 0.20 * layers["quality"].mean()
    )


def library_cube(
    audio: np.ndarray,
    sample_rate: int,
    *,
    stem: str,
    engine: str,
    revision: int = 1,
    params: CubeParams = CubeParams(),
) -> tuple[dict, tuple]:
    """Build the Library cube document. Returns (doc, point_cloud) where
    point_cloud feeds :func:`write_cube_png`."""
    y = np.asarray(audio, dtype=np.float64)
    if y.ndim != 1 or len(y) == 0:
        raise ValueError("library_cube expects non-empty mono audio")
    if sample_rate <= 0:
        raise ValueError("sample rate must be positive")
    mag = stft_mag(y, n_fft=params.n_fft, hop=params.hop)
    full = compute_layers(mag, sample_rate, params.n_fft)
    layers, sf, st = downsample_cube(full, max_f=params.max_f, max_t=params.max_t)
    cloud = layers_to_points(layers, thresh=params.thresh)
    x, yy, _z, _rgba, vals, lids = cloud
    nf, nt = next(iter(layers.values())).shape
    bin_s = st * params.hop / sample_rate
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
    doc = {
        "source_wav": f"artifacts/library/{stem}.wav",
        "engine": engine,
        "title": f"Inverse-HDR bitdot cube \u2014 {stem}",
        "cube_revision": int(revision),
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
        "pngUrl": f"/library/{stem}_cube3d.png",
        "wavUrl": f"/library/{stem}.wav",
        "legend": dict(LEGEND),
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
