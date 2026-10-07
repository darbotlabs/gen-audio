#!/usr/bin/env python3
"""Score existing WAVs with inv-HDR, BW95, and clip fraction. Does not synthesize."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from gen_audio.cli.compare import main

if __name__ == "__main__":
    raise SystemExit(main())
