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
    path = Path("schemas/examples/viewport.release.json")
    document = json.loads(path.read_text(encoding="utf-8"))
    assert label_hits(document) == []
    ids = {card["id"] for card in document["cards"]}
    assert "cube-fixture" not in ids
    assert "spec-fixture" not in ids
    assert "cast-sample" not in ids


def test_t6_fixture_absent_in_release_and_dev_doc_stays_fixture():
    from gen_audio.release_gate import card_badge

    release = json.loads(Path("schemas/examples/viewport.release.json").read_text(encoding="utf-8"))
    assert label_hits(release) == []
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
