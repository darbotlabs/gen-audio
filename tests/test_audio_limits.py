from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf

from gen_audio.artifacts import publish_copy, staged_files
from gen_audio.audio_io import as_mono, read_wav, validate_sample_rate, write_wav
from gen_audio.cast import load_cast_map, parse_script, read_script
from gen_audio.guards import RequestLimitError, read_bounded_bytes, sha256_file, verify_model_checksums, write_bytes_exclusive
from gen_audio.improve import highpass, improve, peak_normalize, resample_audio, trim_silence


@pytest.mark.parametrize("rate", [0, -1, True, 24000.5, float("inf"), float("nan"), 192001])
def test_sample_rate_must_be_bounded_integer(rate):
    with pytest.raises(ValueError):
        validate_sample_rate(rate)


@pytest.mark.parametrize("audio", [
    np.array([np.nan]), np.array([np.inf]), np.array([1e300]), np.array(["x"]),
    np.ones((3, 9)), np.ones((1, 2, 3)), np.array([1 + 2j]),
])
def test_audio_conversion_rejects_invalid_samples(audio):
    with pytest.raises(ValueError):
        as_mono(audio)


def test_float32_channels_do_not_overflow_while_mixing():
    mixed = as_mono(np.full((4, 2), 3e38, dtype=np.float32))
    assert np.isfinite(mixed).all()


def test_wav_metadata_limit_is_checked_before_decode(tmp_path, monkeypatch):
    path = tmp_path / "long.wav"
    sf.write(path, np.ones(100), 1000)
    monkeypatch.setenv("GENAID_MAX_AUDIO_SAMPLES", "10")
    with pytest.raises(RequestLimitError):
        read_wav(path)


def test_encoded_file_byte_limit(tmp_path, monkeypatch):
    path = tmp_path / "large.wav"
    path.write_bytes(b"x" * 101)
    monkeypatch.setenv("GENAID_MAX_AUDIO_FILE_BYTES", "100")
    with pytest.raises(RequestLimitError):
        read_wav(path)
    with pytest.raises(RequestLimitError):
        publish_copy(path, tmp_path / "durable")
    destination = tmp_path / "new.wav"
    with pytest.raises(RequestLimitError):
        write_wav(destination, np.ones(100), 24000)
    assert not destination.exists()


def test_nonfinite_float_wav_cannot_be_loaded(tmp_path):
    path = tmp_path / "invalid.wav"
    sf.write(path, np.array([0.1, np.nan]), 24000, subtype="FLOAT")
    with pytest.raises(ValueError, match="NaN"):
        read_wav(path)


def test_empty_wav_cannot_be_loaded_or_written(tmp_path):
    path = tmp_path / "empty.wav"
    sf.write(path, np.empty(0), 24000)
    with pytest.raises(ValueError, match="empty"):
        read_wav(path)
    with pytest.raises(ValueError, match="empty"):
        write_wav(tmp_path / "new.wav", np.empty(0), 24000)
    assert not (tmp_path / "new.wav").exists()


def test_resampling_checks_expansion_before_scipy(monkeypatch):
    monkeypatch.setenv("GENAID_MAX_AUDIO_SAMPLES", "100")
    with pytest.raises(RequestLimitError):
        resample_audio(np.ones(100), 1000, 24000)


@pytest.mark.parametrize("call", [
    lambda audio: peak_normalize(audio, float("nan")),
    lambda audio: highpass(audio, 24000, cutoff_hz=float("inf")),
    lambda audio: highpass(audio, 24000, order=100000),
    lambda audio: trim_silence(audio, 24000, threshold_db=float("nan")),
    lambda audio: trim_silence(audio, 24000, pad_ms=float("inf")),
    lambda audio: improve(audio, True),
])
def test_publish_parameters_are_finite_and_bounded(call):
    with pytest.raises(ValueError):
        call(np.ones(1000))


def test_bounded_script_and_cast_reads(tmp_path, monkeypatch):
    monkeypatch.setenv("GENAID_MAX_TEXT_FILE_BYTES", "32")
    script = tmp_path / "script.txt"
    script.write_bytes(b"#" + b"x" * 100)
    with pytest.raises(RequestLimitError):
        read_script(script)
    with pytest.raises(RequestLimitError):
        load_cast_map(script)
    with pytest.raises(ValueError, match="regular"):
        read_bounded_bytes(tmp_path, 100)


def test_nan_speed_in_cast_and_huge_speaker_id_rejected(tmp_path):
    path = tmp_path / "cast.json"
    path.write_text(json.dumps({
        "engine": "kokoro_onnx", "speakers": {"1": {"voice": "af_heart", "speed": float("nan")}},
    }))
    with pytest.raises(ValueError):
        load_cast_map(path)
    with pytest.raises(ValueError):
        parse_script("Speaker " + "1" * 129 + ": hello")


def test_script_segments_limited_during_parse(monkeypatch):
    monkeypatch.setenv("GENAID_MAX_SCRIPT_SEGMENTS", "2")
    with pytest.raises(RequestLimitError):
        parse_script("\n".join(f"Speaker {number}: hello" for number in range(3)))


def test_no_clobber_is_atomic_against_racing_writer(tmp_path):
    wav, manifest = tmp_path / "out.wav", tmp_path / "out.json"
    with pytest.raises(FileExistsError):
        with staged_files([wav, manifest]) as staged:
            staged[0].write_bytes(b"new wave")
            staged[1].write_bytes(b"new manifest")
            manifest.write_bytes(b"concurrent writer")
    assert not wav.exists()
    assert manifest.read_bytes() == b"concurrent writer"
    assert not list(tmp_path.glob("*.partial"))


def test_failed_stage_does_not_touch_existing_single_file(tmp_path):
    path = tmp_path / "out.wav"
    path.write_bytes(b"original")
    with pytest.raises(RuntimeError):
        with staged_files([path], replace=True) as staged:
            staged[0].write_bytes(b"partial")
            raise RuntimeError("encode failed")
    assert path.read_bytes() == b"original"
    assert list(tmp_path.iterdir()) == [path]


def test_write_bytes_exclusive_rejects_existing_destination(tmp_path):
    write_bytes_exclusive(str(tmp_path), "out.wav", b"first")
    with pytest.raises(FileExistsError):
        write_bytes_exclusive(str(tmp_path), "out.wav", b"second")
    assert (tmp_path / "out.wav").read_bytes() == b"first"


def test_output_symlink_is_not_followed(tmp_path):
    target = tmp_path / "target.wav"
    target.write_bytes(b"keep")
    alias = tmp_path / "alias.wav"
    alias.symlink_to(target)
    with pytest.raises(FileExistsError):
        write_wav(alias, np.ones(20), 24000)
    assert target.read_bytes() == b"keep"


def test_checksum_pin_supports_spaces_and_rejects_duplicates(tmp_path, monkeypatch):
    monkeypatch.delenv("GENAID_MODEL_SHA256_FILE", raising=False)
    file = tmp_path / "model with spaces.onnx"
    file.write_bytes(b"fixture")
    pin = tmp_path / "SHA256SUMS"
    line = f"{hashlib.sha256(b'fixture').hexdigest()}  model with spaces.onnx\n"
    pin.write_text(line)
    assert verify_model_checksums(str(tmp_path), required_files=(file,))["verified"]
    pin.write_text(line + line)
    with pytest.raises(ValueError, match="duplicate"):
        verify_model_checksums(str(tmp_path))


def test_hashing_is_bounded(tmp_path):
    path = tmp_path / "large.bin"
    path.write_bytes(b"x" * 101)
    with pytest.raises(RequestLimitError):
        sha256_file(path, max_bytes=100)


def test_invalid_checksum_policy_fails_closed(tmp_path, monkeypatch):
    monkeypatch.setenv("GENAID_REQUIRE_MODEL_CHECKSUM", "treu")
    with pytest.raises(ValueError, match="boolean"):
        verify_model_checksums(str(tmp_path))
