# gen-audio

Darbot Gen-Audio is a desktop app plus a Python SDK for multi-speaker podcast tooling: a card viewport, fixture spectrogram and inverse-HDR cube views, a stateless MCP server, an Agent Client Protocol agent, a trace-writing harness, and connectors for Copilot, Claude, GPT, and Gemini.

[genaid](https://github.com/darbotlabs/genaid) is a separate JavaScript prompting framework. This repository does not vendor that code.

Model weights, voice packs, API keys, and scratch renders are not committed. Curated WAVs in `apps/desktop/public/library` are distributable assets and are not ignored. Every desktop build checks their actual bytes, hashes, and audio metadata against the asset catalog and speech claims. The profile spectrogram is a browser preview, not speech; the pipeline cube remains a fixture, while library spatial views use the associated audio analysis.

## Desktop app

The supported installer pipeline targets Windows (WebView2, NSIS and MSI). Linux CI covers the native crates; Linux/macOS installers are not produced by the Windows pipeline. Ship the installer from the build script. A bare desktop `cargo build` does not enable Tauri's embedded frontend protocol.

Release builds embed `apps/desktop/dist` (`frontendDist`: `../dist`). `devUrl` is only for `tauri dev`. The tracked build scripts install locked npm dependencies, build and smoke-test the actual MCP executable without opening a port, stage it, then invoke the pinned local Tauri CLI. Tauri's `beforeBuildCommand` builds the frontend and aborts on failure; existing `dist` files cannot bypass that step. Cargo's reported artifact path is used, including when `CARGO_TARGET_DIR` is set.

The installed app keeps a tray icon and a taskbar button while the window is open. It starts its own authenticated `gen-audio-mcp` beside the executable on `127.0.0.1:8765` (or `GEN_AUDIO_MCP_ADDR`), with an owned-listener fallback when that address is occupied. The frontend obtains the negotiated address and process token through Tauri IPC; it never attaches to an unrelated listener. Closing the window hides it. Quit stops the owned sidecar. Windows login autostart is opt-in with `GEN_AUDIO_AUTOSTART=1`; the installer does not create a startup shortcut.

**Use this on Windows**, with Node/npm, native MSVC Rust, and Visual Studio C++ Build Tools installed. The script discovers Developer PowerShell automatically; no machine-specific wrapper is needed.

```powershell
.\scripts\build-tauri-windows.ps1             # NSIS and MSI installers
.\scripts\build-tauri-windows.ps1 -NoBundle   # Embedded executable only; no installer
```

This is the single maintained build entry point; `-Mode Full` and `-Mode NoBundle` are also supported. It builds but does not install, launch, or stop an existing app. It does not clean Cargo caches. The default is two Cargo jobs; set `CARGO_BUILD_JOBS` to override. There is no second Unix wrapper.

`artifacts/anti-pattern/` is a local, ignored quarantine for obsolete wrappers and one-off patchers, not an alternate build path. Do not execute its scripts or depend on them from maintained code. Local one-offs from the artifact root and library are preserved under `artifact-root/` and `library/` there. Library media is left intact. Reusable fixes belong in tracked source, `scripts/`, and tests; never patch TypeScript, schemas, or `dist` from an asset directory. `dist` and `target` are generated output, not source.

Keep execution output under `artifacts`: compiler/installer logs in `build-logs`, test output in `test-logs`, model/setup logs in `runtime-logs`, and UI captures in `screenshots`, grouped by date or run. `artifacts/library` holds local media and its provenance, not build helpers. The packaged `public/library` gate rejects executable scripts, logs, caches and temporary files. Analysis reports and media are preserved; cleanup does not delete model weights, environments, Cargo caches or pending source changes.

```bash
# library crates and ACP / MCP / harness tests
cargo test --workspace --exclude gen-audio-desktop
cargo check -p gen-audio-desktop

# UI
cd apps/desktop
npm ci
npm test
npm run build
npm run dev   # native Tauri window and Rust backend; stage the sidecar with the build script first
```

For a clean signed MSIX build and installation on Windows:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File D:\gen-audio\scripts\build-tauri-windows.ps1 -Mode Msix -Clean -Install
```

For a clean production MSI, use `-Mode Msi -Clean`. Its current output is
`target/release/bundle/msi`; archived deliverables use `preserved-artifacts`.

`-Clean` preserves existing installers and logs under `artifacts/builds`, clears
Cargo and frontend build outputs, disables compiler caches, and uses a fresh
temporary npm cache. MSIX packaging uses Windows SDK MakeAppx/SignTool and a
valid Darbot Labs code-signing certificate in the current user's certificate
store; select another certificate with `-SigningCertificateThumbprint`. The
package contains the desktop executable, embedded UI, MCP sidecar, SDK source
and curated assets; the installed application uses its own runtime payload.
Rebuilding an installed application version increments the fourth MSIX version
component so updates preserve application data without an uninstall. Local
Python runtimes, model weights and WebView2 remain runtime prerequisites.

The board sits inside a viewport border beside a blue setup pane. Drag a green engine card into the upper half of that pane to load the voice model. **Agent** is a voice persona (Anton, Alice, Khortana, Rocky, and the rest of the catalog, up to 8 on a track). **Voice** is the TTS or G2P model (kokoro-onnx, dayour/kokoro, misaki, and the unavailable slots). Copilot, Claude, GPT, and Gemini stay on the Connectors slide. `af_heart` is a Kokoro pack id referenced by persona Alice, not a Voice option. The `+` next to Agent adds another persona. Duration and a pasted prompt sit under that. Those controls redraw the spectrogram panel for that profile. The map is not speech.

Viewport layers jump to voice models, audio clips, an empty video layer, and the spatial cube. Library tiles with a real `wavUrl` expose play, pause, and a scrubber. Missing media is reported explicitly. VibeVoice has a desktop generation adapter with runtime and memory preflight; Magpie and Pocket TTS have no synthesis adapters. Profile flipcards expose persona metadata and engine manifests, while spectral and spatial views belong to audio clips. Clip renames are saved as semantic metadata through the Rust library store.

Each card is a live tile: the front stays compact, and Enter or Flip turns it. Arrow keys move between cards. Home and End jump. "Clear viewport" shows the empty state inside the same border. Dev and test builds (`VITE_GEN_AUDIO_FIXTURES=1` with a debug Tauri shell) add "Load labeled example" and "Run Python improve on fixture"; release builds have neither control, and the release exe does not register the `viewport_example` or `run_fixture_improve` commands.

The card contract is `schemas/card-viewport.schema.json`. The example board is `schemas/examples/viewport.example.json`.

## MCP, ACP, and harness

```bash
cargo run -p gen-audio-mcp -- --smoke
cargo run -p gen-audio-acp -- --smoke
cargo run -p gen-audio-harness -- --fixture
```

MCP is stateless: `initialize` stores no session and HTTP does not set `Mcp-Session-Id`. Loopback HTTP requires a cryptographically random 64-hex `GEN_AUDIO_MCP_TOKEN`; callers send it in an Authorization bearer header, never a URL. All data routes, including health, controls and artifacts, are protected. Unknown browser origins/hosts, non-JSON MCP POSTs and non-loopback binds are rejected; the old remote-bind override is no longer accepted. CLI stdio does not use HTTP authentication.

HTTP refusals are counted and logged without request contents or credentials. The desktop retains bounded, rotating sidecar diagnostics in its app log directory. `ui_playback` distinguishes user versus automatic playback; seek responses report the requested and actual landed position after the window acknowledges the result, or an explicit failure. Control payload contracts live under `schemas/examples/control`.

`GET /health` is liveness; `/ready` means listener readiness, not loaded-model readiness. The desktop polls authenticated `/control` with bounded backoff. UI tools queue controls or return catalog data; `ui_generate` alone does not write speech. `synth` accepts exactly one of `script` or `scriptText`, plus either a cast map or mapped `agents` and `voice`. It no longer silently renders the sample instead of supplied text. Successful synthesis requires a matching receipt, WAV, manifest, hash and requested cast/text provenance. Only then are issued WAV/JSON files downloadable from authenticated `/artifacts/<name>`. The desktop uses the submitted script and exposes verified audio playback and download.

Kokoro still requires local `GEN_AUDIO_MODEL_DIR`, `GEN_AUDIO_KOKORO_MODEL` and `GEN_AUDIO_KOKORO_VOICES`. Required-checksum mode covers both files and fails closed; provenance hashes alone are not trusted pins. Only Alice and Frank currently have verified Kokoro pack mappings; unmapped personas are refused. The voice-profile contract is `schemas/voice_profile.schema.json`.

ACP is session-scoped because that protocol requires `sessionId`. `session/new` binds personas and a voice model; each session retains its own issued artifacts until cancellation. Generation uses supplied script content and actual tool outcomes rather than a fixed false speech claim. Prompt audio remains false. The exact prompts `health` and `status` return connector health. The harness prints JSONL and skips synthesis instead of inventing speech. `--fixture` writes a labeled sine WAV; `--improve` runs the Python publish chain on it.

Connector environment variables, mock behavior, and the live flag are in [docs/CONNECTORS.md](docs/CONNECTORS.md).

## Migrated prototype workflows

The original `T:\Gen.AID.Audio` is a read-only reference, not a dependency. Its first migrated workflows live in Rust and are exposed through the existing authenticated MCP/Tauri path:

- **Script authoring:** open **Script authoring and LLM draft** to analyze numbered dialogue, explicitly convert zero-based labels, or alternate unlabeled lines between selected personas. Word-based duration estimates and warnings stay outside the spoken script.
- **Draft/rewrite:** enter a separate topic, select style/provider, and preview the exact request before confirming. The live connector must be configured; mock or truncated output is rejected. Review the result and choose **Apply validated script** before synthesizing. This does not load the prototype's Phi runtime.
- **Podcast mastering:** after generation or reopening a saved generation, choose **Create podcast master**. Rust runs the prototype's high-pass/low-pass/compressor and measured two-pass loudness normalization, targeting -16 LUFS, -1.5 dBTP and LRA 4. The original remains downloadable; the separate master has its own measured report. Requires FFmpeg on PATH (or `GEN_AUDIO_FFMPEG`), mono 24 kHz PCM16 input up to ten minutes, and a bounded execution budget.

MCP clients use `script_analyze`, `draft_plan`, `draft_generate`, and `master_audio`. Drafting requires confirmation bound to the previewed prompt. Mastering returns `processedAudio: true`, not a new speech-synthesis claim. See `docs/ARCHITECTURE.md` for the migration map and deferred streaming/job/recording work.

## Python SDK

The installable package is still `gen_audio` (`pip install -e .`). It turns a two-speaker script into a WAV with [kokoro-onnx](https://github.com/thewh1teagle/kokoro-onnx) when you supply model files, then runs the 24 kHz publish chain, draws spectrogram PNGs, and can run the cube-revision sketch.

## What actually runs

| Piece | Status |
| --- | --- |
| `Speaker N` parsing and cast-map voice resolution | Implemented |
| Multi-turn synthesis via `kokoro-onnx` | Implemented when local model files are provided |
| Publish chain: trim silence, ~70 Hz high-pass, peak ~0.89, resample 24 kHz | Implemented |
| Before/after spectrogram PNGs | Implemented |
| Inverse-HDR / BW95 / clip-fraction revision sketch | Implemented as a heuristic, not a mastering standard |
| Engine compare on WAVs you already have | Implemented as measurement only; it does not rank engines |
| VibeVoice | Isolated CPU worker implemented; runtime, tokenizer, reference WAVs and memory preflight required. Local real inference is not yet validated |
| PersonaPlex | Separate speech-to-speech worker requiring input audio and persona conditioning; not a verbatim text-to-speech engine. Local real inference is not yet validated |
| Magpie, Pocket TTS, dayour Kokoro | No synthesis adapter; imported media does not imply an installed runtime |
| misaki | Grapheme-to-phoneme library used by Kokoro. Not a waveform engine, and this repo does not call it |
| Node HTTP process | Not started by import. URL helpers plus `python -m gen_audio.node_http` |
| Tauri desktop card viewport | Implemented with a Rust viewport reducer. Agent is a persona and Voice selects a synthesis adapter. Spectral and spatial views use audio assets; fixture cards are excluded from production |
| Stateless MCP (`gen-audio-mcp`) | Implemented (stdio and loopback HTTP) |
| ACP agent (`gen-audio-acp`) | Implemented handshake and `session/prompt` |
| Harness traces (`gen-audio-harness`) | Implemented. Synth is skipped without weights |
| Copilot, Claude, GPT, Gemini connectors | Implemented. Mock unless a key is set and `GEN_AUDIO_CONNECTOR_LIVE=1` |

kokoro-onnx voice ids in the example cast are `af_heart` (Alice, Speaker 1) and `am_michael` (Frank, Speaker 2). Those ids exist in the Kokoro v1 voice list. A voice id still has to be present in the voice pack you load; this repo does not check that until kokoro-onnx does.

## Install

Python 3.14 is required (`requires-python = ">=3.14,<3.15"`). Install it from [python.org](https://www.python.org/downloads/) or with uv:

```bash
uv python install 3.14
uv venv --python 3.14 .venv
source .venv/bin/activate
pip install -e ".[dev]"
```

`soundfile` needs the system `libsndfile` library (`libsndfile1` on Debian/Ubuntu).

The synthesizer extra is separate, and it still needs the ONNX model and the voices file on disk:

```bash
pip install -e ".[kokoro]"
export GEN_AUDIO_KOKORO_MODEL=/path/to/kokoro-v1.0.onnx
export GEN_AUDIO_KOKORO_VOICES=/path/to/voices-v1.0.bin
```

Those filenames are the v1 pair documented by kokoro-onnx. Weights are published with the [kokoro-onnx](https://github.com/thewh1teagle/kokoro-onnx) project and the [Kokoro-82M ONNX](https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX) repo. Do not commit them here. `.gitignore` ignores `*.onnx`, `*.bin`, `*.wav`, and `models/`.

## Quickstart

From a checkout, with the environment variables above set:

```bash
python scripts/synth_kokoro_onnx.py examples/podcast_script_sample.txt \
  --cast-map voices/cast_map.example.json \
  -o audio/sample.wav
# sample.wav is float32 so peaks outside [-1, 1] are not clipped yet.

python scripts/improve.py audio/sample.wav -o audio/sample-24k.wav

python scripts/spectrogram.py \
  --before audio/sample.wav \
  --after audio/sample-24k.wav \
  --out-dir artifacts/sample

python scripts/cube_revision.py audio/sample-24k.wav \
  -o audio/sample-cube.wav \
  --plot artifacts/sample/cube.png
```

The same commands are installed as `gen-audio-synth`, `gen-audio-improve`, `gen-audio-spectrogram`, and `gen-audio-cube`.

`examples/podcast_script_sample.txt` is a labeled fixture. It is not a news script and not a recording.

Library use of the publish chain, which does not need a model:

```python
from gen_audio.audio_io import read_wav, write_wav
from gen_audio.improve import improve

audio, sample_rate = read_wav("take.wav")
result = improve(audio, sample_rate)
write_wav("take-24k.wav", result.audio, result.sample_rate)
```

Score WAVs rendered elsewhere. This writes measurements; it does not declare a better engine:

```bash
python scripts/compare_wavs.py \
  kokoro_onnx=audio/sample.wav \
  pocket_tts=audio/pocket-render.wav \
  -o artifacts/scores.json
```

`python scripts/compare_wavs.py --list-engines` prints the compare list and the status of each id.

## Script format

```text
# comments are ignored
Speaker 1: Same-line text.
Speaker 2 (Frank):
Following lines stay with this speaker until the next header.
```

The parenthetical name is stored and is not the lookup key. `voices/cast_map.example.json` maps speaker `"1"` and `"2"` to kokoro-onnx voices. Speeds outside 0.5–2.0 are rejected because that is the range kokoro-onnx accepts.

## Publish chain

Order is fixed: trim leading and trailing silence (default −40 dB relative to the file peak, 30 ms pad), zero-phase high-pass at 70 Hz, peak-normalize to 0.89 linear, resample to 24 kHz. Internal pauses are kept. Details and the scratch-directory copy step are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

Set `GEN_AUDIO_ARTIFACT_DIR` (or pass `--artifact-dir`) when a job writes under a temporary directory. The helpers copy the finished WAV, PNG, or JSON there.

## Nodes

Per-node base URL: `http://<node>:8002/genaid-audio`

Health: `http://<node>:8002/genaid-audio/health`

`10.1.8.70:8002` is the shared gateway row in the Power Table, not a stand-in for each node. See [docs/SERVE_APIM.md](docs/SERVE_APIM.md).

## Scripts

Windows tooling has four PowerShell entry points plus the installed-app MCP stdio bridge. Extend maintained commands rather than adding one-off wrappers. The sprawl gate scans authored files, including ignored output folders; model snapshots and the explicit quarantine are reported exclusions, and filesystem links are not followed.

| Entry point | What it does |
|---|---|
| `scripts\build-tauri-windows.ps1 [-Mode Full\|NoBundle]` | Single build pipeline: verifies locked library media, builds the sidecar with locked Cargo dependencies, smoke-tests that executable without a shared HTTP port, verifies the staged hash, installs locked npm dependencies and invokes the pinned local Tauri CLI. The frontend hook builds fresh release content. Outputs are checked by content rather than modification time; valid unchanged outputs are accepted. Full mode produces NSIS/MSI; NoBundle embeds the same release frontend without installers. A transcript and final `BUILD_OK`/`BUILD_FAIL` identify the run. |
| `scripts\test.ps1 [-Tag name] [-Skip Pssa,Sprawl,Regen,Cargo,Npm,Python] [-SprawlExclude path] [-FromLock \| -WithWav]` | An EOL preflight first (`git ls-files --eol`: a tracked file with `eol=lf` in `.gitattributes` but CRLF in the working copy fails the run, is listed, and the fix is printed; nothing is rewritten), then PSScriptAnalyzer 1.24.0 with `PSScriptAnalyzerSettings.psd1` (any finding fails), the sprawl gate, a regen check (reruns the `build_assets` and `asset_vectors` examples and `cube_revision.py manifest`, then `git diff --exit-code` over `assets.json`, `manifest.json`, `assets.dev.json`, `media.lock.json`, `fixtures_v1.json`, `v1.json` and `viewport.{example,release}.json`; a missing gitignored library WAV fails the step, and `-FromLock` (CI) takes the WAV facts from `schemas/asset-object/media.lock.json`; PNGs are checked by pixels in pytest because zlib-ng makes their bytes vary), `cargo test --workspace`, `npm test` and `tsc --noEmit` in `apps/desktop`, and `pytest`. `-WithWav` is the merge check that the PR template requires. It first proves the staged WAVs are the `media.lock.json` bytes (Wavs step), then runs pytest with `GEN_AUDIO_REQUIRE_WAVS=1`, so a WAV test that would skip fails instead: the byte-for-byte cube regeneration, inv_hdr and strip tests must run. Prints a `TEST_SUMMARY` line. Logs go to `artifacts/test-logs/`. |
| `scripts\mcp-call.ps1 -Tool <name> [-ArgsJson <json>] [-ConnectionFile <path>] [-Port <n>]` | One authenticated MCP tool call. Requires the process token in the environment or a private transient desktop connection export. Never pass a token in a URL or command argument. `-OutFile` writes only the JSON-RPC body. |
| `scripts\ui-shot.ps1 -Out <png>` | Captures the Gen-Audio window with Win32 `PrintWindow`. No input injection and no screen-scrape fallback. Fails on a blank capture and prints the PNG's sha256. |

`scripts/lib/devenv.ps1` owns shared toolchain/build helpers. Python command shims delegate to `gen_audio.cli`; `connect-desktop-mcp.py` is the maintained installed-app stdio bridge. Cube generation supports `--method library_r3` and `--method pipeline_r2` through `cube_revision.py layers`; `inv_hdr` remains RMS/peak, separate from composite layer scores. `cube_revision.py manifest` derives metadata from the generated files. The active Library manifest has one home in `apps/desktop/public/library`; Vite copies it at build time.

## Layout

```text
src/gen_audio/          installable package
  cast.py               Speaker N parsing and cast maps
  synth_kokoro_onnx.py  multi-turn kokoro-onnx render
  improve.py            24 kHz publish chain
  spectrogram.py        before/after PNG helpers
  guards.py             length, path, checksum, and TTS error guards
  gateway.py            /health versus /ready and proxy status classes
  node_http.py          loopback genaid-audio health and ready routes
  cube_revision.py      serial inv-HDR / BW95 / clip_frac sketch
  cube_layers.py        four-layer Library cube; inv_hdr is rms/peak, layer_score the composite (cube_revision.py layers)
  compare.py            measure existing WAVs
  engines.py            compare-list registry
  serve.py              per-node URL and health-body helpers
scripts/                CLI entry points for the modules above
voices/cast_map.example.json
examples/podcast_script_sample.txt
docs/ARCHITECTURE.md
docs/SERVE_APIM.md
```

## Tests

```bash
pip install -e ".[dev]"
pytest
```

The desktop has separate **Voice models** and **Agent Voice Profiles** tabs. Agent profiles include all engine bindings, searchable traits and projects, hash-verified acoustic-reference audition, and downloads of the complete profile, Adaptive Card JSON, and JSON Schema. Profiles are defined in `voices/manifest.json` and checked against `schemas/voice_profile.schema.json`; linked projects use repository-relative paths.

Generated audio records AgentName as `metadata.artist` and keeps individual names in `metadata.artists` and `metadata.agentNames`. A multi-agent Artist is joined in order, for example `Alice; Frank`. The WAV embeds the same Artist before final hashing. Library reopening, Improve, the analysis pipeline, and mastering retain attribution. Isolated VibeVoice/PersonaPlex workers accept `--agent-name`; the desktop VibeVoice adapter supplies names from the selected profiles.

The default tests do not load large model weights. Opt in to real local Kokoro generation with `GEN_AUDIO_RUN_REAL_KOKORO=1`. After building `gen-audio-mcp`, `GEN_AUDIO_RUN_REAL_MCP=1` enables the authenticated two-voice HTTP round trip, issued artifact download, Artist metadata preservation, and downstream audio-processing regression. These checks never replace curated library media.

## Isolated model workers

The application stays on Python 3.14. PersonaPlex and the supplied VibeVoice checkpoints use separate Python 3.12 environments; do not mix their pinned dependencies with each other or the application. Runtime source revisions and direct dependencies are in `runtimes/requirements-personaplex.txt` and `runtimes/requirements-vibevoice.txt`. These are install requirements, not a verified cross-platform transitive lock.

The worker is invoked through the isolated interpreter with this checkout's `src` on `PYTHONPATH`. Start with `--preflight`, which inspects assets, dependency versions, and available memory without importing Torch or loading model weights:

```powershell
$env:PYTHONPATH = (Resolve-Path .\src)
py -3.12 -m gen_audio.model_workers --engine vibevoice --model-dir .\models\Vibevoice-1.5B --text "Speaker 1: A local speech request." --voice-ref .\reference.wav --preflight
py -3.12 -m gen_audio.model_workers --engine personaplex --model-dir .\models\Personaplex-7B --input-audio .\input.wav --persona "You are a helpful conversational assistant." --voice-ref .\reference.wav --preflight
```

Generation additionally requires `--output` and a ready preflight. References and conversation input must be mono 24 kHz PCM16 WAVs. Only use voices you are authorized to use. VibeVoice needs an explicit compatible local Qwen tokenizer directory (`--tokenizer`); it never falls back to the Hub. PersonaPlex uses raw reference audio, not unrestricted `.pt` pickle presets. Offline network restrictions, duration/token limits, and a parent-process hard deadline remain mandatory.

Worker success describes actual produced audio and includes a SHA256 receipt; it does not assert transcript accuracy. PersonaPlex output is a generated conversation response, not the supplied persona read aloud. On this host, preflight reported missing isolated dependencies and insufficient memory for all supplied large checkpoints. No real PersonaPlex/VibeVoice inference result is claimed.

MCP exposes the workers through `model_preflight` and `model_generate`. Configure absolute `GEN_AUDIO_VIBEVOICE_PYTHON`, `GEN_AUDIO_VIBEVOICE_MODEL_DIR` and `GEN_AUDIO_VIBEVOICE_TOKENIZER` paths, or the corresponding `GEN_AUDIO_PERSONAPLEX_*` variables (its tokenizer is a SentencePiece file). Tool callers cannot choose runtime executables or arbitrary filesystem paths: `voiceRefs` and PersonaPlex `inputAudio` are names of already-issued scratch WAVs. VibeVoice uses `scriptText`; PersonaPlex uses `persona` plus input audio. Capped output is reported as incomplete, not a finished transcript. Generation is bounded by the parent subprocess deadline as well as worker limits.

## License

MIT. See [LICENSE](LICENSE).
