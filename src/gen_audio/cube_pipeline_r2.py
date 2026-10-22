"""Layer method ``pipeline_r2``: PR #4's cube formulas, for comparison only.

The Cube tab's Compare mode draws this cube beside the clip's Library cube
(``library_r3``, :mod:`gen_audio.cube_layers`) on one playback slice. It is
never a clip's primary cube.

Why this is its own module: a cube's identity is the content hash of the
module that produced it (``provenance.generator_sha256``). If these formulas
lived in cube_layers.py, editing them would change the uid of every Library
cube even though the library_r3 formulas did not change. Each formula lives
in its own module, so editing one cannot move the identity of the other.

Each pipeline_r2 cube JSON records ``provenance``: ``generator`` (this
file's repo path, :data:`GENERATOR_PATH`), ``generator_sha256`` (sha256 of
this file's bytes with CRLF normalized to LF, :func:`generator_sha256`),
``layer_method`` and ``params``. Those bytes are the compare cube asset's
identity, exactly as for a Library cube (gen_audio.cube_layers), so editing
this file moves the compare cubes' uids and needs a regen
(``cube_revision.py layers ... --method pipeline_r2``), and it never moves a
Library cube. build_assets and tests/test_cube_pipeline_r2.py fail on a stale
hash.

Shared math is imported from :mod:`gen_audio.cube_layers`, never copied:
``preview_points`` (the point preview selection, so the two clouds differ
only by the formulas), ``layers_to_points``, ``layer_score``, ``CubeParams``,
``LAYER_NAMES`` and ``LEGEND``; the PNG is ``cube_layers.write_cube_png``. A copy
would fork that math silently. tests/test_cube_pipeline_r2.py fails if this
module defines its own ``preview_points`` or ``stft_mag``.

What is r2's own (a faithful port of darbotlabs/gen-audio@204d0de
``src/gen_audio/pipeline.py``: ``cube_document`` L275-L337,
``_stft_magnitude`` L340-L349, ``_frame_flatness`` L383-L387, ``_unit``
L390-L394, ``_layer_stats`` L397-L406):

- STFT (:func:`stft`): zero padding of n_fft/2 before and n_fft/2 + hop
  after, ``1 + n // hop`` frames, Hann. This is a different formula from
  ``cube_layers.stft_mag`` (reflect padding), not a copy of it.
- Grid (:func:`grid`): crop to whole 5 x 33 (freq x time) blocks and
  block-average; the layers are computed on that grid:
  ``signal`` = grid / max(grid),
  ``tonality`` = (1 - per-frame spectral flatness) where signal > 1e-4,
  ``confidence`` = grid / (grid + p70(grid) + 1e-12),
  ``quality`` = 0.5 signal + 0.5 tonality.
- Layer stats over the flattened grid (:func:`layer_stats`), as in PR #4.

inv_hdr is ``cube_revision.measure(...).inv_hdr`` (rms / peak), the same as
library_r3. A pipeline_r2 cube JSON records ``layer_method``,
``layer_method_label``, ``layer_method_source``, ``layer_stats_on``,
``source_sha256`` (the WAV's sha256) and ``compare_to`` (the clip's Library
cube JSON).
"""

from __future__ import annotations

from pathlib import Path

import numpy as np

from gen_audio.cube_layers import (
    LAYER_NAMES,
    LEGEND,
    CubeParams,
    layer_score,
    layers_to_points,
    preview_points,
)
from gen_audio.cube_revision import measure
from gen_audio.library_manifest import normalized_sha256

__all__ = [
    "DOWNSAMPLE",
    "GENERATOR_PATH",
    "LAYER_METHOD",
    "LAYER_METHOD_LABEL",
    "LAYER_METHOD_SOURCE",
    "PARAMS",
    "REVISION",
    "check_generator",
    "compute_layers",
    "cube_json_name",
    "cube_png_name",
    "generator_sha256",
    "grid",
    "layer_stats",
    "pipeline_r2_cube",
    "stft",
]

LAYER_METHOD = "pipeline_r2"
LAYER_METHOD_LABEL = "Pipeline formulas, rev 2 (PR #4)"
LAYER_METHOD_SOURCE = (
    "darbotlabs/gen-audio@204d0de45766ad31af89d8f5aee6c1a014010834 src/gen_audio/pipeline.py "
    "cube_document L275-L337, _stft_magnitude L340-L349, _frame_flatness L383-L387, _unit L390-L394"
)
# PR #4 CUBE_DOWNSAMPLE (pipeline.py L251): freq x time block size of the r2 grid.
DOWNSAMPLE = (5, 33)
# n_fft, hop, thresh and per_layer as for library_r3 (max_f / max_t are unused: the grid is fixed 5 x 33 blocks).
PARAMS = CubeParams()
# cube_revision of every pipeline_r2 cube (PR #4's cube_document is revision 2).
REVISION = 2
GENERATOR_PATH = "src/gen_audio/cube_pipeline_r2.py"
REPO_ROOT = Path(__file__).resolve().parents[2]


def generator_sha256(repo: Path | str = REPO_ROOT) -> str:
    """sha256 of this module's bytes with CRLF normalized to LF (the compare cubes' identity)."""
    return normalized_sha256((Path(repo) / GENERATOR_PATH).read_bytes())


def provenance_params() -> dict:
    """The params a pipeline_r2 cube is made with (recorded in its provenance)."""
    return {
        "n_fft": PARAMS.n_fft,
        "hop": PARAMS.hop,
        "grid_sf_st": list(DOWNSAMPLE),
        "thresh": PARAMS.thresh,
        "per_layer": PARAMS.per_layer,
    }


def check_generator(doc: dict, repo: Path | str = REPO_ROOT) -> None:
    """Raise ValueError unless a pipeline_r2 cube JSON was made by this module's bytes."""
    provenance = doc.get("provenance") or {}
    recorded, current = provenance.get("generator_sha256"), generator_sha256(repo)
    if provenance.get("generator") != GENERATOR_PATH or recorded != current:
        raise ValueError(
            f"cube {doc.get('wavUrl')} names {provenance.get('generator')} sha256 {recorded}, but it is a "
            f"{LAYER_METHOD} cube and {GENERATOR_PATH} hashes to {current} (CRLF->LF); regenerate: "
            "python scripts/cube_revision.py layers WAV OUT --stem STEM --engine ENGINE --method pipeline_r2"
        )


def stft(samples: np.ndarray, n_fft: int = 1024, hop: int = 256) -> np.ndarray:
    """PR #4 ``_stft_magnitude`` (pipeline.py L340-L349): zero padding of
    n_fft/2 before and n_fft/2 + hop after, ``1 + n // hop`` frames, Hann."""
    values = np.asarray(samples, dtype=np.float64).reshape(-1)
    frames = 1 + int(values.size) // hop
    padded = np.pad(values, (n_fft // 2, n_fft // 2 + hop))
    window = np.hanning(n_fft)
    strides = (padded.strides[0] * hop, padded.strides[0])
    view = np.lib.stride_tricks.as_strided(padded, shape=(frames, n_fft), strides=strides, writeable=False)
    return np.abs(np.fft.rfft(view * window, axis=1)).T


def grid(mag: np.ndarray) -> np.ndarray:
    """PR #4 cube_document grid (pipeline.py L290-L296): crop to whole 5 x 33
    blocks and block-average. Raises when the clip is shorter than one block."""
    sf, st = DOWNSAMPLE
    freq_bins, time_bins = mag.shape[0] // sf, mag.shape[1] // st
    if freq_bins < 1 or time_bins < 1:
        raise ValueError("audio is too short for a pipeline_r2 cube")
    cropped = mag[: freq_bins * sf, : time_bins * st]
    return cropped.reshape(freq_bins, sf, time_bins, st).mean(axis=(1, 3))


def compute_layers(cells: np.ndarray) -> dict[str, np.ndarray]:
    """PR #4 cube_document L297-L310 on the r2 grid (from :func:`grid`), float64 like the original."""
    peak = float(np.max(cells)) if cells.size else 0.0
    signal = np.zeros_like(cells) if peak <= 1e-12 else cells / peak  # _unit, L390-L394
    power = np.maximum(cells, 1e-12)  # _frame_flatness, L383-L387
    flatness = np.clip(np.exp(np.mean(np.log(power), axis=0)) / np.maximum(np.mean(power, axis=0), 1e-12), 0.0, 1.0)
    tonality = (1.0 - flatness) * (signal > 1e-4)
    floor = float(np.percentile(cells, 70))
    confidence = np.clip(cells / (cells + floor + 1e-12), 0.0, 1.0)
    quality = 0.5 * signal + 0.5 * np.clip(tonality, 0.0, 1.0)
    return {
        "signal": signal,
        "tonality": np.clip(tonality, 0.0, 1.0),
        "confidence": confidence,
        "quality": np.clip(quality, 0.0, 1.0),
    }


def layer_stats(layer: np.ndarray) -> dict:
    """PR #4 ``_layer_stats`` (pipeline.py L397-L406), over the flattened grid layer."""
    flat = layer.reshape(-1)
    return {
        "mean": float(np.mean(flat)),
        "std": float(np.std(flat)),
        "p50": float(np.percentile(flat, 50)),
        "p90": float(np.percentile(flat, 90)),
        "active_frac": float(np.mean(flat > 0.2)),
        "shape": [int(layer.shape[0]), int(layer.shape[1])],
    }


def cube_json_name(stem: str) -> str:
    """File name of a pipeline_r2 cube JSON: library_<stem>_pipeline_r2_cube3d.json."""
    return f"library_{stem}_{LAYER_METHOD}_cube3d.json"


def cube_png_name(stem: str) -> str:
    """File name of a pipeline_r2 cube PNG: <stem>_pipeline_r2_cube3d.png."""
    return f"{stem}_{LAYER_METHOD}_cube3d.png"


def _check_sha256(value: object) -> str:
    if not (isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)):
        raise ValueError(f"layer_method {LAYER_METHOD} needs the WAV's lowercase hex source_sha256")
    return value


def pipeline_r2_cube(
    audio: np.ndarray,
    sample_rate: int,
    *,
    stem: str,
    engine: str,
    source_sha256: str,
) -> tuple[dict, tuple]:
    """Build the pipeline_r2 comparison cube document of one WAV. Returns
    (doc, point_cloud) where point_cloud feeds ``cube_layers.write_cube_png``.

    ``source_sha256`` is the lowercase hex sha256 of the WAV file the samples
    came from, so the UI can refuse to compare cubes of different WAVs.
    ``provenance`` names this module and :func:`generator_sha256`.
    """
    source_sha256 = _check_sha256(source_sha256)
    params = PARAMS
    y = np.asarray(audio, dtype=np.float64)
    if y.ndim != 1 or len(y) == 0:
        raise ValueError("pipeline_r2_cube expects non-empty mono audio")
    if sample_rate <= 0:
        raise ValueError("sample rate must be positive")
    mag = stft(y, n_fft=params.n_fft, hop=params.hop)
    # PR #4 computes the layers on the grid, so its stats (and layer_score) are grid stats.
    layers = compute_layers(grid(mag))
    sf, st = DOWNSAMPLE
    cloud = layers_to_points(layers, thresh=params.thresh)
    x = cloud[0]
    nf, nt = next(iter(layers.values())).shape
    bin_s = st * params.hop / sample_rate
    points, counts = preview_points(cloud, (nf, nt), params.per_layer)
    stats = {name: layer_stats(layers[name]) for name in LAYER_NAMES}
    doc = {
        "source_wav": f"artifacts/library/{stem}.wav",
        "engine": engine,
        "title": f"Inverse-HDR bitdot cube \u2014 {stem} \u2014 {LAYER_METHOD_LABEL}",
        "cube_revision": REVISION,
        "sample_rate": int(sample_rate),
        "duration_s": float(len(y) / sample_rate),
        "n_fft": params.n_fft,
        "hop": params.hop,
        "inv_hdr": measure(y, sample_rate).inv_hdr,
        "layer_score": layer_score(layers),
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
        "pngUrl": f"/library/{cube_png_name(stem)}",
        "wavUrl": f"/library/{stem}.wav",
        "legend": dict(LEGEND),
        "layer_method": LAYER_METHOD,
        "layer_method_label": LAYER_METHOD_LABEL,
        "layer_method_source": LAYER_METHOD_SOURCE,
        "layer_stats_on": "r2 grid: 5 x 33 block average of the STFT magnitude, before the layer formulas",
        "source_sha256": source_sha256,
        "compare_to": f"/library/library_{stem}_cube3d.json",
        "provenance": {
            "generator": GENERATOR_PATH,
            "generator_sha256": generator_sha256(),
            "layer_method": LAYER_METHOD,
            "params": provenance_params(),
        },
    }
    return doc, cloud
