//! Process-local UI command bus.
//!
//! This is not an MCP session. `initialize` still stores nothing, HTTP still
//! omits `Mcp-Session-Id`, and a command does not write speech. The desktop
//! window reads the ring over `GET /control` or the short SSE stream.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gen_audio_core::asset_catalog;
use gen_audio_core::bridge;
use gen_audio_core::catalog::{self, MAX_AGENTS_PER_TRACK};
use gen_audio_core::library_store;
use gen_audio_core::paths;
use gen_audio_core::viewport::{self, Action, CoverageInput, FlipTo, Origin};
use serde_json::{json, Value};

struct Event {
    seq: u64,
    body: Value,
}

struct BusInner {
    next: u64,
    events: VecDeque<Event>,
    /// Seek landings the desktop window reported (`ui_seek_report`), by seq on
    /// this bus. They travel with the ring so two buses can both use seq 1.
    seek_reports: BTreeMap<u64, Value>,
}

struct Bus {
    inner: Mutex<BusInner>,
    reported: Condvar,
}

impl Bus {
    fn fresh() -> Self {
        Self {
            inner: Mutex::new(BusInner {
                next: 1,
                events: VecDeque::new(),
                seek_reports: BTreeMap::new(),
            }),
            reported: Condvar::new(),
        }
    }
}

pub const CAP: usize = 128;

fn process_bus() -> Arc<Bus> {
    static BUS: OnceLock<Arc<Bus>> = OnceLock::new();
    BUS.get_or_init(|| Arc::new(Bus::fresh())).clone()
}

// The desktop window and its HTTP workers share `process_bus`. A test that
// publishes and then reads the ring binds its own bus (and the same bus on a
// worker thread, when the read is over HTTP). Seek reports live on that bus.
#[cfg(test)]
thread_local! {
    static BUS_OVERRIDE: RefCell<Option<Arc<Bus>>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct BusGuard {
    previous: Option<Arc<Bus>>,
}

#[cfg(test)]
impl Drop for BusGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        BUS_OVERRIDE.with(|slot| *slot.borrow_mut() = previous);
    }
}

#[cfg(test)]
fn bind_bus(bus: Arc<Bus>) -> BusGuard {
    BUS_OVERRIDE.with(|slot| {
        let previous = slot.borrow_mut().replace(bus);
        BusGuard { previous }
    })
}

#[cfg(test)]
fn bind_fresh_bus() -> BusGuard {
    bind_bus(Arc::new(Bus::fresh()))
}

/// A bus a test can share across the threads that publish and read it.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestBus(Arc<Bus>);

#[cfg(test)]
impl TestBus {
    pub(crate) fn fresh() -> Self {
        Self(Arc::new(Bus::fresh()))
    }

    pub(crate) fn bind(&self) -> BusGuard {
        bind_bus(self.0.clone())
    }
}

fn current_bus() -> Arc<Bus> {
    #[cfg(test)]
    if let Some(bus) = BUS_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return bus;
    }
    process_bus()
}

fn with_bus<R>(f: impl FnOnce(&mut BusInner) -> R) -> R {
    let bus = current_bus();
    let mut inner = bus.inner.lock().expect("control bus");
    f(&mut inner)
}

static ATTACHED_REFS: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

/// How long `ui_playback` seek waits for the window's landing by default.
pub const SEEK_WAIT_DEFAULT_MS: u64 = 2000;
const SEEK_WAIT_MAX_MS: u64 = 10_000;
const SEEK_REASON_MAX: usize = 240;

/// The seconds a queued `playback` seek event asked for, if `seq` is one.
fn queued_seek_seconds(seq: u64) -> Option<f64> {
    with_bus(|bus| {
        let event = bus.events.iter().find(|event| event.seq == seq)?;
        let body = &event.body;
        if body["op"] != "playback" || body["args"]["action"] != "seek" {
            return None;
        }
        body["args"]["seconds"].as_f64()
    })
}

/// Wait up to `wait_ms` for the window to report where seek `seq` landed.
fn await_seek_landing(seq: u64, requested: f64, wait_ms: u64) -> Value {
    let bus = current_bus();
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let mut inner = bus.inner.lock().expect("control bus");
    loop {
        if let Some(report) = inner.seek_reports.get(&seq) {
            return report.clone();
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        inner = bus
            .reported
            .wait_timeout(inner, deadline - now)
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

static CANCEL: Mutex<BTreeMap<String, Arc<AtomicBool>>> = Mutex::new(BTreeMap::new());

pub fn publish(op: &str, args: &Value) -> u64 {
    with_bus(|bus| {
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
    })
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
    with_bus(|bus| {
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
        ControlDelta {
            cursor,
            gap,
            oldest,
            events,
        }
    })
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
    let uid = uid
        .as_str()
        .ok_or((-32602, "uid must be a string".to_string()))?;
    let asset = asset_catalog::require(uid).map_err(asset_catalog::CatalogError::rpc)?;
    let tile = asset_catalog::tile_id_for(&asset)
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
                format!(
                    "{id} is a connector, not an Agent. Agent is a persona such as alice or anton."
                ),
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
    let reduced = viewport::apply_global(Action::Navigate {
        slide: reference.clone(),
    })
    .map_err(|err| (err.code, err.message))?;
    if let Some(tile) = args.get("tileId").and_then(Value::as_str) {
        if !id_ok(tile) {
            return Err((-32602, "tileId is invalid".into()));
        }
    }
    // The queued event always carries the canonical id; the UI accepts both.
    args["slide"] = reduced
        .get("slide")
        .cloned()
        .unwrap_or_else(|| json!(format!("slide:{slug}")));
    let mut out = queued(
        "navigate",
        args,
        "Queued a viewport navigation. This does not render audio.",
    );
    if deprecated {
        let warning = format!(
            "ui_navigate slide \"{reference}\" is a deprecated alias; use \"slide:{slug}\""
        );
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
    if let Err(err) = viewport::apply_global(Action::Focus {
        uid: Some(tile.to_string()),
    }) {
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
    let origin = match args.get("origin").and_then(Value::as_str).unwrap_or("user") {
        "user" => Origin::User,
        "auto" => Origin::Auto,
        other => return Err((-32602, format!("origin must be user or auto, got {other}"))),
    };
    let playback_id = playback_asset(args, tile);
    if let Err(err) = apply_playback(&playback_id, action, args, origin) {
        if err.0 != -32602 || !err.1.contains("unknown asset") {
            return Err(err);
        }
    }
    let clip = catalog::library_clip(tile);
    let has_wav = clip.and_then(|item| item.wav_url).is_some()
        || library_store::list_overlays()
            .iter()
            .any(|row| row["id"] == tile && row["wavUrl"].is_string());
    let mut playback_args = args.clone();
    if action == "play" {
        playback_args["origin"] = json!(origin.as_str());
    }
    let mut payload = queued(
        "playback",
        playback_args,
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
    let bus = current_bus();
    {
        let mut inner = bus.inner.lock().expect("control bus");
        if inner.seek_reports.contains_key(&seq) {
            return Err((-32602, format!("seek {seq} already reported")));
        }
        inner.seek_reports.insert(seq, report.clone());
        while inner.seek_reports.len() > CAP {
            inner.seek_reports.pop_first();
        }
    }
    bus.reported.notify_all();
    Ok(json!({"recorded": true, "seekSeq": seq, "report": report, "synthesizedSpeech": false}))
}

/// Play the viewport asset the caller named. A generated clip's tile id is
/// `gen-…` while the reducer asset is the `ga:` uid. Baked clips stay on their
/// legacy id, which is the viewport key.
fn playback_asset(args: &Value, tile: &str) -> String {
    if let Some(uid) = args.get("uid").and_then(Value::as_str) {
        if viewport::contains_global(uid) {
            return uid.to_string();
        }
    }
    if viewport::contains_global(tile) {
        return tile.to_string();
    }
    if let Some(uid) = asset_catalog::uid_for_legacy("audio_clip", tile) {
        if viewport::contains_global(&uid) {
            return uid;
        }
    }
    tile.to_string()
}

fn apply_playback(
    tile: &str,
    action: &str,
    args: &Value,
    origin: Origin,
) -> Result<(), (i32, String)> {
    let action = match action {
        "play" => Action::Play {
            uid: tile.to_string(),
            origin,
        },
        "pause" => Action::Pause {
            uid: tile.to_string(),
        },
        "seek" => Action::Seek {
            uid: tile.to_string(),
            t: args.get("seconds").and_then(Value::as_f64).unwrap_or(0.0),
        },
        _ => return Ok(()),
    };
    let reduced = viewport::apply_global(action).map_err(|err| (err.code, err.message))?;
    if let Some(events) = reduced.get("events").and_then(Value::as_array) {
        for event in events {
            let op = event.get("op").and_then(Value::as_str).unwrap_or("play");
            publish(op, event);
        }
    }
    Ok(())
}

fn flip_view(raw: &str) -> Result<String, (i32, String)> {
    if raw.contains("..") || raw.contains('/') || raw.contains('\\') {
        return Err((-32602, "view is invalid".into()));
    }
    if let Some(rest) = raw.strip_prefix("view:") {
        if !id_ok(rest) {
            return Err((-32602, "view is invalid".into()));
        }
        return Ok(raw.to_string());
    }
    if !id_ok(raw) {
        return Err((-32602, "tileId is invalid".into()));
    }
    Ok(format!("view:{raw}"))
}

pub fn ui_flip(args: &Value) -> Result<Value, (i32, String, Option<Value>)> {
    let args = &with_uid_tile(args).map_err(|(code, message)| (code, message, None))?;
    let raw = args
        .get("tileId")
        .or_else(|| args.get("view"))
        .and_then(Value::as_str)
        .ok_or((-32602, "ui_flip needs tileId or uid".to_string(), None))?;
    let view = flip_view(raw).map_err(|(code, message)| (code, message, None))?;
    let has_next = args.get("next").is_some();
    let has_face = args.get("face").is_some();
    let has_flipped = args.get("flipped").is_some();
    if usize::from(has_next) + usize::from(has_face) + usize::from(has_flipped) > 1 {
        return Err((
            -32602,
            "ui_flip takes one of next, face, flipped".into(),
            None,
        ));
    }
    let section = args.get("section").and_then(Value::as_str);
    let bare_section = section.is_some() && !has_next && !has_face && !has_flipped;
    let back_section = section.is_some()
        && !bare_section
        && !has_next
        && ((has_face && args.get("face").and_then(Value::as_str) == Some("back"))
            || (!has_face && has_flipped && args.get("flipped").and_then(Value::as_bool) == Some(true)));
    let (to, warning) = if bare_section || back_section {
        let section = section.unwrap();
        let face = face_for_deprecated_section(section)?;
        let code = if bare_section {
            "deprecated_bare_section"
        } else {
            "deprecated_back_section"
        };
        let detail = if bare_section {
            format!("section \"{section}\" without a face is deprecated; it selects face \"{face}\"")
        } else {
            format!("back plus section \"{section}\" is deprecated; it selects face \"{face}\"")
        };
        (FlipTo::Face(face.to_string()), Some((code, detail)))
    } else if has_next {
        if args.get("next").and_then(Value::as_bool) != Some(true) {
            return Err((-32602, "next must be true".into(), None));
        }
        (FlipTo::Next, None)
    } else if let Some(face) = args.get("face").and_then(Value::as_str) {
        (FlipTo::Face(face.to_string()), None)
    } else {
        let flipped = args.get("flipped").and_then(Value::as_bool).unwrap_or(true);
        (
            FlipTo::Face(if flipped { "back" } else { "front" }.into()),
            None,
        )
    };
    let mut event = viewport::apply_global(Action::Flip { view, to })
        .map_err(|err| (err.code, err.message, err.data))?;
    let tile = raw.strip_prefix("view:").unwrap_or(raw);
    event["tileId"] = json!(tile);
    if let Some(section) = section {
        event["section"] = json!(section);
    }
    if let Some((code, detail)) = &warning {
        let line = format!("gen-audio-mcp: deprecation: {code}: {detail}");
        write_stderr_line(&line);
    }
    if let Some(persona) = args.get("personaId").and_then(Value::as_str) {
        if catalog::persona(persona).is_none() {
            return Err((-32602, format!("unknown persona {persona}"), None));
        }
        event["personaId"] = json!(persona);
        event["profile"] = voice_profile_get(&json!({"personaId": persona}))
            .map_err(|(code, message)| (code, message, None))?;
    }
    let mut payload = queued(
        "flip",
        event,
        "Queued a flipcard. The payload is the voice profile when a persona is named. This does not render audio.",
    );
    for key in ["face_id", "face_index", "face_count", "glyph_label"] {
        if let Some(value) = payload["args"].get(key) {
            payload[key] = value.clone();
        }
    }
    if let Some((code, detail)) = warning {
        payload["warnings"] = json!([{"code": code, "detail": detail}]);
    }
    Ok(payload)
}

/// Q1: a deprecated `section` names the face that renders that section.
fn face_for_deprecated_section(section: &str) -> Result<&str, (i32, String, Option<Value>)> {
    match section {
        "identity" | "honesty" | "clip" => Ok("clip"),
        "cube" => Ok("cube"),
        "layers" => Ok("layers"),
        "spectrogram" => Ok("spectrogram"),
        "relations" => Ok("relations"),
        "model" => Ok("model"),
        "cubes" | "model_cubes" => Ok("cubes"),
        "connector" => Ok("connector"),
        "profile" => Ok("profile"),
        "persona" => Ok("persona"),
        other => Err((-32602, format!("unknown section {other}"), None)),
    }
}

fn write_stderr_line(line: &str) {
    let text = format!("{line}\n");
    #[cfg(unix)]
    {
        extern "C" {
            fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
        }
        let _ = unsafe { write(2, text.as_ptr() as *const std::ffi::c_void, text.len()) };
    }
    #[cfg(not(unix))]
    {
        eprintln!("{line}");
    }
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
    std::env::var("GEN_AUDIO_KOKORO_MODEL")
        .ok()
        .filter(|value| !value.is_empty())
        .is_some()
        && std::env::var("GEN_AUDIO_KOKORO_VOICES")
            .ok()
            .filter(|value| !value.is_empty())
            .is_some()
}

pub fn ui_generate(args: &Value) -> Result<Value, (i32, String)> {
    if args.get("cancel").and_then(Value::as_bool) == Some(true) {
        let job = args
            .get("job")
            .and_then(Value::as_str)
            .ok_or((-32602, "cancel needs job".to_string()))?;
        return request_cancel(job);
    }
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
        .or_else(|| {
            args.get("durationMin")
                .and_then(Value::as_u64)
                .map(|minutes| minutes as f64 * 60.0)
        })
        .unwrap_or(180.0);
    let personas: Vec<String> = args
        .get("agents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let seq = publish("generate", &normalized);
    let job = format!("job-{seq}");
    viewport::apply_global(Action::Generate {
        prompt_ref: prompt_note.to_string(),
        personas: personas.clone(),
        voice: voice.clone(),
        duration_s,
        focus,
        job: job.clone(),
    })
    .map_err(|err| (err.code, err.message))?;
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !adapter {
        return finish_generate(
            seq,
            job,
            voice.clone(),
            normalized,
            side,
            "unavailable",
            format!("{voice} has no synth adapter. No audio was written."),
            false,
        );
    }
    if voice == "kokoro_onnx" && !kokoro_weights_ready() {
        return finish_generate(
            seq,
            job,
            voice,
            normalized,
            side,
            "refused",
            "GEN_AUDIO_KOKORO_MODEL and GEN_AUDIO_KOKORO_VOICES are unset. No speech was invented."
                .into(),
            false,
        );
    }
    if prompt.trim().is_empty() {
        return finish_generate(
            seq,
            job,
            voice,
            normalized,
            side,
            "refused",
            "empty prompt; no sample script is used".into(),
            false,
        );
    }
    if prompt.len() > 200_000 {
        return finish_generate(
            seq,
            job,
            voice,
            normalized,
            side,
            "refused",
            "prompt is too long".into(),
            false,
        );
    }
    match begin_generate(&voice, &personas, duration_s, &prompt) {
        Ok((work, child)) => {
            start_generate(seq, job, voice, normalized, side, work, child, duration_s)
        }
        Err(reason) => {
            let invoked = !reason.starts_with("failed to start");
            finish_generate(
                seq, job, voice, normalized, side, "refused", reason, invoked,
            )
        }
    }
}

fn finish_generate(
    seq: u64,
    job: String,
    voice: String,
    normalized: Value,
    side: Value,
    phase: &str,
    detail: String,
    synth_invoked: bool,
) -> Result<Value, (i32, String)> {
    let speech = false;
    let uid = Value::Null;
    let mut job_body = if phase != "done" {
        viewport::apply_global(Action::Job {
            job: job.clone(),
            target: None,
            phase: phase.into(),
            reason: Some(detail.clone()),
            kind: Some("audio_clip".into()),
            synthesized: false,
            coverage: None,
        })
        .map_err(|err| (err.code, err.message))?
    } else {
        json!({"op": "job", "job": job, "phase": phase, "target": uid, "landed": speech})
    };
    job_body["voice"] = json!(voice);
    job_body["synthesizedSpeech"] = json!(speech);
    job_body["reason"] = json!(detail);
    let job_seq = publish("job", &job_body);
    let progress = note_progress(&voice, phase, &detail, speech);
    Ok(json!({
        "ok": phase == "done",
        "synthesizedSpeech": speech,
        "synthesized": speech,
        "op": "generate",
        "seq": seq,
        "jobSeq": job_seq,
        "job": job,
        "args": normalized,
        "sidepane": side,
        "synthInvoked": synth_invoked,
        "landed": speech,
        "createdUid": uid,
        "phase": phase,
        "progress": progress,
        "note": detail,
        "stream": {"path": "/control/stream", "transport": "sse", "stateless": true}
    }))
}

fn request_cancel(job: &str) -> Result<Value, (i32, String)> {
    let flag = CANCEL.lock().expect("cancel").get(job).cloned();
    let Some(flag) = flag else {
        return Err((-32602, format!("unknown job {job}")));
    };
    flag.store(true, Ordering::SeqCst);
    Ok(json!({
        "ok": true,
        "phase": "cancel",
        "job": job,
        "synthesizedSpeech": false,
        "landed": false,
        "note": "cancel requested"
    }))
}

fn generate_timeout(duration_s: f64) -> Duration {
    let secs = (duration_s * 3.0 + 90.0).clamp(30.0, 1800.0);
    Duration::from_secs_f64(secs)
}

fn begin_generate(
    voice: &str,
    personas: &[String],
    duration_s: f64,
    prompt: &str,
) -> Result<(paths::TempWorkDir, std::process::Child), String> {
    let repo = paths::find_repo_root()
        .ok_or_else(|| "failed to start: repository root was not found".to_string())?;
    let work = paths::make_work_dir().map_err(|err| format!("failed to start: {err}"))?;
    let scripts = work.join("scripts");
    std::fs::create_dir_all(&scripts).map_err(|err| format!("failed to start: {err}"))?;
    std::fs::write(scripts.join("prompt.txt"), prompt)
        .map_err(|err| format!("failed to start: {err}"))?;
    let plan = bridge::plan_generate(&repo, &work, personas, voice, duration_s)
        .map_err(|err| format!("failed to start: {err}"))?;
    let child = bridge::start_plan(&plan)?;
    Ok((work, child))
}

fn start_generate(
    seq: u64,
    job: String,
    voice: String,
    normalized: Value,
    side: Value,
    work: paths::TempWorkDir,
    mut child: std::process::Child,
    duration_s: f64,
) -> Result<Value, (i32, String)> {
    let flag = Arc::new(AtomicBool::new(false));
    CANCEL
        .lock()
        .expect("cancel")
        .insert(job.clone(), flag.clone());
    let reduced = match viewport::apply_global(Action::Job {
        job: job.clone(),
        target: None,
        phase: "running".into(),
        reason: None,
        kind: Some("audio_clip".into()),
        synthesized: false,
        coverage: None,
    }) {
        Ok(body) => body,
        Err(err) => {
            CANCEL.lock().expect("cancel").remove(&job);
            stop_child(&mut child);
            let _ = child.wait();
            return Err((err.code, err.message));
        }
    };
    let mut running = reduced;
    running["voice"] = json!(&voice);
    running["synthesizedSpeech"] = json!(false);
    let job_seq = publish("job", &running);
    let timeout = generate_timeout(duration_s);
    let job_thread = job.clone();
    let voice_thread = voice.clone();
    if let Err(err) = std::thread::Builder::new()
        .name(format!("gen-{job}"))
        .spawn(move || complete_generate(job_thread, voice_thread, work, child, flag, timeout))
    {
        CANCEL.lock().expect("cancel").remove(&job);
        return finish_generate(
            seq,
            job,
            voice,
            normalized,
            side,
            "refused",
            format!("failed to start: {err}"),
            false,
        );
    }
    Ok(json!({
        "ok": true,
        "synthesizedSpeech": false,
        "synthesized": false,
        "op": "generate",
        "seq": seq,
        "jobSeq": job_seq,
        "job": job,
        "args": normalized,
        "sidepane": side,
        "synthInvoked": true,
        "landed": false,
        "createdUid": Value::Null,
        "phase": "running",
        "note": "generate started",
        "stream": {"path": "/control/stream", "transport": "sse", "stateless": true}
    }))
}

fn complete_generate(
    job: String,
    voice: String,
    work: paths::TempWorkDir,
    child: std::process::Child,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
) {
    let (timed_out, cancelled, stderr) = wait_child(child, &cancel, timeout);
    CANCEL.lock().expect("cancel").remove(&job);
    if cancelled {
        refuse_running(&job, &voice, "cancelled");
        return;
    }
    if timed_out {
        refuse_running(&job, &voice, "generate timed out");
        return;
    }
    let manifest: Value = std::fs::read_to_string(work.join("manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({"ok": false, "synthesizedSpeech": false, "reason": stderr}));
    if manifest["synthesizedSpeech"] != true {
        let reason = manifest["reason"]
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or("generate refused. No speech was invented.");
        refuse_running(&job, &voice, reason);
        return;
    }
    let imported = match library_store::import_pipeline(&work, &manifest) {
        Ok(imported) => imported,
        Err(err) => {
            refuse_running(&job, &voice, &err);
            return;
        }
    };
    let reduced = match viewport::apply_global(Action::Job {
        job: job.clone(),
        target: Some(imported.wav_uid.clone()),
        phase: "done".into(),
        reason: None,
        kind: Some("audio_clip".into()),
        synthesized: true,
        coverage: None,
    }) {
        Ok(body) => body,
        Err(err) => {
            refuse_running(&job, &voice, &err.message);
            return;
        }
    };
    let mut body = reduced;
    body["voice"] = json!(&voice);
    body["synthesizedSpeech"] = json!(true);
    body["legacyId"] = json!(&imported.legacy_id);
    body["wavUrl"] = json!(&imported.wav_url);
    body["specUrl"] = json!(&imported.spec_url);
    body["cubeUrl"] = json!(&imported.cube_url);
    body["createdUid"] = json!(&imported.wav_uid);
    if let Some(video) = &imported.video_url {
        body["videoUrl"] = json!(video);
    }
    publish_job(&voice, body, true);
    if let Ok(spec) = viewport::apply_global(Action::Job {
        job: format!("{job}-spec"),
        target: Some(format!("{}:spectrogram", imported.wav_uid)),
        phase: "done".into(),
        reason: None,
        kind: Some("spectrogram".into()),
        synthesized: true,
        coverage: None,
    }) {
        publish_job(&voice, spec, true);
    }
    let coverage = CoverageInput {
        selector_start: 0.0,
        selector_end: imported.selector_end,
        clip_duration_s: imported.clip_duration_s,
        recorded_source_duration_s: imported.recorded_source_duration_s,
        sec_per_bin: imported.sec_per_bin,
        clip_in_src: imported.clip_in_src,
    };
    if let Ok(cube) = viewport::apply_global(Action::Job {
        job: format!("{job}-cube"),
        target: Some(format!("{}:cube", imported.wav_uid)),
        phase: "done".into(),
        reason: None,
        kind: Some("cube".into()),
        synthesized: true,
        coverage: Some(coverage),
    }) {
        publish_job(&voice, cube, true);
    }
    if imported.video_url.is_some() {
        if let Ok(mut video) = viewport::apply_global(Action::Job {
            job: format!("{job}-video"),
            target: Some(format!("{}:video", imported.wav_uid)),
            phase: "done".into(),
            reason: None,
            kind: Some("video".into()),
            synthesized: true,
            coverage: None,
        }) {
            if let Some(url) = &imported.video_url {
                video["videoUrl"] = json!(url);
            }
            publish_job(&voice, video, true);
        }
    }
    let honoured = manifest["durationHonoured"].as_bool().unwrap_or(false);
    let speech_s = manifest["speech_s"]
        .as_f64()
        .unwrap_or(imported.clip_duration_s);
    let _ = note_progress(
        &voice,
        "ok",
        &format!("clip landed. speech_s {speech_s:.3}. durationHonoured {honoured}."),
        true,
    );
}

fn publish_job(voice: &str, mut body: Value, speech: bool) {
    body["voice"] = json!(voice);
    body["synthesizedSpeech"] = json!(speech);
    publish("job", &body);
    if let Some(uid) = body
        .get("focus")
        .and_then(Value::as_str)
        .filter(|uid| !uid.is_empty())
    {
        publish("focus", &json!({"uid": uid}));
    }
}

fn refuse_running(job: &str, voice: &str, reason: &str) {
    let body = viewport::apply_global(Action::Job {
        job: job.to_string(),
        target: None,
        phase: "refused".into(),
        reason: Some(reason.to_string()),
        kind: Some("audio_clip".into()),
        synthesized: false,
        coverage: None,
    })
    .unwrap_or_else(
        |_| json!({"op": "job", "job": job, "phase": "refused", "reason": reason, "landed": false}),
    );
    publish_job(voice, body, false);
    let _ = note_progress(voice, "refused", reason, false);
}

fn wait_child(
    mut child: std::process::Child,
    cancel: &AtomicBool,
    timeout: Duration,
) -> (bool, bool, String) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out = std::thread::spawn(move || read_pipe(stdout));
    let err = std::thread::spawn(move || read_pipe(stderr));
    let started = Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    loop {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            stop_child(&mut child);
            break;
        }
        if started.elapsed() > timeout {
            timed_out = true;
            stop_child(&mut child);
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    let _ = child.wait();
    let _ = out.join();
    let stderr = err.join().unwrap_or_default();
    (timed_out, cancelled, stderr)
}

/// Signal the generate child and its process group. The group leader is the
/// python process started with `process_group(0)`; grandchildren stay in that
/// group unless they call setpgid themselves.
fn stop_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id();
        if pid > 0 {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    let _ = child.kill();
}

fn read_pipe<R: Read>(pipe: Option<R>) -> String {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf);
    }
    let text = String::from_utf8_lossy(&buf);
    text.chars()
        .rev()
        .take(400)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

pub fn ui_compare(args: &Value) -> Result<Value, (i32, String)> {
    let uids = args
        .get("uids")
        .and_then(Value::as_array)
        .ok_or((-32602, "ui_compare needs uids".to_string()))?;
    let uids: Vec<String> = uids
        .iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect();
    if uids.len()
        != args["uids"]
            .as_array()
            .map(|items| items.len())
            .unwrap_or(0)
    {
        return Err((-32602, "uids must be strings".into()));
    }
    viewport::apply_global(Action::Compare { uids: uids.clone() })
        .map_err(|err| (err.code, err.message))?;
    if let Some(select) = args.get("select").and_then(Value::as_str) {
        if !uids.iter().any(|uid| uid == select) {
            return Err((-32602, "select is outside compare".into()));
        }
        let reduced = viewport::apply_global(Action::Play {
            uid: select.to_string(),
            origin: Origin::User,
        })
        .map_err(|err| (err.code, err.message))?;
        if let Some(events) = reduced.get("events").and_then(Value::as_array) {
            for event in events {
                let op = event.get("op").and_then(Value::as_str).unwrap_or("play");
                publish(op, event);
            }
        }
    }
    Ok(queued(
        "compare",
        args.clone(),
        "Queued compare. Selecting a side is a user Play, so focus and the clock follow that side.",
    ))
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
    let format = args
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("adaptivecard");
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
    let mut clips = serde_json::to_value(catalog::library_clips())
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    for overlay in library_store::list_overlays() {
        let id = overlay.get("id").and_then(Value::as_str).unwrap_or("");
        if let Some(slot) = clips
            .iter_mut()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        {
            if let Some(title) = overlay.get("title").filter(|title| title.is_string()) {
                slot["title"] = title.clone();
            }
        } else if overlay.get("generated") == Some(&json!(true)) {
            clips.push(overlay);
        }
    }
    json!({
        "ok": true,
        "synthesizedSpeech": false,
        "openedFiles": false,
        "clips": clips,
        "note": "Catalog plus clips landed by generate. WAV bytes are not opened here."
    })
}

fn rename_target(clip_id: &str) -> String {
    if viewport::contains_global(clip_id) {
        return clip_id.to_string();
    }
    if let Some(uid) = asset_catalog::uid_for_legacy("audio_clip", clip_id) {
        if viewport::contains_global(&uid) {
            return uid;
        }
    }
    clip_id.to_string()
}

pub fn library_rename(args: &Value) -> Result<Value, (i32, String)> {
    let clip_id = args
        .get("clipId")
        .and_then(Value::as_str)
        .ok_or((-32602, "library_rename needs clipId".to_string()))?;
    let known = catalog::library_clip(clip_id).is_some()
        || library_store::list_overlays()
            .iter()
            .any(|row| row.get("id").and_then(Value::as_str) == Some(clip_id));
    if !known {
        return Err((-32602, format!("unknown library clip {clip_id}")));
    }
    let semantic = args.get("semanticName").and_then(Value::as_str);
    let face = args.get("faceName").and_then(Value::as_str);
    match (semantic, face) {
        (None, None) => {
            return Err((
                -32602,
                "library_rename needs semanticName or faceName".into(),
            ))
        }
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
    if let Some(name) = semantic.or(face) {
        viewport::apply_global(Action::Rename {
            uid: rename_target(clip_id),
            name: name.to_string(),
        })
        .map_err(|err| (err.code, err.message))?;
    }
    let rev =
        library_store::persist_rename(clip_id, semantic, face).map_err(|err| (-32603, err))?;
    let mut out = queued(
        "rename",
        args.clone(),
        "Queued a display rename. The WAV file is not rewritten.",
    );
    out["persisted"] = json!(true);
    out["display_rev"] = json!(rev);
    Ok(out)
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
    let mut profile =
        catalog::voice_profile_value(id).ok_or((-32602, "profile missing".to_string()))?;
    let attached = ATTACHED_REFS
        .lock()
        .expect("refs")
        .get(id)
        .cloned()
        .unwrap_or_default();
    if !attached.is_empty() {
        let refs = profile
            .get_mut("refs")
            .and_then(Value::as_array_mut)
            .ok_or((-32603, "profile refs missing".to_string()))?;
        for reference in attached {
            if !refs
                .iter()
                .any(|item| item.as_str() == Some(reference.as_str()))
            {
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
    let clip = catalog::library_clip(clip_id)
        .ok_or((-32602, format!("unknown library clip {clip_id}")))?;
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
    let clip = catalog::library_clip(clip_id)
        .ok_or((-32602, format!("unknown library clip {clip_id}")))?;
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
    let path = allowlisted_library_file(repo, web_path).ok_or((
        -32603,
        "cube JSON is not an allowlisted library file".to_string(),
    ))?;
    let text = std::fs::read_to_string(&path).map_err(|err| (-32603, err.to_string()))?;
    let value: Value = serde_json::from_str(&text).map_err(|err| (-32603, err.to_string()))?;
    let layers_in = value
        .get("layers")
        .and_then(Value::as_object)
        .ok_or((-32603, "cube JSON has no layers".to_string()))?;
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
    let points = value
        .get("points_preview")
        .and_then(Value::as_array)
        .map(|rows| rows.len())
        .unwrap_or(0);
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
    let base = catalog::persona(persona_id)
        .map(|person| person.refs.len())
        .unwrap_or(0);
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

fn propose_names(
    repo: Option<&Path>,
    clip: &catalog::LibraryClipMeta,
) -> (String, String, &'static str) {
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
        "/library/library_kokoro_onnx.synth.json" => {
            "apps/desktop/public/library/library_kokoro_onnx.synth.json"
        }
        "/library/library_kokoro_onnx_cube3d.json" => {
            "apps/desktop/public/library/library_kokoro_onnx_cube3d.json"
        }
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
    if path.starts_with(&root) {
        Some(path)
    } else {
        None
    }
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

    fn fresh_viewport() -> gen_audio_core::viewport::ViewportGuard {
        gen_audio_core::viewport::bind_viewport(gen_audio_core::viewport::ViewportHandle::release())
    }

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
        assert_eq!(
            alias["args"]["slide"], "slide:library",
            "the alias is rewritten to the canonical id"
        );
        assert!(alias["deprecation"]
            .as_str()
            .unwrap()
            .contains("slide:library"));
        assert_eq!(
            ui_navigate(&json!({"slide": "slide:nope"})).unwrap_err().0,
            -32602
        );
        assert_eq!(
            ui_navigate(&json!({"slide": "slide:"})).unwrap_err().0,
            -32602
        );
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
        let _viewport = fresh_viewport();
        let _kokoro = gen_audio_core::bridge::NoKokoroEnv::new();
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
        let recorded = snap["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["job"] == job)
            .unwrap();
        assert_eq!(recorded["phase"], "refused");
        assert!(recorded["target"].is_null());
        assert!(snap["views"]
            .as_array()
            .unwrap()
            .iter()
            .all(|view| view["asset"] != job));
    }

    #[test]
    fn t17_mcp_viewport_get_reports_navigate_focus_seek_flip_rename() {
        let _viewport = fresh_viewport();
        ui_navigate(&json!({"slide": "spatial"})).unwrap();
        let snap = viewport_get();
        assert_eq!(snap["ui"]["slide"], "slide:spatial");
        assert_eq!(snap["synthesizedSpeech"], false);
        let renamed =
            library_rename(&json!({"clipId": "lib-misaki-kokoro", "semanticName": "Narrator A"}))
                .unwrap();
        assert_eq!(renamed["op"], "rename");
        let card =
            card_export(&json!({"uid": "lib-misaki-kokoro", "format": "adaptivecard"})).unwrap();
        assert_eq!(card["version"], "1.5");
        assert_eq!(card["card"]["version"], "1.5");
        assert!(
            card["card"]["fallbackText"]
                .as_str()
                .unwrap()
                .contains("Narrator A")
                || card["card"]["fallbackText"]
                    .as_str()
                    .unwrap()
                    .contains("misaki")
        );
        assert_eq!(renamed["persisted"], true);
        assert!(renamed["display_rev"].as_u64().unwrap() >= 2);
        let renamed_text = renamed.to_string();
        assert!(!renamed_text.contains("catalog.json"), "{renamed_text}");
        assert!(!renamed_text.contains("/.local/"), "{renamed_text}");
    }

    #[test]
    fn t17_ui_flip_next_face_aliases_and_one_selector() {
        let _viewport = fresh_viewport();
        let server = crate::Server::boot();
        let omitted = crate::handle(
            &server,
            json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"ui_flip","arguments":{"tileId":"conn-mcp","face":"relations"}}}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(omitted["error"]["code"], -32602, "{omitted}");
        assert_eq!(
            omitted["error"]["data"]["valid_faces"],
            json!(["connector"]),
            "{omitted}"
        );
        assert_eq!(
            omitted["error"]["data"]["aliases"],
            json!(["front", "back"]),
            "{omitted}"
        );

        let rejected = crate::handle(
            &server,
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"ui_flip","arguments":{"tileId":"lib-kokoro","next":true}}}),
        )
        .unwrap()
        .unwrap();
        let text = rejected["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        let parsed: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        assert_eq!(parsed["face_id"], "cube", "{rejected}");

        let next = ui_flip(&json!({"tileId": "lib-misaki-kokoro", "next": true})).unwrap();
        assert_eq!(next["face_id"], "cube");
        assert_eq!(next["face_index"], 1);
        assert_eq!(next["face_count"], 5);
        assert_eq!(
            viewport_get()["ui"]["faces"]["view:lib-misaki-kokoro"]["face_id"],
            "cube"
        );

        let spectrogram =
            ui_flip(&json!({"tileId": "lib-misaki-kokoro", "face": "spectrogram"})).unwrap();
        assert_eq!(spectrogram["face_id"], "spectrogram");
        assert_eq!(spectrogram["face_index"], 3);
        assert_eq!(spectrogram["face_count"], 5);

        let relations = ui_flip(&json!({"tileId": "lib-misaki-kokoro", "next": true})).unwrap();
        assert_eq!(relations["face_id"], "relations");
        assert_eq!(relations["face_index"], 4);
        let wrapped = ui_flip(&json!({"tileId": "lib-misaki-kokoro", "next": true})).unwrap();
        assert_eq!(wrapped["face_id"], "clip");
        assert_eq!(wrapped["face_index"], 0);

        let back = ui_flip(&json!({"tileId": "lib-misaki-kokoro", "face": "back"})).unwrap();
        assert_eq!(back["face_id"], "cube");
        assert_eq!(back["face_index"], 1);
        let front = ui_flip(&json!({"tileId": "lib-misaki-kokoro", "flipped": false})).unwrap();
        assert_eq!(front["face_id"], "clip");
        assert_eq!(front["face_index"], 0);
        let bare = ui_flip(&json!({"tileId": "lib-misaki-kokoro"})).unwrap();
        assert_eq!(bare["face_id"], "cube");
        assert_eq!(bare["face_index"], 1);

        let waveform =
            ui_flip(&json!({"tileId": "lib-misaki-kokoro", "face": "waveform"})).unwrap_err();
        assert_eq!(waveform.0, -32602);
        assert!(
            waveform
                .1
                .contains("clip, cube, layers, spectrogram, relations"),
            "{}",
            waveform.1
        );
        assert_eq!(
            viewport_get()["ui"]["faces"]["view:lib-misaki-kokoro"]["face_id"],
            "cube"
        );

        let omitted = ui_flip(&json!({"tileId": "lib-magpie", "face": "cube"})).unwrap_err();
        assert!(
            omitted.1.contains("valid faces: clip, relations"),
            "{}",
            omitted.1
        );
        let magpie = ui_flip(&json!({"tileId": "lib-magpie", "next": true})).unwrap();
        assert_eq!(magpie["face_id"], "relations");
        assert_eq!(magpie["face_index"], 1);
        assert_eq!(magpie["face_count"], 2);

        for (tile, face_id, count) in [
            ("engine-kokoro", "cubes", 3),
            ("engine-vibevoice", "cubes", 3),
            ("engine-kokoro-dayour", "cubes", 3),
        ] {
            let moved = ui_flip(&json!({"tileId": tile, "next": true})).unwrap();
            assert_eq!(moved["face_id"], face_id, "{tile}");
            assert_eq!(moved["face_index"], 1, "{tile}");
            assert_eq!(moved["face_count"], count, "{tile}");
        }

        let model = ui_flip(&json!({"tileId": "engine-magpie", "next": true})).unwrap();
        assert_eq!(model["face_id"], "model");
        assert_eq!(model["face_index"], 0);
        assert_eq!(model["face_count"], 1);
        assert_eq!(model["glyph_label"], "Card has 1 face: Model");
        let pocket = ui_flip(&json!({"tileId": "engine-pocket", "next": true})).unwrap();
        assert_eq!(pocket["glyph_label"], "Card has 1 face: Model");
        let model_relations =
            ui_flip(&json!({"tileId": "engine-magpie", "face": "relations"})).unwrap_err();
        assert!(
            model_relations.1.contains("valid faces: model"),
            "{}",
            model_relations.1
        );
        let g2p = ui_flip(&json!({"tileId": "engine-misaki", "face": "cubes"})).unwrap_err();
        assert!(g2p.1.contains("valid faces: model, relations"), "{}", g2p.1);
        let one = ui_flip(&json!({"tileId": "engine-magpie", "face": "back"})).unwrap_err();
        assert!(one.1.contains("needs 2 faces"), "{}", one.1);

        let snap = viewport_get();
        for id in [
            "conn-mcp",
            "conn-acp",
            "conn-harness",
            "conn-copilot",
            "conn-claude",
            "conn-gpt",
            "conn-gemini",
        ] {
            let face = &snap["ui"]["faces"][&format!("view:{id}")];
            assert_eq!(face["face_count"], 1, "{id}");
            assert_eq!(face["ids"][0], "connector", "{id}");
            assert_eq!(face["glyph_label"], "Card has 1 face: Connector", "{id}");
        }
        let connector = ui_flip(&json!({"tileId": "conn-mcp", "face": "relations"})).unwrap_err();
        assert_eq!(connector.0, -32602);
        assert!(
            connector.1.contains("valid faces: connector"),
            "{}",
            connector.1
        );

        let two = ui_flip(&json!({
            "tileId": "lib-misaki-kokoro",
            "next": true,
            "face": "cube"
        }))
        .unwrap_err();
        assert_eq!(two.0, -32602);
        assert_eq!(two.1, "ui_flip takes one of next, face, flipped");
    }

    #[test]
    fn ui_flip_rejects_a_pathological_view_and_applies_a_real_one() {
        let _viewport = fresh_viewport();
        let bad = ui_flip(&json!({"view": "view:../../etc"})).unwrap_err();
        assert_eq!(bad.0, -32602);
        let unknown = ui_flip(&json!({"view": "view:not-a-card"})).unwrap_err();
        assert_eq!(unknown.0, -32602);
        let ok = ui_flip(&json!({"view": "view:lib-kokoro-onnx", "face": "back"})).unwrap();
        assert_eq!(ok["op"], "flip");
        assert_eq!(ok["args"]["tileId"], "lib-kokoro-onnx");
        assert_eq!(
            viewport_get()["ui"]["flipped"]["view:lib-kokoro-onnx"]["face"],
            "back"
        );
        assert_eq!(
            ui_generate(&json!({"cancel": true, "job": "job-missing"}))
                .unwrap_err()
                .0,
            -32602
        );
    }

    #[test]
    fn t18_compare_of_three_is_rejected_and_select_follows_focus() {
        let _viewport = fresh_viewport();
        ui_playback(&json!({"tileId": "lib-kokoro", "action": "play", "origin": "user"})).unwrap();
        let err =
            ui_compare(&json!({"uids": ["lib-kokoro-onnx", "lib-misaki-kokoro", "lib-kokoro"]}))
                .unwrap_err();
        assert_eq!(err.0, -32602);
        assert_eq!(viewport_get()["ui"]["focus"], "lib-kokoro");
        ui_compare(&json!({"uids": ["lib-kokoro-onnx", "lib-misaki-kokoro"], "select": "lib-misaki-kokoro"})).unwrap();
        let snap = viewport_get();
        assert_eq!(snap["ui"]["focus"], "lib-misaki-kokoro");
        assert_eq!(snap["ui"]["clock"]["source"], "lib-misaki-kokoro");
        assert_eq!(snap["ui"]["compare"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn c5_bare_slide_is_a_logged_alias_and_canonical_is_not() {
        let _viewport = fresh_viewport();
        let bare = ui_navigate(&json!({"slide": "library"})).unwrap();
        assert_eq!(bare["args"]["slide"], "slide:library");
        assert!(bare["deprecation"]
            .as_str()
            .unwrap()
            .contains("slide:library"));
        let canonical = ui_navigate(&json!({"slide": "slide:spatial"})).unwrap();
        assert_eq!(canonical["args"]["slide"], "slide:spatial");
        assert!(canonical.get("deprecation").is_none());
        assert_eq!(viewport_get()["ui"]["slide"], "slide:spatial");
    }

    #[test]
    fn user_play_publishes_focus_before_play() {
        let _viewport = fresh_viewport();
        let _bus = bind_fresh_bus();
        let before = since(u64::MAX).cursor;
        let played = ui_playback(
            &json!({"tileId": "lib-kokoro-onnx", "action": "play", "origin": "user"}),
        )
        .unwrap();
        assert_eq!(played["args"]["origin"], "user");
        let seq = played["seq"].as_u64().unwrap();
        let delta = since(before);
        assert!(
            !delta.gap,
            "gap oldest={:?} cursor={}",
            delta.oldest,
            delta.cursor
        );
        let ops: Vec<_> = delta
            .events
            .iter()
            .filter(|event| {
                let event_seq = event["seq"].as_u64().unwrap_or(0);
                event_seq > before
                    && event_seq <= seq
                    && (event["args"]["uid"] == "lib-kokoro-onnx"
                        || event["args"]["playing"] == "lib-kokoro-onnx")
            })
            .map(|event| event["op"].as_str().unwrap_or(""))
            .collect();
        let focus_at = ops.iter().position(|op| *op == "focus");
        let play_at = ops.iter().position(|op| *op == "play");
        assert!(
            focus_at.is_some() && play_at.is_some() && focus_at < play_at,
            "focus then play, got {ops:?}"
        );
        let auto =
            ui_playback(&json!({"tileId": "lib-kokoro", "action": "play", "origin": "auto"}))
                .unwrap();
        assert_eq!(auto["args"]["origin"], "auto");
        let after_auto = since(seq);
        assert!(
            !after_auto.gap,
            "gap oldest={:?} cursor={}",
            after_auto.oldest,
            after_auto.cursor
        );
        let auto_ops: Vec<_> = after_auto
            .events
            .iter()
            .filter(|event| {
                event["args"]["playing"] == "lib-kokoro" || event["args"]["uid"] == "lib-kokoro"
            })
            .map(|event| event["op"].as_str().unwrap_or(""))
            .collect();
        assert!(auto_ops.contains(&"play"), "{auto_ops:?}");
        assert!(!auto_ops.contains(&"focus"), "{auto_ops:?}");
        assert_eq!(viewport_get()["ui"]["focus"], "lib-kokoro-onnx");
        assert_eq!(viewport_get()["ui"]["clock"]["source"], "lib-kokoro-onnx");
    }

    /// T17 8b and 8c. Each deprecated call succeeds, carries one warning with
    /// its own code, and writes one stderr line. The bare form does not also
    /// report `deprecated_back_section`.
    #[test]
    fn t17_section_deprecations_warn_once_and_select_the_face() {
        let _viewport = fresh_viewport();
        let (bare, bare_err) = stderr_during(|| {
            ui_flip(&json!({"tileId": "lib-misaki-kokoro", "section": "honesty"})).unwrap()
        });
        assert_eq!(stderr_lines(&bare_err).len(), 1, "{bare_err:?}");
        assert!(
            stderr_lines(&bare_err)[0].contains("deprecated_bare_section"),
            "{bare_err}"
        );
        assert!(
            !bare_err.contains("deprecated_back_section"),
            "{bare_err}"
        );
        let warnings = bare["warnings"].as_array().expect(&bare.to_string());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0]["code"], "deprecated_bare_section");
        assert!(warnings[0]["detail"].as_str().unwrap().contains("honesty"));
        assert_eq!(bare["face_id"], "clip");
        assert_eq!(bare["face_index"], 0);
        assert_eq!(bare["face_count"], 5);
        assert_eq!(
            viewport_get()["ui"]["faces"]["view:lib-misaki-kokoro"]["face_id"],
            "clip"
        );

        let (backed, back_err) = stderr_during(|| {
            ui_flip(&json!({
                "tileId": "lib-misaki-kokoro",
                "face": "back",
                "section": "honesty"
            }))
            .unwrap()
        });
        assert_eq!(stderr_lines(&back_err).len(), 1, "{back_err:?}");
        assert!(
            stderr_lines(&back_err)[0].contains("deprecated_back_section"),
            "{back_err}"
        );
        assert!(
            !back_err.contains("deprecated_bare_section"),
            "{back_err}"
        );
        let back_warnings = backed["warnings"].as_array().expect(&backed.to_string());
        assert_eq!(back_warnings.len(), 1, "{back_warnings:?}");
        assert_eq!(back_warnings[0]["code"], "deprecated_back_section");
        assert_eq!(backed["face_id"], "clip");
        assert_eq!(backed["face_index"], 0);

        let plain = ui_flip(&json!({"tileId": "lib-kokoro", "next": true})).unwrap();
        assert!(plain.get("warnings").is_none(), "{plain}");
    }

    fn stderr_lines(text: &str) -> Vec<&str> {
        text.lines().filter(|line| !line.is_empty()).collect()
    }

    fn stderr_during(f: impl FnOnce() -> Value) -> (Value, String) {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        extern "C" {
            fn pipe(fds: *mut i32) -> i32;
            fn dup(fd: i32) -> i32;
            fn dup2(old: i32, new: i32) -> i32;
            fn close(fd: i32) -> i32;
            fn read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize;
        }
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0, "pipe");
        let saved = unsafe { dup(2) };
        assert!(saved >= 0, "dup");
        assert_eq!(unsafe { dup2(fds[1], 2) }, 2, "dup2");
        let value = f();
        assert_eq!(unsafe { dup2(saved, 2) }, 2, "restore");
        unsafe {
            close(fds[1]);
            close(saved);
        }
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = unsafe { read(fds[0], tmp.as_mut_ptr() as *mut std::ffi::c_void, tmp.len()) };
            if n <= 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n as usize]);
        }
        unsafe { close(fds[0]) };
        (value, String::from_utf8_lossy(&buf).into_owned())
    }
}
