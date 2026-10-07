//! Stateless Model Context Protocol server.
//!
//! `initialize` stores nothing. `tools/call` does not read a session map.
//! HTTP responses do not set `Mcp-Session-Id`. Stdio is newline-delimited JSON-RPC.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use gen_audio_connectors::health_all;
use gen_audio_core::benchmark::{reference_rows, SOURCE_NOTE};
use gen_audio_core::bridge::{self, PythonTool};
use gen_audio_core::engines;
use gen_audio_core::fixture::{self, write_fixture_tone};
use gen_audio_core::paths::{self, find_repo_root, Scratch};
use gen_audio_core::redact::redact_secrets;
use gen_audio_core::serve::{self, health_url, node_base_url, ready_url};
use serde_json::{json, Value};

pub struct Server {
    pub scratch: Scratch,
    pub repo: Option<PathBuf>,
}

impl Server {
    pub fn boot() -> Self {
        let scratch = if let Ok(raw) = std::env::var("GEN_AUDIO_WORK_DIR") {
            Scratch::new(PathBuf::from(raw)).expect("GEN_AUDIO_WORK_DIR must be a dedicated directory outside the repository")
        } else {
            Scratch::create().expect("work directory")
        };
        Self {
            scratch,
            repo: find_repo_root(),
        }
    }

    /// Fresh scratch for one remote HTTP request. Not shared with other clients.
    pub fn isolated() -> Self {
        Self {
            scratch: Scratch::create().expect("isolated work directory"),
            repo: find_repo_root(),
        }
    }
}

pub fn smoke() -> Result<String, String> {
    let server = Server::boot();
    let init = handle(&server, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))?
        .ok_or("initialize did not answer")?;
    if init.get("id").is_none() {
        return Err("initialize did not answer".into());
    }
    let listed = handle(
        &server,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )?
    .ok_or("tools/list did not answer")?;
    let tools = listed["result"]["tools"]
        .as_array()
        .ok_or("tools/list missing tools")?;
    let names: Vec<_> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for required in [
        "synth",
        "improve",
        "spectrogram",
        "cube_revision",
        "serve_health",
        "connector_health",
        "fixture_tone",
        "list_connectors",
        "ui_navigate",
        "ui_select_tile",
        "ui_playback",
        "ui_set_sidepane",
        "ui_generate",
        "library_list",
        "library_rename",
        "voice_profile_get",
        "voice_profile_list",
    ] {
        if !names.contains(&required) {
            return Err(format!("missing tool {required}"));
        }
    }
    let tone = handle(
        &server,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fixture_tone","arguments":{}}}),
    )?
    .ok_or("fixture tool did not answer")?;
    let text = tone["result"]["content"][0]["text"]
        .as_str()
        .ok_or("fixture tool returned no text")?;
    if !text.contains("notPodcast") {
        return Err("fixture tool did not label the tone".into());
    }
    Ok(format!("mcp smoke ok ({} tools)", names.len()))
}

pub fn handle(server: &Server, message: Value) -> Result<Option<Value>, String> {
    let obj = message.as_object().ok_or("request must be an object")?;
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Ok(Some(error_response(obj.get("id").cloned(), -32600, "jsonrpc must be 2.0")));
    }
    let id = obj.get("id").cloned();
    let notification = match &id {
        None | Some(Value::Null) => true,
        _ => false,
    };
    let method = obj.get("method").and_then(Value::as_str).unwrap_or("");
    let params = obj.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => initialize_result(&params),
        "notifications/initialized" | "initialized" => {
            return Ok(None);
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_defs()})),
        "tools/call" => call_tool(server, &params),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    if notification {
        return Ok(None);
    }
    match result {
        Ok(value) => Ok(Some(json!({"jsonrpc":"2.0","id": id, "result": value}))),
        Err((code, message)) => Ok(Some(error_response(id, code, &message))),
    }
}

fn initialize_result(params: &Value) -> Result<Value, (i32, String)> {
    const SUPPORTED: &[&str] = &["2024-11-05", "2025-03-26"];
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");
    if !SUPPORTED.contains(&requested) {
        return Err((-32602, format!("unsupported protocolVersion {requested}")));
    }
    Ok(json!({
        "protocolVersion": requested,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "gen-audio", "version": "0.1.0"},
        "instructions": "Stateless Gen-Audio MCP. initialize stores no session. UI tools queue a loopback command and do not write speech. Agent ids are personas (anton, alice, khortana, rocky, and the rest of the catalog), not connector ids. Voice ids are TTS models (kokoro_onnx, kokoro_dayour, misaki, vibevoice, magpie, pocket_tts). af_heart is a Kokoro pack ref, not a Voice id. GET /health is liveness. GET /ready means this listener is up, not that a model is loaded. GET /control/stream is a short stateless SSE snapshot."
    }))
}

fn tool_defs() -> Vec<Value> {
    vec![
        tool("synth", "Plan or run kokoro-onnx synthesis. Without model env vars this returns a refusal and writes no speech."),
        tool("improve", "Run the Python publish chain on a WAV already in the work directory."),
        tool("spectrogram", "Run the Python spectrogram helper on work-directory WAVs."),
        tool("cube_revision", "Run the Python inverse-HDR cube revision sketch on a work-directory WAV."),
        tool("serve_health", "Build a per-node genaid-audio health URL. Optional TCP/HTTP probe is off unless probe=true."),
        tool("connector_health", "Health for one connector id or the full roster of seven."),
        tool("list_connectors", "List connector ids and modes."),
        tool("list_engines", "List compare-list engines and whether this repo can synthesize with them."),
        tool("fixture_tone", "Write a labeled sine WAV. Not speech and not a podcast."),
        tool("benchmark_reference", "Return 2026-10-06 compare figures. measuredHere is false."),
        tool("harness_plan", "Parse the sample script and return a harness-style plan. Does not synthesize."),
        tool("ui_navigate", "Queue a viewport slide change for the local Gen-Audio window. Does not render audio."),
        tool("ui_select_tile", "Queue selection of a card id. Loads a 3D cube only when that tile has cube JSON."),
        tool("ui_playback", "Queue play, pause, or seek for a library tile. Does not open the WAV in this process."),
        tool("ui_set_sidepane", "Queue Agent personas (max 8) and a Voice TTS model. Connector ids are rejected."),
        tool("ui_generate", "Record a generation request. synthesizedSpeech is false. Does not call synth."),
        tool("library_list", "List the library catalog. Does not open WAV bytes. Unavailable clips stay unavailable."),
        tool("library_rename", "Queue a semantic name and/or face name. Does not rewrite the WAV."),
        tool("voice_profile_get", "Return one persona profile: tone, purpose, domain, accent, traits, refs. Not audio."),
        tool("voice_profile_list", "List personas and TTS voice models. LLM ids are connectors, not agents."),
    ]
}

fn tool(name: &str, description: &str) -> Value {
    let (properties, required) = match name {
        "synth" => (json!({"script": {"type": "string"}, "castMap": {"type": "string"}, "output": {"type": "string"}}), json!(["output"])),
        "improve" => (json!({"input": {"type": "string"}, "output": {"type": "string"}}), json!(["input", "output"])),
        "cube_revision" => (json!({"input": {"type": "string"}, "output": {"type": "string"}, "maxSteps": {"type": "integer"}}), json!(["input", "output"])),
        "spectrogram" => (json!({"before": {"type": "string"}, "after": {"type": "string"}}), json!(["before"])),
        "serve_health" => (json!({"host": {"type": "string"}, "port": {"type": "integer"}, "probe": {"type": "boolean"}}), json!([])),
        "connector_health" => (json!({"id": {"type": "string"}}), json!([])),
        "ui_navigate" => (json!({"slide": {"type": "string"}, "tileId": {"type": "string"}}), json!(["slide"])),
        "ui_select_tile" => (json!({"tileId": {"type": "string"}}), json!(["tileId"])),
        "ui_playback" => (json!({"tileId": {"type": "string"}, "action": {"type": "string"}, "seconds": {"type": "number"}}), json!(["tileId", "action"])),
        "ui_set_sidepane" | "ui_generate" => (
            json!({"agents": {"type": "array"}, "voice": {"type": "string"}, "durationMin": {"type": "integer"}, "promptNote": {"type": "string"}}),
            json!(["agents", "voice"]),
        ),
        "library_rename" => (json!({"clipId": {"type": "string"}, "semanticName": {"type": "string"}, "faceName": {"type": "string"}}), json!(["clipId"])),
        "voice_profile_get" => (json!({"personaId": {"type": "string"}, "agentName": {"type": "string"}}), json!(["personaId"])),
        _ => (json!({}), json!([])),
    };
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "properties": properties,
            "required": required
        }
    })
}

pub fn call_tool(server: &Server, params: &Value) -> Result<Value, (i32, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call needs a name".into()))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    known_arguments(name, &args)?;
    let payload = match name {
        "synth" => synth(server, &args)?,
        "improve" => python_tool(server, PythonTool::Improve, &args)?,
        "spectrogram" => python_tool(server, PythonTool::Spectrogram, &args)?,
        "cube_revision" => python_tool(server, PythonTool::Cube, &args)?,
        "serve_health" => serve_health(&args)?,
        "connector_health" => connector_health(&args)?,
        "list_connectors" => json!({"connectors": health_all()}),
        "list_engines" => json!({"liveSynth": false, "weightsBundled": false, "engines": engines::engines()}),
        "fixture_tone" => fixture(server)?,
        "benchmark_reference" => json!({"measuredHere": false, "sourceNote": SOURCE_NOTE, "rows": reference_rows()}),
        "harness_plan" => harness_plan(server)?,
        "ui_navigate" => control::ui_navigate(&args)?,
        "ui_select_tile" => control::ui_select_tile(&args)?,
        "ui_playback" => control::ui_playback(&args)?,
        "ui_set_sidepane" => control::ui_set_sidepane(&args)?,
        "ui_generate" => control::ui_generate(&args)?,
        "library_list" => control::library_list(),
        "library_rename" => control::library_rename(&args)?,
        "voice_profile_get" => control::voice_profile_get(&args)?,
        "voice_profile_list" => control::voice_profile_list(),
        _ => return Err((-32602, format!("unknown tool {name}"))),
    };
    Ok(json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "isError": payload.get("ok").and_then(Value::as_bool) == Some(false)
    }))
}

fn known_arguments(name: &str, args: &Value) -> Result<(), (i32, String)> {
    let allowed: &[&str] = match name {
        "synth" => &["script", "castMap", "output"],
        "improve" => &["input", "output"],
        "spectrogram" => &["before", "after"],
        "cube_revision" => &["input", "output", "maxSteps"],
        "serve_health" => &["host", "port", "probe"],
        "connector_health" => &["id"],
        "list_connectors" | "list_engines" | "fixture_tone" | "benchmark_reference" | "harness_plan" | "library_list" | "voice_profile_list" => &[],
        "ui_navigate" => &["slide", "tileId"],
        "ui_select_tile" => &["tileId"],
        "ui_playback" => &["tileId", "action", "seconds"],
        "ui_set_sidepane" | "ui_generate" => &["agents", "voice", "durationMin", "promptNote"],
        "library_rename" => &["clipId", "semanticName", "faceName"],
        "voice_profile_get" => &["personaId", "agentName"],
        _ => &[],
    };
    let obj = args
        .as_object()
        .ok_or((-32602, "arguments must be an object".to_string()))?;
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err((-32602, format!("unexpected argument {key}")));
        }
    }
    Ok(())
}

fn synth(server: &Server, args: &Value) -> Result<Value, (i32, String)> {
    match server.repo.as_deref() {
        Some(repo) => match bridge::plan(PythonTool::Synth, repo, &server.scratch, args) {
            Err(message) if message.contains("No speech was invented") => Ok(json!({
                "ok": false,
                "synthesized": false,
                "reason": message,
                "mock": true
            })),
            Err(message) => Err((-32602, message)),
            Ok(plan) => bridge::run_plan(&plan).map_err(|message| (-32603, message)),
        },
        None => Ok(json!({
            "ok": false,
            "synthesized": false,
            "reason": "repository root not found; no speech was invented",
            "mock": true
        })),
    }
}

fn python_tool(server: &Server, tool: PythonTool, args: &Value) -> Result<Value, (i32, String)> {
    let repo = server
        .repo
        .as_ref()
        .ok_or((-32603, "repository root not found".to_string()))?;
    let plan = bridge::plan(tool, repo, &server.scratch, args).map_err(|message| (-32602, message))?;
    let ran = bridge::run_plan(&plan).map_err(|message| (-32603, message))?;
    let _ = server.scratch.adopt_new_files();
    Ok(ran)
}

fn fixture(server: &Server) -> Result<Value, (i32, String)> {
    let tone = write_fixture_tone(&server.scratch).map_err(|err| (-32603, err))?;
    let meta = std::fs::read_to_string(&tone.sidecar).map_err(|err| (-32603, err.to_string()))?;
    let meta: Value = serde_json::from_str(&meta).map_err(|err| (-32603, err.to_string()))?;
    Ok(json!({
        "ok": true,
        "wav": fixture::FIXTURE_WAV_NAME,
        "sidecar": meta
    }))
}

fn serve_health(args: &Value) -> Result<Value, (i32, String)> {
    let host = args.get("host").and_then(Value::as_str).unwrap_or("<node>");
    let port = args.get("port").and_then(Value::as_u64).unwrap_or(8002) as u16;
    if host == "<node>" {
        return Ok(json!({
            "healthy": false,
            "probed": false,
            "baseUrl": "http://<node>:8002/genaid-audio",
            "healthUrl": "http://<node>:8002/genaid-audio/health",
            "readyUrl": "http://<node>:8002/genaid-audio/ready",
            "expectedBody": serve::health_payload(),
            "note": "Placeholder host. This is not a live genaid-audio probe. Pass a real host to build a row. probe=true performs an HTTP GET."
        }));
    }
    let base = node_base_url(host, port).map_err(|err| (-32602, err))?;
    let health = health_url(host, port).map_err(|err| (-32602, err))?;
    let ready = ready_url(host, port).map_err(|err| (-32602, err))?;
    let probe = args.get("probe").and_then(Value::as_bool).unwrap_or(false);
    if probe {
        serve::probe_host_allowed(host).map_err(|err| (-32602, err))?;
    }
    if !probe {
        return Ok(json!({
            "healthy": false,
            "probed": false,
            "baseUrl": base,
            "healthUrl": health,
            "readyUrl": ready,
            "expectedBody": serve::health_payload(),
            "note": "URL only. /health is liveness. /ready is readiness. This is not a live genaid-audio probe."
        }));
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(2))
        .timeout_read(std::time::Duration::from_secs(2))
        .redirects(0)
        .build();
    match agent.get(&health).call() {
        Ok(response) => {
            let body = response.into_string().unwrap_or_default();
            let clipped: String = body.chars().take(512).collect();
            let parsed: Value = serde_json::from_str(&clipped).unwrap_or(Value::Null);
            let service = parsed.get("service").and_then(Value::as_str);
            let status = parsed.get("status").and_then(Value::as_str);
            let matches = service == Some(serve::SERVICE_NAME);
            Ok(json!({
                "ok": matches,
                "healthy": matches,
                "probed": true,
                "baseUrl": base,
                "healthUrl": health,
                "readyUrl": ready,
                "service": service,
                "status": status
            }))
        }
        Err(err) => Ok(json!({
            "ok": false,
            "healthy": false,
            "probed": true,
            "baseUrl": base,
            "healthUrl": health,
            "error": redact_secrets(&err.to_string())
        })),
    }
}

fn connector_health(args: &Value) -> Result<Value, (i32, String)> {
    match args.get("id").and_then(Value::as_str) {
        Some(id) => {
            let report = gen_audio_connectors::health_one(id).map_err(|err| (-32602, err.to_string()))?;
            Ok(json!(report))
        }
        None => Ok(json!({"connectors": health_all()})),
    }
}

fn harness_plan(server: &Server) -> Result<Value, (i32, String)> {
    let repo = server
        .repo
        .as_ref()
        .ok_or((-32603, "repository root not found".to_string()))?;
    let text = std::fs::read_to_string(
        paths::read_user_repo_file(repo, "examples/podcast_script_sample.txt").map_err(|err| (-32603, err))?,
    )
    .map_err(|err| (-32603, err.to_string()))?;
    let turns = gen_audio_core::script::parse_script(&text).map_err(|err| (-32602, err))?;
    Ok(json!({
        "ok": true,
        "synthesized": false,
        "turns": turns.len(),
        "speakers": turns.iter().map(|turn| &turn.speaker_id).collect::<Vec<_>>(),
        "next": "fixture_tone then improve. synth stays refused until model files are configured."
    }))
}

fn error_response(id: Option<Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {"code": code, "message": redact_secrets(message)}
    })
}

pub fn stdio_loop(server: &Server) {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > 1024 * 1024 {
            let _ = writeln!(stdout, "{}", error_response(None, -32600, "request larger than 1 MiB"));
            let _ = stdout.flush();
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                let _ = writeln!(stdout, "{}", error_response(None, -32700, "parse error"));
                let _ = stdout.flush();
                continue;
            }
        };
        match handle(server, message) {
            Ok(Some(response)) => {
                let _ = writeln!(stdout, "{response}");
                let _ = stdout.flush();
            }
            Ok(None) => {}
            Err(err) => {
                let _ = writeln!(stdout, "{}", error_response(None, -32603, &err));
                let _ = stdout.flush();
            }
        }
    }
}

pub mod control;
pub mod http;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_do_not_require_a_stored_session() {
        let server = Server::boot();
        let first = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_connectors","arguments":{}}}),
        )
        .unwrap()
        .unwrap();
        assert!(first["result"]["content"][0]["text"].as_str().unwrap().contains("mcp"));
        let second = handle(
            &server,
            json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"fixture_tone","arguments":{}}}),
        )
        .unwrap()
        .unwrap();
        assert!(second["result"]["content"][0]["text"].as_str().unwrap().contains("notPodcast"));
        let note = handle(
            &server,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .unwrap();
        assert!(note.is_none());
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn untrusted_tool_args_cannot_read_arbitrary_files() {
        let server = Server::boot();
        let readme = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"synth","arguments":{"output":"x.wav","script":"README.md"}}}),
        )
        .unwrap()
        .unwrap();
        let message = readme["error"]["message"].as_str().unwrap_or("");
        assert!(message.contains("examples/"), "{message}");
        let extra = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"serve_health","arguments":{"host":"169.254.169.254","probe":true,"token":"sk-supersecret"}}}),
        )
        .unwrap()
        .unwrap();
        assert!(extra["error"]["message"].as_str().unwrap_or("").contains("unexpected argument"));
        let probe = handle(
            &server,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"serve_health","arguments":{"host":"169.254.169.254","probe":true}}}),
        )
        .unwrap()
        .unwrap();
        assert!(probe["error"]["message"].as_str().unwrap_or("").contains("allowlist"));
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn unprobed_serve_url_is_not_healthy_and_protocol_is_negotiated() {
        let server = Server::boot();
        let health = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"serve_health","arguments":{}}}),
        )
        .unwrap()
        .unwrap();
        let text = health["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"healthy\":false"), "{text}");
        assert!(text.contains("\"probed\":false"), "{text}");
        assert_eq!(health["result"]["isError"], false);
        let init = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        let rejected = handle(
            &server,
            json!({"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
        )
        .unwrap()
        .unwrap();
        assert!(rejected["error"]["message"].as_str().unwrap().contains("unsupported"));
        let engines = handle(
            &server,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_engines","arguments":{}}}),
        )
        .unwrap()
        .unwrap();
        let listed = engines["result"]["content"][0]["text"].as_str().unwrap();
        assert!(listed.contains("\"liveSynth\":false"), "{listed}");
        assert!(listed.contains("\"weightsBundled\":false") || listed.contains("\"weights_bundled\":false"), "{listed}");
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn sidepane_rejects_connector_ids_and_kokoro_pack_ids() {
        let server = Server::boot();
        let rejected_agent = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ui_set_sidepane","arguments":{"agents":["copilot"],"voice":"kokoro_onnx"}}}),
        )
        .unwrap()
        .unwrap();
        let agent_msg = rejected_agent["error"]["message"].as_str().unwrap_or("");
        assert!(agent_msg.contains("connector") || agent_msg.contains("persona"), "{agent_msg}");
        let rejected_voice = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ui_set_sidepane","arguments":{"agents":["alice"],"voice":"af_heart"}}}),
        )
        .unwrap()
        .unwrap();
        let voice_msg = rejected_voice["error"]["message"].as_str().unwrap_or("");
        assert!(voice_msg.contains("af_heart"), "{voice_msg}");
        let ok = handle(
            &server,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ui_set_sidepane","arguments":{"agents":["alice","rocky"],"voice":"kokoro_onnx"}}}),
        )
        .unwrap()
        .unwrap();
        let text = ok["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"synthesizedSpeech\":false"), "{text}");
        assert!(text.contains("alice"), "{text}");
        let profile = handle(
            &server,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"voice_profile_get","arguments":{"personaId":"alice"}}}),
        )
        .unwrap()
        .unwrap();
        let profile_text = profile["result"]["content"][0]["text"].as_str().unwrap();
        assert!(profile_text.contains("af_heart"), "{profile_text}");
        assert!(profile_text.contains("\"notPodcast\":true"), "{profile_text}");
        assert!(profile_text.contains("kokoro_onnx"), "{profile_text}");
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }
}
