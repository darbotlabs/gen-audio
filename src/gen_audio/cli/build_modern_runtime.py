"""Build reviewable compatibility wheels from hash-pinned upstream archives."""
from __future__ import annotations
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[3]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", choices=("vibevoice", "personaplex", "datasets", "all"), default="all")
    args = parser.parse_args()
    sources = json.loads((ROOT / "runtimes/modern-sources.json").read_text())
    patches = json.loads((ROOT / "runtimes/modern-patches.json").read_text())
    output = ROOT / "target/runtime-wheels"
    output.mkdir(parents=True, exist_ok=True)
    scratch = ROOT / "target/runtime-builds"
    scratch.mkdir(parents=True, exist_ok=True)
    for name, source in sources.items():
        if args.package not in ("all", name):
            continue
        with urllib.request.urlopen(source["url"], timeout=60) as response:
            data = response.read(128 * 1024 * 1024 + 1)
        if hashlib.sha256(data).hexdigest() != source["sha256"]:
            raise RuntimeError(f"{name}: upstream archive checksum mismatch")
        with tempfile.TemporaryDirectory(prefix=name + "-", dir=scratch) as directory:
            base = Path(directory).resolve()
            if source["url"].split("?")[0].endswith(".zip") or "/zip/" in source["url"]:
                with zipfile.ZipFile(io.BytesIO(data)) as archive:
                    for member in archive.infolist():
                        target = (base / member.filename).resolve()
                        if not target.is_relative_to(base) or member.file_size > 32 * 1024 * 1024:
                            raise RuntimeError("Unsafe archive member")
                        if member.is_dir():
                            target.mkdir(parents=True, exist_ok=True)
                        else:
                            target.parent.mkdir(parents=True, exist_ok=True)
                            target.write_bytes(archive.read(member))
            else:
                with tarfile.open(fileobj=io.BytesIO(data)) as archive:
                    archive.extractall(base, filter="data")
            project = base / source["root"]
            for patch in patches[name]:
                target = (project / patch["path"]).resolve()
                if not target.is_relative_to(project):
                    raise RuntimeError("Patch path escapes project")
                text = target.read_text(encoding="utf-8")
                if text.count(patch["before"]) != patch.get("count", 1):
                    raise RuntimeError(f"Patch does not match pinned source: {name}/{patch['path']}")
                target.write_text(text.replace(patch["before"], patch["after"]), encoding="utf-8", newline="\n")
            environment = {**os.environ, "SOURCE_DATE_EPOCH": "1767225600"}
            subprocess.run([sys.executable, "-m", "pip", "wheel", "--no-deps", "--no-build-isolation",
                            "--wheel-dir", str(output), str(project)], check=True, env=environment)
        distribution = source.get("distribution", name).replace("-", "_")
        module = source.get("module", name)
        wheel = next(output.glob(f"{distribution}-{source['version']}-*.whl"))
        with zipfile.ZipFile(wheel) as archive:
            files = {p: hashlib.sha256(archive.read(p)).hexdigest() for p in archive.namelist()
                     if p.endswith(".py") and p.startswith(module + "/")}
        provenance = {"source": source, "patches_sha256": hashlib.sha256(json.dumps(patches[name], sort_keys=True).encode()).hexdigest(),
                      "wheel": wheel.name, "wheel_sha256": hashlib.sha256(wheel.read_bytes()).hexdigest(), "files": files}
        (output / f"{name}.provenance.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
        (ROOT / f"runtimes/{name}-modern.lock.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"built": str(wheel), "sha256": provenance["wheel_sha256"]}), flush=True)

if __name__ == "__main__":
    main()
