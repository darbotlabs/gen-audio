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
"""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

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
    and drop machine-specific ``absWav`` paths (``wav`` is the repo-relative one).

    Returns the clip ids whose entry changed. Writes only when something did.
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
    return changed


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
