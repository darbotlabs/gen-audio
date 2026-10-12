//! Persist verified synthesis artifacts as ga1 assets. This function receives
//! backend-verified receipts, never caller-supplied speech claims.
use crate::{
    asset::{self, EnvelopeParts},
    asset_catalog, bridge,
    paths::{self, Scratch},
};
use serde_json::{json, Map, Value};
use std::{fs, path::PathBuf};

fn root() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?;
    let root = PathBuf::from(base).join("Gen-Audio/library");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    paths::reject_symlink(&root)?;
    Ok(root)
}
fn copy_new(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    paths::reject_symlink(path)?;
    use std::io::Write;
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if paths::read_bounded(path, 128 * 1024 * 1024)? == bytes {
                Ok(())
            } else {
                Err("saved artifact differs; overwrite refused".into())
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
pub fn save(scratch: &Scratch, result: &Value) -> Result<Value, String> {
    save_at(scratch, result, &root()?)
}
fn save_at(scratch: &Scratch, result: &Value, base: &std::path::Path) -> Result<Value, String> {
    if result["synthesizedSpeech"] != true {
        return Err("Only independently verified generated speech can be saved".into());
    }
    let wav = scratch.open_input(result["wav"].as_str().ok_or("missing WAV name")?)?;
    let manifest =
        scratch.open_input(result["manifest"].as_str().ok_or("missing manifest name")?)?;
    let bytes = paths::read_bounded(&wav, 128 * 1024 * 1024)?;
    let report = paths::read_bounded(&manifest, 512 * 1024)?;
    let (rate, samples) = bridge::wav_metadata(&bytes)?;
    let digest = asset::sha256_hex(&bytes);
    if result["sha256"] != digest || result["samples"].as_u64() != Some(samples) {
        return Err("verified generation changed before save".into());
    }
    let engine = result["engine"].as_str().unwrap_or("kokoro_onnx");
    let voice_model =
        asset_catalog::uid_for_legacy("voice_model", engine).ok_or("engine asset missing")?;
    let complete = result["complete"].as_bool().unwrap_or(true);
    let provenance = json!({"engine":engine,"voice_model":voice_model,"generator":"gen-audio verified synthesis",
        "params":{"generation_complete":complete,"transcript_verified":false,"manifest_sha256":asset::sha256_hex(&report),"manifest_bytes":report.len(),"personas":result.get("personas").cloned().unwrap_or(json!([])),
        "reference_ids":result.get("referenceIds").cloned().unwrap_or(json!([])),"profile_version":result.get("profileVersion").cloned().unwrap_or(json!("2.0"))}});
    let envelope=asset::build_envelope(EnvelopeParts {kind:"audio_clip",legacy_id:None,
        title:format!("{} generation · {}",engine,if complete {"complete"} else {"incomplete"}),
        summary:Some("Locally generated, WAV integrity verified; audible transcript not verified".into()),status:"ok",
        fields:json!({"engine":engine,"sample_rate_hz":rate,"duration_ms":samples*1000/rate as u64,"channels":1}),
        media:vec![json!({"role":"wav","path":"audio.wav","mime":"audio/wav","sha256":digest,"bytes":bytes.len()})],
        src:vec![],relations:Map::new(),honesty:json!({"synthesized_speech":true,"fixture":false,"not_podcast":false,"claims":["real_wav","synthesized_speech"]}),
        provenance:provenance.as_object().unwrap().clone(),body:json!({"sample_rate":rate,"duration_s":samples as f64/rate as f64})
    }).map_err(|e|e.to_string())?;
    asset::validate_envelope(&envelope).map_err(|e| e.to_string())?;
    let uid = envelope["uid"].as_str().ok_or("minted asset UID missing")?;
    let folder = base.join(uid.rsplit(':').next().unwrap());
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    paths::reject_symlink(&folder)?;
    let asset_path = folder.join("asset.json");
    if asset_path.exists() {
        // ga1 audio identity excludes per-run paths and provenance. Identical
        // audio must remain one asset while each generation retains its report.
        let existing: Value = serde_json::from_slice(&paths::read_bounded(&asset_path, 512 * 1024)?)
            .map_err(|_| "Saved generation asset is invalid")?;
        asset::validate_envelope(&existing).map_err(|e| e.to_string())?;
        if existing["uid"] != uid || existing["honesty"]["synthesized_speech"] != true {
            return Err("Saved generation identity/claim mismatch".into());
        }
        for media in existing["media"].as_array().ok_or("Saved media is missing")? {
            if asset::verify_media(&folder, media).map_err(|e| e.to_string())? != asset::MediaCheck::Verified {
                return Err("Saved generation media is missing".into());
            }
        }
        let original = paths::read_bounded(&folder.join("generation.json"), 512 * 1024)?;
        if existing["provenance"]["params"]["manifest_sha256"] != asset::sha256_hex(&original)
            || existing["provenance"]["params"]["manifest_bytes"].as_u64() != Some(original.len() as u64) {
            return Err("Saved generation manifest integrity mismatch".into());
        }
        let runs = folder.join("runs");
        fs::create_dir_all(&runs).map_err(|e| e.to_string())?;
        paths::reject_symlink(&runs)?;
        copy_new(&runs.join(format!("{}.json", asset::sha256_hex(&report))), &report)?;
        return Ok(json!({"uid":uid,"saved":true,"complete":complete,"schema_version":"1.0.0","duplicateAudio":true}));
    }
    copy_new(&folder.join("audio.wav"), &bytes)?;
    copy_new(&folder.join("generation.json"), &report)?;
    copy_new(
        &folder.join("asset.json"),
        &serde_json::to_vec_pretty(&envelope).map_err(|e| e.to_string())?,
    )?;
    Ok(json!({"uid":uid,"saved":true,"complete":complete,"schema_version":"1.0.0"}))
}
pub fn list() -> Result<Value, String> {
    list_at(&root()?)
}
fn list_at(base: &std::path::Path) -> Result<Value, String> {
    let mut assets = Vec::new();
    for entry in fs::read_dir(base).map_err(|e| e.to_string())?.flatten() {
        if assets.len() >= 1000 {
            break;
        }
        let folder = entry.path();
        if paths::reject_symlink(&folder).is_err() || !folder.is_dir() {
            continue;
        }
        let path = folder.join("asset.json");
        let Ok(bytes) = paths::read_bounded(&path, 512 * 1024) else {
            continue;
        };
        let Ok(asset) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if asset::validate_envelope(&asset).is_ok() {
            assets.push(asset);
        }
    }
    assets.sort_by(|a, b| a["uid"].as_str().cmp(&b["uid"].as_str()));
    let count = assets.len();
    assets.truncate(100);
    Ok(json!({"assets":assets,"count":count,"limit":100,"synthesizedSpeech":false}))
}
pub fn reopen(scratch: &Scratch, uid: &str) -> Result<Value, String> {
    reopen_at(scratch, uid, &root()?)
}
fn reopen_at(scratch: &Scratch, uid: &str, base: &std::path::Path) -> Result<Value, String> {
    let (kind, digest) = asset::parse_uid(uid).map_err(|e| e.to_string())?;
    if kind != "audio_clip" {
        return Err("saved generation must be an audio_clip UID".into());
    }
    let _ = digest;
    let suffix = uid.rsplit(':').next().unwrap();
    let folder = base.join(suffix);
    paths::reject_symlink(&folder)?;
    let envelope: Value = serde_json::from_slice(&paths::read_bounded(
        &folder.join("asset.json"),
        512 * 1024,
    )?)
    .map_err(|e| e.to_string())?;
    asset::validate_envelope(&envelope).map_err(|e| e.to_string())?;
    if envelope["uid"] != uid || envelope["honesty"]["synthesized_speech"] != true {
        return Err("saved asset identity/claim mismatch".into());
    }
    for media in envelope["media"].as_array().ok_or("missing media")? {
        if asset::verify_media(&folder, media).map_err(|e| e.to_string())?
            != asset::MediaCheck::Verified
        {
            return Err("saved generation media missing".into());
        }
    }
    let bytes = paths::read_bounded(&folder.join("audio.wav"), 128 * 1024 * 1024)?;
    let report = paths::read_bounded(&folder.join("generation.json"), 512 * 1024)?;
    let params = &envelope["provenance"]["params"];
    if params["manifest_sha256"] != asset::sha256_hex(&report)
        || params["manifest_bytes"].as_u64() != Some(report.len() as u64)
    {
        return Err("saved generation manifest integrity mismatch".into());
    }
    let (rate, samples) = bridge::wav_metadata(&bytes)?;
    let wav = format!("saved-{suffix}.wav");
    let manifest = format!("saved-{suffix}.json");
    for (name, content) in [(&wav, &bytes), (&manifest, &report)] {
        let dest = if let Ok(path) = scratch.open_input(name) {
            path
        } else {
            scratch.prepare_output(name)?
        };
        copy_new(&dest, content)?;
    }
    let complete = envelope["provenance"]["params"]["generation_complete"]
        .as_bool()
        .unwrap_or(false);
    Ok(
        json!({"ok":complete,"synthesizedSpeech":true,"artifact_verified":true,"complete":complete,"limit_reached":!complete,
        "wav":wav,"manifest":manifest,"sample_rate":rate,"samples":samples,"duration_s":samples as f64/rate as f64,
        "sha256":asset::sha256_hex(&bytes),"assetUid":uid,"engine":envelope["provenance"]["engine"],
        "metadata":serde_json::from_slice::<Value>(&report).map_err(|_| "Invalid saved report")?.get("metadata").cloned().unwrap_or(json!({}))}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_envelopes_round_trip_and_tampering_is_refused() {
        // A managed reference stands in for a backend-verified artifact here;
        // this is an archive contract test, not a synthesis test.
        let work = Scratch::create().unwrap();
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prepared = crate::voice_library::prepare(&repo, &work, "en-Alice_woman").unwrap();
        let name = prepared["wav"].as_str().unwrap();
        let bytes = fs::read(work.open_input(name).unwrap()).unwrap();
        let (rate, samples) = bridge::wav_metadata(&bytes).unwrap();
        let report = work.prepare_output("archive-contract.json").unwrap();
        fs::write(&report, b"{\"test_fixture\":true}").unwrap();
        let base = work.dir.join("archive");
        fs::create_dir(&base).unwrap();
        let receipt = json!({"synthesizedSpeech":true,"wav":name,"manifest":"archive-contract.json",
            "sha256":asset::sha256_hex(&bytes),"samples":samples,"sample_rate":rate,
            "engine":"vibevoice","complete":false,"personas":["alice"],"referenceIds":["en-Alice_woman"]});
        let saved = save_at(&work, &receipt, &base).unwrap();
        assert_eq!(save_at(&work, &receipt, &base).unwrap()["uid"], saved["uid"]);
        let listed = list_at(&base).unwrap();
        assert_eq!(listed["count"], 1);
        let uid = saved["uid"].as_str().unwrap();
        let opened = reopen_at(&work, uid, &base).unwrap();
        assert_eq!(opened["sha256"], receipt["sha256"]);
        assert_eq!(opened["ok"], false);
        assert_eq!(opened["complete"], false);
        assert_eq!(opened["artifact_verified"], true);
        assert_eq!(opened["limit_reached"], true);
        let folder = base.join(uid.rsplit(':').next().unwrap());
        fs::write(&report, b"{\"test_fixture\":true,\"different_run\":true}").unwrap();
        let repeated = save_at(&work, &receipt, &base).unwrap();
        assert_eq!(repeated["uid"], saved["uid"]);
        assert_eq!(repeated["duplicateAudio"], true);
        assert_eq!(fs::read(folder.join("generation.json")).unwrap(), b"{\"test_fixture\":true}");
        assert_eq!(list_at(&base).unwrap()["count"], 1);
        fs::write(folder.join("audio.wav"), b"tampered").unwrap();
        assert!(reopen_at(&work, uid, &base).is_err());
        assert!(save_at(&work, &receipt, &base).is_err());
        let runs = folder.join("runs");
        for entry in fs::read_dir(&runs).unwrap().flatten() { fs::remove_file(entry.path()).unwrap(); }
        fs::remove_dir(runs).unwrap();
        for file in ["audio.wav", "generation.json", "asset.json"] {
            fs::remove_file(folder.join(file)).unwrap();
        }
        fs::remove_dir(folder).unwrap();
        fs::remove_dir(base).unwrap();
        for entry in fs::read_dir(&work.dir).unwrap().flatten() {
            fs::remove_file(entry.path()).unwrap();
        }
        fs::remove_dir(&work.dir).unwrap();
    }
}
