"""Opt-in CPU smoke test; never renders or changes desktop library assets."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

import numpy as np
import pytest
import soundfile as sf


@pytest.mark.skipif(os.environ.get("GEN_AUDIO_RUN_REAL_KOKORO") != "1", reason="opt-in local-model smoke test")
def test_real_kokoro_cli_multi_turn(tmp_path):
    root = Path(__file__).resolve().parents[1]
    script = tmp_path / "smoke.txt"
    script.write_text(
        "Speaker 1: This is a local speech synthesis check.\n"
        "Speaker 2: Both voices are present in the finished audio.\n",
        encoding="utf-8",
    )
    output = tmp_path / "speech.wav"
    env = os.environ.copy()
    env.pop("GEN_AUDIO_ARTIFACT_DIR", None)
    run = subprocess.run(
        [
            sys.executable, str(root / "scripts" / "synth_kokoro_onnx.py"), str(script),
            "--cast-map", str(root / "voices" / "cast_map.example.json"),
            "--model", str(root / "models" / "kokoro-onnx" / "kokoro-v1.0.onnx"),
            "--voices", str(root / "models" / "kokoro-onnx" / "voices-v1.0.bin"),
            "-o", str(output), "--json", "--improve", "--max-duration", "30",
        ],
        cwd=root, env=env, capture_output=True, text=True, timeout=120,
    )
    assert run.returncode == 0, run.stderr
    receipt = json.loads(run.stdout.strip().splitlines()[-1])
    audio, rate = sf.read(output, dtype="float32")
    manifest = json.loads(Path(receipt["manifest"]).read_text(encoding="utf-8"))
    assert receipt["ok"] and receipt["synthesizedSpeech"]
    assert np.isfinite(audio).all() and np.any(audio)
    assert rate == receipt["sample_rate"] == 24000
    assert audio.size == receipt["samples"] > 0
    assert 0 < receipt["duration_s"] <= 30
    assert receipt["sha256"] == hashlib.sha256(output.read_bytes()).hexdigest()
    assert manifest["turn_count"] == manifest["speaker_count"] == 2
    assert manifest["provenance"]["model"]["path"].endswith("kokoro-v1.0.onnx")
    assert manifest["provenance"]["voices"]["path"].endswith("voices-v1.0.bin")
    assert all("text" not in turn for turn in manifest["turns"])
