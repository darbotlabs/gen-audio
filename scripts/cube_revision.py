#!/usr/bin/env python3
"""Run the inverse-HDR cube revision sketch."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from gen_audio.cli.cube import main

if __name__ == "__main__":
    raise SystemExit(main())
