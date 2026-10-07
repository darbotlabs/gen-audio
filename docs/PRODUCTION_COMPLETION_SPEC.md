# Production completion

Python stays `requires-python = ">=3.14,<3.15"` with the classifier `Programming Language :: Python :: 3.14` only. Length caps, path confinement, checksum pins, `/health` versus `/ready`, and TTS error classes are Gen-Audio code in `gen_audio.guards` (see `docs/genlm-audio/REVIEW-2026-10-06.md`). This file lists what is still open before a Windows install is the product.

## Blockers

### Windows package and tray service

Acceptance:

- `scripts/build-tauri-windows.ps1` builds `gen-audio-mcp.exe`, runs an MCP `initialize` handshake on `127.0.0.1:8765`, stages `apps/desktop/src-tauri/binaries/gen-audio-mcp-<triple>.exe`, runs `npm ci` and `npm run build` in `apps/desktop`, and fails if `apps/desktop/dist/index.html` is missing. Only then does it invoke `tauri build --bundles nsis,msi`. It does not `cargo build` the desktop crate first. It fails unless an NSIS `*setup.exe` and an `.msi` exist. Launch the installer, not a pre-dist `gen-audio-desktop.exe` (that WebView is `http://localhost:1420`).
- The installer ships `gen-audio.exe` and `gen-audio-mcp.exe` side by side. Launch starts the sidecar, shows the window on the taskbar, and keeps a tray icon. If the sidecar binary is absent, the process falls back to the in-process listener. Closing the window hides it. Quit stops the sidecar.
- The NSIS hook adds a Startup shortcut. A packaged sidecar start also writes `HKCU\...\Run\DarbotGenAudio` unless `GEN_AUDIO_AUTOSTART=0`.
- `mcp_status` reports the bound address, `mode` (`sidecar`, `in-process`, `existing`, or `failed`), and whether `initialize` returned protocol `2025-03-26` and server name `gen-audio`.

`scripts/build-tauri.sh` is the Linux counterpart (deb when the Tauri CLI and webkit SDK are present). SMAX runs the Windows script.

### GenAID Audio limits on the production path

Acceptance:

- `tests/test_guards.py` (14 tests) passes on Python 3.14.
- Speaker scripts parsed by `gen_audio.cast` and the Rust harness reject oversized segments. Environment variables may tighten the caps and cannot raise the hard ceilings.
- Kokoro model resolution honors `GENAID_MODEL_BASE`, `GENAID_LOCAL_FILES_ONLY`, and the SHA256 pin.
- `GET /genaid-audio/health` is liveness. `GET /genaid-audio/ready` is readiness. Gateway health stays HTTP 200 when speech is down; gateway ready is 503. TTS proxy failures are 504, 503, forwarded 4xx, or 502, and the log line has no user-text prefix.
- Generate holds `model_lock`. Podcast disk IO uses `run_off_loop`.

## Gap inventory

Hits of the starter-name, backup, fallback, backlog, intended, stub, mock, placeholder, TODO, FIXME, HACK, not implemented, WIP, temporary, residual, sketch, NYI, dummy, and fake. Cargo.lock and dependency lockfiles are excluded. Historical review prose in `docs/REVIEW_BUGBASH.md` is append-only and is not rewritten.

| Location | Quote | Severity | Close plan |
| --- | --- | --- | --- |
| `docs/SERVE_APIM.md` (previous wording) | called the repo a starter that does not define an HTTP API | High | Closed. The doc now says this package does not start Serve and does not define a synthesis HTTP API. |
| `src/gen_audio/cube_revision.py`, CLI, README, `pyproject.toml` description | "cube-revision sketch" | Low | Keep. The inverse-HDR loop is a heuristic, not a mastering spec. The review says not to pretend it is the missing spectrogram benchmark. |
| `docs/ARCHITECTURE.md` desktop section | "browser DFT / WebGL sketch" | Low | Keep. The window drawing is not matplotlib `specgram`. The Python PNG path is the full spectrogram. |
| `crates/gen-audio-connectors` and connector docs | `mock` | Medium | Keep until a credential and `GEN_AUDIO_CONNECTOR_LIVE=1` are both set. Live calls are implemented behind that gate. |
| `schemas/examples/viewport.example.json` connector cards | `"mode": "mock"` | Low | Example data. The desktop replaces mode from `connector_statuses` when the shell is up. |
| `crates/gen-audio-core/src/bridge.rs` | `fallback` python program name | Low | Keep. It selects `python` or `python3`, then the basename allowlist. It is not a second synthesizer. |
| `docs/SERVE_APIM.md` shared gateway step | "explicit fallback" | Low | Keep. The shared gateway is not the catalog. |
| `src/gen_audio/artifacts.py` | "temporary" job directory | Low | Keep. Scratch disks are why `GEN_AUDIO_ARTIFACT_DIR` exists. |
| `docs/REVIEW_BUGBASH.md` | "Residual risk", "placeholder host", "sketch" | Info | Append-only review log. Do not edit prior rounds. |
| `crates/gen-audio-mcp` serve_health note | "Placeholder host" | Low | Keep. `<node>` is not probed. |
| VibeVoice / PersonaPlex / genlm-audio / Gradio adapters | "No adapter" on compare-list rows | High | Open. This repo scores WAVs and does not vendor those runtimes. They must use Python `>=3.14,<3.15` if they import `gen_audio`. |
| Real MP3, LUFS, auth on `/genaid-audio`, GPU cancel, chunk-and-stitch | Review P0/P1 unchecked boxes | High | Open. Listed in the review. Not claimed fixed. |
| Packaged installer executed on this Linux agent | no MSI here | Medium | `scripts/build-tauri-windows.ps1` is the SMAX path. This agent checks the crates and the MCP handshake. |

No `TODO`, `FIXME`, `HACK`, `NYI`, `WIP`, `dummy`, or `fake` markers remain in product source. `backup` and `backlog` have no hits.

## Workstreams

1. **Guards.** Done when `pytest tests/test_guards.py` passes on 3.14 and script parsing calls the same ceilings.
2. **Node probes.** Done when `gen_audio.node_http.dispatch` returns 200 for `/genaid-audio/health` and `/genaid-audio/ready` with the model unloaded, and `gateway_ready(None)` is 503.
3. **Desktop tray and MCP.** Done when `cargo check -p gen-audio-desktop` succeeds and the Windows script's handshake grep matches `protocolVersion` `2025-03-26`.
4. **Installers.** Done when SMAX produces NSIS and MSI from `scripts/build-tauri-windows.ps1` and the tray icon is visible after launch.
5. **Compare-list engines.** Open until an adapter exists. Do not mark VibeVoice, PersonaPlex, or Gradio implemented.

## D — Desktop honesty (fixtures ≠ podcast)

### Done

- Embedded Tauri release uses `frontendDist` (`../dist`) with `custom-protocol`; launch the installer / `gen-audio.exe` from `tauri build`, not a bare `cargo build` that loads `devUrl` (`http://localhost:1420`).
- Tray: close hides to tray; tray **Quit** stops the sidecar and `app.exit(0)`. `tauri-plugin-single-instance` focuses the existing window on second start (no dual tray).
- Library slide: livetiles backed by real synth WAVs on disk (`artifacts/library/`, served via `apps/desktop/public/library/`). WAVs are gitignored; JSON/PNG thumbs and `manifest.json` may ship in-tree. Manifest marks `synthesizedSpeech: true` only for real clips; unavailable engines stay `unavailable` with reasons — no fake speech.
- Fixture / example viewport content is labeled as fixture theater in the UI (not podcast speech). Browser DFT / WebGL cube sketch is not the Python matplotlib spectrogram path.
- Guards: `pytest tests/test_guards.py` → 14 passed on Python 3.14.

### Open on purpose

- `schemas/examples/viewport.example.json` remains example/fixture data for boot; it is not a generated podcast.
- Connector cards may show `mode: mock` until credentials + `GEN_AUDIO_CONNECTOR_LIVE=1`.
- Compare-list adapters (VibeVoice, PersonaPlex, Gradio, Magpie runtime on SMAX) stay open until a real harness exists — do not stamp them as podcast-ready.
- Cube-revision / browser DFT stay labeled sketch / not-podcast where they are not the full Python PNG spectrogram.

### Not done (product yellow)

- Full tray Quit click-through proof on every new build (document trigger + PIDs gone).
- Fresh NSIS + MSI from `scripts/build-tauri-windows.ps1` after each dirty UI/tray change when Optimus re-stamps A.