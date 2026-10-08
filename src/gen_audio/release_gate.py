"""Scan a release viewport for stub labels."""

from __future__ import annotations

import re

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
