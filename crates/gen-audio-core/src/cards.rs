//! Runtime checks for the card viewport document.
//!
//! The JSON Schema in `schemas/card-viewport.schema.json` is the contract.
//! This checker enforces the same required fields so a bad document never
//! reaches the renderer as if it were valid.

use serde_json::Value;

pub const CARD_KINDS: &[&str] = &[
    "EngineStatus",
    "SpectrogramPanel",
    "Cube3D",
    "PodcastCast",
    "ServeHealth",
    "BenchmarkCompare",
    "ConnectorStatus",
    "LibraryClip",
    "VoiceProfile",
];

pub const CONNECTOR_IDS: &[&str] = &[
    "mcp",
    "acp",
    "harness",
    "copilot",
    "claude",
    "gpt",
    "gemini",
    "local",
];

pub fn validate_viewport(document: &Value) -> Result<(), String> {
    let root = document.as_object().ok_or("viewport must be an object")?;
    expect_const(root.get("version"), "1.0", "version")?;
    expect_string(root.get("title"), "title", 1, 160)?;
    if let Some(columns) = root.get("columns") {
        let n = columns.as_u64().ok_or("columns must be an integer")?;
        if !(1..=4).contains(&n) {
            return Err("columns must be from 1 to 4".into());
        }
    }
    let cards = root
        .get("cards")
        .and_then(Value::as_array)
        .ok_or("cards must be an array")?;
    let mut seen = std::collections::BTreeSet::new();
    for card in cards {
        validate_card(card, &mut seen)?;
    }
    Ok(())
}

fn validate_card(card: &Value, seen: &mut std::collections::BTreeSet<String>) -> Result<(), String> {
    let obj = card.as_object().ok_or("card must be an object")?;
    let id = expect_string(obj.get("id"), "id", 1, 64)?;
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!("card id {id} has unsupported characters"));
    }
    if !seen.insert(id.clone()) {
        return Err(format!("duplicate card id {id}"));
    }
    let kind = expect_string(obj.get("kind"), "kind", 1, 40)?;
    if !CARD_KINDS.contains(&kind.as_str()) {
        return Err(format!("card {id} has unknown kind {kind}"));
    }
    expect_string(obj.get("title"), "title", 1, 120)?;
    if let Some(uid) = obj.get("uid") {
        // Asset object model v1: the card uid sits alongside the legacy id.
        let uid = uid.as_str().ok_or_else(|| format!("card {id} uid must be a string"))?;
        match crate::asset::parse_uid(uid) {
            Ok(("card", _)) => {}
            _ => return Err(format!("card {id} uid must be ga:card:<26 base32>")),
        }
    }
    if let Some(span) = obj.get("span") {
        let n = span.as_u64().ok_or("span must be an integer")?;
        if !(1..=3).contains(&n) {
            return Err(format!("card {id} span must be 1, 2, or 3"));
        }
    }
    let body = obj.get("body").ok_or_else(|| format!("card {id} is missing body"))?;
    validate_body(&id, &kind, body)?;
    if let Some(adaptive) = obj.get("adaptive") {
        validate_adaptive(&id, adaptive)?;
    }
    Ok(())
}

fn validate_body(id: &str, kind: &str, body: &Value) -> Result<(), String> {
    let obj = body
        .as_object()
        .ok_or_else(|| format!("card {id} body must be an object"))?;
    match kind {
        "EngineStatus" => {
            expect_string(obj.get("engineId"), "engineId", 1, 40)?;
            let status = expect_string(obj.get("status"), "status", 1, 40)?;
            if !matches!(
                status.as_str(),
                "implemented" | "external" | "library" | "weights_absent" | "unavailable"
            ) {
                return Err(format!("card {id} has unknown engine status"));
            }
            expect_string(obj.get("summary"), "summary", 1, 400)?;
        }
        "SpectrogramPanel" | "Cube3D" => {
            expect_const(obj.get("source"), "fixture-tone", "source")?;
            expect_const(obj.get("notPodcast"), true, "notPodcast")?;
            let disclaimer = expect_string(obj.get("disclaimer"), "disclaimer", 12, 400)?;
            if !disclaimer.to_ascii_lowercase().contains("not") {
                return Err(format!("card {id} disclaimer must say the visual is not a podcast render"));
            }
        }
        "PodcastCast" => {
            expect_const(obj.get("sampleScript"), true, "sampleScript")?;
            let speakers = obj
                .get("speakers")
                .and_then(Value::as_array)
                .ok_or("speakers must be an array")?;
            if speakers.is_empty() {
                return Err("speakers must not be empty".into());
            }
            for speaker in speakers {
                let sp = speaker.as_object().ok_or("speaker must be an object")?;
                expect_string(sp.get("id"), "id", 1, 8)?;
                expect_string(sp.get("name"), "name", 1, 40)?;
                expect_string(sp.get("voice"), "voice", 1, 40)?;
            }
        }
        "ServeHealth" => {
            expect_string(obj.get("host"), "host", 1, 80)?;
            let port = obj.get("port").and_then(Value::as_u64).ok_or("port must be an integer")?;
            if !(1..=65535).contains(&port) {
                return Err("port out of range".into());
            }
            expect_string(obj.get("baseUrl"), "baseUrl", 1, 200)?;
            expect_string(obj.get("healthUrl"), "healthUrl", 1, 220)?;
            let probed = obj.get("probed").and_then(Value::as_bool).ok_or("probed must be a boolean")?;
            if probed {
                let ok = obj.get("ok").and_then(Value::as_bool);
                if ok.is_none() {
                    return Err("probed serve cards must include ok".into());
                }
            }
            let role = expect_string(obj.get("role"), "role", 1, 32)?;
            if role != "node" && role != "shared-gateway" {
                return Err("serve role must be node or shared-gateway".into());
            }
        }
        "BenchmarkCompare" => {
            expect_const(obj.get("measuredHere"), false, "measuredHere")?;
            let note = expect_string(obj.get("sourceNote"), "sourceNote", 12, 400)?;
            if !note.to_ascii_lowercase().contains("not remeasured") {
                return Err("benchmark sourceNote must say the figures are not remeasured here".into());
            }
            let rows = obj.get("rows").and_then(Value::as_array).ok_or("rows must be an array")?;
            for row in rows {
                let row = row.as_object().ok_or("benchmark row must be an object")?;
                expect_string(row.get("engine"), "engine", 1, 40)?;
                expect_string(row.get("metric"), "metric", 1, 40)?;
                if !row.get("value").map(|v| v.is_number() || v.is_string()).unwrap_or(false) {
                    return Err("benchmark value must be a number or string".into());
                }
            }
        }
        "ConnectorStatus" => {
            let cid = expect_string(obj.get("connectorId"), "connectorId", 1, 32)?;
            if !CONNECTOR_IDS.contains(&cid.as_str()) {
                return Err(format!("unknown connector {cid}"));
            }
            let mode = expect_string(obj.get("mode"), "mode", 1, 32)?;
            if !matches!(
                mode.as_str(),
                "mock" | "live" | "local" | "token_present" | "misconfigured"
            ) {
                return Err(format!("card {id} has unknown connector mode"));
            }
            if obj.get("authenticated").and_then(Value::as_bool).is_none() {
                return Err("authenticated must be a boolean".into());
            }
            expect_string(obj.get("detail"), "detail", 1, 400)?;
        }
        
        "LibraryClip" => {
            expect_string(obj.get("engineId"), "engineId", 1, 40)?;
            let status = expect_string(obj.get("status"), "status", 1, 40)?;
            if !matches!(
                status.as_str(),
                "ok" | "running" | "weights_absent" | "unavailable" | "external"
            ) {
                return Err(format!("card {id} has unknown library status"));
            }
            let speech = obj
                .get("synthesizedSpeech")
                .and_then(Value::as_bool)
                .ok_or_else(|| format!("card {id} synthesizedSpeech must be a boolean"))?;
            expect_string(obj.get("summary"), "summary", 1, 400)?;
            if speech {
                expect_string(obj.get("wavUrl"), "wavUrl", 1, 260)?;
                let dur = obj
                    .get("duration_s")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| format!("card {id} duration_s must be a number"))?;
                if dur <= 0.0 {
                    return Err(format!("card {id} duration_s must be > 0"));
                }
            } else if obj.get("wavUrl").is_some() {
                return Err(format!("card {id} must not set wavUrl unless synthesizedSpeech is true"));
            }
            optional_string(obj.get("semanticName"), "semanticName", 1, 80)?;
            optional_string(obj.get("faceName"), "faceName", 1, 80)?;
            optional_string(obj.get("sidecarUrl"), "sidecarUrl", 1, 260)?;
            optional_string(obj.get("cubeJsonUrl"), "cubeJsonUrl", 1, 260)?;
        }
        "VoiceProfile" => validate_voice_profile(id, obj)?,
        _ => return Err(format!("unhandled kind {kind}")),
    }
    Ok(())
}

fn validate_voice_profile(id: &str, obj: &serde_json::Map<String, Value>) -> Result<(), String> {
    let agent_name = expect_string(obj.get("agentName"), "agentName", 1, 40)?;
    let persona_id = expect_string(obj.get("personaId"), "personaId", 1, 40)?;
    let person = crate::catalog::persona(&persona_id)
        .ok_or_else(|| format!("card {id} personaId {persona_id} is not a persona"))?;
    if person.name != agent_name {
        return Err(format!("card {id} agentName must match persona {persona_id}"));
    }
    let voice_id = expect_string(obj.get("voiceModel"), "voiceModel", 1, 40)?;
    if crate::catalog::voice_model(&voice_id).is_none() {
        return Err(format!(
            "card {id} voiceModel {voice_id} is not a TTS model. Pack ids such as af_heart belong in refs."
        ));
    }
    expect_string(obj.get("tone"), "tone", 1, 80)?;
    expect_string(obj.get("purpose"), "purpose", 1, 160)?;
    expect_string(obj.get("domain"), "domain", 1, 160)?;
    expect_string(obj.get("accent"), "accent", 1, 80)?;
    expect_string(obj.get("traits"), "traits", 12, 400)?;
    let refs = obj
        .get("refs")
        .and_then(Value::as_array)
        .ok_or("refs must be an array")?;
    if refs.is_empty() || refs.len() > 8 {
        return Err(format!("card {id} refs must contain 1 to 8 strings"));
    }
    for reference in refs {
        expect_string(Some(reference), "refs", 1, 80)?;
    }
    expect_const(obj.get("spectrogram2d"), "none", "spectrogram2d")?;
    let spatial = expect_string(obj.get("spectrogram3d"), "spectrogram3d", 1, 40)?;
    // AP-OPT-1: only what the product renders; nothing renders a persona cube.
    if !matches!(spatial.as_str(), "none" | "fixture-cube") {
        return Err(format!("card {id} spectrogram3d is not a known hook"));
    }
    expect_const(obj.get("notPodcast"), true, "notPodcast")?;
    if let Some(speech) = obj.get("synthesizedSpeech") {
        expect_const(Some(speech), false, "synthesizedSpeech")?;
    }
    if obj.get("wavUrl").is_some() {
        return Err(format!("card {id} must not claim a WAV on a voice profile"));
    }
    let disclaimer = expect_string(obj.get("disclaimer"), "disclaimer", 12, 400)?;
    if !disclaimer.to_ascii_lowercase().contains("not") {
        return Err(format!("card {id} disclaimer must say the profile is not a podcast render"));
    }
    if obj.get("cubeJsonUrl").is_some() {
        return Err(format!("card {id} must not link a cube on a voice profile"));
    }
    Ok(())
}

fn optional_string(value: Option<&Value>, field: &str, min: usize, max: usize) -> Result<(), String> {
    if value.is_some() {
        expect_string(value, field, min, max)?;
    }
    Ok(())
}

fn validate_adaptive(id: &str, adaptive: &Value) -> Result<(), String> {
    let obj = adaptive
        .as_object()
        .ok_or_else(|| format!("card {id} adaptive must be an object"))?;
    expect_const(obj.get("type"), "AdaptiveCard", "adaptive.type")?;
    expect_string(obj.get("version"), "version", 1, 8)?;
    let body = obj
        .get("body")
        .and_then(Value::as_array)
        .ok_or("adaptive body must be an array")?;
    if body.is_empty() {
        return Err(format!("card {id} adaptive body is empty"));
    }
    Ok(())
}

fn expect_const(value: Option<&Value>, expected: impl Into<Value>, field: &str) -> Result<(), String> {
    let expected = expected.into();
    match value {
        Some(found) if found == &expected => Ok(()),
        _ => Err(format!("field {field} must be {expected}")),
    }
}

fn expect_string(value: Option<&Value>, field: &str, min: usize, max: usize) -> Result<String, String> {
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("field {field} must be a string"))?;
    if text.chars().count() < min || text.chars().count() > max {
        return Err(format!("field {field} length is out of range"));
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_viewport_matches_the_checker() {
        let raw = include_str!("../../../schemas/examples/viewport.example.json");
        let document: Value = serde_json::from_str(raw).unwrap();
        validate_viewport(&document).unwrap();
    }

    #[test]
    fn release_viewport_matches_the_checker() {
        let raw = include_str!("../../../schemas/examples/viewport.release.json");
        let document: Value = serde_json::from_str(raw).unwrap();
        validate_viewport(&document).unwrap();
        assert!(document["cards"].as_array().unwrap().iter().all(|card| !matches!(card["id"].as_str(), Some("spec-fixture" | "cube-fixture" | "bench-ref" | "cast-sample" | "serve-node" | "serve-gateway"))));
    }

    /// AP-OPT-1 parity with validate.ts: nothing renders a persona cube, so
    /// spectrogram3d "library-cube-hook" (with or without cubeJsonUrl) is refused.
    #[test]
    fn voice_profile_refuses_the_library_cube_hook() {
        let raw = include_str!("../../../schemas/examples/voice_profile.optimus.json");
        let value: Value = serde_json::from_str(raw).unwrap();
        for with_url in [true, false] {
            let mut hooked = value.as_object().unwrap().clone();
            hooked.insert("spectrogram3d".into(), Value::from("library-cube-hook"));
            if with_url {
                hooked.insert("cubeJsonUrl".into(), Value::from("/library/library_kokoro_cube3d.json"));
            }
            let error = validate_voice_profile("profile-optimus", &hooked).expect_err("library-cube-hook accepted");
            assert!(error.contains("spectrogram3d"), "{error}");
        }
    }

    #[test]
    fn voice_profile_example_matches_the_checker() {
        let raw = include_str!("../../../schemas/examples/voice_profile.alice.json");
        let value: Value = serde_json::from_str(raw).unwrap();
        let obj = value.as_object().expect("profile object");
        validate_voice_profile("profile-alice", obj).unwrap();
        assert_eq!(value["agentName"], "Alice");
        assert_eq!(value["voiceModel"], "kokoro_onnx");
        assert_ne!(value["agentName"], "af_heart");
        assert!(value["refs"].to_string().contains("af_heart"));
        assert_eq!(value["notPodcast"], true);
    }

    #[test]
    fn optimus_profile_is_a_persona_and_not_a_wav_claim() {
        let raw = include_str!("../../../schemas/examples/voice_profile.optimus.json");
        let value: Value = serde_json::from_str(raw).unwrap();
        let obj = value.as_object().expect("profile object");
        validate_voice_profile("profile-optimus", obj).unwrap();
        assert_eq!(value["personaId"], "optimus");
        assert_eq!(value["agentName"], "Optimus Timelarp");
        assert_eq!(value["notPodcast"], true);
        assert_eq!(value["synthesizedSpeech"], false);
        assert!(value.get("wavUrl").is_none());
        let public = include_str!("../../../apps/desktop/public/library/voice_profile.optimus.json");
        assert_eq!(public, raw);
    }

    #[test]
    fn voice_profile_rejects_a_pack_id_as_the_voice_model() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../schemas/examples/viewport.example.json")).unwrap();
        let card = document["cards"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|card| card["kind"] == "VoiceProfile")
            .unwrap();
        card["body"]["voiceModel"] = Value::String("af_heart".into());
        let err = validate_viewport(&document).unwrap_err();
        assert!(err.contains("af_heart") || err.contains("TTS"), "{err}");
    }

    #[test]
    fn benchmark_cannot_claim_it_was_measured_here() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../schemas/examples/viewport.example.json")).unwrap();
        document["cards"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter(|card| card["kind"] == "BenchmarkCompare")
            .for_each(|card| card["body"]["measuredHere"] = Value::Bool(true));
        assert!(validate_viewport(&document).is_err());
    }

    #[test]
    fn card_uid_must_be_a_card_kind_uid() {
        let mut doc: Value =
            serde_json::from_str(include_str!("../../../schemas/examples/viewport.example.json")).unwrap();
        assert!(validate_viewport(&doc).is_ok());
        doc["cards"][0]["uid"] = Value::String("ga:audio_clip:vtwxksrsuci7zygslimzfy7kdy".into());
        assert!(validate_viewport(&doc).is_err());
        doc["cards"][0]["uid"] = Value::String("ga:card:not-base32".into());
        assert!(validate_viewport(&doc).is_err());
    }
}
