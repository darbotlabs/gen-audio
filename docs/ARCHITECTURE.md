# Architecture

gen-audio is a small Python 3.14 toolkit (`requires-python` is `>=3.14,<3.15`). The installable import name is `gen_audio`. It turns a labeled script into a mono WAV when kokoro-onnx and local model files are available, and it can process a WAV you already have without any model.

The same floor applies to the spectrogram, genaid-audio URL helpers, podcast script parsing, and base SDK. Isolated model workers use their own pinned Python 3.12 environment; they are not dependencies of the Python 3.14 SDK. genlm-audio and Gradio are not vendored here. Request limits, voice and model path confinement, checksum pins, and `/health` versus `/ready` live in `gen_audio.guards` and are described in `docs/genlm-audio/REVIEW-2026-10-06.md`.

[darbotlabs/genaid](https://github.com/darbotlabs/genaid) is a different project (a JavaScript prompting framework). Nothing in that repository is imported here.

## Engines

`gen_audio.engines` is a registry, not a loader.

| Id | Kind | In this package |
| --- | --- | --- |
| `kokoro_onnx` | waveform synth | `KokoroOnnxSynthesizer` calls `kokoro_onnx.Kokoro.create` per turn. You pass the ONNX file and the voices file. |
| `kokoro_dayour` | waveform synth | Compare-list name for the dayour Kokoro runtime. No code path calls it. |
| `misaki` | grapheme-to-phoneme | Kokoro's G2P library (hexgrad/misaki). Not a waveform synth. kokoro-onnx phonemizes inside its own dependency tree. This package does not import misaki. |
| `vibevoice` | waveform synth | Desktop/MCP dispatch through a pinned, isolated Python 3.12 CPU worker. Preflight requires local weights, tokenizer, dependencies, and sufficient RAM. Not a base SDK synthesizer. |
| `magpie` | waveform synth | Compare-list slot for Magpie TTS. No adapter. |
| `pocket_tts` | waveform synth | Compare-list slot for Kyutai Pocket TTS, named as the CPU stand-in when a Magpie render is unavailable. No adapter. |

`gen_audio.compare.score_wav` reads a finished WAV and records inv-HDR, BW95, and clip fraction. An unregistered engine id is allowed and marked `"registered": false`. The score file does not sort engines or assign a quality rating.

Synthesis is off unless you construct `KokoroOnnxSynthesizer`. Importing `gen_audio` does not import kokoro-onnx.

### Desktop VibeVoice and unified voices

The desktop Generate action calls authenticated MCP `studio_preflight`, then `synth`. Rust normalizes scripts to contiguous speaker numbers, confines managed reference WAVs to issued scratch, and checks their canonical SHA-256 and mono 24 kHz PCM16 format. The supervised CPU worker checks pinned dependencies, local model/tokenizer metadata, and memory before loading. A verified bounded partial artifact remains explicitly incomplete; it is never reported as a complete render.

`voices/manifest.json` is the canonical version 2.0 library: nine personas, nine managed references, and seven engine manifests per persona. Kokoro ONNX/dayour, Misaki, VibeVoice, PersonaPlex, Magpie, and Pocket TTS retain separate capabilities and mapped/unmapped states. Misaki is G2P; PersonaPlex is speech-to-speech. A profile is not proof that every engine has an installed synthesizer. Profile flipcards expose and download the same complete manifest, validated by JSON Schema, TypeScript, and Rust.

Verified generated WAVs and manifests are saved under `%LOCALAPPDATA%/Gen-Audio/library/{asset-digest}` with an `audio_clip` envelope. Reopening verifies media hashes and facts before issuing fresh scratch artifacts. Local installed-runtime settings are read from `%LOCALAPPDATA%/Gen-Audio/runtime.json` (or `GEN_AUDIO_CONFIG`), with explicit environment variables taking precedence. `scripts/connect-desktop-mcp.py` connects stdio clients to the installed app through a transient owner-only native connection export; bearer tokens are never printed.

See [the migration record](MIGRATION-2026-10-08.md) for runtime requirements and remaining features. Stored prior VibeVoice media does not establish a successful render on the current machine.

### kokoro-onnx path

1. Parse the script (`gen_audio.cast.parse_script`).
2. Load the cast map. `engine` must be `kokoro_onnx`.
3. Resolve each turn's speaker id to `voice`, `lang`, and `speed`.
4. Call `Kokoro.create(text, voice=..., speed=..., lang=...)` once per turn.
5. Concatenate mono float audio with `gap_s` seconds of silence between turns (default 0.35). If a later turn reports a different sample rate, that turn is resampled to the first turn's rate before the join.
6. Optionally run the publish chain (`improve_publish=True` or `--improve`).

The synth CLI writes a float32 WAV when `--improve` is off, because PCM_16 clips anything outside [-1, 1] and the publish chain has not run yet. With `--improve`, the file is PCM_16 at 24 kHz after the peak has been brought to about 0.89.

kokoro-onnx v1 reports 24 kHz. The code keeps whatever rate `create` returns, and the publish chain is still what forces the 24 kHz deliverable. That matters for WAVs that did not come from Kokoro.

Paths:

* `--model` or `GEN_AUDIO_KOKORO_MODEL`
* `--voices` or `GEN_AUDIO_KOKORO_VOICES`

There is no download step. A missing file raises `FileNotFoundError` before the optional package is imported. A missing install raises `ImportError` telling you to `pip install 'gen-audio[kokoro]'`.

The example cast map uses `af_heart` and `am_michael`. Those are Kokoro v1 voice ids. kokoro-onnx raises if the loaded voice pack does not contain the id.

Speeds are checked against 0.5–2.0 at cast-map load, which is the range kokoro-onnx asserts.

## 24 kHz publish path

Podcast delivery in this toolkit is mono, 24 kHz. `improve()` is the only function that defines that deliverable. Cube revision does not resample and does not replace this chain.

`PUBLISH_SAMPLE_RATE` is 24000. Resampling uses `scipy.signal.resample_poly` with the integer up/down ratio of the two rates.

Float arrays inside the library are amplitude units, about [-1, 1]. `read_wav` asks soundfile for float32. Integer arrays passed straight into `as_mono` are divided by the integer full-scale value.

`write_wav` defaults to PCM_16. A 0.89 peak can move by about one 16-bit step on the way back in. Pass `subtype="FLOAT"` if you need the float32 samples preserved.

## Improve pipeline order

The order is intentional. Do not reorder the stages in callers by running them ad hoc and calling the result "the publish chain".

1. **Trim leading and trailing silence.** Threshold defaults to −40 dB relative to that file's peak, so a quiet take is not judged against full scale. 30 ms is put back on each cut when those samples exist, so a soft consonant on the edge is less likely to be clipped. Samples between the first and last loud sample stay, including pauses.
2. **High-pass at 70 Hz.** Butterworth order 2, zero-phase (`sosfiltfilt`, so the magnitude response is applied twice). This runs before peak normalization so a low-frequency thump does not decide the gain. Speech at a normal fundamental is above this cutoff; the stage is a rumble trim, not a telephone filter.
3. **Peak normalize to 0.89 linear.** That is about −1.0 dBFS (`20 log10(0.89)`). Silence (peak under `1e-12`) is left alone, but the trim stage already refuses an all-silent file.
4. **Resample to 24 kHz.** Last, so every engine lands on the same grid. The resampler can move the peak a little. The 0.89 target applies to the pre-resample signal.

An empty file, a non-finite sample, or a file that is entirely under the trim threshold raises `ValueError` instead of writing a substitute tone.

## Spectrogram panels

`spectrogram.py` writes PNGs with matplotlib's Agg canvas (no window is opened).

* `write_spectrogram` — one magnitude spectrogram.
* `write_before_after_panel` — before on the top row, after on the bottom.

NFFT is 1024 when the buffer is long enough, otherwise 256. Fewer than 256 samples raises. The plot is a visual check of the render and of the publish chain. It is not a MOS and it does not identify which engine produced the file.

Use the raw concatenation as "before" and the `improve` output as "after" when you want to see the publish chain. `--improve` on the synthesizer collapses that pair into one file; leave it off if you want both.

## Cube revision sketch

`cube_revision.py` measures three numbers and may apply one signal op at a time.

| Name | Definition in this repo |
| --- | --- |
| inv-HDR | `rms / peak`, the inverse of the linear crest factor. `hdr_db` is `20 log10(peak / rms)` when both are positive. |
| BW95 | Hertz between the 2.5 and 97.5 percentiles of Hann-weighted rFFT power (the central 95% of spectral power). |
| clip fraction | Fraction of samples with absolute value ≥ 0.99. |

Candidate ops, in priority order, using the default thresholds:

| Condition | Op | What it actually does |
| --- | --- | --- |
| clip fraction > 1e-4 | `reduce_clip` | Scales the buffer so the peak is 0.89, then clips to that range. It does not rebuild flattened peaks. |
| inv-HDR > 0.45 | `expand_crest` | Multiplies samples below 0.35×peak by 0.55. A buffer that sits entirely on the peak does not change. |
| inv-HDR < 0.08 | `lift_body` | Applies a 0.75 power curve to the normalized magnitude and restores the original peak. Zeros and the peak sample stay put. |
| BW95 < 2500 Hz | `widen_band` | Mixes in a 1.8 kHz high-passed copy (mix 0.35) and restores the original peak. A pure tone has no energy to reveal, so its spectrum does not grow. |

After an op changes the samples, the three measurements are taken again. An op that does not change the buffer is skipped. The loop stops at 4 successful changes, or sooner when nothing left in the candidate list has an effect.

The 3D plot places a point at `(inv_hdr, bw95/nyquist, clip_frac)` before and after. That is the "cube": three axes in about the unit range, not a graphics scene and not an HDR image format.

These thresholds are knobs on `CubeThresholds`. They are not a broadcast specification.

Run the publish chain when you need the 24 kHz deliverable. Run cube revision only when you want this measurement-driven pass. They are not stacked inside one function.

## Durable artifacts

Scratch directories (`/tmp`, Ray worker local disks, `tempfile.TemporaryDirectory`) go away when the process or the node does. The WAV, the spectrogram PNGs, and the JSON manifest are the artifacts worth keeping.

* Pass `--artifact-dir`, or set `GEN_AUDIO_ARTIFACT_DIR`.
* `publish_copy` copies a finished file into that directory.
* If neither is set, the CLI leaves the file at the `-o` path you named. Pick that path on durable storage yourself.

Do not point the artifact directory at a folder you then commit. Generated audio and model weights are gitignored.

## Module map

| Module | Role |
| --- | --- |
| `cast.py` | Script parser and cast map |
| `synth_kokoro_onnx.py` | kokoro-onnx multi-turn render |
| `improve.py` | Publish chain and its stage functions |
| `spectrogram.py` | PNG helpers |
| `cube_revision.py` | Metrics, serial ops, cube scatter |
| `compare.py` | Score existing files |
| `engines.py` | Compare-list text |
| `serve.py` | Per-node URL builder and health JSON |
| `audio_io.py` | WAV read/write, mono float |
| `artifacts.py` | Copy out of scratch |
| `cli/` | argparse entry points used by `scripts/` |

`scripts/*.py` insert `src/` on `sys.path` so a checkout runs without an editable install, as long as the dependencies are present. Console scripts (`gen-audio-synth`, and the rest) come from `pip install`.

## Desktop shell

`apps/desktop` is a Vite + TypeScript viewport. `apps/desktop/src-tauri` is the Tauri 2 shell (Windows WebView2, macOS WebKit, Linux webkit2gtk 4.1). IPC commands include `connector_statuses`, `viewport_example`, `mcp_status`, and `run_fixture_improve`. The last writes a fixture tone and calls `scripts/improve.py`; it does not accept a shell string. `mcp_status` supplies the owned listener's address and transient bearer credential to the frontend, not to public logs.

The browser checker in `apps/desktop/src/validate.ts` enforces the same honesty rules as `gen_audio_core::cards`: fixture visuals, `sampleScript: true`, unprobed versus probed serve rows, connector ids, and benchmark notes that say the figures were not remeasured. A rejected document replaces the board with that error. An empty card list keeps the empty-state sentence. Cards are validated again in `gen_audio_core::cards` before a document is treated as renderable. `SpectrogramPanel` and `Cube3D` must set `source` to `fixture-tone` and `notPodcast` to true. `BenchmarkCompare.measuredHere` must be false. Figures in the example board are copied from the 2026-10-06 compare notes (`gen_audio_core::benchmark`) and are not recomputed here. Inverse-HDR is not a publish ranking: those notes preferred the wider VibeVoice final even when Kokoro's inv-HDR rose more.

The in-window spectrogram is a browser DFT of the side-pane voice profile: persona ids (up to 8), the Voice TTS model, duration, and the engine dropped into the load slot. Connector ids are not personas. The caption says that map is not the Python `specgram` and not a podcast. The pipeline cube card stays a WebGL sketch of the sine fixture. The spatial slide draws `points_preview` from a library cube JSON (signal, tonality, confidence, quality) when a clip has one. The Python chain remains the publish path.

## Connectors

| Surface | Crate / binary | Session | Live behavior |
| --- | --- | --- | --- |
| MCP | `gen-audio-mcp` | None. `initialize` stores nothing. HTTP sets `X-Gen-Audio-Stateless` and never `Mcp-Session-Id`. `GET /ready` is listener readiness (`speech: false`). `GET /control/stream` is a short SSE snapshot of the process-local command ring, not a client session. | Tools call the Python CLIs, write a fixture tone, or queue a UI command. Flip, harvest, and progress events are on that ring. Harvest reads sidecar counts only. `cube_layers` omits point clouds and absolute paths. UI tools do not invent speech. Synth progress is `running`, `refused`, or `unavailable` until a result sets `synthesizedSpeech` true. |
| ACP | `gen-audio-acp` | In-memory `sessionId`, required by ACP, dropped on `session/cancel`. | `health` / `status` return connector health. Other prompts call MCP voice-profile and UI tools and emit `tool_call` updates. It does not call vendor APIs and it does not invent speech. |
| Harness | `gen-audio-harness` | None. JSONL trace on stdout. | Skips synth. Optional fixture and Python improve. |
| Copilot | `gen-audio-connectors` | None | GitHub Models `POST /inference/chat/completions`, or `COPILOT_STUDIO_ENDPOINT` if you set one. Not the in-IDE Copilot SDK. |
| Claude | same | None | Anthropic Messages API. `claude -p` only when `GEN_AUDIO_CLAUDE_CODE_CLI=1`. |
| GPT | same | None | OpenAI-compatible chat completions. |
| Gemini | same | None | `generateContent`. The key is a header, not a query parameter. |

Vendor calls also require `GEN_AUDIO_CONNECTOR_LIVE=1`. Without it, a present token is reported as `token_present` and `complete` stays a mock envelope. Redirects are disabled so a `Location` header cannot carry `Authorization` to another host. Model ids reject `..` and `/`. Tool paths from MCP reject absolute paths, drive-letter paths (`C:\...`), UNC paths, and `..`. User-supplied repo reads are only `examples/` and `voices/`. Python scripts are an exact trusted list. Writes are a single file name inside the process scratch, and reads only succeed for files this process created. Model files for synth must live under `GEN_AUDIO_MODEL_DIR`. Python child processes do not inherit vendor API keys. Vendor URLs must be `https` to a public host, or `http` on `127.0.0.1` / `localhost` exactly (`http://127.0.0.1.evil.com` is rejected). `serve_health` with `probe=true` only contacts loopback, `10.1.8.70`, or `GEN_AUDIO_PROBE_HOSTS`.

## Work directory

`GEN_AUDIO_WORK_DIR`, or a dedicated generated work directory managed by `Scratch`. It must not be a sensitive root or an arbitrary repository subdirectory. Loopback MCP shares issued artifacts across calls, so synthesis can feed analysis and mastering. Private scripts/cast files are not public file capabilities. Do not commit scratch. All HTTP data routes require the process bearer token and validate host/origin; direct non-loopback binding is disabled. Scratch is explicit data flow, not an MCP session ID.

## Original prototype migration

The read-only `T:\Gen.AID.Audio` prototype is a behavioral reference, not a runtime dependency. Its `genaid-audio.py` Gradio UI and `api_server.py` FastAPI service are not copied into the desktop. Model inference remains behind isolated workers; Rust owns input contracts, orchestration and artifact validation.

| Prototype behavior | Native migration | Deliberate correction |
| --- | --- | --- |
| Numbered speakers and rotating unlabeled lines | `authoring` analyzes/imports dialogue with an explicit indexing convention | No guessing between zero- and one-based labels; diagnostics never become spoken text |
| Phi prompt construction and script generation | Local draft planning plus reviewed, opt-in configured connector generation | Duration budget counts one conversation, not duration multiplied by speakers; local Phi model loading is not claimed |
| Raw preview and final mastered WAV | Separate original and mastered audio controls and downloads | Source stays unchanged; processing never asserts new speech synthesis |
| FFmpeg podcast mastering | Bounded native process pipeline targeting -16 LUFS, -1.5 dBTP and LRA 4 | Measures actual results, rejects failed analysis and publishes a provenance report |
| Managed voices and readiness | Existing profile-v2 voice inventory and isolated-worker preflight | A voice mapping or checkpoint on disk is not a loaded/validated model |

`draft_plan` prepares the exact provider prompt locally. `draft_generate` requires explicit confirmation tied to that prompt's digest; the desktop displays the recipient and text first. Generated dialogue is validated separately and applied only after review. Mock connector output is never presented as a draft. No legacy script, deployment hostname, credential file or caller-selected executable participates.

Deferred migration chunks are model-native PCM streaming with acknowledged cancellation, a bounded/durable job manager, local Phi lifecycle, and recording/import of new voice references. The prototype has no implemented source-document pipeline or independently editable segment stitching; those would be new features, not recovered functionality. Inferred word-based timing remains an estimate, never forced alignment.
