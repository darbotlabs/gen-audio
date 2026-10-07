//! Plans a script and records a JSONL trace.
//!
//! Mock mode never calls kokoro-onnx. A fixture tone can be written and, when
//! the Python checkout is available, passed to the improve CLI.

use gen_audio_connectors::health_all;
use gen_audio_core::fixture::write_fixture_tone;
use gen_audio_core::paths::{self, find_repo_root};
use gen_audio_core::script::{self, Turn};
use gen_audio_mcp::Server;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct HarnessOptions {
    pub script_text: String,
    pub cast_engine: String,
    pub write_fixture: bool,
    pub run_improve: bool,
}

pub fn run(options: &HarnessOptions) -> Result<Vec<Value>, String> {
    let mut trace = Vec::new();
    trace.push(event("harness_start", json!({"writeFixture": options.write_fixture})));
    let turns = script::parse_script(&options.script_text)?;
    trace.push(event(
        "script_loaded",
        json!({
            "turns": turns.len(),
            "speakers": speaker_ids(&turns)
        }),
    ));
    trace.push(event(
        "cast_resolved",
        json!({
            "engine": options.cast_engine,
            "note": "Voice ids for kokoro-onnx come from the cast map. Other engines are not synthesized by this harness."
        }),
    ));
    trace.push(event(
        "tool_plan",
        json!({
            "tool": "synth",
            "status": "skipped",
            "reason": "weights_absent_or_mock",
            "synthesized": false
        }),
    ));
    if options.write_fixture {
        let server = Server::boot();
        let tone = write_fixture_tone(&server.work)?;
        trace.push(event(
            "fixture_tone",
            json!({
                "wav": "fixture-tone.wav",
                "kind": "fixture-tone",
                "notPodcast": true,
                "samples": tone.samples
            }),
        ));
        if options.run_improve {
            let repo = find_repo_root();
            if repo.is_none() {
                trace.push(event(
                    "tool_result",
                    json!({"tool": "improve", "status": "skipped", "reason": "repo_not_found"}),
                ));
            } else {
                let planned = gen_audio_core::bridge::plan(
                    gen_audio_core::bridge::PythonTool::Improve,
                    repo.as_ref().unwrap(),
                    &server.work,
                    &json!({"input": "fixture-tone.wav", "output": "fixture-24k.wav"}),
                );
                match planned {
                    Ok(plan) => {
                        let ran = gen_audio_core::bridge::run_plan(&plan)?;
                        let status = if ran["ok"].as_bool() == Some(true) { "ok" } else { "failed" };
                        trace.push(event(
                            "tool_result",
                            json!({"tool": "improve", "status": status, "fixture": true, "detail": ran}),
                        ));
                    }
                    Err(err) => {
                        trace.push(event(
                            "tool_result",
                            json!({"tool": "improve", "status": "skipped", "reason": err}),
                        ));
                    }
                }
            }
        }
        trace.push(event(
            "work_dir",
            json!({"path": server.work, "kept": true}),
        ));
    }
    for report in health_all() {
        trace.push(event(
            "connector_health",
            json!({
                "id": report.connector_id,
                "mode": report.mode,
                "authenticated": report.authenticated
            }),
        ));
    }
    trace.push(event("harness_end", json!({"events": trace.len() + 1})));
    Ok(trace)
}

pub fn trace_to_jsonl(trace: &[Value]) -> String {
    trace
        .iter()
        .map(|event| event.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn event(name: &str, fields: Value) -> Value {
    let mut obj = fields.as_object().cloned().unwrap_or_default();
    obj.insert("event".into(), Value::String(name.into()));
    Value::Object(obj)
}

fn speaker_ids(turns: &[Turn]) -> Vec<String> {
    let mut ids = Vec::new();
    for turn in turns {
        if !ids.contains(&turn.speaker_id) {
            ids.push(turn.speaker_id.clone());
        }
    }
    ids
}

pub fn load_repo_script() -> Result<String, String> {
    let repo = paths::find_repo_root().ok_or("repository root not found")?;
    let path = paths::read_repo_relative(&repo, "examples/podcast_script_sample.txt")?;
    std::fs::read_to_string(path).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_skips_synth_and_records_connectors() {
        let trace = run(&HarnessOptions {
            script_text: "Speaker 1: Hello from a sample.\nSpeaker 2: Noted.\n".into(),
            cast_engine: "kokoro_onnx".into(),
            write_fixture: true,
            run_improve: false,
        })
        .unwrap();
        let names: Vec<_> = trace.iter().filter_map(|row| row["event"].as_str()).collect();
        assert!(names.contains(&"tool_plan"));
        assert!(names.contains(&"fixture_tone"));
        assert!(names.contains(&"connector_health"));
        assert!(names.contains(&"harness_end"));
        let plan = trace.iter().find(|row| row["event"] == "tool_plan").unwrap();
        assert_eq!(plan["synthesized"], false);
        let jsonl = trace_to_jsonl(&trace);
        assert!(jsonl.lines().count() >= 7);
    }
}
