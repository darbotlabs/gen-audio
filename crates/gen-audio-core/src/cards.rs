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
];

pub const CONNECTOR_IDS: &[&str] = &[
    "mcp",
    "acp",
    "harness",
    "copilot",
    "claude",
    "gpt",
    "gemini",
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
        _ => return Err(format!("unhandled kind {kind}")),
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
}
