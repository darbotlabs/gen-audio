#!/usr/bin/env python3
"""Thin shim: the strip lives in gen_audio.spectrogram_strip (CLI: gen-audio-spectrogram-strip)."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from gen_audio.cli.spectrogram_strip import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
