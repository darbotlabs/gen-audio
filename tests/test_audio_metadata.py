import struct

import numpy as np
import pytest

from gen_audio.audio_metadata import agent_metadata, read_artist, tag_wav
from gen_audio.audio_io import read_wav, write_wav
from gen_audio.model_workers.contracts import Request
from gen_audio.model_workers.runtime import publish


def test_tags_preserve_exact_pcm_and_replace_artist_without_growth(tmp_path):
    path = tmp_path / "sample.wav"
    samples = np.sin(np.arange(2400) * 0.05).astype(np.float32) * 0.1
    write_wav(path, samples, 24000)
    before, rate = read_wav(path)
    metadata = tag_wav(path, ["Alice", "Frank", "Alice"])
    assert metadata == {"artist": "Alice; Frank", "artists": ["Alice", "Frank"], "agentNames": ["Alice", "Frank"]}
    assert read_artist(path) == "Alice; Frank"
    after, after_rate = read_wav(path)
    assert after_rate == rate
    assert np.array_equal(after, before)
    tag_wav(path, ["Jos\u00e9"])
    size = path.stat().st_size
    tag_wav(path, ["Jos\u00e9"])
    assert path.stat().st_size == size and read_artist(path) == "Jos\u00e9"


@pytest.mark.parametrize("names", [[""], ["Alice\nOther"], [" Alice"], ["x" * 129], ["Agent"] * 9])
def test_bad_artist_names_are_rejected(names):
    with pytest.raises(ValueError):
        agent_metadata(names)


def test_model_worker_publishes_artist_before_hashing(tmp_path):
    output = tmp_path / "voice.wav"
    request = Request(engine="vibevoice", model_dir=tmp_path, output=output, agent_names=("Alice", "Frank"))
    receipt = publish(request, np.sin(np.arange(2400) * 0.05).astype(np.float32) * 0.1, {})
    import hashlib
    import json
    manifest = json.loads(output.with_suffix(".synth.json").read_text(encoding="utf-8"))
    assert read_artist(output) == "Alice; Frank"
    assert receipt["sha256"] == hashlib.sha256(output.read_bytes()).hexdigest()
    assert manifest["metadata"] == receipt["metadata"]


def test_truncated_chunks_are_rejected_without_modifying_the_input(tmp_path):
    path = tmp_path / "broken.wav"
    raw = b"RIFF" + struct.pack("<I", 14) + b"WAVE" + b"LIST" + struct.pack("<I", 100) + b"xx"
    path.write_bytes(raw)
    with pytest.raises(ValueError):
        tag_wav(path, ["Alice"])
    assert path.read_bytes() == raw


def test_rf64_input_remains_supported_by_the_publish_chain(tmp_path):
    import soundfile as sf
    from gen_audio.cli.improve import main
    source, output = tmp_path / "source.wav", tmp_path / "published.wav"
    sf.write(source, np.sin(np.arange(2400) * 0.05) * 0.1, 24000, format="RF64", subtype="PCM_16")
    assert source.read_bytes()[:4] == b"RF64"
    assert read_artist(source) is None
    assert main([str(source), "-o", str(output)]) == 0
    assert read_wav(output)[0].size > 0
