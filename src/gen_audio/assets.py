"""Content-addressed asset objects for pipeline outputs.

``uid`` is a v1 ``ga:`` identity for the kinds the schema knows. Auxiliary
files keep a sha256 prefix and are not hashed into an identity.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

from gen_audio.identity import mint, ms_from_frames, round_half_up

_KIND = {
    "wav": "audio_clip",
    "audio_clip": "audio_clip",
    "script": "podcast_script",
    "podcast_script": "podcast_script",
    "spectrogram_2d": "spectrogram_2d",
    "cube": "cube_ihdr",
    "cube_ihdr": "cube_ihdr",
}
_ROLE = {
    "audio_clip": "wav",
    "podcast_script": "script_txt",
    "spectrogram_2d": "spectrogram_png",
    "cube_ihdr": "cube_json",
}


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
    engine: str | None = None,
    sample_rate: int | None = None,
    n_frames: int | None = None,
    fields: dict | None = None,
    media_role: str | None = None,
) -> dict:
    """Build one asset object. Known kinds get a minted ``ga:`` uid."""
    file_path = Path(path)
    digest = sha256_file(file_path)
    schema_kind = _KIND.get(kind)
    parents = [item for item in derived_from if item.startswith("ga:")]
    payload: dict = {
        "sha256": digest,
        "kind": kind,
        "derived_from": list(derived_from),
        "bytes": file_path.stat().st_size,
        "name": file_path.name,
    }
    if duration_s is not None:
        payload["duration_s"] = float(duration_s)
    if schema_kind is None:
        payload["uid"] = f"asset-{digest[:16]}"
        return payload
    identity = dict(fields or {})
    if schema_kind == "audio_clip":
        identity.setdefault("engine", engine or "unknown")
        identity.setdefault("sample_rate_hz", int(sample_rate or 0))
        if "duration_ms" not in identity:
            if n_frames is not None and sample_rate:
                identity["duration_ms"] = ms_from_frames(int(n_frames), int(sample_rate))
            elif duration_s is not None:
                identity["duration_ms"] = round_half_up(float(duration_s) * 1000.0)
            else:
                identity["duration_ms"] = 0
    elif schema_kind == "podcast_script" and "n_words" not in identity:
        text = file_path.read_text(encoding="utf-8")
        turns = [line for line in text.splitlines() if line.strip()]
        identity.setdefault("format", "speaker")
        identity.setdefault("n_turns", len(turns))
        identity.setdefault("n_words", len(text.split()))
    role = media_role or _ROLE[schema_kind]
    payload["uid"] = mint(schema_kind, identity, [{"role": role, "sha256": digest}], parents)
    payload["fields"] = identity
    return payload
