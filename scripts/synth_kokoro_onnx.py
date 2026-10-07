#!/usr/bin/env python3
"""Synthesize a Speaker script with kokoro-onnx."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from gen_audio.cli.synth import main

if __name__ == "__main__":
    raise SystemExit(main())
