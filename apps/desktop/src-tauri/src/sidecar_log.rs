//! The MCP sidecar's stderr, kept: a size-capped rotating file in the Tauri
//! app_log_dir (gen-audio-mcp.log, .log.1, .log.2; 1 MiB each).
//!
//! The sidecar writes one stderr line per rejected request (method, path,
//! status, reason, peer) and its startup errors. Before this, the desktop
//! spawned it with stderr set to null, so on a user's machine those lines went
//! nowhere: a diagnostic written to a stream nobody can read is the same as no
//! diagnostic.
//!
//! Every line is redacted whole, before it is capped: no request bodies, no
//! header values, no query strings or `token=`/`key=`/`password=` values, no
//! emails, no home-directory user names, nothing token-shaped. In-crate
//! rotation, no new dependency.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// File name of the current log in the app log dir.
pub const LOG_NAME: &str = "gen-audio-mcp.log";
/// Bytes per file before it rotates.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Files kept: the current one plus `.1` and `.2`.
pub const FILES: usize = 3;
/// Bytes (UTF-8, cut on a char boundary) of one redacted line that are kept.
pub const MAX_LINE: usize = 512;
/// Bytes of one stderr line read before redaction. The rest of a longer line
/// is drained and dropped, never logged as a line of its own: a split before
/// redaction let a secret's tail through as an innocent-looking line.
pub const MAX_PIPE_LINE: usize = 64 * 1024;

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
///
/// A line that cannot be written (a rotation that fails, a closed file) is
/// never lost silently: the first failure says so once on the desktop's own
/// stderr, every lost line is counted, and the first write that succeeds
/// again is preceded by one marker line with the count and the reason.
pub struct RotatingLog {
    dir: PathBuf,
    max_bytes: u64,
    files: usize,
    file: Option<File>,
    size: u64,
    dropped: u64,
    drop_reason: String,
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
            dropped: 0,
            drop_reason: String::new(),
        })
    }

    /// Lines lost since the last successful write (0 once a marker reported them).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let result = self.report_drops().and_then(|()| self.append(line));
        if let Err(err) = &result {
            if self.dropped == 0 {
                eprintln!("gen-audio-desktop: sidecar log write failed ({err}); counting dropped lines until it recovers");
            }
            self.dropped += 1;
            self.drop_reason = redact(&err.to_string());
        }
        result
    }

    /// Writes the one marker line for the lines lost so far, if any.
    fn report_drops(&mut self) -> io::Result<()> {
        if self.dropped == 0 {
            return Ok(());
        }
        let marker = format!("{} gen-audio-desktop: sidecar log dropped {} line(s): {}", now_ms(), self.dropped, self.drop_reason);
        self.append(&marker)?;
        self.dropped = 0;
        Ok(())
    }

    fn append(&mut self, line: &str) -> io::Result<()> {
        let max = usize::try_from(self.max_bytes.saturating_sub(1)).unwrap_or(usize::MAX);
        let line = cut(line, max);
        let bytes = line.len() as u64 + 1;
        if self.file.is_none() || (self.size > 0 && self.size + bytes > self.max_bytes) {
            self.rotate().map_err(|err| io::Error::new(err.kind(), format!("rotate failed: {err}")))?;
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

/// `name=value` pairs whose value goes, whatever its length: a name ending in
/// one of these (token, access_token, key, api_key, apikey, password, ...).
const SECRET_NAMES: [&str; 5] = ["token", "key", "password", "passwd", "secret"];

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

/// `token=abc` -> `token=<redacted>` for every SECRET_NAMES name, any length.
fn redact_secret_values(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(eq) = rest.find('=') {
        let (head, tail) = rest.split_at(eq);
        let name_start = head.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).map_or(0, |i| i + 1);
        let name = head[name_start..].to_ascii_lowercase();
        out.push_str(head);
        out.push('=');
        let value = &tail[1..];
        if SECRET_NAMES.iter().any(|secret| name.ends_with(secret)) {
            let end = value.find(|c: char| c.is_whitespace() || c == '&' || c == ';' || c == ',').unwrap_or(value.len());
            if end > 0 {
                out.push_str("<redacted>");
            }
            rest = &value[end..];
        } else {
            rest = value;
        }
    }
    out.push_str(rest);
    out
}

fn email_local(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

fn email_domain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

/// `name@host.tld` -> `<email>`.
fn redact_emails(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('@') {
        let local_start = rest[..at].rfind(|c: char| !email_local(c)).map_or(0, |i| i + 1);
        let domain_len = rest[at + 1..].find(|c: char| !email_domain(c)).unwrap_or(rest.len() - at - 1);
        let domain = rest[at + 1..at + 1 + domain_len].trim_end_matches('.');
        if local_start < at && domain.contains('.') && !domain.starts_with('.') {
            out.push_str(&rest[..local_start]);
            out.push_str("<email>");
            rest = &rest[at + 1 + domain.len()..];
        } else {
            out.push_str(&rest[..=at]);
            rest = &rest[at + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// `C:\\Users\\<name>`, `C:/Users/<name>`, `/Users/<name>`, `/home/<name>` ->
/// the same prefix with `<user>`.
fn redact_home_paths(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    for prefix in ["\\users\\", "/users/", "/home/"] {
        let mut from = 0;
        while let Some(found) = lower[from..].find(prefix) {
            let start = from + found + prefix.len();
            let len = line[start..].find(|c: char| matches!(c, '\\' | '/' | '"' | '\'' | ':' | ';' | ',') || c.is_whitespace()).unwrap_or(line.len() - start);
            if len > 0 {
                cuts.push((start, start + len));
            }
            from = start;
        }
    }
    cuts.sort_unstable();
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    for (start, end) in cuts {
        if start < at {
            continue;
        }
        out.push_str(&line[at..start]);
        out.push_str("<user>");
        at = end;
    }
    out.push_str(&line[at..]);
    out
}

/// One stderr line made safe to keep. The whole line is redacted first and
/// only then capped, so no cut can separate a secret from what marks it:
/// control characters become spaces; from the first `{` on is a request body;
/// a `?query` is dropped; `token=`/`key=`/`password=`-style values go whatever
/// their length; emails and home-directory user names go; from a header or
/// secret word on is dropped; token-shaped runs are replaced; the result is
/// capped at MAX_LINE bytes on a char boundary.
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
    let line = redact_home_paths(&redact_emails(&redact_secret_values(&line)));
    let mut line = line;
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
    cut(&out, MAX_LINE).trim_end().to_string()
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
        match reader.by_ref().take(MAX_PIPE_LINE as u64).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.last() != Some(&b'\n') && buf.len() == MAX_PIPE_LINE {
            // Over the ceiling: drop the rest of this line, and the partial
            // word at the cut (it may be a secret's head), so nothing of it
            // is logged, and never as a line of its own.
            if !drain_line(&mut reader) {
                break;
            }
            let keep = buf.iter().rposition(u8::is_ascii_whitespace).unwrap_or(0);
            buf.truncate(keep);
        }
        let Some(log) = log.as_mut() else { continue };
        let line = redact(String::from_utf8_lossy(&buf).trim_end());
        if !line.is_empty() {
            let _ = log.write_line(&format!("{} {line}", now_ms()));
        }
    }
}

/// Reads and discards up to and including the next newline; false at the end
/// of the stream or on a read error.
fn drain_line(reader: &mut impl BufRead) -> bool {
    loop {
        let (consumed, done) = match reader.fill_buf() {
            Ok([]) | Err(_) => return false,
            Ok(bytes) => match bytes.iter().position(|b| *b == b'\n') {
                Some(at) => (at + 1, true),
                None => (bytes.len(), false),
            },
        };
        reader.consume(consumed);
        if done {
            return true;
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

    /// A rotation that fails (here: `.log.2` is a non-empty directory, so the
    /// `.log.1` -> `.log.2` rename fails) must not lose lines silently: they
    /// are counted, and the first write that succeeds again is preceded by one
    /// marker line saying how many were dropped and why.
    #[test]
    fn a_failed_rotation_is_counted_and_marked_not_silent() {
        let dir = scratch("rotate-fail");
        let mut log = RotatingLog::open(&dir, 256, FILES).unwrap();
        log.write_line(&format!("first line {}", "x".repeat(200))).unwrap();
        fs::write(log_path(&dir, 1), "older\n").unwrap();
        fs::create_dir_all(log_path(&dir, 2).join("blocker")).unwrap();
        for i in 0..3 {
            assert!(log.write_line(&format!("lost {i} {}", "y".repeat(100))).is_err(), "rotation should fail");
        }
        assert_eq!(log.dropped(), 3);
        fs::remove_dir_all(log_path(&dir, 2)).unwrap();
        log.write_line("after recovery").unwrap();
        assert_eq!(log.dropped(), 0, "the marker reported them");
        let current = fs::read_to_string(log_path(&dir, 0)).unwrap();
        let marker = current.lines().find(|line| line.contains("sidecar log dropped 3 line(s)")).unwrap_or_else(|| panic!("no drop marker in {current:?}"));
        assert!(marker.contains("rotate failed"), "{marker}");
        assert!(current.ends_with("after recovery\n"), "{current:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Feeds `input` through `pump` into a fresh log and returns its lines,
    /// without the unix-ms stamp.
    fn pumped(name: &str, input: &str) -> (Vec<String>, PathBuf) {
        let dir = scratch(name);
        let log = RotatingLog::open(&dir, MAX_BYTES, FILES).unwrap();
        pump(std::io::Cursor::new(input.as_bytes().to_vec()), Some(log));
        let lines = log_text(&dir).lines().map(|line| line.split_once(' ').map_or(String::new(), |(_, rest)| rest.to_string())).collect();
        (lines, dir)
    }

    /// Optimus's probe: a stderr line over 4 KiB was split before redaction,
    /// so a secret straddling the split lost its head to `<redacted>` and its
    /// tail ('ECRETxyz') reached the log as a new, innocent-looking line.
    #[test]
    fn a_secret_straddling_the_read_split_leaks_no_fragment() {
        let filler = "ok ".repeat(1362); // 4086 bytes: "password=S" ends at byte 4096
        let token = "Zk3Qx9Lm2Rt7Vb4NwY8pH6cJ"; // token-shaped whole, not in halves
        let token_filler = "ok ".repeat(1362 - 4);
        for (secret_line, secret) in [
            (format!("{filler}password=SECRETxyz tail-marker"), "SECRETxyz"),
            (format!("{token_filler}error: key {token} refused"), token),
        ] {
            let (lines, dir) = pumped("straddle", &format!("{secret_line}\nsecond line\n"));
            assert_eq!(lines.len(), 2, "one stderr line is one log line: {lines:?}");
            assert_eq!(lines[1], "second line");
            let text = lines.join("\n");
            for start in 0..secret.len() - 2 {
                let fragment = &secret[start..];
                assert!(!text.contains(fragment), "{fragment:?} of {secret:?} reached the log");
            }
            let _ = fs::remove_dir_all(&dir);
        }
        // Past the 64 KiB read ceiling the rest of the line is drained, never logged.
        let long = format!("{}password=SECRETxyz {}", "ok ".repeat(64 * 1024 / 3), "tail ".repeat(10));
        let (lines, dir) = pumped("ceiling", &format!("{long}\nafter\n"));
        assert_eq!(lines.len(), 2, "{:?}", lines.iter().map(String::len).collect::<Vec<_>>());
        assert!(!lines.join("\n").contains("ECRET") && lines[1] == "after");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn redact_covers_emails_home_paths_query_values_and_auth_of_any_length() {
        let cases = [
            ("error: notify dayour@example.com failed", "dayour@example.com", "<email>"),
            (r"open C:\Users\dayour\AppData\Local\x.wav failed", "dayour", r"C:\Users\<user>\AppData"),
            ("open c:/users/dayour/x.wav failed", "dayour", "c:/users/<user>/x.wav"),
            ("open /home/dayour/gen-audio/x.wav failed", "dayour", "/home/<user>/gen-audio/x.wav"),
            ("open /Users/dayour/x.wav failed", "dayour", "/Users/<user>/x.wav"),
            ("fetch failed token=abc 403", "abc", "token=<redacted> 403"),
            ("fetch failed access_token=t0 403", "t0", "access_token=<redacted> 403"),
            ("fetch failed key=k1 status 403", "k1", "key=<redacted> status 403"),
            ("fetch failed api_key=k2&x=1 403", "k2", "<redacted>"),
            ("fetch failed password=pw 403", "pw 403", "<redacted>"),
            ("Authorization: zz9", "zz9", "<redacted>"),
            ("upstream said bearer q7", "q7", "<redacted>"),
        ];
        for (raw, secret, kept) in cases {
            let line = redact(raw);
            assert!(!line.contains(secret), "{raw:?} -> {line:?} still has {secret:?}");
            assert!(line.contains(kept), "{raw:?} -> {line:?} should keep {kept:?}");
        }
        let plain = "gen-audio-mcp http: rejected POST /mcp 403 origin peer=127.0.0.1:51234";
        assert_eq!(redact(plain), plain, "a peer address is not an email or a secret");
    }

    /// Optimus's L1 probe: a secret and an email that straddle the 512-byte
    /// cap. Capped first, the cut leaves `Zk3Qx9Lm2R` (too short to look like
    /// a token) and `dayour@micro` (no dot, so not an email), and both leak.
    /// Redacted whole and then capped, neither does.
    #[test]
    fn a_secret_and_an_email_straddling_the_line_cap_are_redacted_whole() {
        let token = "Zk3Qx9Lm2Rt7Vb4NwY8pH6cJ";
        let email = "dayour@microsoft.com";
        let mut leaks = Vec::new();
        for (lead, secret, head, marker) in [("error: key ", token, "Zk3Qx9Lm2R", "<redacted>"), ("error: notify ", email, "dayour@micro", "<email>")] {
            let pad = MAX_LINE - head.len() - lead.len();
            let raw = format!("{}{}{lead}{secret} refused", "ok ".repeat(pad / 3), " ".repeat(pad % 3));
            assert!(raw[..MAX_LINE].ends_with(head), "{secret:?} straddles byte {MAX_LINE}");
            let line = redact(&raw);
            assert!(line.len() <= MAX_LINE, "{} bytes", line.len());
            // The longest piece (4+ bytes) of the secret that reached the line.
            let leaked = (0..secret.len()).flat_map(|start| (start + 4..=secret.len()).map(move |end| &secret[start..end])).filter(|piece| line.contains(piece)).max_by_key(|piece| piece.len());
            if let Some(piece) = leaked {
                leaks.push(format!("{piece:?} of {secret:?}"));
            } else if !line.contains(marker) {
                leaks.push(format!("no {marker} for {secret:?}"));
            }
        }
        assert!(leaks.is_empty(), "leaked across the {MAX_LINE}-byte cap: {leaks:?}");
    }

    #[test]
    fn the_line_cap_counts_bytes_and_cuts_on_a_char_boundary() {
        // 511 bytes of short words, then a 2-byte char straddling byte 512.
        let straddle = format!("{}xé tail", "x ".repeat((MAX_LINE - 1) / 2));
        for raw in ["é".repeat(300), straddle.clone(), "日本語 ".repeat(200)] {
            let line = redact(&raw);
            assert!(line.len() <= MAX_LINE, "{} bytes > {MAX_LINE}", line.len());
            assert!(line.len() >= MAX_LINE - 4, "cut at the byte cap, not far before it: {}", line.len());
        }
        assert_eq!(redact(&straddle), format!("{}x", "x ".repeat((MAX_LINE - 1) / 2)));
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
