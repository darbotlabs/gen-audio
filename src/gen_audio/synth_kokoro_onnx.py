"""Local Kokoro adapter; model downloads and fabricated checksum pins are forbidden.

``GENAID_MODEL_SHA256_FILE`` (or model-adjacent ``SHA256SUMS``) is relative
to the model's directory. It must cover both loaded files whenever present.
``GENAID_REQUIRE_MODEL_CHECKSUM=1`` refuses unpinned models even without a
``GENAID_MODEL_BASE``. Observed hashes alone are provenance, never trusted pins.
"""

from __future__ import annotations

import os
import threading
import time
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path

import numpy as np

from gen_audio.cast import CastMap, Turn, VoiceAssignment
from gen_audio.guards import (
    max_generation_seconds, sha256_file, verify_model_checksums,
)
from gen_audio.synthesis import (
    DEFAULT_TURN_GAP_S, SynthResult, SynthTurn, synthesize_turns, validate_request,
)

_MODEL_LOCK = threading.Lock()

def resolve_model_paths(
    model: str | Path | None = None, voices: str | Path | None = None,
) -> tuple[Path, Path]:
    """Resolve local files; confine both real paths when a model base is configured."""
    model_raw = model or os.environ.get("GEN_AUDIO_KOKORO_MODEL")
    voices_raw = voices or os.environ.get("GEN_AUDIO_KOKORO_VOICES")
    if not model_raw or not voices_raw:
        raise RuntimeError(
            "kokoro-onnx model paths are missing. Pass --model and --voices, or set "
            "GEN_AUDIO_KOKORO_MODEL and GEN_AUDIO_KOKORO_VOICES. This package does not download weights."
        )
    paths = []
    base_raw = os.environ.get("GENAID_MODEL_BASE", "").strip()
    base = Path(base_raw).resolve() if base_raw else None
    for raw in (model_raw, voices_raw):
        text = str(raw).replace("\\", "/").lower()
        if "://" in text or text.startswith("file:") or "\x00" in text:
            raise ValueError("model and voices must be local file paths")
        path = Path(raw)
        # Explicit relative paths retain the CLI's working-directory semantics.
        path = path.resolve()
        if base is not None and not path.is_relative_to(base):
            raise ValueError("model and voices must stay inside GENAID_MODEL_BASE")
        paths.append(path)
    verify_model_checksums(str(paths[0].parent), required_files=tuple(paths))
    return paths[0], paths[1]


def _package_version(name: str) -> str | None:
    try:
        return version(name)
    except PackageNotFoundError:
        return None


class KokoroOnnxSynthesizer:
    """Kokoro implementation of EngineAdapter plus the backwards-compatible facade."""

    engine_id = "kokoro_onnx"

    def __init__(self, model_path: str | Path, voices_path: str | Path) -> None:
        model, voices = resolve_model_paths(model_path, voices_path)
        for path, label in ((model, "model"), (voices, "voices")):
            if not path.is_file():
                raise FileNotFoundError(f"kokoro-onnx {label} file not found: {path}")
        pins = verify_model_checksums(str(model.parent), required_files=(model, voices))
        identities = {
            label: {"path": str(path), "bytes": path.stat().st_size, "sha256": sha256_file(path)}
            for label, path in (("model", model), ("voices", voices))
        }
        try:
            from kokoro_onnx import Kokoro
        except ImportError as exc:
            raise ImportError("kokoro-onnx is not installed; install 'gen-audio[kokoro]'") from exc
        self._kokoro = Kokoro(str(model), str(voices))
        for label, path in (("model", model), ("voices", voices)):
            if sha256_file(path) != identities[label]["sha256"]:
                raise ValueError(f"{label} changed during engine loading")
        # Verify again so the provenance cannot label post-verification changes as pinned.
        verify_model_checksums(str(model.parent), required_files=(model, voices))
        self.provenance = {
            "local_files_only": True,
            "model": identities["model"],
            "voices": identities["voices"],
            "checksums": {
                **pins,
                "source": str(Path(os.environ.get("GENAID_MODEL_SHA256_FILE", "").strip() or model.parent / "SHA256SUMS").resolve())
                if pins["verified"] else None,
                "root": str(model.parent),
            },
            "packages": {name: _package_version(name) for name in ("kokoro-onnx", "onnxruntime")},
        }

    def validate_loaded_assets(self, model_path: Path, voices_path: Path) -> None:
        model, voices = resolve_model_paths(model_path, voices_path)
        for label, path in (("model", model), ("voices", voices)):
            if not path.samefile(self.provenance[label]["path"]) or sha256_file(path) != self.provenance[label]["sha256"]:
                raise ValueError("Loaded Kokoro assets differ from the configured request; unload and reload the engine")

    def validate_assignment(self, assignment: VoiceAssignment) -> None:
        if assignment.voice not in self._kokoro.voices:
            raise ValueError(f"kokoro-onnx voice {assignment.voice!r} is not in the loaded voices pack")

    def generate(self, text: str, assignment: VoiceAssignment) -> tuple[np.ndarray, int]:
        try:
            return self._kokoro.create(
                text, voice=assignment.voice, speed=assignment.speed, lang=assignment.lang,
            )
        except AssertionError as exc:
            raise ValueError(f"kokoro-onnx rejected voice {assignment.voice!r}: {exc}") from exc

    def synthesize_turns(
        self, turns: list[Turn], cast: CastMap, *, gap_s: float = DEFAULT_TURN_GAP_S,
        improve_publish: bool = False, max_duration_s: float | None = None,
        generation_timeout_s: float | None = None,
    ) -> SynthResult:
        validate_request(turns, cast, self.engine_id, gap_s, max_duration_s)
        timeout = max_generation_seconds(generation_timeout_s)
        deadline = time.monotonic() + timeout
        if not _MODEL_LOCK.acquire(timeout=timeout):
            raise TimeoutError("speech generation timed out waiting for the model")
        try:
            return synthesize_turns(
                self, turns, cast, gap_s=gap_s, improve_publish=improve_publish,
                max_duration_s=max_duration_s, generation_timeout_s=timeout, deadline=deadline,
            )
        finally:
            _MODEL_LOCK.release()
