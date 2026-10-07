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
| `OPENAI_BASE_URL` | gpt | Default `https://api.openai.com/v1`. http is allowed only for `127.0.0.1` and `localhost`. |
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

`serve_health` builds `http://<host>:8002/genaid-audio/health`. It does not report the node healthy unless `probe` is true and the JSON body has `"service": "genaid-audio"`. `GET /health` on the MCP port is the MCP process, not the Ray Serve app. That body says `"service": "gen-audio-mcp"`.

Synth from MCP reads `GEN_AUDIO_KOKORO_MODEL` and `GEN_AUDIO_KOKORO_VOICES` from the environment, and both files must sit under `GEN_AUDIO_MODEL_DIR`. Tool arguments cannot pass an arbitrary filesystem path.
