"""Run the installed AMD quicktest model in an isolated NPU environment.

Usage: .venv-npu/Scripts/python scripts/probe-npu.py
Uses no audio/model weights, never changes drivers or firmware. Artifacts stay
under artifacts/validation/accelerators, with a fresh compiler cache per run.
"""
import json
import os
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
SDK = Path(os.environ.get("RYZEN_AI_INSTALLATION_PATH", r"C:\Program Files\RyzenAI\1.6.1"))
OUT = ROOT / "artifacts/validation/accelerators"
OUT.mkdir(parents=True, exist_ok=True)
os.environ["RYZEN_AI_INSTALLATION_PATH"] = str(SDK)
os.environ["PATH"] = os.pathsep.join([str(SDK / "onnxruntime/bin"), str(SDK / "voe-4.0-win_amd64/bin"), str(SDK / "xrt"), os.environ["PATH"]])
handles = [os.add_dll_directory(str(path)) for path in [SDK / "onnxruntime/bin", SDK / "voe-4.0-win_amd64/bin", SDK / "xrt"] if path.is_dir()]
import numpy as np
import onnxruntime as ort

report = {"runtime": ort.__version__, "availableProviders": ort.get_available_providers(),
          "model": str(SDK / "quicktest/test_model.onnx"), "audioModelTested": False}
try:
    if "VitisAIExecutionProvider" not in report["availableProviders"]:
        raise RuntimeError("Installed ONNX Runtime has no NPU execution provider")
    started = time.monotonic()
    session = ort.InferenceSession(report["model"], providers=["VitisAIExecutionProvider"], provider_options=[{
        "cache_dir": str(OUT), "cache_key": f"quicktest-{time.time_ns()}"}])
    session.disable_fallback()
    report["sessionProviders"] = session.get_providers()
    if "VitisAIExecutionProvider" not in report["sessionProviders"]:
        raise RuntimeError("NPU session fell back to a CPU provider")
    outputs = session.run(None, {"input": np.random.default_rng(0).random((1,3,32,32), dtype=np.float32)})
    report.update(ok=all(np.isfinite(value).all() for value in outputs), shapes=[list(value.shape) for value in outputs], elapsedSeconds=time.monotonic()-started)
except Exception as error:
    report.update(ok=False, error=str(error))
finally:
    (OUT / "npu-probe.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
sys.exit(0 if report.get("ok") else 1)
