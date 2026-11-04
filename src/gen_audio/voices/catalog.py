"""Load a voice catalog and resolve asset paths against the voice library."""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path

from gen_audio.voices.errors import VoiceError
from gen_audio.voices.fields import check_rel_path, optional_str, reject_unknown, require_object, require_str
from gen_audio.voices.model import SCHEMA_VERSION, G2pConfig, VoiceDescriptor, load_g2p, load_voice, read_json

LIBRARY_ENV = "GENAID_VOICES_DIR"
_SOURCES = {"naming-conventions", "library-scan", "handwritten"}
_CATALOG_ID = re.compile(r"^[a-z][a-z0-9_-]{0,63}\Z")
_MANIFEST_SOURCE = re.compile(r"^manifest:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[0-9a-f]{7,40}\Z")
_CATALOG_KEYS = {"schema_version", "id", "catalog_source", "note", "library", "voices", "g2p", "excluded"}
_LIBRARY_KEYS = {"env", "path"}
_EXCLUDED_KEYS = {"path", "kind", "sha256", "size", "bytes", "note"}
_EXCLUDED_KINDS = {"example_output", "text_example", "rendered_sample"}
_SHA256_RE = re.compile(r"^[0-9a-f]{64}\Z")
_LFS_SIZE_RE = re.compile(r"^[0-9]+(?:\.[0-9]+)? (?:KB|MB)\Z")


@dataclass(frozen=True)
class ExcludedSample:
    """A library file that is not a voice."""

    path: str
    kind: str
    sha256: str | None = None
    size: str | None = None
    byte_count: int | None = None
    note: str | None = None


@dataclass(frozen=True)
class Catalog:
    schema_version: str
    id: str
    catalog_source: str
    note: str | None
    library_env: str
    library_path: str | None
    voices: tuple[VoiceDescriptor, ...]
    g2p: tuple[G2pConfig, ...]
    path: Path | None = None
    excluded: tuple[ExcludedSample, ...] = ()

    def voice(self, voice_id: str) -> VoiceDescriptor:
        for item in self.voices:
            if item.id == voice_id:
                return item
        known = ", ".join(item.id for item in self.voices)
        raise VoiceError(f"unknown voice {voice_id}; known: {known}")

    def g2p_config(self, g2p_id: str) -> G2pConfig:
        for item in self.g2p:
            if item.id == g2p_id:
                return item
        known = ", ".join(item.id for item in self.g2p)
        raise VoiceError(f"unknown g2p config {g2p_id}; known: {known}")


def load_catalog(path: Path | str) -> Catalog:
    """Load a catalog and every voice and G2P document it names."""
    source = Path(path)
    payload = read_json(source)
    reject_unknown(payload, _CATALOG_KEYS, f"catalog {source.name}")
    version = require_str(payload, "schema_version", label="catalog", min_len=5, max_len=16)
    if version != SCHEMA_VERSION:
        raise VoiceError(f"catalog schema_version must be {SCHEMA_VERSION}")
    catalog_id = require_str(payload, "id", label="catalog", min_len=1, max_len=64)
    if _CATALOG_ID.fullmatch(catalog_id) is None:
        raise VoiceError("catalog id must be a lowercase slug")
    origin = require_str(payload, "catalog_source", label="catalog", min_len=8, max_len=120)
    if origin not in _SOURCES and _MANIFEST_SOURCE.fullmatch(origin) is None:
        raise VoiceError("catalog_source must name a manifest repo@commit or a handwritten source")
    note = optional_str(payload, "note", label="catalog", min_len=1, max_len=1600)
    library = require_object(payload.get("library"), "catalog library")
    reject_unknown(library, _LIBRARY_KEYS, "catalog library")
    env = require_str(library, "env", label="catalog library", min_len=8, max_len=32)
    if env != LIBRARY_ENV:
        raise VoiceError(f"catalog library env must be {LIBRARY_ENV}")
    library_path = optional_str(library, "path", label="catalog library", min_len=1, max_len=240)
    voices = tuple(_load_entries(source, payload.get("voices"), load_voice, "voices"))
    g2p = tuple(_load_entries(source, payload.get("g2p"), load_g2p, "g2p"))
    _unique(voices, "voice")
    _unique(g2p, "g2p config")
    by_g2p = {item.id: item for item in g2p}
    for voice in voices:
        if voice.g2p is None:
            continue
        config = by_g2p.get(voice.g2p)
        if config is None:
            raise VoiceError(f"{voice.id} references unknown g2p config {voice.g2p}")
        _g2p_agrees(voice, config)
    return Catalog(
        schema_version=version,
        id=catalog_id,
        catalog_source=origin,
        note=note,
        library_env=env,
        library_path=library_path,
        voices=voices,
        g2p=g2p,
        path=source,
        excluded=_excluded(payload.get("excluded")),
    )


def library_root(explicit: Path | str | None = None, *, catalog: Catalog | None = None) -> Path | None:
    """Directory that holds voice assets.

    An explicit path wins, then ``GENAID_VOICES_DIR``, then ``library.path`` on
    the catalog. Unset means the catalog can still be validated; asset files
    are not required.
    """
    if explicit is not None and str(explicit).strip():
        root = Path(explicit)
    else:
        raw = os.environ.get(LIBRARY_ENV, "").strip()
        if raw:
            root = Path(raw)
        elif catalog is not None and catalog.library_path:
            root = Path(catalog.library_path)
        else:
            return None
    if not root.is_dir():
        raise VoiceError(f"voice library is not a directory: {root}")
    return root.resolve()


def resolve_asset(asset_path: str, library: Path) -> Path:
    """Absolute path of one asset. Rejects paths that leave ``library``."""
    root = library.resolve()
    relative = check_rel_path(asset_path, label="asset path")
    candidate = (root / relative).resolve()
    if candidate != root and root not in candidate.parents:
        raise VoiceError(f"asset path escapes the voice library: {relative}")
    return candidate


def missing_assets(catalog: Catalog, library: Path) -> list[str]:
    """Asset paths in ``catalog`` that are not files under ``library``."""
    missing: list[str] = []
    for voice in catalog.voices:
        for asset in voice.assets:
            if not resolve_asset(asset.path, library).is_file():
                missing.append(asset.path)
    for config in catalog.g2p:
        if config.lexicon and not resolve_asset(config.lexicon, library).is_file():
            missing.append(config.lexicon)
    return missing


def _load_entries(catalog_path: Path, value, loader, label: str) -> list:
    if not isinstance(value, list):
        raise VoiceError(f"catalog {label} must be an array")
    if label == "voices" and not value:
        raise VoiceError("catalog voices must name at least one descriptor")
    if len(value) > (512 if label == "voices" else 64):
        raise VoiceError(f"catalog {label} is too long")
    root = catalog_path.parent.resolve()
    loaded = []
    seen: set[str] = set()
    for raw in value:
        if not isinstance(raw, str):
            raise VoiceError(f"catalog {label} entries must be relative JSON paths")
        relative = check_rel_path(raw, label=f"catalog {label} entry")
        if not relative.endswith(".json"):
            raise VoiceError(f"catalog {label} entry must be a JSON file")
        if relative in seen:
            raise VoiceError(f"catalog repeats {relative}")
        seen.add(relative)
        candidate = (root / relative).resolve()
        if root not in candidate.parents:
            raise VoiceError(f"catalog entry escapes the catalog directory: {relative}")
        if not candidate.is_file():
            raise VoiceError(f"catalog entry not found: {relative}")
        loaded.append(loader(candidate))
    return loaded


def _excluded(value: object) -> tuple[ExcludedSample, ...]:
    if value is None:
        return ()
    if not isinstance(value, list) or len(value) > 64:
        raise VoiceError("catalog excluded must be an array of at most 64")
    found: list[ExcludedSample] = []
    seen: set[str] = set()
    for index, raw in enumerate(value):
        item = require_object(raw, f"excluded {index}")
        reject_unknown(item, _EXCLUDED_KEYS, f"excluded {index}")
        rel = check_rel_path(
            require_str(item, "path", label=f"excluded {index}", min_len=1, max_len=220),
            label=f"excluded {index} path",
        )
        kind = require_str(item, "kind", label=f"excluded {index}", min_len=8, max_len=20)
        if kind not in _EXCLUDED_KINDS:
            raise VoiceError(f"excluded {index} kind is not a sample kind")
        if rel in seen:
            raise VoiceError(f"catalog excluded repeats {rel}")
        seen.add(rel)
        sha256 = optional_str(item, "sha256", label=f"excluded {index}", min_len=64, max_len=64)
        if sha256 is not None and _SHA256_RE.fullmatch(sha256) is None:
            raise VoiceError(f"excluded {index} sha256 must be 64 lowercase hex characters")
        size = optional_str(item, "size", label=f"excluded {index}", min_len=4, max_len=16)
        if size is not None and _LFS_SIZE_RE.fullmatch(size) is None:
            raise VoiceError(f"excluded {index} size must be a git lfs display size")
        byte_count = item.get("bytes")
        if byte_count is not None and (isinstance(byte_count, bool) or not isinstance(byte_count, int) or byte_count < 0):
            raise VoiceError(f"excluded {index} bytes must be a non-negative integer")
        note = optional_str(item, "note", label=f"excluded {index}", min_len=1, max_len=200)
        found.append(ExcludedSample(rel, kind, sha256, size, byte_count, note))
    return tuple(found)


def _unique(items, label: str) -> None:
    seen: set[str] = set()
    for item in items:
        if item.id in seen:
            raise VoiceError(f"duplicate {label} id {item.id}")
        seen.add(item.id)


def _g2p_agrees(voice: VoiceDescriptor, config: G2pConfig) -> None:
    lang = voice.language
    if lang in {"en-us", "en-gb"}:
        if config.backend != "misaki-en" or config.variant != lang:
            raise VoiceError(f"{voice.id} must use the Misaki {lang} config")
        return
    if lang == "ja" and config.backend == "misaki-ja":
        return
    if lang == "zh" and config.backend == "misaki-zh":
        return
    if lang in {"en-us", "en-gb", "ja", "zh"}:
        raise VoiceError(f"{voice.id} g2p {config.id} does not match language {lang}")
    if config.backend != "espeak":
        raise VoiceError(f"{voice.id} language {lang} needs an espeak G2P config")
    espeak = config.espeak_language or ""
    if espeak != lang and not espeak.startswith(lang + "-") and not lang.startswith(espeak + "-"):
        raise VoiceError(f"{voice.id} espeak language does not match {lang}")
