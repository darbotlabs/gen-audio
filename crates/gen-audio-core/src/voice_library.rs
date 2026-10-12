//! One persona profile contains bindings for every known engine. Acoustic
//! references are immutable, hash-verified library inputs, not persona names.
use crate::{
    asset::sha256_hex,
    bridge,
    paths::{self, Scratch},
};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

const MANIFEST: &str = include_str!("../../../voices/manifest.json");
pub fn manifest() -> &'static Value {
    static LIBRARY: OnceLock<Value> = OnceLock::new();
    LIBRARY
        .get_or_init(|| serde_json::from_str(MANIFEST).expect("validated voice library manifest"))
}
pub fn profile(persona: &str) -> Option<&'static Value> {
    manifest()["profiles"].get(persona)
}
pub fn reference(id: &str) -> Option<&'static Value> {
    manifest()["references"]
        .as_array()?
        .iter()
        .find(|r| r["id"] == id)
}
pub fn reference_for(persona: &str, engine: &str) -> Option<&'static str> {
    profile(persona)?["engineManifests"][engine]["referenceId"].as_str()
}
pub fn list() -> Value {
    let refs = manifest()["references"]
        .as_array()
        .expect("references")
        .iter()
        .map(|r| {
            let mut value = r.clone();
            value.as_object_mut().expect("reference").remove("file");
            value
        })
        .collect::<Vec<_>>();
    json!({"schemaVersion":"2.0","references":refs,"synthesizedSpeech":false})
}
pub fn validate_bindings(bindings: &Value) -> Result<(), String> {
    let map = bindings
        .as_object()
        .ok_or("engineManifests must be an object")?;
    let known = [
        "kokoro_onnx",
        "kokoro_dayour",
        "misaki",
        "vibevoice",
        "personaplex",
        "magpie",
        "pocket_tts",
    ];
    if map.len() != known.len() || known.iter().any(|key| !map.contains_key(*key)) {
        return Err("Profile must contain all seven engine manifests".into());
    }
    for (engine, binding) in map {
        let obj = binding
            .as_object()
            .ok_or("engine manifest must be an object")?;
        if obj.keys().any(|key| {
            ![
                "capability",
                "adapter",
                "mapping",
                "language",
                "requires",
                "voiceId",
                "referenceId",
                "downstreamEngine",
            ]
            .contains(&key.as_str())
        }) {
            return Err("unknown engine manifest field".into());
        }
        if !binding["capability"]
            .as_str()
            .is_some_and(|s| ["script_tts", "speech_to_speech", "g2p"].contains(&s))
            || !binding["adapter"]
                .as_str()
                .is_some_and(|s| ["synth", "model_generate", "g2p", "external"].contains(&s))
        {
            return Err("invalid engine capability/adapter".into());
        }
        let mapped = match binding["mapping"].as_str() {
            Some("mapped") => true,
            Some("unmapped") => false,
            _ => return Err("invalid engine mapping".into()),
        };
        let lang = binding["language"].as_str().ok_or("language missing")?;
        let chunks = lang.split('-').collect::<Vec<_>>();
        if chunks.is_empty()
            || chunks.len() > 2
            || chunks
                .iter()
                .any(|s| s.len() != 2 || !s.bytes().all(|b| b.is_ascii_lowercase()))
        {
            return Err("invalid binding language".into());
        }
        let requires = binding["requires"]
            .as_array()
            .ok_or("engine requirements missing")?;
        let mut seen = std::collections::HashSet::new();
        if requires.is_empty()
            || requires.len() > 12
            || requires.iter().any(|v| {
                !v.as_str().is_some_and(|s| {
                    !s.is_empty()
                        && s.len() <= 64
                        && s.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                        && seen.insert(s)
                })
            })
        {
            return Err("invalid/duplicate engine requirements".into());
        }
        let voice = binding.get("voiceId");
        let reference = binding.get("referenceId");
        if mapped != (voice.is_some() ^ reference.is_some())
            || (!mapped
                && (voice.is_some()
                    || reference.is_some()
                    || binding.get("downstreamEngine").is_some()))
        {
            return Err(
                "mapped binding requires exactly one engine voice or acoustic reference".into(),
            );
        }
        if let Some(id) = voice {
            let id = id.as_str().ok_or("voiceId must be a string")?;
            let (prefix, tail) = id.split_once('_').ok_or("invalid pack voiceId")?;
            if id.len() > 80
                || prefix.len() != 2
                || !prefix.bytes().all(|b| b.is_ascii_lowercase())
                || tail.is_empty()
                || !tail
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err("invalid pack voiceId".into());
            }
        }
        if let Some(id) = reference {
            let id = id.as_str().ok_or("referenceId must be a string")?;
            if id.len() < 2
                || id.len() > 80
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !id.as_bytes()[0].is_ascii_alphanumeric()
            {
                return Err("invalid referenceId".into());
            }
        }
        if binding
            .get("downstreamEngine")
            .is_some_and(|v| v != "kokoro_onnx")
        {
            return Err("invalid downstream engine".into());
        }
        if engine == "misaki" && binding["capability"] != "g2p" {
            return Err("Misaki is G2P".into());
        }
        if engine == "personaplex" && binding["capability"] != "speech_to_speech" {
            return Err("PersonaPlex is conversation speech".into());
        }
        if !matches!(engine.as_str(), "misaki" | "personaplex")
            && binding["capability"] != "script_tts"
        {
            return Err("invalid script-TTS capability".into());
        }
    }
    Ok(())
}

/// Library fields share the profile schema and never contain caller file URLs.
pub fn validate_profile_library(profile: &Value) -> Result<(), String> {
    fn object<'a>(value: &'a Value, fields: &[&str]) -> Result<&'a serde_json::Map<String, Value>, String> {
        let map = value.as_object().ok_or("Profile library entry must be an object")?;
        if map.keys().any(|key| !fields.contains(&key.as_str())) { return Err("Unknown profile library field".into()); }
        Ok(map)
    }
    fn text(value: &Value, min: usize, max: usize) -> Result<&str, String> {
        let value = value.as_str().ok_or("Profile library text must be a string")?;
        if !(min..=max).contains(&value.chars().count()) { return Err("Profile library text exceeds limits".into()); }
        Ok(value)
    }
    for (field, cap) in [("tags",20), ("samples",32), ("linkedProjects",32)] {
        let Some(value) = profile.get(field) else { continue; };
        let entries = value.as_array().ok_or("Profile library field must be an array")?;
        if entries.len() > cap { return Err("Profile library array exceeds limit".into()); }
        for (index, entry) in entries.iter().enumerate() {
            match field {
                "tags" => { text(entry,1,64)?; if entries[..index].contains(entry) { return Err("Profile tags must be unique".into()); } }
                "samples" => {
                    object(entry,&["referenceId","label","kind"])?;
                    let id = text(&entry["referenceId"],1,80)?;
                    if reference(id).is_none() || entry["kind"] != "acoustic_reference" { return Err("Profile sample must name a managed acoustic reference".into()); }
                    text(&entry["label"],1,160)?;
                }
                _ => {
                    object(entry,&["id","name","path","slide"])?;
                    let id = text(&entry["id"],1,80)?;
                    if !id.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c,'_'|'-')) { return Err("Invalid project id".into()); }
                    text(&entry["name"],1,160)?;
                    let path = text(&entry["path"],1,240)?;
                    if path.split('/').any(|part| !part.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c,'_'|'-')) || !part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c,'_'|'-'|'.'))) { return Err("Project paths must stay relative to the project".into()); }
                    if !matches!(entry["slide"].as_str(),Some("studio"|"library"|"models"|"profiles"|"spatial")) { return Err("Invalid project slide".into()); }
                }
            }
        }
    }
    if let Some(metadata) = profile.get("metadata") {
        object(metadata,&["artist"])?; text(&metadata["artist"],1,128)?;
        if metadata["artist"] != profile["agentName"] { return Err("Profile Artist must equal AgentName".into()); }
    }
    if let Some(card) = profile.get("adaptiveCard") {
        object(card,&["$schema","type","version","body","actions"])?;
        if card["type"] != "AdaptiveCard" || card["version"] != "1.5"
            || card.get("$schema").is_some_and(|v| v != "https://adaptivecards.io/schemas/adaptive-card.json") { return Err("Invalid agent Adaptive Card".into()); }
        let body = card["body"].as_array().filter(|v| !v.is_empty() && v.len()<=64).ok_or("Invalid Adaptive Card body")?;
        for item in body {
            match item["type"].as_str() {
                Some("TextBlock") => {
                    object(item,&["type","text","wrap","weight"])?; text(&item["text"],0,4000)?;
                    if item.get("wrap").is_some_and(|v| !v.is_boolean()) || item.get("weight").is_some_and(|v| !matches!(v.as_str(),Some("Bolder"|"Default"))) { return Err("Invalid Adaptive Card TextBlock".into()); }
                }
                Some("FactSet") => {
                    object(item,&["type","facts"])?;
                    for fact in item["facts"].as_array().filter(|v|v.len()<=32).ok_or("Invalid Adaptive Card facts")? {
                        object(fact,&["title","value"])?; text(&fact["title"],0,160)?; text(&fact["value"],0,4000)?;
                    }
                }
                _ => return Err("Unsupported agent Adaptive Card element".into())
            }
        }
        for action in card["actions"].as_array().filter(|v|v.len()<=32).ok_or("Invalid Adaptive Card actions")? {
            object(action,&["type","title","data"])?; text(&action["title"],0,160)?;
            let data = &action["data"]; object(data,&["tool","arguments"])?;
            if action["type"] != "Action.Submit" || !data["arguments"].is_object() || !matches!(data["tool"].as_str(),Some("ui_set_sidepane"|"ui_navigate"|"voice_reference_prepare")) { return Err("Invalid Adaptive Card action".into()); }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unified_bindings_are_complete_and_reject_ambiguous_identity() {
        for person in crate::catalog::personas() {
            let profile = crate::catalog::voice_profile_value(person.id).unwrap();
            crate::cards::validate_voice_profile(&profile).unwrap();
            assert_eq!(profile["metadata"]["artist"], person.name);
            assert!(profile["adaptiveCard"]["actions"].as_array().unwrap().len() >= 3);
        }
        for profile in manifest()["profiles"].as_object().unwrap().values() {
            validate_bindings(&profile["engineManifests"]).unwrap();
        }
        let mut bad = profile("alice").unwrap()["engineManifests"].clone();
        bad.as_object_mut().unwrap().remove("misaki");
        assert!(validate_bindings(&bad).is_err());
        let mut bad = profile("alice").unwrap()["engineManifests"].clone();
        bad["vibevoice"]["mapping"] = json!("unmapped");
        bad["vibevoice"]["voiceId"] = json!("af_heart");
        assert!(validate_bindings(&bad).is_err());
    }
}
/// Copy only a declared, digest-pinned library WAV into this process scratch.
/// No RPC-supplied filesystem path is accepted.
pub fn prepare(repo: &Path, scratch: &Scratch, id: &str) -> Result<Value, String> {
    let entry = reference(id).ok_or("unknown managed voice reference")?;
    let file = entry["file"]
        .as_str()
        .ok_or("voice reference file missing")?;
    let input = paths::read_repo_relative(repo, &format!("voices/{file}"))?;
    let bytes = paths::read_bounded(&input, 2 * 1024 * 1024)?;
    if entry["sha256"] != sha256_hex(&bytes) || entry["bytes"].as_u64() != Some(bytes.len() as u64)
    {
        return Err("managed voice reference digest/size mismatch".into());
    }
    let (rate, samples) = bridge::wav_metadata(&bytes)?;
    if rate != 24000 || samples == 0 || samples as f64 / rate as f64 > 30.0 {
        return Err("managed reference must be nonempty mono 24 kHz WAV up to 30 seconds".into());
    }
    let name = format!("voice-{}-{}.wav", id, &sha256_hex(&bytes)[..12]);
    if let Ok(existing) = scratch.open_input(&name) {
        if sha256_hex(&fs::read(existing).map_err(|e| e.to_string())?) == entry["sha256"] {
            return Ok(
                json!({"id":id,"wav":name,"sha256":entry["sha256"],"synthesizedSpeech":false}),
            );
        }
        return Err("issued voice reference changed".into());
    }
    let output = scratch.prepare_output(&name)?;
    let mut dest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| e.to_string())?;
    use std::io::Write;
    dest.write_all(&bytes).map_err(|e| e.to_string())?;
    dest.sync_all().map_err(|e| e.to_string())?;
    Ok(json!({"id":id,"wav":name,"sha256":entry["sha256"],"synthesizedSpeech":false}))
}
