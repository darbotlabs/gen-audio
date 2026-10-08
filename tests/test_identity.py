"""Golden rounding and mint rows from schemas/asset-object/vectors/v1.json."""

from __future__ import annotations

import json
import unicodedata
from pathlib import Path

from gen_audio.identity import SCHEMA_MAJOR, bin_frames_inferred, canonicalize, mint, ms_from_frames, round_half_up

ROOT = Path(__file__).resolve().parents[1]
VECTORS = ROOT / "schemas" / "asset-object" / "vectors" / "v1.json"


def test_rounding_and_mint_vectors() -> None:
    document = json.loads(VECTORS.read_text(encoding="utf-8"))
    rounding = document["rounding"]
    minted = document["mint"]
    assert rounding and minted
    for vector in rounding:
        if vector["op"] == "ms_from_frames":
            assert ms_from_frames(vector["frames"], vector["rate"]) == vector["expect"]
        elif vector["op"] == "round_half_up":
            assert round_half_up(vector["value"] * vector["scale"]) == vector["expect"]
        elif vector["op"] == "bin_frames_inferred":
            assert (
                bin_frames_inferred(vector["duration_s"], vector["sample_rate_hz"], vector["time_bins"])
                == vector["expect"]
            )
        else:
            raise AssertionError(vector["op"])
    for row in minted:
        fields = json.loads(row["fields_json"])
        if row.get("normalize_nfc"):
            fields = _nfc(fields)
        media = [{"role": item["role"], "sha256": item["sha256"]} for item in row["media"]]
        uid = mint(row["kind"], fields, media, row["src"])
        assert uid == row["expect"]["uid"], row["name"]
        identity = {
            "kind": row["kind"],
            "schema_major": SCHEMA_MAJOR,
            "fields": fields,
            "media": sorted(media, key=lambda item: (item["role"], item["sha256"])),
            "src": sorted(row["src"]),
        }
        assert canonicalize(identity) == row["expect"]["canonical"], row["name"]


def _nfc(value: object) -> object:
    if isinstance(value, str):
        return unicodedata.normalize("NFC", value)
    if isinstance(value, list):
        return [_nfc(item) for item in value]
    if isinstance(value, dict):
        return {unicodedata.normalize("NFC", key): _nfc(item) for key, item in value.items()}
    return value
