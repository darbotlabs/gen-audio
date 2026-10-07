use std::sync::Mutex;

use gen_audio_core::bridge::{self, PythonTool};
use gen_audio_core::fixture::write_fixture_tone;
use gen_audio_mcp::http;
use gen_audio_mcp::Server;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};

#[derive(Clone, Serialize)]
struct McpRuntime {
    addr: String,
    handshake_ok: bool,
    detail: String,
}

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
    write_fixture_tone(&server.scratch)?;
    let repo = server.repo.ok_or("repository root not found")?;
    let plan = bridge::plan(
        PythonTool::Improve,
        &repo,
        &server.scratch,
        &json!({"input": "fixture-tone.wav", "output": "fixture-24k.wav"}),
    )?;
    let ran = bridge::run_plan(&plan)?;
    Ok(json!({
        "ok": ran.get("ok").cloned().unwrap_or(Value::Bool(false)),
        "code": ran.get("code").cloned().unwrap_or(Value::Null),
        "synthesizedSpeech": false,
        "fixture": true
    }))
}

#[tauri::command]
fn mcp_status(state: tauri::State<'_, Mutex<McpRuntime>>) -> Result<McpRuntime, String> {
    state.lock().map(|guard| guard.clone()).map_err(|_| "mcp status lock".into())
}

fn boot_mcp() -> McpRuntime {
    let preferred = std::env::var("GEN_AUDIO_MCP_ADDR").unwrap_or_else(|_| "127.0.0.1:8765".into());
    let bound = http::spawn_loopback(&preferred).or_else(|_| http::spawn_loopback("127.0.0.1:0"));
    match bound {
        Ok(addr) => {
            let text = addr.to_string();
            match http::initialize_handshake(&text) {
                Ok(_) => McpRuntime {
                    addr: text,
                    handshake_ok: true,
                    detail: "initialize ok".into(),
                },
                Err(err) => McpRuntime {
                    addr: text,
                    handshake_ok: false,
                    detail: err,
                },
            }
        }
        Err(err) => McpRuntime {
            addr: preferred,
            handshake_ok: false,
            detail: err.to_string(),
        },
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mcp = boot_mcp();
    let tooltip = format!("Gen-Audio MCP {}", mcp.addr);
    tauri::Builder::default()
        .manage(Mutex::new(mcp))
        .invoke_handler(tauri::generate_handler![
            connector_statuses,
            viewport_example,
            run_fixture_improve,
            mcp_status
        ])
        .setup(move |app| {
            let show = MenuItem::with_id(app, "show", "Show Gen-Audio", true, None::<&str>)?;
            let status = MenuItem::with_id(app, "mcp", tooltip.clone(), false, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &status, &quit])?;
            let icon = tray_rgba();
            let tray_ok = TrayIconBuilder::with_id("gen-audio")
                .tooltip(tooltip)
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)
                .is_ok();
            app.manage(TrayArmed(tray_ok));
            Ok(())
        })
        .on_window_event(|window, event| {
            let armed = window
                .app_handle()
                .try_state::<TrayArmed>()
                .map(|flag| flag.0)
                .unwrap_or(false);
            if armed {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Gen-Audio");
}

struct TrayArmed(bool);

fn tray_rgba() -> tauri::image::Image<'static> {
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for pixel in rgba.chunks_mut(4) {
        pixel.copy_from_slice(&[24, 168, 154, 255]);
    }
    tauri::image::Image::new_owned(rgba, 32, 32)
}
