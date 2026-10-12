use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gen_audio_mcp::http;

pub mod sidecar_log;
use serde::Serialize;
use serde_json::json;
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
    token: Option<String>,
    handshake_ok: bool,
    detail: String,
    mode: String,
}

struct SidecarChild(Mutex<Option<Child>>);
struct ExportedConnections(Mutex<Vec<PathBuf>>);

// Explicit local MCP client bootstrap. Credentials are never printed or put
// in URLs. The caller chooses a new absolute file; Windows grants only the
// current user's SID before any secret is written.
fn export_connection(app:&tauri::AppHandle,args:&[String])->Result<(),String> {
    let Some(index)=args.iter().position(|arg|arg=="--mcp-connection-file") else {return Ok(());};
    let path=PathBuf::from(args.get(index+1).ok_or("missing connection file")?);
    if !path.is_absolute() || path.components().any(|c|matches!(c,std::path::Component::ParentDir)) {return Err("connection file must be absolute without traversal".into());}
    let state=app.state::<Mutex<McpRuntime>>();let runtime=state.lock().map_err(|_|"MCP state lock")?.clone();
    if !runtime.handshake_ok || runtime.token.is_none() {return Err("owned MCP listener is unavailable".into());}
    let mut options=std::fs::OpenOptions::new();options.write(true).create_new(true);
    #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
    let mut file=options.open(&path).map_err(|e|e.to_string())?;
    #[cfg(windows)] {
        let system=PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(||"C:\\Windows".into())).join("System32");
        let mut who=Command::new(system.join("whoami.exe"));who.creation_flags(CREATE_NO_WINDOW);
        let output=who.args(["/user","/fo","csv","/nh"]).output().map_err(|e|e.to_string())?;
        let text=String::from_utf8_lossy(&output.stdout);
        let sid=text.trim().rsplit(',').next().unwrap_or("").trim_matches('"');
        if !sid.starts_with("S-1-") || !sid.bytes().all(|b|b.is_ascii_digit()||b==b'-'||b==b'S') {return Err("cannot determine credential-file owner SID".into());}
        let mut acl=Command::new(system.join("icacls.exe"));acl.creation_flags(CREATE_NO_WINDOW);
        let status=acl.arg(&path).args(["/inheritance:r","/grant:r",&format!("*{sid}:(F)")]).stdout(Stdio::null()).stderr(Stdio::null()).status().map_err(|e|e.to_string())?;
        if !status.success() {return Err("cannot restrict credential-file permissions".into());}
    }
    let mut document=serde_json::to_value(runtime).map_err(|e|e.to_string())?;document["ownerPid"]=json!(std::process::id());
    use std::io::Write;
    file.write_all(&serde_json::to_vec(&document).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    file.sync_all().map_err(|e|e.to_string())?;
    app.state::<ExportedConnections>().0.lock().map_err(|_|"connection export lock")?.push(path);
    Ok(())
}

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
fn viewport_example() -> Result<Value, String> {
    let document = serde_json::from_str(include_str!(
        "../../../../schemas/examples/viewport.example.json"
    ))
    .map_err(|err| format!("viewport JSON: {err}"))?;
    gen_audio_core::cards::validate_viewport(&document)?;
    Ok(document)
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
    #[cfg(test)]
    if std::env::var("GEN_AUDIO_DESKTOP_BOOT_MCP").ok().as_deref() == Some("1") {
        return std::env::var_os("GEN_AUDIO_DESKTOP_TEST_SIDECAR").map(PathBuf::from);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    ["gen-audio-mcp.exe", "gen-audio-mcp"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

fn runtime(
    addr: String,
    token: Option<String>,
    handshake_ok: bool,
    detail: impl Into<String>,
    mode: &str,
) -> McpRuntime {
    McpRuntime {
        addr,
        token,
        handshake_ok,
        detail: detail.into(),
        mode: mode.into(),
    }
}

fn wait_for_handshake(addr: &str, token: &str, boot_nonce: &str, child: &mut Child) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(8) {
        if !matches!(child.try_wait(), Ok(None)) {
            return false;
        }
        if http::initialize_owned_handshake(addr, token, boot_nonce).is_ok() {
            return matches!(child.try_wait(), Ok(None));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

/// Hidden helper process with every stdio stream closed (reg add).
#[cfg(windows)]
fn spawn_hidden(mut cmd: Command) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    hide_console(&mut cmd);
    cmd.spawn()
}

/// The MCP sidecar, hidden, with its stderr (one line per rejected request,
/// startup errors) drained into the rotating gen-audio-mcp.log in the app log
/// dir instead of thrown away (see sidecar_log).
fn spawn_sidecar(mut cmd: Command, log_dir: Option<PathBuf>) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn()?;
    if let Some(stderr) = child.stderr.take() {
        sidecar_log::spawn_pump(stderr, log_dir);
    }
    Ok(child)
}

fn boot_mcp(log_dir: Option<PathBuf>) -> (McpRuntime, Option<Child>) {
    gen_audio_core::paths::reap_stale_mcp_addr();
    let addr = preferred_addr();
    if let Err(err) = http::ensure_bind_allowed(&addr) {
        return (runtime(addr, None, false, err.to_string(), "failed"), None);
    }
    let token = match http::generate_token() {
        Ok(token) => token,
        Err(err) => return (runtime(addr, None, false, err.to_string(), "failed"), None),
    };
    let boot_nonce = match http::generate_token() {
        Ok(nonce) => nonce,
        Err(err) => return (runtime(addr, None, false, err.to_string(), "failed"), None),
    };
    // Never probe or reuse an existing service. Reserve a free port before launching
    // our sidecar; a failed child is reaped before binding our in-process fallback.
    if let (Some(bin), Ok(reservation)) = (sidecar_binary(), std::net::TcpListener::bind(&addr)) {
        let sidecar_addr = reservation
            .local_addr()
            .expect("bound listener address")
            .to_string();
        let mut cmd = Command::new(&bin);
        cmd.args(["--http", &sidecar_addr])
            .env("GEN_AUDIO_MCP_TOKEN", &token)
            .env("GEN_AUDIO_MCP_BOOT_NONCE", &boot_nonce);
        drop(reservation);
        match spawn_sidecar(cmd, log_dir) {
            Ok(mut child) => {
                if wait_for_handshake(&sidecar_addr, &token, &boot_nonce, &mut child) {
                    let _ = gen_audio_core::paths::publish_mcp_addr(&sidecar_addr);
                    return (
                        runtime(
                            sidecar_addr,
                            Some(token),
                            true,
                            "authenticated initialize ok (owned sidecar)",
                            "sidecar",
                        ),
                        Some(child),
                    );
                }
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(err) => {
                // The in-process listener still starts below. The sidecar
                // error has to be visible or a missing binary looks like a
                // successful boot.
                eprintln!("gen-audio: MCP sidecar failed to start: {err}");
            }
        }
    }
    // A port-racing foreign service might have seen the failed handshake's bearer.
    // The fallback owns a new credential as well as a new listener.
    let token = match http::generate_token() {
        Ok(token) => token,
        Err(err) => return (runtime(addr, None, false, err.to_string(), "failed"), None),
    };
    match bind_loopback_range(&addr, &token) {
        Ok(bound) => {
            let text = bound.to_string();
            let _ = gen_audio_core::paths::publish_mcp_addr(&text);
            match http::initialize_handshake(&text, &token) {
                Ok(_) => (
                    runtime(
                        text,
                        Some(token),
                        true,
                        "authenticated initialize ok (owned listener)",
                        "in-process",
                    ),
                    None,
                ),
                Err(err) => (runtime(text, None, false, err, "in-process"), None),
            }
        }
        Err(err) => (runtime(addr, None, false, err.to_string(), "failed"), None),
    }
}

#[cfg(windows)]
fn register_login_autostart() {
    if !autostart_enabled(std::env::var("GEN_AUDIO_AUTOSTART").ok().as_deref()) {
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

#[cfg(any(windows, test))]
fn autostart_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

fn stop_sidecar(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn bind_loopback_range(preferred: &str, token: &str) -> std::io::Result<std::net::SocketAddr> {
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
        match http::spawn_loopback(&addr, token) {
            Ok(bound) => return Ok(bound),
            Err(err) => last = err,
        }
    }
    http::spawn_loopback("127.0.0.1:0", token).map_err(|_| last)
}

fn cleanup_runtime(app: &tauri::AppHandle) {
    gen_audio_core::paths::delete_mcp_addr();
    if let Some(state)=app.try_state::<ExportedConnections>() {
        if let Ok(paths)=state.0.lock() {for path in paths.iter() {let _=std::fs::remove_file(path);}}
    }
    if let Some(state) = app.try_state::<SidecarChild>() {
        if let Ok(mut guard) = state.0.lock() {
            if let Some(child) = guard.as_mut() {
                stop_sidecar(child);
            }
        }
    }
}

fn request_quit(app: &tauri::AppHandle) {
    cleanup_runtime(app);
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
    let pending = runtime(preferred_addr(), None, false, "starting", "starting");

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
                let _=export_connection(app,&args);
                focus_main_window(app);
            }
        }));
    }

    let app = builder
        .manage(Mutex::new(pending))
        .manage(SidecarChild(Mutex::new(None)))
        .manage(ExportedConnections(Mutex::new(Vec::new())))
        .invoke_handler(invoke_handler())
        .setup(move |app| {
            // Release SDK code belongs to the installed package. Model weights
            // and Python environments remain explicit local runtime settings.
            #[cfg(not(debug_assertions))]
            if std::env::var_os("GEN_AUDIO_REPO").is_none() {
                let root = app.path().resource_dir()?.join("runtime");
                if !root.join("pyproject.toml").is_file()
                    || !root.join("src/gen_audio/runtime_owner.py").is_file() {
                    return Err(std::io::Error::other("Installed runtime payload is missing").into());
                }
                std::env::set_var("GEN_AUDIO_REPO", root);
            }
            #[cfg(debug_assertions)]
            viewport_example().map_err(std::io::Error::other)?;
            let (mcp, child) = boot_mcp(app.path().app_log_dir().ok());
            if mcp.handshake_ok {
                register_login_autostart();
            }
            let tooltip = format!("Gen-Audio MCP {} ({})", mcp.addr, mcp.mode);
            if let Ok(mut guard) = app.state::<Mutex<McpRuntime>>().lock() {
                *guard = mcp;
            }
            if let Ok(mut guard) = app.state::<SidecarChild>().0.lock() {
                *guard = child;
            }
            let _=export_connection(app.handle(),&std::env::args().collect::<Vec<_>>());

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
            cleanup_runtime(app);
        }
    });
}

#[cfg(test)]
mod sidecar_boot {
    use super::boot_mcp;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    const TEST_NAME: &str = "sidecar_boot::sidecar_spawn_failure_is_logged_and_boot_continues";

    fn fake_sidecar_path() -> PathBuf {
        PathBuf::from(std::env::var_os("GEN_AUDIO_DESKTOP_TEST_SIDECAR").expect("test sidecar"))
    }

    #[test]
    fn sidecar_spawn_failure_is_logged_and_boot_continues() {
        if std::env::var("GEN_AUDIO_DESKTOP_BOOT_MCP").ok().as_deref() == Some("1") {
            let fake = fake_sidecar_path();
            let mut fake_file = fs::OpenOptions::new().write(true).create_new(true).open(&fake).expect("new fake sidecar");
            std::io::Write::write_all(&mut fake_file, b"not an executable sidecar").expect("write fake sidecar");
            drop(fake_file);
            let (runtime, child) = boot_mcp(None);
            let _ = fs::remove_file(&fake);
            println!("mode={}", runtime.mode);
            println!("handshake={}", runtime.handshake_ok);
            println!("child={}", child.is_some());
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let ok = runtime.mode == "in-process" && child.is_none();
            std::process::exit(if ok { 0 } else { 3 });
        }

        let work = std::env::temp_dir().join(format!("gen-audio-desktop-boot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).expect("work dir");
        let addr_file = work.join("mcp.addr");
        let output = Command::new(std::env::current_exe().expect("current_exe"))
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env("GEN_AUDIO_DESKTOP_BOOT_MCP", "1")
            .env("GEN_AUDIO_DESKTOP_TEST_SIDECAR", work.join("invalid-sidecar.exe"))
            .env("GEN_AUDIO_MCP_ADDR", "127.0.0.1:0")
            .env("GEN_AUDIO_WORK_DIR", &work)
            .env("GEN_AUDIO_MCP_ADDR_FILE", &addr_file)
            .output()
            .expect("re-exec the desktop test harness");
        let _ = fs::remove_dir_all(&work);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "child status {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        );
        assert!(
            stderr.contains("gen-audio: MCP sidecar failed to start:"),
            "spawn failure was not logged\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("mode=in-process"),
            "boot did not continue in-process\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("handshake=true"),
            "in-process listener did not handshake\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("child=false"),
            "a failed spawn still returned a child\nstdout:\n{stdout}"
        );
    }
}

fn tray_rgba() -> tauri::image::Image<'static> {
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for pixel in rgba.chunks_mut(4) {
        pixel.copy_from_slice(&[24, 168, 154, 255]);
    }
    tauri::image::Image::new_owned(rgba, 32, 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(debug_assertions)]
    fn shipped_viewport_passes_production_validation() {
        viewport_example().unwrap();
    }

    #[test]
    fn login_autostart_requires_explicit_opt_in() {
        for value in [None, Some("0"), Some("true"), Some("")] {
            assert!(!autostart_enabled(value));
        }
        assert!(autostart_enabled(Some("1")));
    }
}
