//! A small Agent Client Protocol (ACP) agent.
//!
//! ACP itself is session-scoped: `session/new` returns a `sessionId`. That is
//! not an MCP session. This process keeps the map only in memory for the
//! stdio lifetime and does not pin a client to a Serve node.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
    server: Arc<SessionServer>,
}

struct SessionServer(gen_audio_mcp::Server);

impl Drop for SessionServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0.scratch.dir);
    }
}

pub struct Agent {
    sessions: Mutex<HashMap<String, Track>>,
    next: Mutex<u64>,
    next_tool: std::sync::atomic::AtomicU64,
}

impl Agent {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next: Mutex::new(1),
            next_tool: std::sync::atomic::AtomicU64::new(1),
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
    if id.as_ref().is_some_and(|id| !gen_audio_core::subprocess::valid_rpc_id(id)) {
        return Err("invalid request id".into());
    }
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("jsonrpc must be 2.0".into());
    }
    let method = obj.get("method").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or("invalid method")?;
    let params = obj.get("params").cloned().unwrap_or(json!({}));
    if !params.is_object() { return Err("params must be an object".into()); }
    let notification = id.is_none();
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
    let version = match params.get("protocolVersion") {
        None => 1,
        Some(v) => v.as_u64().ok_or("protocolVersion must be an integer")?,
    };
    if version != 1 {
        return Err(format!("unsupported ACP protocolVersion {version}"));
    }
    Ok(json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": false,
            "promptCapabilities": {"image": false, "audio": false, "embeddedContext": false}
        },
        "agentInfo": {"name": "gen-audio", "version": env!("CARGO_PKG_VERSION")},
        "authMethods": []
    }))
}

fn session_new(agent: &Agent, params: &Value) -> Result<Value, String> {
    let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or("");
    if cwd.is_empty() || cwd.len() > 512 || cwd.chars().any(char::is_control) {
        return Err("session/new requires cwd".into());
    }
    // ACP requires cwd. This agent does not open it and does not read files from it.
    let agents = match params.get("agents") {
        None => Vec::new(),
        Some(value) => value.as_array().ok_or("agents must be an array")?.clone(),
    };
    let voice = match params.get("voice") {
        None => String::new(),
        Some(value) => value.as_str().ok_or("voice must be a string")?.to_string(),
    };
    if !agents.is_empty() || !voice.is_empty() {
        gen_audio_mcp::control::validate_track(&agents, &voice).map_err(|(_, message)| message)?;
    }
    let agent_ids: Vec<String> = agents.iter().filter_map(|value| value.as_str().map(str::to_string)).collect();
    let assets = session_assets(params.get("assets"))?;
    let mut sessions = agent.sessions.lock().map_err(|_| "sessions lock")?;
    if sessions.len() >= 32 { return Err("session capacity reached (32); cancel unused sessions".into()); }
    let mut next = agent.next.lock().expect("session counter");
    let id = format!("gen-audio-session-{next}");
    *next += 1;
    drop(next);
    sessions.insert(
        id.clone(),
        Track {
            cwd: cwd.to_string(),
            agents: agent_ids.clone(),
            voice: voice.clone(),
            assets: assets.clone(),
            server: Arc::new(SessionServer(gen_audio_mcp::Server::try_isolated()?)),
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
    check_session_id(session_id)?;
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
    let mut calls = if prompt.trim_start().starts_with('{') {
        let request: Value = serde_json::from_str(&prompt).map_err(|_| "invalid tool JSON prompt")?;
        let name = request["name"].as_str().ok_or("tool JSON prompt requires name")?;
        let mut args = request.get("arguments").cloned().unwrap_or(json!({}));
        if !args.is_object() { return Err("tool arguments must be an object".into()); }
        if name == "synth" && args.get("castMap").is_none() {
            if args.get("agents").is_none() { args["agents"] = json!(track.agents); }
            if args.get("voice").is_none() { args["voice"] = json!(track.voice); }
        }
        vec![(name.to_string(), args)]
    } else { product_plan(&prompt, &track) };
    if calls.len() > 16 { return Err("prompt exceeds 16 tool calls".into()); }
    if calls.is_empty() {
        calls.push(("voice_profile_list".into(), json!({})));
    }
    let mut notes = Vec::new();
    let mut summary = String::new();
    let mut speech = false;
    let mut failures = 0usize;
    for (name, args) in &calls {
        let tool_id = format!("gen-audio-tool-{}", agent.next_tool.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
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
        match mcp_tool(&track.server.0, name, args.clone()) {
            Ok(value) => {
                let text = value["content"][0]["text"].as_str().unwrap_or("");
                let failed = value["isError"] == true;
                failures += usize::from(failed);
                let payload: Value = serde_json::from_str(text).unwrap_or(Value::Null);
                speech |= !failed && payload["synthesizedSpeech"] == true;
                if !failed && (name == "ui_set_sidepane" || name == "ui_generate") {
                    if let Some(stored) = agent.sessions.lock().expect("sessions").get_mut(session_id) {
                        if let Some(list) = args.get("agents").and_then(Value::as_array) {
                            stored.agents = list.iter().filter_map(|value| value.as_str().map(str::to_string)).collect();
                        }
                        if let Some(voice) = args.get("voice").and_then(Value::as_str) { stored.voice = voice.to_string(); }
                    }
                }
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
                            "status": if failed { "failed" } else { "completed" },
                            "content": [{"type": "text", "text": text}]
                        }
                    }
                }));
            }
            Err(err) => {
                failures += 1;
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
    summary.push_str(&format!("\nsynthesizedSpeech is {speech}. {failures} tool call(s) failed."));
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

fn control_tool(name: &str) -> bool {
    matches!(
        name,
        "ui_navigate"
            | "ui_select_tile"
            | "ui_flip"
            | "ui_playback"
            | "ui_set_sidepane"
            | "ui_generate"
            | "ui_compare"
            | "ui_cube"
            | "library_rename"
            | "viewport_get"
            | "card_export"
    )
}

fn desktop_addr() -> Result<Option<String>, String> {
    let raw = raw_desktop_addr();
    let Some(addr) = raw else {
        return Ok(None);
    };
    ensure_loopback_target(&addr)?;
    Ok(Some(addr))
}

fn read_configured_addr() -> Option<String> {
    if let Ok(addr) = std::env::var("GEN_AUDIO_MCP_ADDR") {
        let addr = addr.trim();
        if !addr.is_empty() {
            return Some(addr.to_string());
        }
    }
    gen_audio_core::paths::read_mcp_addr()
}

fn raw_desktop_addr() -> Option<String> {
    #[cfg(test)]
    {
        let overridden = DESKTOP_ADDR.with(|slot| slot.borrow().clone());
        if let Some(address) = overridden {
            return address;
        }
        // GEN_AUDIO_MCP_ADDR is process-global. Tests that swap it hold this
        // lock across the swap and the read, so a parallel test cannot observe
        // the temporary non-loopback value.
        let _guard = mcp_addr_env_lock();
        return read_configured_addr();
    }
    #[cfg(not(test))]
    read_configured_addr()
}

#[cfg(test)]
fn mcp_addr_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn ensure_loopback_target(addr: &str) -> Result<(), String> {
    gen_audio_mcp::http::ensure_bind_allowed(addr).map_err(|err| err.to_string())?;
    let socket: std::net::SocketAddr = addr
        .parse()
        .map_err(|err| format!("desktop address is not a socket address: {err}"))?;
    if !socket.ip().is_loopback() {
        return Err("ACP refuses a non-loopback desktop address".into());
    }
    Ok(())
}

/// UI actions go to the desktop's bound MCP listener.
/// An in-process server has a different control bus, so it is not a fallback.
fn mcp_tool(server: &gen_audio_mcp::Server, name: &str, args: Value) -> Result<Value, String> {
    if control_tool(name) {
        if let Some(addr) = desktop_addr()? {
            let token = desktop_token().ok_or("desktop control requires GEN_AUDIO_MCP_TOKEN from the owned desktop connection")?;
            let payload = gen_audio_mcp::http::tools_call(&addr, &token, name, &args)?;
            return Ok(json!({
                "content":[{"type":"text","text":payload.to_string()}],
                "isError":payload["ok"] == false
            }));
        }
        return Err(
            "desktop control endpoint is not bound; refusing to apply this action on an isolated bus"
                .into(),
        );
    }
    gen_audio_mcp::call_tool(server, &json!({"name": name, "arguments": args}))
        .map_err(|err| err.message)
}

fn desktop_token() -> Option<String> {
    #[cfg(test)]
    if DESKTOP_ADDR.with(|slot| slot.borrow().is_some()) {
        return Some("a".repeat(64));
    }
    std::env::var("GEN_AUDIO_MCP_TOKEN").ok()
}

#[cfg(test)]
thread_local! {
    static DESKTOP_ADDR: std::cell::RefCell<Option<Option<String>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn bind_desktop_for_test(addr: Option<String>) {
    DESKTOP_ADDR.with(|slot| *slot.borrow_mut() = addr.map(Some));
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
    let (commands, script_text) = match prompt.split_once('\n') {
        Some((commands, text)) if commands.split_whitespace().any(|word| word.eq_ignore_ascii_case("generate")) => (commands, Some(text)),
        _ => (prompt, None),
    };
    let tokens: Vec<String> = commands.split_whitespace().map(str::to_string).collect();
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
    let fallback_agents = track.agents.clone();
    let fallback_voice = track.voice.clone();
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
        let requested = calls
            .iter()
            .find(|(name, _)| name == "ui_generate")
            .map(|(_, args)| args.clone()).unwrap_or(json!({}));
        let mut args = requested;
        if let Some(text) = script_text { args["scriptText"] = json!(text); }
        calls.push(("synth".into(), args));
    }
    calls
}

fn session_cancel(agent: &Agent, params: &Value) -> Result<Value, String> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("session/cancel requires sessionId")?;
    check_session_id(session_id)?;
    agent.sessions.lock().expect("sessions").remove(session_id);
    Ok(json!({}))
}

fn check_session_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        return Err("invalid sessionId".into());
    }
    Ok(())
}

fn prompt_text(prompt: Option<&Value>) -> Result<String, String> {
    let blocks = prompt
        .and_then(Value::as_array)
        .ok_or("prompt must be an array of content blocks")?;
    if blocks.is_empty() || blocks.len() > 64 { return Err("prompt must contain 1 to 64 text blocks".into()); }
    let mut text = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("text") { return Err("only text prompt blocks are supported".into()); }
        let piece = block.get("text").and_then(Value::as_str).ok_or("text block needs text")?;
        if text.len() + piece.len() > 256 * 1024 { return Err("prompt is too long".into()); }
        if !text.is_empty() { text.push('\n'); }
        text.push_str(piece);
    }
    if text.trim().is_empty() || text.chars().count() > 32_000 || text.contains('\0') {
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
    let mut input = stdin.lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let line = match gen_audio_core::subprocess::read_line_bounded(&mut input, 1024 * 1024) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                let _ = writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":error}}));
                let _ = stdout.flush();
                break;
            }
        };
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
        let notification = message.get("id").is_none() && message.is_object();
        let request_id = message.get("id").filter(|id| gen_audio_core::subprocess::valid_rpc_id(id)).cloned().unwrap_or(Value::Null);
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
                if notification {
                    continue;
                }
                let _ = writeln!(
                    stdout,
                    "{}",
                    json!({"jsonrpc":"2.0","id": id, "error": {"code": if err.starts_with("method not found") { -32601 } else if err.starts_with("invalid request") || err.starts_with("jsonrpc") || err.starts_with("request must") || err == "invalid method" { -32600 } else { -32602 }, "message": gen_audio_core::redact::redact_secrets(&err)}})
                );
                let _ = stdout.flush();
            }
        }
    }
}

use std::io::Write;

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
        assert!(blob.contains("scriptText"), "{blob}");
        assert!(blob.contains("\"status\":\"failed\""), "{blob}");
        assert!(blob.contains("tool_call"), "{blob}");
        assert!(!blob.contains("\"synthesizedSpeech\":true"), "{blob}");
        assert!(blob.contains("provide exactly one of script or scriptText"), "{blob}");
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
    fn desktop_target_must_be_loopback() {
        bind_desktop_for_test(None);
        let _guard = mcp_addr_env_lock();
        let saved = std::env::var("GEN_AUDIO_MCP_ADDR").ok();
        std::env::set_var("GEN_AUDIO_MCP_ADDR", "10.1.8.70:8765");
        let err = ensure_loopback_target(&read_configured_addr().expect("env addr")).unwrap_err();
        assert!(err.contains("loopback") || err.contains("non-loopback"), "{err}");
        match saved {
            Some(value) => std::env::set_var("GEN_AUDIO_MCP_ADDR", value),
            None => std::env::remove_var("GEN_AUDIO_MCP_ADDR"),
        }
    }

    #[test]
    fn session_new_binds_library_asset_uids() {
        let agent = Agent::new();
        let clip = gen_audio_core::asset_catalog::uid_for_legacy("audio_clip", "lib-misaki-kokoro").unwrap();
        let cube = gen_audio_core::asset_catalog::uid_for_legacy("cube_ihdr", "lib-misaki-kokoro.cube").unwrap();
        let out = handle(
            &agent,
            json!({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":".","assets":[clip, cube]}}),
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

    #[test]
    fn t14_navigate_reaches_the_bound_desktop_bus() {
        let token = "a".repeat(64);
        let addr = gen_audio_mcp::http::spawn_loopback("127.0.0.1:0", &token).unwrap();
        let addr = addr.to_string();
        bind_desktop_for_test(Some(addr.clone()));
        let before = gen_audio_mcp::http::get_control(&addr, &token, 0).unwrap();
        let cursor = before["cursor"].as_u64().unwrap_or(0);
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
                    "prompt": [{"type": "text", "text": "navigate spatial"}]
                }
            }),
        )
        .unwrap();
        bind_desktop_for_test(None);
        let blob = serde_json::to_string(&prompted.notifications).unwrap();
        assert!(blob.contains("ui_navigate"), "{blob}");
        assert!(!blob.contains("isolated bus"), "{blob}");
        let after = gen_audio_mcp::http::get_control(&addr, &token, cursor).unwrap();
        let events = after["events"].as_array().cloned().unwrap_or_default();
        assert!(
            events.iter().any(|event| {
                event["op"] == "navigate"
                    && event["args"]["slide"] == "slide:spatial"
            }),
            "{after}"
        );
        let view = gen_audio_mcp::http::tools_call(&addr, &token, "viewport_get", &json!({})).unwrap();
        assert_eq!(view["ui"]["slide"], "slide:spatial");
    }

    #[test]
    fn ui_navigate_without_a_desktop_does_not_touch_an_isolated_bus() {
        // An actual installed desktop may be running on the test machine.
        DESKTOP_ADDR.with(|slot| *slot.borrow_mut() = Some(None));
        let marker = format!("isolated-{}", std::process::id());
        let err = mcp_tool(&gen_audio_mcp::Server::isolated(), "ui_navigate", json!({"slide": "spatial", "tileId": marker})).unwrap_err();
        assert!(err.contains("not bound"), "{err}");
        let delta = gen_audio_mcp::control::since(0);
        assert!(
            delta.events.iter().all(|event| event["args"]["tileId"] != marker),
            "isolated ACP published onto the local bus"
        );
        bind_desktop_for_test(None);
    }

    #[test]
    fn session_scratch_survives_calls_and_generation_preserves_content() {
        let agent = Agent::new();
        let created = session_new(&agent, &json!({"cwd":".","agents":["alice"],"voice":"kokoro_onnx"})).unwrap();
        let id = created["sessionId"].as_str().unwrap();
        let dir;
        {
            let sessions = agent.sessions.lock().unwrap();
            let track = &sessions[id];
            dir = track.server.0.scratch.dir.clone();
            let calls = product_plan("generate\nOnly this supplied content.", track);
            let synth = calls.iter().find(|(name,_)| name == "synth").unwrap();
            assert_eq!(synth.1["scriptText"], "Only this supplied content.");
            assert_eq!(synth.1["agents"], json!(["alice"]));
            assert!(synth.1.get("script").is_none());
            mcp_tool(&track.server.0,"fixture_tone",json!({})).unwrap();
            mcp_tool(&track.server.0,"list_engines",json!({})).unwrap();
            assert!(track.server.0.scratch.open_input("fixture-tone.wav").is_ok());
        }
        session_cancel(&agent,&json!({"sessionId":id})).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn session_and_prompt_limits() {
        let agent = Agent::new();
        assert!(initialize(&json!({"protocolVersion":"1"})).is_err());
        assert!(session_new(&agent,&json!({"cwd":".","agents":"alice"})).is_err());
        assert!(prompt_text(Some(&json!([{"type":"image","text":"x"}]))).is_err());
        assert!(prompt_text(Some(&json!([{"type":"text","text":"x".repeat(32_001)}]))).is_err());
        for _ in 0..32 { session_new(&agent,&json!({"cwd":"."})).unwrap(); }
        assert!(session_new(&agent,&json!({"cwd":"."})).unwrap_err().contains("capacity"));
    }
}
