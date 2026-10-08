use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gen_audio_mcp::http;
use serde::Serialize;
#[cfg(debug_assertions)]
use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Hide console for console-subsystem children (MCP sidecar is CUI).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

/// Dev/test-only commands (E1 addendum): they serve the labeled fixture deck
/// and run Python improve on the fixture tone. Compiled, and so registered,
/// only under debug_assertions; the release exe that `tauri build` makes has
/// neither (see `invoke_handler`).
pub const DEV_ONLY_COMMANDS: [&str; 2] = ["viewport_example", "run_fixture_improve"];

#[cfg(debug_assertions)]
#[tauri::command]
fn viewport_example() -> Value {
    serde_json::from_str(include_str!("../../../../schemas/examples/viewport.example.json"))
        .expect("example viewport is valid json")
}

#[cfg(debug_assertions)]
#[tauri::command]
fn run_fixture_improve() -> Result<Value, String> {
    use gen_audio_core::bridge::{self, PythonTool};
    use gen_audio_core::fixture::write_fixture_tone;
    use serde_json::json;
    let server = gen_audio_mcp::Server::boot();
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

fn spawn_hidden(mut cmd: Command) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
}

fn boot_mcp() -> (McpRuntime, Option<Child>) {
    let addr = preferred_addr();
    if http::initialize_handshake(&addr).is_ok() {
        let _ = gen_audio_core::paths::write_mcp_addr(&addr);
        return (
            runtime(addr, true, "initialize ok (already listening)", "existing"),
            None,
        );
    }
    if let Some(bin) = sidecar_binary() {
        let mut cmd = Command::new(&bin);
        cmd.args(["--http", &addr]);
        match spawn_hidden(cmd) {
            Ok(child) => {
                if wait_for_handshake(&addr) {
                    let _ = gen_audio_core::paths::write_mcp_addr(&addr);
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
    match bind_loopback_range(&addr) {
        Ok(bound) => {
            let text = bound.to_string();
            let _ = gen_audio_core::paths::write_mcp_addr(&text);
            let detail = if text == addr {
                "initialize ok (in-process)".to_string()
            } else {
                format!("{addr} was busy; bound {text}. Clients must use this address.")
            };
            match http::initialize_handshake(&text) {
                Ok(_) => (
                    runtime(text, true, detail, "in-process"),
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
    let mut cmd = Command::new("reg");
    cmd.args([
        "add",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
        "/v",
        "DarbotGenAudio",
        "/t",
        "REG_SZ",
        "/d",
        &value,
        "/f",
    ]);
    let _ = spawn_hidden(cmd).and_then(|mut child| child.wait());
}

#[cfg(not(windows))]
fn register_login_autostart() {}

fn stop_sidecar(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}


fn bind_loopback_range(preferred: &str) -> std::io::Result<std::net::SocketAddr> {
    let mut candidates = Vec::new();
    if !preferred.is_empty() {
        candidates.push(preferred.to_string());
    }
    for port in 8765u16..=8770 {
        let addr = format!("127.0.0.1:{port}");
        if !candidates.iter().any(|item| item == &addr) {
            candidates.push(addr);
        }
    }
    let mut last = std::io::Error::other("no loopback port in 8765-8770 was free");
    for addr in candidates {
        match http::spawn_loopback(&addr) {
            Ok(bound) => return Ok(bound),
            Err(err) => last = err,
        }
    }
    Err(last)
}

fn request_quit(app: &tauri::AppHandle) {
    gen_audio_core::paths::delete_mcp_addr();
    if let Some(state) = app.try_state::<SidecarChild>() {
        if let Ok(mut guard) = state.0.lock() {
            if let Some(child) = guard.as_mut() {
                stop_sidecar(child);
            }
        }
    }
    app.exit(0);
}

fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The IPC commands this build registers. Debug builds add DEV_ONLY_COMMANDS;
/// release builds cannot, because those functions are not compiled there.
#[cfg(debug_assertions)]
fn invoke_handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        connector_statuses,
        viewport_example,
        run_fixture_improve,
        mcp_status
    ]
}

#[cfg(not(debug_assertions))]
fn invoke_handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![connector_statuses, mcp_status]
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Placeholder until setup boots MCP (second instance exits in the plugin
    // before setup, so it never starts a second sidecar or tray).
    let pending = runtime(
        preferred_addr(),
        false,
        "starting",
        "starting",
    );

    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        // Must be first so a second launch focuses the existing window and exits
        // before other plugins / setup can open another tray.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // Second launch with --quit invokes the same path as tray Quit (sidecar stop + exit).
            if args.iter().any(|a| a == "--quit") {
                request_quit(app);
            } else {
                focus_main_window(app);
            }
        }));
    }

    let app = builder
        .manage(Mutex::new(pending))
        .manage(SidecarChild(Mutex::new(None)))
        .invoke_handler(invoke_handler())
        .setup(move |app| {
            let (mcp, child) = boot_mcp();
            let tooltip = format!("Gen-Audio MCP {} ({})", mcp.addr, mcp.mode);
            if let Ok(mut guard) = app.state::<Mutex<McpRuntime>>().lock() {
                *guard = mcp;
            }
            if let Ok(mut guard) = app.state::<SidecarChild>().0.lock() {
                *guard = child;
            }

            let show = MenuItem::with_id(app, "show", "Show Gen-Audio", true, None::<&str>)?;
            let status = MenuItem::with_id(app, "mcp", tooltip.clone(), false, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &status, &quit])?;
            let icon = tray_rgba();
            let _ = TrayIconBuilder::with_id("gen-audio")
                .tooltip(tooltip)
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => focus_main_window(app),
                    "quit" => request_quit(app),
                    _ => {}
                })
                .build(app);
            
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Product: X hides to tray; Quit is tray-only. Do this even if tray
                // build failed so a second launch can still focus/show via single-instance.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running Gen-Audio");

    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            gen_audio_core::paths::delete_mcp_addr();
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

fn tray_rgba() -> tauri::image::Image<'static> {
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for pixel in rgba.chunks_mut(4) {
        pixel.copy_from_slice(&[24, 168, 154, 255]);
    }
    tauri::image::Image::new_owned(rgba, 32, 32)
}
