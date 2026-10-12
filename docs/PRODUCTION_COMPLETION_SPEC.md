# Production completion

The base SDK stays `requires-python = ">=3.14,<3.15"` with the classifier `Programming Language :: Python :: 3.14` only. Isolated model workers use pinned Python 3.12 dependencies. Length caps, path confinement, checksum pins, `/health` versus `/ready`, and TTS error classes are Gen-Audio code in `gen_audio.guards` (see `docs/genlm-audio/REVIEW-2026-10-06.md`). This file records production acceptance criteria; [the 2026-10-08 migration record](MIGRATION-2026-10-08.md) tracks the implemented desktop workflow and remaining runtime validation.

## Blockers

### Windows package and tray service

Acceptance:

- `scripts/build-tauri-windows.ps1` installs locked npm dependencies, builds `gen-audio-mcp.exe` with `--locked`, obtains its path from Cargo's artifact record, and runs that binary's `--smoke` in disposable scratch. No shared HTTP port is used. It stages `apps/desktop/src-tauri/binaries/gen-audio-mcp-<triple>.exe`, then invokes the pinned local Tauri CLI. The Tauri hook runs `npm run build` before embedding the frontend. Default output is NSIS + MSI, with newly produced installers required; `-NoBundle` builds only the embedded executable. It does not bare-Cargo-build the desktop, install it, or stop a running app.
- The installer ships `gen-audio.exe` and `gen-audio-mcp.exe` side by side. Launch starts the sidecar, shows the window on the taskbar, and keeps a tray icon. If the sidecar binary is absent, the process falls back to the in-process listener. Closing the window hides it. Quit stops the sidecar.
- The NSIS hook does not add a Startup shortcut. Login persistence is opt-in only with `GEN_AUDIO_AUTOSTART=1`; uninstall still cleans old shortcuts.
- `mcp_status` reports the owned listener's bound address, transient bearer token, `mode` (`sidecar`, `in-process`, or `failed`), and authenticated handshake status. It is a privileged Tauri IPC response, not a public HTTP endpoint; never log the token.

`scripts/build-tauri-windows.ps1` is the single supported build pipeline. Linux CI covers the native desktop crate but does not produce an installer. Quarantined scripts under `artifacts/anti-pattern` are preserved evidence, never build dependencies.

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
3. **Desktop tray and MCP.** Done when `cargo check -p gen-audio-desktop` succeeds and the freshly built sidecar's `--smoke` succeeds, independently of a running desktop.
4. **Installers.** Done when SMAX produces NSIS and MSI from `scripts/build-tauri-windows.ps1` and the tray icon is visible after launch.
5. **Compare-list engines.** Open until an adapter exists. Do not mark VibeVoice, PersonaPlex, or Gradio implemented.

## D — Desktop honesty (fixtures ≠ podcast)

### Done

- Embedded Tauri release uses `frontendDist` (`../dist`) with `custom-protocol`; launch the installer / `gen-audio.exe` from `tauri build`, not a bare `cargo build` that loads `devUrl` (`http://localhost:1420`).
- Tray: close hides to tray; tray **Quit** stops the sidecar and `app.exit(0)`. `tauri-plugin-single-instance` focuses the existing window on second start (no dual tray). A second start with `--quit` invokes the same `request_quit` path.
- Library slide: curated WAVs under `apps/desktop/public/library/` are not ignored. The mandatory frontend prebuild gate checks every declared media hash/size and speech metadata. `artifacts/library/` remains scratch and is not an implicit clean-build dependency; one-off scripts remain quarantined.
- Fixture / example viewport content is labeled as fixture theater in the UI (not podcast speech). Browser DFT / WebGL cube sketch is not the Python matplotlib spectrogram path.
- Guards: `pytest tests/test_guards.py` → 14 passed on Python 3.14.

### Open on purpose

- `schemas/examples/viewport.example.json` remains example/fixture data for boot; it is not a generated podcast.
- Connector cards may show `mode: mock` until credentials + `GEN_AUDIO_CONNECTOR_LIVE=1`.
- Compare-list adapters (VibeVoice, PersonaPlex, Gradio, Magpie runtime on SMAX) stay open until a real harness exists — do not stamp them as podcast-ready.
- Cube-revision / browser DFT stay labeled sketch / not-podcast where they are not the full Python PNG spectrogram.

### Not done (product yellow)

- Manual tray-menu Quit click-through on every new build when UI Automation cannot see the Win11 tray (document trigger + PIDs gone). Product also accepts `gen-audio.exe --quit` on a second start, which calls the same `request_quit` path as tray Quit (stop sidecar + `app.exit(0)`).
- Fresh NSIS + MSI from `scripts/build-tauri-windows.ps1` after each dirty UI/tray change when Optimus re-stamps A.
