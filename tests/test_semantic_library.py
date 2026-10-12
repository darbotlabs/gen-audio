"""Prevent contract/example drift when editing the semantic domain registry."""
import importlib.util
import json
from pathlib import Path


def test_shipped_semantic_contracts_match_authoritative_domain_library():
    root = Path(__file__).resolve().parents[1]
    spec = importlib.util.spec_from_file_location("semantic_library_generator", root / "scripts/generate-semantic-library.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    records, cube = module.contracts()
    for path, expected in [("schemas/semantic-record.schema.json", records),
                           ("schemas/semantic-cube.schema.json", cube),
                           ("schemas/examples/semantic-cubes.json", module.examples())]:
        assert json.loads((root / path).read_text(encoding="utf-8")) == expected, f"Regenerate {path}"
