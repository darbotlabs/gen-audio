"""Keep the Library manifest's cube blocks in step with the cube JSON.

``apps/desktop/public/library/manifest.json`` carries a short mirror of each
four-layer cube (``gen_audio.cube_layers``): revision, inv_hdr, geometry and a
one-line note. Those values used to be copied by hand and drifted (misaki said
cube_revision 2 after the cube JSON moved to 3). :func:`sync_manifest` rewrites
the mirror from the cube JSON; ``scripts/cube_revision.py manifest`` runs it and
``scripts/test.ps1`` (Regen) fails when the committed manifest differs.

Standard library only, so it runs where numpy is not installed (CI's Windows
scripts job). Cubes without ``cube_revision`` come from the retired generator
and are left exactly as shipped.
"""

from __future__ import annotations

import json
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
    """Rewrite the cube mirror of every clip whose cube JSON has cube_revision.

    Returns the clip ids whose block changed. Writes only when something did.
    """
    manifest_path = Path(manifest_path)
    library = Path(library_dir) if library_dir is not None else manifest_path.parent
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    changed: list[str] = []
    for clip in manifest.get("clips", []):
        block = clip.get("cube")
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
