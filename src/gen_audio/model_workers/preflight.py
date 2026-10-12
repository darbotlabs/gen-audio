"""Check local assets, pinned runtimes and available RAM without importing torch."""

from __future__ import annotations

import ctypes
import importlib.metadata
import json
import math
import os
from pathlib import Path
import re
import struct
import sys
import wave

from .contracts import (
    GIB, MAX_INPUT_TOKENS, MAX_JSON_BYTES, MAX_NEW_TOKENS, MAX_REFERENCE_SECONDS,
    MAX_SECONDS, MAX_TEXT_BYTES, MAX_TEXT_CHARS, Request, SAMPLE_RATE, WorkerError,
    runtime_spec,
)
from .checkpoints import combine_headers, validate_shapes, vibevoice_architecture


def _unique_object(pairs: list) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise WorkerError("invalid_asset", f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def _reject_constant(value: str):
    raise WorkerError("invalid_asset", f"Nonfinite JSON constant: {value}")


def read_json(path: Path) -> dict:
    with path.open("rb") as handle:
        raw = handle.read(MAX_JSON_BYTES + 1)
    if len(raw) > MAX_JSON_BYTES:
        raise WorkerError("invalid_asset", f"JSON exceeds bounded size: {path}")
    value = json.loads(raw, object_pairs_hook=_unique_object, parse_constant=_reject_constant)
    if not isinstance(value, dict):
        raise WorkerError("invalid_asset", f"Expected a JSON object: {path}")
    return value


def local_file(path: Path, *, max_bytes: int | None = None) -> Path:
    path = path.resolve()
    if not path.is_file() or path.stat().st_size == 0:
        raise WorkerError("missing_asset", f"Missing or empty local file: {path}")
    if max_bytes is not None and path.stat().st_size > max_bytes:
        raise WorkerError("resource_limit", f"File exceeds {max_bytes} bytes: {path}")
    return path


def contained_file(root: Path, name: str) -> Path:
    if not isinstance(name, str) or not name or Path(name).is_absolute():
        raise WorkerError("invalid_asset", "Asset index contains an invalid local path")
    path = (root / name).resolve()
    if not path.is_relative_to(root.resolve()):
        raise WorkerError("invalid_asset", "Asset index escapes the model directory")
    return local_file(path)


def safetensors_header(path: Path) -> dict:
    """Read bounded metadata only; reject LFS pointers and truncated shards."""
    size = path.stat().st_size
    with path.open("rb") as handle:
        raw = handle.read(8)
        if len(raw) != 8:
            raise WorkerError("invalid_weights", f"Truncated safetensors header: {path}")
        length = struct.unpack("<Q", raw)[0]
        if not 2 <= length <= MAX_JSON_BYTES or length + 8 > size:
            raise WorkerError("invalid_weights", f"Invalid safetensors header (or LFS pointer): {path}")
        header = json.loads(handle.read(length), object_pairs_hook=_unique_object,
                            parse_constant=_reject_constant)
    if not isinstance(header, dict):
        raise WorkerError("invalid_weights", f"Invalid safetensors object: {path}")
    widths = {"F64": 8, "F32": 4, "F16": 2, "BF16": 2, "I64": 8,
              "I32": 4, "I16": 2, "I8": 1, "U8": 1, "BOOL": 1}
    intervals = []
    fp32_bytes = 0
    tensors = {}
    for name, entry in header.items():
        if name == "__metadata__":
            continue
        if not isinstance(entry, dict):
            raise WorkerError("invalid_weights", f"Invalid tensor metadata: {path}")
        shape, offsets, dtype = entry.get("shape"), entry.get("data_offsets"), entry.get("dtype")
        if (not isinstance(shape, list) or len(shape) > 8
                or any(type(n) is not int or n < 0 for n in shape)
                or not isinstance(offsets, list) or len(offsets) != 2
                or any(type(n) is not int or n < 0 for n in offsets)
                or dtype not in widths):
            raise WorkerError("invalid_weights", f"Unsupported tensor metadata: {path}")
        elements = math.prod(shape)
        if offsets[1] - offsets[0] != elements * widths[dtype]:
            raise WorkerError("invalid_weights", f"Inconsistent tensor size: {path}")
        intervals.append(tuple(offsets))
        fp32_bytes += elements * max(4, widths[dtype])
        tensors[name] = entry
    end = 0
    for start, stop in sorted(intervals):
        if start != end:
            raise WorkerError("invalid_weights", f"Overlapping or sparse tensor offsets: {path}")
        end = stop
    if not tensors or end != size - length - 8:
        raise WorkerError("invalid_weights", f"Truncated or inconsistent checkpoint: {path}")
    return {"path": str(path), "bytes": size, "fp32_bytes": fp32_bytes, "tensors": tensors}


def memory_snapshot() -> dict:
    if os.name == "nt":
        class Status(ctypes.Structure):
            _fields_ = [
                ("length", ctypes.c_ulong), ("load", ctypes.c_ulong),
                ("total", ctypes.c_ulonglong), ("available", ctypes.c_ulonglong),
                ("total_page", ctypes.c_ulonglong), ("available_page", ctypes.c_ulonglong),
                ("total_virtual", ctypes.c_ulonglong), ("available_virtual", ctypes.c_ulonglong),
                ("extended_virtual", ctypes.c_ulonglong),
            ]
        status = Status()
        status.length = ctypes.sizeof(Status)
        if not ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
            raise OSError("GlobalMemoryStatusEx failed")
        # A unified-memory machine can reserve most physical RAM for its GPU.
        # CPU allocation guards must use OS-visible RAM, while diagnostics must
        # not confuse that budget with installed memory or accelerator capacity.
        installed_kib = ctypes.c_ulonglong()
        installed = ctypes.windll.kernel32.GetPhysicallyInstalledSystemMemory(ctypes.byref(installed_kib))
        return {"total_bytes": status.total, "available_bytes": status.available,
                "budget_scope": "cpu", "installed_bytes": installed_kib.value * 1024 if installed else None,
                "hardware_reserved_bytes": max(0, installed_kib.value * 1024 - status.total) if installed else None}
    if sys.platform.startswith("linux"):
        data = {}
        for line in Path("/proc/meminfo").read_text().splitlines():
            key, value = line.split(":", 1)
            data[key] = int(value.split()[0]) * 1024
        return {"total_bytes": data["MemTotal"], "available_bytes": data["MemAvailable"]}
    raise WorkerError("memory_unknown", "Cannot safely determine physical memory on this OS")


def check_memory(required_bytes: int, snapshot: dict | None = None) -> dict:
    snapshot = memory_snapshot() if snapshot is None else snapshot
    available, total = snapshot["available_bytes"], snapshot["total_bytes"]
    if (type(available) is not int or type(total) is not int
            or not 0 <= available <= total or total <= 0):
        raise WorkerError("memory_unknown", "Invalid physical memory counters; refusing to load")
    safe = min(available - 2 * GIB, int(total * 0.85))
    return {
        **snapshot, "required_bytes": required_bytes,
        "reserve_bytes": 2 * GIB, "safe_budget_bytes": max(0, safe),
        "sufficient": safe >= required_bytes,
        "estimate_basis": "Conservative architecture FP32 allocation + checkpoint load overlap + bounded KV/activation allowance",
    }


def require_memory(required_bytes: int) -> None:
    report = check_memory(required_bytes)
    if not report["sufficient"]:
        raise WorkerError(
            "insufficient_memory",
            f"Requires {required_bytes / GIB:.2f} GiB plus 2 GiB reserve; "
            f"only {report['available_bytes'] / GIB:.2f} GiB available. No model was loaded.",
        )


def inspect_wav(path: Path, *, seconds: float, minimum: float = 0.0) -> dict:
    path = local_file(path, max_bytes=int(SAMPLE_RATE * seconds * 2) + 65536)
    if path.suffix.lower() != ".wav":
        raise WorkerError(
            "unsupported_voice_format",
            "Use raw mono 24 kHz PCM16 WAV audio. Pickled .pt embeddings are never loaded, "
            "including bundled PersonaPlex presets whose trust cannot be verified here.",
        )
    try:
        with wave.open(str(path), "rb") as handle:
            rate, count = handle.getframerate(), handle.getnframes()
            if (rate != SAMPLE_RATE or handle.getnchannels() != 1
                    or handle.getsampwidth() != 2 or handle.getcomptype() != "NONE"):
                raise WorkerError("unsupported_audio", f"Expected mono 24 kHz PCM16 WAV: {path}")
            if count <= 0 or count > seconds * rate or count < minimum * rate:
                raise WorkerError("resource_limit", f"WAV duration must be {minimum:g}..{seconds:g}s: {path}")
            # Read only a few MB at the hard duration cap, never trust nframes alone.
            if len(handle.readframes(count)) != count * 2:
                raise WorkerError("invalid_audio", f"Truncated WAV: {path}")
    except (wave.Error, EOFError) as exc:
        raise WorkerError("invalid_audio", f"Invalid PCM WAV: {path}: {exc}") from exc
    return {"path": str(path), "sample_rate": rate, "samples": count, "duration_s": count / rate}


def normalized_script(text: str, voices: int) -> str:
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines:
        raise WorkerError("missing_text", "Provide --text or --input-text")
    matches = [re.fullmatch(r"Speaker\s+([1-4]):\s*(\S.*)", line, re.IGNORECASE) for line in lines]
    if not any(matches):
        if voices != 1:
            raise WorkerError("invalid_script", "Plain text requires exactly one --voice-ref")
        if any(re.match(r"Speaker\s+\d+\s*:", line, re.IGNORECASE) for line in lines):
            raise WorkerError("invalid_script", "Speaker IDs must be contiguous 1..4 with nonempty text")
        return "Speaker 1: " + " ".join(lines)
    if not all(matches):
        raise WorkerError("invalid_script", "Every script line must be 'Speaker N: text'; no lines are discarded")
    ids = {int(match[1]) for match in matches}
    if ids != set(range(1, voices + 1)):
        raise WorkerError("invalid_script", "Use contiguous Speaker 1..N and one --voice-ref per speaker in ID order")
    return "\n".join(f"Speaker {int(match[1])}: {match[2]}" for match in matches)


def validate_request(request: Request, *, load_only: bool = False) -> dict:
    runtime_spec(request.engine)
    from .execution import preferences
    preferences(request)
    if type(request.max_new_tokens) is not int or not 1 <= request.max_new_tokens <= MAX_NEW_TOKENS:
        raise WorkerError("resource_limit", f"max-new-tokens must be 1..{MAX_NEW_TOKENS}")
    for value, cap, name in (
        (request.max_seconds, MAX_SECONDS, "max-seconds"),
        (request.timeout_seconds, 3600, "timeout-seconds"),
    ):
        if isinstance(value, bool) or not math.isfinite(value) or not 0 < value <= cap:
            raise WorkerError("resource_limit", f"{name} must be finite, positive and <= {cap}")
    if request.max_seconds < 1:
        raise WorkerError("resource_limit", "max-seconds must be at least 1")
    if type(request.seed) is not int or not 0 <= request.seed <= 2**32 - 1:
        raise WorkerError("resource_limit", "seed must be an integer in 0..2**32-1")
    for text in (request.text, request.persona):
        if text is not None and (len(text) > MAX_TEXT_CHARS or len(text.encode("utf-8")) > MAX_TEXT_BYTES):
            raise WorkerError("resource_limit", "Text/persona exceeds the 8192-character/32768-byte limit")
    if load_only:
        return {}
    if request.engine == "vibevoice":
        if request.input_audio is not None or request.persona is not None:
            raise WorkerError("incompatible_capability", "VibeVoice takes script text and WAV voice references, not conversation input")
        if not request.text or not request.text.strip():
            raise WorkerError("missing_text", "VibeVoice requires --text or --input-text")
        if not 1 <= len(request.voice_refs) <= 4:
            raise WorkerError("missing_voice", "VibeVoice requires 1..4 explicit WAV --voice-ref paths")
        return {"script": normalized_script(request.text, len(request.voice_refs))}
    if request.text is not None:
        raise WorkerError(
            "incompatible_capability",
            "PersonaPlex is speech-to-speech, not verbatim script TTS. Use --input-audio and --persona.",
        )
    if request.input_audio is None or not request.persona or not request.persona.strip():
        raise WorkerError("missing_conversation_input", "PersonaPlex requires both --input-audio and --persona")
    if len(request.voice_refs) != 1:
        raise WorkerError("missing_voice", "PersonaPlex requires one raw WAV --voice-ref; unsafe .pt presets are unsupported")
    return {}


def dependency_report(spec: dict) -> tuple[dict, list[dict]]:
    installed, errors = {}, []
    if sys.version_info[:2] != (3, 12) or struct.calcsize("P") != 8:
        errors.append({"code": "incompatible_python", "message": "Run this backend in isolated 64-bit Python 3.12, not the Python 3.14 app environment"})
    for package, expected in spec["versions"].items():
        try:
            distribution = importlib.metadata.distribution(package)
            installed[package] = distribution.version
            # The original CPU builds remain supported. The isolated ROCm
            # profile upgrades torch without relaxing other dependency pins.
            accelerated = package == "torch" and distribution.version == "2.9.1+rocm7.2.1"
            compatible = distribution.version == expected if "+" in expected else distribution.version.split("+")[0] == expected
            if not compatible and not accelerated:
                errors.append({"code": "incompatible_dependency", "message": f"{package}=={expected} required; found {distribution.version}"})
            port = ({"vibevoice": "vibevoice", "datasets": "datasets"}
                    if spec.get("profile") == "vibevoice-modern" else
                    {"moshi-personaplex": "personaplex"}
                    if spec.get("profile") == "personaplex-modern" else {}).get(package)
            if port:
                from .profiles import verify_port
                verify_port(distribution, port)
            elif package == spec["distribution"]:
                raw = distribution.read_text("direct_url.json")
                direct = json.loads(raw) if raw else {}
                vcs = direct.get("vcs_info", {})
                url = direct.get("url", "").removesuffix(".git").lower()
                if (url != spec["repository"].lower()
                        or vcs.get("vcs") != "git" or vcs.get("commit_id") != spec["revision"]):
                    errors.append({"code": "unverified_runtime", "message": f"{package} must be installed from pinned Git revision {spec['revision']}"})
        except importlib.metadata.PackageNotFoundError:
            installed[package] = None
            errors.append({"code": "missing_dependency", "message": f"Missing {package}=={expected}"})
        except (ValueError, TypeError, AttributeError, OSError, KeyError):
            errors.append({"code": "unverified_runtime", "message": f"Invalid installation provenance for {package}"})
    return installed, errors


def preflight(request: Request, *, load_only: bool = False, resident: bool = False) -> dict:
    errors: list[dict] = []
    from .profiles import installed_spec
    spec = installed_spec(runtime_spec(request.engine))
    report = {
        "schema_version": "1.0", "ok": False, "ready": False, "preflight": True,
        "synthesizedSpeech": False, "engine": request.engine, "capability": spec["capability"],
        "device": request.device, "dtype": "float32", "runtime": spec,
        "inference_validated": False, "errors": errors, "assets": {},
    }

    def check(operation):
        try:
            return operation()
        except (WorkerError, OSError, ValueError, TypeError, KeyError) as exc:
            errors.append({"code": getattr(exc, "code", "invalid_asset"), "message": str(exc)})
            return None

    check(lambda: validate_request(request, load_only=load_only))
    report["dependencies"], dependency_errors = dependency_report(spec)
    errors.extend(dependency_errors)
    from .execution import preferences, resolve, gpu_memory, component_policy
    root = request.model_dir.resolve()
    report["model_dir"] = str(root)
    assets = report["assets"]
    weights = []
    architecture = None
    model_precision = None
    if request.engine == "vibevoice":
        config = check(lambda: read_json(local_file(root / "config.json")))
        if config is not None:
            model_precision = config.get("torch_dtype")
            architecture = check(lambda: vibevoice_architecture(config))
            if architecture is not None:
                assets["config"] = config
                assets["architecture_fp32_bytes"] = architecture["fp32_bytes"]
        processor = check(lambda: read_json(local_file(root / "preprocessor_config.json")))
        if processor is not None:
            audio_config = processor.get("audio_processor", {})
            if (not isinstance(audio_config, dict)
                    or processor.get("speech_tok_compress_ratio", 3200) != 3200
                    or audio_config.get("sampling_rate", SAMPLE_RATE) != SAMPLE_RATE):
                errors.append({"code": "unsupported_model", "message": "Only 24 kHz/3200-sample VibeVoice checkpoints are supported"})
            else:
                assets["processor_config"] = processor
        index_path = root / "model.safetensors.index.json"
        if index_path.is_file() and (root / "model.safetensors").is_file():
            errors.append({"code": "ambiguous_weights", "message": "Keep either a monolithic model.safetensors or an indexed shard set, not both"})
        if index_path.is_file():
            index = check(lambda: read_json(index_path))
            mapping = index.get("weight_map") if index else None
            if not isinstance(mapping, dict) or not mapping:
                errors.append({"code": "invalid_weights", "message": "Missing or empty safetensors weight_map"})
            else:
                names = set()
                for name in mapping.values():
                    if not isinstance(name, str) or not name.endswith(".safetensors"):
                        errors.append({"code": "invalid_weights", "message": "Only local safetensors shards are allowed"})
                    else:
                        names.add(name)
                for name in sorted(names):
                    header = check(lambda name=name: safetensors_header(contained_file(root, name)))
                    if header:
                        weights.append(header)
                        expected_keys = {key for key, shard in mapping.items() if shard == name}
                        if expected_keys != set(header["tensors"]):
                            errors.append({"code": "invalid_weights", "message": f"Shard keys do not match index: {name}"})
        else:
            header = check(lambda: safetensors_header(local_file(root / "model.safetensors")))
            if header:
                weights.append(header)
        if architecture is not None:
            check(lambda: validate_shapes(combine_headers(weights), architecture["shapes"],
                                          architecture["aliases"]))
        report["checkpoint_validation"] = "static architecture headers only; runtime meta validation still required"
        tokenizer = (request.tokenizer or root / "tokenizer").resolve()
        assets["tokenizer"] = str(tokenizer)
        for name in ("tokenizer_config.json", "tokenizer.json", "vocab.json", "merges.txt"):
            check(lambda name=name: local_file(tokenizer / name, max_bytes=MAX_JSON_BYTES))
        report["tokenizer_note"] = "Provide an explicit compatible local Qwen2 tokenizer directory; no Hub fallback. A 1.5B snapshot may omit it."
    else:
        for name in ("model.safetensors", "tokenizer-e351c8d8-checkpoint125.safetensors"):
            header = check(lambda name=name: safetensors_header(local_file(root / name)))
            if header:
                weights.append(header)
        tokenizer = (request.tokenizer or root / "tokenizer_spm_32k_3.model").resolve()
        if check(lambda: local_file(tokenizer, max_bytes=MAX_JSON_BYTES)):
            assets["tokenizer"] = str(tokenizer)
        if request.input_audio is not None:
            info = check(lambda: inspect_wav(request.input_audio, seconds=min(MAX_SECONDS, request.max_seconds)))
            if info:
                assets["input_audio"] = info
                if math.ceil(info["samples"] / 1920) > request.max_new_tokens:
                    errors.append({"code": "resource_limit", "message": "Input audio needs more PersonaPlex stream steps than max-new-tokens; shorten the WAV or raise the cap"})
        report["voice_note"] = "Raw WAV only. Bundled .pt voices are deliberately not unpickled; no trusted pinned voice digest has been established."
    assets["voices"] = []
    for path in request.voice_refs[:4]:
        info = check(lambda path=path: inspect_wav(
            path, seconds=MAX_REFERENCE_SECONDS,
            minimum=0.5 if request.engine == "personaplex" else 0.0,
        ))
        if info:
            assets["voices"].append(info)
    if request.output is not None:
        from gen_audio.artifacts import check_destinations
        check(lambda: check_destinations([request.output, request.output.with_suffix(".synth.json")]))
        if request.output.suffix.lower() != ".wav":
            errors.append({"code": "invalid_output", "message": "Output must have .wav extension"})
    assets["weights"] = [{k: v for k, v in w.items() if k != "tensors"} for w in weights]
    report["execution"] = check(lambda: component_policy(
        resolve(preferences(request), model_precision=model_precision), request.engine))
    if report["execution"]:
        report["device"] = report["execution"]["torch_device"]
        report["dtype"] = report["execution"]["precision"]
        report["execution"]["precision_source"] = (
            "model_config" if model_precision and report["execution"]["requested"]["precision"] == "auto"
            else "requested" if report["execution"]["requested"]["precision"] != "auto" else "runtime_default")
    # The load-overlap term intentionally accounts for mapped checkpoint pages
    # becoming resident while BF16 weights are converted to FP32 on the CPU.
    fp32 = sum(w["fp32_bytes"] for w in weights)
    disk = sum(w["bytes"] for w in weights)
    kv = 0
    if architecture is not None:
        fp32 = max(fp32, architecture["fp32_bytes"])
        kv = architecture["kv_bytes_per_token"] * (MAX_INPUT_TOKENS + MAX_NEW_TOKENS)
    codec_fp32 = 0
    if request.engine == "personaplex" and len(weights) == 2:
        codec_fp32 = weights[1]["fp32_bytes"]  # Mimi retains FP32; no unused duplicate codec.
        kv = 2 * 32 * 4096 * 3000 * 4  # LM's fixed 3000-frame streaming K/V ring at FP32.
    staging = max((w["bytes"] for w in weights), default=0)
    if request.engine == "personaplex" and weights:
        # LM tensors are mapped/copied individually; Mimi still loads one
        # complete, much smaller checkpoint alongside its initialized module.
        staging = max(max((t["data_offsets"][1] - t["data_offsets"][0]
                           for t in weights[0]["tensors"].values()), default=0),
                      2 * codec_fp32)
    floor = 12 * GIB if request.engine == "vibevoice" else 40 * GIB
    required = kv + 6 * GIB if resident else max(floor, fp32 + disk + kv + 6 * GIB)
    execution = report["execution"]
    if execution and execution["device"] == "gpu":
        scale = 1 if execution["precision"] == "float32" else .5
        # Safetensors shards are mapped/staged one at a time. GPU allocations
        # and host staging must independently fit their available budgets.
        required = int((0 if resident else (fp32 - codec_fp32) * scale + codec_fp32) + kv * scale + 6 * GIB)
        host_required = GIB if resident else staging + GIB
        device_memory = check(lambda: gpu_memory(execution, required))
        host_memory = check(lambda: check_memory(host_required))
        report["memory"] = ({**device_memory, "cpu": host_memory,
            "sufficient": bool(device_memory["sufficient"] and host_memory and host_memory["sufficient"]),
            "estimate_basis": "Device weights at selected precision + KV/activation reserve; separate host shard staging budget"}
            if device_memory else None)
    elif execution and execution["precision"] == "bfloat16":
        required = int((0 if resident else (fp32 - codec_fp32) * .5 + codec_fp32 + staging) + kv * .5 + 2 * GIB)
        report["memory"] = check(lambda: check_memory(required))
        if report["memory"]:
            report["memory"]["estimate_basis"] = "BF16 language weights + FP32 codecs where required + largest shard overlap + bounded KV/activation reserve"
    else:
        report["memory"] = check(lambda: check_memory(required))
    report["weights_resident"] = resident
    if report["memory"] and not report["memory"]["sufficient"]:
        errors.append({
            "code": "insufficient_memory",
            "message": f"{report['memory'].get('budget_scope', 'cpu').upper()} allocation needs estimated {required / GIB:.2f} GiB plus reserve; "
                       f"{report['memory']['available_bytes'] / GIB:.2f} GiB available. Host and accelerator budgets must both fit. Weights were not loaded.",
        })
    report["limits"] = {
        "max_new_tokens": request.max_new_tokens,
        "max_seconds": request.max_seconds if math.isfinite(request.max_seconds) else None,
        "max_input_tokens": MAX_INPUT_TOKENS,
        "timeout_seconds": request.timeout_seconds if math.isfinite(request.timeout_seconds) else None,
    }
    report["ready"] = report["ok"] = not errors
    return report
