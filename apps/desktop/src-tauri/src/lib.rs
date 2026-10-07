use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
    mode: String,
}

struct SidecarChild(Mutex<Option<Child>>);

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
    state
        .lock()
        .map(|guard| guard.clone())
        .map_err(|_| "mcp status lock".into())
}

fn preferred_addr() -> String {
    std::env::var("GEN_AUDIO_MCP_ADDR").unwrap_or_else(|_| "127.0.0.1:8765".into())
}

fn sidecar_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    ["gen-audio-mcp.exe", "gen-audio-mcp"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

fn runtime(addr: String, handshake_ok: bool, detail: impl Into<String>, mode: &str) -> McpRuntime {
    McpRuntime {
        addr,
        handshake_ok,
        detail: detail.into(),
        mode: mode.into(),
    }
}

fn wait_for_handshake(addr: &str) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(8) {
        if http::initialize_handshake(addr).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn boot_mcp() -> (McpRuntime, Option<Child>) {
    let addr = preferred_addr();
    if http::initialize_handshake(&addr).is_ok() {
        return (
            runtime(addr, true, "initialize ok (already listening)", "existing"),
            None,
        );
    }
    if let Some(bin) = sidecar_binary() {
        match Command::new(&bin)
            .args(["--http", &addr])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                if wait_for_handshake(&addr) {
                    register_login_autostart();
                    return (
                        runtime(addr, true, "initialize ok (sidecar)", "sidecar"),
                        Some(child),
                    );
                }
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(_) => {}
        }
    }
    match http::spawn_loopback(&addr).or_else(|_| http::spawn_loopback("127.0.0.1:0")) {
        Ok(bound) => {
            let text = bound.to_string();
            match http::initialize_handshake(&text) {
                Ok(_) => (
                    runtime(text, true, "initialize ok (in-process)", "in-process"),
                    None,
                ),
                Err(err) => (runtime(text, false, err, "in-process"), None),
            }
        }
        Err(err) => (runtime(addr, false, err.to_string(), "failed"), None),
    }
}

#[cfg(windows)]
fn register_login_autostart() {
    if std::env::var("GEN_AUDIO_AUTOSTART").ok().as_deref() == Some("0") {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let value = format!("\"{}\"", exe.display());
    let _ = Command::new("reg")
        .args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "DarbotGenAudio",
            "/t",
            "REG_SZ",
            "/d",
            &value,
            "/f",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn register_login_autostart() {}

fn stop_sidecar(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (mcp, child) = boot_mcp();
    let tooltip = format!("Gen-Audio MCP {} ({})", mcp.addr, mcp.mode);
    let app = tauri::Builder::default()
        .manage(Mutex::new(mcp))
        .manage(SidecarChild(Mutex::new(child)))
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
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        if let Some(state) = app.try_state::<SidecarChild>() {
                            if let Ok(mut guard) = state.0.lock() {
                                if let Some(child) = guard.as_mut() {
                                    stop_sidecar(child);
                                }
                            }
                        }
                        app.exit(0);
                    }
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
        .build(tauri::generate_context!())
        .expect("error while running Gen-Audio");

    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = app.try_state::<SidecarChild>() {
                if let Ok(mut guard) = state.0.lock() {
                    if let Some(child) = guard.as_mut() {
                        stop_sidecar(child);
                    }
                }
            }
        }
    });
}

struct TrayArmed(bool);

fn tray_rgba() -> tauri::image::Image<'static> {
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for pixel in rgba.chunks_mut(4) {
        pixel.copy_from_slice(&[24, 168, 154, 255]);
    }
    tauri::image::Image::new_owned(rgba, 32, 32)
}
