//! User library for clips this process generated.
//!
//! Files and v1 envelopes live under the user library root, never only in the
//! temp work directory. The root is outside the repository. Writes are
//! confined to that root and the catalog file is replaced atomically.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::asset::{self, glyph_from_uid, sha256_hex, MediaDigest};
use crate::paths::find_repo_root;

thread_local! {
    static ROOT_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Tests pin the library root to a temp directory. Other threads are unaffected.
pub fn set_root_override_for_test(path: Option<PathBuf>) {
    ROOT_OVERRIDE.with(|slot| *slot.borrow_mut() = path);
}

pub fn user_library_dir() -> Result<PathBuf, String> {
    let overridden = ROOT_OVERRIDE.with(|slot| slot.borrow().clone());
    let dir = if let Some(path) = overridden {
        path
    } else if let Ok(raw) = std::env::var("GEN_AUDIO_LIBRARY") {
        if raw.trim().is_empty() {
            return Err("GEN_AUDIO_LIBRARY is empty".into());
        }
        PathBuf::from(raw)
    } else {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
            .unwrap_or_else(std::env::temp_dir);
        base.join("gen-audio").join("library")
    };
    fs::create_dir_all(&dir).map_err(|err| format!("library directory: {err}"))?;
    let dir = dir
        .canonicalize()
        .map_err(|err| format!("library directory: {err}"))?;
    if let Some(repo) = find_repo_root() {
        if dir.starts_with(&repo) || repo.starts_with(&dir) {
            return Err("library directory must not contain or sit inside the repository".into());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .map_err(|err| err.to_string())?;
    }
    Ok(dir)
}

pub fn runtime_assets() -> Vec<Value> {
    let Ok(root) = user_library_dir() else {
        return Vec::new();
    };
    let path = root.join("catalog.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    doc.get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Copy pipeline outputs into the library and write their v1 envelopes.
/// The caller publishes Job done only after this returns.
pub fn import_pipeline(work: &Path, manifest: &Value) -> Result<Imported, String> {
    let root = user_library_dir()?;
    let assets = manifest
        .get("assets")
        .and_then(Value::as_object)
        .ok_or("manifest has no assets")?;
    let wav = assets.get("wav").ok_or("manifest has no wav asset")?;
    let wav_uid = wav
        .get("uid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !wav_uid.starts_with("ga:audio_clip:") {
        return Err("generate did not mint an audio_clip uid".into());
    }
    let legacy = legacy_for(&wav_uid);
    let folder = root.join("generated").join(&legacy);
    fs::create_dir_all(&folder).map_err(|err| format!("library folder: {err}"))?;
    let folder = folder
        .canonicalize()
        .map_err(|err| format!("library folder: {err}"))?;
    if !folder.starts_with(&root) {
        return Err("library folder escaped the library root".into());
    }

    let files = manifest.get("files").cloned().unwrap_or(Value::Null);
    let wav_name = files
        .get("wav")
        .and_then(Value::as_str)
        .unwrap_or("fitted.wav");
    copy_from_work(work, wav_name, &folder, "fitted.wav")?;
    copy_optional(
        work,
        files.get("spectrogramPng").and_then(Value::as_str),
        &folder,
        "spectrogram.png",
    )?;
    copy_optional(
        work,
        files.get("spectrogramJson").and_then(Value::as_str),
        &folder,
        "spectrogram.json",
    )?;
    copy_optional(
        work,
        files.get("cubeJson").and_then(Value::as_str),
        &folder,
        "cube.json",
    )?;
    copy_optional(
        work,
        files.get("cubePng").and_then(Value::as_str),
        &folder,
        "cube.png",
    )?;
    let video_copied = copy_optional(
        work,
        files.get("video").and_then(Value::as_str),
        &folder,
        "clip.mp4",
    )?;
    let script_rel = work.join("scripts").join("script.txt");
    if script_rel.is_file() {
        copy_from_work(work, "scripts/script.txt", &folder, "script.txt")?;
    }

    let cube_doc = fs::read_to_string(folder.join("cube.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    let prefix = format!("generated/{legacy}");
    let engine = manifest
        .get("engine")
        .and_then(Value::as_str)
        .unwrap_or("kokoro_onnx");
    let voice_model = baked_voice_model(engine);
    let mut envelopes = Vec::new();
    if script_rel.is_file() {
        if let Some(script) = assets.get("script") {
            envelopes.push(envelope(
                script,
                &format!("{prefix}/script.txt"),
                "text/plain",
                "Generated script",
                &[],
                false,
                None,
                None,
                &Value::Null,
            )?);
        }
    }
    envelopes.push(envelope(
        wav,
        &format!("{prefix}/fitted.wav"),
        "audio/wav",
        "Generated clip",
        &["real_wav", "synthesized_speech"],
        true,
        Some(engine),
        voice_model.as_deref(),
        &cube_doc,
    )?);
    let spec_uid = if let Some(spec) = assets.get("spectrogram") {
        let env = envelope(
            spec,
            &format!("{prefix}/spectrogram.png"),
            "image/png",
            "Generated spectrogram",
            &["library_spectrogram"],
            false,
            None,
            None,
            &Value::Null,
        )?;
        let uid = env["uid"].as_str().unwrap_or("").to_string();
        envelopes.push(env);
        uid
    } else {
        String::new()
    };
    let cube_uid = if let Some(cube) = assets.get("cube") {
        let env = envelope(
            cube,
            &format!("{prefix}/cube.json"),
            "application/json",
            "Generated cube",
            &["library_cube"],
            false,
            None,
            None,
            &cube_doc,
        )?;
        let uid = env["uid"].as_str().unwrap_or("").to_string();
        envelopes.push(env);
        uid
    } else {
        String::new()
    };
    for env in &envelopes {
        asset::validate_envelope(env)
            .map_err(|err| format!("library envelope {}: {err}", env["uid"]))?;
    }
    upsert(&root, envelopes)?;

    let wav_sha = wav.get("sha256").and_then(Value::as_str).unwrap_or("");
    let source_sha = cube_doc
        .get("source_sha256")
        .and_then(Value::as_str)
        .unwrap_or("");
    let derived = cube_doc.get("derived_from").and_then(Value::as_array);
    let clip_in_src = !wav_sha.is_empty()
        && source_sha == wav_sha
        && derived.is_some_and(|items| {
            items
                .iter()
                .any(|item| item.as_str() == Some(wav_uid.as_str()))
        });
    let video_url = if video_copied {
        Some(format!("/library/{prefix}/clip.mp4"))
    } else {
        None
    };
    Ok(Imported {
        wav_uid,
        legacy_id: legacy,
        spec_uid,
        cube_uid,
        wav_url: format!("/library/{prefix}/fitted.wav"),
        spec_url: format!("/library/{prefix}/spectrogram.png"),
        cube_url: format!("/library/{prefix}/cube.json"),
        video_url,
        clip_duration_s: manifest
            .get("duration_s")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        recorded_source_duration_s: cube_doc
            .get("duration_s")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        sec_per_bin: cube_doc
            .get("bin_seconds")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        selector_end: cube_doc
            .get("cube_covers_s")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        clip_in_src,
    })
}

#[derive(Debug)]
pub struct Imported {
    pub wav_uid: String,
    pub legacy_id: String,
    pub spec_uid: String,
    pub cube_uid: String,
    pub wav_url: String,
    pub spec_url: String,
    pub cube_url: String,
    pub video_url: Option<String>,
    pub clip_duration_s: f64,
    pub recorded_source_duration_s: f64,
    pub sec_per_bin: f64,
    pub selector_end: f64,
    pub clip_in_src: bool,
}

/// Rows for `library_list`: generated clips, plus display overrides of baked clips.
pub fn list_overlays() -> Vec<Value> {
    runtime_assets()
        .into_iter()
        .filter(|asset| asset.get("kind").and_then(Value::as_str) == Some("audio_clip"))
        .map(|asset| {
            let legacy = asset.get("legacy_id").and_then(Value::as_str).unwrap_or("");
            let title = asset
                .pointer("/display/semantic_name")
                .or_else(|| asset.pointer("/display/title"))
                .and_then(Value::as_str)
                .unwrap_or(legacy);
            let wav = asset
                .get("media")
                .and_then(Value::as_array)
                .and_then(|items| items.iter().find(|item| item.get("role").and_then(Value::as_str) == Some("wav")))
                .and_then(|item| item.get("path").and_then(Value::as_str))
                .map(|path| format!("/library/{path}"));
            json!({
                "id": legacy,
                "title": title,
                "engineId": asset.pointer("/fields/engine").cloned().unwrap_or(Value::Null),
                "status": asset.get("status").cloned().unwrap_or(json!("ok")),
                "synthesizedSpeech": asset.pointer("/honesty/synthesized_speech").cloned().unwrap_or(json!(false)),
                "wavUrl": wav,
                "cubeJsonUrl": Value::Null,
                "sidecarUrl": Value::Null,
                "summary": asset.pointer("/display/summary").cloned().unwrap_or(json!("Generated clip")),
                "uid": asset.get("uid").cloned().unwrap_or(Value::Null),
                "generated": legacy.starts_with("gen-")
            })
        })
        .collect()
}

/// Persist a display rename onto the catalog envelope and bump `display_rev`.
pub fn persist_rename(
    legacy_id: &str,
    semantic: Option<&str>,
    face: Option<&str>,
) -> Result<u64, String> {
    let root = user_library_dir()?;
    let mut asset =
        find_audio_clip(legacy_id).ok_or_else(|| format!("unknown library clip {legacy_id}"))?;
    let display = asset
        .as_object_mut()
        .ok_or("envelope is not an object")?
        .entry("display")
        .or_insert_with(|| json!({}));
    let display = display.as_object_mut().ok_or("display is not an object")?;
    if let Some(name) = semantic {
        display.insert("semantic_name".into(), json!(name));
        display.insert("title".into(), json!(name));
    }
    if let Some(name) = face {
        display.insert("face_name".into(), json!(name));
    }
    let rev = display
        .get("display_rev")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .saturating_add(1);
    display.insert("display_rev".into(), json!(rev));
    asset::validate_envelope(&asset).map_err(|err| format!("renamed envelope: {err}"))?;
    upsert(&root, vec![asset])?;
    Ok(rev)
}

pub fn open_library_media(rel: &str) -> Result<(PathBuf, &'static str), String> {
    crate::asset::check_media_path(rel).map_err(|err| err.to_string())?;
    let root = user_library_dir()?;
    let path = root.join(rel);
    if path
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("refusing a library symlink".into());
    }
    let canon = path
        .canonicalize()
        .map_err(|_| "library media is not in the library".to_string())?;
    if !canon.starts_with(&root) || !canon.is_file() {
        return Err("library media escaped the library root".into());
    }
    let mime = match canon.extension().and_then(|ext| ext.to_str()) {
        Some("wav") => "audio/wav",
        Some("png") => "image/png",
        Some("json") => "application/json",
        Some("mp4") => "video/mp4",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    Ok((canon, mime))
}

fn find_audio_clip(legacy_id: &str) -> Option<Value> {
    if let Some(asset) = runtime_assets().into_iter().find(|asset| {
        asset.get("kind").and_then(Value::as_str) == Some("audio_clip")
            && asset.get("legacy_id").and_then(Value::as_str) == Some(legacy_id)
    }) {
        return Some(asset);
    }
    baked_assets().into_iter().find(|asset| {
        asset.get("kind").and_then(Value::as_str) == Some("audio_clip")
            && asset.get("legacy_id").and_then(Value::as_str) == Some(legacy_id)
    })
}

fn baked_voice_model(engine: &str) -> Option<String> {
    baked_assets()
        .into_iter()
        .find(|asset| {
            asset.get("kind").and_then(Value::as_str) == Some("voice_model")
                && asset.get("legacy_id").and_then(Value::as_str) == Some(engine)
        })
        .and_then(|asset| asset.get("uid").and_then(Value::as_str).map(str::to_string))
}

fn baked_assets() -> Vec<Value> {
    let doc: Value = serde_json::from_str(include_str!(
        "../../../apps/desktop/public/library/assets.json"
    ))
    .unwrap_or(Value::Null);
    doc.get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn upsert(root: &Path, incoming: Vec<Value>) -> Result<(), String> {
    let mut assets = runtime_assets();
    for env in incoming {
        let uid = env
            .get("uid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(slot) = assets
            .iter_mut()
            .find(|item| item.get("uid").and_then(Value::as_str) == Some(uid.as_str()))
        {
            *slot = env;
        } else {
            assets.push(env);
        }
    }
    assets.sort_by(|a, b| {
        a.get("uid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("uid").and_then(Value::as_str).unwrap_or(""))
    });
    let body = serde_json::to_vec_pretty(&json!({"schema_version": "1.0.0", "assets": assets}))
        .map_err(|err| err.to_string())?;
    let dest = root.join("catalog.json");
    let tmp = root.join(".catalog.json.tmp");
    if dest.parent() != Some(root) || tmp.parent() != Some(root) {
        return Err("catalog path escaped the library root".into());
    }
    fs::write(&tmp, body).map_err(|err| format!("catalog write: {err}"))?;
    fs::rename(&tmp, &dest).map_err(|err| format!("catalog rename: {err}"))?;
    Ok(())
}

/// Copy recorded speaker facts from cube.json. Missing facts stay unresolved.
fn apply_speech_facts(body: &mut Value, kind: &str, cube: &Value) {
    let speakers = cube.get("speakers").cloned().unwrap_or(json!("unresolved"));
    body["speakers"] = speakers.clone();
    if speakers.as_str() == Some("unresolved") {
        return;
    }
    if let Some(segments) = cube.get("segments") {
        body["segments"] = segments.clone();
    }
    if kind == "cube_ihdr" {
        if let Some(duration) = cube.get("duration_s") {
            body["duration_s"] = duration.clone();
        }
        if let Some(bins) = cube.get("speaker_idx") {
            body["speaker_idx"] = bins.clone();
        }
    }
}

fn envelope(
    asset: &Value,
    rel: &str,
    mime: &str,
    title: &str,
    claims: &[&str],
    synthesized: bool,
    engine: Option<&str>,
    voice_model: Option<&str>,
    cube: &Value,
) -> Result<Value, String> {
    let uid = asset
        .get("uid")
        .and_then(Value::as_str)
        .ok_or("asset uid missing")?;
    let (kind, _) = asset::parse_uid(uid).map_err(|err| err.to_string())?;
    let sha = asset
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or("asset sha256 missing")?;
    let bytes = read_media_bytes(rel)?;
    if sha256_hex(&bytes) != sha {
        return Err(format!("{rel} sha256 does not match the minted asset"));
    }
    let role = match kind {
        "audio_clip" => "wav",
        "spectrogram_2d" => "spectrogram_png",
        "cube_ihdr" => "cube_json",
        "podcast_script" => "script_txt",
        other => return Err(format!("cannot catalog kind {other}")),
    };
    // B2: a real cube's ga: uid covers cube_json and cube_png. The png sits
    // beside the json (generated/{id}/cube.png) and is hashed into the mint.
    let mut media_files: Vec<(&str, String, String, u64, &str)> = vec![(role, rel.to_string(), sha.to_string(), bytes.len() as u64, mime)];
    if kind == "cube_ihdr" {
        let png_rel = match rel.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/cube.png"),
            None => "cube.png".to_string(),
        };
        let png_bytes = read_media_bytes(&png_rel).map_err(|_| {
            "a real cube needs cube.png beside cube.json".to_string()
        })?;
        media_files.push((
            "cube_png",
            png_rel,
            sha256_hex(&png_bytes),
            png_bytes.len() as u64,
            "image/png",
        ));
    }
    let fields = coerce_ints(asset.get("fields").cloned().unwrap_or(json!({})));
    let src: Vec<String> = asset
        .get("derived_from")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .filter(|item| item.starts_with("ga:"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let digests: Vec<MediaDigest> = media_files
        .iter()
        .map(|(role, _, sha, _, _)| MediaDigest {
            role: (*role).to_string(),
            sha256: sha.clone(),
        })
        .collect();
    let minted = asset::mint(kind, &fields, &digests, &src).map_err(|err| err.to_string())?;
    if minted.uid != uid {
        return Err(format!("uid {uid} != recomputed {}", minted.uid));
    }
    let glyph = glyph_from_uid(uid).map_err(|err| err.to_string())?;
    let mut provenance = json!({"generator": "ui_generate"});
    if let Some(engine) = engine {
        provenance["engine"] = json!(engine);
    }
    if let Some(model) = voice_model {
        provenance["voice_model"] = json!(model);
    }
    let mut body = json!({});
    if kind == "audio_clip" {
        body["wav_url"] = json!(format!("/library/{rel}"));
        if let Some(duration) = asset.get("duration_s") {
            body["duration_s"] = duration.clone();
        }
    }
    if kind == "audio_clip" || kind == "cube_ihdr" {
        apply_speech_facts(&mut body, kind, cube);
    }
    Ok(json!({
        "schema_version": "1.0.0",
        "uid_scheme": "ga1",
        "kind": kind,
        "uid": uid,
        "legacy_id": legacy_for(uid),
        "status": "ok",
        "fields": fields,
        "media": media_files.iter().map(|(role, path, sha, nbytes, mime)| json!({
            "role": role,
            "path": path,
            "sha256": sha,
            "bytes": nbytes,
            "mime": mime
        })).collect::<Vec<_>>(),
        "src": src,
        "relations": {},
        "honesty": {
            "claims": claims,
            "fixture": false,
            "synthesized_speech": synthesized
        },
        "provenance": provenance,
        "display": {"title": title, "glyph": glyph, "display_rev": 1},
        "body": body
    }))
}

fn coerce_ints(fields: Value) -> Value {
    let Some(map) = fields.as_object() else {
        return fields;
    };
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        if key == "engine" || key == "colormap" || key == "format" || key == "source_sha256" {
            out.insert(key.clone(), value.clone());
            continue;
        }
        if let Some(number) = value.as_f64() {
            if number.is_finite()
                && number >= 0.0
                && number.fract() == 0.0
                && number <= u64::MAX as f64
            {
                out.insert(key.clone(), json!(number as u64));
                continue;
            }
        }
        if let Some(number) = value.as_i64() {
            if number >= 0 {
                out.insert(key.clone(), json!(number as u64));
                continue;
            }
        }
        out.insert(key.clone(), value.clone());
    }
    Value::Object(out)
}

fn read_media_bytes(rel: &str) -> Result<Vec<u8>, String> {
    let (path, _) = open_library_media(rel)?;
    fs::read(&path).map_err(|err| format!("read {rel}: {err}"))
}

fn legacy_for(uid: &str) -> String {
    let body = uid.rsplit(':').next().unwrap_or("clip");
    format!("gen-{body}")
}

fn copy_optional(
    work: &Path,
    rel: Option<&str>,
    folder: &Path,
    dest_name: &str,
) -> Result<bool, String> {
    let Some(rel) = rel.filter(|text| !text.is_empty()) else {
        return Ok(false);
    };
    if !work.join(rel).is_file() {
        return Ok(false);
    }
    copy_from_work(work, rel, folder, dest_name)?;
    Ok(true)
}

fn copy_from_work(work: &Path, rel: &str, folder: &Path, dest_name: &str) -> Result<(), String> {
    if rel.contains("..") || rel.starts_with('/') || rel.starts_with('\\') {
        return Err("output path escaped the work directory".into());
    }
    if dest_name.contains('/') || dest_name.contains('\\') || dest_name.contains("..") {
        return Err("library file name is invalid".into());
    }
    let work = work
        .canonicalize()
        .map_err(|err| format!("work directory: {err}"))?;
    let source = work.join(rel);
    if source
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("refusing a work-directory symlink".into());
    }
    let source = source
        .canonicalize()
        .map_err(|err| format!("work file: {err}"))?;
    if !source.starts_with(&work) || !source.is_file() {
        return Err("output path escaped the work directory".into());
    }
    let root = user_library_dir()?;
    let folder = folder
        .canonicalize()
        .map_err(|err| format!("library folder: {err}"))?;
    if !folder.starts_with(&root) {
        return Err("library folder escaped the library root".into());
    }
    let dest = folder.join(dest_name);
    if dest.parent() != Some(folder.as_path()) {
        return Err("library file escaped the library root".into());
    }
    fs::copy(&source, &dest).map_err(|err| format!("library copy: {err}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_root_refuses_the_repository() {
        let repo = find_repo_root().expect("repo");
        set_root_override_for_test(Some(repo));
        let err = user_library_dir().unwrap_err();
        assert!(err.contains("repository"), "{err}");
        set_root_override_for_test(None);
    }

    #[test]
    fn import_pipeline_writes_a_resolvable_catalog_uid() {
        let root = std::env::temp_dir().join(format!("ga-lib-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        set_root_override_for_test(Some(root.clone()));
        let work = std::env::temp_dir().join(format!("ga-lib-work-{}", std::process::id()));
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).unwrap();
        let bytes = b"RIFF-library-clip";
        fs::write(work.join("fitted.wav"), bytes).unwrap();
        let sha = sha256_hex(bytes);
        let fields = json!({"engine": "kokoro_onnx", "sample_rate_hz": 24000, "duration_ms": 1000});
        let minted = asset::mint(
            "audio_clip",
            &fields,
            &[MediaDigest {
                role: "wav".into(),
                sha256: sha.clone(),
            }],
            &[],
        )
        .unwrap();
        let manifest = json!({
            "duration_s": 1.0,
            "engine": "kokoro_onnx",
            "synthesizedSpeech": true,
            "assets": {"wav": {"uid": minted.uid, "sha256": sha, "fields": fields, "derived_from": []}},
            "files": {"wav": "fitted.wav"}
        });
        let imported = import_pipeline(&work, &manifest).expect("import");
        assert_eq!(imported.wav_uid, minted.uid);
        assert!(imported.legacy_id.starts_with("gen-"));
        assert!(imported.wav_url.starts_with("/library/generated/"));
        let resolved = crate::asset_catalog::resolve(&imported.wav_uid).expect("resolve");
        match resolved {
            crate::asset_catalog::Resolve::Found(asset) => {
                assert_eq!(asset["legacy_id"], imported.legacy_id);
                assert!(asset["honesty"]["synthesized_speech"] == true);
            }
            other => panic!("expected the imported uid, got {other:?}"),
        }
        let rev = persist_rename(&imported.legacy_id, Some("Narrator"), None).expect("rename");
        assert!(rev >= 2);
        let again = crate::asset_catalog::require(&imported.wav_uid).expect("renamed");
        assert_eq!(again["display"]["semantic_name"], "Narrator");
        assert!(!again.to_string().contains(&root.display().to_string()));
        set_root_override_for_test(None);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn import_pipeline_mints_a_real_cube_with_png() {
        let root = std::env::temp_dir().join(format!("ga-lib-cube-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        set_root_override_for_test(Some(root.clone()));
        let work = std::env::temp_dir().join(format!("ga-lib-cube-work-{}", std::process::id()));
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).unwrap();
        let wav_bytes = b"RIFF-library-clip";
        let json_bytes = br#"{"speakers":"unresolved","duration_s":1.0}"#;
        let png_bytes = b"\x89PNG-cube";
        fs::write(work.join("fitted.wav"), wav_bytes).unwrap();
        fs::write(work.join("cube.json"), json_bytes).unwrap();
        fs::write(work.join("cube.png"), png_bytes).unwrap();
        let wav_sha = sha256_hex(wav_bytes);
        let json_sha = sha256_hex(json_bytes);
        let png_sha = sha256_hex(png_bytes);
        let wav_fields = json!({"engine": "kokoro_onnx", "sample_rate_hz": 24000, "duration_ms": 1000});
        let wav = asset::mint(
            "audio_clip",
            &wav_fields,
            &[MediaDigest { role: "wav".into(), sha256: wav_sha.clone() }],
            &[],
        )
        .unwrap();
        let cube_fields = json!({
            "source_sha256": wav_sha,
            "sample_rate_hz": 24000,
            "bin_frames": 256,
            "time_bins": 1,
            "freq_bins": 102,
            "duration_ms": 1000,
            "covers_ms": 1000,
            "inv_hdr_ppm": 70211,
            "cube_revision": 3,
            "n_points": 1
        });
        let cube = asset::mint(
            "cube_ihdr",
            &cube_fields,
            &[
                MediaDigest { role: "cube_json".into(), sha256: json_sha.clone() },
                MediaDigest { role: "cube_png".into(), sha256: png_sha.clone() },
            ],
            &[wav.uid.clone()],
        )
        .unwrap();
        assert!(cube.uid.starts_with("ga:cube_ihdr:"), "{}", cube.uid);
        let manifest = json!({
            "duration_s": 1.0,
            "engine": "kokoro_onnx",
            "synthesizedSpeech": true,
            "assets": {
                "wav": {"uid": wav.uid, "sha256": wav_sha, "fields": wav_fields, "derived_from": []},
                "cube": {
                    "uid": cube.uid,
                    "sha256": json_sha,
                    "fields": cube_fields,
                    "derived_from": [wav.uid],
                    "media": [
                        {"role": "cube_json", "sha256": json_sha},
                        {"role": "cube_png", "sha256": png_sha}
                    ]
                }
            },
            "files": {"wav": "fitted.wav", "cubeJson": "cube.json", "cubePng": "cube.png"}
        });
        let imported = import_pipeline(&work, &manifest).expect("import");
        assert_eq!(imported.cube_uid, cube.uid);
        match crate::asset_catalog::resolve(&imported.cube_uid).expect("resolve") {
            crate::asset_catalog::Resolve::Found(asset) => {
                let roles: Vec<&str> = asset["media"].as_array().unwrap().iter().map(|item| item["role"].as_str().unwrap()).collect();
                assert_eq!(roles, ["cube_json", "cube_png"]);
                assert!(asset["honesty"]["claims"].as_array().unwrap().iter().any(|claim| claim == "library_cube"));
            }
            other => panic!("expected the imported cube, got {other:?}"),
        }
        set_root_override_for_test(None);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&work);
    }
}
