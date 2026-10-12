"""Qualification profiles retain original runtimes and verify upstream ports."""
import hashlib
import importlib.metadata
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

def installed_spec(spec):
    try:
        version = importlib.metadata.version(spec["distribution"])
    except importlib.metadata.PackageNotFoundError:
        return spec
    name = {"vibevoice": "vibevoice", "moshi-personaplex": "personaplex"}.get(spec["distribution"])
    if name and version == "0.1.0+genaudio.1":
        profile = json.loads((ROOT / f"runtimes/{name}-modern.json").read_text(encoding="utf-8"))
        lock = json.loads((ROOT / f"runtimes/{name}-modern.lock.json").read_text(encoding="utf-8"))
        return {**spec, "versions": profile["versions"], "profile": profile["name"],
                "patches_sha256": lock["patches_sha256"], "wheel_sha256": lock["wheel_sha256"],
                "requirements": f"runtimes/{name}-modern.json"}
    return spec

def verify_port(distribution, name):
    lock = json.loads((ROOT / f"runtimes/{name}-modern.lock.json").read_text(encoding="utf-8"))
    for relative, expected in lock["files"].items():
        path = Path(distribution.locate_file(relative))
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError(f"Modified upstream compatibility file: {relative}")
    return lock["patches_sha256"]
