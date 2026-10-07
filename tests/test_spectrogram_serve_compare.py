from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest

from gen_audio.artifacts import publish_copy
from gen_audio.audio_io import write_wav
from gen_audio.cli.compare import main as compare_main
from gen_audio.cli.cube import main as cube_main
from gen_audio.cli.improve import main as improve_main
from gen_audio.cli.spectrogram import main as spectrogram_main
from gen_audio.compare import score_wav
from gen_audio.engines import ENGINES, get_engine
from gen_audio.serve import (
    SHARED_GATEWAY_HOST,
    health_payload,
    health_url,
    node_base_url,
    power_table_row,
    shared_gateway_row,
)
from gen_audio.spectrogram import write_before_after_panel, write_spectrogram


def _tone(path: Path, sample_rate: int = 16_000) -> None:
    count = sample_rate // 2
    time = np.arange(count) / sample_rate
    audio = (0.4 * np.sin(2 * np.pi * 440 * time)).astype(np.float32)
    write_wav(path, audio, sample_rate)


def test_spectrogram_panel(tmp_path: Path):
    before = _array()
    after = before * 0.5
    single = write_spectrogram(tmp_path / "one.png", before, 16_000, title="before")
    panel = write_before_after_panel(tmp_path / "panel.png", before, 16_000, after, 24_000, title="fixture")
    assert single.stat().st_size > 1000
    assert panel.stat().st_size > 1000


def test_short_audio_is_refused():
    with pytest.raises(ValueError, match="too short"):
        write_spectrogram("ignored.png", np.zeros(10, dtype=np.float32), 16_000, title="x")


def test_node_urls_keep_prefix_and_shared_gateway_is_its_own_row():
    assert node_base_url("10.1.8.21") == "http://10.1.8.21:8002/genaid-audio"
    assert health_url("10.1.8.21") == "http://10.1.8.21:8002/genaid-audio/health"
    assert health_payload()["service"] == "genaid-audio"
    row = power_table_row("node-a", "10.1.8.21")
    shared = shared_gateway_row()
    assert row["base_url"] != shared["base_url"]
    assert shared["host"] == SHARED_GATEWAY_HOST
    assert shared["role"] == "shared-gateway"
    assert str(shared["health_url"]).endswith("/genaid-audio/health")
    with pytest.raises(ValueError):
        node_base_url("http://10.1.8.21")
    with pytest.raises(ValueError):
        node_base_url("10.1.8.21:8002")


def test_compare_marks_external_engine_and_unknown_id(tmp_path: Path):
    wav = tmp_path / "take.wav"
    _tone(wav)
    known = score_wav("pocket_tts", wav)
    assert known["registered"] is True
    assert known["engine_status"] == "external"
    assert get_engine("kokoro_onnx").status == "implemented"
    assert "misaki" in ENGINES
    assert ENGINES["misaki"].kind == "g2p"
    unknown = score_wav("local-render", wav)
    assert unknown["registered"] is False
    assert unknown["metrics"]["n"] > 0


def test_publish_copy_uses_explicit_dir(tmp_path: Path):
    source = tmp_path / "note.txt"
    source.write_text("ok\n", encoding="utf-8")
    copied = publish_copy(source, tmp_path / "durable")
    assert copied.read_text(encoding="utf-8") == "ok\n"
    assert copied.parent == tmp_path / "durable"


def test_cli_improve_spectrogram_and_cube(tmp_path: Path):
    source = tmp_path / "in.wav"
    _tone(source)
    published = tmp_path / "out.wav"
    assert improve_main([str(source), "-o", str(published)]) == 0
    assert published.is_file()
    spec_dir = tmp_path / "spec"
    assert spectrogram_main(["--before", str(source), "--after", str(published), "--out-dir", str(spec_dir)]) == 0
    assert (spec_dir / "spectrogram_panel.png").is_file()
    revised = tmp_path / "cube.wav"
    plot = tmp_path / "cube.png"
    assert cube_main([str(published), "-o", str(revised), "--plot", str(plot)]) == 0
    report = json.loads(revised.with_suffix(".json").read_text(encoding="utf-8"))
    assert "before" in report and "after" in report
    scores = tmp_path / "scores.json"
    assert compare_main([f"kokoro_onnx={published}", "-o", str(scores)]) == 0
    payload = json.loads(scores.read_text(encoding="utf-8"))
    assert payload[0]["engine"] == "kokoro_onnx"
    assert payload[0]["engine_status"] == "implemented"


def _array() -> np.ndarray:
    sample_rate = 16_000
    time = np.arange(sample_rate // 2) / sample_rate
    return (0.4 * np.sin(2 * np.pi * 440 * time)).astype(np.float32)
