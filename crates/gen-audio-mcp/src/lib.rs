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
        "ui_seek_report",
        "ui_set_sidepane",
        "ui_generate",
        "library_list",
        "library_rename",
        "library_harvest",
        "voice_profile_get",
        "voice_profile_list",
        "ui_flip",
        "ui_cube",
        "viewport_get",
        "cube_layers",
        "asset_resolve",
        "asset_list",
        "asset_glyph",
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
        tool("ui_flip", "Queue a flipcard. Optional personaId attaches the voice-profile payload. Does not render audio."),
        tool("ui_playback", "Queue play, pause, or seek for a library tile. A seek waits (waitMs, default 2000) for the window and returns {requested_t, landed_t, ok, reason}. Does not open the WAV in this process."),
        tool("ui_seek_report", "Desktop window only: report where a queued ui_playback seek (bus seq) landed. Records the result ui_playback returns."),
        tool("ui_cube", "Set the Cube tab mode: single, or compare (the clip's Library cube, library_r3, beside the same WAV's pipeline_r2 cube, one playback slice). compare takes tileId or uid. The desktop Compare button posts this same tool."),
        tool("viewport_get", "Read the UI state this server owns: cube_mode (single or compare) and cube_compare {tileId, clip_uid, left_method, right_method, left_cube_uid, right_cube_uid} or null."),
        tool("ui_set_sidepane", "Queue Agent personas (max 8) and a Voice TTS model. Connector ids are rejected."),
        tool("ui_generate", "Record a generation request. synthesizedSpeech is false. Does not call synth."),
        tool("library_list", "List the library catalog. Does not open WAV bytes. Unavailable clips stay unavailable."),
        tool("library_rename", "Queue a semantic name and/or face name. Does not rewrite the WAV."),
        tool("library_harvest", "Propose a semantic name and face name from the filename and sidecar counts. apply writes a clip ref onto a persona. Does not decode the WAV."),
        tool("voice_profile_get", "Return one persona profile: tone, purpose, domain, accent, traits, refs. Not audio."),
        tool("voice_profile_list", "List personas and TTS voice models. LLM ids are connectors, not agents."),
        tool("cube_layers", "Read signal, tonality, confidence, and quality summaries from an allowlisted library cube JSON. Omits point clouds and absolute paths."),
        tool("asset_resolve", "Resolve a ga1 asset uid (or a ga:<kind>: prefix of 8+ chars) from the library catalog. Media come back as /library URLs plus sha256."),
        tool("asset_list", "Page through library assets in uid order. Optional kind; limit 1 to 100 (default 50); cursor is the last uid returned."),
        tool("asset_glyph", "Braille glyph, dot pattern, kind hue class and aria label for a ga1 uid. Visual hint only; resolve by uid."),
    ]
}

static UID_PROP: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
    json!({"type": "string", "description": "ga1 asset uid, ga:<kind>:<26 base32>", "maxLength": 44})
});

fn tool(name: &str, description: &str) -> Value {
    let (properties, required) = match name {
        "synth" => (json!({"script": {"type": "string"}, "castMap": {"type": "string"}, "output": {"type": "string"}}), json!(["output"])),
        "improve" => (json!({"input": {"type": "string"}, "output": {"type": "string"}}), json!(["input", "output"])),
        "cube_revision" => (json!({"input": {"type": "string"}, "output": {"type": "string"}, "maxSteps": {"type": "integer"}}), json!(["input", "output"])),
        "spectrogram" => (json!({"before": {"type": "string"}, "after": {"type": "string"}}), json!(["before"])),
        "serve_health" => (json!({"host": {"type": "string"}, "port": {"type": "integer"}, "probe": {"type": "boolean"}}), json!([])),
        "connector_health" => (json!({"id": {"type": "string"}}), json!([])),
        "ui_navigate" => (
            json!({
                "slide": {"type": "string", "description": "slide:<slug> (e.g. slide:library). A bare slug is a deprecated alias: it resolves and the result carries a deprecation note."},
                "tileId": {"type": "string"},
                "uid": UID_PROP.clone()
            }),
            json!(["slide"]),
        ),
        "ui_select_tile" => (json!({"tileId": {"type": "string"}, "uid": UID_PROP.clone()}), json!([])),
        "ui_flip" => (json!({"tileId": {"type": "string"}, "uid": UID_PROP.clone(), "flipped": {"type": "boolean"}, "personaId": {"type": "string"}}), json!([])),
        "ui_playback" => (json!({"tileId": {"type": "string"}, "uid": UID_PROP.clone(), "action": {"type": "string"}, "seconds": {"type": "number"}, "origin": {"type": "string", "enum": ["user", "auto"], "description": "play only; default user"}, "waitMs": {"type": "integer", "minimum": 0, "maximum": 10000, "description": "seek only; default 2000"}}), json!(["action"])),
        "ui_cube" => (
            json!({
                "mode": {"type": "string", "enum": control::CUBE_MODES, "description": "single, or compare (Library formulas rev 3 beside Pipeline formulas rev 2 on the same WAV)"},
                "tileId": {"type": "string", "description": "library clip tile id, e.g. lib-misaki-kokoro; compare only needs it when no clip is in Compare yet"},
                "uid": UID_PROP.clone()
            }),
            json!(["mode"]),
        ),
        "ui_seek_report" => (
            json!({"seq": {"type": "integer", "minimum": 1}, "requested_t": {"type": "number"}, "landed_t": {"type": ["number", "null"]}, "ok": {"type": "boolean"}, "reason": {"type": "string", "maxLength": 240}}),
            json!(["seq", "requested_t", "landed_t", "ok", "reason"]),
        ),
        "asset_resolve" | "asset_glyph" => (json!({"uid": UID_PROP.clone()}), json!(["uid"])),
        "asset_list" => (json!({"kind": {"type": "string"}, "cursor": {"type": "string"}, "limit": {"type": "integer", "minimum": 1, "maximum": 100}}), json!([])),
        "ui_set_sidepane" | "ui_generate" => (
            json!({"agents": {"type": "array"}, "voice": {"type": "string"}, "durationMin": {"type": "integer"}, "promptNote": {"type": "string"}}),
            json!(["agents", "voice"]),
        ),
        "library_rename" => (json!({"clipId": {"type": "string"}, "semanticName": {"type": "string"}, "faceName": {"type": "string"}}), json!(["clipId"])),
        "library_harvest" => (json!({"clipId": {"type": "string"}, "personaId": {"type": "string"}, "apply": {"type": "boolean"}}), json!(["clipId"])),
        "cube_layers" => (json!({"clipId": {"type": "string"}}), json!(["clipId"])),
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
        "ui_flip" => control::ui_flip(&args)?,
        "ui_playback" => control::ui_playback(&args)?,
        "ui_cube" => control::ui_cube(&args)?,
        "viewport_get" => control::viewport_get(),
        "ui_seek_report" => control::ui_seek_report(&args)?,
        "ui_set_sidepane" => control::ui_set_sidepane(&args)?,
        "ui_generate" => control::ui_generate(&args)?,
        "library_list" => control::library_list(),
        "library_rename" => control::library_rename(&args)?,
        "library_harvest" => control::library_harvest(server.repo.as_deref(), &args)?,
        "cube_layers" => control::cube_layers(server.repo.as_deref(), &args)?,
        "voice_profile_get" => control::voice_profile_get(&args)?,
        "voice_profile_list" => control::voice_profile_list(),
        "asset_resolve" => assets::asset_resolve(&args)?,
        "asset_list" => assets::asset_list(&args)?,
        "asset_glyph" => assets::asset_glyph(&args)?,
        _ => return Err((-32602, format!("unknown tool {name}"))),
    };
    Ok(json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "isError": payload.get("ok").and_then(Value::as_bool) == Some(false)
    }))
}

/// Argument names each tool accepts. Must equal the tool's advertised
/// inputSchema properties (tested): the UI and agents speak one contract.
fn allowed_arguments(name: &str) -> &'static [&'static str] {
    match name {
        "synth" => &["script", "castMap", "output"],
        "improve" => &["input", "output"],
        "spectrogram" => &["before", "after"],
        "cube_revision" => &["input", "output", "maxSteps"],
        "serve_health" => &["host", "port", "probe"],
        "connector_health" => &["id"],
        "list_connectors" | "list_engines" | "fixture_tone" | "benchmark_reference" | "harness_plan" | "library_list" | "voice_profile_list" | "viewport_get" => &[],
        "ui_navigate" => &["slide", "tileId", "uid"],
        "ui_select_tile" => &["tileId", "uid"],
        "ui_flip" => &["tileId", "uid", "flipped", "personaId"],
        "ui_playback" => &["tileId", "uid", "action", "seconds", "origin", "waitMs"],
        "ui_cube" => &["mode", "tileId", "uid"],
        "ui_seek_report" => &["seq", "requested_t", "landed_t", "ok", "reason"],
        "asset_resolve" | "asset_glyph" => &["uid"],
        "asset_list" => &["kind", "cursor", "limit"],
        "ui_set_sidepane" | "ui_generate" => &["agents", "voice", "durationMin", "promptNote"],
        "library_rename" => &["clipId", "semanticName", "faceName"],
        "library_harvest" => &["clipId", "personaId", "apply"],
        "cube_layers" => &["clipId"],
        "voice_profile_get" => &["personaId", "agentName"],
        _ => &[],
    }
}

fn known_arguments(name: &str, args: &Value) -> Result<(), (i32, String)> {
    let allowed = allowed_arguments(name);
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
            Err(message) if message.contains("No speech was invented") => {
                let progress = control::note_progress("kokoro_onnx", "refused", &message, false);
                Ok(json!({
                    "ok": false,
                    "synthesized": false,
                    "synthesizedSpeech": false,
                    "phase": "refused",
                    "reason": message,
                    "mock": true,
                    "progress": progress
                }))
            }
            Err(message) => Err((-32602, message)),
            Ok(plan) => {
                let _ = control::note_progress("kokoro_onnx", "running", "kokoro-onnx synth started", false);
                let mut ran = bridge::run_plan(&plan).map_err(|message| (-32603, message))?;
                let speech = ran.get("synthesizedSpeech").and_then(Value::as_bool) == Some(true);
                let phase = if speech { "ok" } else { "refused" };
                let detail = if speech {
                    "synth reported speech"
                } else {
                    "synth finished without a speech claim"
                };
                ran["phase"] = json!(phase);
                ran["progress"] = control::note_progress("kokoro_onnx", phase, detail, speech);
                Ok(ran)
            }
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

pub mod assets;
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

    /// Rule 5 falsifier: Compare is reachable and readable over MCP. Fails if
    /// ui_cube or viewport_get is unregistered or unlisted, if the schema stops
    /// constraining mode, or if viewport_get omits cube_mode / cube_compare.
    /// The arguments are the ones the desktop Compare button posts
    /// (schemas/examples/ui_cube.contract.json, also read by the npm test).
    #[test]
    fn ui_cube_enters_and_leaves_compare_and_viewport_get_reports_it() {
        let server = Server::isolated();
        let contract: Value = serde_json::from_str(include_str!("../../../schemas/examples/ui_cube.contract.json")).unwrap();
        let rpc = |id: u64, call: &Value| handle(&server, json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":call})).unwrap().unwrap();
        let body = |response: &Value| -> Value {
            assert!(response.get("error").is_none(), "{response}");
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
        };
        let listed = handle(&server, json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap().unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        let find = |name: &str| tools.iter().find(|tool| tool["name"] == name).cloned().unwrap_or_else(|| panic!("{name} not listed"));
        let cube = find("ui_cube");
        assert_eq!(cube["inputSchema"]["properties"]["mode"]["enum"], json!(["single", "compare"]));
        assert_eq!(cube["inputSchema"]["required"], json!(["mode"]));
        assert_eq!(find("viewport_get")["inputSchema"]["properties"], json!({}));

        let entered = body(&rpc(2, &contract["enter"]["call"]));
        assert_eq!(entered["op"], "cube");
        assert_eq!(entered["args"], contract["enter"]["state"], "the queued state is the contract state");
        let seq = entered["seq"].as_u64().unwrap();
        let (_, events) = control::since(seq - 1);
        let event = events.iter().find(|event| event["seq"] == seq).expect("ui_cube queued a bus event");
        assert_eq!(event["op"], "cube");
        assert_eq!(event["args"], contract["enter"]["state"]);
        let read = body(&rpc(3, &contract["read"]["call"]));
        assert_eq!(read["cube_mode"], "compare");
        assert_eq!(read["cube_compare"], contract["enter"]["state"]["cube_compare"]);
        assert_eq!(read["cube_compare"]["left_method"], "library_r3");
        assert_eq!(read["cube_compare"]["right_method"], "pipeline_r2");
        assert_eq!(read["cube_seq"], seq);

        // compare without a clip keeps the clip already in Compare.
        let again = body(&rpc(4, &json!({"name": "ui_cube", "arguments": {"mode": "compare"}})));
        assert_eq!(again["args"], contract["enter"]["state"]);
        // A rejected call leaves the state alone.
        let refused = rpc(5, &json!({"name": "ui_cube", "arguments": {"mode": "compare", "tileId": "lib-kokoro-onnx"}}));
        assert_eq!(refused["error"]["code"], -32602);
        assert_eq!(body(&rpc(6, &contract["read"]["call"]))["cube_mode"], "compare");
        let extra = rpc(7, &json!({"name": "ui_cube", "arguments": {"mode": "single", "left": "pipeline_r2"}}));
        assert!(extra["error"]["message"].as_str().unwrap().contains("unexpected argument left"));

        let left = body(&rpc(8, &contract["exit"]["call"]));
        assert_eq!(left["args"], contract["exit"]["state"]);
        let read = body(&rpc(9, &contract["read"]["call"]));
        assert_eq!(read["cube_mode"], "single");
        assert!(read.get("cube_compare").is_some_and(Value::is_null), "single reports cube_compare: null, not a missing field");
        let no_clip = rpc(10, &json!({"name": "ui_cube", "arguments": {"mode": "compare"}}));
        assert!(no_clip["error"]["message"].as_str().unwrap().contains("needs tileId or uid"));
        // A cube uid names its clip: either cube of the pair enters Compare on misaki.
        let by_cube = body(&rpc(11, &json!({"name": "ui_cube", "arguments": {"mode": "compare", "uid": "ga:cube_ihdr:zq3x2xysrfaf4tunakpqz6vhxi"}})));
        assert_eq!(by_cube["args"], contract["enter"]["state"]);
        body(&rpc(12, &contract["exit"]["call"]));
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn harvest_attaches_a_clip_ref_without_decoding_audio() {
        // E3: hermetic. With GEN_AUDIO_KOKORO_* set, the synth call below
        // would start a real synth and the refusal asserts would not hold.
        let _env = gen_audio_core::bridge::NoKokoroEnv::new();
        let server = Server::boot();
        let harvested = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"library_harvest","arguments":{"clipId":"lib-kokoro-onnx","personaId":"alice","apply":true}}}),
        )
        .unwrap()
        .unwrap();
        let text = harvested["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"decodedAudio\":false"), "{text}");
        assert!(text.contains("\"openedWav\":false"), "{text}");
        assert!(text.contains("816 words") || text.contains("sidecar-counts") || text.contains("kokoro_onnx"), "{text}");
        assert!(!text.contains("voices-v1.0.bin"), "{text}");
        assert!(!text.contains("D:\\\\"), "{text}");
        let profile = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"voice_profile_get","arguments":{"personaId":"alice"}}}),
        )
        .unwrap()
        .unwrap();
        let profile_text = profile["result"]["content"][0]["text"].as_str().unwrap();
        assert!(profile_text.contains("clip:lib-kokoro-onnx"), "{profile_text}");
        assert!(profile_text.contains("\"notPodcast\":true"), "{profile_text}");
        let cube = handle(
            &server,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cube_layers","arguments":{"clipId":"lib-kokoro-onnx"}}}),
        )
        .unwrap()
        .unwrap();
        let cube_text = cube["result"]["content"][0]["text"].as_str().unwrap();
        assert!(cube_text.contains("signal"), "{cube_text}");
        assert!(cube_text.contains("tonality"), "{cube_text}");
        assert!(cube_text.contains("confidence"), "{cube_text}");
        assert!(cube_text.contains("quality"), "{cube_text}");
        assert!(cube_text.contains("\"absolutePathsOmitted\":true"), "{cube_text}");
        assert!(!cube_text.contains("points_preview"), "{cube_text}");
        assert!(!cube_text.contains("D:\\\\"), "{cube_text}");
        let empty = handle(
            &server,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"cube_layers","arguments":{"clipId":"lib-magpie"}}}),
        )
        .unwrap()
        .unwrap();
        let empty_text = empty["result"]["content"][0]["text"].as_str().unwrap();
        assert!(empty_text.contains("unavailable"), "{empty_text}");
        assert!(!empty_text.contains("points_preview"), "{empty_text}");
        let flip = handle(
            &server,
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"ui_flip","arguments":{"tileId":"profile-alice","personaId":"alice","flipped":true}}}),
        )
        .unwrap()
        .unwrap();
        let flip_text = flip["result"]["content"][0]["text"].as_str().unwrap();
        assert!(flip_text.contains("\"op\":\"flip\""), "{flip_text}");
        assert!(flip_text.contains("af_heart"), "{flip_text}");
        let synth = handle(
            &server,
            json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"synth","arguments":{"output":"request.wav","script":"examples/podcast_script_sample.txt"}}}),
        )
        .unwrap()
        .unwrap();
        let synth_text = synth["result"]["content"][0]["text"].as_str().unwrap();
        assert!(synth_text.contains("\"phase\":\"refused\""), "{synth_text}");
        assert!(synth_text.contains("No speech was invented"), "{synth_text}");
        assert!(!synth_text.contains("\"synthesizedSpeech\":true"), "{synth_text}");
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    // One contract, tested from both sides: the fixtures below are exactly
    // what the desktop window posts (apps/desktop/tests/play-control.test.ts
    // asserts that from the desktop's own builders); here the server accepts them.
    const DESKTOP_USER_PLAY: &str = include_str!("../../../schemas/examples/control/desktop-user-play.json");
    const SEEK_ROUND_TRIP: &str = include_str!("../../../schemas/examples/control/seek-round-trip.json");

    fn tool_text(response: &Value) -> Value {
        let text = response["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no tool text: {response}"));
        serde_json::from_str(text).expect("tool text is json")
    }

    fn bus_events_for(tile: &str, after: u64) -> Vec<Value> {
        control::since(after).1.into_iter().filter(|event| event["args"]["tileId"] == tile).collect()
    }

    #[test]
    fn contract_the_desktop_play_payload_is_accepted_and_carries_origin_user() {
        let server = Server::boot();
        let request: Value = serde_json::from_str(DESKTOP_USER_PLAY).unwrap();
        let tile = request["params"]["arguments"]["tileId"].as_str().unwrap().to_string();
        let before = control::since(0).0;
        let response = handle(&server, request).unwrap().unwrap();
        assert!(response.get("error").is_none(), "the server rejected the desktop's Play payload: {response}");
        let body = tool_text(&response);
        assert_eq!(body["args"]["origin"], "user");
        let events = bus_events_for(&tile, before);
        assert!(events.iter().any(|event| event["op"] == "playback" && event["args"]["origin"] == "user"), "{events:?}");
    }

    #[test]
    fn ui_playback_origin_is_a_validated_enum_and_a_bad_value_changes_nothing() {
        let server = Server::boot();
        let mut request: Value = serde_json::from_str(DESKTOP_USER_PLAY).unwrap();
        // A tile only this test uses, so the bus check cannot see another test's event.
        request["params"]["arguments"]["tileId"] = json!("contract-origin-bad");
        for bad in [json!("agent"), json!("mcp"), json!(""), json!(5), json!(null)] {
            request["params"]["arguments"]["origin"] = bad.clone();
            let before = control::since(0).0;
            let response = handle(&server, request.clone()).unwrap().unwrap();
            assert_eq!(response["error"]["code"], -32602, "origin {bad}: {response}");
            assert!(bus_events_for("contract-origin-bad", before).is_empty(), "origin {bad} reached the bus");
        }
        request["params"]["arguments"]["origin"] = json!("auto");
        let response = handle(&server, request).unwrap().unwrap();
        assert_eq!(tool_text(&response)["args"]["origin"], "auto");
    }

    #[test]
    fn every_tool_accepts_exactly_its_advertised_input_schema_properties() {
        for def in tool_defs() {
            let name = def["name"].as_str().unwrap();
            let mut advertised: Vec<&str> = def["inputSchema"]["properties"].as_object().unwrap().keys().map(String::as_str).collect();
            let mut allowed = allowed_arguments(name).to_vec();
            advertised.sort_unstable();
            allowed.sort_unstable();
            assert_eq!(advertised, allowed, "{name}: inputSchema and the allowed-args list disagree");
        }
    }

    #[test]
    fn contract_seek_round_trip_returns_where_the_window_landed() {
        let fixture: Value = serde_json::from_str(SEEK_ROUND_TRIP).unwrap();
        let seek = fixture["agentSeek"].clone();
        let seconds = seek["params"]["arguments"]["seconds"].as_f64().unwrap();
        let before = control::since(0).0;
        let agent = std::thread::spawn(move || {
            let server = Server::boot();
            handle(&server, seek).unwrap().unwrap()
        });
        // The window: find the queued seek, then post the fixture report for its seq.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let seq = loop {
            let found = control::since(before).1.into_iter().find(|event| {
                event["op"] == "playback" && event["args"]["action"] == "seek" && event["args"]["seconds"].as_f64() == Some(seconds)
            });
            if let Some(event) = found {
                assert!(event["args"].get("waitMs").is_none(), "waitMs stays off the bus");
                break event["seq"].as_u64().unwrap();
            }
            assert!(std::time::Instant::now() < deadline, "the seek never reached the bus");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let mut report = fixture["desktopReport"].clone();
        report["params"]["arguments"]["seq"] = json!(seq);
        let server = Server::boot();
        let recorded = handle(&server, report.clone()).unwrap().unwrap();
        assert!(recorded.get("error").is_none(), "the server rejected the desktop's seek report: {recorded}");
        let result = tool_text(&agent.join().unwrap());
        for key in ["requested_t", "landed_t", "ok", "reason"] {
            let (got, want) = (&result[key], &fixture["agentResult"][key]);
            match want.as_f64() {
                Some(number) => assert_eq!(got.as_f64(), Some(number), "{key}: {result}"),
                None => assert_eq!(got, want, "{key}: {result}"),
            }
        }
        // A second report for the same seek is refused.
        assert_eq!(handle(&server, report).unwrap().unwrap()["error"]["code"], -32602);
    }

    #[test]
    fn a_seek_no_window_answers_says_so_and_bad_reports_record_nothing() {
        let server = Server::boot();
        let call = |name: &str, args: Value| {
            handle(&server, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name": name, "arguments": args}})).unwrap().unwrap()
        };
        let timed_out = call("ui_playback", json!({"tileId": "contract-seek-quiet", "action": "seek", "seconds": 3.25, "waitMs": 0}));
        let body = tool_text(&timed_out);
        assert_eq!(body["requested_t"], 3.25);
        assert!(body["landed_t"].is_null());
        assert_eq!(body["ok"], false);
        assert!(body["reason"].as_str().unwrap().contains("no window reported"), "{body}");
        let seq = body["seq"].as_u64().unwrap();
        let report = |args: Value| call("ui_seek_report", args);
        let code = |response: Value| response["error"]["code"].clone();
        assert_eq!(code(report(json!({"seq": seq + 100_000, "requested_t": 3.25, "landed_t": 3.25, "ok": true, "reason": "x"}))), -32602, "unknown seq");
        assert_eq!(code(report(json!({"seq": seq, "requested_t": 9.0, "landed_t": 3.25, "ok": true, "reason": "x"}))), -32602, "wrong requested_t");
        assert_eq!(code(report(json!({"seq": seq, "requested_t": 3.25, "landed_t": null, "ok": true, "reason": "x"}))), -32602, "ok without landed_t");
        assert_eq!(code(report(json!({"seq": seq, "requested_t": 3.25, "landed_t": -1, "ok": false, "reason": "x"}))), -32602, "negative landed_t");
        assert_eq!(code(report(json!({"seq": seq, "requested_t": 3.25, "landed_t": 0, "ok": false, "reason": "r".repeat(241)}))), -32602, "long reason");
        assert_eq!(code(call("ui_playback", json!({"tileId": "contract-seek-quiet", "action": "seek", "seconds": 1, "waitMs": 60_000}))), -32602, "waitMs cap");
        assert_eq!(code(call("ui_playback", json!({"tileId": "contract-seek-quiet", "action": "pause", "waitMs": 10}))), -32602, "waitMs on pause");
        // None of the bad reports was recorded: the honest one still is.
        let honest = report(json!({"seq": seq, "requested_t": 3.25, "landed_t": 0, "ok": false, "reason": "seek failed: this WAV source cannot seek (stays at 0:00)"}));
        assert!(honest.get("error").is_none(), "{honest}");
        assert_eq!(tool_text(&honest)["report"]["landed_t"], 0.0);
    }
}
