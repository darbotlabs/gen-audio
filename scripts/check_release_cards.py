"""Fail a release viewport if a shipped card carries a stub label."""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from gen_audio.release_gate import label_hits


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if len(args) != 1:
        print("usage: check_release_cards.py <viewport.json>", file=sys.stderr)
        return 2
    document = json.loads(Path(args[0]).read_text(encoding="utf-8"))
    hits = label_hits(document)
    if hits:
        print("release viewport has stub labels:")
        for hit in hits:
            print(f"  {hit}")
        return 1
    print(f"release viewport ok ({len(document.get('cards', []))} cards)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
