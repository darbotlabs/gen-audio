//! The MCP sidecar's stderr, kept: a size-capped rotating file in the Tauri
//! app_log_dir (gen-audio-mcp.log, .log.1, .log.2; 1 MiB each).
//!
//! The sidecar writes one stderr line per rejected request (method, path,
//! status, reason, peer) and its startup errors. Before this, the desktop
//! spawned it with stderr set to null, so on a user's machine those lines went
//! nowhere: a diagnostic written to a stream nobody can read is the same as no
//! diagnostic.
//!
//! Every line is redacted before it is written: no request bodies, no header
//! values, no query strings, nothing token-shaped. In-crate rotation, no new
//! dependency.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// File name of the current log in the app log dir.
pub const LOG_NAME: &str = "gen-audio-mcp.log";
/// Bytes per file before it rotates.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Files kept: the current one plus `.1` and `.2`.
pub const FILES: usize = 3;
/// Characters of one redacted line that are kept.
pub const MAX_LINE: usize = 512;
/// Bytes read from the pipe per line at most (a longer line is split).
const MAX_READ: u64 = 4096;

/// `gen-audio-mcp.log` for index 0, else `gen-audio-mcp.log.<index>`.
pub fn log_path(dir: &Path, index: usize) -> PathBuf {
    if index == 0 {
        dir.join(LOG_NAME)
    } else {
        dir.join(format!("{LOG_NAME}.{index}"))
    }
}

/// Append-only log that rotates before a write would take the current file
/// past `max_bytes`, keeping `files` files in all (so at most
/// `files * max_bytes` bytes on disk).
pub struct RotatingLog {
    dir: PathBuf,
    max_bytes: u64,
    files: usize,
    file: Option<File>,
    size: u64,
}

impl RotatingLog {
    pub fn open(dir: &Path, max_bytes: u64, files: usize) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = OpenOptions::new().create(true).append(true).open(log_path(dir, 0))?;
        let size = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            max_bytes,
            files: files.max(1),
            file: Some(file),
            size,
        })
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let max = usize::try_from(self.max_bytes.saturating_sub(1)).unwrap_or(usize::MAX);
        let line = cut(line, max);
        let bytes = line.len() as u64 + 1;
        if self.size > 0 && self.size + bytes > self.max_bytes {
            self.rotate()?;
        }
        let file = match self.file.as_mut() {
            Some(file) => file,
            None => return Err(io::Error::other("log file closed")),
        };
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        self.size += bytes;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Windows cannot rename an open file: close it first.
        self.file = None;
        let _ = fs::remove_file(log_path(&self.dir, self.files - 1));
        for index in (0..self.files - 1).rev() {
            match fs::rename(log_path(&self.dir, index), log_path(&self.dir, index + 1)) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
                _ => {}
            }
        }
        self.file = Some(OpenOptions::new().create(true).write(true).truncate(true).open(log_path(&self.dir, 0))?);
        self.size = 0;
        Ok(())
    }
}

/// The longest prefix of `text` that is at most `max` bytes, on a char boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Words that start header- or secret-shaped content; the rest of the line goes.
const SENSITIVE: [&str; 8] = ["authorization", "cookie", "bearer", "basic ", "x-api-key", "api_key", "password", "secret"];

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=' | '~')
}

/// A run of token characters that looks like a credential: 32+ long, or 16+
/// long with at least 3 digits and 3 letters, or 16+ long mixing upper case,
/// lower case and digits (keys, JWT parts, hex digests).
fn token_shaped(run: &str) -> bool {
    let digits = run.chars().filter(char::is_ascii_digit).count();
    let letters = run.chars().filter(char::is_ascii_alphabetic).count();
    let mixed = run.chars().any(|c| c.is_ascii_uppercase()) && run.chars().any(|c| c.is_ascii_lowercase()) && digits > 0;
    run.len() >= 32 || (run.len() >= 16 && ((digits >= 3 && letters >= 3) || mixed))
}

/// One stderr line made safe to keep: control characters become spaces; from
/// the first `{` on is a request body; a `?query` is dropped; from a header or
/// secret word on is dropped; token-shaped runs are replaced; the result is
/// capped at MAX_LINE characters.
pub fn redact(raw: &str) -> String {
    let mut line: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if let Some(start) = line.find('{') {
        line.truncate(start);
        line.push_str("<body redacted>");
    }
    let mut from = 0;
    while let Some(found) = line[from..].find('?') {
        let start = from + found;
        let end = line[start..].find(char::is_whitespace).map_or(line.len(), |i| start + i);
        line.replace_range(start..end, "?<redacted>");
        from = start + "?<redacted>".len();
    }
    let lower = line.to_ascii_lowercase();
    if let Some(start) = SENSITIVE.iter().filter_map(|word| lower.find(word)).min() {
        line.truncate(start);
        line.push_str("<redacted>");
    }
    let mut out = String::with_capacity(line.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if token_shaped(run) {
            out.push_str("<redacted>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in line.chars() {
        if token_char(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    let kept: String = out.chars().take(MAX_LINE).collect();
    kept.trim_end().to_string()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_millis()).unwrap_or(0)
}

/// Reads `reader` to its end, writing each line, redacted and prefixed with a
/// unix-ms stamp, to `log`. Always drains, so a sidecar never blocks on a full
/// pipe even when the log could not be opened.
pub fn pump(mut reader: impl BufRead, mut log: Option<RotatingLog>) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.by_ref().take(MAX_READ).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Some(log) = log.as_mut() else { continue };
        let line = redact(String::from_utf8_lossy(&buf).trim_end());
        if !line.is_empty() {
            let _ = log.write_line(&format!("{} {line}", now_ms()));
        }
    }
}

/// Starts the thread that drains the sidecar's stderr into
/// `<dir>/gen-audio-mcp.log` (MAX_BYTES x FILES). With no dir, or a dir that
/// cannot be opened, it still drains.
pub fn spawn_pump(stderr: impl Read + Send + 'static, dir: Option<PathBuf>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("gen-audio-mcp-stderr".into())
        .spawn(move || {
            let log = dir.and_then(|dir| RotatingLog::open(&dir, MAX_BYTES, FILES).ok());
            pump(BufReader::new(stderr), log);
        })
        .expect("spawn sidecar stderr thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gen-audio-sidecar-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn log_text(dir: &Path) -> String {
        let mut text = String::new();
        for i in (0..FILES).rev() {
            text.push_str(&fs::read_to_string(log_path(dir, i)).unwrap_or_default());
        }
        text
    }

    #[test]
    fn rotation_caps_the_size_at_files_times_max_bytes() {
        let dir = scratch("rotate");
        let mut log = RotatingLog::open(&dir, 1024, FILES).unwrap();
        for i in 0..2000 {
            log.write_line(&format!("line {i:05} {}", "x".repeat(40))).unwrap();
        }
        drop(log);
        let mut names: Vec<String> = fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["gen-audio-mcp.log", "gen-audio-mcp.log.1", "gen-audio-mcp.log.2"]);
        for name in &names {
            let size = fs::metadata(dir.join(name)).unwrap().len();
            assert!(size > 0 && size <= 1024, "{name} is {size} bytes");
        }
        let current = fs::read_to_string(log_path(&dir, 0)).unwrap();
        assert!(current.ends_with(&format!("line 01999 {}\n", "x".repeat(40))), "newest line is in the current file");
        assert!(!log_text(&dir).contains("line 00000 "), "the oldest lines rotated out");
        // Reopening appends to the current file and still honours the cap.
        let mut log = RotatingLog::open(&dir, 1024, FILES).unwrap();
        log.write_line("after reopen").unwrap();
        assert!(fs::metadata(log_path(&dir, 0)).unwrap().len() <= 1024);
        assert_eq!(MAX_BYTES, 1024 * 1024);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn redact_strips_bodies_headers_queries_and_token_shaped_strings() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let cases = [
            ("gen-audio-mcp http: rejected POST /mcp?access_token=abc 403 origin peer=127.0.0.1:5000", "abc"),
            ("gen-audio-mcp http: rejected {\"jsonrpc\":\"2.0\",\"params\":{\"secret\":\"body-marker\"}} - 400 bad request", "body-marker"),
            ("Authorization: Bearer sk-live-0123456789abcdefABCDEF", "sk-live"),
            ("error: upstream said cookie=session-value-here", "session-value"),
            (&*format!("error: token {jwt} expired"), "eyJhbGci"),
            ("error: key ghp_16C7e42F292c6912E7710c838347Ae178B4a was refused", "ghp_16C7"),
        ];
        for (raw, secret) in cases {
            let line = redact(raw);
            assert!(!line.contains(secret), "{raw:?} -> {line:?}");
            assert!(line.contains("<redacted>") || line.contains("<body redacted>"), "{line:?} says it redacted");
        }
        let plain = "gen-audio-mcp http: rejected POST /mcp 415 content-type peer=127.0.0.1:51234";
        assert_eq!(redact(plain), plain, "an ordinary rejection line is kept whole");
        assert_eq!(redact("gen-audio-mcp: listening on library_kokoro_cube3d"), "gen-audio-mcp: listening on library_kokoro_cube3d");
        assert!(redact(&"a".repeat(5000)).len() <= MAX_LINE + "<redacted>".len());
        assert!(!redact("a\u{1b}[31mred\rb").contains(['\u{1b}', '\r']));
    }

    /// The child half of the pipe tests: a real loopback server in its own
    /// process whose stderr is the sidecar's stderr. Runs only when spawned.
    #[test]
    #[ignore = "spawned by the pipe tests as a stand-in sidecar"]
    fn stand_in_sidecar() {
        if std::env::var("GEN_AUDIO_STAND_IN_SIDECAR").is_err() {
            return;
        }
        let bound = gen_audio_mcp::http::spawn_loopback("127.0.0.1:0").unwrap();
        println!("SIDECAR_ADDR={bound}");
        std::io::stdout().flush().unwrap();
        let _ = std::io::stdin().read(&mut [0u8; 1]);
    }

    /// Spawns the stand-in sidecar with stderr piped into `spawn_pump`, sends
    /// `request`, and returns the log text once a rejection line lands.
    fn through_the_pipe(name: &str, request: &str) -> (String, PathBuf) {
        let dir = scratch(name);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["sidecar_log::tests::stand_in_sidecar", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env("GEN_AUDIO_STAND_IN_SIDECAR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pump = spawn_pump(child.stderr.take().unwrap(), Some(dir.clone()));
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let addr = loop {
            let mut line = String::new();
            assert!(stdout.read_line(&mut line).unwrap() > 0, "stand-in sidecar exited before it listened");
            // libtest prints "test ... " without a newline before the line.
            if let Some(at) = line.find("SIDECAR_ADDR=") {
                break line[at + "SIDECAR_ADDR=".len()..].trim().to_string();
            }
        };
        let mut stream = TcpStream::connect(&addr).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        assert!(response.starts_with("HTTP/1.1 4"), "{response}");
        drop(child.stdin.take());
        let _ = child.wait();
        pump.join().unwrap();
        (log_text(&dir), dir)
    }

    #[test]
    fn a_rejected_request_produces_a_line_in_the_log_file() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (text, dir) = through_the_pipe("rejected", &request);
        let line = text.lines().find(|line| line.contains("rejected POST /mcp 403")).unwrap_or_else(|| panic!("no rejection line in {text:?}"));
        let stamp = line.split(' ').next().unwrap();
        assert!(stamp.parse::<u64>().is_ok(), "{line}: starts with a unix-ms stamp");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_body_an_authorization_header_and_a_token_never_reach_the_log() {
        let token = "tok_9f8e7d6c5b4a39281706f5e4d3c2b1a0ZYXWVUTS";
        let bearer = "Bearer sk-ant-b0dy-0123456789abcdefghijABCDEFGHIJ";
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","arguments":{"text":"body-marker-7"}}}"#;
        let request = format!(
            "POST /mcp?access_token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nAuthorization: {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (text, dir) = through_the_pipe("redact", &request);
        assert!(text.contains("rejected POST /mcp?<redacted> 403"), "the rejection is still logged: {text:?}");
        for secret in [token, "tok_9f8e", bearer, "sk-ant", "Authorization", "body-marker-7", "tools/call"] {
            assert!(!text.contains(secret), "{secret:?} reached the log: {text:?}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sidecar_spawn_pipes_stderr_into_the_log() {
        let source = include_str!("lib.rs");
        let spawn = &source[source.find("fn spawn_sidecar").expect("spawn_sidecar")..];
        let spawn = &spawn[..spawn.find("\n}\n").unwrap()];
        assert!(spawn.contains(".stderr(Stdio::piped())") && spawn.contains("sidecar_log::spawn_pump"), "{spawn}");
        assert!(!spawn.contains("Stdio::null()) // stderr") && !spawn.contains(".stderr(Stdio::null())"), "{spawn}");
        assert!(source.contains("app_log_dir()"), "the log lives in the Tauri app_log_dir");
    }
}
