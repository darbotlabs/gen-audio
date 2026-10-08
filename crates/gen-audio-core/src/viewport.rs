//! Canonical viewport reducer.
//!
//! The desktop, MCP, and ACP all read this store. TypeScript renders it.
//! The clock keeps `source` plus the last committed Seek. A user Play emits
//! focus, then play, and rebinds the cube. Autoplay, scroll, resync, and
//! hover do not. Playback frames never enter the reducer.
//!
//! Coverage uses the cube's own `sec_per_bin`. Reject reasons stay in one order:
//! start < 0, clip missing from `src`, recorded source duration lie, selector end.

use std::collections::BTreeMap;
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
        Self { code: -32602, message: message.into() }
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
    Navigate { slide: String },
    Focus { uid: Option<String> },
    Compare { uids: Vec<String> },
    /// Switches `clock.source` inside `compare` and does not change focus.
    CompareSelect { uid: String },
    Seek { uid: String, t: f64 },
    Play { uid: String, origin: Origin },
    Pause { uid: String },
    Flip { view: String, face: String, section: Option<String> },
    Rename { uid: String, name: String },
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
    },
    Superseded { old: String, next: String },
    Rebind { view: String, next: String },
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

#[derive(Clone, Debug)]
struct FlipState {
    face: String,
    section: Option<String>,
}

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
    flipped: BTreeMap<String, FlipState>,
    names: BTreeMap<String, String>,
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
            flipped: BTreeMap::new(),
            names: BTreeMap::new(),
        };
        for clip in catalog::library_clips() {
            let honesty = if clip.synthesized_speech {
                "library"
            } else if clip.wav_url.is_none() {
                "unavailable"
            } else {
                "library"
            };
            vp.assets.insert(
                clip.id.to_string(),
                Asset {
                    kind: "audio_clip".into(),
                    title: clip.title.to_string(),
                    honesty: honesty.into(),
                    duration_s: None,
                    media: if clip.wav_url.is_some() { "present" } else { "wav_missing" }.into(),
                    display_rev: 1,
                },
            );
            let view_id = format!("view:{}", clip.id);
            vp.views.push(View {
                id: view_id.clone(),
                asset: clip.id.to_string(),
                home: "slide:library".into(),
                reference: false,
            });
            if let Some(members) = vp.slides.iter_mut().find(|item| item.id == "slide:library") {
                members.members.push(view_id);
            }
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
            Action::CompareSelect { uid } => {
                if !self.compare.iter().any(|item| item == &uid) {
                    return Err(ReduceError::invalid("compare select is outside compare"));
                }
                self.clock_source = Some(uid.clone());
                Ok(json!({"op": "compare_select", "clock": {"source": uid}, "focus": self.focus}))
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
                Ok(json!({"op": "pause", "playing": self.playing, "clock": {"source": self.clock_source}}))
            }
            Action::Flip { view, face, section } => {
                if face != "front" && face != "back" {
                    return Err(ReduceError::invalid("face must be front or back"));
                }
                if !self.views.iter().any(|item| item.id == view) {
                    return Err(ReduceError::invalid(format!("unknown view {view}")));
                }
                self.flipped.insert(view.clone(), FlipState { face: face.clone(), section: section.clone() });
                Ok(json!({"op": "flip", "view": view, "face": face, "section": section}))
            }
            Action::Rename { uid, name } => {
                let asset = self.assets.get_mut(&uid).ok_or_else(|| ReduceError::invalid(format!("unknown asset {uid}")))?;
                let len = name.chars().count();
                if !(1..=80).contains(&len) {
                    return Err(ReduceError::invalid("name length is out of range"));
                }
                asset.display_rev = asset.display_rev.saturating_add(1);
                asset.title = name.clone();
                self.names.insert(uid.clone(), name.clone());
                Ok(json!({"op": "rename", "uid": uid, "name": name, "display_rev": asset.display_rev}))
            }
            Action::Generate { prompt_ref, personas, voice, duration_s, focus, job } => {
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
            Action::Job { job, target, phase, reason, kind, synthesized } => {
                self.apply_job(job, target, phase, reason, kind, synthesized)
            }
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
        let focus = self.jobs.iter().find(|item| item.id == job).map(|item| item.focus).unwrap_or(true);
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
        } else if kind == "spectrogram" || kind == "cube" {
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

    fn land_clip(&mut self, uid: &str, focus: bool) -> bool {
        if self.assets.contains_key(uid) && self.views.iter().any(|view| view.asset == uid && !view.reference) {
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
        self.ensure_derived(&spec, "spectrogram", "slide:studio", "pending");
        self.ensure_derived(&cube, "cube", "slide:spatial", "pending");
        append_member(&mut self.slides, "slide:pipeline", &format!("view:{cube}"));
        if let Some(pipeline) = self.slides.iter_mut().find(|slide| slide.id == "slide:pipeline") {
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
        self.assets.entry(asset.to_string()).or_insert_with(|| Asset {
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
        let asset = self.assets.get_mut(uid).ok_or_else(|| ReduceError::invalid(format!("unknown derived asset {uid}")))?;
        if asset.kind != kind {
            return Err(ReduceError::invalid("derived job kind does not match the asset"));
        }
        asset.honesty = "real".into();
        asset.media = "present".into();
        Ok(())
    }

    fn honesty_of(&self, uid: &str) -> String {
        self.assets.get(uid).map(|asset| asset.honesty.clone()).unwrap_or_else(|| "unresolved".into())
    }

    fn require_asset(&self, uid: &str) -> Result<(), ReduceError> {
        if self.assets.contains_key(uid) {
            Ok(())
        } else {
            Err(ReduceError::invalid(format!("unknown asset {uid}")))
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
                "flipped": self.flipped.iter().map(|(id, state)| (id.clone(), json!({"face": state.face, "section": state.section}))).collect::<serde_json::Map<String, Value>>()
            },
            "pipelineEmpty": self.slides.iter().find(|slide| slide.id == "slide:pipeline").and_then(|slide| slide.empty.clone())
        })
    }

    pub fn export_card(&self, uid: &str) -> Result<Value, ReduceError> {
        let asset = self.assets.get(uid).ok_or_else(|| ReduceError::invalid(format!("unknown asset {uid}")))?;
        Ok(adaptive_card(&asset.title, &asset.kind, &asset.honesty, &asset.media, asset.display_rev))
    }
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
        Ok(ResolvedSlide { canonical, deprecated_alias: true, log: Some(log) })
    } else {
        Ok(ResolvedSlide { canonical, deprecated_alias: false, log: None })
    }
}

pub fn canonical_slide(slide: &str) -> Result<String, ReduceError> {
    Ok(resolve_slide(slide)?.canonical)
}

pub fn facts_snapshot(uid: &str, kind: &str, title: &str, honesty: &str, media: &str, display_rev: u64) -> Value {
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

pub fn adaptive_card(title: &str, kind: &str, honesty: &str, media: &str, display_rev: u64) -> Value {
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
/// 2. clip is not in the cube src
/// 3. recorded source duration differs from the clip by more than one bin
/// 4. selector end > clip duration + one bin
pub fn validate_coverage(input: &CoverageInput) -> Result<CoverageOk, String> {
    if input.sec_per_bin <= 0.0 || !input.sec_per_bin.is_finite() {
        return Err("cube sec_per_bin must be positive".into());
    }
    if input.selector_start < 0.0 {
        return Err("start < 0".into());
    }
    if !input.clip_in_src {
        return Err("clip is not in the cube src".into());
    }
    let duration_delta = (input.recorded_source_duration_s - input.clip_duration_s).abs();
    if duration_delta > input.sec_per_bin + 1e-9 {
        return Err("recorded source duration differs from clip duration by more than one bin".into());
    }
    let limit = input.clip_duration_s + input.sec_per_bin;
    if input.selector_end > limit + 1e-9 {
        return Err("selector end > clip duration + one bin".into());
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
    Ok(CoverageOk { covered_s: covered, of_s, ratio, partial })
}

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

fn state() -> &'static Mutex<Viewport> {
    static STATE: OnceLock<Mutex<Viewport>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Viewport::release()))
}

pub fn apply_global(action: Action) -> Result<Value, ReduceError> {
    state().lock().expect("viewport").apply(action)
}

pub fn snapshot_global() -> Value {
    state().lock().expect("viewport").snapshot()
}

pub fn export_global(uid: &str) -> Result<Value, ReduceError> {
    state().lock().expect("viewport").export_card(uid)
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
        .any(|view| honesty_state(&view["snapshot"]["honesty"]) == "fixture" || honesty_state(&view["honesty"]) == "fixture")
}

fn honesty_state(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    value.get("state").and_then(Value::as_str).unwrap_or("").to_string()
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

    #[test]
    fn t6_release_doc_has_no_fixture_and_pipeline_is_empty() {
        let doc = Viewport::release().snapshot();
        assert!(!release_has_fixture(&doc), "{doc}");
        let pipeline = doc["slides"].as_array().unwrap().iter().find(|slide| slide["id"] == "slide:pipeline").unwrap();
        assert_eq!(pipeline["members"].as_array().unwrap().len(), 0);
        assert_eq!(pipeline["empty"], "no pipeline output yet");
        let states: Vec<_> = doc["views"]
            .as_array()
            .unwrap()
            .iter()
            .map(|view| view["snapshot"]["honesty"]["state"].as_str().unwrap().to_string())
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
            })
            .unwrap();
        assert_eq!(done["landed"], true);
        assert_eq!(done["spectrogram"]["honesty"], "pending");
        assert_eq!(done["cube"]["honesty"], "pending");
        let snap = vp.snapshot();
        let library = snap["slides"].as_array().unwrap().iter().find(|slide| slide["id"] == "slide:library").unwrap();
        assert!(library["members"].as_array().unwrap().iter().any(|id| id == "view:clip-brief"));
        assert!(snap["views"].as_array().unwrap().iter().any(|view| view["asset"] == "clip-brief:spectrogram"));
        assert!(snap["views"].as_array().unwrap().iter().any(|view| view["asset"] == "clip-brief:cube"));
        assert!(snap["annotations"].as_array().unwrap().iter().any(|ann| ann["target"]["source"] == "clip-brief"));
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
        })
        .unwrap();
        vp.apply(Action::Job {
            job: "job-cube".into(),
            target: Some("clip-brief:cube".into()),
            phase: "done".into(),
            reason: None,
            kind: Some("cube".into()),
            synthesized: true,
        })
        .unwrap();
        let after = vp.snapshot();
        let spec = after["views"].as_array().unwrap().iter().find(|view| view["asset"] == "clip-brief:spectrogram").unwrap();
        let cube = after["views"].as_array().unwrap().iter().find(|view| view["asset"] == "clip-brief:cube").unwrap();
        assert_eq!(spec["snapshot"]["honesty"]["state"], "real");
        assert_eq!(cube["snapshot"]["honesty"]["state"], "real");
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
            })
            .unwrap();
        assert_eq!(refused["landed"], false);
        assert!(refused["createdUid"].is_null());
        assert_eq!(refused["target"], Value::Null);
        let snap = vp.snapshot();
        assert_eq!(snap["views"].as_array().unwrap().len(), before);
        assert!(snap["views"].as_array().unwrap().iter().all(|view| view["snapshot"]["honesty"]["state"] != "real"));
        let job = snap["jobs"].as_array().unwrap().iter().find(|job| job["job"] == "job-refuse").unwrap();
        assert_eq!(job["phase"], "refused");
        assert!(job["reason"].as_str().unwrap().contains("unset"));
    }

    #[test]
    fn t17_headless_actions_round_trip_through_viewport_get() {
        let mut vp = Viewport::release();
        vp.apply(Action::Navigate { slide: "spatial".into() }).unwrap();
        vp.apply(Action::Focus { uid: Some("lib-misaki-kokoro".into()) }).unwrap();
        vp.apply(Action::Seek { uid: "lib-misaki-kokoro".into(), t: 12.5 }).unwrap();
        vp.apply(Action::Flip {
            view: "view:lib-misaki-kokoro".into(),
            face: "back".into(),
            section: Some("honesty".into()),
        })
        .unwrap();
        vp.apply(Action::Rename { uid: "lib-misaki-kokoro".into(), name: "Narrator A".into() }).unwrap();
        let snap = vp.snapshot();
        assert_eq!(snap["ui"]["slide"], "slide:spatial");
        assert_eq!(snap["ui"]["focus"], "lib-misaki-kokoro");
        assert_eq!(snap["ui"]["clock"]["source"], "lib-misaki-kokoro");
        assert_eq!(snap["ui"]["clock"]["t"], 12.5);
        assert_eq!(snap["ui"]["flipped"]["view:lib-misaki-kokoro"]["face"], "back");
        let view = snap["views"].as_array().unwrap().iter().find(|view| view["asset"] == "lib-misaki-kokoro").unwrap();
        assert_eq!(view["snapshot"]["title"], "Narrator A");
        assert_eq!(view["snapshot"]["display_rev"], 2);
        assert!(snap["ui"]["clock"].get("playing").is_none());
    }

    #[test]
    fn t4_user_play_rebinds_focus_and_autoplay_does_not() {
        let mut vp = Viewport::release();
        vp.apply(Action::Focus { uid: Some("lib-misaki-kokoro".into()) }).unwrap();
        let played = vp
            .apply(Action::Play { uid: "lib-kokoro-onnx".into(), origin: Origin::User })
            .unwrap();
        let events = played["events"].as_array().unwrap();
        assert_eq!(events[0]["op"], "focus");
        assert_eq!(events[0]["uid"], "lib-kokoro-onnx");
        assert_eq!(events[1]["op"], "play");
        assert_eq!(played["focus"], "lib-kokoro-onnx");
        assert_eq!(vp.snapshot()["ui"]["focus"], "lib-kokoro-onnx");
        assert_eq!(vp.snapshot()["ui"]["clock"]["source"], "lib-kokoro-onnx");
        let auto = vp
            .apply(Action::Play { uid: "lib-kokoro".into(), origin: Origin::Auto })
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
        vp.apply(Action::Compare { uids: vec!["lib-kokoro-onnx".into(), "lib-misaki-kokoro".into()] }).unwrap();
        vp.apply(Action::Focus { uid: Some("lib-kokoro".into()) }).unwrap();
        vp.apply(Action::Play { uid: "lib-kokoro-onnx".into(), origin: Origin::User }).unwrap();
        let side_a = vp.snapshot();
        assert_eq!(side_a["ui"]["focus"], "lib-kokoro-onnx");
        assert_eq!(side_a["ui"]["clock"]["source"], "lib-kokoro-onnx");
        vp.apply(Action::Play { uid: "lib-misaki-kokoro".into(), origin: Origin::User }).unwrap();
        let side_b = vp.snapshot();
        assert_eq!(side_b["ui"]["focus"], "lib-misaki-kokoro");
        assert_eq!(side_b["ui"]["clock"]["source"], "lib-misaki-kokoro");
        let err = vp
            .apply(Action::Compare {
                uids: vec!["lib-kokoro-onnx".into(), "lib-misaki-kokoro".into(), "lib-kokoro".into()],
            })
            .unwrap_err();
        assert_eq!(err.code, -32602);
        assert_eq!(vp.snapshot()["ui"]["compare"].as_array().unwrap().len(), 2);
        assert_eq!(vp.snapshot()["ui"]["focus"], "lib-misaki-kokoro");
    }

    #[test]
    fn c5_canonical_slide_id_and_deprecated_bare_alias() {
        let mut vp = Viewport::release();
        let canonical = vp.apply(Action::Navigate { slide: "slide:spatial".into() }).unwrap();
        assert_eq!(canonical["slide"], "slide:spatial");
        assert!(canonical.get("deprecatedAlias").is_none());
        let alias = vp.apply(Action::Navigate { slide: "library".into() }).unwrap();
        assert_eq!(alias["slide"], "slide:library");
        assert_eq!(alias["deprecatedAlias"], true);
        assert!(alias["log"].as_str().unwrap().contains("deprecated slide alias 'library'"));
        assert_eq!(vp.snapshot()["ui"]["slide"], "slide:library");
        assert!(vp.apply(Action::Navigate { slide: "slide:nope".into() }).is_err());
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
        assert_eq!(err, "recorded source duration differs from clip duration by more than one bin");

        let mut all = misaki();
        all.selector_start = -0.1;
        all.clip_in_src = false;
        all.recorded_source_duration_s = 43.425;
        all.selector_end = 139.375 + 2.0 * 0.352;
        assert_eq!(validate_coverage(&all).unwrap_err(), "start < 0");
        all.selector_start = 0.0;
        assert_eq!(validate_coverage(&all).unwrap_err(), "clip is not in the cube src");
        all.clip_in_src = true;
        assert_eq!(
            validate_coverage(&all).unwrap_err(),
            "recorded source duration differs from clip duration by more than one bin"
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
        let view = facts["views"].as_array().unwrap().iter().find(|view| view["asset"] == "lib-misaki-kokoro").unwrap();
        assert_eq!(view["snapshot"]["sections"][0]["id"], "identity");
        assert_eq!(view["snapshot"]["sections"][1]["id"], "honesty");
    }

    #[test]
    fn play_does_not_store_a_playhead_frame() {
        let mut vp = Viewport::release();
        vp.apply(Action::Seek { uid: "lib-misaki-kokoro".into(), t: 4.0 }).unwrap();
        vp.apply(Action::Play { uid: "lib-misaki-kokoro".into(), origin: Origin::User }).unwrap();
        let snap = vp.snapshot();
        assert_eq!(snap["ui"]["clock"]["t"], 4.0);
        assert_eq!(snap["ui"]["playing"], "lib-misaki-kokoro");
    }
}
