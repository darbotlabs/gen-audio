//! Canonical viewport reducer.
//!
//! The desktop, MCP, and ACP all read this store. TypeScript renders it.
//! The clock keeps `source` plus the last committed Seek. A user Play emits
//! focus, then play, and rebinds the cube. Autoplay, scroll, resync, and
//! hover do not. Playback frames never enter the reducer.
//!
//! Coverage uses the cube's own `sec_per_bin`. Reject reasons stay in one order:
//! start < 0, selector end, clip missing from `src`, recorded source duration lie.

use std::collections::BTreeMap;
#[cfg(any(test, feature = "test-support"))]
use std::cell::RefCell;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::catalog;

const MEDIA_FRAGMENTS: &str = "http://www.w3.org/TR/media-frags/";
const PARTIAL_BELOW: f64 = 0.95;

#[derive(Clone, Debug)]
pub struct ReduceError {
    pub code: i32,
    pub message: String,
}

impl ReduceError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Auto,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::Auto => "auto",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, ReduceError> {
        match raw {
            "user" => Ok(Origin::User),
            "auto" => Ok(Origin::Auto),
            _ => Err(ReduceError::invalid("origin must be user or auto")),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    Navigate {
        slide: String,
    },
    Focus {
        uid: Option<String>,
    },
    Compare {
        uids: Vec<String>,
    },
    Seek {
        uid: String,
        t: f64,
    },
    Play {
        uid: String,
        origin: Origin,
    },
    Pause {
        uid: String,
    },
    Flip {
        view: String,
        face: String,
        section: Option<String>,
    },
    Rename {
        uid: String,
        name: String,
    },
    Generate {
        prompt_ref: String,
        personas: Vec<String>,
        voice: String,
        duration_s: f64,
        focus: bool,
        job: String,
    },
    Job {
        job: String,
        target: Option<String>,
        phase: String,
        reason: Option<String>,
        kind: Option<String>,
        synthesized: bool,
        coverage: Option<CoverageInput>,
    },
    Superseded {
        old: String,
        next: String,
    },
    Rebind {
        view: String,
        next: String,
    },
}

#[derive(Clone, Debug)]
struct Asset {
    kind: String,
    title: String,
    honesty: String,
    #[allow(dead_code)]
    duration_s: Option<f64>,
    media: String,
    display_rev: u64,
}

#[derive(Clone, Debug)]
struct View {
    id: String,
    asset: String,
    home: String,
    reference: bool,
}

#[derive(Clone, Debug)]
struct Slide {
    id: String,
    title: String,
    members: Vec<String>,
    query_kind: Option<String>,
    empty: Option<String>,
}

#[derive(Clone, Debug)]
struct Annotation {
    id: String,
    body: String,
    source: String,
    start_s: f64,
    end_s: f64,
    sec_per_bin: f64,
}

#[derive(Clone, Debug)]
struct JobRec {
    id: String,
    phase: String,
    target: Option<String>,
    reason: Option<String>,
    kind: String,
    focus: bool,
    voice: String,
    prompt_ref: String,
    duration_s: f64,
    personas: Vec<String>,
    synthesized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FaceDef {
    id: &'static str,
    name: &'static str,
}

const CLIP_FACES: &[FaceDef] = &[
    FaceDef { id: "clip", name: "Clip" },
    FaceDef { id: "cube", name: "Cube" },
    FaceDef { id: "layers", name: "Layers" },
    FaceDef { id: "spectrogram", name: "Spectrogram" },
    FaceDef { id: "relations", name: "Relations" },
];
const MODEL_FACES: &[FaceDef] = &[
    FaceDef { id: "model", name: "Model" },
    FaceDef { id: "cubes", name: "Cubes" },
    FaceDef { id: "relations", name: "Relations" },
];
const CONNECTOR_FACE: FaceDef = FaceDef { id: "connector", name: "Connector" };
const PROFILE_FACES: &[FaceDef] = &[
    FaceDef { id: "profile", name: "Profile" },
    FaceDef { id: "persona", name: "Persona" },
    FaceDef { id: "relations", name: "Relations" },
];

/// Release deck tile id → `VoiceModel.id`. Card ids are not catalog engine ids.
const RELEASE_ENGINES: &[(&str, &str)] = &[
    ("engine-kokoro", "kokoro_onnx"),
    ("engine-vibevoice", "vibevoice"),
    ("engine-magpie", "magpie"),
    ("engine-pocket", "pocket_tts"),
    ("engine-kokoro-dayour", "kokoro_dayour"),
    ("engine-misaki", "misaki"),
];

const RELEASE_CONNECTORS: &[&str] = &["mcp", "acp", "harness", "copilot", "claude", "gpt", "gemini"];

#[derive(Clone, Debug)]
pub struct Viewport {
    slides: Vec<Slide>,
    views: Vec<View>,
    assets: BTreeMap<String, Asset>,
    annotations: Vec<Annotation>,
    jobs: Vec<JobRec>,
    focus: Option<String>,
    compare: Vec<String>,
    clock_source: Option<String>,
    /// Set only by Seek. Absent until the first committed seek.
    clock_t: Option<f64>,
    playing: Option<String>,
    slide: String,
    /// Explicit flips only. A missing view reads as index 0.
    face_index: BTreeMap<String, usize>,
    names: BTreeMap<String, String>,
}

fn install_deck_view(vp: &mut Viewport, id: &str, title: &str, kind: &str, home: &str, honesty: &str) {
    vp.assets.insert(
        id.to_string(),
        Asset {
            kind: kind.into(),
            title: title.to_string(),
            honesty: honesty.into(),
            duration_s: None,
            media: "absent".into(),
            display_rev: 1,
        },
    );
    let view_id = format!("view:{id}");
    vp.views.push(View {
        id: view_id.clone(),
        asset: id.to_string(),
        home: home.into(),
        reference: false,
    });
    append_member(&mut vp.slides, home, &view_id);
}

/// Install one view per enveloped clip. An envelope whose tile id is not in
/// `library_clips()` is a data error: the deck and the views would otherwise
/// diverge.
fn install_enveloped_library_views(vp: &mut Viewport, envelopes: &[Value]) -> Result<(), String> {
    for asset in envelopes {
        let id = asset.get("legacy_id").and_then(Value::as_str).unwrap_or("");
        let Some(clip) = catalog::library_clip(id) else {
            return Err(format!("audio_clip envelope {id} has no catalog row"));
        };
        install_library_view(vp, clip.id, clip.title, clip.synthesized_speech, clip.wav_url.is_some());
    }
    Ok(())
}

fn install_library_view(
    vp: &mut Viewport,
    id: &str,
    title: &str,
    synthesized_speech: bool,
    wav_present: bool,
) {
    let honesty = if synthesized_speech {
        "library"
    } else if !wav_present {
        "unavailable"
    } else {
        "library"
    };
    vp.assets.insert(
        id.to_string(),
        Asset {
            kind: "audio_clip".into(),
            title: title.to_string(),
            honesty: honesty.into(),
            duration_s: None,
            media: if wav_present { "present" } else { "wav_missing" }.into(),
            display_rev: 1,
        },
    );
    let view_id = format!("view:{id}");
    vp.views.push(View {
        id: view_id.clone(),
        asset: id.to_string(),
        home: "slide:library".into(),
        reference: false,
    });
    if let Some(members) = vp.slides.iter_mut().find(|item| item.id == "slide:library") {
        members.members.push(view_id);
    }
}

impl Viewport {
    /// Fresh install. Library clips that exist stay `library` or `unavailable`.
    /// The pipeline slide has no cube and no fixture view.
    pub fn release() -> Self {
        let mut vp = Self {
            slides: vec![
                slide("slide:models", "Voice models", None, None),
                slide("slide:profiles", "Voice profiles", None, None),
                slide("slide:studio", "Studio", Some("spectrogram"), None),
                slide("slide:library", "Library", Some("audio_clip"), None),
                slide("slide:video", "Video", Some("video"), None),
                slide("slide:spatial", "Spatial", Some("cube"), None),
                slide("slide:connectors", "Connectors", None, None),
                slide(
                    "slide:pipeline",
                    "Pipeline",
                    None,
                    Some("no pipeline output yet"),
                ),
            ],
            views: Vec::new(),
            assets: BTreeMap::new(),
            annotations: Vec::new(),
            jobs: Vec::new(),
            focus: None,
            compare: Vec::new(),
            clock_source: None,
            clock_t: None,
            playing: None,
            slide: "slide:library".into(),
            face_index: BTreeMap::new(),
            names: BTreeMap::new(),
        };
        // Every baked audio_clip envelope must already be a catalog row.
        // Envelope-less catalog tiles are added below. Member order is the
        // baked catalog's uid order, then those envelope-less tiles. The
        // contract pins the set of tile ids, not this order. The desktop
        // paints the library deck from its own cardIds list.
        let envelopes: Vec<Value> = crate::asset_catalog::baked_audio_clip_ids()
            .iter()
            .filter_map(|id| crate::asset_catalog::baked_asset("audio_clip", id))
            .collect();
        install_enveloped_library_views(&mut vp, &envelopes)
            .expect("release library envelopes");
        for clip in catalog::library_clips() {
            if envelopes.iter().any(|asset| asset.get("legacy_id").and_then(Value::as_str) == Some(clip.id)) {
                continue;
            }
            install_library_view(
                &mut vp,
                clip.id,
                clip.title,
                clip.synthesized_speech,
                clip.wav_url.is_some(),
            );
        }
        for (tile, engine_id) in RELEASE_ENGINES {
            let title = catalog::voice_model(engine_id).map(|model| model.label).unwrap_or(engine_id);
            install_deck_view(&mut vp, tile, title, "voice_model", "slide:models", "library");
        }
        for connector in RELEASE_CONNECTORS {
            let tile = format!("conn-{connector}");
            install_deck_view(&mut vp, &tile, connector, "connector", "slide:connectors", "connector");
        }
        for person in catalog::personas() {
            let id = format!("profile-{}", person.id);
            vp.assets.insert(
                id.clone(),
                Asset {
                    kind: "voice_profile".into(),
                    title: person.name.to_string(),
                    honesty: "profile".into(),
                    duration_s: None,
                    media: "absent".into(),
                    display_rev: 1,
                },
            );
            let view_id = format!("view:{id}");
            vp.views.push(View {
                id: view_id.clone(),
                asset: id,
                home: "slide:profiles".into(),
                reference: false,
            });
            append_member(&mut vp.slides, "slide:profiles", &view_id);
        }
        vp
    }

    pub fn apply(&mut self, action: Action) -> Result<Value, ReduceError> {
        match action {
            Action::Navigate { slide } => {
                let resolved = resolve_slide(&slide)?;
                self.slide = resolved.canonical.clone();
                let mut body = json!({"op": "navigate", "slide": resolved.canonical});
                if resolved.deprecated_alias {
                    let line = resolved.log.clone().unwrap_or_default();
                    eprintln!("gen-audio: {line}");
                    body["deprecatedAlias"] = json!(true);
                    body["log"] = json!(line);
                }
                Ok(body)
            }
            Action::Focus { uid } => {
                if let Some(uid) = uid.as_deref() {
                    self.require_asset(uid)?;
                    self.focus = Some(uid.to_string());
                } else {
                    self.focus = None;
                }
                Ok(json!({"op": "focus", "uid": self.focus}))
            }
            Action::Compare { uids } => {
                if uids.len() > 2 {
                    return Err(ReduceError::invalid("compare.length 3 is not allowed"));
                }
                for uid in &uids {
                    self.require_asset(uid)?;
                }
                self.compare = uids;
                Ok(json!({"op": "compare", "uids": self.compare}))
            }
            Action::Seek { uid, t } => {
                self.require_asset(&uid)?;
                if t < 0.0 || !t.is_finite() {
                    return Err(ReduceError::invalid("seek t must be >= 0"));
                }
                self.clock_source = Some(uid.clone());
                self.clock_t = Some(t);
                Ok(json!({"op": "seek", "clock": {"source": uid, "t": t}}))
            }
            Action::Play { uid, origin } => {
                self.require_asset(&uid)?;
                let mut events = Vec::new();
                if origin == Origin::User {
                    self.focus = Some(uid.clone());
                    self.clock_source = Some(uid.clone());
                    events.push(json!({"op": "focus", "uid": uid, "origin": "user"}));
                }
                self.playing = Some(uid.clone());
                events.push(json!({
                    "op": "play",
                    "playing": uid,
                    "origin": origin.as_str(),
                    "focus": self.focus,
                    "clock": {"source": self.clock_source}
                }));
                Ok(json!({
                    "op": "play",
                    "origin": origin.as_str(),
                    "events": events,
                    "focus": self.focus,
                    "playing": uid,
                    "clock": {"source": self.clock_source}
                }))
            }
            Action::Pause { uid } => {
                self.require_asset(&uid)?;
                if self.playing.as_deref() == Some(uid.as_str()) {
                    self.playing = None;
                }
                Ok(
                    json!({"op": "pause", "playing": self.playing, "clock": {"source": self.clock_source}}),
                )
            }
            Action::Flip {
                view,
                face,
                section,
            } => {
                if face != "front" && face != "back" {
                    return Err(ReduceError::invalid("face must be front or back"));
                }
                if !self.views.iter().any(|item| item.id == view) {
                    return Err(ReduceError::invalid(format!("unknown view {view}")));
                }
                let count = self.faces_for(&view).len();
                let index = if face == "front" || count <= 1 {
                    0
                } else {
                    1
                };
                self.face_index.insert(view.clone(), index);
                Ok(json!({"op": "flip", "view": view, "face": face, "section": section}))
            }
            Action::Rename { uid, name } => {
                let asset = self
                    .assets
                    .get_mut(&uid)
                    .ok_or_else(|| ReduceError::invalid(format!("unknown asset {uid}")))?;
                let len = name.chars().count();
                if !(1..=80).contains(&len) {
                    return Err(ReduceError::invalid("name length is out of range"));
                }
                asset.display_rev = asset.display_rev.saturating_add(1);
                asset.title = name.clone();
                self.names.insert(uid.clone(), name.clone());
                Ok(
                    json!({"op": "rename", "uid": uid, "name": name, "display_rev": asset.display_rev}),
                )
            }
            Action::Generate {
                prompt_ref,
                personas,
                voice,
                duration_s,
                focus,
                job,
            } => {
                if self.jobs.iter().any(|item| item.id == job) {
                    return Ok(json!({"op": "generate", "job": job, "idempotent": true}));
                }
                self.jobs.push(JobRec {
                    id: job.clone(),
                    phase: "queued".into(),
                    target: None,
                    reason: None,
                    kind: "audio_clip".into(),
                    focus,
                    voice,
                    prompt_ref,
                    duration_s,
                    personas,
                    synthesized: false,
                });
                Ok(json!({"op": "generate", "job": job, "phase": "queued", "landed": false}))
            }
            Action::Job {
                job,
                target,
                phase,
                reason,
                kind,
                synthesized,
                coverage,
            } => self.apply_job(job, target, phase, reason, kind, synthesized, coverage),
            Action::Superseded { old, next } => {
                self.require_asset(&old)?;
                self.require_asset(&next)?;
                Ok(json!({"op": "superseded", "old": old, "next": next, "rebound": false}))
            }
            Action::Rebind { view, next } => {
                self.require_asset(&next)?;
                let Some(found) = self.views.iter_mut().find(|item| item.id == view) else {
                    return Err(ReduceError::invalid(format!("unknown view {view}")));
                };
                found.asset = next.clone();
                Ok(json!({"op": "rebind", "view": view, "asset": next}))
            }
        }
    }

    fn apply_job(
        &mut self,
        job: String,
        target: Option<String>,
        phase: String,
        reason: Option<String>,
        kind: Option<String>,
        synthesized: bool,
        coverage: Option<CoverageInput>,
    ) -> Result<Value, ReduceError> {
        let kind = kind.unwrap_or_else(|| "audio_clip".into());
        if let Some(existing) = self.jobs.iter_mut().find(|item| item.id == job) {
            existing.phase = phase.clone();
            existing.target = target.clone();
            existing.reason = reason.clone();
            existing.kind = kind.clone();
            existing.synthesized = synthesized;
        } else {
            self.jobs.push(JobRec {
                id: job.clone(),
                phase: phase.clone(),
                target: target.clone(),
                reason: reason.clone(),
                kind: kind.clone(),
                focus: true,
                voice: String::new(),
                prompt_ref: String::new(),
                duration_s: 0.0,
                personas: Vec::new(),
                synthesized,
            });
        }
        if phase == "refused" || phase == "unavailable" || phase == "failed" {
            if let Some(existing) = self.jobs.iter_mut().find(|item| item.id == job) {
                existing.target = None;
                existing.synthesized = false;
            }
            return Ok(json!({
                "op": "job",
                "job": job,
                "phase": phase,
                "reason": reason,
                "target": Value::Null,
                "landed": false,
                "createdUid": Value::Null
            }));
        }
        if phase != "done" {
            return Ok(json!({"op": "job", "job": job, "phase": phase, "landed": false}));
        }
        let Some(uid) = target.clone() else {
            return Err(ReduceError::invalid("job done needs a target uid"));
        };
        if !synthesized {
            return Err(ReduceError::invalid(
                "refusing to land a real view without synthesized audio",
            ));
        }
        let focus = self
            .jobs
            .iter()
            .find(|item| item.id == job)
            .map(|item| item.focus)
            .unwrap_or(true);
        if kind == "audio_clip" {
            let created = self.land_clip(&uid, focus);
            let spec = derived_id(&uid, "spectrogram");
            let cube = derived_id(&uid, "cube");
            Ok(json!({
                "op": "job",
                "job": job,
                "phase": "done",
                "target": uid,
                "landed": true,
                "created": created,
                "spectrogram": {"asset": spec, "honesty": self.honesty_of(&spec)},
                "cube": {"asset": cube, "honesty": self.honesty_of(&cube)},
                "focus": self.focus,
                "clock": {"source": self.clock_source}
            }))
        } else if kind == "spectrogram" {
            self.promote_derived(&uid, &kind)?;
            Ok(json!({
                "op": "job",
                "job": job,
                "phase": "done",
                "target": uid,
                "honesty": "real",
                "landed": true
            }))
        } else if kind == "cube" {
            let Some(coverage) = coverage else {
                self.refuse_job(&job, "cube job has no coverage");
                return Ok(json!({
                    "op": "job",
                    "job": job,
                    "phase": "refused",
                    "reason": "cube job has no coverage",
                    "target": uid,
                    "landed": false
                }));
            };
            if let Err(reason) = validate_coverage(&coverage) {
                self.refuse_job(&job, &reason);
                return Ok(json!({
                    "op": "job",
                    "job": job,
                    "phase": "refused",
                    "reason": reason,
                    "target": uid,
                    "landed": false
                }));
            }
            if let Some(annotation) = self.annotations.iter_mut().find(|item| item.body == uid) {
                annotation.start_s = coverage.selector_start;
                annotation.end_s = coverage.selector_end;
                annotation.sec_per_bin = coverage.sec_per_bin;
            }
            self.promote_derived(&uid, &kind)?;
            Ok(json!({
                "op": "job",
                "job": job,
                "phase": "done",
                "target": uid,
                "honesty": "real",
                "landed": true,
                "sec_per_bin": coverage.sec_per_bin
            }))
        } else if kind == "video" {
            self.promote_derived(&uid, &kind)?;
            Ok(json!({
                "op": "job",
                "job": job,
                "phase": "done",
                "target": uid,
                "honesty": "real",
                "landed": true
            }))
        } else {
            Err(ReduceError::invalid(format!("unknown job kind {kind}")))
        }
    }

    fn refuse_job(&mut self, job: &str, reason: &str) {
        if let Some(existing) = self.jobs.iter_mut().find(|item| item.id == job) {
            existing.phase = "refused".into();
            existing.reason = Some(reason.to_string());
            existing.synthesized = false;
        }
    }

    fn land_clip(&mut self, uid: &str, focus: bool) -> bool {
        if self.assets.contains_key(uid)
            && self
                .views
                .iter()
                .any(|view| view.asset == uid && !view.reference)
        {
            return false;
        }
        self.assets.entry(uid.to_string()).or_insert_with(|| Asset {
            kind: "audio_clip".into(),
            title: uid.to_string(),
            honesty: "real".into(),
            duration_s: None,
            media: "present".into(),
            display_rev: 1,
        });
        if let Some(asset) = self.assets.get_mut(uid) {
            if asset.honesty == "pending" {
                asset.honesty = "real".into();
            }
        }
        let view_id = format!("view:{uid}");
        if !self.views.iter().any(|view| view.id == view_id) {
            self.views.push(View {
                id: view_id.clone(),
                asset: uid.to_string(),
                home: "slide:library".into(),
                reference: false,
            });
            append_member(&mut self.slides, "slide:library", &view_id);
        }
        let spec = derived_id(uid, "spectrogram");
        let cube = derived_id(uid, "cube");
        let video = derived_id(uid, "video");
        self.ensure_derived(&spec, "spectrogram", "slide:studio", "pending");
        self.ensure_derived(&cube, "cube", "slide:spatial", "pending");
        self.ensure_derived(&video, "video", "slide:video", "pending");
        append_member(&mut self.slides, "slide:pipeline", &format!("view:{cube}"));
        if let Some(pipeline) = self
            .slides
            .iter_mut()
            .find(|slide| slide.id == "slide:pipeline")
        {
            if !pipeline.members.is_empty() {
                pipeline.empty = None;
            }
        }
        let annotation_id = format!("ann:{uid}:clock");
        if !self.annotations.iter().any(|item| item.id == annotation_id) {
            self.annotations.push(Annotation {
                id: annotation_id,
                body: cube,
                source: uid.to_string(),
                start_s: 0.0,
                end_s: 0.0,
                sec_per_bin: 0.0,
            });
        }
        if focus {
            self.focus = Some(uid.to_string());
            self.clock_source = Some(uid.to_string());
        }
        true
    }

    fn ensure_derived(&mut self, asset: &str, kind: &str, home: &str, honesty: &str) {
        self.assets
            .entry(asset.to_string())
            .or_insert_with(|| Asset {
                kind: kind.into(),
                title: asset.to_string(),
                honesty: honesty.into(),
                duration_s: None,
                media: "pending".into(),
                display_rev: 1,
            });
        let view_id = format!("view:{asset}");
        if !self.views.iter().any(|view| view.id == view_id) {
            self.views.push(View {
                id: view_id.clone(),
                asset: asset.to_string(),
                home: home.to_string(),
                reference: false,
            });
            append_member(&mut self.slides, home, &view_id);
        }
    }

    fn promote_derived(&mut self, uid: &str, kind: &str) -> Result<(), ReduceError> {
        let asset = self
            .assets
            .get_mut(uid)
            .ok_or_else(|| ReduceError::invalid(format!("unknown derived asset {uid}")))?;
        if asset.kind != kind {
            return Err(ReduceError::invalid(
                "derived job kind does not match the asset",
            ));
        }
        asset.honesty = "real".into();
        asset.media = "present".into();
        Ok(())
    }

    fn honesty_of(&self, uid: &str) -> String {
        self.assets
            .get(uid)
            .map(|asset| asset.honesty.clone())
            .unwrap_or_else(|| "unresolved".into())
    }

    fn require_asset(&self, uid: &str) -> Result<(), ReduceError> {
        if self.assets.contains_key(uid) {
            Ok(())
        } else {
            Err(ReduceError::invalid(format!("unknown asset {uid}")))
        }
    }

    /// Ordered faces that apply to this view. Omitted faces are absent, so `len()` is M.
    fn faces_for(&self, view_id: &str) -> Vec<FaceDef> {
        let Some(view) = self.views.iter().find(|item| item.id == view_id) else {
            return Vec::new();
        };
        let kind = self.assets.get(&view.asset).map(|asset| asset.kind.as_str()).unwrap_or("");
        let tile = view_id.strip_prefix("view:").unwrap_or(view_id);
        match kind {
            "audio_clip" => {
                let wav = self.assets.get(&view.asset).is_some_and(|asset| asset.media == "present");
                CLIP_FACES
                    .iter()
                    .copied()
                    .filter(|face| match face.id {
                        "cube" | "layers" | "spectrogram" => wav,
                        "relations" => clip_has_relation(tile),
                        _ => true,
                    })
                    .collect()
            }
            "voice_model" => {
                let Some(engine_id) = RELEASE_ENGINES
                    .iter()
                    .find(|(tile_id, _)| *tile_id == tile)
                    .map(|(_, engine_id)| *engine_id)
                else {
                    return vec![MODEL_FACES[0]];
                };
                MODEL_FACES
                    .iter()
                    .copied()
                    .filter(|face| match face.id {
                        "cubes" => engine_has_cube_row(engine_id),
                        "relations" => engine_has_link(engine_id),
                        _ => true,
                    })
                    .collect()
            }
            "connector" => vec![CONNECTOR_FACE],
            "voice_profile" => {
                let persona = tile.strip_prefix("profile-").unwrap_or(tile);
                PROFILE_FACES
                    .iter()
                    .copied()
                    .filter(|face| match face.id {
                        "persona" => persona_face_applies(persona),
                        "relations" => profile_names_a_voice_model(persona),
                        _ => true,
                    })
                    .collect()
            }
            "cube" => vec![FaceDef { id: "cube", name: "Cube" }],
            "spectrogram" => vec![FaceDef { id: "spectrogram", name: "Spectrogram" }],
            "video" => vec![FaceDef { id: "video", name: "Video" }],
            _ => Vec::new(),
        }
    }

    pub fn snapshot(&self) -> Value {
        let views: Vec<Value> = self
            .views
            .iter()
            .filter(|view| !view.reference)
            .map(|view| {
                let asset = self.assets.get(&view.asset);
                let honesty = asset.map(|item| item.honesty.as_str()).unwrap_or("unresolved");
                let title = asset.map(|item| item.title.as_str()).unwrap_or("");
                let kind = asset.map(|item| item.kind.as_str()).unwrap_or("");
                let media = asset.map(|item| item.media.as_str()).unwrap_or("unresolved");
                let display_rev = asset.map(|item| item.display_rev).unwrap_or(0);
                json!({
                    "id": view.id,
                    "asset": view.asset,
                    "home": view.home,
                    "snapshot": facts_snapshot(&view.asset, kind, title, honesty, media, display_rev)
                })
            })
            .collect();
        let slides: Vec<Value> = self
            .slides
            .iter()
            .map(|slide| {
                let mut value = json!({
                    "id": slide.id,
                    "title": slide.title,
                    "members": slide.members,
                });
                if let Some(kind) = &slide.query_kind {
                    value["query"] = json!({"kind": kind});
                }
                if let Some(empty) = &slide.empty {
                    value["empty"] = json!(empty);
                }
                value
            })
            .collect();
        let jobs: Vec<Value> = self
            .jobs
            .iter()
            .map(|job| {
                json!({
                    "job": job.id,
                    "phase": job.phase,
                    "target": job.target,
                    "reason": job.reason,
                    "kind": job.kind,
                    "synthesized": job.synthesized,
                    "voice": job.voice,
                    "prompt_ref": job.prompt_ref,
                    "duration_s": job.duration_s,
                    "personas": job.personas
                })
            })
            .collect();
        let annotations: Vec<Value> = self
            .annotations
            .iter()
            .map(|item| {
                json!({
                    "id": item.id,
                    "motivation": "describing",
                    "body": item.body,
                    "target": {
                        "source": item.source,
                        "selector": {
                            "type": "FragmentSelector",
                            "conformsTo": MEDIA_FRAGMENTS,
                            "value": format!("t={},{}", trim_seconds(item.start_s), trim_seconds(item.end_s))
                        }
                    },
                    "sec_per_bin": item.sec_per_bin
                })
            })
            .collect();
        let mut clock = json!({"source": self.clock_source});
        if let Some(t) = self.clock_t {
            clock["t"] = json!(t);
        }
        json!({
            "ok": true,
            "synthesizedSpeech": false,
            "slides": slides,
            "views": views,
            "annotations": annotations,
            "jobs": jobs,
            "ui": {
                "focus": self.focus,
                "compare": self.compare,
                "clock": clock,
                "playing": self.playing,
                "slide": self.slide,
                "flipped": self.face_index.iter().map(|(id, index)| {
                    let face = if *index == 0 { "front" } else { "back" };
                    (id.clone(), json!({"face": face, "section": Value::Null}))
                }).collect::<serde_json::Map<String, Value>>(),
                "faces": self.views.iter().filter(|view| !view.reference).map(|view| {
                    let applicable = self.faces_for(&view.id);
                    let index = self.face_index.get(&view.id).copied().unwrap_or(0).min(applicable.len().saturating_sub(1));
                    let current = applicable.get(index);
                    let face_id = current.map(|face| face.id).unwrap_or("");
                    let face_name = current.map(|face| face.name).unwrap_or("");
                    let tile = view.id.strip_prefix("view:").unwrap_or(view.id.as_str());
                    (view.id.clone(), json!({
                        "tileId": tile,
                        "face_id": face_id,
                        "face_name": face_name,
                        "face_index": index,
                        "face_count": applicable.len(),
                        "ids": applicable.iter().map(|face| face.id).collect::<Vec<_>>()
                    }))
                }).collect::<serde_json::Map<String, Value>>()
            },
            "pipelineEmpty": self.slides.iter().find(|slide| slide.id == "slide:pipeline").and_then(|slide| slide.empty.clone())
        })
    }

    pub fn export_card(&self, uid: &str) -> Result<Value, ReduceError> {
        let asset = self
            .assets
            .get(uid)
            .ok_or_else(|| ReduceError::invalid(format!("unknown asset {uid}")))?;
        Ok(adaptive_card(
            &asset.title,
            &asset.kind,
            &asset.honesty,
            &asset.media,
            asset.display_rev,
        ))
    }
}

fn catalog_rows() -> Vec<Value> {
    crate::asset_catalog::assets()
}

fn model_row(engine_id: &str) -> Option<Value> {
    catalog_rows().into_iter().find(|asset| {
        asset.get("kind").and_then(Value::as_str) == Some("voice_model")
            && asset.get("legacy_id").and_then(Value::as_str) == Some(engine_id)
    })
}

fn clips_naming(engine_id: &str, key: &str) -> Vec<Value> {
    let Some(uid) = model_row(engine_id).and_then(|row| row.get("uid").and_then(Value::as_str).map(str::to_string)) else {
        return Vec::new();
    };
    catalog_rows()
        .into_iter()
        .filter(|asset| {
            asset.get("kind").and_then(Value::as_str) == Some("audio_clip")
                && asset
                    .pointer(&format!("/provenance/{key}"))
                    .and_then(Value::as_str)
                    == Some(uid.as_str())
        })
        .collect()
}

fn envelope_has_wav(asset: &Value) -> bool {
    asset.get("media").and_then(Value::as_array).is_some_and(|items| {
        items.iter().any(|item| item.get("role").and_then(Value::as_str) == Some("wav"))
    })
}

fn engine_is_g2p(engine_id: &str) -> bool {
    model_row(engine_id).is_some_and(|row| {
        row.pointer("/honesty/claims")
            .and_then(Value::as_array)
            .is_some_and(|claims| claims.iter().any(|claim| claim.as_str() == Some("g2p_only")))
    })
}

/// A cube row exists only when an enveloped clip names this model through
/// `provenance.voice_model` and that clip's WAV is present. `g2p_only` has none.
fn engine_has_cube_row(engine_id: &str) -> bool {
    if engine_is_g2p(engine_id) {
        return false;
    }
    clips_naming(engine_id, "voice_model").iter().any(envelope_has_wav)
}

/// Catalog tiles with no envelope are not link targets. Profiles count when
/// their `voiceModel` is this engine.
fn engine_has_link(engine_id: &str) -> bool {
    if !clips_naming(engine_id, "voice_model").is_empty() || !clips_naming(engine_id, "g2p_model").is_empty() {
        return true;
    }
    catalog::personas().iter().any(|person| {
        catalog::voice_profile_value(person.id)
            .and_then(|value| value.get("voiceModel").and_then(Value::as_str).map(str::to_string))
            .as_deref()
            == Some(engine_id)
    })
}

/// Enveloped clips link through provenance. A tile with no envelope links only
/// when its catalog `engine_id` is a real voice model.
fn clip_has_relation(tile: &str) -> bool {
    if let Some(asset) = crate::asset_catalog::baked_asset("audio_clip", tile) {
        let named = asset.pointer("/provenance/voice_model").and_then(Value::as_str).is_some()
            || asset.pointer("/provenance/g2p_model").and_then(Value::as_str).is_some();
        if named {
            return true;
        }
        let uid = asset.get("uid").and_then(Value::as_str).unwrap_or("");
        return catalog_rows().iter().any(|row| {
            matches!(row.get("kind").and_then(Value::as_str), Some("cube_ihdr" | "spectrogram_2d"))
                && row.get("src").and_then(Value::as_array).is_some_and(|src| src.iter().any(|item| item.as_str() == Some(uid)))
        });
    }
    catalog::library_clip(tile).is_some_and(|clip| catalog::voice_model(clip.engine_id).is_some())
}

fn persona_face_applies(persona_id: &str) -> bool {
    catalog::persona(persona_id).is_some_and(|person| {
        !person.domain.is_empty() || !person.accent.is_empty() || !person.traits.is_empty() || !person.refs.is_empty()
    })
}

fn profile_names_a_voice_model(persona_id: &str) -> bool {
    catalog::voice_profile_value(persona_id)
        .and_then(|value| value.get("voiceModel").and_then(Value::as_str).map(str::to_string))
        .is_some_and(|engine_id| catalog::voice_model(&engine_id).is_some())
}

fn slide(id: &str, title: &str, query_kind: Option<&str>, empty: Option<&str>) -> Slide {
    Slide {
        id: id.into(),
        title: title.into(),
        members: Vec::new(),
        query_kind: query_kind.map(str::to_string),
        empty: empty.map(str::to_string),
    }
}

fn derived_id(uid: &str, kind: &str) -> String {
    format!("{uid}:{kind}")
}

fn append_member(slides: &mut [Slide], slide_id: &str, view_id: &str) {
    if let Some(slide) = slides.iter_mut().find(|item| item.id == slide_id) {
        if !slide.members.iter().any(|item| item == view_id) {
            slide.members.push(view_id.to_string());
        }
    }
}

pub struct ResolvedSlide {
    pub canonical: String,
    pub deprecated_alias: bool,
    pub log: Option<String>,
}

/// `slide:<slug>` is canonical. A bare slug is accepted and reported as a deprecated alias.
pub fn resolve_slide(slide: &str) -> Result<ResolvedSlide, ReduceError> {
    let (slug, deprecated) = match slide.strip_prefix("slide:") {
        Some(slug) => (slug, false),
        None => (slide, true),
    };
    if slug.is_empty() || !catalog::SLIDES.contains(&slug) {
        return Err(ReduceError::invalid(format!("unknown slide {slide}")));
    }
    let canonical = format!("slide:{slug}");
    if deprecated {
        let log = format!("deprecated slide alias '{slide}'; use '{canonical}'");
        Ok(ResolvedSlide {
            canonical,
            deprecated_alias: true,
            log: Some(log),
        })
    } else {
        Ok(ResolvedSlide {
            canonical,
            deprecated_alias: false,
            log: None,
        })
    }
}

pub fn canonical_slide(slide: &str) -> Result<String, ReduceError> {
    Ok(resolve_slide(slide)?.canonical)
}

pub fn facts_snapshot(
    uid: &str,
    kind: &str,
    title: &str,
    honesty: &str,
    media: &str,
    display_rev: u64,
) -> Value {
    json!({
        "snapshot_of": uid,
        "kind": kind,
        "title": title,
        "display_rev": display_rev,
        "honesty": {"state": honesty},
        "media": media,
        "sections": facts(uid, kind, title, honesty, media)
    })
}

pub fn facts(uid: &str, kind: &str, title: &str, honesty: &str, media: &str) -> Vec<Value> {
    vec![
        json!({
            "id": "identity",
            "facts": [
                {"label": "Title", "value": title, "field": "title"},
                {"label": "Kind", "value": kind, "field": "kind"},
                {"label": "Asset", "value": uid, "field": "uid"}
            ]
        }),
        json!({
            "id": "honesty",
            "facts": [
                {"label": "Status", "value": honesty, "field": "honesty"},
                {"label": "Media", "value": media, "field": "media"}
            ]
        }),
    ]
}

pub fn adaptive_card(
    title: &str,
    kind: &str,
    honesty: &str,
    media: &str,
    display_rev: u64,
) -> Value {
    let sections = facts(title, kind, title, honesty, media);
    let mut body = vec![json!({
        "type": "TextBlock",
        "text": title,
        "wrap": true
    })];
    for section in &sections {
        let facts_json: Vec<Value> = section["facts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|fact| json!({"title": fact["label"], "value": fact["value"]}))
            .collect();
        body.push(json!({"type": "TextBlock", "text": section["id"], "wrap": true}));
        body.push(json!({"type": "FactSet", "facts": facts_json}));
    }
    let fallback = format!("{title}. {kind}. {honesty}. {media}. rev {display_rev}.");
    json!({
        "type": "AdaptiveCard",
        "version": "1.5",
        "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
        "fallbackText": fallback,
        "body": body
    })
}

/// Coverage check. Reasons are returned in this fixed order, first match wins:
/// 1. start < 0
/// 2. selector end > clip duration + one bin
/// 3. clip is not in the cube src
/// 4. recorded source duration differs from the clip by more than one bin
pub fn validate_coverage(input: &CoverageInput) -> Result<CoverageOk, String> {
    if input.sec_per_bin <= 0.0 || !input.sec_per_bin.is_finite() {
        return Err("cube sec_per_bin must be positive".into());
    }
    if input.selector_start < 0.0 {
        return Err("start < 0".into());
    }
    let limit = input.clip_duration_s + input.sec_per_bin;
    if input.selector_end > limit + 1e-9 {
        return Err("selector end > clip duration + one bin".into());
    }
    if !input.clip_in_src {
        return Err("clip is not in the cube src".into());
    }
    let duration_delta = (input.recorded_source_duration_s - input.clip_duration_s).abs();
    if duration_delta > input.sec_per_bin + 1e-9 {
        return Err(
            "recorded source duration differs from clip duration by more than one bin".into(),
        );
    }
    if input.selector_end < input.selector_start {
        return Err("selector end is before the start".into());
    }
    let covered = (input.selector_end - input.selector_start).max(0.0);
    let of_s = input.clip_duration_s;
    let ratio = if of_s > 0.0 { covered / of_s } else { 0.0 };
    let partial = if ratio + 1e-12 < PARTIAL_BELOW {
        Some(json!({"covered_s": covered, "of_s": of_s}))
    } else {
        None
    };
    Ok(CoverageOk {
        covered_s: covered,
        of_s,
        ratio,
        partial,
    })
}

#[derive(Clone, Debug)]
pub struct CoverageInput {
    pub selector_start: f64,
    pub selector_end: f64,
    pub clip_duration_s: f64,
    pub recorded_source_duration_s: f64,
    pub sec_per_bin: f64,
    pub clip_in_src: bool,
}

#[derive(Debug)]
pub struct CoverageOk {
    pub covered_s: f64,
    pub of_s: f64,
    pub ratio: f64,
    pub partial: Option<Value>,
}

fn process_viewport() -> &'static Mutex<Viewport> {
    static STATE: OnceLock<Mutex<Viewport>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Viewport::release()))
}

/// One viewport. The desktop uses the process default (one window). A test
/// that asserts a sequence of actions binds its own handle for this thread.
/// Release builds have no binding: the type exists only for tests.
#[derive(Clone)]
#[cfg(any(test, feature = "test-support"))]
pub struct ViewportHandle {
    inner: Arc<Mutex<Viewport>>,
}

#[cfg(any(test, feature = "test-support"))]
impl ViewportHandle {
    pub fn release() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Viewport::release())),
        }
    }

    pub fn apply(&self, action: Action) -> Result<Value, ReduceError> {
        self.inner.lock().expect("viewport").apply(action)
    }

    pub fn snapshot(&self) -> Value {
        self.inner.lock().expect("viewport").snapshot()
    }

    fn contains(&self, uid: &str) -> bool {
        self.inner
            .lock()
            .expect("viewport")
            .assets
            .contains_key(uid)
    }

    fn export(&self, uid: &str) -> Result<Value, ReduceError> {
        self.inner.lock().expect("viewport").export_card(uid)
    }
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static BOUND_VIEWPORT: RefCell<Option<ViewportHandle>> = const { RefCell::new(None) };
}

/// Restores the previous binding, including the process default.
#[cfg(any(test, feature = "test-support"))]
pub struct ViewportGuard {
    previous: Option<ViewportHandle>,
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for ViewportGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        BOUND_VIEWPORT.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// Viewport reads and writes on this thread use `handle` until the guard drops.
#[cfg(any(test, feature = "test-support"))]
pub fn bind_viewport(handle: ViewportHandle) -> ViewportGuard {
    BOUND_VIEWPORT.with(|slot| {
        let previous = slot.borrow_mut().replace(handle);
        ViewportGuard { previous }
    })
}

#[cfg(any(test, feature = "test-support"))]
fn bound_viewport() -> Option<ViewportHandle> {
    BOUND_VIEWPORT.with(|slot| slot.borrow().clone())
}

pub fn apply_global(action: Action) -> Result<Value, ReduceError> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(handle) = bound_viewport() {
        return handle.apply(action);
    }
    process_viewport().lock().expect("viewport").apply(action)
}

pub fn snapshot_global() -> Value {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(handle) = bound_viewport() {
        return handle.snapshot();
    }
    process_viewport().lock().expect("viewport").snapshot()
}

pub fn contains_global(uid: &str) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(handle) = bound_viewport() {
        return handle.contains(uid);
    }
    process_viewport()
        .lock()
        .expect("viewport")
        .assets
        .contains_key(uid)
}

pub fn export_global(uid: &str) -> Result<Value, ReduceError> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(handle) = bound_viewport() {
        return handle.export(uid);
    }
    process_viewport()
        .lock()
        .expect("viewport")
        .export_card(uid)
}

fn trim_seconds(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        let text = format!("{value:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

pub fn release_has_fixture(document: &Value) -> bool {
    document
        .get("views")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|view| {
            honesty_state(&view["snapshot"]["honesty"]) == "fixture"
                || honesty_state(&view["honesty"]) == "fixture"
        })
}

fn honesty_state(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    value
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn misaki() -> CoverageInput {
        CoverageInput {
            selector_start: 0.0,
            selector_end: 139.04,
            clip_duration_s: 139.375,
            recorded_source_duration_s: 139.375,
            sec_per_bin: 0.352,
            clip_in_src: true,
        }
    }

    /// C-M1: after `Viewport::release()`, the `view:lib-*` set is the eight
    /// LibraryClip ids in `viewport.release.json`.
    #[test]
    fn release_library_views_match_the_release_deck() {
        let document: Value =
            serde_json::from_str(include_str!("../../../schemas/examples/viewport.release.json"))
                .unwrap();
        let mut cards: Vec<String> = document["cards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|card| card["kind"] == "LibraryClip")
            .filter_map(|card| card["id"].as_str().map(str::to_string))
            .collect();
        cards.sort();
        let doc = Viewport::release().snapshot();
        let mut views: Vec<String> = doc["views"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|view| view["id"].as_str())
            .filter(|id| id.starts_with("view:lib-"))
            .map(|id| id.trim_start_matches("view:").to_string())
            .collect();
        views.sort();
        assert_eq!(
            views, cards,
            "release view:lib-* set disagrees with viewport.release.json"
        );
        let mut catalog_ids: Vec<String> = catalog::library_clips()
            .iter()
            .map(|clip| clip.id.to_string())
            .collect();
        catalog_ids.sort();
        assert_eq!(
            catalog_ids, cards,
            "library_clips() disagrees with the release deck"
        );
    }

    #[test]
    fn an_orphan_audio_clip_envelope_is_an_error() {
        let mut vp = Viewport::release();
        let before = vp.views.len();
        let orphan = json!({
            "kind": "audio_clip",
            "legacy_id": "lib-orphan-envelope",
            "display": {"title": "Orphan"},
            "honesty": {"synthesized_speech": true},
            "body": {"wav_url": "/library/orphan.wav"}
        });
        let err = install_enveloped_library_views(&mut vp, &[orphan]).unwrap_err();
        assert!(err.contains("lib-orphan-envelope"), "{err}");
        assert_eq!(vp.views.len(), before, "orphan envelope installed a view");
        assert!(vp.views.iter().all(|view| view.asset != "lib-orphan-envelope"));
    }

    /// §2: M is the faces that apply. A full table for every tile, or a join on
    /// catalog `engine_id`, reports a different list.
    #[test]
    fn release_face_lists_follow_the_omission_rule() {
        let doc = Viewport::release().snapshot();
        let faces = &doc["ui"]["faces"];
        let expected: &[(&str, &[&str])] = &[
            ("view:lib-misaki-kokoro", &["clip", "cube", "layers", "spectrogram", "relations"]),
            ("view:lib-cube-explainer", &["clip", "cube", "layers", "spectrogram", "relations"]),
            ("view:lib-bitdot-braille-vibevoice", &["clip", "cube", "layers", "spectrogram", "relations"]),
            ("view:lib-kokoro", &["clip", "cube", "layers", "spectrogram", "relations"]),
            ("view:lib-kokoro-onnx", &["clip", "cube", "layers", "spectrogram", "relations"]),
            ("view:lib-magpie", &["clip", "relations"]),
            ("view:lib-vibevoice", &["clip", "relations"]),
            ("view:lib-pocket", &["clip", "relations"]),
            ("view:engine-kokoro", &["model", "cubes", "relations"]),
            ("view:engine-vibevoice", &["model", "cubes", "relations"]),
            ("view:engine-kokoro-dayour", &["model", "cubes", "relations"]),
            ("view:engine-misaki", &["model", "relations"]),
            ("view:engine-magpie", &["model"]),
            ("view:engine-pocket", &["model"]),
            ("view:conn-mcp", &["connector"]),
            ("view:conn-acp", &["connector"]),
            ("view:conn-harness", &["connector"]),
            ("view:conn-copilot", &["connector"]),
            ("view:conn-claude", &["connector"]),
            ("view:conn-gpt", &["connector"]),
            ("view:conn-gemini", &["connector"]),
            ("view:profile-anton", &["profile", "persona", "relations"]),
            ("view:profile-alice", &["profile", "persona", "relations"]),
            ("view:profile-khortana", &["profile", "persona", "relations"]),
            ("view:profile-rocky", &["profile", "persona", "relations"]),
            ("view:profile-optimus", &["profile", "persona", "relations"]),
        ];
        for (view, ids) in expected {
            let got: Vec<String> = faces[view]["ids"]
                .as_array()
                .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            assert_eq!(got, ids.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(), "{view} faces {faces}");
            assert_eq!(faces[view]["face_count"].as_u64(), Some(ids.len() as u64), "{view}");
            assert_eq!(faces[view]["face_index"].as_u64(), Some(0), "{view}");
            assert_eq!(faces[view]["face_id"].as_str(), ids.first().copied(), "{view}");
        }
    }

    #[test]
    fn t6_release_doc_has_no_fixture_and_pipeline_is_empty() {
        let doc = Viewport::release().snapshot();
        assert!(!release_has_fixture(&doc), "{doc}");
        let pipeline = doc["slides"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slide| slide["id"] == "slide:pipeline")
            .unwrap();
        assert_eq!(pipeline["members"].as_array().unwrap().len(), 0);
        assert_eq!(pipeline["empty"], "no pipeline output yet");
        let states: Vec<_> = doc["views"]
            .as_array()
            .unwrap()
            .iter()
            .map(|view| {
                view["snapshot"]["honesty"]["state"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert!(states.iter().all(|state| state != "fixture"));
        assert!(states.iter().any(|state| state == "library"));
    }

    #[test]
    fn t16_generate_job_done_lands_once_and_derived_views_start_pending() {
        let mut vp = Viewport::release();
        vp.apply(Action::Generate {
            prompt_ref: "prompt.txt".into(),
            personas: vec!["alice".into(), "frank".into()],
            voice: "kokoro_onnx".into(),
            duration_s: 180.0,
            focus: true,
            job: "job-gen".into(),
        })
        .unwrap();
        let done = vp
            .apply(Action::Job {
                job: "job-gen".into(),
                target: Some("clip-brief".into()),
                phase: "done".into(),
                reason: None,
                kind: Some("audio_clip".into()),
                synthesized: true,
                coverage: None,
            })
            .unwrap();
        assert_eq!(done["landed"], true);
        assert_eq!(done["spectrogram"]["honesty"], "pending");
        assert_eq!(done["cube"]["honesty"], "pending");
        let snap = vp.snapshot();
        let library = snap["slides"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slide| slide["id"] == "slide:library")
            .unwrap();
        assert!(library["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "view:clip-brief"));
        assert!(snap["views"]
            .as_array()
            .unwrap()
            .iter()
            .any(|view| view["asset"] == "clip-brief:spectrogram"));
        assert!(snap["views"]
            .as_array()
            .unwrap()
            .iter()
            .any(|view| view["asset"] == "clip-brief:cube"));
        assert!(snap["views"]
            .as_array()
            .unwrap()
            .iter()
            .any(|view| view["asset"] == "clip-brief:video"
                && view["snapshot"]["honesty"]["state"] == "pending"));
        let video_slide = snap["slides"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slide| slide["id"] == "slide:video")
            .unwrap();
        assert!(video_slide["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "view:clip-brief:video"));
        assert!(snap["annotations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|ann| ann["target"]["source"] == "clip-brief"));
        assert_eq!(snap["ui"]["focus"], "clip-brief");
        assert_eq!(snap["ui"]["clock"]["source"], "clip-brief");
        let again = vp
            .apply(Action::Job {
                job: "job-gen".into(),
                target: Some("clip-brief".into()),
                phase: "done".into(),
                reason: None,
                kind: Some("audio_clip".into()),
                synthesized: true,
                coverage: None,
            })
            .unwrap();
        assert_eq!(again["created"], false);
        let count = vp.snapshot()["views"].as_array().unwrap().len();
        assert_eq!(count, snap["views"].as_array().unwrap().len());
        vp.apply(Action::Job {
            job: "job-spec".into(),
            target: Some("clip-brief:spectrogram".into()),
            phase: "done".into(),
            reason: None,
            kind: Some("spectrogram".into()),
            synthesized: true,
            coverage: None,
        })
        .unwrap();
        vp.apply(Action::Job {
            job: "job-cube".into(),
            target: Some("clip-brief:cube".into()),
            phase: "done".into(),
            reason: None,
            kind: Some("cube".into()),
            synthesized: true,
            coverage: Some(CoverageInput {
                selector_start: 0.0,
                selector_end: 139.04,
                clip_duration_s: 139.375,
                recorded_source_duration_s: 139.375,
                sec_per_bin: 0.352,
                clip_in_src: true,
            }),
        })
        .unwrap();
        let after = vp.snapshot();
        let spec = after["views"]
            .as_array()
            .unwrap()
            .iter()
            .find(|view| view["asset"] == "clip-brief:spectrogram")
            .unwrap();
        let cube = after["views"]
            .as_array()
            .unwrap()
            .iter()
            .find(|view| view["asset"] == "clip-brief:cube")
            .unwrap();
        assert_eq!(spec["snapshot"]["honesty"]["state"], "real");
        assert_eq!(cube["snapshot"]["honesty"]["state"], "real");
        let annotation = after["annotations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["body"] == "clip-brief:cube")
            .unwrap();
        assert_eq!(annotation["sec_per_bin"], 0.352);
        assert_eq!(annotation["target"]["selector"]["value"], "t=0,139.04");
        let video = vp
            .apply(Action::Job {
                job: "job-video".into(),
                target: Some("clip-brief:video".into()),
                phase: "done".into(),
                reason: None,
                kind: Some("video".into()),
                synthesized: true,
                coverage: None,
            })
            .unwrap();
        assert_eq!(video["honesty"], "real");
    }

    #[test]
    fn job_moves_queued_to_running_before_done() {
        let mut vp = Viewport::release();
        vp.apply(Action::Generate {
            prompt_ref: "note".into(),
            personas: vec!["alice".into()],
            voice: "kokoro_onnx".into(),
            duration_s: 3.0,
            focus: true,
            job: "job-run".into(),
        })
        .unwrap();
        assert_eq!(vp.snapshot()["jobs"][0]["phase"], "queued");
        let running = vp
            .apply(Action::Job {
                job: "job-run".into(),
                target: None,
                phase: "running".into(),
                reason: None,
                kind: Some("audio_clip".into()),
                synthesized: false,
                coverage: None,
            })
            .unwrap();
        assert_eq!(running["phase"], "running");
        assert_eq!(running["landed"], false);
        assert_eq!(vp.snapshot()["jobs"][0]["phase"], "running");
    }

    #[test]
    fn t8_refused_job_does_not_land_a_real_view() {
        let mut vp = Viewport::release();
        let before = vp.snapshot()["views"].as_array().unwrap().len();
        let refused = vp
            .apply(Action::Job {
                job: "job-refuse".into(),
                target: None,
                phase: "refused".into(),
                reason: Some("GEN_AUDIO_KOKORO_MODEL is unset. No speech was invented.".into()),
                kind: Some("audio_clip".into()),
                synthesized: false,
                coverage: None,
            })
            .unwrap();
        assert_eq!(refused["landed"], false);
        assert!(refused["createdUid"].is_null());
        assert_eq!(refused["target"], Value::Null);
        let snap = vp.snapshot();
        assert_eq!(snap["views"].as_array().unwrap().len(), before);
        assert!(snap["views"]
            .as_array()
            .unwrap()
            .iter()
            .all(|view| view["snapshot"]["honesty"]["state"] != "real"));
        let job = snap["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["job"] == "job-refuse")
            .unwrap();
        assert_eq!(job["phase"], "refused");
        assert!(job["reason"].as_str().unwrap().contains("unset"));
    }

    #[test]
    fn t17_headless_actions_round_trip_through_viewport_get() {
        let mut vp = Viewport::release();
        vp.apply(Action::Navigate {
            slide: "spatial".into(),
        })
        .unwrap();
        vp.apply(Action::Focus {
            uid: Some("lib-misaki-kokoro".into()),
        })
        .unwrap();
        vp.apply(Action::Seek {
            uid: "lib-misaki-kokoro".into(),
            t: 12.5,
        })
        .unwrap();
        vp.apply(Action::Flip {
            view: "view:lib-misaki-kokoro".into(),
            face: "back".into(),
            section: Some("honesty".into()),
        })
        .unwrap();
        vp.apply(Action::Rename {
            uid: "lib-misaki-kokoro".into(),
            name: "Narrator A".into(),
        })
        .unwrap();
        let snap = vp.snapshot();
        assert_eq!(snap["ui"]["slide"], "slide:spatial");
        assert_eq!(snap["ui"]["focus"], "lib-misaki-kokoro");
        assert_eq!(snap["ui"]["clock"]["source"], "lib-misaki-kokoro");
        assert_eq!(snap["ui"]["clock"]["t"], 12.5);
        assert_eq!(
            snap["ui"]["flipped"]["view:lib-misaki-kokoro"]["face"],
            "back"
        );
        let view = snap["views"]
            .as_array()
            .unwrap()
            .iter()
            .find(|view| view["asset"] == "lib-misaki-kokoro")
            .unwrap();
        assert_eq!(view["snapshot"]["title"], "Narrator A");
        assert_eq!(view["snapshot"]["display_rev"], 2);
        assert!(snap["ui"]["clock"].get("playing").is_none());
    }

    #[test]
    fn t4_user_play_rebinds_focus_and_autoplay_does_not() {
        let mut vp = Viewport::release();
        vp.apply(Action::Focus {
            uid: Some("lib-misaki-kokoro".into()),
        })
        .unwrap();
        let played = vp
            .apply(Action::Play {
                uid: "lib-kokoro-onnx".into(),
                origin: Origin::User,
            })
            .unwrap();
        let events = played["events"].as_array().unwrap();
        assert_eq!(events[0]["op"], "focus");
        assert_eq!(events[0]["uid"], "lib-kokoro-onnx");
        assert_eq!(events[1]["op"], "play");
        assert_eq!(played["focus"], "lib-kokoro-onnx");
        assert_eq!(vp.snapshot()["ui"]["focus"], "lib-kokoro-onnx");
        assert_eq!(vp.snapshot()["ui"]["clock"]["source"], "lib-kokoro-onnx");
        let auto = vp
            .apply(Action::Play {
                uid: "lib-kokoro".into(),
                origin: Origin::Auto,
            })
            .unwrap();
        let auto_events = auto["events"].as_array().unwrap();
        assert_eq!(auto_events.len(), 1);
        assert_eq!(auto_events[0]["op"], "play");
        assert_eq!(auto_events[0]["origin"], "auto");
        let snap = vp.snapshot();
        assert_eq!(snap["ui"]["focus"], "lib-kokoro-onnx");
        assert_eq!(snap["ui"]["clock"]["source"], "lib-kokoro-onnx");
        assert_eq!(snap["ui"]["playing"], "lib-kokoro");
    }

    #[test]
    fn t18_user_play_toggles_compare_and_rejects_three() {
        let mut vp = Viewport::release();
        vp.apply(Action::Compare {
            uids: vec!["lib-kokoro-onnx".into(), "lib-misaki-kokoro".into()],
        })
        .unwrap();
        vp.apply(Action::Focus {
            uid: Some("lib-kokoro".into()),
        })
        .unwrap();
        vp.apply(Action::Play {
            uid: "lib-kokoro-onnx".into(),
            origin: Origin::User,
        })
        .unwrap();
        let side_a = vp.snapshot();
        assert_eq!(side_a["ui"]["focus"], "lib-kokoro-onnx");
        assert_eq!(side_a["ui"]["clock"]["source"], "lib-kokoro-onnx");
        vp.apply(Action::Play {
            uid: "lib-misaki-kokoro".into(),
            origin: Origin::User,
        })
        .unwrap();
        let side_b = vp.snapshot();
        assert_eq!(side_b["ui"]["focus"], "lib-misaki-kokoro");
        assert_eq!(side_b["ui"]["clock"]["source"], "lib-misaki-kokoro");
        let err = vp
            .apply(Action::Compare {
                uids: vec![
                    "lib-kokoro-onnx".into(),
                    "lib-misaki-kokoro".into(),
                    "lib-kokoro".into(),
                ],
            })
            .unwrap_err();
        assert_eq!(err.code, -32602);
        assert_eq!(vp.snapshot()["ui"]["compare"].as_array().unwrap().len(), 2);
        assert_eq!(vp.snapshot()["ui"]["focus"], "lib-misaki-kokoro");
    }

    #[test]
    fn compare_has_no_select_action() {
        // Selecting inside compare is not its own action. Clock source moves
        // through Seek and Play. The old variant's name is built here so this
        // file does not contain that identifier once the variant is gone.
        let marker = ["Compare", "Select"].concat();
        let src = include_str!("viewport.rs");
        assert!(!src.contains(&marker), "dead {marker} action must not stay");
        let mut vp = Viewport::release();
        vp.apply(Action::Compare {
            uids: vec!["lib-kokoro-onnx".into(), "lib-misaki-kokoro".into()],
        })
        .unwrap();
        assert_eq!(vp.snapshot()["ui"]["compare"].as_array().unwrap().len(), 2);
        assert!(vp.snapshot()["ui"]["clock"]["source"].is_null());
    }

    #[test]
    fn c5_canonical_slide_id_and_deprecated_bare_alias() {
        let mut vp = Viewport::release();
        let canonical = vp
            .apply(Action::Navigate {
                slide: "slide:spatial".into(),
            })
            .unwrap();
        assert_eq!(canonical["slide"], "slide:spatial");
        assert!(canonical.get("deprecatedAlias").is_none());
        let alias = vp
            .apply(Action::Navigate {
                slide: "library".into(),
            })
            .unwrap();
        assert_eq!(alias["slide"], "slide:library");
        assert_eq!(alias["deprecatedAlias"], true);
        assert!(alias["log"]
            .as_str()
            .unwrap()
            .contains("deprecated slide alias 'library'"));
        assert_eq!(vp.snapshot()["ui"]["slide"], "slide:library");
        assert!(vp
            .apply(Action::Navigate {
                slide: "slide:nope".into()
            })
            .is_err());
    }

    #[test]
    fn t19_coverage_uses_cube_bin_and_fixed_reason_order() {
        let ok = validate_coverage(&misaki()).unwrap();
        assert!(ok.partial.is_none(), "99.8% coverage is not partial");
        let pct = (ok.ratio * 1000.0).round() / 10.0;
        assert!((pct - 99.8).abs() < 0.05, "{pct}");

        let mut past = misaki();
        past.selector_end = 139.375 + 2.0 * 0.352;
        let err = validate_coverage(&past).unwrap_err();
        assert_eq!(err, "selector end > clip duration + one bin");

        let mut lie = misaki();
        lie.recorded_source_duration_s = 43.425;
        let err = validate_coverage(&lie).unwrap_err();
        assert_eq!(
            err,
            "recorded source duration differs from clip duration by more than one bin"
        );

        let mut all = misaki();
        all.selector_start = -0.1;
        all.clip_in_src = false;
        all.recorded_source_duration_s = 43.425;
        all.selector_end = 139.375 + 2.0 * 0.352;
        assert_eq!(validate_coverage(&all).unwrap_err(), "start < 0");
        all.selector_start = 0.0;
        assert_eq!(
            validate_coverage(&all).unwrap_err(),
            "selector end > clip duration + one bin"
        );
        all.selector_end = 139.04;
        assert_eq!(
            validate_coverage(&all).unwrap_err(),
            "clip is not in the cube src"
        );
        all.clip_in_src = true;
        assert_eq!(
            validate_coverage(&all).unwrap_err(),
            "recorded source duration differs from clip duration by more than one bin"
        );

        let mut other = misaki();
        other.sec_per_bin = 0.5;
        other.clip_duration_s = 10.0;
        other.recorded_source_duration_s = 10.0;
        other.selector_end = 10.4;
        assert!(validate_coverage(&other).is_ok());
        other.selector_end = 10.0 + 0.5 + 0.01;
        assert_eq!(
            validate_coverage(&other).unwrap_err(),
            "selector end > clip duration + one bin"
        );
    }

    #[test]
    fn card_export_is_adaptive_card_1_5_from_facts() {
        let vp = Viewport::release();
        let card = vp.export_card("lib-misaki-kokoro").unwrap();
        assert_eq!(card["type"], "AdaptiveCard");
        assert_eq!(card["version"], "1.5");
        assert!(card["fallbackText"].as_str().unwrap().contains("library"));
        let facts = Viewport::release().snapshot();
        let view = facts["views"]
            .as_array()
            .unwrap()
            .iter()
            .find(|view| view["asset"] == "lib-misaki-kokoro")
            .unwrap();
        assert_eq!(view["snapshot"]["sections"][0]["id"], "identity");
        assert_eq!(view["snapshot"]["sections"][1]["id"], "honesty");
    }

    #[test]
    fn play_does_not_store_a_playhead_frame() {
        let mut vp = Viewport::release();
        vp.apply(Action::Seek {
            uid: "lib-misaki-kokoro".into(),
            t: 4.0,
        })
        .unwrap();
        vp.apply(Action::Play {
            uid: "lib-misaki-kokoro".into(),
            origin: Origin::User,
        })
        .unwrap();
        let snap = vp.snapshot();
        assert_eq!(snap["ui"]["clock"]["t"], 4.0);
        assert_eq!(snap["ui"]["playing"], "lib-misaki-kokoro");
    }
}
