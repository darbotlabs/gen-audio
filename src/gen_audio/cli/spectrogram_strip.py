"""CLI: write the livetile 2D spectrogram strip (PNG + sidecar JSON) for one WAV."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from gen_audio.spectrogram_strip import StripParams, write_strip


def build_parser() -> argparse.ArgumentParser:
    defaults = StripParams()
    parser = argparse.ArgumentParser(description="Write library_<stem>_spec2d.png and its sidecar JSON for a library WAV.")
    parser.add_argument("wav", type=Path)
    parser.add_argument("--clip-id", required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--n-fft", type=int, default=defaults.n_fft)
    parser.add_argument("--columns-per-second", type=int, default=defaults.columns_per_second)
    parser.add_argument("--bands", type=int, default=defaults.bands)
    parser.add_argument("--f-min", type=float, default=defaults.f_min)
    parser.add_argument("--f-max", type=float, default=defaults.f_max)
    parser.add_argument("--db-floor", type=int, default=defaults.db_floor)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    params = StripParams(args.n_fft, args.columns_per_second, args.bands, args.f_min, args.f_max, args.db_floor)
    print(json.dumps(write_strip(args.wav, args.clip_id, args.out_dir, params)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
