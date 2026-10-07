//! A small Agent Client Protocol (ACP) agent.
//!
//! ACP itself is session-scoped: `session/new` returns a `sessionId`. That is
//! not an MCP session. This process keeps the map only in memory for the
//! stdio lifetime and does not pin a client to a Serve node.

use std::collections::HashMap;
use std::sync::Mutex;

use gen_audio_connectors::health_all;
use serde_json::{json, Value};

pub struct Agent {
    sessions: Mutex<HashMap<String, String>>,
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
    let mut next = agent.next.lock().expect("session counter");
    let id = format!("gen-audio-session-{next}");
    *next += 1;
    drop(next);
    agent
        .sessions
        .lock()
        .expect("sessions")
        .insert(id.clone(), cwd.to_string());
    Ok(json!({"sessionId": id}))
}

fn session_prompt(agent: &Agent, params: &Value) -> Result<(Value, Vec<Value>), String> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("session/prompt requires sessionId")?;
    if !agent.sessions.lock().expect("sessions").contains_key(session_id) {
        return Err("unknown sessionId".into());
    }
    let prompt = prompt_text(params.get("prompt"))?;
    let summary = if prompt.to_ascii_lowercase().contains("health") || prompt == "status" {
        let lines: Vec<String> = health_all()
            .into_iter()
            .map(|item| format!("{}={:?}", item.connector_id, item.mode))
            .collect();
        format!(
            "Gen-Audio connector health (local report, not a vendor completion): {}",
            lines.join(", ")
        )
    } else {
        format!(
            "Gen-Audio ACP agent received {} characters. It does not synthesize speech and it does not call a vendor model from this prompt.",
            prompt.chars().count()
        )
    };
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
    Ok((json!({"stopReason": "end_turn"}), vec![note]))
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
}
