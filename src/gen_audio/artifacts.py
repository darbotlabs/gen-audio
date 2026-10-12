"""Copy finished files out of scratch directories.

Ray workers and other short-lived jobs often write under a temporary
directory that disappears with the process. Set ``GEN_AUDIO_ARTIFACT_DIR``
(or pass a destination) and copy the WAV, PNG, and JSON you mean to keep.
"""

from __future__ import annotations

import os
import shutil
from contextlib import contextmanager
from pathlib import Path
from uuid import uuid4


def publication_path(path: Path | str) -> Path:
    """Resolve the parent, not a possibly symlinked destination leaf."""
    path = Path(path).absolute()
    return path.parent.resolve() / path.name


def check_destinations(paths: list[Path], *, replace: bool = False) -> list[Path]:
    destinations = [publication_path(path) for path in paths]
    if not destinations or len(set(destinations)) != len(destinations):
        raise ValueError("artifact destinations must be non-empty and distinct")
    if replace and len(destinations) != 1:
        raise ValueError("replacement is only supported for a single artifact")
    for path in destinations:
        if path.is_symlink() or (path.exists() and (not replace or not path.is_file())):
            raise FileExistsError(f"artifact already exists: {path}")
    return destinations


@contextmanager
def staged_files(paths: list[Path], *, replace: bool = False):
    """Stage beside each destination, fsync, then publish complete files.

    No-clobber publication uses hard links, not an existence check followed by
    replace. Filesystems without hard-link support fail closed. Multi-file
    publication rolls back on failure; it is not a filesystem transaction.
    Put manifests last: their appearance marks a completed WAV publication.
    """
    destinations = check_destinations(paths, replace=replace)
    staged: list[Path] = []
    committed: list[tuple[Path, Path]] = []
    complete = False
    try:
        for destination in destinations:
            destination.parent.mkdir(parents=True, exist_ok=True)
            stage = destination.with_name(f".{destination.name}.{uuid4().hex}.partial")
            with stage.open("xb"):
                pass
            staged.append(stage)
        yield staged
        for stage in staged:
            with stage.open("r+b") as handle:
                os.fsync(handle.fileno())
        for stage, destination in zip(staged, destinations, strict=True):
            if replace:
                os.replace(stage, destination)
            else:
                os.link(stage, destination)
                committed.append((stage, destination))
        complete = True
    finally:
        if not complete:
            for stage, destination in reversed(committed):
                # Never delete a concurrent writer's replacement.
                try:
                    if destination.samefile(stage):
                        destination.unlink()
                except FileNotFoundError:
                    pass
        for stage in staged:
            stage.unlink(missing_ok=True)


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
    from gen_audio.guards import RequestLimitError, max_audio_file_bytes
    if source.stat().st_size > max_audio_file_bytes():
        raise RequestLimitError("artifact exceeds file byte limit")
    root = resolve_artifact_dir(dest_dir)
    if root is None:
        return source
    destination = root / source.name
    if destination.resolve() == source.resolve():
        return source
    with staged_files([destination]) as staged:
        shutil.copyfile(source, staged[0])
    return destination
