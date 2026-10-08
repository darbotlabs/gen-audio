# gen-audio

Darbot Gen-Audio is a desktop app plus a Python SDK for multi-speaker podcast tooling: a card viewport, fixture spectrogram and inverse-HDR cube views, a stateless MCP server, an Agent Client Protocol agent, a trace-writing harness, and connectors for Copilot, Claude, GPT, and Gemini.

[genaid](https://github.com/darbotlabs/genaid) is a separate JavaScript prompting framework. This repository does not vendor that code.

This repo does not ship model weights, voice binaries, API keys, or podcast renders. The spectrogram panel draws a browser formant map for the agent, voice, duration, perspectives, and loaded engine in the side pane. That map is not speech and not the Python spectrogram. The cube view stays a sine fixture tone.

## Desktop app

Targets: Windows (WebView2, NSIS and MSI), macOS (WebKit, dmg), Linux (webkit2gtk 4.1, deb/appimage). Ship the installer from the build script. Do not launch `target/release/gen-audio-desktop.exe` from a bare `cargo build -p gen-audio-desktop`: that compile leaves the `custom-protocol` feature off, so the WebView loads `devUrl` `http://localhost:1420` and Edge reports `ERR_CONNECTION_REFUSED`. MCP on `127.0.0.1:8765` can still be healthy while that window is dead.

Release builds embed `apps/desktop/dist` (`frontendDist`: `../dist`). `devUrl` is only for `tauri dev`. The scripts build the Vite app and refuse to continue if `apps/desktop/dist/index.html` is missing, then run `tauri build`. The installed app keeps a tray icon and a taskbar button while the window is open. It starts `gen-audio-mcp` beside the executable on `127.0.0.1:8765` (or `GEN_AUDIO_MCP_ADDR`). Closing the window hides it. Quit is on the tray menu and stops the sidecar. On Windows, a successful sidecar start registers the app under the current user's Run key unless `GEN_AUDIO_AUTOSTART=0`.

```bash
scripts/build-tauri.sh                 # Linux: mcp handshake, npm run build, then tauri build --bundles deb
# Windows: scripts\build-tauri-windows.ps1 -Mode Full|NoBundle (see Scripts below)
```

```bash
# library crates and ACP / MCP / harness tests
cargo test --workspace --exclude gen-audio-desktop
cargo check -p gen-audio-desktop

# UI
cd apps/desktop
npm install
npm run build
npm run dev   # browser preview on :1420, or `cargo tauri dev` inside src-tauri
```

The board sits inside a viewport border beside a blue setup pane. Drag a green engine card into the upper half of that pane to load the voice model. **Agent** is a voice persona (Anton, Alice, Khortana, Rocky, and the rest of the catalog, up to 8 on a track). **Voice** is the TTS or G2P model (kokoro-onnx, dayour/kokoro, misaki, and the unavailable slots). Copilot, Claude, GPT, and Gemini stay on the Connectors slide. `af_heart` is a Kokoro pack id referenced by persona Alice, not a Voice option. The `+` next to Agent adds another persona. Duration and a pasted prompt sit under that. Those controls redraw the spectrogram panel for that profile. The map is not speech.

Viewport layers jump to voice models, audio clips, an empty video layer, and the spatial cube. Library tiles with a real `wavUrl` expose play, pause, and a scrubber. If the WAV is not in this build the transport says the file is missing and does not invent audio. Magpie, VibeVoice, and Pocket stay unavailable. Flip a voice-profile tile for tone, purpose, domain, accent, traits, refs, a 2D browser map, and a 3D hook when a library cube JSON exists. Rename on a clip changes the semantic name and the face name in the window only.

Each card is a live tile: the front stays compact, and Enter or Flip turns it. Arrow keys move between cards. Home and End jump. "Clear viewport" shows the empty state inside the same border. "Run Python improve on fixture" calls the Python SDK from the Tauri shell and does nothing useful in a plain browser.

The card contract is `schemas/card-viewport.schema.json`. The example board is `schemas/examples/viewport.example.json`.

## MCP, ACP, and harness

```bash
cargo run -p gen-audio-mcp -- --smoke
cargo run -p gen-audio-mcp -- --http 127.0.0.1:8765
cargo run -p gen-audio-acp -- --smoke
cargo run -p gen-audio-harness -- --fixture
```

MCP is stateless: `initialize` stores no session, HTTP does not set `Mcp-Session-Id`, and the listener refuses non-loopback binds unless `GEN_AUDIO_MCP_HTTP_ALLOW_REMOTE=1`. `GET /health` is liveness. `GET /ready` means this MCP listener is up (`speech` is false, models are not loaded). It is not genaid-audio `/ready`. `GET /control` and `GET /control/stream` expose a short loopback command ring (SSE, no session id) for the desktop window. UI tools (`ui_navigate`, `ui_select_tile`, `ui_flip`, `ui_playback`, `ui_set_sidepane`, `ui_generate`, `library_list`, `library_rename`, `library_harvest`, `voice_profile_get`, `voice_profile_list`, `cube_layers`) queue that ring or return catalog data. They do not write speech. `ui_cube {mode: "single"|"compare", tileId?|uid?}` sets the Cube tab mode (Compare: the clip's Library cube, `library_r3`, beside the same WAV's `pipeline_r2` cube on one playback slice); the Compare button posts the same tool, and `viewport_get` reads `cube_mode` and `cube_compare` back (contract: `schemas/examples/ui_cube.contract.json`). `ui_generate` records a running or unavailable phase and does not call `synth`. `synth` still refuses when the Kokoro model env vars are unset and publishes `phase: refused` with `synthesizedSpeech: false`. The voice-profile contract is `schemas/voice_profile.schema.json`.

ACP is session-scoped because that protocol requires `sessionId`. `session/new` can bind persona agents and a Voice model. A prompt such as `profile alice`, `agent alice rocky voice kokoro_onnx`, or `generate` calls those MCP tools and emits `tool_call` updates. `generate` also calls `synth` when the Voice model has an adapter, and the result stays a refusal until model files are configured. Prompt audio is false. The exact prompts `health` and `status` still return connector health. The harness prints JSONL. It skips synthesis instead of inventing speech. `--fixture` writes a labeled sine WAV. `--improve` runs the Python publish chain on that fixture when the checkout is available.

Connector environment variables, mock behavior, and the live flag are in [docs/CONNECTORS.md](docs/CONNECTORS.md).

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
| VibeVoice, Magpie, Pocket TTS, dayour Kokoro | Names on the compare list only. No adapter and no weights |
| misaki | Grapheme-to-phoneme library used by Kokoro. Not a waveform engine, and this repo does not call it |
| Node HTTP process | Not started by import. URL helpers plus `python -m gen_audio.node_http` |
| Tauri desktop card viewport | Implemented. Agent is a persona and Voice is a TTS model. Library tiles play a WAV when the file loads. The spectrogram panel follows that profile. The pipeline cube card stays a fixture; the spatial slide draws library cube JSON when a clip has one |
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

Windows tooling has exactly four entry points. Each one runs under Windows PowerShell 5.1 and PowerShell 7, checks every native exit code, and exits non-zero on failure. Extend these with a parameter instead of adding a new script: `scripts/test.ps1` fails on any other `*.ps1`, `*.bat`, `*.cmd` or `*.py` outside the allowlist, including untracked and gitignored files such as anything under `artifacts/`.

| Entry point | What it does |
|---|---|
| `scripts\build-tauri-windows.ps1 [-Mode Full\|NoBundle]` | The only build path. Imports the MSVC environment (`Import-VsDevEnv`), builds and handshakes `gen-audio-mcp`, stages it for `externalBin`, runs `npm ci`, then the pinned tauri CLI (`@tauri-apps/cli@2.12.1`). `Full` (default) makes NSIS and MSI. `NoBundle` runs `tauri build --no-bundle`, which still embeds `frontendDist`. It checks that `dist` and every output are from this run, that `dist/index.html` has the header markers, that the build is not a dev build (`cargo:rustc-cfg=dev`, which would load `localhost:1420`), and that `target\release` holds one `gen-audio-desktop` fingerprint. It prints one `BUILD_OK` line, and only on success. |
| `scripts\test.ps1 [-Tag name] [-Skip Pssa,Sprawl,Regen,Cargo,Npm,Python] [-SprawlExclude path]` | PSScriptAnalyzer 1.24.0 with `PSScriptAnalyzerSettings.psd1` (any finding fails), the sprawl gate, a regen check (reruns the `build_assets` and `asset_vectors` examples, then `git diff --exit-code` over `assets.json`, `assets.dev.json`, `fixtures_v1.json`, `v1.json` and `viewport.{example,release}.json`; `build_assets` is skipped, and says so, when the gitignored library WAVs are absent; PNGs are checked by pixels in pytest because zlib-ng makes their bytes vary), `cargo test --workspace`, `npm test` and `tsc --noEmit` in `apps/desktop`, and `pytest`. Prints a `TEST_SUMMARY` line. Logs go to `artifacts/test-logs/`. |
| `scripts\mcp-call.ps1 -Tool <name> [-ArgsJson <json>] [-Port <n>]` | One MCP `tools/call` over HTTP. Finds the server from `-Port`, `GEN_AUDIO_MCP_ADDR`, `127.0.0.1:8765`, then the loopback ports a `gen-audio` process listens on, and checks `initialize` says `gen-audio`. Example: `scripts\mcp-call.ps1 -Tool ui_navigate -ArgsJson '{"slide":"slide:library"}'`. |
| `scripts\ui-shot.ps1 -Out <png>` | Captures the Gen-Audio window with Win32 `PrintWindow`. No input injection and no screen-scrape fallback. Fails on a blank capture and prints the PNG's sha256. |

`scripts/lib/devenv.ps1` holds the shared helpers (`Import-VsDevEnv`, `Invoke-Checked`) and is the only file that may source `vcvars64.bat`. The `scripts/*.py` files are thin CLI shims into `gen_audio.cli`. The Library cube JSON comes from `python scripts/cube_revision.py layers WAV OUT_JSON --stem STEM --engine ENGINE --revision N --png PNG`. The Library manifest has one home, `apps/desktop/public/library/manifest.json`. Vite copies it into `dist` at build time.

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

The tests synthesize tones in memory. They do not load kokoro-onnx and they do not need model files. A real kokoro render is a manual step once the ONNX files are on disk.

## License

MIT. See [LICENSE](LICENSE).
