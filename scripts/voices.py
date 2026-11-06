#!/usr/bin/env python3
"""List and validate voice catalogs and cast maps. Does not synthesize."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from gen_audio.cli.voices import main

if __name__ == "__main__":
    raise SystemExit(main())
