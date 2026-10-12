"""Offline fence and checked, durable speech publication."""

from __future__ import annotations

from contextlib import ExitStack, contextmanager
import hashlib
import json
import os
from pathlib import Path
import socket
import time
from unittest.mock import patch
import wave

from gen_audio.artifacts import check_destinations, staged_files

from .contracts import Request, SAMPLE_RATE, WorkerError
from .preflight import inspect_wav, preflight


@contextmanager
def offline_runtime():
    """Set flags before importing runtimes, and deny socket traffic as defense in depth."""
    def deny(*args, **kwargs):
        raise WorkerError("network_forbidden", "Network access is disabled in model workers")

    flags = {key: "1" for key in (
        "HF_HUB_OFFLINE", "TRANSFORMERS_OFFLINE", "HF_DATASETS_OFFLINE",
        "HF_HUB_DISABLE_TELEMETRY", "DO_NOT_TRACK",
    )}
    flags["TOKENIZERS_PARALLELISM"] = "false"
    with ExitStack() as stack:
        stack.enter_context(patch.dict(os.environ, flags))
        for name in ("connect", "connect_ex", "sendto", "sendall", "send"):
            stack.enter_context(patch.object(socket.socket, name, deny))
        stack.enter_context(patch.object(socket, "create_connection", deny))
        stack.enter_context(patch.object(socket, "getaddrinfo", deny))
        yield


def check_deadline(deadline: float) -> None:
    if time.monotonic() >= deadline:
        raise WorkerError("generation_timeout", "Model worker exceeded its cooperative generation deadline")


def checked_audio(audio, *, max_seconds: float, allow_silence: bool = False):
    import numpy as np

    if hasattr(audio, "detach"):
        audio = audio.detach().float().cpu().numpy()
    values = np.asarray(audio)
    if values.ndim == 2 and values.shape[0] == 1:
        values = values[0]
    if values.ndim != 1 or values.size == 0 or values.size > SAMPLE_RATE * max_seconds:
        raise WorkerError("invalid_generated_audio", "Expected nonempty mono audio within the duration cap")
    if values.dtype.kind != "f" or not np.isfinite(values).all():
        raise WorkerError("invalid_generated_audio", "Audio must contain only finite floating-point samples")
    if np.any(np.abs(values) > np.finfo(np.float32).max):
        raise WorkerError("invalid_generated_audio", "Audio samples exceed finite FP32 range")
    if not allow_silence and not np.any(values):
        raise WorkerError("invalid_generated_audio", "Model produced only silence")
    return values.astype(np.float32)


def read_pcm(path: Path, *, seconds: float, allow_silence: bool = False):
    import numpy as np

    inspect_wav(path, seconds=seconds)
    with wave.open(str(path), "rb") as handle:
        values = np.frombuffer(handle.readframes(handle.getnframes()), dtype="<i2").astype(np.float32) / 32768.0
    return checked_audio(values, max_seconds=seconds, allow_silence=allow_silence)


def publish(request: Request, audio, provenance: dict) -> dict:
    import numpy as np

    if request.output is None:
        raise WorkerError("missing_output", "Generation requires --output")
    audio = checked_audio(audio, max_seconds=request.max_seconds)
    pcm = np.rint(np.clip(audio, -1, 1) * 32767).astype("<i2")
    if not np.any(pcm):
        raise WorkerError("invalid_generated_audio", "Output quantizes to silence; no speech artifact published")
    output, manifest = check_destinations([request.output, request.output.with_suffix(".synth.json")])
    receipt = {
        "schema_version": "1.0", "ok": True, "synthesizedSpeech": True,
        "engine": request.engine, "output": str(output), "manifest": str(manifest),
        "sample_rate": SAMPLE_RATE, "samples": int(pcm.size), "duration_s": pcm.size / SAMPLE_RATE,
    }
    with staged_files([output, manifest]) as staged:
        with wave.open(str(staged[0]), "wb") as handle:
            handle.setnchannels(1)
            handle.setsampwidth(2)
            handle.setframerate(SAMPLE_RATE)
            handle.writeframes(pcm.tobytes())
        with wave.open(str(staged[0]), "rb") as handle:
            if handle.getnframes() != pcm.size or len(handle.readframes(pcm.size)) != pcm.size * 2:
                raise WorkerError("publication_failed", "Written WAV failed frame verification")
        from gen_audio.audio_metadata import tag_wav
        receipt["metadata"] = tag_wav(staged[0], request.agent_names)
        receipt["sha256"] = hashlib.sha256(staged[0].read_bytes()).hexdigest()
        record = {
            **receipt, "task": "speech_to_speech" if request.engine == "personaplex" else "script_tts",
            "transcript_verified": False,
            "output_semantics": "generated_conversation_speech" if request.engine == "personaplex" else "generated_script_speech",
            "clipped_samples": int(np.count_nonzero(np.abs(audio) > 1)),
            "provenance": provenance,
        }
        staged[1].write_text(json.dumps(record, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    return receipt


def execute(request: Request, *, resident=None) -> dict:
    report = preflight(request, resident=resident is not None) if resident is not None else preflight(request)
    if not report["ready"]:
        return report
    if request.output is None:
        raise WorkerError("missing_output", "Generation requires --output")
    check_destinations([request.output, request.output.with_suffix(".synth.json")])
    from .execution import recheck_memory, synchronize
    recheck_memory(report)
    deadline = time.monotonic() + request.timeout_seconds
    synchronize(report["execution"])
    started = time.perf_counter()
    with offline_runtime():
        if request.engine == "vibevoice":
            from .vibevoice import generate
        else:
            from .personaplex import generate
        audio, details = generate(request, report, deadline, resident=resident) if resident is not None else generate(request, report, deadline)
    check_deadline(deadline)
    synchronize(report["execution"])
    elapsed = time.perf_counter() - started
    return publish(request, audio, {
        "runtime_revision": report["runtime"]["revision"],
        "runtime_repository": report["runtime"]["repository"],
        "runtime_profile": report["runtime"].get("profile", "original"),
        "runtime_patches_sha256": report["runtime"].get("patches_sha256"),
        "runtime_wheel_sha256": report["runtime"].get("wheel_sha256"),
        "dependencies": report["dependencies"], "model_dir": report["model_dir"],
        "weights": report["assets"]["weights"],
        "device": report["device"], "dtype": report["dtype"], "execution": report["execution"], "offline": True,
        "timing": {"generation_seconds": elapsed, "includes_model_load": resident is None,
                   "real_time_factor": elapsed / (audio.size / SAMPLE_RATE)},
        "seed": request.seed, "limits": report["limits"], **details,
    })
