#!/usr/bin/env python3
"""Shim. The generator lives in gen_audio.cli.generate."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from gen_audio.cli.generate import main

if __name__ == "__main__":
    raise SystemExit(main())
