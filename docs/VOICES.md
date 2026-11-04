# Voices

A voice is a named asset plus the parameters of one speech framework. A persona (`voice_profile.schema.json`) is a different document: it names an agent such as Alice and may point at a voice. This page is the voice library.

The compare-list in `gen_audio.engines` says which synthesizers this package can score. The voice registry says which frameworks can describe a voice. Those lists overlap on purpose and they are not the same list.

| Piece | Where |
| --- | --- |
| JSON Schema 2020-12, `schema_version` `1.0.0` | `schemas/voices/` |
| Shipped framework rows | `schemas/voices/registry.v1.json` |
| Example voices, G2P configs, catalog, cast | `voices/` |
| Python loader, resolver, adapters | `src/gen_audio/voices/` |
| CLI | `gen-audio-voices` or `python scripts/voices.py` |
| Rust types and the same cross-field checks | `crates/gen-audio-core/src/voices.rs` |

Nothing in this layer downloads weights or writes audio. Kokoro `.pt` packs, ONNX files, VibeVoice checkpoints, and PersonaPlex weights stay outside the git repo.

## Library root

Asset paths on a voice are relative to a directory you choose:

1. `--library` on the CLI
2. `GENAID_VOICES_DIR`
3. `library.path` on a catalog, when that field is set

The example catalog leaves `library.path` empty. `gen-audio-voices validate` checks the documents without that directory. `--require-assets` fails until the files are actually there.

`voices/catalog.example.json` is a manifest scan of [DarbotLM/genaid-voices](https://github.com/DarbotLM/genaid-voices) at `2f749142420cfeb4b4ee881ffa68f305ad79a96a` (`catalog_source` `manifest:DarbotLM/genaid-voices@2f749142420cfeb4b4ee881ffa68f305ad79a96a`). The cloud agent could not read that private repo. The catalog was built from a verified local listing: `git ls-tree -r -l HEAD` plus `git lfs ls-files -l -s` (sha256 oid and the human size `git lfs` prints). Exact byte counts were not in that listing, so `size` is the LFS display size (`297 KB`, `7.0 MB`) and text files use the `ls-tree` byte count in `bytes`.

That commit has 82 LFS files:

| Files | Count | In the catalog |
| --- | --- | --- |
| `_hf_vibevoice_samples/voices/*.wav` | 9 | vibevoice voices. Asset path is this copy |
| root `*.wav` | 9 | not separate voices. Same oid as the `voices/` copy, recorded on `duplicates` |
| `streaming_model/**/*.pt` | 50 | livevoice. 25 beside `streaming_model/`, 25 under `experimental_voices/<lang>/` |
| `example_output/*.wav` | 13 | `excluded`, kind `example_output` |
| `realtime_model/vibevoice_tts_german.wav` | 1 | `excluded`, kind `rendered_sample` |

Twelve `text_examples/*.txt` files are `excluded` with kind `text_example`. README, `.gitattributes`, and `.gitignore` are repo metadata, not voices. There are no Kokoro packs in the repo. Kokoro and PersonaPlex descriptors under `voices/kokoro/`, `voices/kokoro_onnx/`, and `voices/personaplex/` stay as illustrative schema examples and are not catalog entries.

Reference-wav languages in this commit are `en`, `zh`, and `in` (`in-Samuel_man.wav`). Streaming preset codes are `de`, `en`, `fr`, `in`, `it`, `jp`, `kr`, `nl`, `pl`, `pt`, and `sp`. `jp`, `sp`, `kr`, and `in` are the file codes (`jp` rather than `ja`, `sp` rather than `es`, `kr` rather than `ko`). `in` is not rewritten to `hi`. `_bgm` is a real suffix on `en-Mary_woman_bgm` and `zh-Anchen_man_bgm`. `experimental` follows the `experimental_voices/` directory. Top-level `de-Spk0_man.pt`, `jp-Spk0_man.pt`, `sp-Spk0_woman.pt`, `kr-Spk0_woman.pt`, and `in-Samuel_man.pt` are not experimental. There is no `experimental_voices` directory for `en`, `in`, `it`, or `nl`.

## Common voice descriptor

`schemas/voices/voice.v1.schema.json`

| Field | Meaning |
| --- | --- |
| `id` | `framework.local-id`, for example `kokoro_onnx.af_heart` or `vibevoice.en-Alice_woman` |
| `display_name` | Short name for a UI |
| `language` | Language tag. Livevoice uses the published preset code when that is not BCP-47 (`jp`, `sp`, `kr`) |
| `gender` | `female`, `male`, `neutral`, or `unspecified` (a mixed-gender blend) |
| `framework` | Registry id. The schema allows any slug; the loader rejects ids that are not registered |
| `assets` | `{role, path, format, member?}`. `path` is relative. `..`, absolute paths, and backslashes are rejected |
| `source` | `name`, `https` URL, license, optional attribution |
| `tags` | Short lowercase tags |
| `g2p` | Misaki config id. Required for Kokoro, omitted for the other frameworks |
| `params` | Framework object. Checked against that framework's schema |

## Frameworks

`wired` in the registry means this package can hand the plan to a synthesizer that already runs here. The other rows validate, resolve paths, and build a plan. Their `prepare` result has `executable: false` and names what is missing.

### Kokoro (PyTorch) — `kokoro`

Schema: `schemas/voices/frameworks/kokoro.v1.schema.json`. Engine id: `kokoro_dayour` (the dayour/kokoro fork of hexgrad/kokoro). Not wired: this package does not import torch and does not open `.pt` files.

A pack id is `<lang><gender>_<name>`:

| Prefix | Language | `lang_code` |
| --- | --- | --- |
| `af_` / `am_` | `en-us` | `a` |
| `bf_` / `bm_` | `en-gb` | `b` |
| `ef_` / `em_` | `es` | `e` |
| `ff_` / `fm_` | `fr-fr` | `f` |
| `hf_` / `hm_` | `hi` | `h` |
| `if_` / `im_` | `it` | `i` |
| `jf_` / `jm_` | `ja` | `j` |
| `pf_` / `pm_` | `pt-br` | `p` |
| `zf_` / `zm_` | `zh` | `z` |

`f` is female and `m` is male. `speed` is 0.5–2.0. A blend lists 2–8 pack ids whose weights are greater than 0 and sum to 1. The adapter mixes equal-shaped arrays with `blend_vectors`; the caller has to load the tensors. Each pack is a `voicepack` asset in `.pt` form, `member` set to the pack id.

### kokoro-onnx — `kokoro_onnx`

Schema: `schemas/voices/frameworks/kokoro_onnx.v1.schema.json`. Wired.

The v1 pair is `kokoro-v1.0.onnx` and `voices-v1.0.bin`. The `.bin` file is a NumPy npz of named float32 style vectors. `blend_from_pack` loads that file and applies the same weighted sum. `Kokoro.create` can take the resulting array as `voice` instead of a pack id. The existing multi-turn synthesizer still speaks the single pack id on the cast speaker (`af_heart`, `am_michael`) when `GEN_AUDIO_KOKORO_MODEL` and `GEN_AUDIO_KOKORO_VOICES` point at real files.

`prepare` reports `executable: true` only when those files exist and the `kokoro-onnx` package is installed. It does not synthesize by itself.

Later language packs use a versioned bin name such as `voices-v1.1-zh.bin`. The `voices_file` pattern allows that. The example English voices point at `voices-v1.0.bin`.

### Misaki — `misaki`

Schema for a voice's params is empty: Misaki is not a voice. The document you author is `schemas/voices/g2p.v1.schema.json`.

| `backend` | What it selects |
| --- | --- |
| `misaki-en` | English G2P. `variant` is `en-us` or `en-gb`. `british` follows that variant. `transformer` is the spaCy transformer switch. `espeak_fallback` turns on espeak for out-of-lexicon words, and `espeak_language` must match the variant |
| `misaki-ja` | Japanese Misaki. `language` is `ja` |
| `misaki-zh` | Chinese Misaki. `language` is `zh` |
| `espeak` | Languages Misaki does not cover itself. `espeak_language` is required |

`custom_entries` is a small overlay (the English example records the usual `kokoro` pronunciation). `lexicon` is an optional file relative to the library. This package does not import Misaki and does not phonemize text. `prepare` on a G2P config returns a plan with `executable: false`.

### VibeVoice — `vibevoice`

Schema: `schemas/voices/frameworks/vibevoice.v1.schema.json`. Not wired to the gated experimental synthesizer in `gen_audio.adapters`. That function still refuses until a measured run, and it does not read this schema.

Voices are reference WAVs:

```text
<lang>-<Name>_<gender>[_bgm].wav
en-Alice_woman.wav
zh-Bowen_man.wav
en-Mary_woman_bgm.wav
in-Samuel_man.wav
```

`gender` is `woman`, `man`, or `neutral`. `_bgm` sets `background_music`. The scanned library uses `en`, `zh`, and `in`, so a voice is not rejected for being outside English and Chinese. The published 7B card still says the model was trained on English and Chinese; `in-Samuel_man` is in the library anyway. At most 4 speakers, in the `Speaker N:` script this repo already parses. A reference WAV is not pinned to the 1.5B or 7B checkpoint, so scanned voices omit `model_size`. `cfg_scale` and `diffusion_steps` are inference knobs and are omitted unless a document sets them.

| `model_size` | `model_id` |
| --- | --- |
| `1.5B` | `microsoft/VibeVoice-1.5B` |
| `7B` | `microsoft/VibeVoice-Large` |

The 7B checkpoint config attached to the model card (`config.json`, `preprocessor_config.json`) is that Large model: `model_type` `vibevoice`, Qwen2.5-7B (`hidden_size` 3584), acoustic VAE dim 64, semantic VAE dim 128, DDPM cosine schedule, `ddpm_num_inference_steps` 20, `ddpm_num_steps` 1000, 24 kHz, speech tokenizer compression 3200. `diffusion_steps` in the voice defaults to that 20. `cfg_scale` is an inference knob and is not a field of the checkpoint config. The example uses 1.3. The schema allows 0.5–5.

`sample_rate` is 24000. `max_speakers` is 4. A cast with five VibeVoice speakers fails `validate_cast` even though the generic cast map allows eight speakers.

### Livevoice — `livevoice`

Schema: `schemas/voices/frameworks/livevoice.v1.schema.json`. Not wired. There is no compare-list engine id; `engine_id` is null.

This is VibeVoice-Realtime 0.5B (`microsoft/VibeVoice-Realtime-0.5B`): one speaker, streaming text, a `.pt` preset that is a prefilled KV-cache rather than a reference WAV. The published realtime card is English-first and quotes about 200 ms to first audio. Presets look like:

```text
streaming_model/en-Carter_man.pt
streaming_model/de-Spk0_man.pt
streaming_model/experimental_voices/de/de-Spk2_woman.pt
streaming_model/in-Samuel_man.pt
streaming_model/jp-Spk0_man.pt
streaming_model/sp-Spk0_woman.pt
```

`experimental` is true only for files under `experimental_voices/<lang>/`. The 25 top-level presets are not experimental, including German, Japanese, Spanish, Korean, and `in`. Preset codes in this commit are `de`, `en`, `fr`, `in`, `it`, `jp`, `kr`, `nl`, `pl`, `pt`, and `sp`. A second livevoice speaker in one cast is rejected.

`preset_kind` is `kv_cache`. `single_speaker` is true. `sample_rate` is 24000.

The name livevoice here is this streaming family. [LiveVoice](https://livevoice.io) is a separate remote-interpreting product. It is not a speech model and nothing here calls it.

### PersonaPlex — `personaplex`

Schema: `schemas/voices/frameworks/personaplex.v1.schema.json`. Not wired. Weights were not available when this schema was written.

Public model card and README for `nvidia/personaplex-7b-v1` (Hugging Face, gated, released 2026-01-15; code at NVIDIA/personaplex):

* Full-duplex speech-to-speech, Moshi architecture, about 7B parameters, 24 kHz.
* Before the conversation the model takes two prompts: a voice prompt and a text prompt.
* Voice prompts are stock embeddings. Natural: `NATF0`–`NATF3`, `NATM0`–`NATM3`. Variety: `VARF0`–`VARF4`, `VARM0`–`VARM4`. The file is the code plus `.pt` (`NATF2.pt`).
* The text prompt sets role and persona. The project's prompting guide uses an assistant role ("You are a wise and friendly teacher..."), customer-service roles, and casual conversation roles. A community thread on the model card reports that the text prompt does not reliably carry arbitrary facts; it shapes persona. `text_prompt_role` records which of those three families a prompt belongs to.
* The offline entry point documented publicly is `python -m moshi.offline` with `--voice-prompt`, `--text-prompt`, `--input-wav`, `--output-wav`, optional `--seed` and `--cpu-offload`.

`gated` is always true. `mock_offline_request` returns those arguments and `weights_loaded: false`. One PersonaPlex speaker per cast. `pocket_tts` remains the compare-list CPU stand-in from before this schema; it is not a PersonaPlex voice.

## Cast map

`schemas/voices/cast_map.v1.schema.json`

`voices/cast_map.example.json` is schema 1.0.0 and still has what the sample script and the harness read:

* `engine` is `kokoro_onnx`
* speaker `1` is Alice, voice `af_heart`
* speaker `2` is Frank, voice `am_michael`

Speaker `3` is Bowen on `vibevoice.zh-Bowen_man`. `characters` maps Alice and Carter onto scanned VibeVoice references, and Frank and Carter onto the matching livevoice presets (`livevoice.en-Frank_man`, `livevoice.en-Carter_man`). Kokoro pack ids stay on speakers 1 and 2 only, because those packs are not in the scanned catalog. The kokoro-onnx synthesizer refuses a turn whose speaker framework is not `kokoro_onnx`.

A map with no `schema_version` is the older shape (`engine`, `speakers`, `name`, `voice`, `lang`, `speed`). Keys that start with `_` are still ignored there. Kokoro frameworks keep speed 0.5–2.0. Any other framework accepts a finite speed greater than 0 and at most 4, because generated casts always wrote `speed`. That number is not a VibeVoice or PersonaPlex parameter.

`gen-audio-voices validate-cast voices/cast_map.example.json --catalog voices/catalog.example.json` checks the links.

## CLI

```bash
python scripts/voices.py frameworks
python scripts/voices.py list --framework vibevoice
python scripts/voices.py validate voices/catalog.example.json
python scripts/voices.py validate voices/catalog.example.json --require-assets --library "$GENAID_VOICES_DIR"
python scripts/voices.py validate-cast voices/cast_map.example.json --catalog voices/catalog.example.json
```

`frameworks` prints id, kind, engine id, `wired` or `schema`, and the registry summary. `list` prints JSON. Validate commands print one `ok` line or `error:` on stderr.

## Adding a framework

1. Add `schemas/voices/frameworks/<id>.v1.schema.json` for the `params` object.
2. Add a row to `schemas/voices/registry.v1.json` (`id`, `kind`, `engine_id`, `params_schema`, `wired`, `summary`). `wired` stays false until a synthesizer in this repo actually runs it.
3. Register a `FrameworkSpec` from Python: `validate_params(params, *, language, gender, assets)` and `prepare(voice, library)`. Built-ins live in `src/gen_audio/voices/builtins.py` and must match the registry row, including the summary text. A process-local experiment can `register` without a row and `unregister` afterwards; built-ins cannot be removed.
4. Add a match arm in `crates/gen-audio-core/src/voices.rs` (`validate_params`) so the Rust checker knows the new id. The registry test lists the shipped ids in file order.
5. Add an example under `voices/<id>/`, point `voices/catalog.example.json` at it, and extend `tests/test_voices.py`. The Rust test `voices_v1` checks every catalog entry against JSON Schema and `validate_voice_json`.

Set `speaker_limit` when the model cannot take as many speakers as the cast map (8). VibeVoice is 4, livevoice and PersonaPlex are 1.
