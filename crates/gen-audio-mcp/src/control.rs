//! Process-local UI command bus.
//!
//! This is not an MCP session. `initialize` still stores nothing, HTTP still
//! omits `Mcp-Session-Id`, and a command does not write speech. The desktop
//! window reads the ring over `GET /control` or the short SSE stream.

use std::collections::VecDeque;
use std::sync::Mutex;

use gen_audio_core::catalog::{self, MAX_AGENTS_PER_TRACK};
use serde_json::{json, Value};

struct Event {
    seq: u64,
    body: Value,
}

struct Bus {
    next: u64,
    events: VecDeque<Event>,
}

const CAP: usize = 128;

static BUS: Mutex<Bus> = Mutex::new(Bus {
    next: 1,
    events: VecDeque::new(),
});

pub fn publish(op: &str, args: &Value) -> u64 {
    let mut bus = BUS.lock().expect("control bus");
    let seq = bus.next;
    bus.next = bus.next.saturating_add(1);
    bus.events.push_back(Event {
        seq,
        body: json!({
            "seq": seq,
            "op": op,
            "args": args,
            "synthesizedSpeech": false
        }),
    });
    while bus.events.len() > CAP {
        bus.events.pop_front();
    }
    seq
}

pub fn since(after: u64) -> (u64, Vec<Value>) {
    let bus = BUS.lock().expect("control bus");
    let events = bus
        .events
        .iter()
        .filter(|event| event.seq > after)
        .map(|event| event.body.clone())
        .collect();
    let cursor = bus.next.saturating_sub(1);
    (cursor, events)
}

fn queued(op: &str, args: Value, note: &str) -> Value {
    let seq = publish(op, &args);
    json!({
        "ok": true,
        "synthesizedSpeech": false,
        "op": op,
        "seq": seq,
        "args": args,
        "note": note,
        "stream": {"path": "/control/stream", "transport": "sse", "stateless": true}
    })
}

fn id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

pub fn ui_navigate(args: &Value) -> Result<Value, (i32, String)> {
    let slide = args
        .get("slide")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_navigate needs slide".to_string()))?;
    if !catalog::SLIDES.contains(&slide) {
        return Err((-32602, format!("unknown slide {slide}")));
    }
    if let Some(tile) = args.get("tileId").and_then(Value::as_str) {
        if !id_ok(tile) {
            return Err((-32602, "tileId is invalid".into()));
        }
    }
    Ok(queued(
        "navigate",
        args.clone(),
        "Queued a viewport navigation. This does not render audio.",
    ))
}

pub fn ui_select_tile(args: &Value) -> Result<Value, (i32, String)> {
    let tile = args
        .get("tileId")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_select_tile needs tileId".to_string()))?;
    if !id_ok(tile) {
        return Err((-32602, "tileId is invalid".into()));
    }
    Ok(queued(
        "select",
        args.clone(),
        "Queued a tile selection. A cube loads only when that tile has cube JSON.",
    ))
}

pub fn ui_playback(args: &Value) -> Result<Value, (i32, String)> {
    let tile = args
        .get("tileId")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_playback needs tileId".to_string()))?;
    if !id_ok(tile) {
        return Err((-32602, "tileId is invalid".into()));
    }
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_playback needs action".to_string()))?;
    if !matches!(action, "play" | "pause" | "seek") {
        return Err((-32602, "action must be play, pause, or seek".into()));
    }
    if action == "seek" {
        let seconds = args
            .get("seconds")
            .and_then(Value::as_f64)
            .ok_or((-32602, "seek needs seconds".to_string()))?;
        if !(0.0..=86_400.0).contains(&seconds) {
            return Err((-32602, "seconds out of range".into()));
        }
    }
    let clip = catalog::library_clip(tile);
    let has_wav = clip.and_then(|item| item.wav_url).is_some();
    let mut payload = queued(
        "playback",
        args.clone(),
        "Queued playback. The window plays a file only when that tile's WAV actually loads. Missing bytes stay missing.",
    );
    payload["catalogWav"] = json!(has_wav);
    payload["openedFile"] = json!(false);
    Ok(payload)
}

pub fn ui_set_sidepane(args: &Value) -> Result<Value, (i32, String)> {
    let agents = args
        .get("agents")
        .and_then(Value::as_array)
        .ok_or((-32602, "ui_set_sidepane needs agents".to_string()))?;
    if agents.is_empty() || agents.len() > MAX_AGENTS_PER_TRACK {
        return Err((-32602, "agents must contain 1 to 8 persona ids".into()));
    }
    let mut seen = std::collections::BTreeSet::new();
    for agent in agents {
        let id = agent
            .as_str()
            .ok_or((-32602, "agent id must be a string".to_string()))?;
        if catalog::is_connector_id(id) {
            return Err((
                -32602,
                format!("{id} is a connector, not an Agent. Agent is a persona such as alice or anton."),
            ));
        }
        if catalog::persona(id).is_none() {
            return Err((-32602, format!("unknown persona {id}")));
        }
        if !seen.insert(id) {
            return Err((-32602, format!("duplicate persona {id}")));
        }
    }
    let voice = args
        .get("voice")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_set_sidepane needs voice".to_string()))?;
    if matches!(voice, "af_heart" | "am_michael" | "alice" | "frank") {
        return Err((
            -32602,
            format!("{voice} is not a Voice model. af_heart and am_michael are Kokoro pack refs on personas Alice and Frank. Voice is kokoro_onnx, kokoro_dayour, misaki, vibevoice, magpie, or pocket_tts."),
        ));
    }
    let model = catalog::voice_model(voice).ok_or((-32602, format!("unknown voice model {voice}")))?;
    if let Some(minutes) = args.get("durationMin") {
        let minutes = minutes
            .as_u64()
            .ok_or((-32602, "durationMin must be an integer".to_string()))?;
        if !(1..=30).contains(&minutes) {
            return Err((-32602, "durationMin must be from 1 to 30".into()));
        }
    }
    let mut payload = queued(
        "sidepane",
        args.clone(),
        "Queued side pane Agent personas and Voice model. This does not synthesize.",
    );
    payload["voiceLabel"] = json!(model.label);
    payload["synthAdapter"] = json!(model.synth_adapter);
    payload["unavailable"] = json!(model.unavailable);
    Ok(payload)
}

pub fn ui_generate(args: &Value) -> Result<Value, (i32, String)> {
    let prompt_note = args.get("promptNote").and_then(Value::as_str).unwrap_or("");
    if prompt_note.chars().count() > 200 {
        return Err((-32602, "promptNote is too long".into()));
    }
    let side = ui_set_sidepane(args)?;
    let voice = args.get("voice").and_then(Value::as_str).unwrap_or("");
    let model = catalog::voice_model(voice);
    let adapter = model.map(|item| item.synth_adapter).unwrap_or(false);
    let seq = publish("generate", args);
    Ok(json!({
        "ok": true,
        "synthesizedSpeech": false,
        "synthesized": false,
        "op": "generate",
        "seq": seq,
        "args": args,
        "sidepane": side,
        "synthInvoked": false,
        "synthTool": if adapter { "synth" } else { "" },
        "note": if adapter {
            "Request recorded. Call the synth tool to run kokoro-onnx. This tool does not write audio. Unset model env vars refuse with no invented speech."
        } else {
            "Request recorded. This voice model has no synth adapter in this app. No audio was written."
        },
        "stream": {"path": "/control/stream", "transport": "sse", "stateless": true}
    }))
}

pub fn library_list() -> Value {
    json!({
        "ok": true,
        "synthesizedSpeech": false,
        "openedFiles": false,
        "clips": catalog::library_clips(),
        "note": "Catalog only. WAV bytes are not opened here. Unavailable engines stay unavailable."
    })
}

pub fn library_rename(args: &Value) -> Result<Value, (i32, String)> {
    let clip_id = args
        .get("clipId")
        .and_then(Value::as_str)
        .ok_or((-32602, "library_rename needs clipId".to_string()))?;
    if catalog::library_clip(clip_id).is_none() {
        return Err((-32602, format!("unknown library clip {clip_id}")));
    }
    let semantic = args.get("semanticName").and_then(Value::as_str);
    let face = args.get("faceName").and_then(Value::as_str);
    match (semantic, face) {
        (None, None) => return Err((-32602, "library_rename needs semanticName or faceName".into())),
        _ => {}
    }
    for (field, value) in [("semanticName", semantic), ("faceName", face)] {
        if let Some(text) = value {
            let len = text.chars().count();
            if !(1..=80).contains(&len) {
                return Err((-32602, format!("{field} length is out of range")));
            }
        }
    }
    Ok(queued(
        "rename",
        args.clone(),
        "Queued a display rename. The WAV file is not rewritten.",
    ))
}

pub fn voice_profile_get(args: &Value) -> Result<Value, (i32, String)> {
    let id = args
        .get("personaId")
        .or_else(|| args.get("agentName"))
        .and_then(Value::as_str)
        .ok_or((-32602, "voice_profile_get needs personaId".to_string()))?;
    let id = catalog::personas()
        .iter()
        .find(|person| person.id == id || person.name.eq_ignore_ascii_case(id))
        .map(|person| person.id)
        .ok_or((-32602, format!("unknown persona {id}")))?;
    let profile = catalog::voice_profile_value(id).ok_or((-32602, "profile missing".to_string()))?;
    Ok(profile)
}

pub fn voice_profile_list() -> Value {
    json!({
        "ok": true,
        "synthesizedSpeech": false,
        "maxAgentsPerTrack": MAX_AGENTS_PER_TRACK,
        "personas": catalog::personas(),
        "voiceModels": catalog::voice_models(),
        "connectorsAreNotAgents": ["copilot", "claude", "gpt", "gemini"],
        "packIdsAreNotVoiceModels": ["af_heart", "am_michael"],
        "note": "Agent is a persona. Voice is a TTS or G2P model. LLM ids stay on Connectors."
    })
}
