"""CLI for the inverse-HDR cube.

Two commands share this entry point (scripts/cube_revision.py):

- ``cube_revision.py INPUT -o OUTPUT [...]`` runs the revision sketch and writes
  WAV, JSON and an optional plot (unchanged).
- ``cube_revision.py layers WAV OUT_JSON --stem STEM --engine ENGINE [--revision N]
  [--label LABEL] [--png PNG]`` builds the four-layer Library cube JSON (and
  PNG) that the Library tiles and the Cube tab load. See gen_audio.cube_layers.
- ``cube_revision.py manifest [--manifest PATH]`` rewrites the cube mirror in
  the Library manifest from the cube JSON (gen_audio.library_manifest;
  standard library only, no WAVs needed).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from gen_audio.library_manifest import sync_manifest

# numpy/soundfile-backed modules load inside the commands that need them, so
# `manifest` runs on a bare Python (CI's Windows scripts job).
LIBRARY_MANIFEST = Path(__file__).resolve().parents[3] / "apps" / "desktop" / "public" / "library" / "manifest.json"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Revise a WAV with the inverse-HDR / BW95 / clip-fraction sketch. Does not replace the publish chain."
    )
    parser.add_argument("input", type=Path, help="input WAV")
    parser.add_argument("-o", "--output", type=Path, required=True, help="revised WAV")
    parser.add_argument("--report", type=Path, help="JSON report path (default: output path with .json)")
    parser.add_argument("--plot", type=Path, help="optional 3D cube PNG")
    parser.add_argument("--max-steps", type=int, help="maximum serial ops (default: cube_revision.DEFAULT_MAX_STEPS)")
    parser.add_argument("--artifact-dir", help="copy outputs here (or set GEN_AUDIO_ARTIFACT_DIR)")
    return parser


def build_layers_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="cube_revision.py layers",
        description="Build the four-layer inverse-HDR Library cube JSON for one WAV (covers the full WAV).",
    )
    parser.add_argument("wav", type=Path, help="input WAV")
    parser.add_argument("out", type=Path, help="output cube JSON, e.g. apps/desktop/public/library/library_<stem>_cube3d.json")
    parser.add_argument("--stem", required=True, help="clip file stem; sets wavUrl, pngUrl and source_wav")
    parser.add_argument("--engine", required=True, help="engine id recorded in the cube JSON")
    parser.add_argument("--revision", type=int, default=1, help="cube_revision (bump when the cube changes)")
    parser.add_argument("--label", help="clip name in the title, e.g. misaki\u2192kokoro (default: the stem)")
    parser.add_argument("--png", type=Path, help="also write the 3D scatter PNG here")
    return parser


def manifest_main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="cube_revision.py manifest",
        description="Rewrite the Library manifest cube blocks from the four-layer cube JSON (no WAVs needed).",
    )
    parser.add_argument("--manifest", type=Path, default=LIBRARY_MANIFEST, help="manifest.json (default: the app's)")
    args = parser.parse_args(argv)
    try:
        changed = sync_manifest(args.manifest)
    except (OSError, ValueError, KeyError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(f"manifest cube mirror: {', '.join(changed) if changed else 'up to date'}")
    return 0


def layers_main(argv: list[str]) -> int:
    from gen_audio.audio_io import read_wav
    from gen_audio.cube_layers import library_cube, write_cube_png

    args = build_layers_parser().parse_args(argv)
    try:
        audio, sample_rate = read_wav(args.wav)
        doc, cloud = library_cube(
            audio, sample_rate, stem=args.stem, engine=args.engine, revision=args.revision, label=args.label
        )
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(doc, ensure_ascii=False), encoding="utf-8")
        written = [args.out]
        if args.png is not None:
            written.append(write_cube_png(args.png, cloud, doc["title"]))
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    summary = {k: v for k, v in doc.items() if k not in ("points_preview", "layers")}
    print(json.dumps(summary, ensure_ascii=False))
    for path in written:
        print(f"wrote {path}")
    return 0


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] == "layers":
        return layers_main(argv[1:])
    if argv and argv[0] == "manifest":
        return manifest_main(argv[1:])
    from gen_audio.artifacts import publish_copy
    from gen_audio.audio_io import read_wav, write_wav
    from gen_audio.cube_revision import DEFAULT_MAX_STEPS, revise, write_cube_plot

    args = build_parser().parse_args(argv)
    if args.max_steps is None:
        args.max_steps = DEFAULT_MAX_STEPS
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
