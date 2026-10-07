# Review bug bash

Serial review of the Tauri app, card viewport, and the seven connectors. Each round argues the design is wrong, applies a fix, then records what still looks fragile. Rounds are appended. Earlier rounds are not rewritten.

## Round 1 — Security, supply chain, secret leakage, untrusted tool args

### Inverse debate

The strongest case that the first connector design was wrong:

The scratch directory and the repository were treated as one trust zone. `synth` took `script` and `castMap` as repo-relative strings and opened whatever file canonicalized inside the checkout, so a tool call could read `README.md`, CI config, or a `.env` someone had dropped in the tree. `improve` would open any `.wav`, `.json`, or `.png` already sitting in `GEN_AUDIO_WORK_DIR`, including a symlink planted there. Vendor URL checks used a prefix (`http://127.0.0.1` matches `http://127.0.0.1.evil.com`). `serve_health` with `probe=true` would GET any host the caller named, which is SSRF once the HTTP transport is reachable. HTTP 500 bodies were built by interpolating the error string into JSON. The Claude prompt was an argv element, so it showed up in the process list, and `CLAUDE_CODE_BIN` could be any executable. The Tauri capability was `core:default`, which includes path and window plugins the board does not need. Python children inherited vendor API keys they never use. Tool `inputSchema` was `additionalProperties: true`.

That is the wrong abstraction: “relative path inside a root” is not an authorization policy.

### Steelman and what changed

Authorization is an allowlist plus a process-issued file set.

- User-supplied repo reads are only `examples/` and `voices/`, with no hidden components, a 256 KiB cap, and `..` rejected. Python entry points are the five known `scripts/*.py` files.
- `Scratch` records files this process created. Pre-existing names in the directory stay unreadable. Symlinks are refused. The directory cannot be the repo, a parent of the repo, the home directory, or the temp root. Fresh directories are mode `0700` on Unix.
- `serve_health` probes only loopback, `10.1.8.70`, or `GEN_AUDIO_PROBE_HOSTS`. The response keeps `service` and `status`, not the raw body.
- Vendor endpoints are parsed. HTTP hosts must be exactly `127.0.0.1` or `localhost`. HTTPS userinfo, query strings, and control characters are rejected. Non-loopback HTTPS must resolve to a public address unless `GEN_AUDIO_CONNECTOR_ALLOW_PRIVATE=1`. Model ids cannot contain `/`.
- Header values cannot contain control characters. Redaction covers more token prefixes (`ghu_`, `ghs_`, `glpat-`, `xox*`, `ya29.`).
- Python CLIs are spawned only when `GEN_AUDIO_PYTHON` is named `python`, `python3`, or `py`, and vendor keys are removed from that child’s environment.
- Claude Code takes the prompt on stdin. The binary’s file name must be `claude`. Other vendor keys are removed from its environment. `ANTHROPIC_API_KEY` stays, because the CLI reads it from the environment.
- MCP tool arguments reject unknown keys. Schemas set `additionalProperties: false`. Stdio lines and HTTP bodies are capped at 1 MiB. HTTP errors are `serde_json` values. `Content-Length` is required for POST. A non-loopback bind uses a fresh scratch per connection and deletes it after the response.
- The harness `--script-file` flag uses the same repo allowlist. Traces no longer print the scratch path.
- The desktop capability allows only `connector_statuses`, `viewport_example`, and `run_fixture_improve`. `run_fixture_improve` returns `ok`, `code`, `synthesizedSpeech`, and `fixture`, not Python stdout.

### Adversarial findings

| Finding | Result |
| --- | --- |
| `synth` `script=README.md` reads the repo | Rejected. Message says tool arguments may only read `examples/` or `voices/`. Covered by `untrusted_tool_args_cannot_read_arbitrary_files`. |
| `serve_health` probe of `169.254.169.254` | Rejected as not allowlisted. |
| Unexpected `token` argument next to a probe | Rejected before the probe runs. |
| `http://127.0.0.1.evil.com` and `https://user:token@host` | Rejected by the endpoint parser. |
| Pre-existing `secret.json` in the scratch | `open_input` refuses it. |
| `core:default` path/window surface | Removed. `cargo check -p gen-audio-desktop` passes with the three command permissions. |

### Reflection

Assumed that “canonical path is inside the repo” was enough, and that a shared work directory was only a convenience for chaining `fixture_tone` into `improve`. Both broke as soon as the caller was treated as untrusted. The issued-file set is the replacement: chaining still works on loopback because `fixture_tone` issues the names it writes, and it is not an MCP session id.

Residual risk: DNS is checked once and `ureq` resolves again, so a rebinding name can still move. `adopt_new_files` trusts new names that appear in the scratch after startup; that is safe only while the directory stays mode `0700`. Loopback HTTP still shares one scratch among local clients. Prefix redaction misses tokens that do not match the known shapes. `probe=false` still returns a URL for any syntactically valid host, and the shared gateway remains probeable on purpose. `GEN_AUDIO_MCP_HTTP_ALLOW_REMOTE=1`, `GEN_AUDIO_PROBE_HOSTS`, and `GEN_AUDIO_CONNECTOR_ALLOW_PRIVATE=1` are operator overrides that widen the network. The Claude child still sees `ANTHROPIC_API_KEY`. There is no signature check on the Python interpreter or on kokoro files under `GEN_AUDIO_MODEL_DIR`.

Next round should attack the board itself: schema versus the TypeScript checker, empty state, keyboard focus, Windows paths, and what the webview is allowed to learn from IPC.
