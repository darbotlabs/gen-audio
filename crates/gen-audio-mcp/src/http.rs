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
    let listener = bind_listener(addr)?;
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
    stream.write_all(header.as_bytes()).map_err(|err| err.to_string())?;
    stream.write_all(&body).map_err(|err| err.to_string())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|err| err.to_string())?;
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
            std::thread::spawn(move || {
                let isolated = Server::isolated();
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
    let socket: SocketAddr = addr
        .parse()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("addr: {err}")))?;
    let remote_ok = std::env::var("GEN_AUDIO_MCP_HTTP_ALLOW_REMOTE").ok().as_deref() == Some("1");
    if !socket.ip().is_loopback() && !remote_ok {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing non-loopback bind; set GEN_AUDIO_MCP_HTTP_ALLOW_REMOTE=1 to override",
        ));
    }
    Ok(())
}

pub fn handle_connection(server: &Server, mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let (head, body) = match read_request(&mut stream) {
        Ok(parts) => parts,
        Err(kind) => return write_response(&mut stream, kind.0, kind.1, ""),
    };
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    // DNS rebinding: a page on evil.example:<port> that resolves to 127.0.0.1
    // sends Host evil.example:<port>. Only the listener's own address passes.
    if !host_allowed(header(&head, "host"), stream.local_addr().ok()) {
        return write_response(&mut stream, 403, br#"{"error":"host not allowed"}"#, "");
    }
    let cors = match header(&head, "origin") {
        None => String::new(),
        Some(origin) if ALLOWED_ORIGINS.contains(&origin) => cors_headers(&stream, origin),
        Some(_) => return write_response(&mut stream, 403, br#"{"error":"origin not allowed"}"#, ""),
    };
    if method == "OPTIONS" {
        return write_response(&mut stream, 204, b"", &cors);
    }
    if method == "GET" && path == "/health" {
        let body = br#"{"status":"ok","service":"gen-audio-mcp","role":"mcp","stateless":true,"genaidAudioProbed":false}"#;
        return write_response(&mut stream, 200, body, &cors);
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
        let (cursor, events) = crate::control::since(after);
        let body = serde_json::to_vec(&serde_json::json!({
            "stateless": true,
            "speech": false,
            "cursor": cursor,
            "events": events
        }))
        .unwrap_or_else(|_| b"{}".to_vec());
        return write_response(&mut stream, 200, &body, &cors);
    }
    if method != "POST" || path != "/mcp" {
        return write_response(&mut stream, 404, br#"{"error":"not found"}"#, &cors);
    }
    // A text/plain (or untyped) POST is a CORS "simple request": a page can
    // send it with no preflight. JSON-RPC here is application/json only.
    if !is_json_content_type(header(&head, "content-type")) {
        return write_response(&mut stream, 415, br#"{"error":"content-type must be application/json"}"#, &cors);
    }
    if head.to_ascii_lowercase().contains("transfer-encoding:") {
        return write_response(&mut stream, 400, br#"{"error":"chunked bodies are not accepted"}"#, &cors);
    }
    let Some(length) = content_length(&head) else {
        return write_response(&mut stream, 411, br#"{"error":"content-length required"}"#, &cors);
    };
    if body.len() != length {
        return write_response(&mut stream, 400, br#"{"error":"incomplete body"}"#, &cors);
    }
    let message: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return write_response(&mut stream, 400, br#"{"error":"invalid json"}"#, &cors),
    };
    match handle(server, message) {
        Ok(Some(response)) => {
            let bytes = serde_json::to_vec(&response).unwrap_or_else(|_| b"{}".to_vec());
            write_response(&mut stream, 200, &bytes, &cors)
        }
        Ok(None) => write_response(&mut stream, 202, b"{}", &cors),
        Err(err) => {
            let payload = serde_json::to_vec(&serde_json::json!({"error": err})).unwrap_or_else(|_| b"{}".to_vec());
            write_response(&mut stream, 500, &payload, &cors)
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

fn read_request(stream: &mut TcpStream) -> Result<(String, Vec<u8>), (u16, &'static [u8])> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if buf.len() > 1024 * 1024 + 8192 {
            return Err((413, br#"{"error":"body too large"}"#));
        }
        match stream.read(&mut tmp) {
            Ok(0) => break None,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => {
                break None;
            }
            Err(_) => return Err((400, br#"{"error":"bad request"}"#)),
        }
    };
    let Some(pos) = header_end else {
        return Err((400, br#"{"error":"bad request"}"#));
    };
    let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
    if head.to_ascii_lowercase().contains("transfer-encoding:") {
        return Err((400, br#"{"error":"chunked bodies are not accepted"}"#));
    }
    let mut body = buf[pos + 4..].to_vec();
    let Some(length) = content_length(&head) else {
        if body.is_empty() && head.lines().next().unwrap_or("").starts_with("GET ") {
            return Ok((head, body));
        }
        return Err((411, br#"{"error":"content-length required"}"#));
    };
    if length > 1024 * 1024 {
        return Err((413, br#"{"error":"body too large"}"#));
    }
    while body.len() < length {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                let need = length - body.len();
                body.extend_from_slice(&tmp[..n.min(need)]);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => break,
            Err(_) => return Err((400, br#"{"error":"bad request"}"#)),
        }
    }
    if body.len() != length {
        return Err((400, br#"{"error":"incomplete body"}"#));
    }
    Ok((head, body))
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
        let (_cursor, events) = crate::control::since(sent);
        for event in events {
            let seq = event.get("seq").and_then(|value| value.as_u64()).unwrap_or(sent);
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

fn write_response(stream: &mut TcpStream, status: u16, body: &[u8], cors: &str) -> std::io::Result<()> {
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
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Gen-Audio-Stateless: 1\r\n{cors}\r\n",
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

        let seq = crate::control::publish("navigate", &serde_json::json!({"slide": "library", "marker": "ready-stream"}));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(&Server::boot(), stream).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        let req = format!("GET /control/stream?after={}&wait=0 HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n", seq.saturating_sub(1));
        stream.write_all(req.as_bytes()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.contains("text/event-stream"), "{text}");
        assert!(text.contains("ready-stream"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("mcp-session-id"));
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let _ = handle_connection(&Server::boot(), stream);
        });
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(build(port).as_bytes()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        worker.join().unwrap();
        let status = text.split_whitespace().nth(1).and_then(|code| code.parse().ok()).unwrap_or(0);
        (status, text)
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
}
