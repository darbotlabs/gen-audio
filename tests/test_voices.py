"""Voice schemas, the framework registry, and the catalog resolver."""

from __future__ import annotations

import importlib
import json
from pathlib import Path

import numpy as np
import pytest

from gen_audio.cast import load_cast_map, parse_script, resolve_character
from gen_audio.synth_kokoro_onnx import KokoroOnnxSynthesizer
from gen_audio.voices import (
    VoiceError,
    blend_from_pack,
    blend_vectors,
    get_framework,
    library_root,
    list_frameworks,
    load_catalog,
    load_voice,
    missing_assets,
    mock_offline_request,
    register,
    unregister,
    validate_cast,
    validate_voice,
)
from gen_audio.voices.model import load_g2p
from gen_audio.voices.plan import SynthesisPlan
from gen_audio.voices.registry import FrameworkSpec
from gen_audio.cli.voices import main

ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "voices" / "catalog.example.json"
CAST = ROOT / "voices" / "cast_map.example.json"


def _voice(**overrides) -> dict:
    payload = {
        "schema_version": "1.0.0",
        "id": "example_tts.demo",
        "display_name": "Demo",
        "language": "en",
        "gender": "female",
        "framework": "example_tts",
        "assets": [{"role": "prompt", "path": "example/demo.wav", "format": "wav"}],
        "source": {
            "name": "Example",
            "url": "https://example.com/voices",
            "license": "MIT",
        },
        "tags": ["demo"],
        "params": {"token": "ok"},
    }
    payload.update(overrides)
    return payload


def test_example_catalog_covers_every_builtin_framework():
    catalog = load_catalog(CATALOG)
    assert catalog.catalog_source == "naming-conventions"
    assert catalog.library_env == "GENAID_VOICES_DIR"
    frameworks = {voice.framework for voice in catalog.voices}
    builtin = {spec.id for spec in list_frameworks() if spec.kind != "g2p"}
    assert frameworks == builtin
    assert {item.id for item in catalog.g2p} >= {"misaki.en-us", "misaki.en-gb", "misaki.ja", "misaki.zh", "misaki.es"}
    heart = catalog.voice("kokoro_onnx.af_heart")
    assert heart.params["voice_id"] == "af_heart"
    assert heart.g2p == "misaki.en-us"
    assert catalog.g2p_config("misaki.en-us").british is False
    assert catalog.g2p_config("misaki.en-gb").variant == "en-gb"
    de = catalog.voice("livevoice.de-Spk0_man")
    assert de.params["experimental"] is True
    assert "experimental_voices/" in de.assets[0].path


def test_mixed_cast_keeps_alice_and_frank_and_resolves_other_frameworks():
    cast = load_cast_map(CAST)
    catalog = load_catalog(CATALOG)
    assert cast.engine == "kokoro_onnx"
    assert cast.speakers["1"].voice == "af_heart"
    assert cast.speakers["2"].name == "Frank"
    assert cast.speakers["3"].framework == "vibevoice"
    assert resolve_character(cast, "alice", "kokoro") == "kokoro.af_heart"
    assert resolve_character(cast, "alice", "personaplex") == "personaplex.NATF2"
    assert resolve_character(cast, "carter", "livevoice") == "livevoice.en-Carter_man"
    validate_cast(cast, catalog)
    with pytest.raises(KeyError, match="known frameworks"):
        resolve_character(cast, "bowen", "kokoro_onnx")


def test_kokoro_synth_refuses_a_vibevoice_speaker_in_the_mixed_cast():
    synth = KokoroOnnxSynthesizer.__new__(KokoroOnnxSynthesizer)
    cast = load_cast_map(CAST)
    turns = parse_script("Speaker 3: Hello from Bowen.\n")
    with pytest.raises(ValueError, match="vibevoice"):
        synth.synthesize_turns(turns, cast)


def test_vibevoice_speed_is_not_forced_into_the_kokoro_range(tmp_path: Path):
    path = tmp_path / "cast.json"
    path.write_text(
        json.dumps(
            {
                "engine": "vibevoice",
                "default_speed": 3,
                "speakers": {"1": {"name": "Bowen", "voice": "zh-Bowen_man"}},
            }
        ),
        encoding="utf-8",
    )
    cast = load_cast_map(path)
    assert cast.speakers["1"].speed == 3
    assert cast.speakers["1"].framework == "vibevoice"


def test_vibevoice_cast_rejects_a_fifth_speaker(tmp_path: Path):
    speakers = {
        str(index): {"name": f"S{index}", "voice": f"en-S{index}_man", "framework": "vibevoice"}
        for index in range(1, 6)
    }
    path = tmp_path / "cast.json"
    path.write_text(json.dumps({"schema_version": "1.0.0", "engine": "vibevoice", "speakers": speakers}), encoding="utf-8")
    cast = load_cast_map(path)
    with pytest.raises(VoiceError, match="at most 4"):
        validate_cast(cast)


def test_livevoice_rejects_a_second_speaker(tmp_path: Path):
    path = tmp_path / "cast.json"
    path.write_text(
        json.dumps(
            {
                "engine": "livevoice",
                "speakers": {
                    "1": {"name": "A", "voice": "en-Carter_man"},
                    "2": {"name": "B", "voice": "de-Spk0_man"},
                },
            }
        ),
        encoding="utf-8",
    )
    with pytest.raises(VoiceError, match="at most 1"):
        validate_cast(load_cast_map(path))


def test_voice_id_prefix_path_and_gender_are_rejected():
    heart = json.loads((ROOT / "voices" / "kokoro" / "af_heart.json").read_text(encoding="utf-8"))
    heart["id"] = "kokoro_onnx.af_heart"
    with pytest.raises(VoiceError, match="must start with kokoro"):
        validate_voice(heart)
    heart = json.loads((ROOT / "voices" / "kokoro" / "af_heart.json").read_text(encoding="utf-8"))
    heart["assets"][0]["path"] = "../secrets/af_heart.pt"
    with pytest.raises(VoiceError, match="library"):
        validate_voice(heart)
    heart["assets"][0]["path"] = "kokoro/voices/af_heart.pt"
    heart["gender"] = "male"
    with pytest.raises(VoiceError, match="gender"):
        validate_voice(heart)


def test_blend_weights_must_sum_to_one_and_the_vector_mix_is_weighted():
    payload = json.loads((ROOT / "voices" / "kokoro" / "blend_heart_bella.json").read_text(encoding="utf-8"))
    payload["params"]["blend"][0]["weight"] = 0.9
    with pytest.raises(VoiceError, match="sum to 1"):
        validate_voice(payload)
    left = np.array([1.0, 0.0], dtype=np.float32)
    right = np.array([0.0, 1.0], dtype=np.float32)
    mixed = blend_vectors({"af_heart": left, "af_bella": right}, [("af_heart", 0.75), ("af_bella", 0.25)])
    assert mixed.tolist() == pytest.approx([0.75, 0.25])
    with pytest.raises(VoiceError, match="not in the style pack"):
        blend_vectors({"af_heart": left}, [("af_heart", 0.5), ("af_missing", 0.5)])


def test_blend_from_pack_reads_an_npz(tmp_path: Path):
    path = tmp_path / "voices-v1.0.bin"
    with path.open("wb") as handle:
        np.savez(handle, af_heart=np.array([2.0, 0.0], dtype=np.float32), af_nicole=np.array([0.0, 2.0], dtype=np.float32))
    mixed = blend_from_pack(path, [("af_heart", 0.5), ("af_nicole", 0.5)])
    assert mixed.tolist() == pytest.approx([1.0, 1.0])


def test_plugin_framework_registers_and_unregisters():
    def validate_params(params, *, language, gender, assets):
        del language, gender, assets
        if params.get("token") != "ok":
            raise VoiceError("token")
        return dict(params)

    def prepare(voice, library=None):
        del library
        return SynthesisPlan(
            framework="example_tts",
            voice_id=voice.id,
            engine_id=None,
            kind="synth",
            wired=False,
            executable=False,
            missing=("example runtime is not bundled",),
            params=voice.params,
            resolved_files={},
        )

    spec = FrameworkSpec(
        id="example_tts",
        kind="synth",
        engine_id=None,
        summary="Process-local example framework.",
        wired=False,
        params_schema="frameworks/example_tts.v1.schema.json",
        validate_params=validate_params,
        prepare=prepare,
        speaker_limit=2,
    )
    register(spec)
    try:
        voice = validate_voice(_voice())
        plan = get_framework("example_tts").prepare(voice, None)
        assert plan.executable is False
        assert voice.framework == "example_tts"
        assert any(item.id == "example_tts" for item in list_frameworks())
    finally:
        unregister("example_tts")
    with pytest.raises(VoiceError, match="unknown framework"):
        validate_voice(_voice())
    with pytest.raises(VoiceError, match="cannot be unregistered"):
        unregister("kokoro_onnx")


def test_personaplex_plan_is_a_mock_and_does_not_load_weights():
    voice = load_voice(ROOT / "voices" / "personaplex" / "NATF2.json")
    plan = get_framework("personaplex").prepare(voice, None)
    assert plan.wired is False
    assert plan.executable is False
    request = mock_offline_request(plan)
    assert request["module"] == "moshi.offline"
    assert request["voice_prompt"] == "NATF2.pt"
    assert request["weights_loaded"] is False
    assert "teacher" in request["text_prompt"]


def test_kokoro_onnx_is_executable_only_when_files_and_the_package_exist(tmp_path: Path, monkeypatch):
    voice = load_voice(ROOT / "voices" / "kokoro_onnx" / "af_heart.json")
    monkeypatch.delenv("GEN_AUDIO_KOKORO_MODEL", raising=False)
    monkeypatch.delenv("GEN_AUDIO_KOKORO_VOICES", raising=False)
    bare = get_framework("kokoro_onnx").prepare(voice, None)
    assert bare.executable is False
    assert any("GEN_AUDIO_KOKORO_MODEL" in item for item in bare.missing)
    model = tmp_path / "kokoro-v1.0.onnx"
    voices = tmp_path / "voices-v1.0.bin"
    model.write_bytes(b"not-a-model")
    voices.write_bytes(b"not-a-pack")
    monkeypatch.setenv("GEN_AUDIO_KOKORO_MODEL", str(model))
    monkeypatch.setenv("GEN_AUDIO_KOKORO_VOICES", str(voices))
    monkeypatch.setattr(importlib.util, "find_spec", lambda name: object() if name == "kokoro_onnx" else None)
    ready = get_framework("kokoro_onnx").prepare(voice, None)
    assert ready.executable is True
    assert ready.resolved_files["model"].endswith("kokoro-v1.0.onnx")


def test_torch_kokoro_and_misaki_do_not_become_waveform_engines():
    voice = load_voice(ROOT / "voices" / "kokoro" / "af_heart.json")
    plan = get_framework("kokoro").prepare(voice, None)
    assert plan.engine_id == "kokoro_dayour"
    assert plan.executable is False
    assert any("not vendored" in item for item in plan.missing)
    g2p = load_g2p(ROOT / "voices" / "g2p" / "misaki.ja.json")
    phonemizer = get_framework("misaki").prepare(g2p, None)
    assert phonemizer.kind == "g2p"
    assert phonemizer.executable is False
    assert any("does not emit a waveform" in item for item in phonemizer.missing)


def test_library_root_uses_the_env_var_and_rejects_escape(tmp_path: Path, monkeypatch):
    monkeypatch.setenv("GENAID_VOICES_DIR", str(tmp_path))
    assert library_root() == tmp_path.resolve()
    catalog = load_catalog(CATALOG)
    missing = missing_assets(catalog, tmp_path)
    assert "en-Alice_woman.wav" in missing
    monkeypatch.delenv("GENAID_VOICES_DIR", raising=False)
    assert library_root() is None
    outside = tmp_path / "outside"
    outside.mkdir()
    nested = tmp_path / "lib"
    nested.mkdir()
    link = nested / "escape"
    link.symlink_to(outside, target_is_directory=True)
    from gen_audio.voices.catalog import resolve_asset

    with pytest.raises(VoiceError, match="escapes"):
        resolve_asset("escape/secret.wav", nested)


def test_cli_lists_validates_and_checks_the_cast(capsys):
    assert main(["frameworks"]) == 0
    text = capsys.readouterr().out
    assert "kokoro_onnx" in text and "wired" in text
    assert "personaplex" in text and "schema" in text
    assert main(["list", "--framework", "livevoice"]) == 0
    rows = json.loads(capsys.readouterr().out)
    assert {row["id"] for row in rows} == {"livevoice.en-Carter_man", "livevoice.de-Spk0_man"}
    assert main(["validate", str(CATALOG)]) == 0
    assert "18 voices" in capsys.readouterr().out
    assert main(["validate-cast", str(CAST), "--catalog", str(CATALOG)]) == 0
    assert "speakers=3" in capsys.readouterr().out
    assert main(["validate", str(CATALOG), "--require-assets"]) == 1
    assert "voice library is unset" in capsys.readouterr().err


def test_g2p_english_variant_must_follow_the_british_flag():
    payload = json.loads((ROOT / "voices" / "g2p" / "misaki.en-us.json").read_text(encoding="utf-8"))
    payload["british"] = True
    with pytest.raises(VoiceError, match="british"):
        load_g2p_from(payload)


def load_g2p_from(payload: dict):
    from gen_audio.voices.builtins import validate_g2p_document

    return validate_g2p_document(payload)
