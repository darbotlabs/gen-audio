"""Resident offline model owner. Only Rust supplies startup paths and CLI argv."""
from __future__ import annotations

import argparse
from contextlib import redirect_stderr, redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import time

MAX_MESSAGE = 256 * 1024


class BoundedText(io.StringIO):
    def write(self, value):
        if self.tell() + len(value) > MAX_MESSAGE:
            raise ValueError("Runtime output limit exceeded")
        return super().write(value)


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as handle:
        while block := handle.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


class Owner:
    def __init__(self, engine, model_dir=None, tokenizer=None, execution=None):
        self.engine = engine
        self.model_dir = Path(model_dir).resolve() if model_dir else None
        self.tokenizer = Path(tokenizer).resolve() if tokenizer else None
        self.resident = None
        self.identities = {}
        self.execution = execution

    def load(self):
        from .model_workers.runtime import offline_runtime
        if self.resident is not None:
            self.verify_assets()
            return self.status()
        with offline_runtime(), redirect_stdout(sys.stderr):
            if self.engine == "kokoro_onnx":
                from .synth_kokoro_onnx import KokoroOnnxSynthesizer, resolve_model_paths
                model, voices = resolve_model_paths()
                runtime = KokoroOnnxSynthesizer(model, voices)
                self.identities = {path: digest(path) for path in (model, voices)}
                self.resident = runtime
            else:
                from .model_workers.contracts import Request
                from .model_workers.preflight import preflight
                request = Request(engine=self.engine, model_dir=self.model_dir, tokenizer=self.tokenizer,
                                  timeout_seconds=115, execution=self.execution)
                report = preflight(request, load_only=True)
                if not report["ready"]:
                    return {**self.status(), "errors":report["errors"], "memory":report.get("memory")}
                paths = [Path(item["path"]) for item in report["assets"]["weights"]]
                paths += [self.model_dir / name for name in ("config.json", "preprocessor_config.json")
                          if (self.model_dir / name).is_file()]
                paths += ([self.tokenizer / name for name in
                           ("tokenizer_config.json", "tokenizer.json", "vocab.json", "merges.txt")]
                          if self.tokenizer.is_dir() else [self.tokenizer])
                identities = {path: digest(path) for path in paths}
                if self.engine == "vibevoice":
                    from .model_workers.vibevoice import load
                else:
                    from .model_workers.personaplex import load
                runtime = load(request, report, time.monotonic() + 115)
                if any(digest(path) != value for path, value in identities.items()):
                    raise ValueError("Model assets changed during loading")
                runtime.update(model_dir=str(self.model_dir), tokenizer_path=str(self.tokenizer))
                self.identities, self.resident = identities, runtime
        return self.status()

    def verify_assets(self):
        if any(digest(path) != value for path, value in self.identities.items()):
            self.resident = None
            raise ValueError("Loaded model assets changed; reload the engine")

    def status(self):
        return {"engine":self.engine, "loaded":self.resident is not None, "pid":os.getpid(),
                "phase":"loaded" if self.resident is not None else "unloaded",
                "execution": self.resident.get("execution") if isinstance(self.resident, dict) else None,
                "assets":[{"name":path.name, "sha256":value} for path, value in self.identities.items()]}

    def run(self, args, *, entrypoint="synth"):
        if self.resident is None:
            raise ValueError("Engine is not loaded")
        if (not isinstance(args, list) or len(args) > 128
                or any(not isinstance(arg, str) or "\x00" in arg for arg in args)
                or sum(len(arg.encode("utf-8")) for arg in args) > 32768):
            raise ValueError("Invalid bounded runtime argv")
        self.verify_assets()
        if self.engine == "kokoro_onnx":
            if entrypoint == "generate":
                from .cli.generate import main
            elif entrypoint == "synth":
                from .cli.synth import main
            else:
                raise ValueError("Unknown Kokoro entry point")
        else:
            if entrypoint != "synth":
                raise ValueError("Unknown model worker entry point")
            from .model_workers.cli import main
        stdout, stderr = BoundedText(), BoundedText()
        from .model_workers.runtime import offline_runtime
        with offline_runtime(), redirect_stdout(stdout), redirect_stderr(stderr):
            code = main(args, resident=self.resident)
        return {"code":code, "stdout":stdout.getvalue(), "stderr":"", **self.status()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", required=True, choices=("kokoro_onnx", "vibevoice", "personaplex"))
    parser.add_argument("--model-dir")
    parser.add_argument("--tokenizer")
    parser.add_argument("--execution")
    args = parser.parse_args()
    owner = Owner(args.engine, args.model_dir, args.tokenizer, json.loads(args.execution) if args.execution else None)
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    sys.stdin.reconfigure(encoding="utf-8")
    while raw := sys.stdin.buffer.readline(MAX_MESSAGE + 1):
        if len(raw) > MAX_MESSAGE or not raw.endswith(b"\n"):
            return 2
        request_id = None
        try:
            message = json.loads(raw)
            if not isinstance(message, dict) or set(message) - {"id", "operation", "args"}:
                raise ValueError("Invalid runtime message")
            request_id = message.get("id")
            if type(request_id) is not int or request_id < 1:
                raise ValueError("Invalid runtime request id")
            operation = message.get("operation")
            if operation == "load":
                result = owner.load()
            elif operation == "status":
                result = owner.status()
            elif operation == "run":
                result = owner.run(message.get("args"))
            elif operation == "run_generate":
                result = owner.run(message.get("args"), entrypoint="generate")
            else:
                raise ValueError("Unknown runtime operation")
            response = {"id":request_id, "ok":True, "result":result}
        except (Exception, SystemExit) as error:
            # Arguments and spoken text never enter diagnostic output.
            response = {"id":request_id, "ok":False, "error":str(error)[:2000], "loaded":owner.resident is not None}
        encoded = json.dumps(response, ensure_ascii=True, allow_nan=False)
        if len(encoded.encode("utf-8")) > MAX_MESSAGE:
            return 2
        print(encoded, flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
