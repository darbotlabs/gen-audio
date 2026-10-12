"""Resolve an upgrade, reject downgrades, then install the reviewed resolution.

Run with the target isolated runtime's Python. --minimum remembers versions
that a previous resolver transaction may have accidentally lowered.
"""
from __future__ import annotations
import argparse
import importlib.metadata
import json
from pathlib import Path
import subprocess
import sys
from packaging.utils import canonicalize_name
from packaging.version import Version

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report-dir", type=Path, required=True)
    parser.add_argument("--minimum", action="append", default=[])
    parser.add_argument("requirements", nargs="+")
    args = parser.parse_args()
    output = args.report_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    installed_before = {canonicalize_name(d.metadata['Name']): d.version for d in importlib.metadata.distributions()}
    floors = dict(installed_before)
    for minimum in args.minimum:
        name, version = minimum.split("==", 1)
        name = canonicalize_name(name)
        floors[name] = str(max(Version(floors.get(name, "0")), Version(version)))
    constraints = output / "no-downgrades.txt"
    # PEP 440 disallows local labels in >= constraints; the full version is
    # compared again below before installation, retaining ROCm build identity.
    constraints.write_text("\n".join(f"{n}>={Version(v).public}" for n, v in sorted(floors.items())) + "\n")
    report = output / "resolution.json"
    command = [sys.executable, "-m", "pip", "install", "--disable-pip-version-check", "--progress-bar", "off",
               "--upgrade", "--upgrade-strategy", "only-if-needed", "--constraint", str(constraints),
               *args.requirements, *(minimum.split("==", 1)[0] for minimum in args.minimum)]
    subprocess.run([*command, "--dry-run", "--report", str(report)], check=True)
    resolved = json.loads(report.read_text(encoding="utf-8"))["install"]
    changes = []
    locked = []
    for item in resolved:
        name, version = canonicalize_name(item['metadata']['name']), item['metadata']['version']
        if name in floors and Version(version) < Version(floors[name]):
            raise RuntimeError(f"Downgrade rejected: {name} {floors[name]} -> {version}")
        download = item['download_info']
        digest = download.get('archive_info', {}).get('hashes', {}).get('sha256')
        if not digest or not download['url'].split('#')[0].endswith('.whl'):
            raise RuntimeError(f"Upgrade requires a hashed wheel: {name}")
        locked.append(f"{name} @ {download['url']} --hash=sha256:{digest}")
        changes.append({'package':name,'before':installed_before.get(name),'after':version})
    lock = output / "resolved-wheels.txt"
    lock.write_text("\n".join(locked) + "\n", encoding="utf-8")
    (output / "upgrade-plan.json").write_text(json.dumps(changes, indent=2) + "\n")
    if locked:
        subprocess.run([sys.executable,"-m","pip","install","--disable-pip-version-check","--progress-bar","off",
                        "--no-deps","--require-hashes","-r",str(lock)],check=True)
    subprocess.run([sys.executable,"-m","pip","check"],check=True)
    print(json.dumps({'upgraded':len(changes),'downgrades':0,'report':str(output)}))

if __name__ == "__main__":
    main()
