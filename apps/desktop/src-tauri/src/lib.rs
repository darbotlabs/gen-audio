use gen_audio_core::bridge::{self, PythonTool};
use gen_audio_core::fixture::write_fixture_tone;
use gen_audio_mcp::Server;
use serde_json::{json, Value};

#[tauri::command]
fn connector_statuses() -> Vec<gen_audio_connectors::HealthReport> {
    gen_audio_connectors::health_all()
}

#[tauri::command]
fn viewport_example() -> Value {
    serde_json::from_str(include_str!("../../../../schemas/examples/viewport.example.json"))
        .expect("example viewport is valid json")
}

#[tauri::command]
fn run_fixture_improve() -> Result<Value, String> {
    let server = Server::boot();
    write_fixture_tone(&server.work)?;
    let repo = server.repo.ok_or("repository root not found")?;
    let plan = bridge::plan(
        PythonTool::Improve,
        &repo,
        &server.work,
        &json!({"input": "fixture-tone.wav", "output": "fixture-24k.wav"}),
    )?;
    bridge::run_plan(&plan)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            connector_statuses,
            viewport_example,
            run_fixture_improve
        ])
        .run(tauri::generate_context!())
        .expect("error while running Gen-Audio");
}
