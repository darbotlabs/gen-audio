"""CLI: write a spectrogram PNG, and a before/after panel when both files are given."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from gen_audio.artifacts import publish_copy
from gen_audio.audio_io import read_wav
from gen_audio.spectrogram import write_before_after_panel, write_spectrogram


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Write spectrogram PNGs for one WAV, or a before/after panel.")
    parser.add_argument("--before", type=Path, required=True, help="WAV to plot (the 'before' file when --after is set)")
    parser.add_argument("--after", type=Path, help="optional second WAV")
    parser.add_argument("--out-dir", type=Path, required=True, help="directory for PNG files")
    parser.add_argument("--title", default="gen-audio", help="plot title")
    parser.add_argument("--artifact-dir", help="copy PNGs here (or set GEN_AUDIO_ARTIFACT_DIR)")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        args.out_dir.mkdir(parents=True, exist_ok=True)
        before, before_sr = read_wav(args.before)
        before_path = args.out_dir / "spectrogram_before.png"
        write_spectrogram(before_path, before, before_sr, title=f"{args.title} — before")
        written = [publish_copy(before_path, args.artifact_dir)]
        if args.after is not None:
            after, after_sr = read_wav(args.after)
            after_path = args.out_dir / "spectrogram_after.png"
            panel_path = args.out_dir / "spectrogram_panel.png"
            write_spectrogram(after_path, after, after_sr, title=f"{args.title} — after")
            write_before_after_panel(
                panel_path,
                before,
                before_sr,
                after,
                after_sr,
                title=args.title,
            )
            written.append(publish_copy(after_path, args.artifact_dir))
            written.append(publish_copy(panel_path, args.artifact_dir))
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    for path in written:
        print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
