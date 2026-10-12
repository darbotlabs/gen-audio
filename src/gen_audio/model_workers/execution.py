"""One execution contract for CPU, CUDA and ROCm workers.

Backend names describe the runtime, device names describe hardware. ROCm uses
PyTorch's cuda API internally; manifests always retain the actual backend.
"""
from __future__ import annotations

from dataclasses import asdict, dataclass
from contextlib import nullcontext
import os
import re

from .contracts import GIB, WorkerError


@dataclass(frozen=True)
class Execution:
    backend: str = "auto"
    device: str = "auto"
    device_index: int = 0
    precision: str = "auto"
    attention: str = "auto"
    fallback: str = "error"

    @classmethod
    def parse(cls, value):
        if not isinstance(value, dict) or set(value) - set(cls.__dataclass_fields__):
            raise WorkerError("invalid_execution", "Execution contains an unknown option; use the execution schema")
        result = cls(**value)
        if (result.backend not in ("auto", "cpu", "cuda", "rocm", "directml", "vitisai")
                or result.device not in ("auto", "cpu", "gpu", "npu")
                or result.precision not in ("auto", "float32", "float16", "bfloat16")
                or result.attention not in ("auto", "math", "efficient")
                or result.fallback not in ("error", "cpu")
                or type(result.device_index) is not int or not 0 <= result.device_index <= 15):
            raise WorkerError("invalid_execution", "Invalid execution option")
        expected = {"cpu": "cpu", "cuda": "gpu", "rocm": "gpu", "directml": "gpu", "vitisai": "npu"}.get(result.backend)
        if expected and result.device not in ("auto", expected):
            raise WorkerError("invalid_execution", "Execution backend and device disagree")
        if (result.backend == "cpu" or result.device == "cpu") and result.device_index:
            raise WorkerError("invalid_execution", "CPU device_index must be zero")
        return result


def preferences(request) -> Execution:
    if request.execution is not None:
        if request.device != "cpu":
            raise WorkerError("invalid_execution", "Use either execution or the compatibility device argument")
        return Execution.parse(request.execution)
    if request.device in ("auto", "gpu"):
        return Execution(device=request.device)
    match = re.fullmatch(r"(cpu|cuda|rocm)(?::([0-9]+))?", request.device) if isinstance(request.device, str) else None
    if not match:
        raise WorkerError("unsupported_device", "Use cpu, cuda, rocm, gpu or auto, or the execution object")
    return Execution.parse({"backend": match[1], "device_index": int(match[2] or 0)})


def resolve(options: Execution, *, model_precision: str | None = None) -> dict:
    requested = asdict(options)
    if model_precision not in (None, "float32", "float16", "bfloat16"):
        raise WorkerError("unsupported_precision", "Unsupported checkpoint precision")

    def cpu(reason=None):
        precision = (model_precision or "float32") if options.precision == "auto" else options.precision
        if precision == "float16":
            raise WorkerError("unsupported_precision", "CPU workers support float32 or bfloat16; choose one explicitly")
        if options.attention not in ("auto", "math"):
            raise WorkerError("unsupported_attention", "CPU workers require automatic or math attention")
        return {"schema_version": "1.0", "requested": requested, "backend": "cpu", "device": "cpu",
                "device_index": 0, "torch_device": "cpu", "precision": precision,
                "fallback_reason": reason, "hardware": {"name": "CPU"}, "attention": options.attention}

    if options.backend == "cpu" or options.device == "cpu":
        return cpu()
    if options.backend in ("directml", "vitisai") or options.device == "npu":
        # Valid shared options, but these PyTorch model adapters have no ONNX
        # model variant. Never reinterpret them as a GPU/CPU implementation.
        raise WorkerError("unsupported_backend", "This model worker requires a PyTorch CPU, CUDA or ROCm variant; no validated ONNX variant is configured")
    try:
        import torch
        if not torch.cuda.is_available():
            raise WorkerError("accelerator_unavailable", "This Python runtime has no available CUDA/ROCm device")
        backend = "rocm" if torch.version.hip else "cuda"
        if options.backend not in ("auto", backend):
            raise WorkerError("backend_mismatch", f"Requested {options.backend}; this runtime provides {backend}")
        if options.device_index >= torch.cuda.device_count():
            raise WorkerError("accelerator_unavailable", "Requested accelerator index is unavailable")
        index = options.device_index
        with torch.cuda.device(index):
            props = torch.cuda.get_device_properties(index)
            bf16 = torch.cuda.is_bf16_supported()
        precision = (model_precision or ("bfloat16" if bf16 else "float16")) if options.precision == "auto" else options.precision
        if precision == "bfloat16" and not bf16:
            raise WorkerError("unsupported_precision", "Selected accelerator/runtime does not support bfloat16")
        experimental = backend == "rocm" and options.attention == "efficient"
        if experimental:
            if precision == "float32":
                raise WorkerError("unsupported_attention", "ROCm accelerated attention requires FP16 or BF16")
            os.environ["TORCH_ROCM_AOTRITON_ENABLE_EXPERIMENTAL"] = "1"
        return {"schema_version": "1.0", "requested": requested, "backend": backend, "device": "gpu",
                "device_index": index, "torch_device": f"cuda:{index}", "precision": precision,
                "fallback_reason": None, "attention": options.attention, "experimental_attention": experimental,
                "hardware": {"name": props.name,
                "architecture": getattr(props, "gcnArchName", None), "total_bytes": props.total_memory},
                "runtime": {"torch": str(torch.__version__), "hip": torch.version.hip, "cuda": torch.version.cuda}}
    except (WorkerError, ImportError, OSError, RuntimeError) as error:
        if options.fallback == "cpu":
            return cpu(str(error))
        if isinstance(error, WorkerError):
            raise
        raise WorkerError("accelerator_unavailable", f"Accelerator runtime initialization failed: {type(error).__name__}") from error


def gpu_memory(execution: dict, required: int) -> dict:
    import torch
    with torch.cuda.device(execution["device_index"]):
        available, total = torch.cuda.mem_get_info()
    if not 0 <= available <= total or not total:
        raise WorkerError("memory_unknown", "Invalid accelerator memory counters")
    safe = max(0, min(available - 2 * GIB, int(total * .90)))
    return {"budget_scope": "gpu", "total_bytes": total, "available_bytes": available,
            "required_bytes": required, "reserve_bytes": 2 * GIB, "safe_budget_bytes": safe,
            "sufficient": safe >= required}


def component_policy(execution: dict, engine: str) -> dict:
    """Describe component weight types and kernel policies, not all arithmetic."""
    if engine != "personaplex":
        return execution
    codec_attention = "math" if execution["attention"] == "efficient" else execution["attention"]
    return {**execution, "precision_scope": "language_model_weights", "components": {
        "language_model": {"weight_precision": execution["precision"],
                           "attention_policy": execution["attention"]},
        "audio_codec": {"weight_precision": "float32", "attention_policy": codec_attention,
                        "reason": "Mimi retains its FP32 checkpoint precision; fused LM attention does not change the codec"},
    }}


def recheck_memory(report: dict, *, minimum_cpu_bytes: int = 0):
    from .preflight import require_memory
    memory, execution = report["memory"], report["execution"]
    if execution["device"] == "cpu":
        require_memory(max(memory["required_bytes"], minimum_cpu_bytes))
    else:
        require_memory(memory["cpu"]["required_bytes"])
        if not gpu_memory(execution, memory["required_bytes"])["sufficient"]:
            raise WorkerError("insufficient_memory", "Accelerator memory budget changed before loading/generation")


def synchronize(execution: dict):
    if execution["device"] == "gpu":
        import torch
        torch.cuda.synchronize(execution["device_index"])


def attention_context(execution: dict):
    policy = execution.get("attention", "auto")
    if policy == "auto":
        return nullcontext()
    from torch.nn.attention import SDPBackend, sdpa_kernel
    return sdpa_kernel({"math": SDPBackend.MATH, "efficient": SDPBackend.EFFICIENT_ATTENTION}[policy])
