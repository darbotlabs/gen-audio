//! Optional loopback HTTP transport. One JSON-RPC request per POST. No session id.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use serde_json::Value;

use crate::{handle, Server};

pub fn serve(addr: &str) -> std::io::Result<()> {
    ensure_bind_allowed(addr)?;
    let listener = TcpListener::bind(addr)?;
    let server = Server::boot();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let _ = handle_connection(&server, stream);
            }
            Err(_) => continue,
        }
    }
    Ok(())
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
    let mut buf = vec![0u8; 64 * 1024];
    let mut filled = 0usize;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => {
                filled += n;
                if buf[..filled].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => {
                break;
            }
            Err(err) => return Err(err),
        }
    }
    let text = String::from_utf8_lossy(&buf[..filled]);
    let Some((head, rest)) = text.split_once("\r\n\r\n") else {
        return write_response(&mut stream, 400, br#"{"error":"bad request"}"#);
    };
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if method == "GET" && path == "/health" {
        let body = br#"{"status":"ok","service":"gen-audio-mcp","role":"mcp","stateless":true,"genaidAudioProbed":false}"#;
        return write_response(&mut stream, 200, body);
    }
    if method != "POST" || path != "/mcp" {
        return write_response(&mut stream, 404, br#"{"error":"not found"}"#);
    }
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        return write_response(&mut stream, 400, br#"{"error":"chunked bodies are not accepted"}"#);
    }
    let length = content_length(head).unwrap_or(rest.len());
    if length > 1024 * 1024 {
        return write_response(&mut stream, 413, br#"{"error":"body too large"}"#);
    }
    let mut body = rest.as_bytes().to_vec();
    while body.len() < length && body.len() < 1024 * 1024 {
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let body = &body[..length.min(body.len())];
    let message: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return write_response(&mut stream, 400, br#"{"error":"invalid json"}"#),
    };
    match handle(server, message) {
        Ok(Some(response)) => {
            let bytes = serde_json::to_vec(&response).unwrap_or_else(|_| b"{}".to_vec());
            write_response(&mut stream, 200, &bytes)
        }
        Ok(None) => write_response(&mut stream, 202, b"{}"),
        Err(err) => {
            let payload = format!(r#"{{"error":"{}"}}"#, err.replace('"', "'"));
            write_response(&mut stream, 500, payload.as_bytes())
        }
    }
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

fn write_response(stream: &mut TcpStream, status: u16, body: &[u8]) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Gen-Audio-Stateless: 1\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
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
            .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.contains("gen-audio-mcp"));
        assert!(!text.to_ascii_lowercase().contains("mcp-session-id"));
        assert!(text.contains("X-Gen-Audio-Stateless"));
    }
}
