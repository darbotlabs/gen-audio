"""Before/after spectrogram PNGs.

These plots are visual checks of a render and of the publish chain. They are
not a quality score and they do not identify an engine.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np

from gen_audio.audio_io import as_mono


def write_spectrogram(
    path: Path | str,
    audio: np.ndarray,
    sample_rate: int,
    *,
    title: str,
) -> Path:
    """Write one magnitude spectrogram PNG."""
    values = _require_plotable(audio, sample_rate)
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    figure, axes = _figure(figsize=(8.0, 3.2))
    _draw_specgram(axes, values, int(sample_rate))
    axes.set_title(title)
    axes.set_xlabel("Time (s)")
    axes.set_ylabel("Frequency (Hz)")
    figure.tight_layout()
    figure.savefig(destination, dpi=120)
    return destination


def write_before_after_panel(
    path: Path | str,
    before: np.ndarray,
    before_sr: int,
    after: np.ndarray,
    after_sr: int,
    *,
    title: str = "gen-audio before / after",
) -> Path:
    """Write a two-row panel: before on top, after underneath."""
    before_audio = _require_plotable(before, before_sr)
    after_audio = _require_plotable(after, after_sr)
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    figure, rows = _figure(figsize=(8.0, 6.4), nrows=2)
    _draw_specgram(rows[0], before_audio, int(before_sr))
    rows[0].set_title(f"{title} — before ({before_sr} Hz)")
    rows[0].set_ylabel("Frequency (Hz)")
    _draw_specgram(rows[1], after_audio, int(after_sr))
    rows[1].set_title(f"after ({after_sr} Hz)")
    rows[1].set_xlabel("Time (s)")
    rows[1].set_ylabel("Frequency (Hz)")
    figure.tight_layout()
    figure.savefig(destination, dpi=120)
    return destination


def _require_plotable(audio: np.ndarray, sample_rate: int) -> np.ndarray:
    if int(sample_rate) <= 0:
        raise ValueError("sample rate must be positive")
    values = as_mono(audio)
    if values.size < 256:
        raise ValueError(f"audio is too short for a spectrogram ({values.size} samples)")
    return values


def _figure(figsize: tuple[float, float], nrows: int = 1):
    from matplotlib.backends.backend_agg import FigureCanvasAgg
    from matplotlib.figure import Figure

    figure = Figure(figsize=figsize)
    FigureCanvasAgg(figure)
    if nrows == 1:
        return figure, figure.add_subplot(1, 1, 1)
    return figure, [figure.add_subplot(nrows, 1, index + 1) for index in range(nrows)]


def _draw_specgram(axes, audio: np.ndarray, sample_rate: int) -> None:
    nfft = 1024 if audio.size >= 1024 else 256
    axes.specgram(audio, Fs=sample_rate, NFFT=nfft, noverlap=nfft // 2, cmap="magma")
