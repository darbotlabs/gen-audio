from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest
import soundfile as sf

from gen_audio import artifacts
from gen_audio.cast import load_cast_map, read_script
from gen_audio.cli.synth import main
from gen_audio.synth_kokoro_onnx import KokoroOnnxSynthesizer, resolve_model_paths
from gen_audio.speakers import SpeechSegment, published_segments, remap_through_improve
from gen_audio.improve import improve


@pytest.fixture
def job(tmp_path, monkeypatch):
    for name in (
        "GENAID_MODEL_BASE", "GENAID_MODEL_SHA256_FILE", "GENAID_REQUIRE_MODEL_CHECKSUM",
        "GEN_AUDIO_ARTIFACT_DIR", "GENAID_MAX_AUDIO_SAMPLES", "GENAID_MAX_AUDIO_DURATION_S",
        "GENAID_MAX_GENERATION_SECONDS", "GENAID_MAX_AUDIO_FILE_BYTES",
    ):
        monkeypatch.delenv(name, raising=False)
    model = tmp_path / "model.onnx"
    voices = tmp_path / "voices.bin"
    model.write_bytes(b"fixture-model")
    voices.write_bytes(b"fixture-voices")
    script = tmp_path / "script.txt"
    script.write_text("Speaker 1: First private turn.\nSpeaker 2: Second private turn.\n", encoding="utf-8")
    cast = tmp_path / "cast.json"
    cast.write_text(json.dumps({
        "engine": "kokoro_onnx",
        "speakers": {
            "1": {"name": "Alice", "voice": "af_heart", "lang": "en-us", "speed": 1.0},
            "2": {"name": "Frank", "voice": "am_michael", "lang": "en-gb", "speed": 0.8},
        },
    }), encoding="utf-8")
    state = SimpleNamespace(calls=[], loads=[], responder=None)

    class Kokoro:
        def __init__(self, model_path, voices_path):
            state.loads.append((model_path, voices_path))
            self.voices = {"af_heart": None, "am_michael": None}

        def create(self, text, *, voice, speed, lang):
            state.calls.append((text, voice, speed, lang))
            if state.responder is not None:
                return state.responder()
            rate = 24000 if len(state.calls) == 1 else 16000
            audio = 0.25 * np.sin(np.arange(rate // 10) * (2 * np.pi * 440 / rate))
            return audio.astype(np.float32), rate

    monkeypatch.setitem(sys.modules, "kokoro_onnx", SimpleNamespace(Kokoro=Kokoro))
    output = tmp_path / "out.wav"
    argv = [
        str(script), "--cast-map", str(cast), "--model", str(model), "--voices", str(voices),
        "-o", str(output), "--gap", "0.01", "--json",
    ]
    return SimpleNamespace(
        root=tmp_path, model=model, voices=voices, script=script, cast=cast, state=state,
        output=output, argv=argv,
    )


def test_resident_cli_reuses_weights_and_rejects_changed_assets(job, capsys):
    resident = KokoroOnnxSynthesizer(job.model, job.voices)
    assert main(job.argv, resident=resident) == 0
    receipt = json.loads(capsys.readouterr().out)
    assert receipt["synthesizedSpeech"] and job.output.is_file()
    second = job.argv.copy()
    second[second.index("-o") + 1] = str(job.root / "second.wav")
    assert main(second, resident=resident) == 0
    assert len(job.state.loads) == 1
    job.model.write_bytes(b"changed-model")
    third = second.copy()
    third[third.index("-o") + 1] = str(job.root / "third.wav")
    assert main(third, resident=resident) == 1
    assert not (job.root / "third.wav").exists()


@pytest.mark.skipif(sys.platform != "win32", reason="Windows extended path regression")
def test_resident_accepts_extended_paths_for_the_same_files(job):
    resident = KokoroOnnxSynthesizer(job.model, job.voices)
    argv = job.argv.copy()
    for flag in ("--model", "--voices"):
        index = argv.index(flag) + 1
        argv[index] = "\\\\?\\" + argv[index]
    assert main(argv, resident=resident) == 0
    assert len(job.state.loads) == 1


def pin_fixture(job, *, voices=True):
    # These pins describe the known fixture payloads, not a production trust bootstrap.
    lines = [f"{hashlib.sha256(b'fixture-model').hexdigest()}  model.onnx"]
    if voices:
        lines.append(f"{hashlib.sha256(b'fixture-voices').hexdigest()}  voices.bin")
    (job.root / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")


def test_json_success_multi_turn_counts_hashes_and_privacy(job, capsys):
    assert main(job.argv) == 0
    receipt = json.loads(capsys.readouterr().out.strip().splitlines()[-1])
    assert set(receipt) == {
        "schema_version", "ok", "synthesizedSpeech", "engine", "output", "manifest",
        "sample_rate", "samples", "duration_s", "sha256", "metadata",
    }
    assert receipt["ok"] and receipt["synthesizedSpeech"]
    from gen_audio.audio_metadata import read_artist
    assert receipt["metadata"] == {"artist": "Alice; Frank", "artists": ["Alice", "Frank"], "agentNames": ["Alice", "Frank"]}
    assert read_artist(job.output) == receipt["metadata"]["artist"]
    assert receipt["schema_version"] == "1.0" and receipt["engine"] == "kokoro_onnx"
    assert receipt["output"] == str(job.output.resolve())
    assert Path(receipt["manifest"]).is_absolute()
    audio, rate = sf.read(job.output, dtype="float32")
    assert rate == receipt["sample_rate"] == 24000
    assert audio.size == receipt["samples"] == 5040
    assert receipt["duration_s"] == 0.21
    assert receipt["sha256"] == hashlib.sha256(job.output.read_bytes()).hexdigest()
    manifest = json.loads(Path(receipt["manifest"]).read_text())
    assert manifest["samples"] == 5040 and manifest["gap_samples"] == 240
    assert manifest["turn_count"] == manifest["speaker_count"] == 2
    assert manifest["pre_improve_samples"] == 5040
    assert manifest["wav_subtype"] == "FLOAT" and manifest["sha256"] == receipt["sha256"]
    assert manifest["turns"][1]["source_samples"] == 1600
    assert manifest["turns"][1]["samples"] == 2400
    assert manifest["turns"][1]["start_sample"] == 2640
    assert manifest["turns"][1]["source_sample_rate"] == 16000
    assert "private turn" not in json.dumps(manifest)
    assert all("text" not in turn for turn in manifest["turns"])
    assert manifest["provenance"]["model"]["sha256"] == hashlib.sha256(b"fixture-model").hexdigest()
    assert manifest["provenance"]["voices"]["path"] == str(job.voices)
    assert not manifest["provenance"]["checksums"]["verified"]
    assert job.state.calls == [
        ("First private turn.", "af_heart", 1.0, "en-us"),
        ("Second private turn.", "am_michael", 0.8, "en-gb"),
    ]


def test_improved_success_and_explicit_text_opt_in(job, capsys):
    assert main(job.argv + ["--improve", "--include-text"]) == 0
    receipt = json.loads(capsys.readouterr().out)
    info = sf.info(receipt["output"])
    manifest = json.loads(Path(receipt["manifest"]).read_text())
    assert info.subtype == "PCM_16" and info.frames == receipt["samples"]
    assert info.samplerate == receipt["sample_rate"] == 24000
    assert manifest["improved"] and manifest["text_included"]
    assert manifest["turns"][0]["text"] == "First private turn."
    assert manifest["turn_timing_basis"] == "pre_improve"


def test_adapter_retains_raw_receipts_and_published_speaker_cursors(job):
    def respond():
        rate = 16000
        speech = 0.25 * np.sin(np.arange(1600) * (2 * np.pi * 440 / rate))
        return np.concatenate([np.zeros(1600), speech, np.zeros(1600)]).astype(np.float32), rate

    job.state.responder = respond
    synth = KokoroOnnxSynthesizer(job.model, job.voices)
    turns, cast = read_script(job.script), load_cast_map(job.cast)
    raw = synth.synthesize_turns(turns, cast, gap_s=0.01)
    assert raw.segments == [SpeechSegment(0, 0, 4800), SpeechSegment(1, 4960, 9760)]
    expected_audio = improve(raw.audio, raw.sample_rate)
    result = synth.synthesize_turns(turns, cast, gap_s=0.01, improve_publish=True)
    assert result.sample_rate == 24000
    assert result.segments == remap_through_improve(
        raw.segments, trim_start=expected_audio.trim_start,
        trimmed_len=expected_audio.trimmed_samples, published_len=expected_audio.audio.size,
    )
    manifest = result.manifest()
    assert manifest["segments"] == published_segments(result.segments, result.sample_rate)
    assert manifest["speakers"] == [
        {"idx": 0, "persona": "Alice", "voice": "af_heart", "engine": "kokoro_onnx"},
        {"idx": 1, "persona": "Frank", "voice": "am_michael", "engine": "kokoro_onnx"},
    ]
    assert manifest["turns"][1]["start_sample"] == 4960
    assert manifest["turns"][1]["sample_rate"] == 16000


def test_generate_orchestrator_uses_adapter_timeline_and_clean_json(job, monkeypatch, capsys):
    from gen_audio.asr_wer import AsrError
    from gen_audio.cli.generate import main as generate
    from gen_audio.video_render import VideoError

    monkeypatch.setenv("GEN_AUDIO_KOKORO_MODEL", str(job.model))
    monkeypatch.setenv("GEN_AUDIO_KOKORO_VOICES", str(job.voices))
    monkeypatch.setattr("gen_audio.adapters._module_present", lambda name: name == "kokoro_onnx")

    def no_asr(_path):
        raise AsrError("fixture has no ASR weights")

    def no_video(*_args):
        raise VideoError("fixture has no encoder")

    def respond():
        print("backend diagnostic")
        return (0.25 * np.sin(np.arange(4800) * (2 * np.pi * 440 / 24000))).astype(np.float32), 24000

    monkeypatch.setattr("gen_audio.pipeline.transcribe", no_asr)
    monkeypatch.setattr("gen_audio.pipeline.render_clip_mp4", no_video)
    job.state.responder = respond
    assert generate([
        "--prompt-file", str(job.script), "--personas", "alice,frank",
        "--engine", "kokoro_onnx", "--duration-s", "1", "--out-dir", str(job.root),
    ]) == 0
    captured = capsys.readouterr()
    receipt = json.loads(captured.out)
    assert receipt["ok"] and receipt["synthesizedSpeech"]
    assert "backend diagnostic" in captured.err
    doc = json.loads((job.root / "cube.json").read_text())
    assert [speaker["persona"] for speaker in doc["speakers"]] == ["Alice", "Frank"]
    assert len(doc["segments"]) == 2
    assert doc["provenance"]["layer_method"] == "library_r3"
    manifest = json.loads((job.root / "manifest.json").read_text())
    assert not manifest["compare"]["measuredHere"]
    assert "fixture has no ASR weights" in manifest["compare"]["unavailable"]
    assert manifest["files"]["wav"] == "fitted.wav"
    assert (job.root / "spectrogram.png").is_file()


def test_publication_keeps_labeled_input_provenance_with_hashes(job, capsys):
    assert main(job.argv) == 0
    receipt = json.loads(capsys.readouterr().out)
    manifest = json.loads(Path(receipt["manifest"]).read_text())
    from gen_audio.cli.synth import REPO_ROOT
    from gen_audio.library_manifest import OUTSIDE, outside_repo_label

    for name, path in {"script": job.script, "cast_map": job.cast, "model": job.model, "voices": job.voices,
                       "output": job.output}.items():
        label = outside_repo_label(str(path), REPO_ROOT)
        assert manifest["inputs"][name] == label
        if label.startswith(OUTSIDE + "/"):
            assert manifest["outside_repo"][label] == {
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "bytes": path.stat().st_size,
            }


def test_artifact_copy_is_verified_and_each_manifest_points_to_its_wav(job, capsys):
    durable = job.root / "durable"
    assert main(job.argv + ["--artifact-dir", str(durable)]) == 0
    receipt = json.loads(capsys.readouterr().out)
    assert receipt["output"] == str(durable / "out.wav")
    assert (durable / "out.wav").read_bytes() == job.output.read_bytes()
    for root in (job.root, durable):
        manifest = json.loads((root / "out.json").read_text())
        assert manifest["output"] == str(root / "out.wav")
        assert manifest["manifest"] == str(root / "out.json")
        assert manifest["sha256"] == receipt["sha256"]
    assert not list(job.root.rglob("*.partial"))


@pytest.mark.parametrize("which", ["wav", "json", "durable"])
def test_collision_is_rejected_before_model_load(job, capsys, which):
    path = job.output if which == "wav" else job.output.with_suffix(".json")
    args = job.argv
    if which == "durable":
        durable = job.root / "durable"
        durable.mkdir()
        path = durable / "out.json"
        args = args + ["--artifact-dir", str(durable)]
    path.write_bytes(b"keep")
    assert main(args) == 1
    assert capsys.readouterr().out == ""
    assert path.read_bytes() == b"keep"
    assert not job.state.loads


@pytest.mark.parametrize("bad", [
    (np.array([], dtype=np.float32), 24000),
    (np.zeros(3, dtype=np.float32), 24000),
    (np.array([np.nan], dtype=np.float32), 24000),
    (np.array([np.inf], dtype=np.float32), 24000),
    (np.array([1e300]), 24000),
    (np.ones((2, 2), dtype=np.float32), 24000),
    (np.array(["1"]), 24000),
    (np.ones(4), 0), (np.ones(4), True), (np.ones(4), 1.5),
    (np.ones(4), float("nan")), (np.ones(4), 192001),
])
def test_invalid_engine_result_never_publishes_success(job, capsys, bad):
    job.state.responder = lambda: bad
    assert main(job.argv) == 1
    assert capsys.readouterr().out == ""
    assert not job.output.exists() and not job.output.with_suffix(".json").exists()
    assert not list(job.root.glob("*.partial"))


@pytest.mark.parametrize("flag,value", [
    ("--gap", "nan"), ("--gap", "inf"), ("--gap", "-1"), ("--gap", "601"),
    ("--max-duration", "nan"), ("--max-duration", "0"),
    ("--generation-timeout", "inf"), ("--generation-timeout", "-1"),
])
def test_invalid_limits_rejected_before_loading(job, capsys, flag, value):
    assert main(job.argv + [flag, value]) == 1
    assert not job.state.loads
    assert capsys.readouterr().out == ""


def test_cumulative_duration_cap(job, capsys):
    assert main(job.argv + ["--max-duration", "0.15"]) == 1
    assert len(job.state.calls) == 2
    assert not job.output.exists()
    assert capsys.readouterr().out == ""


def test_sample_ceiling_rejects_before_resampling(job, monkeypatch, capsys):
    monkeypatch.setenv("GENAID_MAX_AUDIO_SAMPLES", "2500")
    assert main(job.argv) == 1
    assert not job.output.exists()
    assert capsys.readouterr().out == ""


def test_native_error_in_second_turn_has_no_partial_output(job, capsys):
    def respond():
        if len(job.state.calls) == 2:
            raise RuntimeError("inference failed")
        return np.ones(2400, dtype=np.float32), 24000
    job.state.responder = respond
    assert main(job.argv) == 1
    assert capsys.readouterr().out == ""
    assert not job.output.exists() and not job.output.with_suffix(".json").exists()


def test_late_unknown_voice_fails_before_any_inference(job):
    cast = json.loads(job.cast.read_text())
    cast["speakers"]["2"]["voice"] = "missing_voice"
    job.cast.write_text(json.dumps(cast))
    assert main(job.argv) == 1
    assert not job.state.calls


def test_deadline_failure_discards_completed_inference(job, monkeypatch, capsys):
    clock = [0.0]
    monkeypatch.setattr("gen_audio.synthesis.time.monotonic", lambda: clock[0])
    def respond():
        clock[0] = 11.0
        return np.ones(2400, dtype=np.float32), 24000
    job.state.responder = respond
    assert main(job.argv + ["--generation-timeout", "5"]) == 1
    assert capsys.readouterr().out == ""
    assert not job.output.exists()


def test_checksums_required_without_model_base_fails_closed(job, monkeypatch):
    monkeypatch.setenv("GENAID_REQUIRE_MODEL_CHECKSUM", "1")
    with pytest.raises(ValueError, match="no SHA256SUMS"):
        resolve_model_paths(job.model, job.voices)
    assert main(job.argv) == 1
    assert not job.state.loads


def test_required_pins_cover_both_model_and_voice_files(job, monkeypatch, capsys):
    pin_fixture(job)
    monkeypatch.setenv("GENAID_REQUIRE_MODEL_CHECKSUM", "1")
    assert main(job.argv) == 0
    receipt = json.loads(capsys.readouterr().out)
    provenance = json.loads(Path(receipt["manifest"]).read_text())["provenance"]
    assert provenance["checksums"]["verified"] and provenance["checksums"]["files"] == 2
    assert provenance["checksums"]["source"] == str(job.root / "SHA256SUMS")


@pytest.mark.parametrize("target", ["model", "voices"])
def test_tampered_pinned_files_rejected_before_loading(job, target):
    pin_fixture(job)
    getattr(job, target).write_bytes(b"tampered")
    assert main(job.argv) == 1
    assert not job.state.loads and not job.output.exists()


def test_partial_pin_is_not_sufficient_even_when_policy_optional(job):
    pin_fixture(job, voices=False)
    with pytest.raises(ValueError, match="every loaded model and voices"):
        KokoroOnnxSynthesizer(job.model, job.voices)
    assert not job.state.loads


def test_voices_cannot_escape_configured_base(job, monkeypatch):
    base = job.root / "confined"
    base.mkdir()
    model = base / "model.onnx"
    model.write_bytes(b"model")
    monkeypatch.setenv("GENAID_MODEL_BASE", str(base))
    with pytest.raises(ValueError, match="inside GENAID_MODEL_BASE"):
        resolve_model_paths(model, job.voices)


def test_publish_failure_rolls_back_wav_and_stages(job, monkeypatch, capsys):
    original = artifacts.os.link
    def fail_manifest(src, dest):
        if Path(dest).suffix == ".json":
            raise OSError("injected manifest failure")
        original(src, dest)
    monkeypatch.setattr(artifacts.os, "link", fail_manifest)
    assert main(job.argv) == 1
    assert capsys.readouterr().out == ""
    assert not job.output.exists() and not job.output.with_suffix(".json").exists()
    assert not list(job.root.glob("*.partial"))


def test_copy_failure_rolls_back_all_destinations(job, monkeypatch, capsys):
    def fail_copy(src, dest):
        raise OSError("injected copy failure")
    monkeypatch.setattr("gen_audio.cli.synth.shutil.copyfile", fail_copy)
    durable = job.root / "durable"
    assert main(job.argv + ["--artifact-dir", str(durable)]) == 1
    assert capsys.readouterr().out == ""
    assert not job.output.exists()
    assert not list(durable.iterdir()) and not list(job.root.glob("*.partial"))


def test_adapter_facade_and_manifest_keep_backward_imports(job):
    synth = KokoroOnnxSynthesizer(job.model, job.voices)
    result = synth.synthesize_turns(read_script(job.script), load_cast_map(job.cast))
    assert result.manifest()["engine"] == "kokoro_onnx"
    assert result.manifest()["seconds"] == result.audio.size / result.sample_rate
