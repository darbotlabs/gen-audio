"""Exercise policy selection independently of the machine's installed GPU."""
from contextlib import nullcontext
from dataclasses import asdict
from types import SimpleNamespace
import sys

import pytest

from gen_audio.model_workers.contracts import WorkerError
from gen_audio.model_workers.execution import Execution, resolve, gpu_memory


def torch_runtime(monkeypatch, *, hip="7.2", available=True, bf16=True):
    runtime = SimpleNamespace(__version__="2.9.1", version=SimpleNamespace(hip=hip, cuda=None if hip else "12.8"),
        cuda=SimpleNamespace(is_available=lambda: available, device_count=lambda: 1,
            device=lambda index: nullcontext(), is_bf16_supported=lambda: bf16,
            get_device_properties=lambda index: SimpleNamespace(name="test GPU", gcnArchName="gfx1151", total_memory=32*2**30),
            mem_get_info=lambda: (30*2**30, 32*2**30)))
    monkeypatch.setitem(sys.modules, "torch", runtime)
    return runtime


@pytest.mark.parametrize("value", [{"backend":"cpu","device":"gpu"}, {"backend":"rocm","device":"npu"},
    {"device_index":True}, {"device_index":16}, {"precision":None}, {"backend":"cpu","device_index":1},
    {"python":"external"}, None, []])
def test_rejects_ambiguous_or_injected_policies(value):
    with pytest.raises(WorkerError): Execution.parse(value)


def test_cpu_does_not_import_torch(monkeypatch):
    monkeypatch.setitem(sys.modules, "torch", None)
    resolved = resolve(Execution.parse({"backend":"cpu"}))
    assert (resolved["backend"], resolved["torch_device"], resolved["precision"]) == ("cpu","cpu","float32")


@pytest.mark.parametrize("hip,expected", [("7.2","rocm"),(None,"cuda")])
def test_auto_reports_actual_backend_and_does_not_rename_hip_cuda_api(monkeypatch, hip, expected):
    torch_runtime(monkeypatch, hip=hip)
    policy=Execution.parse({"backend":"auto"})
    actual=resolve(policy)
    assert actual["backend"] == expected
    assert actual["requested"] == asdict(policy)
    assert actual["torch_device"] == "cuda:0"
    assert actual["precision"] == "bfloat16"
    assert gpu_memory(actual, 2**30)["sufficient"]


def test_unavailable_or_wrong_backend_never_silently_uses_cpu(monkeypatch):
    torch_runtime(monkeypatch)
    with pytest.raises(WorkerError, match="Requested cuda"): resolve(Execution(backend="cuda"))
    fallback=resolve(Execution(backend="cuda",fallback="cpu"))
    assert fallback["backend"] == "cpu" and fallback["fallback_reason"]
    torch_runtime(monkeypatch, available=False)
    with pytest.raises(WorkerError): resolve(Execution())
    assert resolve(Execution(fallback="cpu"))["backend"] == "cpu"


def test_precision_and_unsupported_onnx_requests_are_explicit(monkeypatch):
    torch_runtime(monkeypatch, bf16=False)
    assert resolve(Execution())["precision"] == "float16"
    for policy in [Execution(precision="bfloat16"), Execution(backend="cpu",precision="float16"),
                   Execution(backend="directml"), Execution(backend="vitisai")]:
        with pytest.raises(WorkerError): resolve(policy)


def test_experimental_attention_is_explicit_and_cannot_fall_back_to_cpu(monkeypatch):
    monkeypatch.delenv("TORCH_ROCM_AOTRITON_ENABLE_EXPERIMENTAL", raising=False)
    torch_runtime(monkeypatch)
    actual=resolve(Execution(backend="rocm",attention="efficient"))
    assert actual["experimental_attention"] and actual["attention"]=="efficient"
    for policy in [Execution(backend="cpu",attention="efficient"),
                   Execution(backend="rocm",precision="float32",attention="efficient")]:
        with pytest.raises(WorkerError): resolve(policy)
