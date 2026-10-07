"""CLI: trim, high-pass, peak-normalize, and resample a WAV to 24 kHz."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from gen_audio.artifacts import publish_copy
from gen_audio.audio_io import read_wav, write_wav
from gen_audio.improve import HIGHPASS_HZ, PEAK_TARGET, PUBLISH_SAMPLE_RATE, TRIM_PAD_MS, TRIM_THRESHOLD_DB, improve


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Publish a WAV: trim silence, high-pass, peak-normalize, resample to 24 kHz.")
    parser.add_argument("input", type=Path, help="input WAV")
    parser.add_argument("-o", "--output", type=Path, required=True, help="output WAV")
    parser.add_argument("--hp-hz", type=float, default=HIGHPASS_HZ, help="high-pass cutoff in Hz")
    parser.add_argument("--peak", type=float, default=PEAK_TARGET, help="linear peak target")
    parser.add_argument("--threshold-db", type=float, default=TRIM_THRESHOLD_DB, help="trim threshold relative to peak")
    parser.add_argument("--pad-ms", type=float, default=TRIM_PAD_MS, help="samples restored at each trim edge")
    parser.add_argument("--sample-rate", type=int, default=PUBLISH_SAMPLE_RATE, help="publish sample rate")
    parser.add_argument("--artifact-dir", help="copy the output here (or set GEN_AUDIO_ARTIFACT_DIR)")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        audio, sample_rate = read_wav(args.input)
        result = improve(
            audio,
            sample_rate,
            hp_hz=args.hp_hz,
            peak_target=args.peak,
            target_sr=args.sample_rate,
            threshold_db=args.threshold_db,
            pad_ms=args.pad_ms,
        )
        write_wav(args.output, result.audio, result.sample_rate)
        durable = publish_copy(args.output, args.artifact_dir)
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(
        f"wrote {durable} ({result.sample_rate} Hz, {result.output_seconds:.3f}s, "
        f"peak {result.peak:.4f}; was {result.input_seconds:.3f}s at {result.input_sample_rate} Hz)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
