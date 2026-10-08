"""Content-addressed asset objects for pipeline outputs.

Each object names a file this process wrote, its sha256, and the asset uids
it was derived from. synthesizedSpeech stays a separate claim on the WAV.
"""

from __future__ import annotations

import hashlib
from pathlib import Path


def sha256_file(path: Path | str) -> str:
    """Return the lowercase hex sha256 of a file."""
    digest = hashlib.sha256()
    with Path(path).open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def asset_object(
    path: Path | str,
    *,
    kind: str,
    derived_from: list[str],
    duration_s: float | None = None,
) -> dict:
    """Build one asset object. ``uid`` is the sha256 prefix so it is stable."""
    file_path = Path(path)
    digest = sha256_file(file_path)
    payload: dict = {
        "uid": f"asset-{digest[:16]}",
        "sha256": digest,
        "kind": kind,
        "derived_from": list(derived_from),
        "bytes": file_path.stat().st_size,
        "name": file_path.name,
    }
    if duration_s is not None:
        payload["duration_s"] = float(duration_s)
    return payload
