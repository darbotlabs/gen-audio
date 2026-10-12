//! Authenticated, loopback-only HTTP. Scratch capabilities survive requests, not restarts.
//! CLI HTTP clients must supply GEN_AUDIO_MCP_TOKEN (64 cryptographically random hex digits).
//! Remote binds are disabled even with the old override; use an authenticated TLS proxy.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

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

/// POST one `tools/call` to a running desktop listener and return the tool JSON.
pub fn tools_call(addr: &str, token: &str, name: &str, args: &Value) -> Result<Value, String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": args}
    });
    let response = client_request(addr, token, "/mcp", Some(&payload))?;
    if let Some(message) = response["error"]["message"].as_str() {
        return Err(message.to_string());
    }
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .ok_or("tools/call returned no text")?;
    serde_json::from_str(text).map_err(|err| err.to_string())
}

pub fn get_control(addr: &str, token: &str, after: u64) -> Result<Value, String> {
    client_request(addr, token, &format!("/control?after={after}"), None)
}

fn client_request(addr: &str, token: &str, path: &str, payload: Option<&Value>) -> Result<Value, String> {
    ensure_bind_allowed(addr).map_err(|err| err.to_string())?;
    validate_token(token).map_err(|err| err.to_string())?;
    let addr: SocketAddr = addr.parse().map_err(|_| "invalid listener address")?;
    let body = payload.map(serde_json::to_vec).transpose().map_err(|err| err.to_string())?.unwrap_or_default();
    let method = if payload.is_some() { "POST" } else { "GET" };
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|err| err.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let header = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    write_deadline(&mut stream, header.as_bytes(), deadline).map_err(|err| err.to_string())?;
    write_deadline(&mut stream, &body, deadline).map_err(|err| err.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let n = read_deadline(&mut stream, &mut chunk, deadline).map_err(|err| err.to_string())?;
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 4 * 1024 * 1024 { return Err("MCP response exceeds limit".into()); }
    }
    let text = String::from_utf8_lossy(&buf);
    let Some((head, rest)) = text.split_once("\r\n\r\n") else {
        return Err("mcp response had no body".into());
    };
    if !head.starts_with("HTTP/1.1 200 ") { return Err("authenticated MCP request refused".into()); }
    serde_json::from_str(rest.trim()).map_err(|err| err.to_string())
}

const HEADER_LIMIT: usize = 8192;
const BODY_LIMIT: usize = 1024 * 1024;
const MAX_WORKERS: usize = 8;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const TRUSTED_ORIGINS: &[&str] = ALLOWED_ORIGINS;
type HttpError = (u16, &'static str);

/// Never print or persist this credential. The desktop passes it through child env and IPC.
pub fn generate_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| io::Error::other("secure randomness unavailable"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn validate_token(token: &str) -> io::Result<()> {
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTTP requires GEN_AUDIO_MCP_TOKEN containing 64 cryptographically random hex digits",
        ));
    }
    Ok(())
}

/// Ports whose `handle_connection` should record the `Server` instance id it
/// was given. Keyed by the listener port so parallel tests do not share a
/// list. The id is monotonic, not a pointer: the allocator can reuse an
/// address after the previous Server is dropped. Absent from the release
/// binary: this is `cfg(test)` only.
#[cfg(test)]
static HANDLED_SERVERS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u16, Vec<u64>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(test)]
fn note_handled_server(server: &Server, stream: &TcpStream) {
    let Ok(addr) = stream.local_addr() else { return };
    let Ok(mut watched) = HANDLED_SERVERS.lock() else { return };
    if let Some(ids) = watched.get_mut(&addr.port()) {
        ids.push(server.instance_id);
    }
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
    let method: String = method.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(20).collect();
    let target = target.split('?').next().unwrap_or("-");
    let target: String = target.chars().filter(|c| !c.is_control()).take(200).collect();
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
    const BASELINE: [u16; 5] = [400, 401, 403, 411, 415];

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

fn reject(server: &Server, stream: &mut TcpStream, request: Option<&Request>, err: HttpError, origin: Option<&str>) -> io::Result<()> {
    let (method, target) = request.map(|r| (r.method.as_str(), r.path.as_str())).unwrap_or(("-", "-"));
    log_rejection(stream, method, target, &err.0.to_string(), err.1);
    server.rejections.note(method, target, err.0);
    error_response(stream, err, origin)
}

fn library_rel(path: &str) -> Option<&str> {
    let path = path.split('?').next().unwrap_or(path);
    let rel = path.strip_prefix("/library/")?;
    if rel.is_empty() || rel.ends_with('/') {
        return None;
    }
    Some(rel)
}

fn write_library(server: &Server, stream: &mut TcpStream, request: &Request, rel: &str) -> io::Result<()> {
    let origin = request.header("origin");
    let Ok((path, mime)) = gen_audio_core::library_store::open_library_media(rel) else {
        return reject(server, stream, Some(request), (404, "library media not found"), origin);
    };
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    if size > 192 * 1024 * 1024 {
        return reject(server, stream, Some(request), (413, "library media too large"), origin);
    }
    stream_file(stream, &mut file, size, mime, request)
}

pub fn serve(addr: &str) -> io::Result<()> {
    let token = std::env::var("GEN_AUDIO_MCP_TOKEN").unwrap_or_default();
    validate_token(&token)?;
    gen_audio_core::paths::reap_stale_mcp_addr();
    let listener = bind_listener(addr)?;
    let _ = gen_audio_core::paths::publish_mcp_addr(&listener.local_addr()?.to_string());
    accept_loop(listener, token);
    Ok(())
}

/// Bind our own listener; never attach to a process already listening on this address.
pub fn spawn_loopback(addr: &str, token: &str) -> io::Result<SocketAddr> {
    validate_token(token)?;
    let listener = bind_listener(addr)?;
    let bound = listener.local_addr()?;
    let token = token.to_owned();
    std::thread::Builder::new()
        .name("gen-audio-mcp".into())
        .spawn(move || accept_loop(listener, token))?;
    Ok(bound)
}

pub fn initialize_handshake(addr: &str, token: &str) -> Result<Value, String> {
    handshake(addr, token, None)
}

/// The boot nonce is passed only in the owned child's environment, never in the
/// request. A stale/foreign service cannot pass by ignoring Authorization.
pub fn initialize_owned_handshake(
    addr: &str,
    token: &str,
    boot_nonce: &str,
) -> Result<Value, String> {
    validate_token(boot_nonce).map_err(|err| err.to_string())?;
    handshake(addr, token, Some(boot_nonce))
}

fn handshake(addr: &str, token: &str, boot_nonce: Option<&str>) -> Result<Value, String> {
    ensure_bind_allowed(addr).map_err(|err| err.to_string())?;
    validate_token(token).map_err(|err| err.to_string())?;
    let addr: SocketAddr = addr.parse().map_err(|_| "invalid listener address")?;
    let body = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "gen-audio-desktop", "version": env!("CARGO_PKG_VERSION")}}
    }))
    .map_err(|err| err.to_string())?;
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|err| err.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let header = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    write_deadline(&mut stream, header.as_bytes(), deadline).map_err(|e| e.to_string())?;
    write_deadline(&mut stream, &body, deadline).map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let n = read_deadline(&mut stream, &mut chunk, deadline).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..n]);
        if response.len() > 64 * 1024 {
            return Err("handshake response exceeds limit".into());
        }
    }
    let text = std::str::from_utf8(&response).map_err(|_| "invalid handshake response")?;
    let (head, rest) = text
        .split_once("\r\n\r\n")
        .ok_or("handshake response had no body")?;
    if !head.starts_with("HTTP/1.1 200 ") {
        return Err("authenticated handshake refused".into());
    }
    if let Some(expected) = boot_nonce {
        let actual = head
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("x-gen-audio-boot-nonce"))
            .map(|(_, value)| value.trim());
        if actual != Some(expected) {
            return Err("listener did not prove owned sidecar startup".into());
        }
    }
    let value: Value = serde_json::from_str(rest).map_err(|_| "invalid handshake JSON")?;
    if value["result"]["protocolVersion"] != "2025-03-26"
        || value["result"]["serverInfo"]["name"] != "gen-audio"
    {
        return Err("unexpected handshake protocol or server".into());
    }
    Ok(value)
}

fn bind_listener(addr: &str) -> io::Result<TcpListener> {
    ensure_bind_allowed(addr)?;
    TcpListener::bind(addr)
}

pub fn ensure_bind_allowed(addr: &str) -> io::Result<()> {
    let socket: SocketAddr = addr
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid socket address"))?;
    if !socket.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "remote HTTP is disabled; use an authenticated TLS proxy to the loopback listener",
        ));
    }
    Ok(())
}

struct WorkerPermit(Arc<AtomicUsize>);
impl WorkerPermit {
    fn acquire(active: &Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |n| {
                (n < MAX_WORKERS).then_some(n + 1)
            })
            .ok()?;
        Some(Self(Arc::clone(active)))
    }
}
impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

fn accept_loop(listener: TcpListener, token: String) {
    let shared = Arc::new(Server::boot());
    let token = Arc::new(token);
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        // Do not block the accept thread writing a rejection to an unresponsive peer.
        let Some(permit) = WorkerPermit::acquire(&active) else {
            drop(stream);
            continue;
        };
        let shared = Arc::clone(&shared);
        let token = Arc::clone(&token);
        let _ = std::thread::Builder::new()
            .name("mcp-http-worker".into())
            .spawn(move || {
                let _permit = permit;
                let _ = handle_until(&shared, stream, &token, deadline);
            });
    }
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    length: usize,
}
impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

fn header_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn parse_head(head: &[u8]) -> Result<Request, HttpError> {
    let head = std::str::from_utf8(head).map_err(|_| (400, "invalid header encoding"))?;
    let mut lines = head.split("\r\n");
    let first = lines.next().ok_or((400, "missing request line"))?;
    let parts: Vec<_> = first.split(' ').collect();
    if parts.len() != 3
        || !header_token(parts[0])
        || parts[2] != "HTTP/1.1"
        || !parts[1].starts_with('/')
        || parts[1].starts_with("//")
        || !parts[1].bytes().all(|b| b.is_ascii_graphic() && b != b'#')
    {
        return Err((400, "invalid request line"));
    }
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or((400, "invalid header"))?;
        if !header_token(name) || !value.bytes().all(|b| b == b'\t' || (32..127).contains(&b)) {
            return Err((400, "invalid header"));
        }
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err((400, "duplicate header"));
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err((400, "transfer-encoding is not supported"));
    }
    if headers.contains_key("expect") {
        return Err((417, "expect is not supported"));
    }
    let length = match headers.get("content-length") {
        Some(raw) if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) => {
            raw.parse::<usize>().map_err(|_| (413, "body too large"))?
        }
        Some(_) => return Err((400, "invalid content-length")),
        None if parts[0] == "POST" => return Err((411, "content-length required")),
        None => 0,
    };
    if length > BODY_LIMIT {
        return Err((413, "body too large"));
    }
    if parts[0] != "POST" && length != 0 {
        return Err((400, "unexpected request body"));
    }
    Ok(Request {
        method: parts[0].into(),
        path: parts[1].into(),
        headers,
        length,
    })
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "HTTP deadline exceeded"))
}
fn read_deadline(stream: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> io::Result<usize> {
    stream.set_read_timeout(Some(remaining(deadline)?))?;
    stream.read(buf)
}
fn write_deadline(stream: &mut TcpStream, mut buf: &[u8], deadline: Instant) -> io::Result<()> {
    while !buf.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        let n = stream.write(buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "HTTP write closed",
            ));
        }
        buf = &buf[n..];
    }
    Ok(())
}
fn read_error(err: io::Error) -> HttpError {
    if matches!(
        err.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        (408, "request deadline exceeded")
    } else {
        (400, "incomplete request")
    }
}
fn read_head(stream: &mut TcpStream, deadline: Instant) -> Result<(Request, Vec<u8>), (HttpError, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0; 4096];
    let failure = |error, bytes: &[u8]| {
        let line = bytes.split(|byte| *byte == b'\r' || *byte == b'\n').next().unwrap_or_default();
        (error, String::from_utf8_lossy(&line[..line.len().min(200)]).into_owned())
    };
    loop {
        let n = read_deadline(stream, &mut chunk, deadline).map_err(|e| failure(read_error(e), &buf))?;
        if n == 0 {
            return Err(failure((400, "incomplete headers"), &buf));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if end + 4 > HEADER_LIMIT {
                return Err(failure((431, "headers too large"), &buf));
            }
            let request = parse_head(&buf[..end]).map_err(|e| failure(e, &buf))?;
            let body = buf[end + 4..].to_vec();
            if body.len() > request.length {
                return Err(failure((400, "unexpected trailing bytes"), &buf));
            }
            return Ok((request, body));
        }
        if buf.len() >= HEADER_LIMIT {
            return Err(failure((431, "headers too large"), &buf));
        }
    }
}

fn authorize_boundary(request: &Request, local: SocketAddr) -> Result<(), HttpError> {
    let host = request.header("host").ok_or((400, "host required"))?;
    if host != local.to_string() && host != format!("localhost:{}", local.port()) {
        return Err((403, "untrusted host"));
    }
    if request
        .header("origin")
        .is_some_and(|origin| !TRUSTED_ORIGINS.contains(&origin))
    {
        return Err((403, "untrusted origin"));
    }
    Ok(())
}
fn authorized(request: &Request, token: &str) -> bool {
    let Some(given) = request
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    if given.len() != token.len() {
        return false;
    }
    given
        .bytes()
        .zip(token.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
fn route_method(path: &str) -> Option<&'static str> {
    match path.split('?').next()? {
        "/mcp" if path == "/mcp" => Some("POST"),
        "/health" | "/ready" | "/control" | "/control/stream" => Some("GET"),
        p if p.starts_with("/artifacts/") || p.starts_with("/library/") => Some("GET"),
        _ => None,
    }
}
fn preflight(request: &Request) -> Result<(), HttpError> {
    if request.header("origin").is_none() {
        return Err((403, "preflight requires trusted origin"));
    }
    let method = request
        .header("access-control-request-method")
        .ok_or((400, "preflight method required"))?;
    if route_method(&request.path) != Some(method) {
        return Err((405, "preflight method not allowed"));
    }
    let headers: Vec<_> = request
        .header("access-control-request-headers")
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    if !headers.iter().any(|s| s == "authorization")
        || headers
            .iter()
            .any(|s| !matches!(s.as_str(), "authorization" | "content-type" | "range"))
    {
        return Err((403, "preflight headers not allowed"));
    }
    Ok(())
}
fn json_content_type(value: Option<&str>) -> bool {
    let mut parts = value.unwrap_or("").split(';').map(str::trim);
    parts
        .next()
        .is_some_and(|v| v.eq_ignore_ascii_case("application/json"))
        && parts.all(|p| p.eq_ignore_ascii_case("charset=utf-8"))
}

pub fn handle_connection(server: &Server, stream: TcpStream, token: &str) -> io::Result<()> {
    handle_until(server, stream, token, Instant::now() + REQUEST_TIMEOUT)
}
fn handle_until(
    server: &Server,
    mut stream: TcpStream,
    token: &str,
    deadline: Instant,
) -> io::Result<()> {
    validate_token(token)?;
    #[cfg(test)]
    note_handled_server(server, &stream);
    let (request, mut body) = match read_head(&mut stream, deadline) {
        Ok(parts) => parts,
        Err((err, line)) => {
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or("-");
            let target = parts.next().unwrap_or("-");
            log_rejection(&stream, method, target, &err.0.to_string(), "read");
            server.rejections.note(method, target, err.0);
            return error_response(&mut stream, err, None);
        },
    };
    if let Err(err) = authorize_boundary(&request, stream.local_addr()?) {
        return reject(server, &mut stream, Some(&request), err, None);
    }
    let origin = request.header("origin");
    if request.method == "OPTIONS" {
        return match preflight(&request) {
            Ok(()) => response(&mut stream, 204, b"", origin),
            Err(err) => reject(server, &mut stream, Some(&request), err, origin),
        };
    }
    // Origin is a browser boundary, never a native credential. Every route needs a token.
    if !authorized(&request, token) {
        return reject(server, &mut stream, Some(&request), (401, "bearer token required"), origin);
    }
    if route_method(&request.path) != Some(request.method.as_str()) {
        return reject(server, &mut stream, Some(&request), (404, "route not found"), origin);
    }
    if request.method == "POST" && !json_content_type(request.header("content-type")) {
        return reject(server, &mut stream, Some(&request), (415, "application/json required"), origin);
    }
    let mut chunk = [0; 4096];
    while body.len() < request.length {
        let n = match read_deadline(&mut stream, &mut chunk, deadline) {
            Ok(0) => return reject(server, &mut stream, Some(&request), (400, "incomplete body"), origin),
            Ok(n) => n,
            Err(err) => return reject(server, &mut stream, Some(&request), read_error(err), origin),
        };
        if n > request.length - body.len() {
            return reject(server, &mut stream, Some(&request), (400, "unexpected trailing bytes"), origin);
        }
        body.extend_from_slice(&chunk[..n]);
    }
    if remaining(deadline).is_err() {
        return reject(server, &mut stream, Some(&request), (408, "request deadline exceeded"), origin);
    }
    match request.path.split('?').next().unwrap_or("") {
        "/health" => {
            let (rejected, last_rejected) = server.rejections.snapshot();
            let body = serde_json::to_vec(&serde_json::json!({
                "status":"ok","service":"gen-audio-mcp","role":"mcp","stateless":true,
                "genaidAudioProbed":false,"rejected":rejected,"last_rejected":last_rejected
            }))?;
            response(&mut stream, 200, &body, origin)
        },
        "/ready" => response(&mut stream, 200, br#"{"ready":true,"service":"gen-audio-mcp","stateless":true,"speech":false,"modelsLoaded":false,"note":"Listener readiness only; not loaded TTS readiness."}"#, origin),
        "/control" => {
            let delta = crate::control::since(query_u64(&request.path, "after").unwrap_or(0));
            let bytes = serde_json::to_vec(&serde_json::json!({
                "stateless":true,"speech":false,"cursor":delta.cursor,"events":delta.events,
                "gap":delta.gap,"oldest":delta.oldest,"resync":"viewport_get"
            }))?;
            response(&mut stream, 200, &bytes, origin)
        }
        "/control/stream" => write_control_stream(&mut stream, &request),
        path if path.starts_with("/artifacts/") => artifact(&mut stream, server, &request),
        path if library_rel(path).is_some() => write_library(server, &mut stream, &request, library_rel(path).unwrap()),
        "/mcp" => {
            let message: Value = match serde_json::from_slice(&body) {
                Ok(value) => value,
                Err(_) => return reject(server, &mut stream, Some(&request), (400, "invalid JSON"), origin),
            };
            let method = message["method"].as_str().unwrap_or("").to_owned();
            let tool = message.pointer("/params/name").and_then(Value::as_str).unwrap_or("").to_owned();
            match handle(server, message) {
                Ok(Some(value)) => {
                    if let Some(code) = value.pointer("/error/code") {
                        log_rejection(&stream, &request.method, &format!("{method} {tool}"), &code.to_string(), "rpc");
                    }
                    response(&mut stream, 200, &serde_json::to_vec(&value)?, origin)
                },
                Ok(None) => response(&mut stream, 202, b"{}", origin),
                Err(_) => reject(server, &mut stream, Some(&request), (500, "tool dispatch failed"), origin),
            }
        }
        _ => reject(server, &mut stream, Some(&request), (404, "not found"), origin),
    }
}

fn query_u64(path: &str, key: &str) -> Option<u64> {
    path.split_once('?')?.1.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.parse().ok()).flatten()
    })
}
fn write_control_stream(stream: &mut TcpStream, request: &Request) -> io::Result<()> {
    let after = query_u64(&request.path, "after").unwrap_or(0);
    let wait = query_u64(&request.path, "wait").unwrap_or(0).min(2000);
    let started = Instant::now();
    let delta = loop {
        let delta = crate::control::since(after);
        if delta.gap || !delta.events.is_empty() || started.elapsed() >= Duration::from_millis(wait) {
            break delta;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut body = String::new();
    if delta.gap {
        let data = serde_json::json!({"gap":true,"after":after,"oldest":delta.oldest,"cursor":delta.cursor,"resync":"viewport_get"});
        body.push_str(&format!("event: gap\ndata: {data}\n\n"));
    }
    for event in delta.events.into_iter().filter(|_| !delta.gap) {
        let seq = event["seq"].as_u64().unwrap_or(after);
        body.push_str(&format!("id: {seq}\nevent: control\ndata: {event}\n\n"));
    }
    body.push_str("event: end\ndata: {\"stateless\":true,\"speech\":false}\n\n");
    let deadline = Instant::now() + WRITE_TIMEOUT;
    write_header(
        stream,
        200,
        body.len() as u64,
        "text/event-stream",
        request.header("origin"),
        "",
        deadline,
    )?;
    write_deadline(stream, body.as_bytes(), deadline)
}

fn byte_range(value: Option<&str>, size: u64) -> Result<(u64, u64, bool), ()> {
    let Some(value) = value else {
        return Ok((0, size, false));
    };
    let (start, end) = value
        .strip_prefix("bytes=")
        .ok_or(())?
        .split_once('-')
        .ok_or(())?;
    let number = |s: &str| -> Result<u64, ()> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(());
        }
        s.parse().map_err(|_| ())
    };
    if size == 0 {
        return Err(());
    }
    if start.is_empty() {
        let length = number(end)?.min(size);
        if length == 0 {
            return Err(());
        }
        return Ok((size - length, length, true));
    }
    let start = number(start)?;
    let end = if end.is_empty() {
        size - 1
    } else {
        number(end)?.min(size - 1)
    };
    if start >= size || end < start {
        return Err(());
    }
    Ok((start, end - start + 1, true))
}
fn artifact(stream: &mut TcpStream, server: &Server, request: &Request) -> io::Result<()> {
    let origin = request.header("origin");
    let name = request.path.strip_prefix("/artifacts/").unwrap_or("");
    if name.is_empty()
        || name.len() > 255
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        || !(name.ends_with(".wav") || name.ends_with(".json"))
    {
        return error_response(stream, (404, "artifact not found"), origin);
    }
    let file = server
        .scratch
        .open_input(name)
        .ok()
        .and_then(|path| std::fs::File::open(path).ok());
    let Some(mut file) = file else {
        return error_response(stream, (404, "artifact not found"), origin);
    };
    let meta = file.metadata()?;
    let size = meta.len();
    let wav = name.ends_with(".wav");
    if !meta.is_file()
        || size
            > if wav {
                128 * 1024 * 1024
            } else {
                BODY_LIMIT as u64
            }
    {
        return error_response(stream, (413, "artifact exceeds download limit"), origin);
    }
    stream_file(stream, &mut file, size, if wav { "audio/wav" } else { "application/json" }, request)
}

fn stream_file(stream: &mut TcpStream, file: &mut std::fs::File, size: u64, mime: &str, request: &Request) -> io::Result<()> {
    let origin = request.header("origin");
    let (start, length, partial) = match byte_range(request.header("range"), size) {
        Ok(range) => range,
        Err(()) => {
            return write_header(
                stream,
                416,
                0,
                "application/json",
                origin,
                &format!("Content-Range: bytes */{size}\r\n"),
                Instant::now() + WRITE_TIMEOUT,
            )
        }
    };
    let mut extra = "Accept-Ranges: bytes\r\n".to_owned();
    if partial {
        extra.push_str(&format!(
            "Content-Range: bytes {start}-{}/{size}\r\n",
            start + length - 1
        ));
    }
    let deadline = Instant::now() + WRITE_TIMEOUT;
    write_header(
        stream,
        if partial { 206 } else { 200 },
        length,
        mime,
        origin,
        &extra,
        deadline,
    )?;
    file.seek(SeekFrom::Start(start))?;
    let mut left = length;
    let mut buf = [0; 64 * 1024];
    while left > 0 {
        let take = left.min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..take])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "artifact truncated",
            ));
        }
        write_deadline(stream, &buf[..n], deadline)?;
        left -= n as u64;
    }
    Ok(())
}

fn error_response(
    stream: &mut TcpStream,
    (status, message): HttpError,
    origin: Option<&str>,
) -> io::Result<()> {
    response(
        stream,
        status,
        &serde_json::to_vec(&serde_json::json!({"error":message}))?,
        origin,
    )
}
fn response(
    stream: &mut TcpStream,
    status: u16,
    body: &[u8],
    origin: Option<&str>,
) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut extra = String::new();
    if status == 200 {
        if let Ok(nonce) = std::env::var("GEN_AUDIO_MCP_BOOT_NONCE") {
            if validate_token(&nonce).is_ok() {
                extra = format!("X-Gen-Audio-Boot-Nonce: {nonce}\r\n");
            }
        }
    }
    write_header(
        stream,
        status,
        body.len() as u64,
        "application/json",
        origin,
        &extra,
        deadline,
    )?;
    write_deadline(stream, body, deadline)
}
fn write_header(
    stream: &mut TcpStream,
    status: u16,
    length: u64,
    mime: &str,
    origin: Option<&str>,
    extra: &str,
    deadline: Instant,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    };
    let mut cors = String::new();
    if let Some(origin) = origin.filter(|o| TRUSTED_ORIGINS.contains(o)) {
        cors = format!("Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: authorization, content-type, range\r\nAccess-Control-Expose-Headers: Content-Range, Accept-Ranges\r\n");
    }
    let auth = if status == 401 {
        "WWW-Authenticate: Bearer\r\n"
    } else {
        ""
    };
    let head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {length}\r\nConnection: close\r\nCache-Control: no-store\r\nVary: Origin\r\nX-Content-Type-Options: nosniff\r\nX-Gen-Audio-Stateless: 1\r\n{cors}{auth}{extra}\r\n");
    write_deadline(stream, head.as_bytes(), deadline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Shutdown;
    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn exchange(server: Arc<Server>, build: impl FnOnce(SocketAddr) -> Vec<u8>) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(&server, stream, TOKEN).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(&build(addr)).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut bytes = Vec::new();
        if let Err(err) = stream.read_to_end(&mut bytes) {
            // Rejecting oversized headers may close with unread bytes on Windows.
            assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
        }
        worker.join().unwrap();
        bytes
    }
    fn server() -> Arc<Server> {
        Arc::new(Server::isolated())
    }
    fn clean(server: Arc<Server>) {
        std::fs::remove_dir_all(&server.scratch.dir).unwrap();
    }
    fn status(bytes: &[u8], code: u16) {
        assert!(
            bytes.starts_with(format!("HTTP/1.1 {code} ").as_bytes()),
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
    fn get(server: &Arc<Server>, path: &str, extra: &str) -> Vec<u8> {
        exchange(Arc::clone(server), |addr| {
            format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\n{extra}\r\n").into_bytes()
        })
    }

    #[test]
    fn rejects_untrusted_requests_before_dispatch() {
        let s = server();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fixture_tone","arguments":{}}}"#;
        for (host, origin, auth, mime, code) in [
            ("evil.example", "", TOKEN, "application/json", 403),
            ("127.0.0.1", "", TOKEN, "application/json", 403),
            ("localhost.evil:8765", "", TOKEN, "application/json", 403),
            ("@local", "null", TOKEN, "application/json", 403),
            (
                "@local",
                "https://evil.example",
                TOKEN,
                "application/json",
                403,
            ),
            (
                "@local",
                "http://localhost:1420.evil",
                TOKEN,
                "application/json",
                403,
            ),
            (
                "@local",
                "http://localhost:1420/",
                TOKEN,
                "application/json",
                403,
            ),
            ("@local", "", "", "application/json", 401),
            (
                "@local",
                "http://localhost:1420",
                "",
                "application/json",
                if cfg!(debug_assertions) { 401 } else { 403 },
            ),
            ("@local", "", "wrong", "application/json", 401),
            ("@local", "", TOKEN, "text/plain", 415),
            ("@local", "", TOKEN, "", 415),
            (
                "@local",
                "",
                TOKEN,
                "application/x-www-form-urlencoded",
                415,
            ),
        ] {
            let bytes = exchange(Arc::clone(&s), |addr| {
                let host = if host == "@local" {
                    addr.to_string()
                } else {
                    host.into()
                };
                let origin = if origin.is_empty() {
                    String::new()
                } else {
                    format!("Origin: {origin}\r\n")
                };
                format!("POST /mcp HTTP/1.1\r\nHost: {host}\r\n{origin}Authorization: Bearer {auth}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
            });
            status(&bytes, code);
            assert!(!String::from_utf8_lossy(&bytes).contains("Allow-Origin: *"));
            assert!(!s.scratch.dir.join("fixture-tone.wav").exists());
        }
        clean(s);
    }
    #[test]
    fn malformed_framing_and_limits() {
        let s = server();
        for (headers, body, code) in [
            ("Content-Length: 2\r\nContent-Length: 2\r\n", "{}", 400),
            ("Content-Length: 2\r\ncontent-length: 3\r\n", "{}", 400),
            (
                "Content-Length: 2\r\nTransfer-Encoding: chunked\r\n",
                "{}",
                400,
            ),
            (
                "Transfer-Encoding: chunked\r\nTransfer-Encoding: identity\r\n",
                "",
                400,
            ),
            ("Transfer-Encoding: identity\r\n", "", 400),
            ("Content-Length : 2\r\n", "{}", 400),
            ("Content-Length: +2\r\n", "{}", 400),
            ("Content-Length: 2,2\r\n", "{}", 400),
            ("Content-Length: 2\r\n", "{}extra", 400),
            ("Content-Length: 5\r\n", "{}", 400),
            ("Content-Length: 1048577\r\n", "", 413),
            ("Content-Length: nope\r\n", "", 400),
            ("", "", 411),
            ("Content-Length: 0\r\nHost: second\r\n", "", 400),
            (
                "Content-Length: 0\r\nOrigin: null\r\nOrigin: http://localhost:1420\r\n",
                "",
                400,
            ),
            (
                "Content-Length: 0\r\nAuthorization: Bearer duplicate\r\n",
                "",
                400,
            ),
            ("Content-Length: 1\r\n", "{", 400),
        ] {
            status(
                &exchange(Arc::clone(&s), |addr| {
                    format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n{headers}\r\n{body}").into_bytes()
                }),
                code,
            );
        }
        status(
            &exchange(Arc::clone(&s), |addr| {
                format!(
                    "GET /health HTTP/1.1\r\nHost: {addr}\r\nX-Pad: {}\r\n\r\n",
                    "x".repeat(HEADER_LIMIT)
                )
                .into_bytes()
            }),
            431,
        );
        assert!(parse_head(b"GET /health HTTP/1.0\r\nHost: localhost").is_err());
        assert!(parse_head(b"GET http://localhost/health HTTP/1.1").is_err());
        assert!(parse_head(b"GET /health HTTP/1.1\r\n Bad: fold").is_err());
        status(
            &exchange(Arc::clone(&s), |_| b"GET /health HTTP/1.1\r\n\r\n".to_vec()),
            400,
        );
        clean(s);
    }
    #[test]
    fn health_control_and_preflight_use_one_boundary() {
        let s = server();
        for path in [
            "/health",
            "/ready",
            "/control",
            "/control/stream?wait=0",
            "/artifacts/test.wav",
        ] {
            status(
                &exchange(Arc::clone(&s), |addr| {
                    format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n").into_bytes()
                }),
                401,
            );
        }
        status(&get(&s, "/health", ""), 200);
        status(&get(&s, "/ready", ""), 200);
        status(&get(&s, "/control", ""), 200);
        status(&get(&s, "/control/stream?wait=0", ""), 200);
        for origin in TRUSTED_ORIGINS {
            let bytes = exchange(Arc::clone(&s), |addr| {
                format!("OPTIONS /mcp HTTP/1.1\r\nHost: localhost:{}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization,content-type\r\n\r\n", addr.port()).into_bytes()
            });
            status(&bytes, 204);
            assert!(String::from_utf8_lossy(&bytes)
                .contains(&format!("Access-Control-Allow-Origin: {origin}\r\n")));
        }
        status(
            &exchange(Arc::clone(&s), |addr| {
                format!("OPTIONS /mcp HTTP/1.1\r\nHost: {addr}\r\nOrigin: null\r\nAccess-Control-Request-Method: POST\r\n\r\n").into_bytes()
            }),
            403,
        );
        clean(s);
    }
    #[test]
    fn authorized_tools_and_issued_artifacts_only() {
        let s = server();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fixture_tone","arguments":{}}}"#;
        status(
            &exchange(Arc::clone(&s), |addr| {
                format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://tauri.localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
            }),
            200,
        );
        let bytes = get(&s, "/artifacts/fixture-tone.wav", "");
        status(&bytes, 200);
        let start = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert!(bytes[start..].starts_with(b"RIFF"));
        let slice = get(&s, "/artifacts/fixture-tone.wav", "Range: bytes=0-3\r\n");
        status(&slice, 206);
        assert!(slice.ends_with(b"RIFF"));
        let size = (bytes.len() - start) as u64;
        assert_eq!(byte_range(Some("bytes=-4"), size), Ok((size - 4, 4, true)));
        status(
            &get(
                &s,
                "/artifacts/fixture-tone.wav",
                "Range: bytes=999999999-\r\n",
            ),
            416,
        );
        status(
            &get(
                &s,
                "/artifacts/fixture-tone.wav",
                "Range: bytes=0-1,2-3\r\n",
            ),
            416,
        );
        let private = s
            .scratch
            .write_private_input(b"private script", ".json")
            .unwrap();
        let private_name = private.file_name().unwrap().to_str().unwrap();
        std::fs::write(s.scratch.dir.join("unissued.json"), b"{}").unwrap();
        for name in [
            private_name,
            "unissued.json",
            "../Cargo.toml",
            "%2e%2e%2fsecret.json",
            "image.png",
        ] {
            status(&get(&s, &format!("/artifacts/{name}"), ""), 404);
        }
        let manifest = s.scratch.prepare_output("issued.json").unwrap();
        std::fs::write(manifest, b"{\"ok\":true}").unwrap();
        status(&get(&s, "/artifacts/issued.json", ""), 200);
        clean(s);
    }
    #[test]
    fn tokens_and_remote_binding_fail_closed() {
        assert!(ensure_bind_allowed("0.0.0.0:8765").is_err());
        assert!(ensure_bind_allowed("[::]:8765").is_err());
        assert!(spawn_loopback("127.0.0.1:0", "").is_err());
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_ne!(a, b);
        validate_token(&a).unwrap();
        let addr = spawn_loopback("127.0.0.1:0", &a).unwrap();
        assert!(initialize_handshake(&addr.to_string(), &b).is_err());
        assert!(initialize_handshake(&addr.to_string(), &a).is_ok());
        assert!(initialize_owned_handshake(&addr.to_string(), &a, &b).is_err());
    }
    #[test]
    fn active_workers_are_bounded_and_released() {
        let active = Arc::new(AtomicUsize::new(0));
        let mut permits: Vec<_> = (0..MAX_WORKERS)
            .map(|_| WorkerPermit::acquire(&active).unwrap())
            .collect();
        assert!(WorkerPermit::acquire(&active).is_none());
        permits.pop();
        assert!(WorkerPermit::acquire(&active).is_some());
        drop(permits);
        assert_eq!(active.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn slow_headers_hit_absolute_deadline() {
        let s = server();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::clone(&s);
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_until(
                &shared,
                stream,
                TOKEN,
                Instant::now() + Duration::from_millis(100),
            )
            .unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(b"GET /health HTTP/1.1\r\nHost: ").unwrap();
        let started = Instant::now();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        status(&response, 408);
        assert!(started.elapsed() < Duration::from_secs(2));
        worker.join().unwrap();
        clean(s);
    }

    /// GET /health on a listener that serves every connection with one shared Server.
    fn health_of(port: u16) -> Value {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n").as_bytes())
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
                let _ = handle_connection(&server, stream.unwrap(), TOKEN);
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
        assert_eq!(before["rejected"], serde_json::json!({"400": 0, "401": 0, "403": 0, "411": 0, "415": 0}), "{before}");
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
        assert_eq!(after_415["rejected"], serde_json::json!({"400": 0, "401": 0, "403": 1, "411": 0, "415": 1}), "{after_415}");
        assert_eq!(after_415["last_rejected"]["status"], 415);
        assert_eq!(after_415["last_rejected"]["path"], "/mcp");
        let text = after_415.to_string();
        for leaked in [marker, "header-marker", "evil.example", "text/plain"] {
            assert!(!text.contains(leaked), "/health must not echo request bodies or headers ({leaked}): {text}");
        }
        assert_eq!(after_415["status"], "ok");
    }

    #[test]
    fn authenticated_accept_loop_shares_rejection_stats_across_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || accept_loop(listener, TOKEN.into()));
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
        // Instance ids, not addresses. A fresh Server::boot() per connection
        // frees the previous one, and the allocator can hand that address out
        // again, so a pointer compare stays green.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        super::HANDLED_SERVERS.lock().unwrap().insert(port, Vec::new());
        std::thread::spawn(move || accept_loop(listener, TOKEN.into()));
        let _ = health_of(port);
        let _ = health_of(port);
        let ids = super::HANDLED_SERVERS.lock().unwrap().remove(&port).unwrap_or_default();
        assert_eq!(ids.len(), 2, "expected one server id per connection, got {ids:?}");
        assert_eq!(ids[0], ids[1], "local connections were given different Server instances: {ids:?}");
    }

    #[test]
    fn readiness_and_control_stream_are_stateless() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(&Server::boot(), stream, TOKEN).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(format!("GET /ready HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n").as_bytes())
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
            handle_connection(&Server::boot(), stream, TOKEN).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        let req = format!(
            "GET /control/stream?after={}&wait=0 HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n",
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
            .env("GEN_AUDIO_MCP_TOKEN", TOKEN)
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
        let addr = spawn_loopback("127.0.0.1:0", TOKEN).unwrap();
        let value = initialize_handshake(&addr.to_string(), TOKEN).unwrap();
        assert_eq!(value["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(value["result"]["serverInfo"]["name"], "gen-audio");
    }

    #[test]
    fn body_reads_and_writes_obey_absolute_deadlines() {
        let s = server();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::clone(&s);
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_until(
                &shared,
                stream,
                TOKEN,
                Instant::now() + Duration::from_millis(100),
            )
            .unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: 200\r\n\r\n{{").as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        status(&response, 408);
        worker.join().unwrap();
        assert!(
            write_deadline(&mut stream, b"x", Instant::now() - Duration::from_millis(1)).is_err()
        );
        clean(s);
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
            let _ = handle_connection(&Server::boot(), stream, TOKEN);
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
        format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOKEN}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
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
                "OPTIONS /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization, content-type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
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
            "OPTIONS /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: keep-alive\r\nAccept: */*\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization, content-type\r\nOrigin: http://tauri.localhost\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36 Edg/154.0.0.0\r\nSec-Fetch-Mode: cors\r\nSec-Fetch-Site: cross-site\r\nSec-Fetch-Dest: empty\r\nReferer: http://tauri.localhost/\r\nAccept-Encoding: gzip, deflate, br, zstd\r\nAccept-Language: en-US,en;q=0.9\r\n\r\n"
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
             "rejected OPTIONS /mcp 403 untrusted origin"),
            (Box::new(|port| post(port, "Content-Type: text/plain\r\n", NAVIGATE)), "rejected POST /mcp 415 application/json required"),
            (Box::new(|port| post(port, "Content-Type: application/json\r\n", NAVIGATE).replace(&format!("Host: 127.0.0.1:{port}"), "Host: evil.example")),
             "rejected POST /mcp 403 untrusted host"),
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
            format!("GET /library/missing.wav HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n")
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
        std::fs::File::create(&wav).unwrap().set_len(192 * 1024 * 1024 + 1).unwrap();
        let library = dir.clone();
        let (status, text, peer, served) = roundtrip_on_worker(
            |port| {
                format!("GET /library/big.wav HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n")
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
            handle_connection(&Server::boot(), stream, TOKEN).unwrap();
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        let req = format!("GET /control/stream?after={anchor}&wait=0 HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.contains("event: gap"), "{text}");
        assert!(text.contains("viewport_get"), "{text}");
    }
}
