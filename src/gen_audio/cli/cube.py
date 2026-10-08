"""CLI for the inverse-HDR cube.

Two commands share this entry point (scripts/cube_revision.py):

- ``cube_revision.py INPUT -o OUTPUT [...]`` runs the revision sketch and writes
  WAV, JSON and an optional plot (unchanged).
- ``cube_revision.py layers WAV OUT_JSON --stem STEM --engine ENGINE
  [--label LABEL] [--png PNG]`` builds the rev 3 four-layer Library cube JSON
  (and PNG) that the Library tiles and the Cube tab load, with the WAV sha256
  and the generator's normalized sha256. See gen_audio.cube_layers.
  ``--method pipeline_r2`` instead writes the comparison cube (PR #4's
  formulas, gen_audio.cube_pipeline_r2) that the Cube tab's Compare mode draws
  next to the library_r3 cube. The command dispatches by method to the module
  that owns the formulas (``LAYER_METHODS``).
- ``cube_revision.py manifest [--manifest PATH] [--record-generator-commit]``
  rewrites the cube mirror in the Library manifest from the cube JSON
  (gen_audio.library_manifest; standard library only, no WAVs needed). With
  ``--record-generator-commit`` it also writes each cube block's informational
  ``generator_commit`` from git history; run it after the regen is committed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

from gen_audio.library_manifest import record_generator_commits, sync_manifest

# numpy/soundfile-backed modules load inside the commands that need them, so
# `manifest` runs on a bare Python (CI's Windows scripts job).
LIBRARY_MANIFEST = Path(__file__).resolve().parents[3] / "apps" / "desktop" / "public" / "library" / "manifest.json"

# layer_method -> the module that owns those formulas. Each module's bytes are
# its cubes' identity (provenance.generator_sha256), so an edit to one method
# never moves the other's uids. Modules load lazily (numpy), see above.
LAYER_METHODS = {"library_r3": "gen_audio.cube_layers", "pipeline_r2": "gen_audio.cube_pipeline_r2"}
DEFAULT_LAYER_METHOD = "library_r3"


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
    parser.add_argument("--label", help="clip name in the title, e.g. misaki\u2192kokoro (default: the stem; library_r3 only)")
    parser.add_argument("--png", type=Path, help="also write the 3D scatter PNG here")
    parser.add_argument(
        "--method",
        choices=tuple(LAYER_METHODS),
        default=DEFAULT_LAYER_METHOD,
        help="layer formulas: library_r3 (default, the Library cube, gen_audio.cube_layers) or "
        "pipeline_r2 (PR #4's formulas, comparison only, gen_audio.cube_pipeline_r2)",
    )
    return parser


def manifest_main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="cube_revision.py manifest",
        description="Rewrite the Library manifest cube blocks from the four-layer cube JSON (no WAVs needed).",
    )
    parser.add_argument("--manifest", type=Path, default=LIBRARY_MANIFEST, help="manifest.json (default: the app's)")
    parser.add_argument(
        "--record-generator-commit",
        action="store_true",
        help="also set each cube block's generator_commit (information only, outside the uid) from git history",
    )
    args = parser.parse_args(argv)
    try:
        changed = sync_manifest(args.manifest)
        recorded = record_generator_commits(args.manifest, LIBRARY_MANIFEST.parents[4]) if args.record_generator_commit else None
    except (OSError, ValueError, KeyError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(f"manifest cube mirror: {', '.join(changed) if changed else 'up to date'}")
    if recorded is not None:
        print(f"generator_commit: {', '.join(recorded) if recorded else 'up to date'}")
    return 0


def build_cube(method: str, audio, sample_rate: int, *, stem: str, engine: str, source_sha256: str, label: str | None = None):
    """(doc, point_cloud) from the module that owns ``method`` (LAYER_METHODS)."""
    if method == "pipeline_r2":
        from gen_audio.cube_pipeline_r2 import pipeline_r2_cube

        return pipeline_r2_cube(audio, sample_rate, stem=stem, engine=engine, source_sha256=source_sha256)
    if method != DEFAULT_LAYER_METHOD:
        raise ValueError(f"unknown layer_method {method!r}; expected one of {', '.join(LAYER_METHODS)}")
    from gen_audio.cube_layers import library_cube

    return library_cube(audio, sample_rate, stem=stem, engine=engine, source_sha256=source_sha256, label=label)


def layers_main(argv: list[str]) -> int:
    from gen_audio.audio_io import read_wav
    from gen_audio.cube_layers import write_cube_png

    args = build_layers_parser().parse_args(argv)
    try:
        audio, sample_rate = read_wav(args.wav)
        doc, cloud = build_cube(
            args.method,
            audio,
            sample_rate,
            stem=args.stem,
            engine=args.engine,
            source_sha256=hashlib.sha256(args.wav.read_bytes()).hexdigest(),
            label=args.label,
        )
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(doc, ensure_ascii=False), encoding="utf-8")
        written = [args.out]
        if args.png is not None:
            written.append(write_cube_png(args.png, cloud, doc["title"]))
    except (OSError, ValueError, RuntimeError) as exc:
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
