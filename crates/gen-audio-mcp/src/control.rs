//! Process-local UI command bus.
//!
//! This is not an MCP session. `initialize` still stores nothing, HTTP still
//! omits `Mcp-Session-Id`, and a command does not write speech. The desktop
//! window reads the ring over `GET /control` or the short SSE stream.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
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

static ATTACHED_REFS: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

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

pub fn note_progress(voice: &str, phase: &str, detail: &str, synthesized: bool) -> Value {
    let phase = if synthesized {
        "ok"
    } else if phase == "ok" {
        "refused"
    } else {
        phase
    };
    let args = json!({
        "voice": voice,
        "phase": phase,
        "detail": detail,
        "synthesizedSpeech": synthesized
    });
    let seq = publish("progress", &args);
    json!({
        "seq": seq,
        "op": "progress",
        "phase": phase,
        "voice": voice,
        "synthesizedSpeech": synthesized,
        "detail": detail
    })
}

pub fn validate_track(agents: &[Value], voice: &str) -> Result<(), (i32, String)> {
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
    if matches!(voice, "af_heart" | "am_michael" | "alice" | "frank") {
        return Err((
            -32602,
            format!("{voice} is not a Voice model. af_heart and am_michael are Kokoro pack refs on personas Alice and Frank. Voice is kokoro_onnx, kokoro_dayour, misaki, vibevoice, magpie, or pocket_tts."),
        ));
    }
    catalog::voice_model(voice).ok_or((-32602, format!("unknown voice model {voice}")))?;
    Ok(())
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

pub fn ui_flip(args: &Value) -> Result<Value, (i32, String)> {
    let tile = args
        .get("tileId")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_flip needs tileId".to_string()))?;
    if !id_ok(tile) {
        return Err((-32602, "tileId is invalid".into()));
    }
    let flipped = args.get("flipped").and_then(Value::as_bool).unwrap_or(true);
    let mut payload = json!({"tileId": tile, "flipped": flipped});
    if let Some(persona) = args.get("personaId").and_then(Value::as_str) {
        if catalog::persona(persona).is_none() {
            return Err((-32602, format!("unknown persona {persona}")));
        }
        payload["personaId"] = json!(persona);
        payload["profile"] = voice_profile_get(&json!({"personaId": persona}))?;
    }
    Ok(queued(
        "flip",
        payload,
        "Queued a flipcard. The payload is the voice profile when a persona is named. This does not render audio.",
    ))
}

pub fn ui_set_sidepane(args: &Value) -> Result<Value, (i32, String)> {
    let agents = args
        .get("agents")
        .and_then(Value::as_array)
        .ok_or((-32602, "ui_set_sidepane needs agents".to_string()))?;
    let voice = args
        .get("voice")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_set_sidepane needs voice".to_string()))?;
    validate_track(agents, voice)?;
    let model = catalog::voice_model(voice).expect("voice checked");
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
    let phase = if adapter { "running" } else { "unavailable" };
    let detail = if adapter {
        "Request recorded. Synth has not written audio. Unset model env vars refuse with no invented speech."
    } else {
        "Request recorded. This voice model has no synth adapter. No audio was written."
    };
    let progress = note_progress(voice, phase, detail, false);
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
        "phase": phase,
        "progress": progress,
        "note": detail,
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
    let mut profile = catalog::voice_profile_value(id).ok_or((-32602, "profile missing".to_string()))?;
    let attached = ATTACHED_REFS.lock().expect("refs").get(id).cloned().unwrap_or_default();
    if !attached.is_empty() {
        let refs = profile
            .get_mut("refs")
            .and_then(Value::as_array_mut)
            .ok_or((-32603, "profile refs missing".to_string()))?;
        for reference in attached {
            if !refs.iter().any(|item| item.as_str() == Some(reference.as_str())) {
                refs.push(Value::String(reference));
            }
        }
    }
    Ok(profile)
}

pub fn library_harvest(repo: Option<&Path>, args: &Value) -> Result<Value, (i32, String)> {
    let clip_id = args
        .get("clipId")
        .and_then(Value::as_str)
        .ok_or((-32602, "library_harvest needs clipId".to_string()))?;
    let clip = catalog::library_clip(clip_id).ok_or((-32602, format!("unknown library clip {clip_id}")))?;
    let apply = args.get("apply").and_then(Value::as_bool).unwrap_or(false);
    let persona_id = args.get("personaId").and_then(Value::as_str);
    if apply && persona_id.is_none() {
        return Err((-32602, "library_harvest apply needs personaId".into()));
    }
    if let Some(persona_id) = persona_id {
        if catalog::persona(persona_id).is_none() {
            return Err((-32602, format!("unknown persona {persona_id}")));
        }
    }
    let (semantic, face, source) = propose_names(repo, clip);
    let reference = format!("clip:{clip_id}");
    let attached = false;
    if apply {
        let persona_id = persona_id.expect("checked");
        attach_ref(persona_id, &reference)?;
        let _ = library_rename(&json!({
            "clipId": clip_id,
            "semanticName": semantic,
            "faceName": face
        }))?;
        let profile = voice_profile_get(&json!({"personaId": persona_id}))?;
        let _ = publish(
            "flipcard",
            &json!({
                "personaId": persona_id,
                "tileId": format!("profile-{persona_id}"),
                "profile": profile
            }),
        );
        return Ok(json!({
            "ok": true,
            "synthesizedSpeech": false,
            "decodedAudio": false,
            "openedWav": false,
            "clipId": clip_id,
            "semanticName": semantic,
            "faceName": face,
            "source": source,
            "ref": reference,
            "attached": true,
            "personaId": persona_id,
            "profile": profile,
            "note": "Names come from the catalog filename and sidecar counts. The WAV was not decoded and was not rewritten."
        }));
    }
    Ok(json!({
        "ok": true,
        "synthesizedSpeech": false,
        "decodedAudio": false,
        "openedWav": false,
        "clipId": clip_id,
        "semanticName": semantic,
        "faceName": face,
        "source": source,
        "ref": reference,
        "attached": attached,
        "personaId": persona_id,
        "note": "Names come from the catalog filename and sidecar counts. The WAV was not decoded and was not rewritten."
    }))
}

pub fn cube_layers(repo: Option<&Path>, args: &Value) -> Result<Value, (i32, String)> {
    let clip_id = args
        .get("clipId")
        .and_then(Value::as_str)
        .ok_or((-32602, "cube_layers needs clipId".to_string()))?;
    let clip = catalog::library_clip(clip_id).ok_or((-32602, format!("unknown library clip {clip_id}")))?;
    let Some(web_path) = clip.cube_json_url else {
        return Ok(json!({
            "ok": false,
            "synthesizedSpeech": false,
            "phase": "unavailable",
            "clipId": clip_id,
            "layers": [],
            "note": "This clip has no cube JSON. No stand-in cloud was invented."
        }));
    };
    let Some(repo) = repo else {
        return Err((-32603, "repository root not found".into()));
    };
    let path = allowlisted_library_file(repo, web_path)
        .ok_or((-32603, "cube JSON is not an allowlisted library file".to_string()))?;
    let text = std::fs::read_to_string(&path).map_err(|err| (-32603, err.to_string()))?;
    let value: Value = serde_json::from_str(&text).map_err(|err| (-32603, err.to_string()))?;
    let layers_in = value.get("layers").and_then(Value::as_object).ok_or((-32603, "cube JSON has no layers".to_string()))?;
    let mut layers = Vec::new();
    for (name, body) in layers_in {
        let obj = body.as_object();
        layers.push(json!({
            "name": name,
            "mean": obj.and_then(|item| item.get("mean")).cloned().unwrap_or(Value::Null),
            "std": obj.and_then(|item| item.get("std")).cloned().unwrap_or(Value::Null),
            "p50": obj.and_then(|item| item.get("p50")).cloned().unwrap_or(Value::Null),
            "p90": obj.and_then(|item| item.get("p90")).cloned().unwrap_or(Value::Null),
            "active_frac": obj.and_then(|item| item.get("active_frac")).cloned().unwrap_or(Value::Null)
        }));
    }
    let points = value.get("points_preview").and_then(Value::as_array).map(|rows| rows.len()).unwrap_or(0);
    Ok(json!({
        "ok": true,
        "synthesizedSpeech": false,
        "clipId": clip_id,
        "engine": value.get("engine").and_then(Value::as_str).unwrap_or(clip.engine_id),
        "inv_hdr": value.get("inv_hdr").cloned().unwrap_or(Value::Null),
        "pointPreviewCount": points,
        "layers": layers,
        "absolutePathsOmitted": true,
        "notMasteringGrade": true,
        "note": "Layer summary from the library cube JSON. Point coordinates and machine paths are omitted."
    }))
}

fn attach_ref(persona_id: &str, reference: &str) -> Result<(), (i32, String)> {
    if reference.len() > 80
        || reference.is_empty()
        || !reference
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == ':' || ch == '-' || ch == '_')
    {
        return Err((-32602, "profile ref is invalid".into()));
    }
    let base = catalog::persona(persona_id).map(|person| person.refs.len()).unwrap_or(0);
    let mut map = ATTACHED_REFS.lock().expect("refs");
    let list = map.entry(persona_id.to_string()).or_default();
    if list.iter().any(|item| item == reference) {
        return Ok(());
    }
    if base + list.len() >= 8 {
        return Err((-32602, "profile refs are capped at 8".into()));
    }
    list.push(reference.to_string());
    Ok(())
}

fn propose_names(repo: Option<&Path>, clip: &catalog::LibraryClipMeta) -> (String, String, &'static str) {
    let face = clip
        .wav_url
        .and_then(|url| url.rsplit('/').next())
        .map(|file| file.trim_end_matches(".wav").replace(['_', '-'], " "))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| clip.title.to_string());
    let face = trim_chars(&face, 80);
    if let Some(web_path) = clip.sidecar_url {
        if let Some(repo) = repo {
            if let Some(path) = allowlisted_library_file(repo, web_path) {
                if let Some(semantic) = sidecar_counts(&path, clip.engine_id) {
                    return (trim_chars(&semantic, 80), face, "sidecar-counts");
                }
            }
        }
    }
    (trim_chars(clip.title, 80), face, "catalog")
}

fn sidecar_counts(path: &Path, engine_id: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if value.get("engine").and_then(Value::as_str) != Some(engine_id) {
        return None;
    }
    let turns = value.get("n_turns").and_then(Value::as_u64)?;
    let words = value.get("n_words").and_then(Value::as_u64)?;
    Some(format!("{engine_id} · {turns} turns · {words} words"))
}

fn trim_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn allowlisted_library_file(repo: &Path, web_path: &str) -> Option<PathBuf> {
    let rel = match web_path {
        "/library/library_kokoro_onnx.synth.json" => "apps/desktop/public/library/library_kokoro_onnx.synth.json",
        "/library/library_kokoro_onnx_cube3d.json" => "apps/desktop/public/library/library_kokoro_onnx_cube3d.json",
        _ => return None,
    };
    let root = repo.canonicalize().ok()?;
    let path = root.join(rel).canonicalize().ok()?;
    if path.starts_with(&root) { Some(path) } else { None }
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
