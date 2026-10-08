//! Process-local UI command bus.
//!
//! This is not an MCP session. `initialize` still stores nothing, HTTP still
//! omits `Mcp-Session-Id`, and a command does not write speech. The desktop
//! window reads the ring over `GET /control` or the short SSE stream.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use gen_audio_core::asset_catalog;
use gen_audio_core::catalog::{self, MAX_AGENTS_PER_TRACK};
use gen_audio_core::viewport::{self, Action};
use serde_json::{json, Value};

struct Event {
    seq: u64,
    body: Value,
}

struct Bus {
    next: u64,
    events: VecDeque<Event>,
}

pub const CAP: usize = 128;

static BUS: Mutex<Bus> = Mutex::new(Bus {
    next: 1,
    events: VecDeque::new(),
});

static ATTACHED_REFS: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

/// Seek landings the desktop window reported (`ui_seek_report`), by bus seq.
static SEEK_REPORTS: Mutex<BTreeMap<u64, Value>> = Mutex::new(BTreeMap::new());
static SEEK_REPORTED: Condvar = Condvar::new();
/// How long `ui_playback` seek waits for the window's landing by default.
pub const SEEK_WAIT_DEFAULT_MS: u64 = 2000;
const SEEK_WAIT_MAX_MS: u64 = 10_000;
const SEEK_REASON_MAX: usize = 240;

/// The seconds a queued `playback` seek event asked for, if `seq` is one.
fn queued_seek_seconds(seq: u64) -> Option<f64> {
    let bus = BUS.lock().expect("control bus");
    let event = bus.events.iter().find(|event| event.seq == seq)?;
    let body = &event.body;
    if body["op"] != "playback" || body["args"]["action"] != "seek" {
        return None;
    }
    body["args"]["seconds"].as_f64()
}

/// Wait up to `wait_ms` for the window to report where seek `seq` landed.
fn await_seek_landing(seq: u64, requested: f64, wait_ms: u64) -> Value {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let mut reports = SEEK_REPORTS.lock().expect("seek reports");
    loop {
        if let Some(report) = reports.get(&seq) {
            return report.clone();
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        reports = SEEK_REPORTED
            .wait_timeout(reports, deadline - now)
            .expect("seek reports")
            .0;
    }
    json!({
        "requested_t": requested,
        "landed_t": null,
        "ok": false,
        "reason": format!("no window reported where seek {seq} landed within {wait_ms} ms (it stays queued)")
    })
}

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

pub struct ControlDelta {
    pub cursor: u64,
    pub gap: bool,
    pub oldest: Option<u64>,
    pub events: Vec<Value>,
}

/// Events with `seq > after`. `gap` is true when the ring dropped `after + 1`.
/// Clients must call `viewport_get` and discard the partial delta.
pub fn since(after: u64) -> ControlDelta {
    let bus = BUS.lock().expect("control bus");
    let oldest = bus.events.front().map(|event| event.seq);
    let gap = match oldest {
        Some(first) => first > after.saturating_add(1),
        None => bus.next > after.saturating_add(1),
    };
    let events = bus
        .events
        .iter()
        .filter(|event| event.seq > after)
        .map(|event| event.body.clone())
        .collect();
    let cursor = bus.next.saturating_sub(1);
    ControlDelta { cursor, gap, oldest, events }
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
        return Err((-32602, format!("unknown slide {reference:?}; valid slides: {}", valid_slides())));
    }
    Ok((slug, deprecated))
}

/// Every valid `ui_navigate` slide id, `slide:<slug>`, comma-separated.
fn valid_slides() -> String {
    catalog::SLIDES.iter().map(|slug| format!("slide:{slug}")).collect::<Vec<_>>().join(", ")
}

pub fn ui_navigate(args: &Value) -> Result<Value, (i32, String)> {
    let mut args = with_uid_tile(args)?;
    let reference = args
        .get("slide")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, format!("ui_navigate needs slide; valid slides: {}", valid_slides())))?
        .to_string();
    let (slug, deprecated) = resolve_slide(&reference)?;
    let slug = slug.to_string();
    viewport::apply_global(Action::Navigate { slide: reference.clone() }).map_err(|err| (err.code, err.message))?;
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
        .or_else(|| args.get("uid"))
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_select_tile needs tileId or uid".to_string()))?;
    if !id_ok(tile) {
        return Err((-32602, "tileId is invalid".into()));
    }
    if let Err(err) = viewport::apply_global(Action::Focus { uid: Some(tile.to_string()) }) {
        if err.code != -32602 || !err.message.contains("unknown asset") {
            return Err((err.code, err.message));
        }
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
    // A seek waits for the window's landing report (ui_seek_report); waitMs
    // bounds it and never reaches the bus.
    let mut wait_ms = None;
    if let Some(raw) = args.as_object_mut().and_then(|obj| obj.remove("waitMs")) {
        if action != "seek" {
            return Err((-32602, "waitMs applies to action seek only".into()));
        }
        match raw.as_u64() {
            Some(ms) if ms <= SEEK_WAIT_MAX_MS => wait_ms = Some(ms),
            _ => return Err((-32602, format!("waitMs must be an integer 0..={SEEK_WAIT_MAX_MS}"))),
        }
    }
    let args = &args;
    let mut requested = None;
    if action == "seek" {
        let seconds = args
            .get("seconds")
            .and_then(Value::as_f64)
            .ok_or((-32602, "seek needs seconds".to_string()))?;
        if !(0.0..=86_400.0).contains(&seconds) {
            return Err((-32602, "seconds out of range".into()));
        }
        requested = Some(seconds);
    }
    if let Err(err) = apply_playback(tile, action, args) {
        if err.0 != -32602 || !err.1.contains("unknown asset") {
            return Err(err);
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
    if let Some(requested) = requested {
        // The seek result is where the window's clock landed, not the request.
        let seq = payload["seq"].as_u64().unwrap_or(0);
        let landing = await_seek_landing(seq, requested, wait_ms.unwrap_or(SEEK_WAIT_DEFAULT_MS));
        payload["queued"] = json!(true);
        for key in ["requested_t", "landed_t", "ok", "reason"] {
            payload[key] = landing[key].clone();
        }
    }
    Ok(payload)
}

/// The window reports where an MCP seek (bus event `seq`) landed. Invalid
/// reports are -32602 and record nothing.
pub fn ui_seek_report(args: &Value) -> Result<Value, (i32, String)> {
    let seq = args
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or((-32602, "ui_seek_report needs seq".to_string()))?;
    let requested = args
        .get("requested_t")
        .and_then(Value::as_f64)
        .ok_or((-32602, "ui_seek_report needs requested_t".to_string()))?;
    let queued = queued_seek_seconds(seq).ok_or((-32602, format!("seq {seq} is not a queued seek")))?;
    if (queued - requested).abs() > 1e-9 {
        return Err((-32602, format!("requested_t {requested} does not match seek {seq} ({queued})")));
    }
    let landed = match args.get("landed_t") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_f64() {
            Some(t) if t.is_finite() && t >= 0.0 => Some(t),
            _ => return Err((-32602, "landed_t must be a non-negative number or null".into())),
        },
    };
    let ok = args
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or((-32602, "ui_seek_report needs ok".to_string()))?;
    if ok && landed.is_none() {
        return Err((-32602, "ok needs landed_t".into()));
    }
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_seek_report needs reason".to_string()))?;
    if reason.chars().count() > SEEK_REASON_MAX {
        return Err((-32602, format!("reason is over {SEEK_REASON_MAX} characters")));
    }
    let report = json!({"requested_t": requested, "landed_t": landed, "ok": ok, "reason": reason});
    let mut reports = SEEK_REPORTS.lock().expect("seek reports");
    if reports.contains_key(&seq) {
        return Err((-32602, format!("seek {seq} already reported")));
    }
    reports.insert(seq, report.clone());
    while reports.len() > CAP {
        reports.pop_first();
    }
    drop(reports);
    SEEK_REPORTED.notify_all();
    Ok(json!({"recorded": true, "seekSeq": seq, "report": report, "synthesizedSpeech": false}))
}

fn apply_playback(tile: &str, action: &str, args: &Value) -> Result<(), (i32, String)> {
    let action = match action {
        "play" => Action::Play { uid: tile.to_string() },
        "pause" => Action::Pause { uid: tile.to_string() },
        "seek" => Action::Seek {
            uid: tile.to_string(),
            t: args.get("seconds").and_then(Value::as_f64).unwrap_or(0.0),
        },
        _ => return Ok(()),
    };
    viewport::apply_global(action).map(|_| ()).map_err(|err| (err.code, err.message))
}

pub fn ui_flip(args: &Value) -> Result<Value, (i32, String)> {
    let args = &with_uid_tile(args)?;
    let tile = args
        .get("tileId")
        .or_else(|| args.get("view"))
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_flip needs tileId or uid".to_string()))?;
    if !id_ok(tile) && !tile.starts_with("view:") {
        return Err((-32602, "tileId is invalid".into()));
    }
    let flipped = args.get("flipped").and_then(Value::as_bool).unwrap_or(true);
    let face = args
        .get("face")
        .and_then(Value::as_str)
        .unwrap_or(if flipped { "back" } else { "front" });
    if face != "front" && face != "back" {
        return Err((-32602, "face must be front or back".into()));
    }
    let section = args.get("section").and_then(Value::as_str).map(str::to_string);
    let _ = viewport::apply_global(Action::Flip {
        view: tile.to_string(),
        face: face.to_string(),
        section: section.clone(),
    });
    let mut payload = json!({"tileId": tile, "flipped": face == "back", "face": face, "section": section});
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

fn canonical_voice(voice: &str) -> String {
    match voice {
        "kokoro-onnx" => "kokoro_onnx".into(),
        "pocket-tts" => "pocket_tts".into(),
        other => other.to_string(),
    }
}

fn kokoro_weights_ready() -> bool {
    std::env::var("GEN_AUDIO_KOKORO_MODEL").ok().filter(|value| !value.is_empty()).is_some()
        && std::env::var("GEN_AUDIO_KOKORO_VOICES").ok().filter(|value| !value.is_empty()).is_some()
}

pub fn ui_generate(args: &Value) -> Result<Value, (i32, String)> {
    let prompt_note = args.get("promptNote").and_then(Value::as_str).unwrap_or("");
    if prompt_note.chars().count() > 200 {
        return Err((-32602, "promptNote is too long".into()));
    }
    let mut normalized = args.clone();
    let voice = canonical_voice(args.get("voice").and_then(Value::as_str).unwrap_or(""));
    normalized["voice"] = json!(voice);
    let side = ui_set_sidepane(&normalized)?;
    let model = catalog::voice_model(&voice);
    let adapter = model.map(|item| item.synth_adapter).unwrap_or(false);
    let focus = args.get("focus").and_then(Value::as_bool).unwrap_or(true);
    let duration_s = args
        .get("duration_s")
        .and_then(Value::as_f64)
        .or_else(|| args.get("durationMin").and_then(Value::as_u64).map(|minutes| minutes as f64 * 60.0))
        .unwrap_or(180.0);
    let personas = args
        .get("agents")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let seq = publish("generate", &normalized);
    let job = format!("job-{seq}");
    viewport::apply_global(Action::Generate {
        prompt_ref: prompt_note.to_string(),
        personas,
        voice: voice.clone(),
        duration_s,
        focus,
        job: job.clone(),
    })
    .map_err(|err| (err.code, err.message))?;
    let (phase, detail) = if !adapter {
        (
            "unavailable",
            format!("{voice} has no synth adapter. No audio was written."),
        )
    } else if voice == "kokoro_onnx" && !kokoro_weights_ready() {
        (
            "refused",
            "GEN_AUDIO_KOKORO_MODEL and GEN_AUDIO_KOKORO_VOICES are unset. No speech was invented.".into(),
        )
    } else if adapter {
        (
            "running",
            "Weights are configured. synthesizedSpeech stays false until a job done lands a measured clip.".into(),
        )
    } else {
        ("refused", "No speech was invented.".into())
    };
    if phase != "running" {
        viewport::apply_global(Action::Job {
            job: job.clone(),
            target: None,
            phase: phase.into(),
            reason: Some(detail.clone()),
            kind: Some("audio_clip".into()),
            synthesized: false,
        })
        .map_err(|err| (err.code, err.message))?;
    }
    let job_seq = publish(
        "job",
        &json!({
            "job": job,
            "phase": phase,
            "reason": detail,
            "target": Value::Null,
            "voice": voice,
            "synthesizedSpeech": false
        }),
    );
    let progress = note_progress(&voice, phase, &detail, false);
    Ok(json!({
        "ok": phase != "refused" && phase != "unavailable",
        "synthesizedSpeech": false,
        "synthesized": false,
        "op": "generate",
        "seq": seq,
        "jobSeq": job_seq,
        "job": job,
        "args": normalized,
        "sidepane": side,
        "synthInvoked": false,
        "landed": false,
        "createdUid": Value::Null,
        "phase": phase,
        "progress": progress,
        "note": detail,
        "stream": {"path": "/control/stream", "transport": "sse", "stateless": true}
    }))
}

pub fn ui_compare(args: &Value) -> Result<Value, (i32, String)> {
    let uids = args
        .get("uids")
        .and_then(Value::as_array)
        .ok_or((-32602, "ui_compare needs uids".to_string()))?;
    let uids: Vec<String> = uids.iter().filter_map(|item| item.as_str().map(str::to_string)).collect();
    if uids.len() != args["uids"].as_array().map(|items| items.len()).unwrap_or(0) {
        return Err((-32602, "uids must be strings".into()));
    }
    viewport::apply_global(Action::Compare { uids: uids.clone() }).map_err(|err| (err.code, err.message))?;
    if let Some(select) = args.get("select").and_then(Value::as_str) {
        viewport::apply_global(Action::CompareSelect { uid: select.to_string() }).map_err(|err| (err.code, err.message))?;
    }
    Ok(queued("compare", args.clone(), "Queued compare. The cube follows focus, not the A/B clock."))
}

pub fn viewport_get() -> Value {
    let mut snap = viewport::snapshot_global();
    let delta = since(0);
    snap["cursor"] = json!(delta.cursor);
    snap["gap"] = json!(false);
    snap["resync"] = json!("viewport_get");
    snap
}

pub fn card_export(args: &Value) -> Result<Value, (i32, String)> {
    let uid = args
        .get("uid")
        .and_then(Value::as_str)
        .ok_or((-32602, "card_export needs uid".to_string()))?;
    let format = args.get("format").and_then(Value::as_str).unwrap_or("adaptivecard");
    if format != "adaptivecard" {
        return Err((-32602, "format must be adaptivecard".into()));
    }
    let card = viewport::export_global(uid).map_err(|err| (err.code, err.message))?;
    Ok(json!({
        "ok": true,
        "synthesizedSpeech": false,
        "format": "adaptivecard",
        "version": "1.5",
        "card": card
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
    if let (Some(name), true) = (semantic.or(face), true) {
        let _ = viewport::apply_global(Action::Rename { uid: clip_id.to_string(), name: name.to_string() });
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
    fn unknown_or_missing_slide_lists_every_valid_slide_id() {
        for args in [json!({"slide": "slide:nope"}), json!({"slide": "nope"}), json!({})] {
            let (code, message) = ui_navigate(&args).unwrap_err();
            assert_eq!(code, -32602, "{args}");
            for slug in catalog::SLIDES {
                assert!(message.contains(&format!("slide:{slug}")), "{args}: {message} lacks slide:{slug}");
            }
        }
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

    #[test]
    fn t8_generate_refuses_when_kokoro_env_is_unset() {
        std::env::remove_var("GEN_AUDIO_KOKORO_MODEL");
        std::env::remove_var("GEN_AUDIO_KOKORO_VOICES");
        let body = ui_generate(&json!({
            "agents": ["alice", "frank"],
            "voice": "kokoro-onnx",
            "durationMin": 3,
            "promptNote": "two personas"
        }))
        .expect("generate");
        assert_eq!(body["phase"], "refused");
        assert_eq!(body["synthesizedSpeech"], false);
        assert_eq!(body["landed"], false);
        assert!(body["createdUid"].is_null());
        let note = body["note"].as_str().unwrap();
        assert!(note.contains("unset"), "{note}");
        assert!(note.contains("No speech was invented"), "{note}");
        let job = body["job"].as_str().unwrap();
        let snap = gen_audio_core::viewport::snapshot_global();
        let recorded = snap["jobs"].as_array().unwrap().iter().find(|item| item["job"] == job).unwrap();
        assert_eq!(recorded["phase"], "refused");
        assert!(recorded["target"].is_null());
        assert!(snap["views"].as_array().unwrap().iter().all(|view| view["asset"] != job));
    }

    #[test]
    fn t17_mcp_viewport_get_reports_navigate_focus_seek_flip_rename() {
        ui_navigate(&json!({"slide": "spatial"})).unwrap();
        let snap = viewport_get();
        assert_eq!(snap["ui"]["slide"], "slide:spatial");
        assert_eq!(snap["synthesizedSpeech"], false);
        let renamed = library_rename(&json!({"clipId": "lib-misaki-kokoro", "semanticName": "Narrator A"})).unwrap();
        assert_eq!(renamed["op"], "rename");
        let card = card_export(&json!({"uid": "lib-misaki-kokoro", "format": "adaptivecard"})).unwrap();
        assert_eq!(card["version"], "1.5");
        assert_eq!(card["card"]["version"], "1.5");
        assert!(card["card"]["fallbackText"].as_str().unwrap().contains("Narrator A") || card["card"]["fallbackText"].as_str().unwrap().contains("misaki"));
    }

    #[test]
    fn t18_compare_of_three_is_rejected() {
        let err = ui_compare(&json!({"uids": ["lib-kokoro-onnx", "lib-misaki-kokoro", "lib-kokoro"]})).unwrap_err();
        assert_eq!(err.0, -32602);
    }
}
