"""Scan the release viewport and the built desktop bundle for stub labels."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from gen_audio.release_gate import dist_hits, label_hits, structural_hits


def _repo() -> Path:
    return Path(__file__).resolve().parents[3]


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Fail when a release card or the built bundle ships a stub label.")
    parser.add_argument("viewport", nargs="?", type=Path, help="viewport JSON (default: schemas/examples/viewport.release.json)")
    parser.add_argument("--dist", type=Path, help="built desktop dist directory (default: apps/desktop/dist)")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = _repo()
    viewport = args.viewport or (root / "schemas" / "examples" / "viewport.release.json")
    dist = args.dist or (root / "apps" / "desktop" / "dist")
    import json

    document = json.loads(viewport.read_text(encoding="utf-8"))
    hits = label_hits(document) + structural_hits(document) + dist_hits(dist)
    if hits:
        print("release gate failed:")
        for hit in hits:
            print(f"  {hit}")
        return 1
    print(f"release gate ok ({len(document.get('cards', []))} cards, dist {dist})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
