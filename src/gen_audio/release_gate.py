"""Scan a release viewport and a built bundle for stub labels."""

from __future__ import annotations

import json
import re
from pathlib import Path

_FORBIDDEN = re.compile(
    r"\b(?:fixture|placeholder|reserved|preview)\b|\bref[- ]only\b|\bsample\b",
    re.IGNORECASE,
)
_SKIP_KEYS = {"sample_rate", "samples", "n_samples"}


def label_hits(document: dict) -> list[str]:
    """Return ``card id: match`` rows for stub labels in shipped cards."""
    hits: list[str] = []
    cards = document.get("cards")
    if not isinstance(cards, list):
        return ["document has no cards array"]
    for card in cards:
        if not isinstance(card, dict):
            hits.append("card is not an object")
            continue
        card_id = str(card.get("id", "?"))
        found = sorted({match.group(0).lower() for match in _FORBIDDEN.finditer(_card_blob(card))})
        for label in found:
            hits.append(f"{card_id}: {label}")
    hits.extend(fixture_honesty_hits(document))
    return hits


def fixture_honesty_hits(document: dict) -> list[str]:
    """A release document fails when any view's honesty state is fixture."""
    hits: list[str] = []
    views = document.get("views")
    if not isinstance(views, list):
        return hits
    for view in views:
        if not isinstance(view, dict):
            hits.append("view is not an object")
            continue
        snapshot = view.get("snapshot")
        if isinstance(snapshot, dict) and "honesty" in snapshot:
            state = _honesty_state(snapshot.get("honesty"))
        else:
            state = _honesty_state(view.get("honesty"))
        if state == "fixture":
            hits.append(f"{view.get('id', '?')}: fixture")
    return hits


def card_badge(card: dict) -> str:
    """Mirror the desktop badge: a fixture cube stays Fixture, never Library or Real."""
    kind = str(card.get("kind", ""))
    body = card.get("body") if isinstance(card.get("body"), dict) else {}
    if kind == "Cube3D":
        if body.get("datasetBound") == "library" or body.get("source") == "library-clip":
            return "library"
        return "fixture"
    if kind == "SpectrogramPanel":
        return "fixture"
    if kind == "LibraryClip" and body.get("synthesizedSpeech") is True:
        return "real"
    return "other"


def _honesty_state(value) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        state = value.get("state")
        if isinstance(state, str):
            return state
    return ""


def _card_blob(card: dict) -> str:
    parts: list[str] = [str(card.get("title", "")), str(card.get("kind", ""))]
    _walk(card.get("body", {}), parts)
    text = "\n".join(parts)
    return _NOT_A_FIXTURE.sub("", text)


_NOT_A_FIXTURE = re.compile(r"\bnot a fixture\b", re.IGNORECASE)


def _walk(value, parts: list[str]) -> None:
    if isinstance(value, str):
        parts.append(value)
    elif isinstance(value, dict):
        for key, item in value.items():
            if key in _SKIP_KEYS:
                continue
            _walk(item, parts)
    elif isinstance(value, list):
        for item in value:
            _walk(item, parts)


_PHRASES = re.compile(r"not remeasured|stand-in|\bstand in\b", re.IGNORECASE)
_HOST = re.compile(r"<[^>]+>")
_DIST_NEEDLES = (
    "cube-fixture",
    "spec-fixture",
    "cast-sample",
    "Ref only",
    "This layer is reserved",
    "Sample cast",
    "sample script",
    "preview, not speech",
    "not remeasured",
    "stand-in",
    "Load labeled example",
    '"state":"fixture"',
    '"honesty":"fixture"',
    "honesty=fixture",
)


def structural_hits(document: dict) -> list[str]:
    """Structural release rules: measured compare, probed serve, no stub phrases."""
    hits: list[str] = []
    cards = document.get("cards")
    if not isinstance(cards, list):
        return ["document has no cards array"]
    for card in cards:
        if not isinstance(card, dict):
            hits.append("card is not an object")
            continue
        card_id = str(card.get("id", "?"))
        kind = str(card.get("kind", ""))
        body = card.get("body") if isinstance(card.get("body"), dict) else {}
        blob = json.dumps(card)
        if _PHRASES.search(blob):
            hits.append(f"{card_id}: stub phrase")
        honesty = body.get("honesty")
        state = _honesty_state(honesty) if honesty is not None else ""
        if state == "fixture" or body.get("honesty") == "fixture":
            hits.append(f"{card_id}: honesty=fixture")
        if kind == "BenchmarkCompare" and body.get("measuredHere") is not True:
            hits.append(f"{card_id}: BenchmarkCompare.measuredHere is not true")
        if kind == "ServeHealth":
            host = str(body.get("host", ""))
            if _HOST.search(host) or not host or host.startswith("<"):
                hits.append(f"{card_id}: ServeHealth host is a placeholder")
            if body.get("probed") is not True:
                hits.append(f"{card_id}: ServeHealth has no probe result")
    hits.extend(fixture_honesty_hits(document))
    return hits


_STUB_CARD_IDS = {
    "serve-node",
    "serve-gateway",
    "cube-fixture",
    "bench-ref",
    "spec-fixture",
    "cast-sample",
}
_DIST_SUFFIXES = {".js", ".html", ".css", ".mjs", ".json"}
_TITLE_KEYS = {"title", "label"}


def catalog_title_hits(document: object, label: str) -> list[str]:
    """Bare stub words in catalog titles and labels. Existing phrase checks stay."""
    hits: list[str] = []

    def walk(value: object, key: str | None = None) -> None:
        if isinstance(value, dict):
            for child_key, item in value.items():
                walk(item, str(child_key))
        elif isinstance(value, list):
            for item in value:
                walk(item, key)
        elif isinstance(value, str) and key in _TITLE_KEYS:
            for match in _FORBIDDEN.finditer(value):
                hits.append(f"{label}: {key} {match.group(0).lower()}")

    walk(document)
    return hits


def _asset_stub_hits(asset: object, label: str) -> list[str]:
    """A catalog envelope fails when it is a fixture, an unprobed serve card, or a stub label."""
    if not isinstance(asset, dict):
        return [f"{label}: asset is not an object"]
    hits: list[str] = []
    legacy = str(asset.get("legacy_id") or "")
    body = asset.get("body") if isinstance(asset.get("body"), dict) else {}
    card_id = str(body.get("id") or legacy or "?")
    kind = str(body.get("kind") or "")
    honesty = asset.get("honesty") if isinstance(asset.get("honesty"), dict) else {}
    if honesty.get("fixture") is True or _honesty_state(body.get("honesty")) == "fixture":
        hits.append(f"{label}: {card_id}: honesty=fixture")
    if card_id in _STUB_CARD_IDS or legacy in _STUB_CARD_IDS:
        hits.append(f"{label}: {card_id}: stub card")
    inner = body.get("body") if isinstance(body.get("body"), dict) else body
    if not isinstance(inner, dict):
        inner = {}
    if kind == "ServeHealth":
        host = str(inner.get("host", ""))
        if _HOST.search(host) or not host or host.startswith("<"):
            hits.append(f"{label}: {card_id}: ServeHealth host is a placeholder")
        if inner.get("probed") is not True:
            hits.append(f"{label}: {card_id}: ServeHealth has no probe result")
    if kind == "BenchmarkCompare" and inner.get("measuredHere") is not True:
        hits.append(f"{label}: {card_id}: BenchmarkCompare.measuredHere is not true")
    if _PHRASES.search(json.dumps(asset)):
        hits.append(f"{label}: {card_id}: stub phrase")
    return hits


def json_document_hits(document: object, label: str) -> list[str]:
    """Structural stub rules for a JSON document that ships inside dist."""
    if not isinstance(document, dict):
        return []
    hits: list[str] = []
    hits.extend(catalog_title_hits(document, label))
    assets = document.get("assets")
    if isinstance(assets, list):
        for asset in assets:
            hits.extend(_asset_stub_hits(asset, label))
        index = document.get("legacy_index")
        if isinstance(index, dict):
            for key in index:
                tail = str(key).split(":", 1)[-1]
                if tail in _STUB_CARD_IDS:
                    hits.append(f"{label}: legacy_index {key}")
        return hits
    if isinstance(document.get("cards"), list):
        hits.extend(structural_hits(document))
    return hits


def dist_hits(dist: Path) -> list[str]:
    """Scan a built desktop bundle, including copied JSON. A missing dist fails the release gate."""
    if not dist.is_dir():
        return [f"dist bundle is missing: {dist}"]
    hits: list[str] = []
    files = [path for path in dist.rglob("*") if path.suffix in _DIST_SUFFIXES]
    if not files:
        return [f"dist bundle has no js/html/css/json: {dist}"]
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        for needle in _DIST_NEEDLES:
            if needle in text:
                hits.append(f"{path.name}: {needle}")
        if "PLACEHOLDER" in text and path.suffix != ".json":
            hits.append(f"{path.name}: PLACEHOLDER")
        if path.suffix != ".json":
            continue
        try:
            document = json.loads(text)
        except json.JSONDecodeError:
            hits.append(f"{path.name}: invalid json")
            continue
        hits.extend(json_document_hits(document, path.name))
    return hits
