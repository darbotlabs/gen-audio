"""Unit tests for GenAID Audio request and path guards."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path

import pytest

from gen_audio.guards import (
    RequestLimitError,
    classify_upstream_failure,
    enforce_text_limit,
    materialize_temp_audio,
    normalize_output_format,
    reject_voice_prompt_reference,
    resolve_model_directory,
    resolve_voice_file,
    safe_storage_name,
    sanitize_header_value,
    validate_script_segments,
    verify_model_checksums,
    write_bytes_exclusive,
)


def test_output_format_allowlist():
    assert normalize_output_format(" WAV ") == "wav"
    assert normalize_output_format("mp3") == "mp3"
    with pytest.raises(ValueError):
        normalize_output_format("wav/../../etc/cron.d/evil")
    with pytest.raises(ValueError):
        normalize_output_format("")


def test_text_limit_rejects_unbounded_input():
    enforce_text_limit("hello", "tts")
    with pytest.raises(RequestLimitError):
        enforce_text_limit("x" * 8001, "tts")
    with pytest.raises(RequestLimitError):
        enforce_text_limit("bad\x00text", "prompt")


def test_script_segment_caps():
    validate_script_segments([{"speaker": "Speaker 1", "text": "Hello"}])
    with pytest.raises(RequestLimitError):
        validate_script_segments([{"speaker": "A", "text": "x" * 4001}])
    with pytest.raises(RequestLimitError):
        validate_script_segments([])


def test_voice_prompt_reference_rejects_urls_and_traversal():
    assert reject_voice_prompt_reference(None) is None
    assert reject_voice_prompt_reference(" en-Alice_woman.wav ") == "en-Alice_woman.wav"
    with pytest.raises(ValueError):
        reject_voice_prompt_reference("https://example.test/voice.wav")
    with pytest.raises(ValueError):
        reject_voice_prompt_reference("voices/../../etc/passwd")
    with pytest.raises(ValueError):
        reject_voice_prompt_reference("file:/etc/passwd")


def test_voice_file_confinement(tmp_path: Path):
    voices = tmp_path / "voices"
    voices.mkdir()
    sample = voices / "en-Alice_woman.wav"
    sample.write_bytes(b"RIFFfake")
    secret = tmp_path / "secret.wav"
    secret.write_bytes(b"RIFFsecret")
    escaped = voices / "link.wav"
    escaped.symlink_to(secret)

    resolved = resolve_voice_file("en-Alice_woman.wav", str(voices))
    assert resolved == os.path.realpath(sample)
    assert resolve_voice_file(str(sample), str(voices)) == resolved

    with pytest.raises(ValueError):
        resolve_voice_file("../secret.wav", str(voices))
    with pytest.raises(ValueError):
        resolve_voice_file(str(secret), str(voices))
    with pytest.raises(ValueError):
        resolve_voice_file(str(escaped), str(voices))
    with pytest.raises(ValueError):
        resolve_voice_file("notes.txt", str(voices))


def test_model_directory_confinement(tmp_path: Path):
    base = tmp_path / "VibeVoice"
    model = base / "1.5B"
    model.mkdir(parents=True)
    other = tmp_path / "other-model"
    other.mkdir()

    assert resolve_model_directory(str(model), str(base)) == os.path.realpath(model)
    assert resolve_model_directory("1.5B", str(base)) == os.path.realpath(model)
    with pytest.raises(ValueError):
        resolve_model_directory(str(other), str(base))
    with pytest.raises(FileNotFoundError):
        resolve_model_directory(str(base / "7B"), str(base))


def test_checksum_pin_verifies_and_rejects_mismatch(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    model = tmp_path / "1.5B"
    model.mkdir()
    weight = model / "model.safetensors"
    payload = b"weights"
    weight.write_bytes(payload)
    digest = hashlib.sha256(payload).hexdigest()
    pin = model / "SHA256SUMS"
    pin.write_text(f"{digest}  model.safetensors\n", encoding="utf-8")
    monkeypatch.delenv("GENAID_MODEL_SHA256_FILE", raising=False)
    monkeypatch.delenv("GENAID_REQUIRE_MODEL_CHECKSUM", raising=False)

    result = verify_model_checksums(str(model))
    assert result == {"verified": True, "skipped": False, "files": 1}

    weight.write_bytes(b"tampered")
    with pytest.raises(ValueError, match="checksum mismatch"):
        verify_model_checksums(str(model))


def test_checksum_required_fails_closed(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    model = tmp_path / "1.5B"
    model.mkdir()
    monkeypatch.delenv("GENAID_MODEL_SHA256_FILE", raising=False)
    monkeypatch.setenv("GENAID_REQUIRE_MODEL_CHECKSUM", "1")
    with pytest.raises(ValueError, match="no SHA256SUMS"):
        verify_model_checksums(str(model))


def test_checksum_pin_rejects_path_escape(tmp_path: Path):
    model = tmp_path / "1.5B"
    model.mkdir()
    outside = tmp_path / "outside.txt"
    outside.write_bytes(b"secret")
    digest = hashlib.sha256(b"secret").hexdigest()
    (model / "SHA256SUMS").write_text(f"{digest}  ../outside.txt\n", encoding="utf-8")
    with pytest.raises(ValueError, match="unsafe path"):
        verify_model_checksums(str(model))


def test_temp_audio_is_removed_when_writer_fails(tmp_path: Path):
    def _boom(_path: str) -> None:
        raise RuntimeError("encode failed")

    with pytest.raises(RuntimeError, match="encode failed"):
        materialize_temp_audio(str(tmp_path), ".wav", _boom)
    assert list(tmp_path.iterdir()) == []


def test_temp_audio_returns_bytes_and_unlinks(tmp_path: Path):
    def _write(path: str) -> None:
        Path(path).write_bytes(b"RIFF")

    data = materialize_temp_audio(str(tmp_path), ".wav", _write)
    assert data == b"RIFF"
    assert list(tmp_path.iterdir()) == []


def test_output_write_stays_inside_directory(tmp_path: Path):
    output = tmp_path / "audio"
    saved = write_bytes_exclusive(str(output), "../podcast_x.wav", b"abc")
    assert os.path.dirname(saved) == os.path.realpath(output)
    assert Path(saved).read_bytes() == b"abc"
    assert not (tmp_path / "podcast_x.wav").exists()
    with pytest.raises(ValueError):
        safe_storage_name("..")
    with pytest.raises(ValueError):
        materialize_temp_audio(str(output), ".wav/../../x", lambda _path: None)


def test_header_sanitizer_strips_crlf():
    assert "\r" not in sanitize_header_value("line\r\nSet-Cookie: x")
    assert "\n" not in sanitize_header_value("line\r\nSet-Cookie: x")
    assert len(sanitize_header_value("a" * 800, limit=10)) == 10


def test_upstream_failure_classification():
    class TimeoutException(Exception):
        pass

    class ConnectError(Exception):
        pass

    class HTTPStatusError(Exception):
        def __init__(self):
            super().__init__("status")
            self.response = type(
                "Response",
                (),
                {"status_code": 422, "text": '{"detail":"bad voice"}', "headers": {"content-type": "application/json"}},
            )()

    assert classify_upstream_failure(TimeoutException())[0] == 504
    assert classify_upstream_failure(ConnectError())[0] == 503
    status, detail = classify_upstream_failure(HTTPStatusError())
    assert status == 422
    assert "bad voice" in detail

    class ServerStatus(Exception):
        def __init__(self):
            super().__init__("status")
            self.response = type("Response", (), {"status_code": 500, "text": "boom", "headers": {"content-type": "text/plain"}})()

    assert classify_upstream_failure(ServerStatus())[0] == 502
