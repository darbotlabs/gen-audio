"""Opt-in authenticated MCP speech round trip using local Kokoro weights."""

from __future__ import annotations

import hashlib
import http.client
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import sys
import time

import pytest
import soundfile as sf


@pytest.mark.skipif(os.environ.get("GEN_AUDIO_RUN_REAL_MCP") != "1", reason="opt-in local MCP/weights test")
def test_authenticated_mcp_speech_and_artifact_delivery(tmp_path):
    root = Path(__file__).resolve().parents[1]
    binary = Path(os.environ.get("GEN_AUDIO_MCP_TEST_BINARY", str(root / "target" / "debug" / ("gen-audio-mcp.exe" if os.name == "nt" else "gen-audio-mcp"))))
    token = secrets.token_hex(32)
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    env = {
        **os.environ,
        "GEN_AUDIO_MCP_TOKEN": token,
        "GEN_AUDIO_WORK_DIR": str(tmp_path),
        "LOCALAPPDATA": str(tmp_path / "appdata"),
        "GEN_AUDIO_PYTHON": sys.executable,
        "GEN_AUDIO_MODEL_DIR": str(root / "models" / "kokoro-onnx"),
        "GEN_AUDIO_KOKORO_MODEL": str(root / "models" / "kokoro-onnx" / "kokoro-v1.0.onnx"),
        "GEN_AUDIO_KOKORO_VOICES": str(root / "models" / "kokoro-onnx" / "voices-v1.0.bin"),
        "OMP_NUM_THREADS": "2",
    }
    env.pop("GEN_AUDIO_ARTIFACT_DIR", None)

    def request(method, path, payload=None, *, authorized=True, origin=None):
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=125)
        headers = {"Content-Type": "application/json"}
        if authorized:
            headers["Authorization"] = f"Bearer {token}"
        if origin:
            headers["Origin"] = origin
        try:
            connection.request(method, path, body=json.dumps(payload) if payload is not None else None, headers=headers)
            response = connection.getresponse()
            return response.status, response.read()
        finally:
            connection.close()

    def rpc(method, params):
        status, raw = request("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
        assert status == 200, raw
        result = json.loads(raw)
        assert "error" not in result, result
        return result["result"]

    process = subprocess.Popen(
        [str(binary), "--http", f"127.0.0.1:{port}"],
        env=env, cwd=root, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.monotonic() + 10
        while True:
            assert process.poll() is None, "MCP listener exited before becoming ready"
            try:
                if request("GET", "/health")[0] == 200:
                    break
            except (ConnectionError, OSError):
                pass
            assert time.monotonic() < deadline, "MCP listener did not become ready"
            time.sleep(0.05)
        assert request("GET", "/health", authorized=False)[0] in (401, 403)
        assert request("GET", "/control", origin="https://untrusted.example")[0] == 403
        initialized = rpc("initialize", {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "local-speech-regression", "version": "1.0"}})
        assert initialized["serverInfo"]["name"] == "gen-audio"
        script_text = "Speaker 1: This speech came through the authenticated local pipeline.\nSPEAKER 2: Both selected voices are present in the output."
        analyzed = rpc("tools/call", {"name": "script_analyze", "arguments": {
            "scriptText": script_text, "agents": ["alice", "frank"], "durationMinutes": 1,
        }})
        normalized = json.loads(analyzed["content"][0]["text"])
        assert len(normalized["turns"]) == 2 and normalized["timingBasis"] == "word_estimate"
        preview = rpc("tools/call", {"name": "draft_plan", "arguments": {
            "request": {"topic": "Sound waves", "agents": ["alice", "frank"], "durationMinutes": 1},
            "connector": "gpt",
        }})
        assert json.loads(preview["content"][0]["text"])["externalRequestSent"] is False
        speech = rpc("tools/call", {"name": "synth", "arguments": {
            "scriptText": normalized["scriptText"],
            "agents": ["alice", "frank"], "voice": "kokoro_onnx",
        }})
        assert not speech["isError"], speech
        receipt = json.loads(speech["content"][0]["text"])
        assert receipt["metadata"] == {"artist":"Alice; Frank","artists":["Alice","Frank"],"agentNames":["Alice","Frank"]}
        assert receipt["ok"] and receipt["synthesizedSpeech"]
        assert receipt["turns"] == 2 and receipt["sample_rate"] == 24000
        code, audio_bytes = request("GET", f"/artifacts/{receipt['wav']}")
        assert code == 200
        assert hashlib.sha256(audio_bytes).hexdigest() == receipt["sha256"]
        output = tmp_path / receipt["wav"]
        assert output.read_bytes() == audio_bytes
        from gen_audio.audio_metadata import read_artist
        assert read_artist(output) == "Alice; Frank"
        ffprobe = shutil.which("ffprobe")
        if ffprobe:
            probe = subprocess.run([ffprobe,"-v","error","-show_entries","format_tags=artist","-of","json",str(output)],
                                   capture_output=True,text=True,encoding="utf-8",timeout=10,check=True)
            assert json.loads(probe.stdout)["format"]["tags"]["artist"] == "Alice; Frank"
        info = sf.info(output)
        assert info.frames == receipt["samples"] and info.samplerate == 24000
        assert 0 < info.duration < 30
        code, manifest = request("GET", f"/artifacts/{receipt['manifest']}")
        assert code == 200
        assert json.loads(manifest)["sha256"] == receipt["sha256"]
        assert json.loads(manifest)["metadata"] == receipt["metadata"]
        opened = rpc("tools/call", {"name":"generated_library_open","arguments":{"uid":receipt["assetUid"]}})
        assert not opened["isError"], opened
        reopened = json.loads(opened["content"][0]["text"])
        assert reopened["metadata"] == receipt["metadata"]
        assert reopened["sha256"] == receipt["sha256"]
        assert request("GET", f"/artifacts/{receipt['wav']}", authorized=False)[0] in (401, 403)
        assert request("GET", "/artifacts/../Cargo.toml")[0] in (400, 403, 404)
        improved = rpc("tools/call", {"name": "improve", "arguments": {"input": receipt["wav"], "output": "improved.wav"}})
        assert not improved["isError"], improved
        assert read_artist(tmp_path / "improved.wav") == "Alice; Frank"
        if shutil.which("ffmpeg"):
            mastered = rpc("tools/call", {"name": "master_audio", "arguments": {"input": receipt["wav"]}})
            assert not mastered["isError"], mastered
            mastered = json.loads(mastered["content"][0]["text"])
            assert mastered["processedAudio"] and not mastered["synthesizedSpeech"]
            assert mastered["metadata"] == receipt["metadata"]
            assert read_artist(tmp_path / mastered["wav"]) == "Alice; Frank"
            assert mastered["sourceSha256"] == receipt["sha256"]
            code, final_wav = request("GET", f"/artifacts/{mastered['wav']}")
            assert code == 200 and hashlib.sha256(final_wav).hexdigest() == mastered["sha256"]
            assert output.read_bytes() == audio_bytes
            assert abs(mastered["report"]["final"]["integrated_lufs"] + 16) <= 1
        report_dir = os.environ.get("GEN_AUDIO_VALIDATION_REPORT_DIR")
        if report_dir:
            destination = Path(report_dir)
            destination.mkdir(parents=True, exist_ok=True)
            report = {"passed":True,"metadata":receipt["metadata"],"wav_artist_verified":True,
                      "wav_sha256":receipt["sha256"],"saved_reopen_preserved_metadata":True,
                      "improve_preserved_artist":True,"master_preserved_artist":bool(shutil.which("ffmpeg")),
                      "ffprobe_artist_verified":bool(ffprobe)}
            (destination / "mcp-artist-validation.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    finally:
        process.terminate()
        process.wait(timeout=10)
