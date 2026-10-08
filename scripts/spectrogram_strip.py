#!/usr/bin/env python3
"""Shim. The strip generator lives in gen_audio.spectrogram_strip."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from gen_audio.cli.spectrogram_strip import main  # noqa: E402
from gen_audio.spectrogram_strip import colormap, ms_from_frames, read_wav, strip, write_png  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
