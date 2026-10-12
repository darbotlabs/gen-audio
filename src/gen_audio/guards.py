"""Request, path, and artifact guards for GenAID Audio.

This module stays importable without torch, Ray, or FastAPI so the limits can
be unit-tested on CPU-only dev boxes (including Windows).
"""

from __future__ import annotations

import hashlib
import math
import os
import re
import stat
import tempfile
from pathlib import Path
from typing import Callable

# Hard ceilings. Environment variables may tighten these, not raise them.
HARD_MAX_TTS_TEXT_CHARS = 8_000
HARD_MAX_SEGMENT_CHARS = 8_000
HARD_MAX_TOTAL_SCRIPT_CHARS = 20_000
HARD_MAX_SCRIPT_SEGMENTS = 128
HARD_MAX_PROMPT_CHARS = 8_000
DEFAULT_MAX_SEGMENT_CHARS = 4_000
DEFAULT_MAX_SCRIPT_SEGMENTS = 64
DEFAULT_VLLM_TIMEOUT_S = 120.0
MAX_VLLM_TIMEOUT_S = 600.0
HARD_MAX_AUDIO_DURATION_S = 600.0
HARD_MAX_AUDIO_SAMPLES = 14_400_000
HARD_MAX_FILE_BYTES = 256 * 1024 * 1024
HARD_MAX_TEXT_FILE_BYTES = 256 * 1024
HARD_MAX_MODEL_FILE_BYTES = 2 * 1024 * 1024 * 1024
HARD_MAX_GENERATION_SECONDS = 600.0

_OUTPUT_FORMATS = {"wav", "mp3"}
_VOICE_SUFFIXES = {".wav"}
_FILENAME_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")
_SHA256_RE = re.compile(r"^[0-9a-fA-F]{64}$")


class RequestLimitError(ValueError):
    """Input exceeded a configured GenAID Audio limit."""


def _positive_int(name: str, default: int, ceiling: int) -> int:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError as exc:
        raise ValueError(f"{name} must be a positive integer") from exc
    if value < 1:
        raise ValueError(f"{name} must be a positive integer")
    return min(value, ceiling)


def max_audio_samples() -> int:
    """14.4M mono samples maximum; GENAID_MAX_AUDIO_SAMPLES may only tighten."""
    return _positive_int("GENAID_MAX_AUDIO_SAMPLES", HARD_MAX_AUDIO_SAMPLES, HARD_MAX_AUDIO_SAMPLES)


def max_audio_file_bytes() -> int:
    """256 MiB encoded audio maximum; GENAID_MAX_AUDIO_FILE_BYTES may tighten."""
    return _positive_int("GENAID_MAX_AUDIO_FILE_BYTES", HARD_MAX_FILE_BYTES, HARD_MAX_FILE_BYTES)


def max_audio_duration(requested: float | None = None) -> float:
    """Ten minutes maximum, tightened by GENAID_MAX_AUDIO_DURATION_S and callers."""
    configured = os.environ.get("GENAID_MAX_AUDIO_DURATION_S", "").strip()
    cap = HARD_MAX_AUDIO_DURATION_S
    for raw in (configured or None, requested):
        if raw is not None:
            value = float(raw)
            if not math.isfinite(value) or value <= 0:
                raise ValueError("maximum audio duration must be finite and positive")
            cap = min(cap, value)
    return cap


def max_generation_seconds(requested: float | None = None) -> float:
    """Cooperative inference deadline; subprocess hosts must enforce hard cancellation."""
    cap = HARD_MAX_GENERATION_SECONDS
    for raw in (os.environ.get("GENAID_MAX_GENERATION_SECONDS", "").strip() or None, requested):
        if raw is not None:
            value = float(raw)
            if not math.isfinite(value) or value <= 0:
                raise ValueError("generation timeout must be finite and positive")
            cap = min(cap, value)
    return cap


def read_bounded_bytes(path: str | Path, limit: int) -> bytes:
    """Read at most limit+1 bytes; refuse devices, pipes, and oversized files."""
    if not stat.S_ISREG(Path(path).stat().st_mode):
        raise ValueError("input must be a regular file")
    with open(path, "rb") as handle:
        metadata = os.fstat(handle.fileno())
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError("input must be a regular file")
        if metadata.st_size > limit:
            raise RequestLimitError(f"input file exceeds {limit} bytes")
        data = handle.read(limit + 1)
    if len(data) > limit:
        raise RequestLimitError(f"input file exceeds {limit} bytes")
    return data


def read_bounded_text(path: str | Path, *, encoding: str = "utf-8", limit: int | None = None) -> str:
    """UTF-8 input capped at 256 KiB, tighten with GENAID_MAX_TEXT_FILE_BYTES."""
    cap = _positive_int(
        "GENAID_MAX_TEXT_FILE_BYTES", HARD_MAX_TEXT_FILE_BYTES, HARD_MAX_TEXT_FILE_BYTES
    )
    return read_bounded_bytes(path, min(cap, limit) if limit is not None else cap).decode(encoding)


def max_tts_text_chars() -> int:
    return _positive_int("GENAID_MAX_TTS_TEXT_CHARS", HARD_MAX_TTS_TEXT_CHARS, HARD_MAX_TTS_TEXT_CHARS)


def max_segment_chars() -> int:
    return _positive_int("GENAID_MAX_SEGMENT_CHARS", DEFAULT_MAX_SEGMENT_CHARS, HARD_MAX_SEGMENT_CHARS)


def max_total_script_chars() -> int:
    return _positive_int(
        "GENAID_MAX_TOTAL_SCRIPT_CHARS",
        HARD_MAX_TOTAL_SCRIPT_CHARS,
        HARD_MAX_TOTAL_SCRIPT_CHARS,
    )


def max_script_segments() -> int:
    return _positive_int("GENAID_MAX_SCRIPT_SEGMENTS", DEFAULT_MAX_SCRIPT_SEGMENTS, HARD_MAX_SCRIPT_SEGMENTS)


def max_prompt_chars() -> int:
    return _positive_int("GENAID_MAX_PROMPT_CHARS", HARD_MAX_PROMPT_CHARS, HARD_MAX_PROMPT_CHARS)


def limit_for(kind: str) -> int:
    if kind == "segment":
        return max_segment_chars()
    if kind in {"prompt", "topic", "enhance"}:
        return max_prompt_chars()
    return max_tts_text_chars()


def enforce_text_limit(value: str, kind: str = "tts") -> str:
    if not isinstance(value, str):
        raise RequestLimitError(f"{kind} must be a string")
    if "\x00" in value:
        raise RequestLimitError(f"{kind} contains a NUL byte")
    ceiling = limit_for(kind)
    if len(value) > ceiling:
        raise RequestLimitError(f"{kind} exceeds {ceiling} characters")
    return value


def normalize_output_format(fmt: str) -> str:
    if not isinstance(fmt, str):
        raise ValueError("output_format must be a string")
    normalized = fmt.strip().lower()
    if normalized not in _OUTPUT_FORMATS:
        raise ValueError("output_format must be 'wav' or 'mp3'")
    return normalized


def reject_voice_prompt_reference(path: str | None) -> str | None:
    """Reject URL and traversal references before a request is proxied.

    Absolute paths are left for the TTS process, which confines them to the
    configured voices directory. This helper does not know that directory.
    """
    if path is None:
        return None
    if not isinstance(path, str):
        raise ValueError("voice_prompt_path must be a string")
    stripped = path.strip()
    if not stripped:
        return None
    if "\x00" in stripped:
        raise ValueError("voice_prompt_path contains a NUL byte")
    lowered = stripped.replace("\\", "/").lower()
    if "://" in lowered or lowered.startswith("file:"):
        raise ValueError("voice_prompt_path must be a local file, not a URL")
    parts = re.split(r"[\\/]", stripped)
    if any(part == ".." for part in parts):
        raise ValueError("voice_prompt_path must not contain '..'")
    return stripped


def _is_within(child: str, root: str) -> bool:
    child_real = os.path.normcase(os.path.realpath(child))
    root_real = os.path.normcase(os.path.realpath(root))
    if child_real == root_real:
        return True
    try:
        return os.path.commonpath([child_real, root_real]) == root_real
    except ValueError:
        return False


def resolve_voice_file(path: str, voices_root: str) -> str:
    """Return a real .wav path that stays inside the voices directory."""
    if not isinstance(path, str) or not path.strip():
        raise ValueError("voice prompt path is empty")
    if "\x00" in path:
        raise ValueError("voice prompt path contains a NUL byte")
    lowered = path.replace("\\", "/").lower()
    if "://" in lowered or lowered.startswith("file:"):
        raise ValueError("voice prompt path must be a local file, not a URL")
    if not voices_root or not os.path.isdir(voices_root):
        raise ValueError("voices directory is not available")
    candidate = path if os.path.isabs(path) else os.path.join(voices_root, path)
    real = os.path.realpath(candidate)
    if not _is_within(real, voices_root) or not os.path.isfile(real):
        raise ValueError("voice prompt must be an existing file inside the voices directory")
    if os.path.splitext(real)[1].lower() not in _VOICE_SUFFIXES:
        raise ValueError("voice prompt must be a .wav file inside the voices directory")
    return real


def resolve_model_directory(path: str, model_base: str) -> str:
    """Return a real model directory that stays inside the model base."""
    if not isinstance(path, str) or not path.strip():
        raise ValueError("model path is empty")
    if "\x00" in path:
        raise ValueError("model path contains a NUL byte")
    if not model_base:
        raise ValueError("model base is not configured")
    real_base = os.path.realpath(model_base)
    candidate = path if os.path.isabs(path) else os.path.join(real_base, path)
    real = os.path.realpath(candidate)
    if not _is_within(real, real_base):
        raise ValueError("model_path must stay inside GENAID_MODEL_BASE")
    if not os.path.isdir(real):
        raise FileNotFoundError(f"Model path not found: {real}")
    return real


def local_files_only_enabled() -> bool:
    return os.environ.get("GENAID_LOCAL_FILES_ONLY", "").strip().lower() in {"1", "true", "yes", "on"}


def vllm_timeout_seconds() -> float:
    raw = os.environ.get("GENAID_VLLM_TIMEOUT_S", "").strip()
    if not raw:
        return DEFAULT_VLLM_TIMEOUT_S
    try:
        value = float(raw)
    except ValueError:
        return DEFAULT_VLLM_TIMEOUT_S
    if not math.isfinite(value) or value <= 0:
        return DEFAULT_VLLM_TIMEOUT_S
    return min(value, MAX_VLLM_TIMEOUT_S)


def validate_script_segments(script: object) -> None:
    if not isinstance(script, list) or not script:
        raise RequestLimitError("script must be a non-empty list")
    segment_cap = max_script_segments()
    if len(script) > segment_cap:
        raise RequestLimitError(f"script exceeds {segment_cap} segments")
    total = 0
    per_segment = max_segment_chars()
    for index, item in enumerate(script):
        if not isinstance(item, dict):
            raise RequestLimitError(f"script[{index}] must be an object with speaker and text")
        speaker = str(item.get("speaker", "")).strip()
        text = item.get("text", "")
        if not isinstance(text, str):
            raise RequestLimitError(f"script[{index}].text must be a string")
        if not speaker or not text.strip():
            raise RequestLimitError(f"script[{index}] needs speaker and text")
        if "\x00" in text:
            raise RequestLimitError(f"script[{index}].text contains a NUL byte")
        if len(speaker) > 128 or "\x00" in speaker:
            raise RequestLimitError(f"script[{index}].speaker is invalid")
        if len(text) > per_segment:
            raise RequestLimitError(f"script[{index}].text exceeds {per_segment} characters")
        total += len(text)
    total_cap = max_total_script_chars()
    if total > total_cap:
        raise RequestLimitError(f"script exceeds {total_cap} characters")


def safe_storage_name(filename: str) -> str:
    if not isinstance(filename, str):
        raise ValueError("filename must be a string")
    base = os.path.basename(filename.replace("\\", "/"))
    if not _FILENAME_RE.fullmatch(base):
        raise ValueError("unsafe output filename")
    return base


def safe_attachment_filename(filename: str, default_format: str = "wav") -> str:
    try:
        return safe_storage_name(filename)
    except ValueError:
        fmt = normalize_output_format(default_format)
        return f"audio.{fmt}"


def sanitize_header_value(value: str, limit: int = 500) -> str:
    if not isinstance(value, str):
        value = str(value)
    cleaned = "".join(ch for ch in value if ch not in "\r\n\x00")
    if len(cleaned) > limit:
        return cleaned[:limit]
    return cleaned


def materialize_temp_audio(output_dir: str, suffix: str, write_fn: Callable[[str], None]) -> bytes:
    """Write audio to a closed temp file and always delete it.

    The file is created with ``mkstemp`` and the descriptor is closed before
    ``write_fn`` runs. ``NamedTemporaryFile`` keeps the handle open, and
    soundfile then fails on Windows with a sharing violation.
    """
    if suffix not in {".wav", ".mp3"}:
        raise ValueError("unsafe audio suffix")
    os.makedirs(output_dir, exist_ok=True)
    fd, path = tempfile.mkstemp(prefix="genaid-", suffix=suffix, dir=output_dir)
    os.close(fd)
    try:
        write_fn(path)
        return read_bounded_bytes(path, max_audio_file_bytes())
    finally:
        Path(path).unlink(missing_ok=True)


def write_bytes_exclusive(directory: str, filename: str, payload: bytes) -> str:
    """Atomically write ``payload`` under ``directory`` and return the real path."""
    if not isinstance(payload, (bytes, bytearray)):
        raise ValueError("payload must be bytes")
    name = safe_storage_name(filename)
    root = os.path.realpath(directory)
    os.makedirs(root, exist_ok=True)
    destination = os.path.join(root, name)
    if not _is_within(destination, root):
        raise ValueError("output path escapes the output directory")
    if len(payload) > max_audio_file_bytes():
        raise RequestLimitError("audio payload exceeds byte limit")
    from gen_audio.artifacts import staged_files
    with staged_files([Path(destination)]) as staged:
        staged[0].write_bytes(payload)
    return os.path.realpath(destination)


def _checksum_file() -> str | None:
    configured = os.environ.get("GENAID_MODEL_SHA256_FILE", "").strip()
    return configured or None


def checksums_required() -> bool:
    value = os.environ.get("GENAID_REQUIRE_MODEL_CHECKSUM", "").strip().lower()
    if value not in {"", "0", "false", "no", "off", "1", "true", "yes", "on"}:
        raise ValueError("GENAID_REQUIRE_MODEL_CHECKSUM must be a boolean")
    return value in {"1", "true", "yes", "on"}


def verify_model_checksums(
    model_dir: str, *, required_files: tuple[str | Path, ...] = ()
) -> dict[str, int | bool]:
    """Verify a SHA256SUMS pin when one is configured or required.

    With no pin file and ``GENAID_REQUIRE_MODEL_CHECKSUM`` unset, this is a
    no-op so existing staged models keep loading. When a pin is present, every
    listed relative file must match. If supplied, every required_file must be
    covered by that pin, even when the pin is optional. Observed hashes are
    never used as trusted expected hashes. Filenames cannot escape model_dir.
    """
    required = checksums_required()
    pin = _checksum_file()
    if pin is None:
        sibling = os.path.join(model_dir, "SHA256SUMS")
        if os.path.isfile(sibling):
            pin = sibling
    if pin is None:
        if required:
            raise ValueError("GENAID_REQUIRE_MODEL_CHECKSUM is set but no SHA256SUMS pin was found")
        return {"verified": False, "skipped": True, "files": 0}
    if not os.path.isfile(pin):
        raise ValueError(f"model checksum pin not found: {pin}")
    pin_real = os.path.realpath(pin)
    # An explicit pin may live outside the model dir (a release manifest).
    # Entries inside it are still confined to model_dir.
    lines = _read_pin_lines(pin_real)
    if not lines:
        raise ValueError("model checksum pin is empty")
    checked = 0
    covered: set[str] = set()
    for digest, relative in lines:
        relative = relative.replace("\\", "/")
        if not relative or any(part in {"", ".", ".."} for part in relative.split("/")):
            raise ValueError("checksum pin contains an unsafe path")
        if os.path.isabs(relative) or ":" in relative:
            raise ValueError("checksum pin contains an absolute path")
        target = os.path.realpath(os.path.join(model_dir, *relative.split("/")))
        if not _is_within(target, model_dir) or not os.path.isfile(target):
            raise ValueError("checksum pin names a file outside the model directory")
        key = os.path.normcase(target)
        if key in covered:
            raise ValueError("checksum pin contains a duplicate path")
        if sha256_file(target) != digest.lower():
            raise ValueError(f"checksum mismatch for {relative}")
        covered.add(key)
        checked += 1
    if any(os.path.normcase(os.path.realpath(path)) not in covered for path in required_files):
        raise ValueError("checksum pin must cover every loaded model and voices file")
    return {"verified": True, "skipped": False, "files": checked}


def _read_pin_lines(path: str) -> list[tuple[str, str]]:
    entries: list[tuple[str, str]] = []
    for raw in read_bounded_text(path, limit=64 * 1024).splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(maxsplit=1)
        if len(parts) != 2 or not _SHA256_RE.fullmatch(parts[0]):
            raise ValueError("checksum pin lines must be '<sha256> <relative-path>'")
        filename = parts[1][1:] if parts[1].startswith("*") else parts[1]
        entries.append((parts[0], filename))
    return entries


def sha256_file(path: str | Path, *, max_bytes: int = HARD_MAX_MODEL_FILE_BYTES) -> str:
    """Hash a regular local file incrementally (also used for artifact provenance)."""
    if not stat.S_ISREG(Path(path).stat().st_mode):
        raise ValueError("hash input must be a regular file")
    hasher = hashlib.sha256()
    with open(path, "rb") as handle:
        metadata = os.fstat(handle.fileno())
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError("hash input must be a regular file")
        if metadata.st_size > max_bytes:
            raise RequestLimitError("hash input exceeds file byte limit")
        total = 0
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            total += len(chunk)
            if total > max_bytes:
                raise RequestLimitError("hash input exceeds file byte limit")
            hasher.update(chunk)
        after = os.fstat(handle.fileno())
        if total != metadata.st_size or after.st_mtime_ns != metadata.st_mtime_ns:
            raise ValueError("file changed while hashing")
    return hasher.hexdigest()


def classify_upstream_failure(exc: BaseException) -> tuple[int, str]:
    """Map a proxied TTS failure to an HTTP status without hiding the class.

    Connection failures stay 503. Timeouts are 504 because the backend actor
    is not cancelled. Upstream 4xx is forwarded. Other upstream HTTP failures
    become 502.
    """
    response = getattr(exc, "response", None)
    status = getattr(response, "status_code", None)
    if isinstance(status, int):
        detail = _short_upstream_detail(response, status)
        if 400 <= status < 500:
            return status, detail
        return 502, detail
    if "timeout" in type(exc).__name__.lower():
        return 504, (
            "GenAID audio request timed out. "
            "The model actor is not cancelled and may still be generating."
        )
    name = type(exc).__name__.lower()
    if name in {"connecterror", "networkerror", "remoteprotocolerror", "requesterror"} or "connect" in name:
        return 503, "GenAID audio backend is unreachable."
    return 502, f"GenAID audio request failed ({type(exc).__name__})."


def _short_upstream_detail(response: object, status: int) -> str:
    content_type = ""
    headers = getattr(response, "headers", None)
    if headers is not None:
        try:
            content_type = str(headers.get("content-type", ""))
        except Exception:
            content_type = ""
    text = ""
    if not content_type or "json" in content_type or content_type.startswith("text/"):
        try:
            text = str(getattr(response, "text", "") or "")
        except Exception:
            text = ""
    snippet = " ".join(text.split())[:300]
    if snippet:
        return f"upstream {status}: {snippet}"
    return f"upstream status {status}"


def task_cause(exc: BaseException) -> BaseException:
    """Unwrap a Ray task error when the ``cause`` attribute is present."""
    cause = getattr(exc, "cause", None)
    if isinstance(cause, BaseException):
        return cause
    return exc
