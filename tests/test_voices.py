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
    load_g2p,
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


def test_example_catalog_is_the_genaid_voices_manifest():
    catalog = load_catalog(CATALOG)
    assert catalog.catalog_source == (
        "manifest:DarbotLM/genaid-voices@2f749142420cfeb4b4ee881ffa68f305ad79a96a"
    )
    assert catalog.library_env == "GENAID_VOICES_DIR"
    assert catalog.g2p == ()
    vibe = [voice for voice in catalog.voices if voice.framework == "vibevoice"]
    live = [voice for voice in catalog.voices if voice.framework == "livevoice"]
    assert len(vibe) == 9
    assert len(live) == 50
    assert {voice.framework for voice in catalog.voices} == {"vibevoice", "livevoice"}
    alice = catalog.voice("vibevoice.en-Alice_woman")
    assert alice.assets[0].path == "_hf_vibevoice_samples/voices/en-Alice_woman.wav"
    assert alice.assets[0].sha256 == "c27ae47421436287a6bd2c3062de2dc2a2855b78c0bb626d472202c359704203"
    assert alice.assets[0].size == "297 KB"
    assert alice.duplicates[0].path == "en-Alice_woman.wav"
    assert alice.duplicates[0].sha256 == alice.assets[0].sha256
    mary = catalog.voice("vibevoice.en-Mary_woman_bgm")
    assert mary.params["background_music"] is True
    assert mary.language == "en"
    samuel = catalog.voice("vibevoice.in-Samuel_man")
    assert samuel.language == "en-IN"
    assert samuel.file_prefix == "in"
    assert samuel.reference_audio["sample_rate"] == 24000
    assert catalog.voice("vibevoice.en-Alice_woman").reference_audio["sample_rate"] == 16000
    assert catalog.voice("vibevoice.en-Alice_woman").reference_audio["duration_s"] == 9.3
    assert catalog.voice("livevoice.in-Samuel_man").language == "en-IN"
    assert catalog.voice("livevoice.in-Samuel_man").params["experimental"] is False
    de0 = catalog.voice("livevoice.de-Spk0_man")
    assert de0.params["experimental"] is True
    assert "experimental_voices/" not in de0.assets[0].path
    de2 = catalog.voice("livevoice.de-Spk2_woman")
    assert de2.params["experimental"] is True
    assert de2.assets[0].path.endswith("experimental_voices/de/de-Spk2_woman.pt")
    assert sum(item.params["experimental"] for item in live) == 43
    kinds = {}
    for item in catalog.excluded:
        kinds[item.kind] = kinds.get(item.kind, 0) + 1
    assert kinds == {"example_output": 13, "text_example": 12, "rendered_sample": 1}
    assert "kokoro" not in {voice.framework for voice in catalog.voices}
    load_voice(ROOT / "voices" / "kokoro_onnx" / "af_heart.json")
    assert load_g2p(ROOT / "voices" / "g2p" / "misaki.en-us.json").british is False


def test_mixed_cast_keeps_alice_and_frank_and_resolves_other_frameworks():
    cast = load_cast_map(CAST)
    catalog = load_catalog(CATALOG)
    assert cast.engine == "kokoro_onnx"
    assert cast.speakers["1"].voice == "af_heart"
    assert cast.speakers["1"].framework == "kokoro_onnx"
    assert cast.speakers["2"].name == "Frank"
    assert cast.speakers["2"].voice == "am_michael"
    assert set(cast.speakers) == {"1", "2"}
    assert resolve_character(cast, "alice", "vibevoice") == "vibevoice.en-Alice_woman"
    assert resolve_character(cast, "frank", "livevoice") == "livevoice.en-Frank_man"
    assert resolve_character(cast, "carter", "livevoice") == "livevoice.en-Carter_man"
    validate_cast(cast, catalog)
    with pytest.raises(KeyError, match="known frameworks"):
        resolve_character(cast, "alice", "kokoro_onnx")


def test_kokoro_synth_refuses_a_vibevoice_speaker_in_the_mixed_cast():
    synth = KokoroOnnxSynthesizer.__new__(KokoroOnnxSynthesizer)
    cast = load_cast_map(CAST)
    turns = parse_script("Speaker 1: Hello from Alice.\n")
    cast.speakers["1"]  # the example speakers are kokoro_onnx and can be planned
    foreign = parse_script("Speaker 9: Hello.\n")
    from gen_audio.cast import CastMap, VoiceAssignment

    mixed = CastMap(
        engine="kokoro_onnx",
        default_lang="en-us",
        default_speed=1.0,
        speakers={"9": VoiceAssignment("9", "Bowen", "zh-Bowen_man", "zh", 1.0, framework="vibevoice")},
    )
    with pytest.raises(ValueError, match="vibevoice"):
        synth.synthesize_turns(foreign, mixed)
    assert turns[0].speaker_id == "1"


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
    assert request["runnable"] is False
    assert request["input_wav"] is None
    assert request["output_wav"] is None
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
    assert any("not bundled" in item for item in plan.missing)
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
    assert "_hf_vibevoice_samples/voices/en-Alice_woman.wav" in missing
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
    assert len(rows) == 50
    assert {"livevoice.en-Carter_man", "livevoice.de-Spk0_man"} <= {row["id"] for row in rows}
    assert main(["validate", str(CATALOG)]) == 0
    assert "59 voices, 0 g2p" in capsys.readouterr().out
    assert main(["validate-cast", str(CAST), "--catalog", str(CATALOG)]) == 0
    assert "speakers=2" in capsys.readouterr().out
    assert main(["validate", str(CATALOG), "--require-assets"]) == 1
    assert "voice library is unset" in capsys.readouterr().err


def test_g2p_english_variant_must_follow_the_british_flag():
    payload = json.loads((ROOT / "voices" / "g2p" / "misaki.en-us.json").read_text(encoding="utf-8"))
    payload["british"] = True
    with pytest.raises(VoiceError, match="british"):
        load_g2p_from(payload)


def test_invalid_fixtures_use_the_shared_error_codes(tmp_path: Path):
    folder = ROOT / "tests" / "fixtures" / "voices" / "invalid"
    cases = json.loads((folder / "cases.json").read_text(encoding="utf-8"))
    for case in cases["cases"]:
        payload = json.loads((folder / case["file"]).read_text(encoding="utf-8"))
        with pytest.raises(VoiceError) as caught:
            if case["kind"] == "voice":
                validate_voice(payload)
            elif case["kind"] == "g2p":
                from gen_audio.voices import validate_g2p

                validate_g2p(payload)
            else:
                path = tmp_path / case["file"]
                path.write_text(json.dumps(payload), encoding="utf-8")
                validate_cast(load_cast_map(path))
        assert caught.value.code == case["code"], case["file"]


def test_vibevoice_7b_is_gpu_only_and_1_5b_passes_the_sampler_arguments():
    from gen_audio.adapters import EngineRefusal, vibevoice_call_plan, vibevoice_profile

    plan = vibevoice_call_plan(vibevoice_profile("1.5B"), "Speaker 1: Hello.", ["wav"])
    assert plan["processor"]["voice_samples"] == [["wav"]]
    assert plan["processor"]["return_attention_mask"] is True
    assert plan["diffusion_steps"] == 20
    assert plan["generate"]["cfg_scale"] == 1.3
    assert plan["generate"]["tokenizer_from"] == "processor.tokenizer"
    with pytest.raises(EngineRefusal, match="GPU-only"):
        vibevoice_call_plan(vibevoice_profile("7B"), "Speaker 1: Hello.", ["wav"])


def test_custom_entries_land_in_lexicon_golds_and_blend_is_a_vector(tmp_path: Path):
    from gen_audio.synth_kokoro_onnx import KokoroOnnxSynthesizer
    from gen_audio.voices.runtime import apply_custom_entries

    class Lexicon:
        def __init__(self):
            self.golds = {}

    class G2P:
        def __init__(self):
            self.lexicon = Lexicon()

    g2p = G2P()
    apply_custom_entries(g2p, {"kokoro": "kOkero"})
    assert g2p.lexicon.golds["kokoro"] == "kOkero"
    pack = tmp_path / "voices-v1.0.bin"
    with pack.open("wb") as handle:
        np.savez(handle, af_heart=np.array([2.0, 0.0], dtype=np.float32), af_nicole=np.array([0.0, 2.0], dtype=np.float32))
    synth = KokoroOnnxSynthesizer.__new__(KokoroOnnxSynthesizer)
    synth._voices_path = pack
    mixed = synth.voice_for_create(
        {"blend": [{"voice_id": "af_heart", "weight": 0.5}, {"voice_id": "af_nicole", "weight": 0.5}]}
    )
    assert list(mixed) == pytest.approx([1.0, 1.0])


def test_misaki_phonemes_and_a_blend_reach_kokoro_onnx_create(tmp_path: Path, monkeypatch):
    from gen_audio.voices.runtime import split_phonemes

    g2p = load_g2p(ROOT / "voices" / "g2p" / "misaki.en-us.json")
    voice = load_voice(ROOT / "voices" / "kokoro_onnx" / "blend_heart_nicole.json")
    pack = tmp_path / "voices-v1.0.bin"
    with pack.open("wb") as handle:
        np.savez(
            handle,
            af_heart=np.array([2.0, 0.0], dtype=np.float32),
            af_nicole=np.array([0.0, 2.0], dtype=np.float32),
        )
    monkeypatch.setattr(
        "gen_audio.voices.runtime.phonemize_with_misaki",
        lambda text, config: "h" * 20 + " " + "e" * 500,
    )
    synth = KokoroOnnxSynthesizer.__new__(KokoroOnnxSynthesizer)
    synth._voices_path = pack
    calls = []

    class FakeKokoro:
        def create(self, spoken, **kwargs):
            calls.append((spoken, kwargs))
            return np.zeros(4, dtype=np.float32), 24000

    synth._kokoro = FakeKokoro()
    audio, rate = synth.synthesize_with_g2p("Hello", dict(voice.params), g2p, speed=1.0, lang="en-us")
    assert rate == 24000 and audio.size == 8
    assert len(calls) == 2
    assert all(item[1]["is_phonemes"] is True for item in calls)
    assert list(calls[0][1]["voice"]) == pytest.approx([1.0, 1.0])
    assert split_phonemes("abcd", limit=2) == ["ab", "cd"]


def test_kokoro_torch_assigns_misaki_and_mixes_the_blend(tmp_path: Path):
    from gen_audio.voices.runtime import run_kokoro_voice

    voice = load_voice(ROOT / "voices" / "kokoro" / "blend_heart_bella.json")
    g2p = load_g2p(ROOT / "voices" / "g2p" / "misaki.en-us.json")
    library = tmp_path / "lib"
    for name, values in (("af_heart", [1.0, 0.0]), ("af_bella", [0.0, 4.0])):
        path = library / "kokoro" / "voices" / f"{name}.pt"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"pt")
    seen = {}

    class Pipeline:
        def __init__(self, **kwargs):
            seen["kwargs"] = kwargs

        def __call__(self, text, voice, speed):
            seen["voice"] = voice
            seen["speed"] = speed
            seen["g2p"] = self.g2p
            yield ("Hello", "h", np.array([0.25, 0.5], dtype=np.float32))

    def torch_load(path: Path):
        if path.name.startswith("af_heart"):
            return np.array([1.0, 0.0], dtype=np.float32)
        return np.array([0.0, 4.0], dtype=np.float32)

    class Marker:
        pass

    marker = Marker()
    monkey_g2p = marker

    def build(config):
        assert config.id == "misaki.en-us"
        return monkey_g2p

    import gen_audio.voices.runtime as runtime

    original = runtime.build_misaki
    runtime.build_misaki = build
    try:
        audio, rate = run_kokoro_voice(
            "Hello",
            voice,
            g2p,
            library,
            pipeline_cls=Pipeline,
            torch_load=torch_load,
        )
    finally:
        runtime.build_misaki = original
    assert rate == 24000
    assert audio.tolist() == pytest.approx([0.25, 0.5])
    assert seen["kwargs"]["repo_id"] == "hexgrad/Kokoro-82M"
    assert seen["kwargs"]["lang_code"] == "a"
    assert seen["kwargs"]["device"] == "cpu"
    assert seen["g2p"] is marker
    assert seen["speed"] == 1.0
    assert seen["voice"].tolist() == pytest.approx([0.75, 1.0])


def test_packaged_voice_data_matches_the_checkout():
    bundled = ROOT / "src" / "gen_audio" / "data"
    for path in (ROOT / "schemas" / "voices").rglob("*.json"):
        relative = path.relative_to(ROOT / "schemas" / "voices")
        assert (bundled / "schemas" / relative).read_bytes() == path.read_bytes(), relative
    assert (bundled / "catalog.example.json").read_bytes() == (ROOT / "voices" / "catalog.example.json").read_bytes()
    for path in (ROOT / "voices" / "g2p").glob("*.json"):
        assert (bundled / "g2p" / path.name).read_bytes() == path.read_bytes()


def test_rebuild_check_fails_when_the_library_does_not_match(tmp_path: Path, capsys):
    wav = tmp_path / "_hf_vibevoice_samples" / "voices"
    wav.mkdir(parents=True)
    (wav / "en-Alice_woman.wav").write_bytes(b"wav")
    (tmp_path / "en-Alice_woman.wav").write_bytes(b"wav")
    assert main(["scan", "--library", str(tmp_path)]) == 0
    counts = json.loads(capsys.readouterr().out)
    assert counts["vibevoice"] == 1
    assert counts["duplicate_wav"] == 1
    assert main(["rebuild", "--library", str(tmp_path), "--catalog", str(CATALOG), "--check"]) == 1
    assert "does not match" in capsys.readouterr().err


def load_g2p_from(payload: dict):
    from gen_audio.voices.builtins import validate_g2p_document

    return validate_g2p_document(payload)
