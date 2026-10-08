//! Process-local UI command bus.
//!
//! This is not an MCP session. `initialize` still stores nothing, HTTP still
//! omits `Mcp-Session-Id`, and a command does not write speech. The desktop
//! window reads the ring over `GET /control` or the short SSE stream.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gen_audio_core::asset_catalog;
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

/// UI tools accept an optional asset `uid`. It must resolve in the library
/// catalog, and when `tileId` is also given both must name the same tile.
fn with_uid_tile(args: &Value) -> Result<Value, (i32, String)> {
    let Some(uid) = args.get("uid") else {
        return Ok(args.clone());
    };
    let uid = uid.as_str().ok_or((-32602, "uid must be a string".to_string()))?;
    let asset = asset_catalog::require(uid).map_err(asset_catalog::CatalogError::rpc)?;
    let tile = asset_catalog::tile_id_for(asset)
        .ok_or((-32602, format!("{uid} is not shown as a tile")))?;
    let mut out = args.clone();
    match args.get("tileId").and_then(Value::as_str) {
        Some(given) if given != tile => {
            return Err((-32602, format!("uid {uid} names tile {tile}, not {given}")));
        }
        Some(_) => {}
        None => out["tileId"] = json!(tile),
    }
    Ok(out)
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

/// Resolve a `ui_navigate` slide reference (Optimus ruling C5). The canonical
/// form is `slide:<slug>` (the converged livetile object model's slide id);
/// a bare slug is a deprecated alias that still resolves. Returns the slug and
/// whether the deprecated alias was used.
pub fn resolve_slide(reference: &str) -> Result<(&str, bool), (i32, String)> {
    let (slug, deprecated) = match reference.strip_prefix("slide:") {
        Some(slug) => (slug, false),
        None => (reference, true),
    };
    if !catalog::SLIDES.contains(&slug) {
        return Err((-32602, format!("unknown slide {reference}; use slide:<slug>, one of {}", catalog::SLIDES.join(", "))));
    }
    Ok((slug, deprecated))
}

pub fn ui_navigate(args: &Value) -> Result<Value, (i32, String)> {
    let mut args = with_uid_tile(args)?;
    let reference = args
        .get("slide")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_navigate needs slide (slide:<slug>)".to_string()))?
        .to_string();
    let (slug, deprecated) = resolve_slide(&reference)?;
    let slug = slug.to_string();
    if let Some(tile) = args.get("tileId").and_then(Value::as_str) {
        if !id_ok(tile) {
            return Err((-32602, "tileId is invalid".into()));
        }
    }
    // The queued event always carries the canonical id; the UI accepts both.
    args["slide"] = json!(format!("slide:{slug}"));
    let mut out = queued("navigate", args, "Queued a viewport navigation. This does not render audio.");
    if deprecated {
        let warning = format!("ui_navigate slide \"{reference}\" is a deprecated alias; use \"slide:{slug}\"");
        eprintln!("gen-audio-mcp: deprecation: {warning}");
        out["deprecation"] = json!(warning);
    }
    Ok(out)
}

pub fn ui_select_tile(args: &Value) -> Result<Value, (i32, String)> {
    let args = &with_uid_tile(args)?;
    let tile = args
        .get("tileId")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_select_tile needs tileId or uid".to_string()))?;
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
    let args = &with_uid_tile(args)?;
    let tile = args
        .get("tileId")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_playback needs tileId or uid".to_string()))?;
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
    // Who started it (Optimus ruling on PR #5, C1): "user" (default; a UI
    // Play click posts it) or "auto". Carried on the bus as-is; focus and
    // rebind are the viewport reducer's job (PR #4), not this tool's.
    let mut args = args.clone();
    if action == "play" {
        let origin = args.get("origin").map_or(Some("user"), Value::as_str);
        match origin {
            Some(origin @ ("user" | "auto")) => args["origin"] = json!(origin),
            _ => return Err((-32602, "origin must be user or auto".into())),
        }
    } else if args.get("origin").is_some() {
        return Err((-32602, "origin applies to action play only".into()));
    }
    let args = &args;
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
    let args = &with_uid_tile(args)?;
    let tile = args
        .get("tileId")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_flip needs tileId or uid".to_string()))?;
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

/// Cube tab mode (rule 5: UI state lives in app state, readable and drivable
/// over MCP). `None` is single; `Some` is Compare: the clip's Library cube
/// (library_r3) beside the same WAV's comparison cube (e.g. pipeline_r2), one
/// playback slice. The desktop Compare button posts `ui_cube` too, so this is
/// the only place the mode changes, and `viewport_get` reads it from here.
///
/// The field is `cube_compare`, not `compare`: PR #4's viewport snapshot
/// already uses `compare` for its list of compared clip uids.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeCompare {
    pub tile_id: String,
    pub clip_uid: String,
    pub left_method: String,
    pub right_method: String,
    pub left_cube_uid: String,
    pub right_cube_uid: String,
}

struct CubeMode {
    compare: Option<CubeCompare>,
    seq: u64,
}

static CUBE_MODE: Mutex<CubeMode> = Mutex::new(CubeMode { compare: None, seq: 0 });

pub const CUBE_MODES: &[&str] = &["single", "compare"];

/// The state object: `ui_cube` queues it on the bus as the `cube` op's args,
/// and `viewport_get` returns the same fields.
pub fn cube_state_value(compare: Option<&CubeCompare>) -> Value {
    match compare {
        None => json!({"cube_mode": "single", "cube_compare": null}),
        Some(pair) => json!({
            "cube_mode": "compare",
            "cube_compare": {
                "tileId": pair.tile_id,
                "clip_uid": pair.clip_uid,
                "left_method": pair.left_method,
                "right_method": pair.right_method,
                "left_cube_uid": pair.left_cube_uid,
                "right_cube_uid": pair.right_cube_uid
            }
        }),
    }
}

/// THE setter for the Cube tab mode. Stores the state and publishes it on the
/// bus under one lock, so the store and the event stream never disagree.
fn set_cube_mode(next: Option<CubeCompare>) -> (u64, Value) {
    let mut mode = CUBE_MODE.lock().expect("cube mode");
    let state = cube_state_value(next.as_ref());
    let seq = publish("cube", &state);
    mode.compare = next;
    mode.seq = seq;
    (seq, state)
}

fn cube_method(asset: &Value) -> &str {
    asset
        .pointer("/provenance/params/layer_method")
        .and_then(Value::as_str)
        .unwrap_or("library_r3")
}

fn sha256_field(asset: &Value) -> Option<&str> {
    asset
        .pointer("/fields/source_sha256")
        .and_then(Value::as_str)
        .filter(|sha| sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// The clip's Library cube and its comparison cube from assets.json, or why
/// Compare cannot be entered. Same rules the window applies before drawing:
/// both are cube_ihdr envelopes of this clip only, the methods differ, and
/// their source_sha256 agree (same WAV). No stand-in cube.
pub fn compare_pair(tile: &str, clip_uid: &str) -> Result<CubeCompare, String> {
    let cubes: Vec<&Value> = asset_catalog::assets()
        .iter()
        .filter(|asset| asset.get("kind").and_then(Value::as_str) == Some("cube_ihdr"))
        .filter(|asset| {
            asset.get("src").and_then(Value::as_array).is_some_and(|src| src.len() == 1 && src[0].as_str() == Some(clip_uid))
        })
        .collect();
    let library = cubes
        .iter()
        .find(|asset| cube_method(asset) == "library_r3")
        .ok_or_else(|| format!("{tile} has no library_r3 cube in assets.json; Compare needs one"))?;
    let other = cubes
        .iter()
        .find(|asset| cube_method(asset) != "library_r3")
        .ok_or_else(|| format!("no comparison cube for {tile} in assets.json (only clips with a pipeline_r2 cube can enter Compare)"))?;
    match (sha256_field(library), sha256_field(other)) {
        (Some(left), Some(right)) if left == right => {}
        _ => return Err(format!("refusing Compare for {tile}: the two cubes do not record the same source_sha256")),
    }
    let uid = |asset: &Value| asset.get("uid").and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(CubeCompare {
        tile_id: tile.to_string(),
        clip_uid: clip_uid.to_string(),
        left_method: "library_r3".into(),
        right_method: cube_method(other).to_string(),
        left_cube_uid: uid(library),
        right_cube_uid: uid(other),
    })
}

/// `ui_cube {mode: "single"|"compare", tileId?, uid?}`. Compare names the clip
/// by tileId or uid (a clip uid, or either cube's uid); without one it keeps
/// the clip already in Compare. Unknown modes, clips without a comparison
/// cube, and uid/tileId clashes are rejected with -32602.
pub fn ui_cube(args: &Value) -> Result<Value, (i32, String)> {
    let mode = args
        .get("mode")
        .ok_or((-32602, "ui_cube needs mode (single or compare)".to_string()))?
        .as_str()
        .ok_or((-32602, "mode must be a string: single or compare".to_string()))?;
    if !CUBE_MODES.contains(&mode) {
        return Err((-32602, format!("unknown mode {mode:?}; mode must be single or compare")));
    }
    let args = with_uid_tile(args)?;
    let tile = match args.get("tileId") {
        None => None,
        Some(value) => {
            let tile = value.as_str().ok_or((-32602, "tileId must be a string".to_string()))?;
            if !id_ok(tile) {
                return Err((-32602, "tileId is invalid".into()));
            }
            Some(tile.to_string())
        }
    };
    let next = if mode == "single" {
        None
    } else {
        let tile = match tile {
            Some(tile) => tile,
            None => CUBE_MODE
                .lock()
                .expect("cube mode")
                .compare
                .as_ref()
                .map(|pair| pair.tile_id.clone())
                .ok_or((-32602, "ui_cube mode compare needs tileId or uid (no clip is in Compare yet)".to_string()))?,
        };
        let clip_uid = asset_catalog::uid_for_legacy("audio_clip", &tile)
            .ok_or((-32602, format!("{tile} is not a library audio clip in assets.json")))?;
        Some(compare_pair(&tile, clip_uid).map_err(|reason| (-32602, reason))?)
    };
    let (seq, state) = set_cube_mode(next);
    Ok(json!({
        "ok": true,
        "synthesizedSpeech": false,
        "op": "cube",
        "seq": seq,
        "args": state,
        "note": "Set the Cube tab mode. viewport_get reads the same state. This does not render audio.",
        "stream": {"path": "/control/stream", "transport": "sse", "stateless": true}
    }))
}

/// Read-only view of the UI state this process owns: the Cube tab mode.
pub fn viewport_get() -> Value {
    let mode = CUBE_MODE.lock().expect("cube mode");
    let mut out = cube_state_value(mode.compare.as_ref());
    out["ok"] = json!(true);
    out["synthesizedSpeech"] = json!(false);
    out["cube_seq"] = json!(mode.seq);
    out["cube_modes"] = json!(CUBE_MODES);
    out["note"] = json!("Cube tab mode as set by ui_cube (the desktop Compare button posts ui_cube too). cube_seq is the control-bus seq of the last change; 0 means never set.");
    out
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
        "/library/library_genaid_full_misaki_kokoro_cube3d.json" => {
            "apps/desktop/public/library/library_genaid_full_misaki_kokoro_cube3d.json"
        }
        "/library/library_cube_explainer_kokoro_onnx_cube3d.json" => {
            "apps/desktop/public/library/library_cube_explainer_kokoro_onnx_cube3d.json"
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_track_and_voice_profile_get_accept_optimus() {
        validate_track(&[json!("optimus")], "kokoro_onnx").expect("optimus track");
        let profile = voice_profile_get(&json!({"personaId": "optimus"})).expect("profile");
        assert_eq!(profile["personaId"], "optimus");
        assert_eq!(profile["notPodcast"], true);
        assert_eq!(profile["synthesizedSpeech"], false);
        assert!(profile.get("wavUrl").is_none());
        assert!(validate_track(&[json!("not-a-persona")], "kokoro_onnx").is_err());
    }

    #[test]
    fn ui_navigate_takes_slide_ids_and_keeps_bare_names_as_deprecated_aliases() {
        let canonical = ui_navigate(&json!({"slide": "slide:library"})).expect("slide:library");
        assert_eq!(canonical["args"]["slide"], "slide:library");
        assert!(canonical.get("deprecation").is_none());
        let alias = ui_navigate(&json!({"slide": "library"})).expect("bare alias still resolves");
        assert_eq!(alias["args"]["slide"], "slide:library", "the alias is rewritten to the canonical id");
        assert!(alias["deprecation"].as_str().unwrap().contains("slide:library"));
        assert_eq!(ui_navigate(&json!({"slide": "slide:nope"})).unwrap_err().0, -32602);
        assert_eq!(ui_navigate(&json!({"slide": "slide:"})).unwrap_err().0, -32602);
        assert_eq!(resolve_slide("slide:spatial").unwrap(), ("spatial", false));
    }

    #[test]
    fn ui_playback_carries_play_origin_user_by_default() {
        let user = ui_playback(&json!({"tileId": "lib-kokoro", "action": "play", "origin": "user"})).unwrap();
        assert_eq!(user["args"]["origin"], "user");
        let default = ui_playback(&json!({"tileId": "lib-kokoro", "action": "play"})).unwrap();
        assert_eq!(default["args"]["origin"], "user");
        let auto = ui_playback(&json!({"tileId": "lib-kokoro", "action": "play", "origin": "auto"})).unwrap();
        assert_eq!(auto["args"]["origin"], "auto");
        assert_eq!(ui_playback(&json!({"tileId": "lib-kokoro", "action": "play", "origin": "mcp"})).unwrap_err().0, -32602);
        assert_eq!(ui_playback(&json!({"tileId": "lib-kokoro", "action": "pause", "origin": "user"})).unwrap_err().0, -32602);
        assert!(ui_playback(&json!({"tileId": "lib-kokoro", "action": "pause"})).unwrap()["args"].get("origin").is_none());
    }

    /// Stateless half of the ui_cube contract (the stateful round trip, with
    /// viewport_get, is one test in lib.rs so parallel tests cannot race it).
    #[test]
    fn ui_cube_rejects_unknown_modes_and_clips_without_a_comparison_cube() {
        let err = |args: Value| ui_cube(&args).unwrap_err();
        assert_eq!(err(json!({})), (-32602, "ui_cube needs mode (single or compare)".to_string()));
        assert_eq!(err(json!({"mode": 1})).0, -32602);
        let unknown = err(json!({"mode": "side-by-side", "tileId": "lib-misaki-kokoro"}));
        assert_eq!(unknown.0, -32602);
        assert!(unknown.1.contains("single or compare"), "{}", unknown.1);
        assert!(err(json!({"mode": "Compare", "tileId": "lib-misaki-kokoro"})).1.contains("unknown mode"), "modes are case-sensitive");
        // kokoro-onnx has a Library cube but no pipeline_r2 cube: no stand-in.
        let lonely = err(json!({"mode": "compare", "tileId": "lib-kokoro-onnx"}));
        assert!(lonely.1.contains("no comparison cube for lib-kokoro-onnx"), "{}", lonely.1);
        assert!(err(json!({"mode": "compare", "tileId": "lib-magpie"})).1.contains("not a library audio clip"));
        assert_eq!(err(json!({"mode": "compare", "tileId": "../etc"})).1, "tileId is invalid");
        let misaki = asset_catalog::uid_for_legacy("audio_clip", "lib-misaki-kokoro").unwrap();
        let clash = err(json!({"mode": "compare", "tileId": "lib-kokoro", "uid": misaki}));
        assert!(clash.1.contains("not lib-kokoro"), "{}", clash.1);
        assert_eq!(err(json!({"mode": "compare", "uid": "ga:cube_ihdr:aaaaaaaaaaaaaaaaaaaaaaaaaa"})).0, -32602);
    }

    #[test]
    fn compare_pair_reads_both_cubes_of_one_wav_from_assets_json() {
        let contract: Value = serde_json::from_str(include_str!("../../../schemas/examples/ui_cube.contract.json")).unwrap();
        let want = &contract["enter"]["state"]["cube_compare"];
        let clip = asset_catalog::uid_for_legacy("audio_clip", "lib-misaki-kokoro").unwrap();
        let pair = compare_pair("lib-misaki-kokoro", clip).unwrap();
        assert_eq!(cube_state_value(Some(&pair))["cube_compare"], *want);
        assert_eq!(cube_state_value(None), contract["exit"]["state"]);
        let bitdot = asset_catalog::uid_for_legacy("audio_clip", "lib-bitdot-braille-vibevoice").unwrap();
        let pair = compare_pair("lib-bitdot-braille-vibevoice", bitdot).unwrap();
        assert_eq!((pair.left_method.as_str(), pair.right_method.as_str()), ("library_r3", "pipeline_r2"));
        assert_ne!(pair.left_cube_uid, pair.right_cube_uid);
    }
}
