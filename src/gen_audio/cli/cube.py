"""CLI: run the inverse-HDR cube revision sketch and write WAV, JSON, and an optional plot."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from gen_audio.artifacts import publish_copy
from gen_audio.audio_io import read_wav, write_wav
from gen_audio.cube_revision import DEFAULT_MAX_STEPS, revise, write_cube_plot


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Revise a WAV with the inverse-HDR / BW95 / clip-fraction sketch. Does not replace the publish chain."
    )
    parser.add_argument("input", type=Path, help="input WAV")
    parser.add_argument("-o", "--output", type=Path, required=True, help="revised WAV")
    parser.add_argument("--report", type=Path, help="JSON report path (default: output path with .json)")
    parser.add_argument("--plot", type=Path, help="optional 3D cube PNG")
    parser.add_argument("--max-steps", type=int, default=DEFAULT_MAX_STEPS, help="maximum serial ops")
    parser.add_argument("--artifact-dir", help="copy outputs here (or set GEN_AUDIO_ARTIFACT_DIR)")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        audio, sample_rate = read_wav(args.input)
        result = revise(audio, sample_rate, max_steps=args.max_steps)
        write_wav(args.output, result.audio, result.sample_rate)
        report_path = args.report or args.output.with_suffix(".json")
        report = {
            "sample_rate": result.sample_rate,
            "ops": result.ops,
            "before": result.before.to_dict(),
            "after": result.after.to_dict(),
            "before_point": result.before.point(),
            "after_point": result.after.point(),
        }
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        written = [publish_copy(args.output, args.artifact_dir), publish_copy(report_path, args.artifact_dir)]
        if args.plot is not None:
            write_cube_plot(args.plot, result.before, result.after)
            written.append(publish_copy(args.plot, args.artifact_dir))
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(f"ops: {', '.join(result.ops) if result.ops else '(none)'}")
    for path in written:
        print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
