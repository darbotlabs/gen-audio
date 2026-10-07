"""Copy finished files out of scratch directories.

Ray workers and other short-lived jobs often write under a temporary
directory that disappears with the process. Set ``GEN_AUDIO_ARTIFACT_DIR``
(or pass a destination) and copy the WAV, PNG, and JSON you mean to keep.
"""

from __future__ import annotations

import os
import shutil
from pathlib import Path


def resolve_artifact_dir(dest_dir: str | Path | None = None) -> Path | None:
    """Return the durable directory, creating it, or None when unset."""
    raw = dest_dir if dest_dir not in (None, "") else os.environ.get("GEN_AUDIO_ARTIFACT_DIR")
    if raw in (None, ""):
        return None
    path = Path(raw)
    path.mkdir(parents=True, exist_ok=True)
    return path


def publish_copy(src: Path | str, dest_dir: str | Path | None = None) -> Path:
    """Copy ``src`` into the artifact directory.

    When no directory is configured, return ``src`` unchanged. A copy next
    to itself is skipped.
    """
    source = Path(src)
    if not source.is_file():
        raise FileNotFoundError(f"artifact source does not exist: {source}")
    root = resolve_artifact_dir(dest_dir)
    if root is None:
        return source
    destination = root / source.name
    if destination.resolve() == source.resolve():
        return source
    shutil.copy2(source, destination)
    return destination
