"""CLI: score existing WAVs. Does not synthesize and does not rank engines."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from gen_audio.artifacts import publish_copy
from gen_audio.compare import score_files
from gen_audio.engines import list_engines


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Measure existing WAV files (engine=path). Unregistered engine ids are allowed and marked."
    )
    parser.add_argument("pairs", nargs="*", help="engine=path pairs, for example kokoro_onnx=take.wav")
    parser.add_argument("-o", "--output", type=Path, help="write the JSON list to this path")
    parser.add_argument("--list-engines", action="store_true", help="print the compare-list ids and exit")
    parser.add_argument("--artifact-dir", help="copy the JSON here (or set GEN_AUDIO_ARTIFACT_DIR)")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.list_engines:
        for spec in list_engines():
            print(f"{spec.id}\t{spec.kind}\t{spec.status}\t{spec.summary}")
        return 0
    if not args.pairs:
        print("error: pass engine=path pairs, or --list-engines", file=sys.stderr)
        return 1
    try:
        items = [_parse_pair(token) for token in args.pairs]
        rows = score_files(items)
        payload = json.dumps(rows, indent=2) + "\n"
        if args.output is not None:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(payload, encoding="utf-8")
            durable = publish_copy(args.output, args.artifact_dir)
            print(f"wrote {durable}")
        else:
            sys.stdout.write(payload)
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


def _parse_pair(token: str) -> tuple[str, Path]:
    if "=" not in token:
        raise ValueError(f"expected engine=path, got {token!r}")
    engine, raw = token.split("=", 1)
    engine = engine.strip()
    if not engine or not raw.strip():
        raise ValueError(f"expected engine=path, got {token!r}")
    return engine, Path(raw.strip())


if __name__ == "__main__":
    raise SystemExit(main())
