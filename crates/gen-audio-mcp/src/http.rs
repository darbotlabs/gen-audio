//! Optional loopback HTTP transport. One JSON-RPC request per POST. No session id.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use serde_json::Value;

use crate::{handle, Server};

/// Browser origins allowed to call this loopback server (AP-devbox2-8). A
/// loopback server is not private: every page the user opens can reach it,
/// so a request that carries any other Origin is refused (403), and the
/// allowed origin is echoed back, never `*`. Requests with no Origin
/// (mcp-call.ps1, agent clients, curl) are not browser pages and pass.
/// Release: the Tauri webview origins (Windows http/https://tauri.localhost,
/// macOS/Linux tauri://localhost). Debug builds add the vite devUrl.
#[cfg(not(debug_assertions))]
pub const ALLOWED_ORIGINS: &[&str] = &["http://tauri.localhost", "https://tauri.localhost", "tauri://localhost"];
#[cfg(debug_assertions)]
pub const ALLOWED_ORIGINS: &[&str] = &[
    "http://tauri.localhost",
    "https://tauri.localhost",
    "tauri://localhost",
    "http://localhost:1420",
    "http://127.0.0.1:1420",
];

pub fn serve(addr: &str) -> std::io::Result<()> {
    gen_audio_core::paths::reap_stale_mcp_addr();
    let listener = bind_listener(addr)?;
    if let Ok(bound) = listener.local_addr() {
        let _ = gen_audio_core::paths::publish_mcp_addr(&bound.to_string());
    }
    accept_loop(listener);
    Ok(())
}

/// Bind the loopback MCP listener and accept on a background thread.
pub fn spawn_loopback(addr: &str) -> std::io::Result<SocketAddr> {
    let listener = bind_listener(addr)?;
    let bound = listener.local_addr()?;
    std::thread::Builder::new()
        .name("gen-audio-mcp".into())
        .spawn(move || accept_loop(listener))
        .map_err(std::io::Error::other)?;
    Ok(bound)
}

pub fn initialize_handshake(addr: &str) -> Result<Value, String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "gen-audio-desktop", "version": "0.1.0"}
        }
    });
    let body = serde_json::to_vec(&payload).map_err(|err| err.to_string())?;
    let mut stream = TcpStream::connect(addr).map_err(|err| err.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| err.to_string())?;
    let header = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .map_err(|err| err.to_string())?;
    stream.write_all(&body).map_err(|err| err.to_string())?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|err| err.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let Some((_, rest)) = text.split_once("\r\n\r\n") else {
        return Err("handshake response had no body".into());
    };
    let value: Value = serde_json::from_str(rest.trim()).map_err(|err| err.to_string())?;
    if value["result"]["protocolVersion"].as_str() != Some("2025-03-26") {
        return Err("handshake protocolVersion".into());
    }
    if value["result"]["serverInfo"]["name"].as_str() != Some("gen-audio") {
        return Err("handshake serverInfo.name".into());
    }
    Ok(value)
}

/// POST one `tools/call` to a running desktop listener and return the tool JSON.
pub fn tools_call(addr: &str, name: &str, args: &Value) -> Result<Value, String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": args}
    });
    let response = post_mcp(addr, &payload)?;
    if let Some(message) = response["error"]["message"].as_str() {
        return Err(message.to_string());
    }
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .ok_or("tools/call returned no text")?;
    serde_json::from_str(text).map_err(|err| err.to_string())
}

pub fn get_control(addr: &str, after: u64) -> Result<Value, String> {
    let mut stream = TcpStream::connect(addr).map_err(|err| err.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| err.to_string())?;
    let req =
        format!("GET /control?after={after} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|err| err.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let Some((_, rest)) = text.split_once("\r\n\r\n") else {
        return Err("control response had no body".into());
    };
    serde_json::from_str(rest.trim()).map_err(|err| err.to_string())
}

fn post_mcp(addr: &str, payload: &Value) -> Result<Value, String> {
    let body = serde_json::to_vec(payload).map_err(|err| err.to_string())?;
    let mut stream = TcpStream::connect(addr).map_err(|err| err.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| err.to_string())?;
    let header = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .map_err(|err| err.to_string())?;
    stream.write_all(&body).map_err(|err| err.to_string())?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|err| err.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let Some((_, rest)) = text.split_once("\r\n\r\n") else {
        return Err("mcp response had no body".into());
    };
    serde_json::from_str(rest.trim()).map_err(|err| err.to_string())
}

fn bind_listener(addr: &str) -> std::io::Result<TcpListener> {
    ensure_bind_allowed(addr)?;
    TcpListener::bind(addr)
}

fn accept_loop(listener: TcpListener) {
    let remote = listener
        .local_addr()
        .map(|socket| !socket.ip().is_loopback())
        .unwrap_or(true);
    let shared = std::sync::Arc::new(Server::boot());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if remote {
            let rejections = std::sync::Arc::clone(&shared.rejections);
            std::thread::spawn(move || {
                // Fresh scratch per remote request, but one set of rejection counts per listener.
                let mut isolated = Server::isolated();
                isolated.rejections = rejections;
                let dir = isolated.scratch.dir.clone();
                let _ = handle_connection(&isolated, stream);
                let _ = std::fs::remove_dir_all(dir);
            });
        } else {
            let shared = std::sync::Arc::clone(&shared);
            std::thread::spawn(move || {
                let _ = handle_connection(&shared, stream);
            });
        }
    }
}

pub fn ensure_bind_allowed(addr: &str) -> std::io::Result<()> {
    let socket: SocketAddr = addr.parse().map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("addr: {err}"))
    })?;
    let remote_ok = std::env::var("GEN_AUDIO_MCP_HTTP_ALLOW_REMOTE")
        .ok()
        .as_deref()
        == Some("1");
    if !socket.ip().is_loopback() && !remote_ok {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing non-loopback bind; set GEN_AUDIO_MCP_HTTP_ALLOW_REMOTE=1 to override",
        ));
    }
    Ok(())
}

/// Ports whose `handle_connection` should record the `Server` it was given.
/// Keyed by the listener port so parallel tests do not share a list. Absent
/// from the release binary: this is `cfg(test)` only.
#[cfg(test)]
static HANDLED_SERVERS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u16, Vec<usize>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(test)]
fn note_handled_server(server: &Server, stream: &TcpStream) {
    let Ok(addr) = stream.local_addr() else { return };
    let Ok(mut watched) = HANDLED_SERVERS.lock() else { return };
    if let Some(ids) = watched.get_mut(&addr.port()) {
        ids.push(server as *const Server as usize);
    }
}

pub fn handle_connection(server: &Server, mut stream: TcpStream) -> std::io::Result<()> {
    #[cfg(test)]
    note_handled_server(server, &stream);
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let (head, body) = match read_request(&mut stream) {
        Ok(parts) => parts,
        Err((status, body, line)) => {
            let mut parts = line.split_whitespace();
            let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            return reject(&server.rejections, &mut stream, method, path, status, body, "", "read");
        }
    };
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    // DNS rebinding: a page on evil.example:<port> that resolves to 127.0.0.1
    // sends Host evil.example:<port>. Only the listener's own address passes.
    if !host_allowed(header(&head, "host"), stream.local_addr().ok()) {
        return reject(&server.rejections, &mut stream, method, path, 403, br#"{"error":"host not allowed"}"#, "", "host");
    }
    let cors = match header(&head, "origin") {
        None => String::new(),
        Some(origin) if ALLOWED_ORIGINS.contains(&origin) => cors_headers(&stream, origin),
        Some(_) => return reject(&server.rejections, &mut stream, method, path, 403, br#"{"error":"origin not allowed"}"#, "", "origin"),
    };
    if method == "OPTIONS" {
        return write_response(&mut stream, 204, b"", &cors);
    }
    if method == "GET" && path == "/health" {
        let (rejected, last_rejected) = server.rejections.snapshot();
        let body = serde_json::to_vec(&serde_json::json!({
            "status": "ok",
            "service": "gen-audio-mcp",
            "role": "mcp",
            "stateless": true,
            "genaidAudioProbed": false,
            "rejected": rejected,
            "last_rejected": last_rejected
        }))
        .unwrap_or_else(|_| b"{}".to_vec());
        return write_response(&mut stream, 200, &body, &cors);
    }
    if method == "GET" && path == "/ready" {
        let body = br#"{"ready":true,"service":"gen-audio-mcp","role":"mcp","stateless":true,"speech":false,"modelsLoaded":false,"note":"Listener is up. This is not genaid-audio /ready and not a loaded TTS model."}"#;
        return write_response(&mut stream, 200, body, &cors);
    }
    if method == "GET" && (path == "/control/stream" || path.starts_with("/control/stream?")) {
        return write_control_stream(&mut stream, path, &cors);
    }
    if method == "GET" && (path == "/control" || path.starts_with("/control?")) {
        let after = query_u64(path, "after").unwrap_or(0);
        let delta = crate::control::since(after);
        let body = serde_json::to_vec(&serde_json::json!({
            "stateless": true,
            "speech": false,
            "cursor": delta.cursor,
            "gap": delta.gap,
            "oldest": delta.oldest,
            "resync": "viewport_get",
            "events": delta.events
        }))
        .unwrap_or_else(|_| b"{}".to_vec());
        return write_response(&mut stream, 200, &body, &cors);
    }
    if method == "GET" {
        if let Some(rel) = library_rel(path) {
            return write_library(&server.rejections, &mut stream, method, path, rel, &cors);
        }
    }
    if method != "POST" || path != "/mcp" {
        return reject(&server.rejections, &mut stream, method, path, 404, br#"{"error":"not found"}"#, &cors, "route");
    }
    // A text/plain (or untyped) POST is a CORS "simple request": a page can
    // send it with no preflight. JSON-RPC here is application/json only.
    if !is_json_content_type(header(&head, "content-type")) {
        return reject(&server.rejections, &mut stream, method, path, 415, br#"{"error":"content-type must be application/json"}"#, &cors, "content-type");
    }
    if head.to_ascii_lowercase().contains("transfer-encoding:") {
        return reject(&server.rejections, &mut stream, method, path, 400, br#"{"error":"chunked bodies are not accepted"}"#, &cors, "chunked");
    }
    let Some(length) = content_length(&head) else {
        return reject(&server.rejections, &mut stream, method, path, 411, br#"{"error":"content-length required"}"#, &cors, "length");
    };
    if body.len() != length {
        return reject(&server.rejections, &mut stream, method, path, 400, br#"{"error":"incomplete body"}"#, &cors, "body");
    }
    let message: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return reject(&server.rejections, &mut stream, method, path, 400, br#"{"error":"invalid json"}"#, &cors, "json"),
    };
    let rpc_method = message.get("method").and_then(Value::as_str).unwrap_or("").to_string();
    let tool = message.pointer("/params/name").and_then(Value::as_str).unwrap_or("").to_string();
    match handle(server, message) {
        Ok(Some(response)) => {
            if let Some(code) = response.pointer("/error/code") {
                // A JSON-RPC rejection: name the tool (or the RPC method).
                let target = if tool.is_empty() { rpc_method.clone() } else { format!("{rpc_method} {tool}") };
                log_rejection(&stream, method, &target, &code.to_string(), "rpc");
            }
            let bytes = serde_json::to_vec(&response).unwrap_or_else(|_| b"{}".to_vec());
            write_response(&mut stream, 200, &bytes, &cors)
        }
        Ok(None) => write_response(&mut stream, 202, b"{}", &cors),
        Err(err) => {
            let payload = serde_json::to_vec(&serde_json::json!({"error": err})).unwrap_or_else(|_| b"{}".to_vec());
            reject(&server.rejections, &mut stream, method, path, 500, &payload, &cors, "handler")
        }
    }
}

/// First value of a header (case-insensitive name), trimmed.
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Host must name this listener: 127.0.0.1, localhost or [::1] (or the bound
/// IP) with the listener's port. Missing Host is refused too.
fn host_allowed(host: Option<&str>, local: Option<SocketAddr>) -> bool {
    let (Some(host), Some(local)) = (host, local) else { return false };
    let port = local.port();
    let ip = match local.ip() {
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        std::net::IpAddr::V4(v4) => v4.to_string(),
    };
    [format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}"), format!("{ip}:{port}")]
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
}

fn is_json_content_type(value: Option<&str>) -> bool {
    value
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

fn read_request(stream: &mut TcpStream) -> Result<(String, Vec<u8>), (u16, &'static [u8], String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if buf.len() > 1024 * 1024 + 8192 {
            return Err((413, br#"{"error":"body too large"}"#, first_line(&buf)));
        }
        match stream.read(&mut tmp) {
            Ok(0) => break None,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos);
                }
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                break None;
            }
            Err(_) => return Err((400, br#"{"error":"bad request"}"#, first_line(&buf))),
        }
    };
    let Some(pos) = header_end else {
        return Err((400, br#"{"error":"bad request"}"#, first_line(&buf)));
    };
    let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
    if head.to_ascii_lowercase().contains("transfer-encoding:") {
        return Err((400, br#"{"error":"chunked bodies are not accepted"}"#, first_line(&buf)));
    }
    let mut body = buf[pos + 4..].to_vec();
    let Some(length) = content_length(&head) else {
        // GET and OPTIONS carry no body. Chromium's CORS preflight sends no
        // Content-Length, so OPTIONS must pass here or every window POST dies.
        let request_line = head.lines().next().unwrap_or("");
        if body.is_empty() && (request_line.starts_with("GET ") || request_line.starts_with("OPTIONS ")) {
            return Ok((head, body));
        }
        return Err((411, br#"{"error":"content-length required"}"#, first_line(&buf)));
    };
    if length > 1024 * 1024 {
        return Err((413, br#"{"error":"body too large"}"#, first_line(&buf)));
    }
    while body.len() < length {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                let need = length - body.len();
                body.extend_from_slice(&tmp[..n.min(need)]);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => break,
            Err(_) => return Err((400, br#"{"error":"bad request"}"#, first_line(&buf))),
        }
    }
    if body.len() != length {
        return Err((400, br#"{"error":"incomplete body"}"#, first_line(&buf)));
    }
    Ok((head, body))
}

/// The request line of whatever was read (for the rejection log).
fn first_line(buf: &[u8]) -> String {
    let end = buf.iter().position(|&byte| byte == b'\r' || byte == b'\n').unwrap_or(buf.len()).min(200);
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Recent rejection log lines (the same lines go to stderr), newest last.
static REJECTIONS: std::sync::Mutex<std::collections::VecDeque<String>> = std::sync::Mutex::new(std::collections::VecDeque::new());

#[cfg(test)]
thread_local! {
    /// Test-only: the rejection lines logged on this thread. A test's worker
    /// thread serves exactly one connection, so these are that connection's
    /// lines, whatever its client port.
    static LOGGED_ON_THIS_THREAD: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// One stderr line per rejected request: method, path or tool, code, reason
/// and peer. Kept in a small ring so tests (and a future status read) can see it.
fn log_rejection(stream: &TcpStream, method: &str, target: &str, code: &str, reason: &str) {
    let peer = stream.peer_addr().map(|addr| addr.to_string()).unwrap_or_else(|_| "?".into());
    let method = if method.is_empty() { "-" } else { method };
    let target = if target.is_empty() { "-" } else { target };
    let line = format!("gen-audio-mcp http: rejected {method} {target} {code} {reason} peer={peer}");
    eprintln!("{line}");
    #[cfg(test)]
    LOGGED_ON_THIS_THREAD.with(|lines| lines.borrow_mut().push(line.clone()));
    if let Ok(mut ring) = REJECTIONS.lock() {
        if ring.len() >= 256 {
            ring.pop_front();
        }
        ring.push_back(line);
    }
}

/// Per-listener counts of rejected HTTP requests by status, plus the last
/// one (method, path without query, status, unix ms), served on /health. A
/// client cannot report a failure on the channel that failed, so the server
/// keeps the count. Never holds request bodies or headers.
#[derive(Default)]
pub struct RejectionStats {
    inner: std::sync::Mutex<(std::collections::BTreeMap<u16, u64>, Option<Value>)>,
}

impl RejectionStats {
    /// The statuses /health always lists, zero or not.
    const BASELINE: [u16; 4] = [400, 403, 411, 415];

    fn note(&self, method: &str, target: &str, status: u16) {
        let path: String = target.split('?').next().unwrap_or("").chars().take(200).collect();
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        if let Ok(mut inner) = self.inner.lock() {
            *inner.0.entry(status).or_insert(0) += 1;
            inner.1 = Some(serde_json::json!({"method": method, "path": path, "status": status, "t": t}));
        }
    }

    /// `{"rejected": {"400": n, ...}, "last_rejected": {...} | null}`.
    pub fn snapshot(&self) -> (Value, Value) {
        let (counts, last) = self.inner.lock().map(|inner| (inner.0.clone(), inner.1.clone())).unwrap_or_default();
        let mut rejected = serde_json::Map::new();
        for status in Self::BASELINE {
            rejected.insert(status.to_string(), Value::from(0u64));
        }
        for (status, count) in counts {
            rejected.insert(status.to_string(), Value::from(count));
        }
        (Value::Object(rejected), last.unwrap_or(Value::Null))
    }
}

pub fn recent_rejections() -> Vec<String> {
    REJECTIONS.lock().map(|ring| ring.iter().cloned().collect()).unwrap_or_default()
}

fn reject(stats: &RejectionStats, stream: &mut TcpStream, method: &str, target: &str, status: u16, body: &[u8], cors: &str, reason: &str) -> std::io::Result<()> {
    log_rejection(stream, method, target, &status.to_string(), reason);
    stats.note(method, target, status);
    write_response(stream, status, body, cors)
}

fn content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

fn query_u64(path: &str, key: &str) -> Option<u64> {
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
        if name == key {
            return value.parse().ok();
        }
    }
    None
}

fn write_control_stream(stream: &mut TcpStream, path: &str, cors: &str) -> std::io::Result<()> {
    let wait_ms = query_u64(path, "wait").unwrap_or(0).min(2_000);
    let after = query_u64(path, "after").unwrap_or(0);
    let started = std::time::Instant::now();
    let mut sent = after;
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nX-Gen-Audio-Stateless: 1\r\n{cors}\r\n"
    );
    stream.write_all(header.as_bytes())?;
    loop {
        let delta = crate::control::since(sent);
        if delta.gap {
            let data = serde_json::json!({
                "gap": true,
                "after": sent,
                "oldest": delta.oldest,
                "cursor": delta.cursor,
                "resync": "viewport_get"
            });
            stream.write_all(format!("event: gap\ndata: {data}\n\n").as_bytes())?;
            break;
        }
        for event in delta.events {
            let seq = event
                .get("seq")
                .and_then(|value| value.as_u64())
                .unwrap_or(sent);
            sent = sent.max(seq);
            let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
            stream.write_all(format!("id: {seq}\nevent: control\ndata: {data}\n\n").as_bytes())?;
        }
        if wait_ms == 0 || started.elapsed() >= std::time::Duration::from_millis(wait_ms) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    stream.write_all(b"event: end\ndata: {\"stateless\":true,\"speech\":false}\n\n")?;
    Ok(())
}

/// CORS headers for an allowlisted origin: the origin itself, never `*`.
fn cors_headers(stream: &TcpStream, origin: &str) -> String {
    let loopback = stream.local_addr().map(|addr| addr.ip().is_loopback()).unwrap_or(false);
    if loopback {
        format!("Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type\r\n")
    } else {
        String::new()
    }
}

fn library_rel(path: &str) -> Option<&str> {
    let path = path.split('?').next().unwrap_or(path);
    let rel = path.strip_prefix("/library/")?;
    if rel.is_empty() || rel.ends_with('/') {
        return None;
    }
    Some(rel)
}

fn write_library(stats: &RejectionStats, stream: &mut TcpStream, method: &str, path: &str, rel: &str, cors: &str) -> std::io::Result<()> {
    const CAP: u64 = 64 * 1024 * 1024;
    match gen_audio_core::library_store::open_library_media(rel) {
        Ok((file, mime)) => {
            let meta = std::fs::metadata(&file).map_err(|_| std::io::Error::other("library media"))?;
            if meta.len() > CAP {
                return reject(stats, stream, method, path, 413, br#"{"error":"library media is too large"}"#, cors, "library");
            }
            let body = std::fs::read(&file).map_err(|_| std::io::Error::other("library media"))?;
            write_typed(stream, 200, mime, &body, cors)
        }
        Err(_) => reject(stats, stream, method, path, 404, br#"{"error":"not found"}"#, cors, "library"),
    }
}

fn write_response(stream: &mut TcpStream, status: u16, body: &[u8], cors: &str) -> std::io::Result<()> {
    write_typed(stream, status, "application/json", body, cors)
}

fn write_typed(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    cors: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nX-Gen-Audio-Stateless: 1\r\n{cors}\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    if status != 204 {
        stream.write_all(body)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn http_health_has_no_session_header() {
        assert!(ensure_bind_allowed("0.0.0.0:8765").is_err());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(&Server::boot(), stream).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes())
            .unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.contains("gen-audio-mcp"));
        assert!(!text.to_ascii_lowercase().contains("mcp-session-id"));
        assert!(text.contains("X-Gen-Audio-Stateless"));
    }

    /// GET /health on a listener that serves every connection with one shared Server.
    fn health_of(port: u16) -> Value {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
            .unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }

    #[test]
    fn rejected_posts_are_counted_on_health_with_the_last_one_named() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let server = std::sync::Arc::new(Server::boot());
            for stream in listener.incoming().take(5) {
                let _ = handle_connection(&server, stream.unwrap());
            }
        });
        let send = |request: String| {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut text = String::new();
            stream.read_to_string(&mut text).unwrap();
            text.split_whitespace().nth(1).and_then(|code| code.parse::<u16>().ok()).unwrap_or(0)
        };
        let before = health_of(port);
        assert_eq!(before["rejected"], serde_json::json!({"400": 0, "403": 0, "411": 0, "415": 0}), "{before}");
        assert!(before["last_rejected"].is_null(), "{before}");

        // A page on another origin: 403, and the tool does not run.
        let marker = "secret-marker-in-the-body";
        let evil = post(port, "Origin: https://evil.example\r\nContent-Type: application/json\r\nX-Note: header-marker\r\n", &NAVIGATE.replace("library", marker));
        assert_eq!(send(evil), 403);
        let after_403 = health_of(port);
        assert_eq!(after_403["rejected"]["403"], 1, "{after_403}");
        let last = &after_403["last_rejected"];
        assert_eq!((last["method"].as_str(), last["path"].as_str(), last["status"].as_u64()), (Some("POST"), Some("/mcp"), Some(403)), "{last}");
        assert!(last["t"].as_u64().is_some_and(|t| t > 1_700_000_000_000), "unix ms: {last}");
        assert_eq!(last.as_object().unwrap().len(), 4, "method, path, status, t and nothing else: {last}");

        // A CORS simple request (text/plain): 415.
        assert_eq!(send(post(port, "Content-Type: text/plain\r\n", NAVIGATE)), 415);
        let after_415 = health_of(port);
        assert_eq!(after_415["rejected"], serde_json::json!({"400": 0, "403": 1, "411": 0, "415": 1}), "{after_415}");
        assert_eq!(after_415["last_rejected"]["status"], 415);
        assert_eq!(after_415["last_rejected"]["path"], "/mcp");
        let text = after_415.to_string();
        for leaked in [marker, "header-marker", "evil.example", "text/plain"] {
            assert!(!text.contains(leaked), "/health must not echo request bodies or headers ({leaked}): {text}");
        }
        assert_eq!(after_415["status"], "ok");
    }

    #[test]
    fn remote_accept_loop_shares_rejection_stats_across_connections() {
        // Bind the wildcard directly. accept_loop treats a non-loopback local
        // address as remote, without GEN_AUDIO_MCP_HTTP_ALLOW_REMOTE.
        let listener = TcpListener::bind("0.0.0.0:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || accept_loop(listener));
        let send = |request: String| {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut text = String::new();
            stream.read_to_string(&mut text).unwrap();
            text.split_whitespace().nth(1).and_then(|code| code.parse::<u16>().ok()).unwrap_or(0)
        };
        assert_eq!(send(post(port, "Content-Type: text/plain\r\n", "{}")), 415);
        assert_eq!(send(post(port, "Content-Type: text/plain\r\n", "{}")), 415);
        let health = health_of(port);
        assert_eq!(health["rejected"]["415"], 2, "remote connections dropped the shared stats: {health}");
    }

    #[test]
    fn local_accept_loop_hands_each_connection_the_same_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        super::HANDLED_SERVERS.lock().unwrap().insert(port, Vec::new());
        std::thread::spawn(move || accept_loop(listener));
        let _ = health_of(port);
        let _ = health_of(port);
        let ids = super::HANDLED_SERVERS.lock().unwrap().remove(&port).unwrap_or_default();
        assert_eq!(ids.len(), 2, "expected one server id per connection, got {ids:?}");
        assert_eq!(ids[0], ids[1], "local connections were given different Server instances: {ids:?}");
    }

    #[test]
    fn ready_is_listener_readiness_and_stream_is_stateless() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(&Server::boot(), stream).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(format!("GET /ready HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes())
            .unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.contains("\"ready\":true"), "{text}");
        assert!(text.contains("\"speech\":false"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("mcp-session-id"));

        let bus = crate::control::TestBus::fresh();
        let _bus = bus.bind();
        let seq = crate::control::publish(
            "navigate",
            &serde_json::json!({"slide": "library", "marker": "ready-stream"}),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handler_bus = bus.clone();
        std::thread::spawn(move || {
            let _bus = handler_bus.bind();
            let (stream, _) = listener.accept().unwrap();
            handle_connection(&Server::boot(), stream).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        let req = format!(
            "GET /control/stream?after={}&wait=0 HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n",
            seq.saturating_sub(1)
        );
        stream.write_all(req.as_bytes()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.contains("text/event-stream"), "{text}");
        assert!(text.contains("ready-stream"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("mcp-session-id"));
    }

    #[test]
    fn serve_logs_mcp_addr_write_failure() {
        if std::env::var("GEN_AUDIO_MCP_ADDR_SERVE_FAIL").ok().as_deref() == Some("1") {
            let _ = serve("127.0.0.1:0");
            return;
        }
        let blocker = std::env::temp_dir().join(format!(
            "gen-audio-mcp-addr-serve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&blocker, b"not-a-directory").unwrap();
        let target = blocker.join("mcp.addr");
        // The child is killed, so its Drop and atexit never run. The parent
        // owns the scratch directory and removes it when this guard drops,
        // including when the assertion below panics.
        let work = gen_audio_core::paths::make_work_dir().expect("work dir");
        let work_path = work.path().to_path_buf();
        // Count before spawn. The child's pid is not known yet; keep every
        // gen-audio-* path and filter to that pid afterwards. A snapshot taken
        // after spawn already contains a directory serve() created itself.
        let preexisting = work_dirs_for_prefix("gen-audio-");
        let mut child = std::process::Command::new(std::env::current_exe().expect("test exe"))
            .arg("serve_logs_mcp_addr_write_failure")
            .arg("--test-threads=1")
            .env("GEN_AUDIO_MCP_ADDR_SERVE_FAIL", "1")
            .env("GEN_AUDIO_MCP_ADDR_FILE", &target)
            .env("GEN_AUDIO_WORK_DIR", &work_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn serve");
        let pid = child.id();
        let before: std::collections::BTreeSet<_> = preexisting
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&format!("gen-audio-{pid}-")))
            })
            .collect();
        let mut stderr = child.stderr.take().expect("stderr");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut acc = Vec::new();
            let mut tmp = [0u8; 512];
            let needle = b"gen-audio-mcp: mcp.addr write failed:";
            loop {
                match std::io::Read::read(&mut stderr, &mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&tmp[..n]);
                        if acc.windows(needle.len()).any(|window| window == needle) {
                            let _ = tx.send(String::from_utf8_lossy(&acc).into_owned());
                            break;
                        }
                    }
                }
            }
        });
        let logged = rx.recv_timeout(std::time::Duration::from_secs(10));
        // The failure line is written before accept_loop boots the scratch.
        // Wait until that directory exists, then kill. Killing is SIGKILL, so
        // the child's Drop and atexit do not run.
        let appeared = std::time::Instant::now();
        while work_dirs_for_prefix(&format!("gen-audio-{pid}-")).difference(&before).next().is_none() {
            if appeared.elapsed() > std::time::Duration::from_millis(300) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
        drop(work);
        let _ = std::fs::remove_file(&blocker);
        let text = logged.expect("serve did not log the mcp.addr write failure");
        assert!(
            text.contains("gen-audio-mcp: mcp.addr write failed:"),
            "{text}"
        );
        assert!(!work_path.exists(), "parent TempWorkDir did not remove {work_path:?}");
        let after = work_dirs_for_prefix(&format!("gen-audio-{pid}-"));
        let leaked: Vec<_> = after.difference(&before).cloned().collect();
        assert!(
            leaked.is_empty(),
            "serve_logs_mcp_addr_write_failure left {} work dir(s): {leaked:?}",
            leaked.len()
        );
    }

    /// Temp directories whose name starts with `prefix`. The before-count is
    /// taken before the child is spawned; the after-count is taken after it
    /// has been killed.
    fn work_dirs_for_prefix(prefix: &str) -> std::collections::BTreeSet<std::path::PathBuf> {
        let mut found = std::collections::BTreeSet::new();
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
            return found;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(prefix) {
                found.insert(entry.path());
            }
        }
        found
    }

    #[test]
    fn spawned_listener_completes_initialize() {
        let addr = spawn_loopback("127.0.0.1:0").unwrap();
        let value = initialize_handshake(&addr.to_string()).unwrap();
        assert_eq!(value["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(value["result"]["serverInfo"]["name"], "gen-audio");
    }

    // ---- AP-devbox2-8: a loopback server is not private ---------------------

    const NAVIGATE: &str = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"ui_navigate","arguments":{"slide":"slide:library"}}}"#;

    /// One request against a fresh loopback listener; `build` gets the port.
    fn roundtrip(build: impl FnOnce(u16) -> String) -> (u16, String) {
        let (status, text, _peer, _served) = roundtrip_seeded(build, |_| {});
        (status, text)
    }

    /// One request on a fresh listener. `prepare` runs on the worker before
    /// the request is read, so a thread-local such as the library root is
    /// visible to the handler. `seed` runs on the caller once the client is
    /// connected, before the request is sent. The returned lines are the ones
    /// that worker logged, never a `peer=` filter: the client port is the
    /// OS's choice and a reused one matched another connection.
    fn roundtrip_prepared(
        build: impl FnOnce(u16) -> String,
        prepare: impl FnOnce() + Send + 'static,
        seed: impl FnOnce(&str),
    ) -> (u16, String, String, Vec<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            prepare();
            let (stream, _) = listener.accept().unwrap();
            let _ = handle_connection(&Server::boot(), stream);
            LOGGED_ON_THIS_THREAD.with(|lines| lines.take())
        });
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let peer = stream.local_addr().unwrap().to_string();
        seed(&peer);
        stream.write_all(build(port).as_bytes()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        let served = worker.join().unwrap();
        let status = text.split_whitespace().nth(1).and_then(|code| code.parse().ok()).unwrap_or(0);
        (status, text, peer, served)
    }

    fn roundtrip_seeded(build: impl FnOnce(u16) -> String, seed: impl FnOnce(&str)) -> (u16, String, String, Vec<String>) {
        roundtrip_prepared(build, || {}, seed)
    }

    fn roundtrip_on_worker(
        build: impl FnOnce(u16) -> String,
        prepare: impl FnOnce() + Send + 'static,
    ) -> (u16, String, String, Vec<String>) {
        roundtrip_prepared(build, prepare, |_| {})
    }

    /// The lines this test counts as one connection's rejection log: the ones
    /// its own worker logged.
    fn rejection_lines(_peer: &str, served: &[String]) -> Vec<String> {
        served.to_vec()
    }

    fn post(port: u16, headers: &str, body: &str) -> String {
        format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    fn response_header<'a>(text: &'a str, name: &str) -> Option<&'a str> {
        let head = text.split("\r\n\r\n").next().unwrap_or("");
        head.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    #[test]
    fn preflight_from_an_allowed_origin_echoes_that_origin_and_varies_on_it() {
        for origin in ALLOWED_ORIGINS {
            let (status, text) = roundtrip(|port| format!(
                "OPTIONS /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ));
            assert!((200..300).contains(&status), "{origin}: {text}");
            assert_eq!(response_header(&text, "access-control-allow-origin"), Some(*origin), "{text}");
            assert_eq!(response_header(&text, "vary"), Some("Origin"), "{text}");
            assert!(response_header(&text, "access-control-allow-headers").is_some_and(|v| v.contains("content-type")), "{text}");
        }
        assert!(!ALLOWED_ORIGINS.contains(&"*"));
    }

    #[test]
    fn preflight_from_another_origin_is_403() {
        let (status, text) = roundtrip(|port| format!(
            "OPTIONS /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ));
        assert_eq!(status, 403, "{text}");
        assert!(response_header(&text, "access-control-allow-origin").is_none(), "{text}");
    }

    #[test]
    fn post_that_is_not_application_json_is_415_and_runs_nothing() {
        // text/plain needs no preflight: a page can send it with fetch(no-cors).
        let (status, text) = roundtrip(|port| post(port, "Origin: https://evil.example\r\nContent-Type: text/plain\r\n", NAVIGATE));
        assert_ne!(status, 200, "ATTACK text/plain simple POST from evil origin ran the tool: {text}");
        for headers in ["Content-Type: text/plain;charset=UTF-8\r\n", ""] {
            let (status, text) = roundtrip(|port| post(port, headers, NAVIGATE));
            assert_eq!(status, 415, "POST with {headers:?}: {text}");
        }
        let (status, text) = roundtrip(|port| post(port, "Content-Type: application/json; charset=utf-8\r\n", NAVIGATE));
        assert_eq!(status, 200, "{text}");
    }

    #[test]
    fn post_from_a_disallowed_origin_is_403() {
        let (status, text) = roundtrip(|port| post(port, "Origin: https://evil.example\r\nContent-Type: application/json\r\n", NAVIGATE));
        assert_eq!(status, 403, "ATTACK cross-origin JSON POST: {text}");
    }

    #[test]
    fn host_other_than_this_listener_is_403() {
        // DNS rebinding: evil.example resolves to 127.0.0.1, the browser still sends its own name.
        let (status, text) = roundtrip(|port| post(port, "Content-Type: application/json\r\n", NAVIGATE).replace(&format!("Host: 127.0.0.1:{port}"), &format!("Host: evil.example:{port}")));
        assert_eq!(status, 403, "ATTACK rebinding Host evil.example accepted: {text}");
        for host in ["localhost:{port}", "127.0.0.1:{port}"] {
            let (status, text) = roundtrip(|port| post(port, "Content-Type: application/json\r\n", NAVIGATE).replace(&format!("Host: 127.0.0.1:{port}"), &format!("Host: {}", host.replace("{port}", &port.to_string()))));
            assert_eq!(status, 200, "{host}: {text}");
        }
    }

    #[test]
    fn post_with_no_origin_still_works_for_agent_clients() {
        let (status, text) = roundtrip(|port| post(port, "Content-Type: application/json\r\n", NAVIGATE));
        assert_eq!(status, 200, "{text}");
        assert!(text.contains("\"result\""), "{text}");
        assert!(response_header(&text, "access-control-allow-origin").is_none(), "no Origin, no CORS grant: {text}");
    }

    #[test]
    fn control_stream_from_a_disallowed_origin_is_403() {
        let (status, text) = roundtrip(|port| format!(
            "GET /control/stream?after=0&wait=0 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n"
        ));
        assert_eq!(status, 403, "ATTACK evil origin read the control stream: {}", text.lines().take(12).collect::<Vec<_>>().join(" | "));
    }

    /// The preflight WebView2/Chromium really sends (captured by CDP on SMAX,
    /// diag-seek): no Content-Length. Invoke-WebRequest adds Content-Length: 0
    /// itself, which is why PowerShell checks never saw the 411.
    #[test]
    fn chromium_preflight_without_content_length_is_2xx_with_cors() {
        let (status, text) = roundtrip(|port| format!(
            "OPTIONS /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: keep-alive\r\nAccept: */*\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\nOrigin: http://tauri.localhost\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36 Edg/154.0.0.0\r\nSec-Fetch-Mode: cors\r\nSec-Fetch-Site: cross-site\r\nSec-Fetch-Dest: empty\r\nReferer: http://tauri.localhost/\r\nAccept-Encoding: gzip, deflate, br, zstd\r\nAccept-Language: en-US,en;q=0.9\r\n\r\n"
        ));
        assert!((200..300).contains(&status), "{text}");
        assert_eq!(response_header(&text, "access-control-allow-origin"), Some("http://tauri.localhost"), "{text}");
        assert!(response_header(&text, "access-control-allow-headers").is_some_and(|v| v.contains("content-type")), "{text}");
        // A POST without Content-Length still needs one.
        let (status, text) = roundtrip(|port| format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n"
        ));
        assert_eq!(status, 411, "{text}");
    }

    /// The flake seen at 06:1x PT on 2026-10-08: an earlier connection from
    /// the same client port had left `... 403 host peer=127.0.0.1:55556` in the
    /// shared ring, and keying on `peer=` counted it as the next request's
    /// line. Forced here, not left to port reuse: a line from "another
    /// connection" with this client's address is in the ring before the
    /// request goes out.
    #[test]
    fn a_rejection_is_counted_for_its_own_connection_even_on_a_reused_client_port() {
        let request = |port: u16| format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n");
        let stale = |peer: &str| REJECTIONS.lock().unwrap().push_back(format!("gen-audio-mcp http: rejected POST /mcp 403 host peer={peer}"));
        let (status, text, peer, served) = roundtrip_seeded(request, stale);
        let lines = rejection_lines(&peer, &served);
        assert_eq!(lines.len(), 1, "status {status}: {lines:?} {text}");
        assert!(lines[0].starts_with("gen-audio-mcp http: rejected POST /mcp 411 read "), "{lines:?}");
    }

    #[test]
    fn every_rejection_is_one_log_line_with_method_target_and_code() {
        let cases: Vec<(Box<dyn FnOnce(u16) -> String>, &str)> = vec![
            (Box::new(|port| format!("OPTIONS /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n")),
             "rejected OPTIONS /mcp 403 origin"),
            (Box::new(|port| post(port, "Content-Type: text/plain\r\n", NAVIGATE)), "rejected POST /mcp 415 content-type"),
            (Box::new(|port| post(port, "Content-Type: application/json\r\n", NAVIGATE).replace(&format!("Host: 127.0.0.1:{port}"), "Host: evil.example")),
             "rejected POST /mcp 403 host"),
            (Box::new(|port| format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n")),
             "rejected POST /mcp 411 read"),
            (Box::new(|port| post(port, "Content-Type: application/json\r\n", r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ui_navigate","arguments":{"slide":"slide:nope"}}}"#)),
             "rejected POST tools/call ui_navigate -32602 rpc"),
        ];
        for (build, want) in cases {
            let (status, text, peer, served) = roundtrip_seeded(build, |_| {});
            let lines = rejection_lines(&peer, &served);
            assert_eq!(lines.len(), 1, "one line per rejection ({want}), status {status}: {lines:?} {text}");
            assert!(lines[0].starts_with(&format!("gen-audio-mcp http: {want} ")), "{lines:?}");
            assert!(lines[0].ends_with(&format!(" peer={peer}")), "the line still names the peer: {lines:?}");
        }
        // Accepted requests log nothing.
        let (status, _text, peer, served) = roundtrip_seeded(|port| post(port, "Content-Type: application/json\r\n", NAVIGATE), |_| {});
        assert_eq!(status, 200);
        assert!(rejection_lines(&peer, &served).is_empty());
    }

    #[test]
    fn library_404_and_413_are_one_rejection_line() {
        let (status, text, peer, served) = roundtrip_seeded(|port| {
            format!("GET /library/missing.wav HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
        }, |_| {});
        assert_eq!(status, 404, "{text}");
        let lines = rejection_lines(&peer, &served);
        assert_eq!(lines.len(), 1, "{lines:?} {text}");
        assert!(
            lines[0].starts_with("gen-audio-mcp http: rejected GET /library/missing.wav 404 "),
            "{lines:?}"
        );
        assert!(lines[0].ends_with(&format!(" peer={peer}")), "{lines:?}");

        let dir = std::env::temp_dir().join(format!(
            "gen-audio-lib-413-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        struct RemoveDir(std::path::PathBuf);
        impl Drop for RemoveDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _remove = RemoveDir(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("big.wav");
        std::fs::File::create(&wav).unwrap().set_len(64 * 1024 * 1024 + 1).unwrap();
        let library = dir.clone();
        let (status, text, peer, served) = roundtrip_on_worker(
            |port| {
                format!("GET /library/big.wav HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
            },
            move || {
                gen_audio_core::library_store::set_root_override_for_test(Some(library));
            },
        );
        assert_eq!(status, 413, "{text}");
        let lines = rejection_lines(&peer, &served);
        assert_eq!(lines.len(), 1, "{lines:?} {text}");
        assert!(
            lines[0].starts_with("gen-audio-mcp http: rejected GET /library/big.wav 413 "),
            "{lines:?}"
        );
        assert!(lines[0].ends_with(&format!(" peer={peer}")), "{lines:?}");
    }

    #[test]
    fn t15_sse_gap_is_signalled_when_the_ring_drops_events() {
        let bus = crate::control::TestBus::fresh();
        let _bus = bus.bind();
        let anchor = crate::control::publish("gap-anchor", &serde_json::json!({"marker": "t15"}));
        for index in 0..(crate::control::CAP + 1) {
            crate::control::publish("gap-fill", &serde_json::json!({"index": index}));
        }
        let delta = crate::control::since(anchor);
        assert!(delta.gap, "dropped events must set gap");
        assert!(delta.oldest.unwrap_or(0) > anchor + 1);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handler_bus = bus.clone();
        std::thread::spawn(move || {
            let _bus = handler_bus.bind();
            let (stream, _) = listener.accept().unwrap();
            handle_connection(&Server::boot(), stream).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        let req = format!("GET /control/stream?after={anchor}&wait=0 HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.contains("event: gap"), "{text}");
        assert!(text.contains("viewport_get"), "{text}");
    }
}
