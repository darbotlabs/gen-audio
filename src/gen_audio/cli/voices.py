"""CLI: list frameworks, validate a voice catalog, validate a cast map."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from gen_audio.cast import load_cast_map
from gen_audio.voices.catalog import library_root, load_catalog, missing_assets
from gen_audio.voices.errors import VoiceError
from gen_audio.voices.model import default_catalog_path
from gen_audio.voices.registry import list_frameworks
from gen_audio.voices.resolve import validate_cast

DEFAULT_CATALOG = default_catalog_path()


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="gen-audio-voices",
        description="List and validate voice catalogs and cast maps. Does not synthesize or download weights.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    frameworks = sub.add_parser("frameworks", help="print the voice framework registry")
    frameworks.set_defaults(func=_frameworks)

    listed = sub.add_parser("list", help="print voice ids from a catalog as JSON")
    listed.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG)
    listed.add_argument("--framework", help="only this framework id")
    listed.set_defaults(func=_list)

    validate = sub.add_parser("validate", help="validate a catalog and the documents it indexes")
    validate.add_argument("catalog", nargs="?", type=Path, default=DEFAULT_CATALOG)
    validate.add_argument("--library", type=Path, help="voice library (else GENAID_VOICES_DIR)")
    validate.add_argument(
        "--require-assets",
        action="store_true",
        help="fail when a descriptor's asset file is not in the library",
    )
    validate.add_argument("--cast", type=Path, help="also validate this cast map against the catalog")
    validate.set_defaults(func=_validate)

    plan = sub.add_parser("plan", help="print the synthesis plan for one catalog voice")
    plan.add_argument("voice_id")
    plan.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG)
    plan.set_defaults(func=_plan)

    scan = sub.add_parser("scan", help="classify files in a voice library")
    scan.add_argument("--library", type=Path)
    scan.set_defaults(func=_scan)

    rebuild = sub.add_parser("rebuild", help="classify a library and optionally check it against a catalog")
    rebuild.add_argument("--library", type=Path)
    rebuild.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG)
    rebuild.add_argument(
        "--check",
        action="store_true",
        help="exit 1 when scanned vibevoice and livevoice paths differ from the catalog",
    )
    rebuild.set_defaults(func=_rebuild)

    cast = sub.add_parser("validate-cast", help="validate a cast map, optionally against a catalog")
    cast.add_argument("cast_map", type=Path)
    cast.add_argument("--catalog", type=Path, help="check voice_id values against this catalog")
    cast.set_defaults(func=_validate_cast)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return int(args.func(args))
    except (OSError, ValueError, KeyError, VoiceError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


def _frameworks(_args) -> int:
    for spec in list_frameworks():
        engine = spec.engine_id or "-"
        wired = "wired" if spec.wired else "schema"
        print(f"{spec.id}\t{spec.kind}\t{engine}\t{wired}\t{spec.summary}")
    return 0


def _list(args) -> int:
    catalog = load_catalog(args.catalog)
    rows = []
    for voice in catalog.voices:
        if args.framework and voice.framework != args.framework:
            continue
        rows.append(
            {
                "id": voice.id,
                "display_name": voice.display_name,
                "language": voice.language,
                "gender": voice.gender,
                "framework": voice.framework,
                "tags": list(voice.tags),
            }
        )
    sys.stdout.write(json.dumps(rows, indent=2) + "\n")
    return 0


def _validate(args) -> int:
    catalog = load_catalog(args.catalog)
    if args.cast is not None:
        from gen_audio.cast import load_cast_map
        from gen_audio.voices.resolve import validate_cast

        validate_cast(load_cast_map(args.cast), catalog)
    if args.require_assets:
        root = library_root(args.library, catalog=catalog)
        if root is None:
            raise VoiceError("voice library is unset; pass --library or set GENAID_VOICES_DIR")
        missing = missing_assets(catalog, root)
        if missing:
            raise VoiceError("missing voice assets: " + ", ".join(missing))
    print(f"ok {len(catalog.voices)} voices, {len(catalog.g2p)} g2p ({catalog.id})")
    return 0


def _plan(args) -> int:
    from gen_audio.voices.registry import get_framework

    catalog = load_catalog(args.catalog)
    voice = catalog.voice(args.voice_id)
    plan = get_framework(voice.framework).prepare(voice, None)
    sys.stdout.write(
        json.dumps(
            {
                "voice_id": plan.voice_id,
                "framework": plan.framework,
                "executable": plan.executable,
                "missing": list(plan.missing),
            },
            indent=2,
        )
        + "\n"
    )
    return 0


def classify_library_file(relative: str) -> str:
    """Classify one path from a genaid-voices style library. Not a voice when excluded."""
    name = relative.replace("\\", "/")
    if name.startswith("_hf_vibevoice_samples/example_output/") and name.endswith(".wav"):
        return "example_output"
    if name.endswith("vibevoice_tts_german.wav"):
        return "rendered_sample"
    if "/text_examples/" in name and name.endswith(".txt"):
        return "text_example"
    if name.startswith("_hf_vibevoice_samples/voices/") and name.endswith(".wav"):
        return "vibevoice"
    if name.endswith(".wav") and "/" not in name:
        return "duplicate_wav"
    if "/streaming_model/" in name and name.endswith(".pt"):
        return "livevoice"
    return "ignore"


def _scan(args) -> int:
    from gen_audio.voices.catalog import library_root

    root = library_root(args.library)
    if root is None:
        raise VoiceError("voice library is unset; pass --library or set GENAID_VOICES_DIR")
    counts: dict[str, int] = {}
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        kind = classify_library_file(path.relative_to(root).as_posix())
        counts[kind] = counts.get(kind, 0) + 1
    sys.stdout.write(json.dumps(dict(sorted(counts.items())), indent=2) + "\n")
    return 0


def _rebuild(args) -> int:
    from gen_audio.voices.catalog import library_root, load_catalog

    root = library_root(args.library)
    if root is None:
        raise VoiceError("voice library is unset; pass --library or set GENAID_VOICES_DIR")
    found: dict[str, list[str]] = {"vibevoice": [], "livevoice": []}
    counts: dict[str, int] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(root).as_posix()
        kind = classify_library_file(relative)
        counts[kind] = counts.get(kind, 0) + 1
        if kind in found:
            found[kind].append(relative)
    if not args.check:
        sys.stdout.write(json.dumps({"counts": dict(sorted(counts.items())), "voices": found}, indent=2) + "\n")
        return 0
    catalog = load_catalog(args.catalog)
    expected: dict[str, list[str]] = {"vibevoice": [], "livevoice": []}
    for voice in catalog.voices:
        if voice.framework in expected:
            expected[voice.framework].extend(asset.path for asset in voice.assets)
    mismatches = []
    for kind in ("vibevoice", "livevoice"):
        if sorted(found[kind]) != sorted(expected[kind]):
            mismatches.append(kind)
    if mismatches:
        raise VoiceError(
            "library scan does not match the catalog for " + ", ".join(mismatches)
        )
    print(
        f"ok rebuild {len(found['vibevoice'])} vibevoice, {len(found['livevoice'])} livevoice ({catalog.id})"
    )
    return 0


def _validate_cast(args) -> int:
    cast = load_cast_map(args.cast_map)
    catalog = load_catalog(args.catalog) if args.catalog is not None else None
    validate_cast(cast, catalog)
    print(f"ok cast {cast.engine} speakers={len(cast.speakers)} characters={len(cast.characters)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
