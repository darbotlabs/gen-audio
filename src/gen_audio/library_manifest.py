"""Keep the Library manifest's cube blocks in step with the cube JSON.

``apps/desktop/public/library/manifest.json`` carries a short mirror of each
four-layer cube (``gen_audio.cube_layers``): revision, inv_hdr, geometry and a
one-line note. Those values used to be copied by hand and drifted (misaki said
cube_revision 2 after the cube JSON moved to 3). :func:`sync_manifest` rewrites
the mirror from the cube JSON; ``scripts/cube_revision.py manifest`` runs it and
``scripts/test.ps1`` (Regen) fails when the committed manifest differs.

A ready clip with no cube block gets one when a four-layer cube JSON in the
library names its WAV (``wavUrl``), so a new cube is wired by regenerating,
not by editing the manifest.

A cube block may also carry ``generator_commit``: the commit whose generator
file hashes to the cube's ``provenance.generator_sha256``. It is information
only (build_assets copies it into the cube envelope's unhashed provenance; the
uid never sees it). :func:`sync_manifest` keeps it as it is;
:func:`record_generator_commits` (``cube_revision.py manifest
--record-generator-commit``) fills it from git history once the regen is
committed. Standard library only, so it runs where numpy is
not installed (CI's Windows scripts job). Cubes without ``cube_revision`` come
from the retired generator and are left exactly as shipped.

Synth sidecars (``*.synth.json`` next to the WAVs) ship in dist too. Their
absolute paths are provenance (which model, script and cast map made the
audio), so :func:`sync_manifest` keeps them but rewrites each one relative to
the repo (:func:`relativize_paths`). The audio bytes and every uid stay as
they are: no asset hashes a sidecar.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

# An absolute filesystem path inside a string: a drive letter (C:\ or C:/), a
# UNC share (\\host\share), or a "/" that starts a path rather than
# continuing a URL ("https://"), a relative path ("../x", "a/b") or a word.
_DRIVE = re.compile(r"(?<![A-Za-z0-9])[A-Za-z]:[\\/][^\s\"'<>|]*")
_UNC = re.compile(r"(?<!\\)\\\\[\w.$-][^\s\"'<>|]*")
_POSIX = re.compile(r"(?<![\w.:/~\\-])/[\w.~-][^\s\"'<>]*")


def _relative(parts: list[str]) -> str:
    parts = [part for part in parts if part]
    return "/".join([".."] + parts)


def relativize_paths(text: str, web_root: frozenset[str] = frozenset()) -> str:
    """Rewrite every absolute path in ``text`` relative to the repo root.

    The repo's parent directory holds the sibling checkouts and work dirs
    (``/workspace`` on the box, ``D:\\`` on SMAX, a home directory
    elsewhere), so a path ``<that parent>/<rest>`` becomes ``../<rest>``:
    ``/workspace/genaid-podcast-compare/models/x`` -> ``../genaid-podcast-compare/models/x``,
    ``D:\\gen-audio\\artifacts\\x.wav`` -> ``../gen-audio/artifacts/x.wav``,
    ``/home/<user>/<rest>`` and ``/Users/<user>/<rest>`` -> ``../<rest>``,
    ``\\\\host\\share\\<rest>`` -> ``../share/<rest>``. Separators become "/".
    A "/" path whose first segment is in ``web_root`` (the app's public dir:
    ``/library/x.wav``) is a URL the app serves, not a machine path, and stays.
    Deterministic and idempotent: the output has no absolute path left.
    """

    def drive(match: re.Match[str]) -> str:
        return _relative(re.split(r"[\\/]", match.group(0)[3:]))

    def unc(match: re.Match[str]) -> str:
        return _relative(re.split(r"[\\/]", match.group(0)[2:])[1:])

    def posix(match: re.Match[str]) -> str:
        parts = match.group(0)[1:].split("/")
        if parts[0] in web_root:
            return match.group(0)
        skip = 2 if parts[0] in ("home", "Users") and len(parts) > 1 else 1
        return _relative(parts[skip:])

    text = _UNC.sub(unc, text)
    text = _DRIVE.sub(drive, text)
    return _POSIX.sub(posix, text)


def _relativize_value(value, web_root: frozenset[str]):
    if isinstance(value, str):
        return relativize_paths(value, web_root)
    if isinstance(value, list):
        return [_relativize_value(item, web_root) for item in value]
    if isinstance(value, dict):
        return {key: _relativize_value(item, web_root) for key, item in value.items()}
    return value


def sync_synth_sidecars(library: Path) -> list[str]:
    """Rewrite absolute paths in every ``*.synth.json`` under ``library``
    repo-relative (:func:`relativize_paths`). Returns the file names it
    rewrote; a sidecar with no absolute path is left byte for byte."""
    web_root = frozenset(entry.name for entry in library.parent.iterdir())
    changed: list[str] = []
    for path in sorted(library.glob("*.synth.json")):
        doc = json.loads(path.read_text(encoding="utf-8"))
        relative = _relativize_value(doc, web_root)
        if relative != doc:
            path.write_text(render(relative), encoding="utf-8")
            changed.append(path.name)
    return changed


def repo_relative(path: Path | str, repo_root: Path | str) -> str:
    """``path`` relative to ``repo_root`` with "/" separators (``../`` for a
    sibling checkout). Across drives, where no relative path exists, the
    :func:`relativize_paths` rule applies. Never returns an absolute path."""
    absolute = os.path.abspath(os.fspath(path))
    try:
        relative = os.path.relpath(absolute, os.path.abspath(os.fspath(repo_root)))
    except ValueError:
        return relativize_paths(absolute)
    return relative.replace(os.sep, "/")

# Mirror keys, in the order they appear in a manifest cube block.
MIRRORED = (
    "inv_hdr",
    "cube_revision",
    "duration_s",
    "cube_shape_f_t",
    "bin_seconds",
    "cube_covers_s",
    "n_points",
    "n_points_source",
)


def cube_note(doc: dict) -> str:
    """One-line human summary of a four-layer cube, derived from its JSON."""
    sf, st = doc.get("downsample_sf_st", [1, 1])
    khz = doc["sample_rate"] / 1000
    return (
        f"Inverse-HDR cube rev {doc['cube_revision']} over the full {doc['duration_s']:.6g} s WAV "
        f"(stft {doc['n_fft']}/{doc['hop']} @ {khz:g} kHz, downsample {sf}x{st}; "
        "signal/tonality/confidence/quality). "
        f"Scrub maps by seconds: bin = floor(t / {doc['bin_seconds']:.6g} s)."
    )


def cube_mirror(doc: dict) -> dict:
    """The manifest fields a four-layer cube JSON determines."""
    mirror = {key: doc[key] for key in MIRRORED}
    mirror["note"] = cube_note(doc)
    return mirror


def render(manifest: dict) -> str:
    """Byte format of manifest.json (ASCII escapes, 2-space indent, trailing newline)."""
    return json.dumps(manifest, indent=2) + "\n"


def sync_manifest(manifest_path: Path | str, library_dir: Path | str | None = None) -> list[str]:
    """Rewrite the cube mirror of every clip whose cube JSON has cube_revision,
    drop machine-specific ``absWav`` paths (``wav`` is the repo-relative one),
    and rewrite the synth sidecars' absolute paths repo-relative
    (:func:`sync_synth_sidecars`).

    Returns the clip ids whose entry changed, then the sidecar file names that
    were rewritten. Writes only when something did.
    """
    manifest_path = Path(manifest_path)
    library = Path(library_dir) if library_dir is not None else manifest_path.parent
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    by_wav: dict[str, tuple[str, dict]] = {}
    for path in sorted(library.glob("*_cube3d.json")):
        doc = json.loads(path.read_text(encoding="utf-8"))
        if "cube_revision" in doc and isinstance(doc.get("wavUrl"), str):
            by_wav[doc["wavUrl"]] = (path.name, doc)
    changed: list[str] = []
    for clip in manifest.get("clips", []):
        # F: the manifest ships in dist, so no machine paths. ``wav`` already
        # holds the repo-relative path; an absolute ``absWav`` (``D:\\...``,
        # ``/home/...``) is dropped here, so the generator removes it.
        if "absWav" in clip:
            del clip["absWav"]
            changed.append(str(clip.get("id")))
        block = clip.get("cube")
        found = by_wav.get(clip.get("wavUrl")) if clip.get("status") == "ok" else None
        if not isinstance(block, dict) and found is not None:
            name, doc = found
            block = clip["cube"] = {"pngUrl": doc["pngUrl"], "jsonUrl": f"/library/{name}"}
        if not isinstance(block, dict) or not isinstance(block.get("jsonUrl"), str):
            continue
        name = block["jsonUrl"].rsplit("/", 1)[-1]
        doc = json.loads((library / name).read_text(encoding="utf-8"))
        if "cube_revision" not in doc:
            continue
        before = dict(block)
        block.update(cube_mirror(doc))
        if block != before:
            changed.append(str(clip.get("id")))
    text = render(manifest)
    if text != manifest_path.read_text(encoding="utf-8"):
        manifest_path.write_text(text, encoding="utf-8")
    return changed + sync_synth_sidecars(library)


def normalized_sha256(data: bytes) -> str:
    """sha256 of source bytes with CRLF normalized to LF (generator identity)."""
    return hashlib.sha256(data.replace(b"\r\n", b"\n")).hexdigest()


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, check=False)


def generator_commit_for(repo: Path | str, generator: str, sha256: str) -> str | None:
    """Newest commit that changed ``generator`` to bytes hashing to ``sha256``."""
    log = _git(Path(repo), "log", "--format=%H", "--", generator)
    if log.returncode != 0:
        return None
    for commit in log.stdout.decode().split():
        blob = _git(Path(repo), "show", f"{commit}:{generator}")
        if blob.returncode == 0 and normalized_sha256(blob.stdout) == sha256:
            return commit
    return None


def commit_holds_generator(repo: Path | str, commit: str, generator: str, sha256: str) -> bool | None:
    """True/False if ``commit`` is in this clone; None when it is not (shallow, rebased)."""
    blob = _git(Path(repo), "show", f"{commit}:{generator}")
    if blob.returncode != 0:
        return None
    return normalized_sha256(blob.stdout) == sha256


def record_generator_commits(manifest_path: Path | str, repo: Path | str, library_dir: Path | str | None = None) -> list[str]:
    """Set each cube block's ``generator_commit`` from history (information only).

    A recorded commit that still holds the cube's generator bytes is kept.
    Otherwise the newest commit that does is written; when none does yet (the
    regen is not committed), the key is left out. Returns the changed clip ids.
    """
    manifest_path = Path(manifest_path)
    library = Path(library_dir) if library_dir is not None else manifest_path.parent
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    changed: list[str] = []
    for clip in manifest.get("clips", []):
        block = clip.get("cube")
        if not isinstance(block, dict) or not isinstance(block.get("jsonUrl"), str):
            continue
        provenance = json.loads((library / block["jsonUrl"].rsplit("/", 1)[-1]).read_text(encoding="utf-8")).get("provenance") or {}
        generator, sha = provenance.get("generator"), provenance.get("generator_sha256")
        if not isinstance(generator, str) or not isinstance(sha, str):
            continue
        current = block.get("generator_commit")
        if isinstance(current, str) and commit_holds_generator(repo, current, generator, sha):
            continue
        found = generator_commit_for(repo, generator, sha)
        if found is None:
            block.pop("generator_commit", None)
        else:
            block["generator_commit"] = found
        if block.get("generator_commit") != current:
            changed.append(str(clip.get("id")))
    text = render(manifest)
    if text != manifest_path.read_text(encoding="utf-8"):
        manifest_path.write_text(text, encoding="utf-8")
    return changed
