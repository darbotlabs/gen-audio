# gen-audio

Darbot Gen-Audio is a desktop app plus a Python SDK for multi-speaker podcast tooling: a card viewport, fixture spectrogram and inverse-HDR cube views, a stateless MCP server, an Agent Client Protocol agent, a trace-writing harness, and connectors for Copilot, Claude, GPT, and Gemini.

[genaid](https://github.com/darbotlabs/genaid) is a separate JavaScript prompting framework. This repository does not vendor that code.

This repo does not ship model weights, voice binaries, API keys, or podcast renders. Spectrogram and cube views in the desktop app are drawn from a sine **fixture tone**. They are not speech.

## Desktop app

Targets: Windows (WebView2, NSIS and MSI), macOS (WebKit, dmg), Linux (webkit2gtk 4.1, deb/appimage). The desktop process starts the stateless MCP listener on `127.0.0.1:8765` (or `GEN_AUDIO_MCP_ADDR`) and keeps a tray icon. Closing the window hides it. Quit is on the tray menu.

```bash
scripts/build-tauri.sh                 # Linux: mcp handshake, release binary, deb when the Tauri CLI is present
# Windows, from PowerShell:
# scripts/build-tauri-windows.ps1      # NSIS + MSI, sidecar gen-audio-mcp.exe, initialize handshake
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

Keyboard: arrow keys move between cards, Home and End jump. "Empty viewport" shows the empty state. "Run Python improve on fixture" calls the Python SDK from the Tauri shell and does nothing useful in a plain browser.

The card contract is `schemas/card-viewport.schema.json`. The example board is `schemas/examples/viewport.example.json`.

## MCP, ACP, and harness

```bash
cargo run -p gen-audio-mcp -- --smoke
cargo run -p gen-audio-mcp -- --http 127.0.0.1:8765
cargo run -p gen-audio-acp -- --smoke
cargo run -p gen-audio-harness -- --fixture
```

MCP is stateless: `initialize` stores no session, HTTP does not set `Mcp-Session-Id`, and the listener refuses non-loopback binds unless `GEN_AUDIO_MCP_HTTP_ALLOW_REMOTE=1`. ACP is session-scoped because that protocol requires `sessionId`. The harness prints JSONL. It skips synthesis instead of inventing speech. `--fixture` writes a labeled sine WAV. `--improve` runs the Python publish chain on that fixture when the checkout is available.

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
| Tauri desktop card viewport | Implemented. Spectrogram and cube views use a fixture tone |
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
