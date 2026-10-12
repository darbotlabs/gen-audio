"""Small fake runtimes; these tests never load torch, weights, or speech models."""

from contextlib import nullcontext
from dataclasses import replace
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import sys
import time
from types import ModuleType, SimpleNamespace
from uuid import uuid4
import wave

import numpy as np
import pytest

from gen_audio.model_workers import checkpoints, cli, preflight as pf, runtime
from gen_audio.model_workers.contracts import GIB, Request, WorkerError, runtime_spec


@pytest.fixture
def workspace():
    root = Path(__file__).resolve().parents[1] / ".pytest_cache" / "model-workers" / uuid4().hex
    root.mkdir(parents=True)
    try:
        yield root
    finally:
        shutil.rmtree(root)


def write_weights(path, tensors=None):
    tensors = tensors if tensors is not None else {"weight": {"dtype": "F32", "shape": [1], "data_offsets": [0, 4]}}
    raw = json.dumps(tensors).encode()
    raw += b" " * (-len(raw) % 8)
    data_bytes = max((value["data_offsets"][1] for value in tensors.values()), default=0)
    with path.open("wb") as handle:
        handle.write(struct.pack("<Q", len(raw)) + raw)
        handle.truncate(8 + len(raw) + data_bytes)


def tiny_vibe_config():
    return {
        "model_type": "vibevoice",
        "decoder_config": {
            "model_type": "qwen2", "vocab_size": 2048, "max_position_embeddings": 65536,
            "hidden_size": 16, "intermediate_size": 32, "num_hidden_layers": 2,
            "num_attention_heads": 2, "num_key_value_heads": 1,
        },
        "acoustic_tokenizer_config": {
            "vae_dim": 4, "encoder_n_filters": 1, "decoder_n_filters": 1,
            "encoder_depths": "1-1-1-1-1-1-1",
        },
        "semantic_tokenizer_config": {
            "vae_dim": 8, "encoder_n_filters": 1, "encoder_depths": "1-1-1-1-1-1-1",
        },
        "diffusion_head_config": {
            "hidden_size": 16, "latent_size": 4, "head_layers": 1, "head_ffn_ratio": 3.0,
            "ddpm_num_steps": 1000, "ddpm_num_inference_steps": 20,
        },
    }


def write_shapes(path, shapes):
    tensors, offset = {}, 0
    for name, shape in shapes.items():
        end = offset + math.prod(shape) * 4
        tensors[name] = {"dtype": "F32", "shape": list(shape), "data_offsets": [offset, end]}
        offset = end
    write_weights(path, tensors)


def write_vibe_weights(root, shapes):
    write_shapes(root / "part.safetensors", shapes)
    (root / "model.safetensors.index.json").write_text(
        json.dumps({"weight_map": dict.fromkeys(shapes, "part.safetensors")}))


def write_wav(path, seconds=0.5, *, rate=24000, channels=1, width=2):
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(channels)
        handle.setsampwidth(width)
        handle.setframerate(rate)
        handle.writeframes(b"\x20" * int(seconds * rate) * width * channels)
    return path


@pytest.fixture
def worker_request(workspace):
    root = workspace / "model"
    root.mkdir()
    config = tiny_vibe_config()
    (root / "config.json").write_text(json.dumps(config))
    (root / "preprocessor_config.json").write_text(json.dumps({
        "speech_tok_compress_ratio": 3200,
        "audio_processor": {"sampling_rate": 24000},
        "language_model_pretrained_name": "would-fetch-from-network",
    }))
    write_vibe_weights(root, checkpoints.vibevoice_architecture(config)["shapes"])
    tokenizer = root / "tokenizer"
    tokenizer.mkdir()
    for name in ("tokenizer_config.json", "tokenizer.json", "vocab.json", "merges.txt"):
        (tokenizer / name).write_text("{}")
    voice = write_wav(workspace / "voice.wav")
    return Request(engine="vibevoice", model_dir=root, output=workspace / "speech.wav",
                   text="Hello there.", voice_refs=(voice,))


@pytest.fixture
def ready(monkeypatch):
    monkeypatch.setattr(pf, "dependency_report", lambda spec: ({"fake": "unit-test"}, []))
    monkeypatch.setattr(pf, "memory_snapshot", lambda: {"total_bytes": 128 * GIB, "available_bytes": 100 * GIB})


def errors(report):
    return {item["code"] for item in report["errors"]}


def persona_request(request):
    root = request.model_dir
    write_weights(root / "model.safetensors")
    write_weights(root / "tokenizer-e351c8d8-checkpoint125.safetensors")
    (root / "tokenizer_spm_32k_3.model").write_bytes(b"fake-for-mock")
    return replace(request, engine="personaplex", text=None, input_audio=request.voice_refs[0],
                   persona="You are a concise, friendly assistant.")


def test_preflight_ready_uses_no_large_runtime(worker_request, ready):
    before = set(sys.modules)
    report = pf.preflight(worker_request)
    assert report["ready"] and report["ok"]
    assert not report["synthesizedSpeech"] and not report["inference_validated"]
    assert not {name.split(".")[0] for name in set(sys.modules) - before} & {"torch", "vibevoice", "moshi", "transformers"}
    assert report["memory"]["required_bytes"] >= 12 * GIB
    assert report["assets"]["architecture_fp32_bytes"] > 0
    assert "runtime meta validation still required" in report["checkpoint_validation"]
    assert not worker_request.output.exists()


def test_load_preflight_has_no_generation_inputs_but_keeps_asset_checks(worker_request, ready):
    request = replace(worker_request, text=None, voice_refs=(), output=None)
    assert "missing_text" in errors(pf.preflight(request))
    report = pf.preflight(request, load_only=True)
    assert report["ready"] and not report["inference_validated"]
    (request.model_dir / "config.json").write_text("{}")
    assert not pf.preflight(request, load_only=True)["ready"]


def test_resident_memory_budget_keeps_generation_reserve(worker_request, ready, monkeypatch):
    cold = pf.preflight(worker_request)
    warm = pf.preflight(worker_request, resident=True)
    assert warm["memory"]["required_bytes"] < cold["memory"]["required_bytes"]
    assert warm["memory"]["required_bytes"] >= 6 * GIB
    monkeypatch.setattr(pf, "memory_snapshot", lambda: {"total_bytes":128 * GIB, "available_bytes":2 * GIB})
    assert "insufficient_memory" in errors(pf.preflight(worker_request, resident=True))


def test_real_cli_preflight_never_imports_torch(worker_request):
    code = """
import json, sys
from pathlib import Path
from gen_audio.model_workers.contracts import Request
from gen_audio.model_workers.preflight import preflight
report = preflight(Request(engine='vibevoice', model_dir=Path(sys.argv[1]), text='hello',
                          voice_refs=(Path(sys.argv[2]),)))
print(json.dumps({'report': report, 'torch_imported': 'torch' in sys.modules,
                  'vibevoice_imported': 'vibevoice' in sys.modules}))
"""
    result = subprocess.run(
        [sys.executable, "-c", code, str(worker_request.model_dir), str(worker_request.voice_refs[0])],
        cwd=Path(__file__).resolve().parents[1], env={**os.environ, "PYTHONPATH": "src"},
        capture_output=True, text=True, timeout=30, check=True,
    )
    data = json.loads(result.stdout)
    assert not data["torch_imported"] and not data["vibevoice_imported"]
    assert not data["report"]["synthesizedSpeech"]


@pytest.mark.parametrize("mutation", [
    {"device": "cuda:bad"}, {"device": "dml"}, {"max_seconds": float("nan")},
    {"max_seconds": float("inf")}, {"max_seconds": 0}, {"max_seconds": 61},
    {"max_new_tokens": 0}, {"max_new_tokens": 1025}, {"max_new_tokens": True},
    {"timeout_seconds": -1}, {"timeout_seconds": 3601}, {"seed": -1},
    {"text": "a" * 8193},
])
def test_request_caps(worker_request, mutation):
    with pytest.raises(WorkerError):
        pf.validate_request(replace(worker_request, **mutation))


def test_no_memory_override_loads_models(worker_request, ready, monkeypatch):
    monkeypatch.setattr(pf, "memory_snapshot", lambda: {"total_bytes": 32 * GIB, "available_bytes": 6 * GIB})
    before = set(sys.modules)
    report = runtime.execute(worker_request)
    assert "insufficient_memory" in errors(report)
    assert not worker_request.output.exists()
    assert "torch" not in set(sys.modules) - before


def test_unknown_memory_fails_closed(worker_request, ready, monkeypatch):
    def fail():
        raise OSError("memory unavailable")
    monkeypatch.setattr(pf, "memory_snapshot", fail)
    report = pf.preflight(worker_request)
    assert not report["ready"]
    assert report["memory"] is None


def test_unified_installed_memory_is_not_misused_as_cpu_allocation_budget():
    report = pf.check_memory(23 * GIB, {
        "total_bytes": 32 * GIB, "available_bytes": 8 * GIB,
        "installed_bytes": 128 * GIB, "hardware_reserved_bytes": 96 * GIB,
        "budget_scope": "cpu",
    })
    assert report["installed_bytes"] == 128 * GIB
    assert report["safe_budget_bytes"] == 6 * GIB
    assert not report["sufficient"]


@pytest.mark.parametrize("asset", ["part.safetensors", "tokenizer/tokenizer.json", "config.json", "preprocessor_config.json"])
def test_missing_assets(worker_request, ready, asset):
    (worker_request.model_dir / Path(asset)).unlink()
    assert "missing_asset" in errors(pf.preflight(worker_request))


def test_missing_voice_and_tokenizer(worker_request, ready):
    missing = worker_request.model_dir / "absent"
    report = pf.preflight(replace(worker_request, tokenizer=missing, voice_refs=(missing / "voice.wav",)))
    assert "missing_asset" in errors(report)
    assert not report["ready"]


def test_invalid_config_structure_and_nonfinite_json(worker_request, ready):
    path = worker_request.model_dir / "config.json"
    path.write_text('{"model_type":"vibevoice","decoder_config":[]}')
    assert "unsupported_model" in errors(pf.preflight(worker_request))
    path.write_text('{"model_type":"vibevoice","bad":NaN}')
    assert "invalid_asset" in errors(pf.preflight(worker_request))


def test_explicit_tokenizer_outside_model_used(worker_request, ready):
    tokenizer = worker_request.model_dir / "tokenizer"
    moved = worker_request.model_dir.parent / "local-qwen"
    tokenizer.rename(moved)
    assert not pf.preflight(worker_request)["ready"]
    assert pf.preflight(replace(worker_request, tokenizer=moved))["ready"]


def test_safe_shard_paths(worker_request, ready):
    (worker_request.model_dir / "model.safetensors.index.json").write_text(
        json.dumps({"weight_map": {"weight": "../outside.safetensors"}})
    )
    write_weights(worker_request.model_dir.parent / "outside.safetensors")
    assert "invalid_asset" in errors(pf.preflight(worker_request))


def test_truncated_safetensors_and_lfs_pointer(worker_request, ready):
    shard = worker_request.model_dir / "part.safetensors"
    shard.write_bytes(shard.read_bytes()[:-1])
    assert "invalid_weights" in errors(pf.preflight(worker_request))
    shard.write_text("version https://git-lfs.github.com/spec/v1\noid sha256:stub\n")
    assert "invalid_weights" in errors(pf.preflight(worker_request))


def test_safetensors_does_not_read_weight_payload(workspace):
    path = workspace / "weights.safetensors"
    write_weights(path)
    report = pf.safetensors_header(path)
    assert report["fp32_bytes"] == 4
    assert list(report["tensors"]) == ["weight"]
    path.write_bytes(struct.pack("<Q", 2**63))
    with pytest.raises(WorkerError, match="header"):
        pf.safetensors_header(path)


def test_index_must_match_actual_tensors(worker_request, ready):
    (worker_request.model_dir / "model.safetensors.index.json").write_text(
        json.dumps({"weight_map": {"missing_key": "part.safetensors"}})
    )
    assert "invalid_weights" in errors(pf.preflight(worker_request))


def test_ambiguous_weights_cannot_bypass_checked_shard_set(worker_request, ready):
    write_weights(worker_request.model_dir / "model.safetensors")
    assert "ambiguous_weights" in errors(pf.preflight(worker_request))


def test_no_pickle_voice_loading(worker_request, ready):
    path = worker_request.model_dir / "NATF0.pt"
    path.write_bytes(b"not allowed to unpickle")
    report = pf.preflight(replace(persona_request(worker_request), voice_refs=(path,)))
    assert "unsupported_voice_format" in errors(report)
    assert not report["ready"]


def test_unsupported_capability(worker_request, ready):
    report = pf.preflight(replace(worker_request, engine="personaplex"))
    assert "incompatible_capability" in errors(report)
    assert not report["synthesizedSpeech"]
    report = pf.preflight(replace(worker_request, input_audio=worker_request.voice_refs[0]))
    assert "incompatible_capability" in errors(report)


def test_missing_conversation_inputs(worker_request, ready):
    assert "missing_conversation_input" in errors(pf.preflight(
        replace(persona_request(worker_request), input_audio=None)))


def test_personaplex_stream_caps(worker_request, ready):
    req = replace(persona_request(worker_request), max_new_tokens=1)
    assert "resource_limit" in errors(pf.preflight(req))


@pytest.mark.parametrize("text,voices", [
    ("Speaker 2: hi", 1), ("Speaker 1: hi\nignored prose", 1),
    ("Speaker 5: hi", 1), ("Speaker 1: ", 1), ("Hello", 2),
    ("Speaker 1: hi\nSpeaker 3: hi", 2),
])
def test_script_never_silently_discards_lines(text, voices):
    with pytest.raises(WorkerError):
        pf.normalized_script(text, voices)


def test_script_voice_order_is_numeric():
    assert pf.normalized_script("Speaker 2: two\nSpeaker 1: one", 2) == "Speaker 2: two\nSpeaker 1: one"
    assert pf.normalized_script("Hello\nworld", 1) == "Speaker 1: Hello world"


@pytest.mark.parametrize("kwargs", [{"rate": 48000}, {"channels": 2}, {"width": 1}])
def test_wav_format_is_explicit(workspace, kwargs):
    path = write_wav(workspace / "input.wav", **kwargs)
    with pytest.raises(WorkerError, match="PCM16"):
        pf.inspect_wav(path, seconds=30)


def test_wav_header_lies_and_resource_limits(workspace):
    path = write_wav(workspace / "input.wav")
    with pytest.raises(WorkerError, match="exceeds|duration"):
        pf.inspect_wav(path, seconds=0.1)
    path.write_bytes(path.read_bytes()[:-2])
    with pytest.raises(WorkerError, match="Truncated"):
        pf.inspect_wav(path, seconds=30)


def test_pin_verification(monkeypatch):
    spec = runtime_spec("vibevoice")

    class Distribution:
        def __init__(self, name):
            self.version = spec["versions"][name] + ("+cpu" if name == "torch" else "")
        def read_text(self, name):
            return json.dumps({"url": spec["repository"] + ".git",
                               "vcs_info": {"vcs": "git", "commit_id": spec["revision"]}})

    monkeypatch.setattr(pf.sys, "version_info", (3, 12, 0))
    monkeypatch.setattr(pf.importlib.metadata, "distribution", Distribution)
    installed, issues = pf.dependency_report(spec)
    assert issues == [] and installed["torch"] == "2.6.0+cpu"
    monkeypatch.setattr(Distribution, "read_text", lambda self, name: None)
    assert "unverified_runtime" in {error["code"] for error in pf.dependency_report(spec)[1]}


def test_incompatible_dependencies_reported_without_import(monkeypatch):
    monkeypatch.setattr(pf.importlib.metadata, "distribution",
                        lambda name: SimpleNamespace(version="5.5.0", read_text=lambda _: None))
    _, issues = pf.dependency_report(runtime_spec("vibevoice"))
    assert "incompatible_dependency" in {error["code"] for error in issues}


def test_offline_network_fence_restores_environment(monkeypatch):
    monkeypatch.delenv("HF_HUB_OFFLINE", raising=False)
    original = socket.socket.connect
    with runtime.offline_runtime():
        assert os.environ["HF_HUB_OFFLINE"] == "1"
        assert os.environ["TRANSFORMERS_OFFLINE"] == "1"
        with pytest.raises(WorkerError, match="Network"):
            socket.getaddrinfo("example.com", 443)
        with socket.socket() as sock:
            with pytest.raises(WorkerError, match="Network"):
                sock.connect(("127.0.0.1", 9))
    assert "HF_HUB_OFFLINE" not in os.environ
    assert socket.socket.connect is original


@pytest.mark.parametrize("audio", [
    np.array([], dtype=np.float32), np.zeros(100, dtype=np.float32),
    np.array([np.nan]), np.array([np.inf]), np.array([[0.1], [0.2]]),
    np.array([1e308]),
    np.ones(24001, dtype=np.float32), np.array([1, 2], dtype=np.int32),
])
def test_invalid_generated_audio_never_publishes(worker_request, audio):
    with pytest.raises(WorkerError):
        runtime.publish(replace(worker_request, max_seconds=1), audio, {})
    assert not worker_request.output.exists()
    assert not worker_request.output.with_suffix(".synth.json").exists()


def test_verified_receipt_hash_and_manifest(worker_request):
    receipt = runtime.publish(worker_request, np.full(2400, 0.125, dtype=np.float32), {"test_only": True})
    assert receipt["output"] == str(worker_request.output.resolve())
    assert receipt["duration_s"] == 0.1
    assert receipt["sha256"] == hashlib.sha256(worker_request.output.read_bytes()).hexdigest()
    record = json.loads(Path(receipt["manifest"]).read_text())
    assert record["output_semantics"] == "generated_script_speech"
    assert record["transcript_verified"] is False
    with pytest.raises(FileExistsError):
        runtime.publish(worker_request, np.ones(20, dtype=np.float32), {})
    assert not list(worker_request.output.parent.glob("*.partial"))


def test_publication_failure_rolls_back(worker_request, monkeypatch):
    original = os.link
    calls = []
    def fail_second(source, dest):
        calls.append(str(dest))
        if len(calls) == 2:
            raise OSError("manifest publication failed")
        return original(source, dest)
    monkeypatch.setattr(os, "link", fail_second)
    with pytest.raises(OSError):
        runtime.publish(worker_request, np.ones(20, dtype=np.float32), {})
    assert not worker_request.output.exists()
    assert not worker_request.output.with_suffix(".synth.json").exists()
    assert not list(worker_request.output.parent.glob("*.partial"))


def test_quantization_silence_is_not_success(worker_request):
    with pytest.raises(WorkerError, match="quantizes"):
        runtime.publish(worker_request, np.full(20, 1e-20, dtype=np.float32), {})


class Tensor:
    def __init__(self, array):
        self.array = np.asarray(array)
        self.shape = self.array.shape
    def to(self, *args, **kwargs):
        return self
    def detach(self):
        return self
    def float(self):
        return Tensor(self.array.astype(np.float32))
    def cpu(self):
        return self
    def numpy(self):
        return self.array
    def min(self):
        return Tensor(self.array.min())
    def max(self):
        return Tensor(self.array.max())
    def item(self):
        return self.array.item()
    def __getitem__(self, key):
        return Tensor(self.array[key])


def install_module(monkeypatch, name, **attrs):
    module = ModuleType(name)
    module.__dict__.update(attrs)
    monkeypatch.setitem(sys.modules, name, module)
    return module


def fake_torch(monkeypatch):
    return install_module(
        monkeypatch, "torch", float32="float32", manual_seed=lambda seed: None,
        get_num_threads=lambda: 16, set_num_threads=lambda count: None,
        is_tensor=lambda value: isinstance(value, Tensor), inference_mode=nullcontext,
        zeros=lambda *shape, **kwargs: Tensor(np.zeros(shape, dtype=np.float32)),
        as_tensor=lambda data, **kwargs: Tensor(data),
    )


def fake_vibevoice(monkeypatch, *, output=None):
    calls = {}
    fake_torch(monkeypatch)
    def init_empty_weights(*, include_buffers):
        assert include_buffers is False
        calls["empty_weights"] = True
        return nullcontext()
    install_module(monkeypatch, "accelerate", init_empty_weights=init_empty_weights)
    install_module(monkeypatch, "transformers")
    install_module(monkeypatch, "transformers.modeling_utils", no_init_weights=nullcontext)

    class Tokenizer:
        speech_start_id, speech_end_id, speech_diffusion_id, eos_token_id, pad_id = 1, 2, 3, 4, 5
        def __len__(self):
            return 1000
        @classmethod
        def from_pretrained(cls, path, **kwargs):
            calls["tokenizer"] = (path, kwargs)
            return cls()

    class Processor:
        def __init__(self, **kwargs):
            calls["processor_init"] = kwargs
        def __call__(self, **kwargs):
            calls["processor_call"] = kwargs
            return {"input_ids": Tensor([[1, 2, 3]]), "attention_mask": Tensor([[1, 1, 1]])}
        @classmethod
        def from_pretrained(cls, *args, **kwargs):
            raise AssertionError("Unsafe tokenizer Hub fallback must not be used")

    class Model:
        expected_shapes = checkpoints.vibevoice_architecture(tiny_vibe_config())["shapes"]
        def __init__(self, config=None):
            self.is_meta = config is not None
            if config is not None:
                calls["meta_config"] = config
        def parameters(self):
            return [SimpleNamespace(is_meta=self.is_meta)]
        def state_dict(self):
            calls["meta_state_dict"] = True
            return {name: SimpleNamespace(shape=shape) for name, shape in self.expected_shapes.items()}
        def tie_weights(self):
            calls["tie_weights"] = True
        @classmethod
        def from_pretrained(cls, path, **kwargs):
            assert calls["meta_state_dict"] and calls["tie_weights"]
            calls["model"] = (path, kwargs)
            model = cls()
            model.config = kwargs["config"]
            model.config.decoder_config._attn_implementation = "sdpa"
            return model, {}
        def eval(self):
            return self
        def set_ddpm_inference_steps(self, **kwargs):
            calls["ddpm"] = kwargs
        def generate(self, **kwargs):
            calls["generate"] = kwargs
            assert kwargs["stop_check_fn"]() is False
            values = np.full((1, 3200), 0.1, dtype=np.float32) if output is None else output
            return SimpleNamespace(speech_outputs=[Tensor(values)], sequences=Tensor([[1, 2, 3, 4]]))

    for name in ("vibevoice", "vibevoice.modular", "vibevoice.processor"):
        install_module(monkeypatch, name)
    def model_config(data):
        return SimpleNamespace(**{key: SimpleNamespace(**value) if key.endswith("_config") else value
                                  for key, value in data.items()})
    install_module(monkeypatch, "vibevoice.modular.configuration_vibevoice",
                   VibeVoiceConfig=SimpleNamespace(from_dict=model_config))
    install_module(monkeypatch, "vibevoice.modular.modular_vibevoice_text_tokenizer", VibeVoiceTextTokenizerFast=Tokenizer)
    install_module(monkeypatch, "vibevoice.modular.modeling_vibevoice_inference", VibeVoiceForConditionalGenerationInference=Model)
    install_module(monkeypatch, "vibevoice.processor.vibevoice_processor", VibeVoiceProcessor=Processor)
    install_module(monkeypatch, "vibevoice.processor.vibevoice_tokenizer_processor",
                   VibeVoiceTokenizerProcessor=lambda **kwargs: kwargs)
    return calls


def test_actual_vibevoice_api_calls_offline_cpu_only(worker_request, ready, monkeypatch):
    calls = fake_vibevoice(monkeypatch)
    receipt = runtime.execute(replace(worker_request, max_seconds=1))
    assert receipt["synthesizedSpeech"]
    path, args = calls["model"]
    assert Path(path) == worker_request.model_dir
    assert args["local_files_only"] and not args["trust_remote_code"] and args["use_safetensors"]
    assert args["weights_only"] and "attn_implementation" not in args
    assert args["torch_dtype"] == "float32" and args["device_map"] == "cpu"
    assert args["config"] is calls["meta_config"]
    assert args["config"].torch_dtype == "float32"
    assert args["config"].decoder_config.torch_dtype == "float32"
    assert args["config"].decoder_config._attn_implementation == "sdpa"
    assert calls["empty_weights"]
    assert calls["tokenizer"][1] == {"local_files_only": True, "trust_remote_code": False}
    assert calls["generate"]["max_new_tokens"] == 7
    assert calls["processor_call"]["text"] == ["Speaker 1: Hello there."]
    assert isinstance(calls["processor_call"]["voice_samples"][0][0], np.ndarray)
    assert "ddpm" not in calls
    assert "cfg_scale" not in calls["generate"]
    assert "generation_config" not in calls["generate"]


def test_vibevoice_invalid_output_has_no_receipt(worker_request, ready, monkeypatch):
    fake_vibevoice(monkeypatch, output=np.array([np.nan]))
    with pytest.raises(WorkerError, match="finite"):
        runtime.execute(worker_request)
    assert not worker_request.output.exists()


def test_vibevoice_incomplete_checkpoint_cannot_generate(worker_request, ready, monkeypatch):
    calls = fake_vibevoice(monkeypatch)
    module = sys.modules["vibevoice.modular.modeling_vibevoice_inference"]
    cls = module.VibeVoiceForConditionalGenerationInference
    monkeypatch.setattr(cls, "from_pretrained", lambda *args, **kwargs: (cls(), {"missing_keys": ["weight"]}))
    with pytest.raises(WorkerError, match="completely"):
        runtime.execute(worker_request)
    assert "generate" not in calls
    assert not worker_request.output.exists()


def test_missing_output_never_loads_backend(worker_request, ready, monkeypatch):
    calls = fake_vibevoice(monkeypatch)
    with pytest.raises(WorkerError, match="output"):
        runtime.execute(replace(worker_request, output=None))
    assert calls == {}


def test_memory_rechecked_before_backend_load(worker_request, ready, monkeypatch):
    calls = fake_vibevoice(monkeypatch)
    snapshots = iter([100 * GIB, 100 * GIB, 1 * GIB])
    monkeypatch.setattr(pf, "memory_snapshot", lambda: {"total_bytes": 128 * GIB, "available_bytes": next(snapshots)})
    with pytest.raises(WorkerError, match="available"):
        runtime.execute(worker_request)
    assert "model" not in calls
    assert "meta_state_dict" not in calls
    assert not worker_request.output.exists()


def fake_personaplex(monkeypatch):
    fake_torch(monkeypatch)
    calls = {"mimi": [], "steps": 0}

    class Mimi:
        sample_rate, frame_rate = 24000, 12.5
        def streaming(self, size):
            return nullcontext()
        def reset_streaming(self):
            pass
        def encode(self, chunk):
            return Tensor(np.zeros((1, 8, 1)))
        def decode(self, tokens):
            return Tensor(np.full((1, 1, 1920), 0.1, dtype=np.float32))

    class LM:
        def __init__(self, *, meta=False):
            self.is_meta = meta
        def eval(self):
            return self
        def parameters(self):
            return [SimpleNamespace(is_meta=self.is_meta)]
        def state_dict(self):
            calls["meta_state_dict"] = True
            return {name: SimpleNamespace(shape=shape) for name, shape in calls["expected_shapes"].items()}

    calls["expected_shapes"] = {"weight": (1,)}

    def get_mimi(path, **kwargs):
        assert calls["meta_state_dict"]
        calls["mimi"].append((path, kwargs))
        return Mimi()

    def get_moshi_lm(path, **kwargs):
        assert kwargs["copy_missing_weights"] is False
        if path is None:
            assert kwargs["device"] == "meta"
            calls["meta_lm"] = kwargs
            return LM(meta=True)
        calls["lm"] = (path, kwargs)
        return LM()

    class LMGen:
        def __init__(self, lm, **kwargs):
            calls["generator"] = kwargs
        def streaming(self, size):
            return nullcontext()
        def reset_streaming(self):
            pass
        def step(self, *args, **kwargs):
            calls["steps"] += 1
            return Tensor(np.full((1, 17, 1), 4, dtype=np.int64))
        def step_system_prompts(self, mimi):
            assert self.voice_prompt_audio.shape == (1, 12000)
            assert self.voice_prompt_embeddings is None
            assert self.text_prompt_tokens == [7, 8, 9]
            self.step()
        def load_voice_prompt_embeddings(self, path):
            raise AssertionError("Never unpickle voices")

    class Tokenizer:
        def __init__(self, path):
            calls["tokenizer"] = path
        def encode(self, text):
            calls["persona"] = text
            return [7, 8, 9]
        def id_to_piece(self, token):
            return "▁reply"

    install_module(monkeypatch, "sentencepiece", SentencePieceProcessor=Tokenizer)
    install_module(monkeypatch, "moshi")
    install_module(monkeypatch, "moshi.models", LMGen=LMGen,
                   loaders=SimpleNamespace(get_mimi=get_mimi, get_moshi_lm=get_moshi_lm))
    install_module(monkeypatch, "moshi.models.lm", normalize_audio=lambda audio, sr, lufs: audio)
    return calls


def test_actual_personaplex_streaming_api_without_offline_cli(worker_request, ready, monkeypatch):
    req = persona_request(worker_request)
    calls = fake_personaplex(monkeypatch)
    receipt = runtime.execute(req)
    assert receipt["ok"] and receipt["samples"] == 12000
    assert len(calls["mimi"]) == 2 and all(path.endswith(".safetensors") for path, kwargs in calls["mimi"])
    assert calls["lm"][1] == {"device": "cpu", "dtype": "float32", "cpu_offload": False,
                             "copy_missing_weights": False}
    assert calls["meta_lm"] == {**calls["lm"][1], "device": "meta"}
    assert not calls["generator"]["save_voice_prompt_embeddings"]
    assert calls["persona"].startswith("<system>")
    assert calls["steps"] == 4 + 1 + math.ceil(12000 / 1920)
    record = json.loads(Path(receipt["manifest"]).read_text())
    assert record["output_semantics"] == "generated_conversation_speech"
    assert not record["transcript_verified"]
    assert record["provenance"]["generated_text_semantics"] == "model_response_not_input_transcription"
    assert "moshi.offline" not in sys.modules


def test_cooperative_timeout_before_loading(worker_request, ready, monkeypatch):
    from gen_audio.model_workers.vibevoice import generate
    calls = fake_vibevoice(monkeypatch)
    with pytest.raises(WorkerError, match="deadline"):
        generate(worker_request, pf.preflight(worker_request), time.monotonic() - 1)
    assert "model" not in calls


def test_cli_one_json_success_and_failure(worker_request, ready, monkeypatch, capsys):
    fake_vibevoice(monkeypatch)
    args = ["--engine", "vibevoice", "--model-dir", str(worker_request.model_dir),
            "--text", worker_request.text, "--voice-ref", str(worker_request.voice_refs[0]),
            "--output", str(worker_request.output)]
    assert cli.main(args) == 0
    receipt = json.loads(capsys.readouterr().out)
    assert receipt["synthesizedSpeech"] and receipt["ok"]
    assert cli.main(args) == 2
    receipt = json.loads(capsys.readouterr().out)
    assert not receipt["synthesizedSpeech"] and not receipt["ok"]
    assert cli.main(["--engine", "unknown"]) == 2
    assert json.loads(capsys.readouterr().out)["errors"][0]["code"] == "invalid_arguments"


def test_preflight_stdout_never_claims_speech(worker_request, ready, capsys):
    assert cli.main(["--engine", "vibevoice", "--model-dir", str(worker_request.model_dir),
                     "--text", worker_request.text, "--voice-ref", str(worker_request.voice_refs[0]),
                     "--preflight"]) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["preflight"] and result["ready"]
    assert not result["synthesizedSpeech"] and "output" not in result


def test_preflight_nonfinite_limits_still_emit_valid_error_json(worker_request, ready, capsys):
    assert cli.main(["--engine", "vibevoice", "--model-dir", str(worker_request.model_dir),
                     "--text", "hello", "--voice-ref", str(worker_request.voice_refs[0]),
                     "--max-seconds", "nan", "--preflight"]) == 2
    result = json.loads(capsys.readouterr().out)
    assert "resource_limit" in errors(result)
    assert result["limits"]["max_seconds"] is None


def test_truncated_7b_checkpoint_cannot_reduce_architecture_memory(worker_request, ready, monkeypatch):
    config = tiny_vibe_config()
    config["decoder_config"].update({
        "hidden_size": 3584, "intermediate_size": 18944, "num_hidden_layers": 28,
        "num_attention_heads": 28, "num_key_value_heads": 4, "vocab_size": 152064,
    })
    for kind, latent in (("acoustic", 64), ("semantic", 128)):
        config[f"{kind}_tokenizer_config"].update({
            "vae_dim": latent, "encoder_n_filters": 32, "encoder_depths": "3-3-3-3-3-3-8",
        })
    config["acoustic_tokenizer_config"]["decoder_n_filters"] = 32
    config["diffusion_head_config"].update({"hidden_size": 3584, "latent_size": 64, "head_layers": 4})
    config["tie_word_embeddings"] = False
    (worker_request.model_dir / "config.json").write_text(json.dumps(config))
    write_vibe_weights(worker_request.model_dir, {"weight": (1,)})
    monkeypatch.setattr(pf, "memory_snapshot", lambda: {"total_bytes": 32 * GIB, "available_bytes": 15 * GIB})
    before = set(sys.modules)
    report = runtime.execute(worker_request)
    assert {"invalid_weights", "insufficient_memory"} <= errors(report)
    assert report["assets"]["architecture_fp32_bytes"] > 34 * GIB
    assert report["memory"]["required_bytes"] > 40 * GIB
    assert not report["ready"] and not worker_request.output.exists()
    assert not {"torch", "accelerate", "vibevoice", "transformers"} & (set(sys.modules) - before)


@pytest.mark.parametrize("section,key,value", [
    ("decoder_config", "hidden_size", None),
    ("decoder_config", "hidden_size", True),
    ("decoder_config", "hidden_size", 0),
    ("decoder_config", "hidden_size", 8193),
    ("decoder_config", "intermediate_size", -1),
    ("decoder_config", "num_hidden_layers", 1000000),
    ("decoder_config", "num_attention_heads", 3),
    ("decoder_config", "num_key_value_heads", 3),
    ("decoder_config", "head_dim", 128),
    ("decoder_config", "torch_dtype", "float64"),
    ("decoder_config", "rope_scaling", {"type": "unknown", "factor": 1000000}),
    ("acoustic_tokenizer_config", "encoder_n_filters", 1000000),
    ("acoustic_tokenizer_config", "encoder_depths", "1-2"),
    ("acoustic_tokenizer_config", "encoder_ratios", [8.0, 5, 5, 4, 2, 2]),
    ("acoustic_tokenizer_config", "decoder_depths", "8-8-8-8-8-8-8"),
    ("semantic_tokenizer_config", "kernel_size", 1000000),
    ("diffusion_head_config", "hidden_size", 32),
    ("diffusion_head_config", "head_layers", 0),
    ("diffusion_head_config", "head_ffn_ratio", float("inf")),
    ("diffusion_head_config", "ddpm_num_steps", 1000000),
])
def test_unsupported_architecture_dimensions_fail_closed(worker_request, ready, section, key, value):
    config = tiny_vibe_config()
    if value is None:
        config[section].pop(key)
    else:
        config[section][key] = value
    if value == float("inf"):
        with pytest.raises(WorkerError):
            checkpoints.vibevoice_architecture(config)
        return
    (worker_request.model_dir / "config.json").write_text(json.dumps(config))
    report = pf.preflight(worker_request)
    assert "unsupported_model" in errors(report) and not report["ready"]


@pytest.mark.parametrize("prefix", [
    "model.language_model.layers.1.",
    "model.acoustic_tokenizer.decoder.",
    "model.semantic_tokenizer.encoder.",
    "model.prediction_head.",
    "model.acoustic_connector.",
])
def test_whole_missing_components_rejected_without_native_runtime(worker_request, ready, prefix):
    shapes = checkpoints.vibevoice_architecture(tiny_vibe_config())["shapes"]
    write_vibe_weights(worker_request.model_dir, {key: shape for key, shape in shapes.items()
                                                 if not key.startswith(prefix)})
    report = pf.preflight(worker_request)
    assert "invalid_weights" in errors(report) and not report["ready"]


def test_same_parameter_count_wrong_shapes_rejected(worker_request, ready):
    shapes = checkpoints.vibevoice_architecture(tiny_vibe_config())["shapes"]
    shapes["model.language_model.layers.0.self_attn.k_proj.weight"] = (16, 8)
    write_vibe_weights(worker_request.model_dir, shapes)
    assert "invalid_weights" in errors(pf.preflight(worker_request))


def test_pinned_static_shapes_cover_codec_decoder_and_diffusion():
    arch = checkpoints.vibevoice_architecture(tiny_vibe_config())
    shapes = arch["shapes"]
    assert shapes["model.language_model.layers.0.self_attn.k_proj.weight"] == (8, 16)
    assert shapes["model.acoustic_tokenizer.encoder.downsample_layers.6.0.conv.conv.weight"] == (64, 32, 16)
    assert shapes["model.acoustic_tokenizer.decoder.upsample_layers.1.0.convtr.convtr.weight"] == (64, 32, 16)
    assert shapes["model.semantic_tokenizer.encoder.stages.6.0.mixer.conv.conv.conv.weight"] == (64, 1, 7)
    assert shapes["model.prediction_head.layers.0.ffn.gate_proj.weight"] == (48, 16)
    assert arch["fp32_bytes"] == sum(math.prod(shape) * 4 for shape in shapes.values())


@pytest.mark.parametrize("missing", ["lm_head.weight", "model.language_model.embed_tokens.weight"])
def test_only_known_tied_embedding_aliases_may_be_omitted(worker_request, ready, monkeypatch, missing):
    shapes = checkpoints.vibevoice_architecture(tiny_vibe_config())["shapes"]
    del shapes[missing]
    write_vibe_weights(worker_request.model_dir, shapes)
    calls = fake_vibevoice(monkeypatch)
    assert runtime.execute(worker_request)["synthesizedSpeech"]
    assert calls["meta_state_dict"]


def test_decoder_tie_flag_does_not_override_inference_root_flag(worker_request, ready):
    config = tiny_vibe_config()
    config["tie_word_embeddings"] = False
    config["decoder_config"]["tie_word_embeddings"] = True
    (worker_request.model_dir / "config.json").write_text(json.dumps(config))
    shapes = checkpoints.vibevoice_architecture(config)["shapes"]
    del shapes["lm_head.weight"]
    write_vibe_weights(worker_request.model_dir, shapes)
    assert "invalid_weights" in errors(pf.preflight(worker_request))


@pytest.mark.parametrize("mismatch", ["missing", "unexpected", "shape"])
def test_vibevoice_runtime_meta_metadata_checked_before_native_load(worker_request, ready, monkeypatch, mismatch):
    calls = fake_vibevoice(monkeypatch)
    cls = sys.modules["vibevoice.modular.modeling_vibevoice_inference"].VibeVoiceForConditionalGenerationInference
    if mismatch == "missing":
        cls.expected_shapes["runtime.additional_buffer"] = (1,)
    elif mismatch == "unexpected":
        del cls.expected_shapes["model.speech_bias_factor"]
    else:
        cls.expected_shapes["model.acoustic_connector.fc1.weight"] = (8, 8)
    with pytest.raises(WorkerError, match="architecture mismatch"):
        runtime.execute(worker_request)
    assert calls["meta_state_dict"] and "model" not in calls and "generate" not in calls
    assert not worker_request.output.exists()


def test_memory_rechecked_after_meta_before_native_load(worker_request, ready, monkeypatch):
    calls = fake_vibevoice(monkeypatch)
    snapshots = iter([100 * GIB, 100 * GIB, 100 * GIB, 1 * GIB])
    monkeypatch.setattr(pf, "memory_snapshot", lambda: {"total_bytes": 128 * GIB, "available_bytes": next(snapshots)})
    with pytest.raises(WorkerError, match="available"):
        runtime.execute(worker_request)
    assert calls["meta_state_dict"] and "model" not in calls


@pytest.mark.parametrize("mismatch", ["missing_depformer", "expanded_attention", "unexpected"])
def test_personaplex_meta_check_prevents_copying_or_partial_loading(worker_request, ready, monkeypatch, mismatch):
    req = persona_request(worker_request)
    calls = fake_personaplex(monkeypatch)
    if mismatch == "missing_depformer":
        calls["expected_shapes"] = {"depformer_in.0.weight": (1,), "depformer_in.8.weight": (1,)}
        write_shapes(req.model_dir / "model.safetensors", {"depformer_in.0.weight": (1,)})
    elif mismatch == "expanded_attention":
        key = "depformer.layers.0.self_attn.in_proj_weight"
        calls["expected_shapes"] = {key: (2, 1)}
        write_shapes(req.model_dir / "model.safetensors", {key: (1, 1)})
    else:
        write_shapes(req.model_dir / "model.safetensors", {"weight": (1,), "extra": (1,)})
    with pytest.raises(WorkerError, match="architecture mismatch"):
        runtime.execute(req)
    assert calls["meta_lm"]["copy_missing_weights"] is False
    assert not calls["mimi"] and "lm" not in calls and calls["steps"] == 0
    assert not req.output.exists()


def test_metadata_check_rejects_nonmeta_runtime_model():
    model = SimpleNamespace(parameters=lambda: [SimpleNamespace(is_meta=False)])
    with pytest.raises(WorkerError, match="meta parameters"):
        checkpoints.validate_meta_model(model, [])


@pytest.mark.parametrize("exception", [OSError, ValueError, RuntimeError, ImportError])
def test_cli_known_failures_are_reported_without_native_exception_secrets(worker_request, monkeypatch, capsys, exception):
    def fail(request):
        raise exception("Authorization: Bearer test-secret")
    monkeypatch.setattr(cli, "execute", fail)
    assert cli.main(["--engine", "vibevoice", "--model-dir", str(worker_request.model_dir)]) == 2
    captured = capsys.readouterr()
    assert json.loads(captured.out)["errors"][0]["code"] == "worker_failed"
    assert "test-secret" not in captured.out + captured.err


@pytest.mark.parametrize("exception", [AssertionError, TypeError, KeyError])
def test_cli_unexpected_programming_errors_propagate(worker_request, monkeypatch, capsys, exception):
    def fail(request):
        raise exception("unexpected defect")
    monkeypatch.setattr(cli, "execute", fail)
    with pytest.raises(exception):
        cli.main(["--engine", "vibevoice", "--model-dir", str(worker_request.model_dir)])
    assert capsys.readouterr().out == ""
