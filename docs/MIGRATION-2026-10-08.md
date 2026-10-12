# Gen.AID.Audio → gen-audio migration

The desktop routes supplied VibeVoice scripts through the same isolated,
pinned Python 3.12 worker as MCP. It normalizes numbered speaker turns,
validates up to four selected personas, prepares declared acoustic references,
checks runtime/checkpoint/tokenizer/memory readiness, and independently verifies
the worker's WAV and generation manifest. It never substitutes Kokoro audio
for VibeVoice. Output limits are upper bounds; capped audio remains explicitly
incomplete and its transcript is not verified.

`voices/manifest.json` is the source of truth for profile version 2.0 engine
bindings: Kokoro ONNX, dayour Kokoro, Misaki G2P, VibeVoice, PersonaPlex,
Magpie and Pocket TTS. A binding is separate from installed/runtime readiness.
Alice and Frank have explicit Kokoro pack and legacy acoustic-reference
assignments. Other personas require an explicit acoustic reference selection.
Nine references were migrated from the old managed library, normalized to mono
24 kHz PCM16, with original and normalized hashes. Legacy license and consent
metadata are preserved as reported; consent is not invented.

The profile flipcards expose every engine binding and download the whole
profile as JSON. The public catalog includes all nine profiles and the prior
Bitdot Braille VibeVoice WAV/cube as an imported render. Its older runtime
provenance is retained and does not validate the current worker.

Successful backend-verified generation saves WAV, manifest and a validated ga1
audio asset under `%LOCALAPPDATA%/Gen-Audio/library`. The desktop can refresh,
verify, reopen, play and download saved generations. Names are immutable;
existing differing bytes cannot be overwritten.

Installation settings live in `%LOCALAPPDATA%/Gen-Audio/runtime.json`.
Process environment overrides them. `GEN_AUDIO_REPO`, `GEN_AUDIO_PYTHON`,
`GEN_AUDIO_MODEL_DIR`, `GEN_AUDIO_KOKORO_MODEL`, `GEN_AUDIO_KOKORO_VOICES`,
`GEN_AUDIO_VIBEVOICE_PYTHON`, `GEN_AUDIO_VIBEVOICE_MODEL_DIR` and
`GEN_AUDIO_VIBEVOICE_TOKENIZER` point to local assets/runtimes; RPC callers
cannot change those filesystem paths. The base SDK is Python 3.14 and the
model worker remains separately pinned to Python 3.12.

Connect an MCP client to the installed app with:

```text
D:/gen-audio/.venv-app/Scripts/python.exe D:/gen-audio/scripts/connect-desktop-mcp.py
```

The bridge requests an owner-only transient connection file through the native
single-instance command and forwards MCP JSON-RPC to the owned loopback server.
It does not print bearer credentials or place them in URLs.

The machine has 128 GiB of physical unified memory installed (eight 16 GiB
modules). Windows currently exposes only 31.65 GiB as CPU-visible physical
memory; both WMI and `GlobalMemoryStatusEx` agree. Real VibeVoice preflight
estimated 22.53 GiB plus a 2 GiB reserve, with about 6.6–10 GiB free during
validation, and refused to load weights. Installed unified capacity is not
the available memory of this pinned CPU runtime. This resource guard is
retained. PersonaPlex has a separate
speech-to-speech contract and is not a script-TTS replacement.

The optional `master_audio` MCP action creates a separate FFmpeg two-pass
podcast master, with high/low-pass conditioning, compression, -16 LUFS and
-1.5 dBTP targets, and an independent final measurement. A real installed
Kokoro render was mastered and its WAV digest and source digest verified;
this is audio processing, not new synthesized speech or transcript validation.

Production validation built both NSIS and MSI packages after `cargo clean`
removed 9.0 GiB. The NSIS package was installed and launched; its owned
sidecar passed authenticated MCP and stdio bridge checks. A real two-speaker
Kokoro render was saved, downloaded, reopened, then reopened again after
restarting the installed app using its saved runtime configuration. The
built interface consumed MCP persona/voice/flipcard controls, displayed the
real VibeVoice memory refusal, and downloaded the generation manifest.
Native screenshot inspection was unavailable because the Computer Use pipe
was missing; interface visual checks used Edge with the actual owned sidecar.

The 1.0.1 source and production bundle include script analysis, a local draft
preview, and provider-backed drafting gated by confirmation of the exact
previewed prompt. No provider request was made during validation. The installed
0.1.0 app predates these tools; the 1.0.1 installer has been built but not installed.

Remaining migration work includes persistent warm model ownership, incremental
audio streaming/cancellation, PersonaPlex desktop conversation controls and real
inference validation, and compatibility endpoints for old HTTP clients.
