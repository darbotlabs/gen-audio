//! A small Agent Client Protocol (ACP) agent.
//!
//! ACP itself is session-scoped: `session/new` returns a `sessionId`. That is
//! not an MCP session. This process keeps the map only in memory for the
//! stdio lifetime and does not pin a client to a Serve node.

use std::collections::HashMap;
use std::sync::Mutex;

use gen_audio_connectors::health_all;
use serde_json::{json, Value};

#[derive(Clone)]
struct Track {
    /// ACP requires cwd. It is stored so the session exists and this process never opens it.
    #[allow(dead_code)]
    cwd: String,
    agents: Vec<String>,
    voice: String,
    /// Library asset uids bound to this track (max 8). Uids only, never paths.
    #[allow(dead_code)]
    assets: Vec<String>,
}

pub struct Agent {
    sessions: Mutex<HashMap<String, Track>>,
    next: Mutex<u64>,
}

impl Agent {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next: Mutex::new(1),
        }
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct AcpOutput {
    pub response: Option<Value>,
    pub notifications: Vec<Value>,
}

pub fn handle(agent: &Agent, message: Value) -> Result<AcpOutput, String> {
    let obj = message.as_object().ok_or("request must be an object")?;
    let id = obj.get("id").cloned();
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("jsonrpc must be 2.0".into());
    }
    let method = obj.get("method").and_then(Value::as_str).unwrap_or("");
    let params = obj.get("params").cloned().unwrap_or(json!({}));
    let notification = matches!(id, None | Some(Value::Null));
    let (result, notes) = match method {
        "initialize" => (initialize(&params)?, Vec::new()),
        "session/new" => (session_new(agent, &params)?, Vec::new()),
        "session/prompt" => session_prompt(agent, &params)?,
        "session/cancel" => {
            let _ = session_cancel(agent, &params)?;
            (json!({}), Vec::new())
        }
        _ => return Err(format!("method not found: {method}")),
    };
    if notification {
        return Ok(AcpOutput {
            response: None,
            notifications: notes,
        });
    }
    Ok(AcpOutput {
        response: Some(json!({"jsonrpc":"2.0","id": id, "result": result})),
        notifications: notes,
    })
}

fn initialize(params: &Value) -> Result<Value, String> {
    let version = params.get("protocolVersion").and_then(Value::as_u64).unwrap_or(1);
    if version != 1 {
        return Err(format!("unsupported ACP protocolVersion {version}"));
    }
    Ok(json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": false,
            "promptCapabilities": {"image": false, "audio": false, "embeddedContext": false}
        },
        "agentInfo": {"name": "gen-audio", "version": "0.1.0"},
        "authMethods": []
    }))
}

fn session_new(agent: &Agent, params: &Value) -> Result<Value, String> {
    let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or("");
    if cwd.is_empty() || cwd.chars().count() > 512 {
        return Err("session/new requires cwd".into());
    }
    // ACP requires cwd. This agent does not open it and does not read files from it.
    let agents = params.get("agents").and_then(Value::as_array).cloned().unwrap_or_default();
    let voice = params.get("voice").and_then(Value::as_str).unwrap_or("").to_string();
    if !agents.is_empty() || !voice.is_empty() {
        gen_audio_mcp::control::validate_track(&agents, &voice).map_err(|(_, message)| message)?;
    }
    let agent_ids: Vec<String> = agents.iter().filter_map(|value| value.as_str().map(str::to_string)).collect();
    let assets = session_assets(params.get("assets"))?;
    let mut next = agent.next.lock().expect("session counter");
    let id = format!("gen-audio-session-{next}");
    *next += 1;
    drop(next);
    agent.sessions.lock().expect("sessions").insert(
        id.clone(),
        Track {
            cwd: cwd.to_string(),
            agents: agent_ids.clone(),
            voice: voice.clone(),
            assets: assets.clone(),
        },
    );
    Ok(json!({"sessionId": id, "agents": agent_ids, "voice": voice, "assets": assets}))
}

/// `session/new` `assets`: up to 8 distinct ga1 uids that resolve in the library catalog.
fn session_assets(value: Option<&Value>) -> Result<Vec<String>, String> {
    use gen_audio_core::asset_catalog::{require, MAX_SESSION_ASSETS};
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or("assets must be an array of asset uids")?;
    if items.len() > MAX_SESSION_ASSETS {
        return Err(format!("assets is capped at {MAX_SESSION_ASSETS} uids"));
    }
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let uid = item.as_str().ok_or("asset uid must be a string")?;
        require(uid).map_err(|error| error.rpc().1)?;
        if out.iter().any(|seen| seen == uid) {
            return Err(format!("duplicate asset {uid}"));
        }
        out.push(uid.to_string());
    }
    Ok(out)
}

fn session_prompt(agent: &Agent, params: &Value) -> Result<(Value, Vec<Value>), String> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("session/prompt requires sessionId")?;
    let track = agent
        .sessions
        .lock()
        .expect("sessions")
        .get(session_id)
        .cloned()
        .ok_or("unknown sessionId")?;
    let prompt = prompt_text(params.get("prompt"))?;
    if prompt.trim().eq_ignore_ascii_case("health") || prompt.trim() == "status" {
        let lines: Vec<String> = health_all()
            .into_iter()
            .map(|item| format!("{}={:?}", item.connector_id, item.mode))
            .collect();
        let summary = format!(
            "Gen-Audio connector health (local report, not a vendor completion): {}",
            lines.join(", ")
        );
        let note = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": summary}
                }
            }
        });
        return Ok((json!({"stopReason": "end_turn"}), vec![note]));
    }
    let mut calls = product_plan(&prompt, &track);
    for (name, args) in &calls {
        if name == "ui_set_sidepane" || name == "ui_generate" {
            if let Some(stored) = agent.sessions.lock().expect("sessions").get_mut(session_id) {
                if let Some(list) = args.get("agents").and_then(Value::as_array) {
                    stored.agents = list.iter().filter_map(|value| value.as_str().map(str::to_string)).collect();
                }
                if let Some(voice) = args.get("voice").and_then(Value::as_str) {
                    stored.voice = voice.to_string();
                }
            }
        }
    }
    if calls.is_empty() {
        calls.push(("voice_profile_list".into(), json!({})));
    }
    let mut notes = Vec::new();
    let mut summary = String::new();
    for (index, (name, args)) in calls.iter().enumerate() {
        let tool_id = format!("gen-audio-tool-{}", index + 1);
        notes.push(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": tool_id,
                    "title": name,
                    "kind": "other",
                    "status": "pending"
                }
            }
        }));
        match mcp_tool(name, args.clone()) {
            Ok(value) => {
                let text = value["content"][0]["text"].as_str().unwrap_or("");
                if !summary.is_empty() {
                    summary.push('\n');
                }
                summary.push_str(text);
                notes.push(json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_id,
                        "update": {
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": tool_id,
                            "status": "completed",
                            "content": [{"type": "text", "text": text}]
                        }
                    }
                }));
            }
            Err(err) => {
                if !summary.is_empty() {
                    summary.push('\n');
                }
                summary.push_str(&err);
                notes.push(json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_id,
                        "update": {
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": tool_id,
                            "status": "failed",
                            "content": [{"type": "text", "text": err}]
                        }
                    }
                }));
            }
        }
    }
    summary.push_str(" synthesizedSpeech is false. This agent does not invent a podcast.");
    notes.push(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": summary}
            }
        }
    }));
    Ok((json!({"stopReason": "end_turn"}), notes))
}

fn mcp_tool(name: &str, args: Value) -> Result<Value, String> {
    let server = gen_audio_mcp::Server::boot();
    let result = gen_audio_mcp::call_tool(&server, &json!({"name": name, "arguments": args}))
        .map_err(|(_, message)| message);
    let _ = std::fs::remove_dir_all(&server.scratch.dir);
    result
}

fn is_verb(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "agent"
            | "agents"
            | "persona"
            | "personas"
            | "voice"
            | "navigate"
            | "slide"
            | "select"
            | "play"
            | "pause"
            | "seek"
            | "profile"
            | "rename"
            | "generate"
            | "list"
            | "flip"
            | "harvest"
    )
}

fn product_plan(prompt: &str, track: &Track) -> Vec<(String, Value)> {
    let tokens: Vec<String> = prompt.split_whitespace().map(str::to_string).collect();
    let mut calls = Vec::new();
    let mut agents: Vec<String> = Vec::new();
    let mut voice: Option<String> = None;
    let mut generate = false;
    let mut index = 0;
    while index < tokens.len() {
        let verb = tokens[index].to_ascii_lowercase();
        match verb.as_str() {
            "agent" | "agents" | "persona" | "personas" => {
                index += 1;
                while index < tokens.len() && !is_verb(&tokens[index]) {
                    agents.push(tokens[index].to_ascii_lowercase());
                    index += 1;
                }
            }
            "voice" => {
                index += 1;
                if index < tokens.len() && !is_verb(&tokens[index]) {
                    voice = Some(tokens[index].to_ascii_lowercase());
                    index += 1;
                }
            }
            "navigate" | "slide" => {
                index += 1;
                if index < tokens.len() && !is_verb(&tokens[index]) {
                    calls.push(("ui_navigate".into(), json!({"slide": tokens[index].to_ascii_lowercase()})));
                    index += 1;
                }
            }
            "select" => {
                index += 1;
                if index < tokens.len() && !is_verb(&tokens[index]) {
                    calls.push(("ui_select_tile".into(), json!({"tileId": tokens[index]})));
                    index += 1;
                }
            }
            "play" | "pause" => {
                let action = verb;
                index += 1;
                if index < tokens.len() && !is_verb(&tokens[index]) {
                    calls.push((
                        "ui_playback".into(),
                        json!({"tileId": tokens[index], "action": action}),
                    ));
                    index += 1;
                }
            }
            "seek" => {
                index += 1;
                if index + 1 < tokens.len() {
                    let seconds: f64 = tokens[index + 1].parse().unwrap_or(-1.0);
                    calls.push((
                        "ui_playback".into(),
                        json!({"tileId": tokens[index], "action": "seek", "seconds": seconds}),
                    ));
                    index += 2;
                }
            }
            "profile" => {
                index += 1;
                if index < tokens.len() && !is_verb(&tokens[index]) {
                    calls.push((
                        "voice_profile_get".into(),
                        json!({"personaId": tokens[index].to_ascii_lowercase()}),
                    ));
                    index += 1;
                }
            }
            "rename" => {
                index += 1;
                if index < tokens.len() {
                    let clip = tokens[index].clone();
                    index += 1;
                    let mut args = json!({"clipId": clip});
                    while index < tokens.len() && tokens[index].contains('=') {
                        if let Some((key, value)) = tokens[index].split_once('=') {
                            if key == "semantic" {
                                args["semanticName"] = json!(value);
                            } else if key == "face" {
                                args["faceName"] = json!(value);
                            }
                        }
                        index += 1;
                    }
                    calls.push(("library_rename".into(), args));
                }
            }
            "generate" => {
                generate = true;
                index += 1;
            }
            "flip" => {
                index += 1;
                if index < tokens.len() && !is_verb(&tokens[index]) {
                    let tile = tokens[index].clone();
                    index += 1;
                    let mut args = json!({"tileId": tile, "flipped": true});
                    if index < tokens.len() && !is_verb(&tokens[index]) {
                        args["personaId"] = json!(tokens[index].to_ascii_lowercase());
                        index += 1;
                    }
                    calls.push(("ui_flip".into(), args));
                }
            }
            "harvest" => {
                index += 1;
                if index < tokens.len() && !is_verb(&tokens[index]) {
                    let clip = tokens[index].clone();
                    index += 1;
                    let mut args = json!({"clipId": clip, "apply": false});
                    if index < tokens.len() && !is_verb(&tokens[index]) {
                        args["personaId"] = json!(tokens[index].to_ascii_lowercase());
                        args["apply"] = json!(true);
                        index += 1;
                    }
                    calls.push(("library_harvest".into(), args));
                }
            }
            "list" => {
                calls.push(("voice_profile_list".into(), json!({})));
                index += 1;
            }
            _ => index += 1,
        }
    }
    let fallback_agents = if track.agents.is_empty() {
        vec!["alice".to_string()]
    } else {
        track.agents.clone()
    };
    let fallback_voice = if track.voice.is_empty() {
        "kokoro_onnx".to_string()
    } else {
        track.voice.clone()
    };
    if !agents.is_empty() || voice.is_some() {
        let agents = if agents.is_empty() { fallback_agents } else { agents };
        let voice = voice.unwrap_or(fallback_voice);
        let args = json!({"agents": agents, "voice": voice});
        if generate {
            calls.insert(0, ("ui_generate".into(), args));
        } else {
            calls.insert(0, ("ui_set_sidepane".into(), args));
        }
    } else if generate {
        calls.insert(
            0,
            (
                "ui_generate".into(),
                json!({"agents": fallback_agents, "voice": fallback_voice}),
            ),
        );
    }
    if generate {
        let voice_id = calls
            .iter()
            .find(|(name, _)| name == "ui_generate")
            .and_then(|(_, args)| args.get("voice").and_then(Value::as_str))
            .unwrap_or("");
        let adapter = gen_audio_core::catalog::voice_model(voice_id)
            .map(|model| model.synth_adapter)
            .unwrap_or(false);
        if adapter {
            calls.push((
                "synth".into(),
                json!({"output": "request.wav", "script": "examples/podcast_script_sample.txt"}),
            ));
        }
    }
    calls
}

fn session_cancel(agent: &Agent, params: &Value) -> Result<Value, String> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("session/cancel requires sessionId")?;
    agent.sessions.lock().expect("sessions").remove(session_id);
    Ok(json!({}))
}

fn prompt_text(prompt: Option<&Value>) -> Result<String, String> {
    let blocks = prompt
        .and_then(Value::as_array)
        .ok_or("prompt must be an array of content blocks")?;
    let mut text = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(piece) = block.get("text").and_then(Value::as_str) {
                text.push_str(piece);
            }
        }
    }
    if text.chars().count() > 8_000 {
        return Err("prompt is too long".into());
    }
    Ok(text)
}

pub fn smoke() -> Result<String, String> {
    let agent = Agent::new();
    let init = handle(
        &agent,
        json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
                "clientInfo": {"name": "gen-audio-smoke", "version": "0.1.0"}
            }
        }),
    )?;
    let version = init
        .response
        .as_ref()
        .and_then(|value| value["result"]["protocolVersion"].as_u64())
        .ok_or("initialize missing protocolVersion")?;
    if version != 1 {
        return Err("unexpected protocol version".into());
    }
    let created = handle(
        &agent,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/new",
            "params": {"cwd": ".", "mcpServers": []}
        }),
    )?;
    let session_id = created.response.as_ref().unwrap()["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let prompted = handle(
        &agent,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "health"}]
            }
        }),
    )?;
    let stop = prompted.response.as_ref().unwrap()["result"]["stopReason"].as_str();
    if stop != Some("end_turn") {
        return Err("prompt did not end the turn".into());
    }
    if prompted.notifications.is_empty() {
        return Err("prompt produced no session/update".into());
    }
    Ok("acp smoke ok".into())
}

pub fn stdio_loop(agent: &Agent) {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    json!({"jsonrpc":"2.0","id": null, "error": {"code": -32700, "message": "parse error"}})
                );
                let _ = stdout.flush();
                continue;
            }
        };
        let request_id = message.get("id").cloned().unwrap_or(Value::Null);
        match handle(agent, message) {
            Ok(output) => {
                for note in output.notifications {
                    let _ = writeln!(stdout, "{note}");
                }
                if let Some(response) = output.response {
                    let _ = writeln!(stdout, "{response}");
                }
                let _ = stdout.flush();
            }
            Err(err) => {
                let id = request_id;
                if id.is_null() {
                    continue;
                }
                let _ = writeln!(
                    stdout,
                    "{}",
                    json!({"jsonrpc":"2.0","id": id, "error": {"code": -32603, "message": err}})
                );
                let _ = stdout.flush();
            }
        }
    }
}

use std::io::{BufRead, Write};

#[cfg(test)]
mod tests {
    use super::*;

    use gen_audio_core::bridge::NoKokoroEnv;

    #[test]
    fn handshake_smoke() {
        assert_eq!(smoke().unwrap(), "acp smoke ok");
    }

    #[test]
    fn cwd_is_not_opened_and_errors_keep_the_request_id() {
        let agent = Agent::new();
        let created = handle(
            &agent,
            json!({"jsonrpc":"2.0","id":7,"method":"session/new","params":{"cwd":"/this/path/is/not/opened"}}),
        )
        .unwrap();
        assert!(created.response.unwrap()["result"]["sessionId"].as_str().unwrap().starts_with("gen-audio-session-"));
        let missing = handle(
            &agent,
            json!({"jsonrpc":"2.0","id":8,"method":"session/load","params":{}}),
        )
        .unwrap_err();
        assert!(missing.contains("method not found"));
        let bad = handle(
            &agent,
            json!({"jsonrpc":"1.0","id":9,"method":"initialize","params":{"protocolVersion":1}}),
        )
        .unwrap_err();
        assert!(bad.contains("2.0"));
    }

    #[test]
    fn prompt_calls_the_voice_profile_tool() {
        let agent = Agent::new();
        let created = handle(
            &agent,
            json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"."}}),
        )
        .unwrap();
        let session_id = created.response.unwrap()["result"]["sessionId"].as_str().unwrap().to_string();
        let prompted = handle(
            &agent,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [{"type": "text", "text": "profile alice"}]
                }
            }),
        )
        .unwrap();
        let blob = serde_json::to_string(&prompted.notifications).unwrap();
        assert!(blob.contains("voice_profile_get"), "{blob}");
        assert!(blob.contains("af_heart"), "{blob}");
        assert!(blob.contains("tool_call"), "{blob}");
        assert!(blob.contains("synthesizedSpeech"), "{blob}");
        assert!(!blob.contains("\"synthesizedSpeech\":true"));
    }

    #[test]
    fn session_persona_generate_calls_synth_and_does_not_invent_speech() {
        let _env = NoKokoroEnv::new();
        let agent = Agent::new();
        let created = handle(
            &agent,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "session/new",
                "params": {"cwd": ".", "agents": ["alice"], "voice": "kokoro_onnx"}
            }),
        )
        .unwrap();
        let result = created.response.unwrap();
        assert_eq!(result["result"]["agents"][0], "alice");
        assert_eq!(result["result"]["voice"], "kokoro_onnx");
        let session_id = result["result"]["sessionId"].as_str().unwrap().to_string();
        let prompted = handle(
            &agent,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [{"type": "text", "text": "generate"}]
                }
            }),
        )
        .unwrap();
        let blob = serde_json::to_string(&prompted.notifications).unwrap();
        assert!(blob.contains("ui_generate"), "{blob}");
        assert!(blob.contains("synth"), "{blob}");
        assert!(blob.contains("No speech was invented"), "{blob}");
        assert!(blob.contains("tool_call"), "{blob}");
        assert!(!blob.contains("\"synthesizedSpeech\":true"), "{blob}");
        assert!(blob.contains("GEN_AUDIO_KOKORO_MODEL and GEN_AUDIO_KOKORO_VOICES are unset"), "{blob}");
        let rejected = handle(
            &agent,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "session/new",
                "params": {"cwd": ".", "agents": ["copilot"], "voice": "af_heart"}
            }),
        )
        .unwrap_err();
        assert!(rejected.contains("connector") || rejected.contains("af_heart"), "{rejected}");
    }

    #[test]
    fn session_new_binds_library_asset_uids() {
        let agent = Agent::new();
        let clip = gen_audio_core::asset_catalog::uid_for_legacy("audio_clip", "lib-misaki-kokoro").unwrap();
        let out = handle(
            &agent,
            json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":".","assets":[clip, "ga:cube_ihdr:biaxxmnibxtcur7nxdu3ffvna4"]}}),
        )
        .unwrap();
        let result = &out.response.unwrap()["result"];
        assert_eq!(result["assets"][0], clip);
        let session = result["sessionId"].as_str().unwrap();
        assert_eq!(agent.sessions.lock().unwrap()[session].assets.len(), 2);
        for bad in [
            json!(["ga:cube_ihdr:aaaaaaaaaaaaaaaaaaaaaaaaaa"]),
            json!(["lib-misaki-kokoro"]),
            json!([clip, clip]),
            json!(vec![clip; 9]),
            json!("ga:cube_ihdr:biaxxmnibxtcur7nxdu3ffvna4"),
        ] {
            let err = handle(&agent, json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":".","assets":bad}}));
            assert!(err.is_err(), "{bad} should be rejected");
        }
    }
}
