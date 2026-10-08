"""Release-gate labels. A stub card fails; a measured card passes."""

from __future__ import annotations

import json
from pathlib import Path

from gen_audio.release_gate import dist_hits, label_hits, structural_hits


def _card(**body):
    return {
        "version": "1.0",
        "title": "Release",
        "cards": [
            {
                "id": "bench-measured",
                "kind": "BenchmarkCompare",
                "title": "Compare",
                "body": body,
            }
        ],
    }


def test_stub_card_fails_the_release_gate():
    document = _card(
        measuredHere=False,
        sourceNote="Ref-only placeholder. This is a fixture preview.",
        rows=[],
    )
    hits = label_hits(document)
    blob = " ".join(hits).lower()
    assert "fixture" in blob
    assert "placeholder" in blob
    assert "preview" in blob
    assert "ref-only" in blob or "ref only" in blob


def test_sample_rate_is_not_a_stub_label():
    document = {
        "version": "1.0",
        "title": "Release",
        "cards": [
            {
                "id": "lib-proof",
                "kind": "LibraryClip",
                "title": "Proof clip",
                "body": {
                    "engineId": "kokoro_onnx",
                    "status": "ok",
                    "synthesizedSpeech": True,
                    "summary": "Measured clip.",
                    "sample_rate": 24000,
                    "duration_s": 6.0,
                    "sha256": "a" * 64,
                    "wavUrl": "/library/proof/fitted.wav",
                },
            }
        ],
    }
    assert label_hits(document) == []


def test_committed_release_viewport_has_no_stub_labels():
    """The shipped deck is the example minus every dev-fixture and stand-in card."""
    document = json.loads(Path("schemas/examples/viewport.release.json").read_text(encoding="utf-8"))
    example = json.loads(Path("schemas/examples/viewport.example.json").read_text(encoding="utf-8"))
    dev = {"bench-ref", "cube-fixture", "spec-fixture", "cast-sample", "serve-node", "serve-gateway"}
    expected = [card for card in example["cards"] if card["id"] not in dev]
    assert document["cards"] == expected
    ids = {card["id"] for card in document["cards"]}
    assert dev.isdisjoint(ids)
    assert label_hits(document) == []
    assert structural_hits(document) == []


def test_t6_fixture_absent_in_release_and_dev_doc_stays_fixture():
    from gen_audio.release_gate import card_badge

    release = json.loads(Path("schemas/examples/viewport.release.json").read_text(encoding="utf-8"))
    assert "cube-fixture" not in {card["id"] for card in release["cards"]}
    injected = json.loads(json.dumps(release))
    injected["views"] = [
        {"id": "view:cube-fixture", "snapshot": {"honesty": {"state": "fixture"}}}
    ]
    hits = label_hits(injected)
    assert any("fixture" in hit for hit in hits), hits

    example = json.loads(Path("schemas/examples/viewport.example.json").read_text(encoding="utf-8"))
    cube = next(card for card in example["cards"] if card["id"] == "cube-fixture")
    badge = card_badge(cube)
    assert badge == "fixture"
    assert badge not in {"library", "real"}


def test_release_document_passes_structural_rules():
    document = json.loads(Path("schemas/examples/viewport.release.json").read_text(encoding="utf-8"))
    assert structural_hits(document) == []
    for card in document["cards"]:
        assert str(card.get("uid", "")).startswith("ga:card:")


def test_dist_scan_fails_on_a_stub_phrase(tmp_path):
    bundle = tmp_path / "dist"
    assets = bundle / "assets"
    library = bundle / "library"
    assets.mkdir(parents=True)
    library.mkdir()
    (assets / "app.js").write_text('const badge = "Ref only";', encoding="utf-8")
    (bundle / "index.html").write_text("<p>ok</p>", encoding="utf-8")
    (library / "assets.json").write_text(
        json.dumps(
            {
                "assets": [
                    {
                        "legacy_id": "cube-fixture",
                        "honesty": {"fixture": True},
                        "body": {"id": "cube-fixture", "kind": "Cube3D", "body": {"source": "fixture-tone"}},
                    }
                ],
                "legacy_index": {"card:cube-fixture": "ga:card:eclezl34uhr3fuj6pukt7r6wkm"},
            }
        ),
        encoding="utf-8",
    )
    hits = dist_hits(bundle)
    blob = " ".join(hits)
    assert "Ref only" in blob
    assert "honesty=fixture" in blob
    assert "cube-fixture" in blob
    (assets / "app.js").write_text("const badge = 'Measured';", encoding="utf-8")
    (library / "assets.json").write_text(
        json.dumps(
            {
                "assets": [
                    {
                        "legacy_id": "engine-pocket",
                        "honesty": {"fixture": False},
                        "body": {
                            "id": "engine-pocket",
                            "kind": "EngineStatus",
                            "body": {"summary": "No adapter in this app."},
                        },
                    }
                ],
                "legacy_index": {"card:engine-pocket": "ga:card:nmfqip53e5thxk43egqjzl27da"},
            }
        ),
        encoding="utf-8",
    )
    assert dist_hits(bundle) == []
    assert dist_hits(tmp_path / "missing") == [f"dist bundle is missing: {tmp_path / 'missing'}"]


def test_dist_scan_fails_on_a_planted_catalog_title(tmp_path):
    """AP-OPT-1: a bare stub word in a catalog title or a JS string fails the gate."""
    bundle = tmp_path / "dist"
    library = bundle / "library"
    assets = bundle / "assets"
    library.mkdir(parents=True)
    assets.mkdir()
    (bundle / "index.html").write_text("<p>ok</p>", encoding="utf-8")
    (assets / "app.js").write_text('const title = "PLACEHOLDER clip";', encoding="utf-8")
    (library / "assets.json").write_text(
        json.dumps(
            {
                "assets": [
                    {
                        "legacy_id": "engine-pocket",
                        "honesty": {"fixture": False},
                        "display": {"title": "PLACEHOLDER clip", "label": "Pocket"},
                        "body": {
                            "id": "engine-pocket",
                            "kind": "EngineStatus",
                            "body": {"summary": "No adapter in this app."},
                        },
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    hits = dist_hits(bundle)
    blob = " ".join(hits)
    assert "assets.json" in blob and "placeholder" in blob.lower(), hits
    assert "app.js" in blob and "PLACEHOLDER" in blob, hits
    (assets / "app.js").write_text("const title = 'Pocket TTS';", encoding="utf-8")
    (library / "assets.json").write_text(
        json.dumps(
            {
                "assets": [
                    {
                        "legacy_id": "engine-pocket",
                        "honesty": {"fixture": False},
                        "display": {"title": "Pocket TTS", "label": "Pocket"},
                        "body": {
                            "id": "engine-pocket",
                            "kind": "EngineStatus",
                            "body": {"summary": "No adapter in this app."},
                        },
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    assert dist_hits(bundle) == []


def test_dist_scan_catches_stub_strings_in_any_case(tmp_path):
    """AP-OPT-1: the gate is case-insensitive and covers more than PLACEHOLDER.

    A quoted "placeholder", "FIXTURE", "TODO stub" or "Sample clip (preview)"
    fails. The HTML attribute name placeholder= and a `.placeholder` property
    are not stub copy, so a release bundle that uses them stays clean.
    """
    bundle = tmp_path / "dist"
    assets = bundle / "assets"
    library = bundle / "library"
    assets.mkdir(parents=True)
    library.mkdir()
    (library / "assets.json").write_text(
        json.dumps(
            {
                "assets": [
                    {
                        "legacy_id": "engine-pocket",
                        "honesty": {"fixture": False},
                        "display": {"title": "Pocket TTS", "label": "Pocket"},
                        "body": {
                            "id": "engine-pocket",
                            "kind": "EngineStatus",
                            "body": {"summary": "No adapter in this app."},
                        },
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    (bundle / "index.html").write_text(
        '<textarea placeholder="Paste a document"></textarea>',
        encoding="utf-8",
    )
    (assets / "app.js").write_text(
        'semantic.placeholder = "Semantic name"; const note = "Not a fixture."; const id = "fixture-tone";',
        encoding="utf-8",
    )
    assert dist_hits(bundle) == [], dist_hits(bundle)
    for source in (
        'const title = "placeholder clip";',
        'const title = "FIXTURE";',
        'const title = "TODO stub";',
        'const title = "Sample clip (preview)";',
    ):
        (assets / "app.js").write_text(source, encoding="utf-8")
        hits = dist_hits(bundle)
        assert hits, source
        assert "app.js" in " ".join(hits), hits
    (assets / "app.js").write_text('semantic.placeholder = "Semantic name";', encoding="utf-8")
    (library / "assets.json").write_text(
        json.dumps(
            {
                "assets": [
                    {
                        "legacy_id": "engine-pocket",
                        "honesty": {"fixture": False},
                        "display": {"title": "TODO stub", "label": "Pocket"},
                        "body": {
                            "id": "engine-pocket",
                            "kind": "EngineStatus",
                            "body": {"summary": "No adapter in this app."},
                        },
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    title_hits = dist_hits(bundle)
    assert any("todo stub" in hit.lower() for hit in title_hits), title_hits


def test_dist_scan_allows_placeholder_syntax_and_rejects_stub_text(tmp_path):
    """A quoted placeholder= attribute, .placeholder, and ::placeholder are not stubs.

    Each row is one quoted string. The syntax rows must produce no hit. The
    stub rows must. A pattern that matches every occurrence of "placeholder"
    fails the three syntax rows.
    """
    bundle = tmp_path / "dist"
    assets = bundle / "assets"
    library = bundle / "library"
    assets.mkdir(parents=True)
    library.mkdir()
    (bundle / "index.html").write_text("<p>ok</p>", encoding="utf-8")
    (library / "assets.json").write_text(
        json.dumps(
            {
                "assets": [
                    {
                        "legacy_id": "engine-pocket",
                        "honesty": {"fixture": False},
                        "display": {"title": "Pocket TTS", "label": "Pocket"},
                        "body": {
                            "id": "engine-pocket",
                            "kind": "EngineStatus",
                            "body": {"summary": "No adapter in this app."},
                        },
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    rows = (
        ('const html = \'<input placeholder="name">\';', False),
        ('const prop = ".placeholder";', False),
        ('const css = "input::placeholder { color: gray }";', False),
        ('const title = "placeholder clip";', True),
        ('const title = "FIXTURE";', True),
        ('const title = "TODO stub";', True),
        ('const title = "Sample clip (preview)";', True),
    )
    for source, stub in rows:
        (assets / "app.js").write_text(source, encoding="utf-8")
        hits = dist_hits(bundle)
        assert bool(hits) is stub, f"{source!r} -> {hits}"
