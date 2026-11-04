"""CLI: list frameworks, validate a voice catalog, validate a cast map."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from gen_audio.cast import load_cast_map
from gen_audio.voices.catalog import library_root, load_catalog, missing_assets
from gen_audio.voices.errors import VoiceError
from gen_audio.voices.model import repo_root
from gen_audio.voices.registry import list_frameworks
from gen_audio.voices.resolve import validate_cast

DEFAULT_CATALOG = repo_root() / "voices" / "catalog.example.json"


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
    validate.set_defaults(func=_validate)

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
    if args.require_assets:
        root = library_root(args.library, catalog=catalog)
        if root is None:
            raise VoiceError("voice library is unset; pass --library or set GENAID_VOICES_DIR")
        missing = missing_assets(catalog, root)
        if missing:
            raise VoiceError("missing voice assets: " + ", ".join(missing))
    print(f"ok {len(catalog.voices)} voices, {len(catalog.g2p)} g2p ({catalog.id})")
    return 0


def _validate_cast(args) -> int:
    cast = load_cast_map(args.cast_map)
    catalog = load_catalog(args.catalog) if args.catalog is not None else None
    validate_cast(cast, catalog)
    print(f"ok cast {cast.engine} speakers={len(cast.speakers)} characters={len(cast.characters)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
