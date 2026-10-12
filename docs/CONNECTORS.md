# Connectors

Seven surfaces are registered with `gen_audio_connectors::roster`, the MCP `list_connectors` tool, and the desktop ConnectorStatus cards.

| Id | Mode without secrets | What "live" means |
| --- | --- | --- |
| `mcp` | `local` | This process. No vendor call. |
| `acp` | `local` | This process. No vendor call. |
| `harness` | `local` | This process. No vendor call. |
| `copilot` | `mock` | HTTPS call to GitHub Models, or to `COPILOT_STUDIO_ENDPOINT` if set. |
| `claude` | `mock` | HTTPS call to the Anthropic Messages API, or the Claude Code CLI when explicitly enabled. |
| `gpt` | `mock` | HTTPS call to the OpenAI chat completions URL. |
| `gemini` | `mock` | HTTPS call to Gemini `generateContent`. |

`complete` on a vendor connector returns text that says it is **not model output** whenever it did not perform the HTTP call. A credential alone does not send the prompt. Set `GEN_AUDIO_CONNECTOR_LIVE=1` as well.

## Environment

Never commit these. `.env` files are gitignored.

| Variable | Connector | Role |
| --- | --- | --- |
| `GEN_AUDIO_CONNECTOR_LIVE` | vendor connectors | Must be `1` before any vendor HTTP call. |
| `GITHUB_TOKEN`, `GH_TOKEN`, or `COPILOT_GITHUB_TOKEN` | copilot | Bearer token for GitHub Models. |
| `COPILOT_GITHUB_MODEL` | copilot | Model id. Default `gpt-4o-mini`. |
| `COPILOT_STUDIO_ENDPOINT` | copilot | Optional https URL you operate. Receives `{"prompt","source"}`. |
| `ANTHROPIC_API_KEY` | claude | Messages API key. |
| `ANTHROPIC_MODEL` | claude | Default `claude-3-5-haiku-latest`. |
| `CLAUDE_CODE_BIN` | claude | Optional path to the `claude` binary. |
| `GEN_AUDIO_CLAUDE_CODE_CLI` | claude | Must be `1` before that binary is spawned. |
| `OPENAI_API_KEY` | gpt | Chat completions key. |
| `OPENAI_MODEL` | gpt | Default `gpt-4o-mini`. |
| `OPENAI_BASE_URL` | gpt | Default `https://api.openai.com/v1`. http is allowed only when the host is exactly `127.0.0.1` or `localhost`. https endpoints must resolve to a public address unless `GEN_AUDIO_CONNECTOR_ALLOW_PRIVATE=1`. |
| `GEMINI_API_KEY` or `GOOGLE_API_KEY` | gemini | Sent as `x-goog-api-key`. |
| `GEMINI_MODEL` | gemini | Default `gemini-2.0-flash`. |

Config shapes live in `schemas/connectors/`.

## Smoke without secrets

```bash
cargo test -p gen-audio-connectors
cargo run -p gen-audio-mcp -- --smoke
cargo run -p gen-audio-acp -- --smoke
cargo run -p gen-audio-harness -- --fixture
```

The MCP smoke writes a fixture tone. The harness trace includes one `connector_health` line per id and a `tool_plan` line with `synthesized: false`.

## MCP tools

`synth`, `improve`, `spectrogram`, `cube_revision`, `serve_health`, `connector_health`, `list_connectors`, `list_engines`, `fixture_tone`, `benchmark_reference`, `harness_plan`.

`serve_health` builds `http://<host>:8002/genaid-audio/health`. An unprobed row sets `healthy` and `probed` to false and does not set `ok`, so `isError` stays false without claiming the node is up. A probe sets `healthy` only when the JSON body has `"service": "genaid-audio"`. `GET /health` on the MCP port is the MCP process, not the node speech route. That body says `"service": "gen-audio-mcp"`. `initialize` accepts protocol versions `2024-11-05` and `2025-03-26` and stores no session. The harness `cast_resolved` event reads `voices/cast_map.example.json` and sets `synthesized` to false. ACP `session/new` requires `cwd` and does not open that directory.

Synth from MCP reads `GEN_AUDIO_KOKORO_MODEL` and `GEN_AUDIO_KOKORO_VOICES` from the environment, and both files must sit under `GEN_AUDIO_MODEL_DIR`. `script` and `castMap` may only name files under `examples/` or `voices/`. The Claude Code prompt is written to the CLI stdin, not argv. `CLAUDE_CODE_BIN` must be a file named `claude`. The desktop capability set allows the three app commands and does not grant shell or filesystem plugins. `run_fixture_improve` returns status fields only, not Python stdout.

### Verified synthesis contract

`synth` requires exactly one of `script` (an allowlisted repository path) or
`scriptText` (UTF-8 content, at most 256 KiB), and exactly one of `castMap`
or the pair `agents` + `voice`. There is no sample-script or sample-cast fallback.
`agents` are ordered Speaker 1, Speaker 2, etc.; single-persona prose is wrapped
as Speaker 1, while multiple personas require explicit Speaker turns.
Only explicit catalog Kokoro pack mappings are accepted (currently Alice and
Frank). Unmapped personas and unavailable engines are rejected, not substituted.
`personaplex` and `vibevoice` have routing extension points but no adapter here.

Example request arguments:

```json
{"scriptText":"Only these supplied words.","agents":["alice"],"voice":"kokoro_onnx"}
```

`output` is an optional unique scratch WAV filename. Omission generates a name;
existing or reserved WAV/manifest names cannot be overwritten. Scratch inputs are
bounded, create-new private files, removed when the plan is dropped. Python receives
`--json --improve` and must return a final typed version `1.0` receipt. The bridge
checks the exact planned paths, PCM16 mono 24 kHz WAV structure, nonempty signal,
byte hash, sample count and duration, matching manifest metadata, and every
requested turn's text hash, speaker and cast. Only then does MCP issue the WAV
and manifest as readable scratch artifacts and return `synthesizedSpeech:true`.
The response contains relative `wav`, `output`, `manifest` names plus `sample_rate`,
`samples`, `duration_s`, `sha256`, and `turns`; it does not expose Python stdout.

Python and Claude CLI use the same shell-free runner: 120-second deadline,
256 KiB stdout/stdin and 64 KiB stderr limits, at most four concurrent children,
stdin EOF, process-tree termination and reaping. Python does not receive vendor
credentials or `GEN_AUDIO_ARTIFACT_DIR`. A deadline, malformed receipt, failed
process or mismatched artifact is an error, never speech success.

ACP keeps a dedicated MCP server and scratch per session until cancellation or
agent exit (32 sessions maximum). `generate` followed by a newline and the supplied
script renders that content with the session's personas/voice. A text block can
also contain `{"name":"synth","arguments":{...}}` or another MCP tool request,
allowing subsequent processing of issued files within the same session. Tool
failures produce failed statuses; summaries reflect actual verified speech.
Stdio requests and identifiers are bounded. ACP cancellation removes the session;
the sequential stdio transport cannot interrupt a tool already running, but the
runner's hard deadline still applies.

Vendor HTTP responses are capped at 128 KiB with a 30-second total timeout and
no redirects. GitHub/OpenAI use actual bearer headers; Anthropic and Gemini use
their API-key headers. Completion extraction uses OpenAI
`choices[0].message.content`, Anthropic text blocks, or Gemini non-thought text
parts. A custom Studio endpoint must return `{"text":"..."}`. Empty/malformed
responses fail safely without echoing headers, URLs, or response bodies. Tests
use loopback mock HTTP servers only; no live API requests or credentials.
