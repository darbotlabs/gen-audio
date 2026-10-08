//! Pure v0 -> v1 migration for the asset object model.
//!
//! v0 is the pre-envelope data: `manifest.json` clips (camelCase + snake_case
//! mix, nested cube blocks, absolute paths), catalog voice models, VoiceProfile
//! documents, viewport cards and spectrogram sidecars. The caller supplies the
//! media digests (sha256, bytes, WAV header facts) in `media`, so this module
//! never reads files and the same input always yields the same envelopes.
//!
//! Dropped on purpose (never identity, never copied): `absWav`, `wav`
//! (`artifacts/...`), cube `source_wav`/`png` absolute paths, `synth_wall_s`,
//! status notes. Titles live in `display`, floats in `body`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::asset::{build_envelope, ms_from_frames, parse_schema_major, round_half_up, AssetError, EnvelopeParts, SCHEMA_VERSION};

fn fail<T>(code: &'static str, detail: impl Into<String>) -> Result<T, AssetError> {
    Err(AssetError { code, detail: detail.into() })
}

/// Dispatch on the document's schema major: absent = v0, 1 = current, else rejected.
pub fn migrate_to_v1(bundle: &Value) -> Result<Value, AssetError> {
    match bundle.get("schema_version").and_then(Value::as_str) {
        None => migrate_v0_to_v1(bundle),
        Some(version) => match parse_schema_major(version)? {
            1 => Ok(bundle.clone()),
            major => fail("unknown_major", format!("schema major {major} is not known to this build")),
        },
    }
}

fn web_to_library_path(url: &str) -> Option<String> {
    url.strip_prefix("/library/").map(str::to_string)
}

/// Seconds (a float view) to identity milliseconds: round half up of the
/// IEEE-754 product `seconds * 1000` (docs/ASSET_OBJECT_MODEL.md, rounding).
fn ms(seconds: f64) -> u64 {
    round_half_up(seconds * 1000.0).unwrap_or(0)
}

fn media_ref(media: &Value, role: &str, path: &str, mime: &str) -> Option<Value> {
    let info = media.get(path)?;
    let sha = info.get("sha256")?.as_str()?;
    let bytes = info.get("bytes")?.as_u64()?;
    Some(json!({"role": role, "path": path, "sha256": sha, "bytes": bytes, "mime": mime}))
}

fn honesty(synthesized: bool, fixture: bool, claims: &[&str]) -> Value {
    json!({"synthesized_speech": synthesized, "fixture": fixture, "claims": claims})
}

fn obj(pairs: Vec<(&str, Value)>) -> Map<String, Value> {
    pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
}

/// Which waveform voice model made a clip, by manifest engineId.
fn clip_voice_model(engine: &str) -> Option<(&'static str, Option<&'static str>)> {
    match engine {
        "kokoro_onnx" => Some(("kokoro_onnx", None)),
        "kokoro_dayour" => Some(("kokoro_dayour", None)),
        // dayour/kokoro KPipeline.generate_from_tokens on misaki G2P phonemes.
        "misaki_kokoro" => Some(("kokoro_dayour", Some("misaki"))),
        // Rendered offline with microsoft/VibeVoice-1.5B; the app has no adapter.
        "vibevoice" => Some(("vibevoice", None)),
        _ => None,
    }
}

pub fn migrate_v0_to_v1(bundle: &Value) -> Result<Value, AssetError> {
    let media = bundle.get("media").cloned().unwrap_or_else(|| json!({}));
    let clips = bundle.pointer("/manifest/clips").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut assets: Vec<Value> = Vec::new();
    let mut index: BTreeMap<String, String> = BTreeMap::new();
    let mut remember = |assets: &mut Vec<Value>, envelope: Value| {
        let key = format!(
            "{}:{}",
            envelope["kind"].as_str().unwrap_or_default(),
            envelope["legacy_id"].as_str().unwrap_or_default()
        );
        let uid = envelope["uid"].as_str().unwrap_or_default().to_string();
        index.insert(key, uid.clone());
        assets.push(envelope);
        uid
    };

    // Voice models, with unavailable manifest "clips" folded into availability.
    let mut model_uid: BTreeMap<String, String> = BTreeMap::new();
    for model in bundle.get("voice_models").and_then(Value::as_array).cloned().unwrap_or_default() {
        let id = model["id"].as_str().unwrap_or_default().to_string();
        let waveform = model["waveform"].as_bool().unwrap_or(false);
        let unavailable = model["unavailable"].as_bool().unwrap_or(false);
        let mut body = obj(vec![
            ("label", model["label"].clone()),
            ("waveform", json!(waveform)),
            ("synth_adapter", json!(model["synthAdapter"].as_bool().unwrap_or(false))),
            ("unavailable", json!(unavailable)),
            ("note", model["note"].clone()),
        ]);
        if let Some(clip) = clips.iter().find(|clip| clip["engineId"] == id.as_str() && clip["status"] != "ok") {
            body.insert(
                "availability".into(),
                json!({"status": clip["status"], "reason": clip["reason"], "legacy_clip_id": clip["id"]}),
            );
        }
        let mut claims = vec![];
        if unavailable {
            claims.push("engine_unavailable");
        }
        if !waveform {
            claims.push("g2p_only");
        }
        let envelope = build_envelope(EnvelopeParts {
            kind: "voice_model",
            legacy_id: Some(id.clone()),
            title: model["label"].as_str().unwrap_or(&id).to_string(),
            summary: None,
            status: if unavailable { "unavailable" } else { "ok" },
            fields: json!({"model_id": id, "waveform": waveform}),
            media: vec![],
            src: vec![],
            relations: Map::new(),
            honesty: honesty(false, false, &claims),
            provenance: obj(vec![("generator", json!("catalog voice model (v0)"))]),
            body: Value::Object(body),
        })?;
        let uid = remember(&mut assets, envelope);
        model_uid.insert(id, uid);
    }

    // Audio clips, cubes and layers.
    let mut clip_uid: BTreeMap<String, String> = BTreeMap::new();
    let mut clip_wav_sha: BTreeMap<String, String> = BTreeMap::new();
    let mut cube_uid_by_json: BTreeMap<String, String> = BTreeMap::new();
    for clip in &clips {
        if clip["status"] != "ok" {
            continue;
        }
        let id = clip["id"].as_str().unwrap_or_default().to_string();
        let engine = clip["engineId"].as_str().unwrap_or_default().to_string();
        let Some(wav_path) = clip["wavUrl"].as_str().and_then(web_to_library_path) else {
            return fail("clip_missing_wav", format!("v0 clip {id} has no /library/ wavUrl"));
        };
        let wav = media_ref(&media, "wav", &wav_path, "audio/wav");
        let info = media.get(&wav_path).cloned().unwrap_or(Value::Null);
        let sample_rate = info["sample_rate"].as_u64().or_else(|| clip["sample_rate"].as_u64()).unwrap_or(0);
        let duration_ms = match (info["frames"].as_u64(), sample_rate) {
            (Some(frames), rate) if rate > 0 => ms_from_frames(frames, rate),
            _ => ms(clip["duration_s"].as_f64().unwrap_or(0.0)),
        };
        let mut fields = json!({"engine": engine, "sample_rate_hz": sample_rate, "duration_ms": duration_ms});
        if let Some(channels) = info["channels"].as_u64() {
            fields["channels"] = json!(channels);
        }
        let synthesized = clip["synthesizedSpeech"].as_bool().unwrap_or(false);
        let (model_id, g2p) = clip_voice_model(&engine).unwrap_or(("", None));
        let mut provenance = obj(vec![("generator", json!("manifest.json clip (v0)")), ("engine", json!(engine))]);
        if let Some(uid) = model_uid.get(model_id) {
            provenance.insert("voice_model".into(), json!(uid));
        }
        if let Some(uid) = g2p.and_then(|name| model_uid.get(name)) {
            provenance.insert("g2p_model".into(), json!(uid));
        }
        // A v0 clip may carry its synth provenance (engine label, generator,
        // created_at, params). It is unhashed, so it never changes the uid.
        if let Some(extra) = clip.get("provenance").and_then(Value::as_object) {
            for key in ["engine", "generator", "created_at", "params"] {
                if let Some(value) = extra.get(key) {
                    provenance.insert(key.into(), value.clone());
                }
            }
        }
        let mut body = obj(vec![
            ("duration_s", clip["duration_s"].clone()),
            ("sample_rate", clip["sample_rate"].clone()),
            ("wav_url", json!(format!("/library/{wav_path}"))),
        ]);
        for key in ["n_words", "n_turns"] {
            if !clip[key].is_null() {
                body.insert(key.into(), clip[key].clone());
            }
        }
        let status = if wav.is_some() { "ok" } else { "missing" };
        let Some(wav) = wav else {
            return fail("clip_missing_wav", format!("v0 clip {id}: no sha256 for {wav_path}; hash the file before migrating"));
        };
        let sha = wav["sha256"].as_str().unwrap_or_default().to_string();
        let envelope = build_envelope(EnvelopeParts {
            kind: "audio_clip",
            legacy_id: Some(id.clone()),
            title: clip["title"].as_str().unwrap_or(&id).to_string(),
            summary: clip["summary"].as_str().map(str::to_string),
            status,
            fields,
            media: vec![wav],
            src: vec![],
            relations: Map::new(),
            honesty: honesty(synthesized, false, if synthesized { &["real_wav", "synthesized_speech"] } else { &["real_wav"] }),
            provenance,
            body: Value::Object(body),
        })?;
        let uid = remember(&mut assets, envelope);
        clip_uid.insert(id.clone(), uid.clone());
        clip_wav_sha.insert(id.clone(), sha.clone());

        let Some(cube) = clip.get("cube").filter(|cube| cube.is_object()) else { continue };
        let json_path = cube["jsonUrl"].as_str().and_then(web_to_library_path).unwrap_or_default();
        let png_path = cube["pngUrl"].as_str().and_then(web_to_library_path).unwrap_or_default();
        let Some(cube_doc) = bundle.pointer("/cube_docs").and_then(|docs| docs.get(&json_path)) else {
            return fail("cube_doc_missing", format!("v0 clip {id}: cube JSON {json_path} not supplied"));
        };
        let cube_sr = cube_doc["sample_rate"].as_u64().unwrap_or(sample_rate);
        let cube_duration_s = cube_doc["duration_s"].as_f64().or_else(|| cube["duration_s"].as_f64()).unwrap_or(0.0);
        let shape = cube_doc["cube_shape_f_t"]
            .as_array()
            .cloned()
            .or_else(|| cube_doc.pointer("/layers/signal/shape").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        let freq_bins = shape.first().and_then(Value::as_u64).unwrap_or(0);
        let time_bins = shape.get(1).and_then(Value::as_u64).unwrap_or(0).max(1);
        let hop = cube_doc["hop"].as_u64();
        let (bin_frames, inferred) = match (cube_doc["downsample_sf_st"].get(1).and_then(Value::as_u64), hop) {
            (Some(step), Some(hop)) => (step * hop, false),
            _ => (round_half_up(cube_duration_s * cube_sr as f64 / time_bins as f64).unwrap_or(0), true),
        };
        let covers_ms = cube_doc["cube_covers_s"]
            .as_f64()
            .map(ms)
            .unwrap_or_else(|| ms_from_frames(time_bins * bin_frames, cube_sr));
        let n_points = cube_doc["n_points"]
            .as_u64()
            .unwrap_or_else(|| cube_doc["points_preview"].as_array().map(|items| items.len() as u64).unwrap_or(0));
        let inv_hdr = cube_doc["inv_hdr"].as_f64().or_else(|| cube["inv_hdr"].as_f64()).unwrap_or(0.0);
        let mut fields = json!({
            "source_sha256": sha,
            "sample_rate_hz": cube_sr,
            "bin_frames": bin_frames,
            "time_bins": time_bins,
            "freq_bins": freq_bins,
            "duration_ms": ms(cube_duration_s),
            "covers_ms": covers_ms,
            "inv_hdr_ppm": round_half_up(inv_hdr * 1_000_000.0).unwrap_or(0),
            "cube_revision": cube_doc["cube_revision"].as_u64().or_else(|| cube["cube_revision"].as_u64()).unwrap_or(1),
            "n_points": n_points,
        });
        if let Some(n_fft) = cube_doc["n_fft"].as_u64() {
            fields["n_fft"] = json!(n_fft);
        }
        if let Some(hop) = hop {
            fields["hop_frames"] = json!(hop);
        }
        let mut cube_media = Vec::new();
        cube_media.extend(media_ref(&media, "cube_json", &json_path, "application/json"));
        cube_media.extend(media_ref(&media, "cube_png", &png_path, "image/png"));
        let cube_envelope = build_envelope(EnvelopeParts {
            kind: "cube_ihdr",
            legacy_id: Some(format!("{id}.cube")),
            title: cube_doc["title"].as_str().map(str::to_string).unwrap_or_else(|| format!("Inverse-HDR cube — {id}")),
            summary: cube["note"].as_str().map(str::to_string),
            status: "ok",
            fields,
            media: cube_media,
            src: vec![uid.clone()],
            relations: Map::new(),
            honesty: honesty(false, false, &["library_cube"]),
            provenance: obj(vec![
                ("generator", json!("scripts/cube_spectrogram_3d.py (inverse-HDR bitdot cube)")),
                ("params", json!({"bins_inferred_from_shape": inferred})),
            ]),
            body: json!({
                "inv_hdr": inv_hdr,
                "duration_s": cube_duration_s,
                "bin_seconds": bin_frames as f64 / cube_sr.max(1) as f64,
                "cube_covers_s": covers_ms as f64 / 1000.0,
                "cube_shape_f_t": [freq_bins, time_bins],
                "json_url": format!("/library/{json_path}"),
                "png_url": format!("/library/{png_path}"),
            }),
        })?;
        let cube_uid = remember(&mut assets, cube_envelope);
        cube_uid_by_json.insert(format!("/library/{json_path}"), cube_uid.clone());
        for (layer_index, name) in crate::asset::LAYER_NAMES.iter().enumerate() {
            let Some(stats) = cube_doc.pointer(&format!("/layers/{name}")) else { continue };
            let mut relations = Map::new();
            relations.insert("layer_of".into(), json!(cube_uid));
            let layer = build_envelope(EnvelopeParts {
                kind: "layer",
                legacy_id: Some(format!("{id}.cube.{name}")),
                title: format!("{name} layer"),
                summary: None,
                status: "ok",
                fields: json!({"name": name, "index": layer_index}),
                media: vec![],
                src: vec![cube_uid.clone()],
                relations,
                honesty: honesty(false, false, &["library_cube"]),
                provenance: obj(vec![("generator", json!("cube JSON layers block (v0)"))]),
                body: stats.clone(),
            })?;
            remember(&mut assets, layer);
        }
    }

    // 2D spectrogram strips (sidecars written by scripts/spectrogram_strip.py).
    for sidecar in bundle.get("spectrograms").and_then(Value::as_array).cloned().unwrap_or_default() {
        let clip_id = sidecar["clip_id"].as_str().unwrap_or_default();
        let Some(parent) = clip_uid.get(clip_id) else {
            return fail("derived_source_missing", format!("spectrogram for unknown clip {clip_id}"));
        };
        let path = sidecar["path"].as_str().unwrap_or_default();
        let Some(png) = media_ref(&media, "spectrogram_png", path, "image/png") else {
            return fail("bad_media", format!("spectrogram {path} has no sha256"));
        };
        let mut fields = sidecar["fields"].clone();
        fields["source_sha256"] = json!(clip_wav_sha.get(clip_id).cloned().unwrap_or_default());
        let envelope = build_envelope(EnvelopeParts {
            kind: "spectrogram_2d",
            legacy_id: Some(format!("{clip_id}.spec2d")),
            title: format!("2D spectrogram — {clip_id}"),
            summary: None,
            status: "ok",
            fields,
            media: vec![png],
            src: vec![parent.clone()],
            relations: Map::new(),
            honesty: honesty(false, false, &["library_spectrogram"]),
            provenance: obj(vec![
                ("generator", json!("scripts/spectrogram_strip.py")),
                ("params", sidecar["params"].clone()),
            ]),
            body: json!({"png_url": format!("/library/{path}"), "seconds_per_px": sidecar["params"]["seconds_per_px"]}),
        })?;
        remember(&mut assets, envelope);
    }

    // Voice profiles (agent personas).
    let mut profile_uid: BTreeMap<String, String> = BTreeMap::new();
    for entry in bundle.get("profiles").and_then(Value::as_array).cloned().unwrap_or_default() {
        let profile = &entry["profile"];
        let persona = profile["personaId"].as_str().unwrap_or_default().to_string();
        let model = profile["voiceModel"].as_str().unwrap_or_default();
        let Some(voice_uid) = model_uid.get(model) else {
            return fail("bad_voice_ref", format!("profile {persona}: voiceModel {model:?} is not a catalog voice model"));
        };
        let mut relations = Map::new();
        if let Some(cube) = profile["cubeJsonUrl"].as_str().and_then(|url| cube_uid_by_json.get(url)) {
            relations.insert("bound_to".into(), json!([cube]));
        }
        let mut profile_media = Vec::new();
        if let Some(path) = entry["path"].as_str() {
            profile_media.extend(media_ref(&media, "profile_json", path, "application/json"));
        }
        let envelope = build_envelope(EnvelopeParts {
            kind: "voice_profile",
            legacy_id: Some(persona.clone()),
            title: profile["agentName"].as_str().unwrap_or(&persona).to_string(),
            summary: None,
            status: "ok",
            fields: json!({
                "persona_id": persona,
                "voice_model": voice_uid,
                "tone": profile["tone"],
                "purpose": profile["purpose"],
                "domain": profile["domain"],
                "accent": profile["accent"],
                "traits": profile["traits"],
                "refs": profile["refs"],
            }),
            media: profile_media,
            src: vec![],
            relations,
            honesty: json!({"synthesized_speech": false, "fixture": false, "not_podcast": true, "claims": ["profile_preview", "not_a_podcast_render"]}),
            provenance: obj(vec![("generator", json!("VoiceProfile (v0)"))]),
            body: profile.clone(),
        })?;
        let uid = remember(&mut assets, envelope);
        profile_uid.insert(persona, uid);
    }

    // Viewport cards: kind card, the old card kind becomes fields.view.
    for card in bundle.get("cards").and_then(Value::as_array).cloned().unwrap_or_default() {
        let id = card["id"].as_str().unwrap_or_default().to_string();
        let view = card["kind"].as_str().unwrap_or_default().to_string();
        let body_in = &card["body"];
        let bound = match view.as_str() {
            "LibraryClip" => clip_uid.get(&id).cloned(),
            "VoiceProfile" => body_in["personaId"].as_str().and_then(|persona| profile_uid.get(persona)).cloned(),
            "EngineStatus" => body_in["engineId"].as_str().and_then(|engine| model_uid.get(engine)).cloned(),
            _ => None,
        };
        let mut relations = Map::new();
        if let Some(uid) = bound {
            relations.insert("bound_to".into(), json!([uid]));
        }
        let fixture = body_in["source"] == "fixture-tone";
        let claims: &[&str] = match view.as_str() {
            _ if fixture => &["fixture_tone"],
            "EngineStatus" | "ServeHealth" | "ConnectorStatus" => &["status_only"],
            "BenchmarkCompare" => &["reference_only"],
            "VoiceProfile" => &["profile_preview", "not_a_podcast_render"],
            "LibraryClip" if body_in["status"] == "ok" => &["real_wav"],
            "LibraryClip" => &["engine_unavailable"],
            _ => &[],
        };
        let mut card_body = card.clone();
        if let Some(map) = card_body.as_object_mut() {
            map.remove("uid");
        }
        let envelope = build_envelope(EnvelopeParts {
            kind: "card",
            legacy_id: Some(id.clone()),
            title: card["title"].as_str().unwrap_or(&id).to_string(),
            summary: None,
            status: "ok",
            fields: json!({"card_id": id, "view": view}),
            media: vec![],
            src: vec![],
            relations,
            honesty: honesty(false, fixture, claims),
            provenance: obj(vec![("generator", json!("viewport.example.json card (v0)"))]),
            body: card_body,
        })?;
        remember(&mut assets, envelope);
    }

    assets.sort_by(|a, b| a["uid"].as_str().cmp(&b["uid"].as_str()));
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "uid_scheme": crate::asset::UID_SCHEME,
        "assets": assets,
        "legacy_index": index,
    }))
}
