//! The MCP sidecar's stderr, kept: a size-capped rotating file in the Tauri
//! app_log_dir (gen-audio-mcp.log, .log.1, .log.2; 1 MiB each).
//!
//! The sidecar writes one stderr line per rejected request (method, target,
//! status, reason, peer and seq) and its startup errors. Before this, the desktop
//! spawned it with stderr set to null, so on a user's machine those lines went
//! nowhere: a diagnostic written to a stream nobody can read is the same as no
//! diagnostic.
//!
//! Every line is redacted whole, before it is capped: no request bodies, no
//! header values, no query strings or `token=`/`key=`/`password=`-style values
//! (also `:`, spaced, quoted and URL-encoded), no URL userinfo, no emails, no
//! home-directory user names, nothing token-shaped. The classes are one table,
//! CLASSES; it is defense in depth, not the boundary. In-crate rotation, no new
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
/// never lost silently: the first failure of an outage says so once on the
/// desktop's own stderr, every lost line is counted, and the first write that
/// succeeds again is preceded by one marker line with the count and the
/// reason. A failed rename names its paths; the reason is redacted like a log
/// line, in the notice and in the marker.
pub struct RotatingLog {
    dir: PathBuf,
    max_bytes: u64,
    files: usize,
    file: Option<File>,
    size: u64,
    dropped: u64,
    drop_reason: String,
    /// Where the one notice per outage goes: the desktop's own stderr.
    notice: Box<dyn Write + Send>,
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
            notice: Box::new(io::stderr()),
        })
    }

    /// Sends the one-time failure notice to `sink` instead of stderr.
    pub fn notice_to(mut self, sink: impl Write + Send + 'static) -> Self {
        self.notice = Box::new(sink);
        self
    }

    /// Lines lost since the last successful write (0 once a marker reported them).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let result = self.report_drops().and_then(|()| self.append(line));
        if let Err(err) = &result {
            // The reason can name the log dir (a home path) and so goes
            // through the same redactor as every log line, in the marker
            // and in the notice alike.
            self.drop_reason = redact(&err.to_string());
            if self.dropped == 0 {
                let _ = writeln!(self.notice, "gen-audio-desktop: sidecar log write failed ({}); counting dropped lines until it recovers", self.drop_reason);
            }
            self.dropped += 1;
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
            let (from, to) = (log_path(&self.dir, index), log_path(&self.dir, index + 1));
            match fs::rename(&from, &to) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => {
                    return Err(io::Error::new(err.kind(), format!("rename {} -> {}: {err}", from.display(), to.display())));
                }
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

/// How one redaction class finds what it removes.
#[derive(Clone, Copy, Debug)]
pub enum Rule {
    /// Control characters become spaces (no terminal escapes in the log).
    Control,
    /// From the first `{` on is a request body.
    Body,
    /// A `?query`, up to the next whitespace.
    Query,
    /// `scheme://user:pass@host`: the userinfo, whatever the host (dotless
    /// `localhost` too).
    Userinfo,
    /// The value after a name ending in this word, whatever its length. The
    /// separator is `=`, `:`, `%3D` or `%3A`, with optional spaces around it
    /// (`token = x`, `pwd: x`, `x%26sig%3Dv`, `#access_token=x`); a quoted
    /// value goes to its closing quote (a second word goes too), else it ends
    /// at whitespace, `&`, `;`, `,` or `%26`.
    Value(&'static str),
    /// `name@host.tld`.
    Email,
    /// The user name after `\Users\`, `/Users/` or `/home/`, past any number
    /// of separators, so a Debug-escaped `C:\\Users\\name` goes too.
    HomePath,
    /// From this word on, the rest of the line.
    Rest(&'static str),
    /// Credential-shaped runs, including base64 ones that contain `/`.
    TokenShaped,
}

/// Every redaction class, in the order `redact` applies them. ONE table:
/// adding a class is one line here (and its row in the tests, which fail
/// until every class has one; a `Value` class is also tried in every
/// separator form).
///
/// Denylist redaction is defense in depth, not the boundary. The boundary is
/// what is written at all: the sidecar's rejection lines carry method, target,
/// status, reason, peer and seq, never a body or a header value. This table only
/// catches what slips through anyway and can never be complete, so a miss is
/// first a reason to stop writing that thing, then a new row here.
pub const CLASSES: &[(&str, Rule)] = &[
    ("control characters", Rule::Control),
    ("request body", Rule::Body),
    ("query string", Rule::Query),
    ("url userinfo", Rule::Userinfo),
    ("token", Rule::Value("token")),
    ("key", Rule::Value("key")),
    ("passwd", Rule::Value("passwd")),
    ("pwd", Rule::Value("pwd")),
    ("pass", Rule::Value("pass")),
    ("auth", Rule::Value("auth")),
    ("credential", Rule::Value("credential")),
    ("session", Rule::Value("session")),
    ("sig", Rule::Value("sig")),
    ("email", Rule::Email),
    ("home directory", Rule::HomePath),
    ("authorization", Rule::Rest("authorization")),
    ("cookie", Rule::Rest("cookie")),
    ("bearer", Rule::Rest("bearer")),
    ("basic", Rule::Rest("basic ")),
    ("x-api-key", Rule::Rest("x-api-key")),
    ("api_key", Rule::Rest("api_key")),
    ("password", Rule::Rest("password")),
    ("secret", Rule::Rest("secret")),
    ("token-shaped", Rule::TokenShaped),
];

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

/// A `/`-joined run that is a credential as a whole: 16+ long with a
/// segment of 6+ that mixes upper case, lower case and digits (base64), so no
/// short piece of it survives. Paths (`/tmp/gen-audio/library`) don't mix.
fn slash_secret(run: &str) -> bool {
    let mixed = |seg: &str| seg.len() >= 6 && seg.chars().any(|c| c.is_ascii_uppercase()) && seg.chars().any(|c| c.is_ascii_lowercase()) && seg.chars().any(|c| c.is_ascii_digit());
    run.contains('/') && run.len() >= 16 && run.split('/').any(mixed)
}

fn redact_token_runs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        // A `/` run is judged whole only by slash_secret: a long path is
        // not a credential, its token-shaped segments are.
        if slash_secret(run) {
            out.push_str("<redacted>");
        } else {
            for (i, seg) in run.split('/').enumerate() {
                if i > 0 {
                    out.push('/');
                }
                out.push_str(if token_shaped(seg) { "<redacted>" } else { seg });
            }
        }
        run.clear();
    };
    for c in line.chars() {
        if token_char(c) || c == '/' {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Bytes in the separator at `at`: `=` or `:` (1), `%3D` or `%3A` (3), else 0.
fn separator_len(bytes: &[u8], at: usize) -> usize {
    match bytes[at] {
        b'=' | b':' => 1,
        b'%' if bytes.get(at + 1) == Some(&b'3') && matches!(bytes.get(at + 2).map(u8::to_ascii_uppercase), Some(b'D' | b'A')) => 3,
        _ => 0,
    }
}

/// Where an unquoted value starting at `start` ends.
fn plain_value_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b if b.is_ascii_whitespace() => break,
            b'&' | b';' | b',' => break,
            b'%' if bytes.get(i + 1) == Some(&b'2') && bytes.get(i + 2) == Some(&b'6') => break,
            _ => i += 1,
        }
    }
    i
}

/// Rule::Value: every value whose name ends in `word`.
fn redact_values(line: &str, word: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        let sep = separator_len(bytes, at);
        if sep == 0 {
            at += 1;
            continue;
        }
        let mut name_end = at;
        while name_end > 0 && bytes[name_end - 1] == b' ' {
            name_end -= 1;
        }
        let mut name_start = name_end;
        while name_start > 0 && name_byte(bytes[name_start - 1]) {
            name_start -= 1;
        }
        if name_start == name_end || !line[name_start..name_end].to_ascii_lowercase().ends_with(word) {
            at += sep;
            continue;
        }
        let mut start = at + sep;
        while start < bytes.len() && bytes[start] == b' ' {
            start += 1;
        }
        let end = match bytes.get(start) {
            Some(&quote @ (b'"' | b'\'')) => line[start + 1..].find(quote as char).map_or(bytes.len(), |i| start + 1 + i + 1),
            Some(_) => plain_value_end(bytes, start),
            None => start,
        };
        if end > start && &line[start..end] != "<redacted>" {
            out.push_str(&line[copied..start]);
            out.push_str("<redacted>");
            copied = end;
        }
        at = end.max(at + sep);
    }
    out.push_str(&line[copied..]);
    out
}

/// Rule::Userinfo: `scheme://user:pass@host` -> `scheme://<redacted>@host`.
fn redact_userinfo(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("://") {
        let tail = &rest[at + 3..];
        out.push_str(&rest[..at + 3]);
        let authority = tail.find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace()).unwrap_or(tail.len());
        match tail[..authority].rfind('@') {
            Some(user_end) if user_end > 0 => {
                out.push_str("<redacted>");
                rest = &tail[user_end..];
            }
            _ => rest = tail,
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

/// Rule::HomePath: `C:\Users\<name>`, `C:\\Users\\<name>`, `C:/Users/<name>`,
/// `/Users/<name>`, `/home/<name>` -> the same prefix with `<user>`.
fn redact_home_paths(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    for prefix in ["\\users\\", "/users/", "/home/"] {
        let mut from = 0;
        while let Some(found) = lower[from..].find(prefix) {
            let mut start = from + found + prefix.len();
            while matches!(line.as_bytes().get(start), Some(b'\\' | b'/')) {
                start += 1;
            }
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

fn apply(rule: Rule, mut line: String) -> String {
    match rule {
        Rule::Control => line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect(),
        Rule::Body => {
            if let Some(start) = line.find('{') {
                line.truncate(start);
                line.push_str("<body redacted>");
            }
            line
        }
        Rule::Query => {
            let mut from = 0;
            while let Some(found) = line[from..].find('?') {
                let start = from + found;
                let end = line[start..].find(char::is_whitespace).map_or(line.len(), |i| start + i);
                line.replace_range(start..end, "?<redacted>");
                from = start + "?<redacted>".len();
            }
            line
        }
        Rule::Userinfo => redact_userinfo(&line),
        Rule::Value(word) => redact_values(&line, word),
        Rule::Email => redact_emails(&line),
        Rule::HomePath => redact_home_paths(&line),
        Rule::Rest(word) => {
            if let Some(start) = line.to_ascii_lowercase().find(word) {
                line.truncate(start);
                line.push_str("<redacted>");
            }
            line
        }
        Rule::TokenShaped => redact_token_runs(&line),
    }
}

/// One stderr line made safe to keep: every class in CLASSES, in order, over
/// the whole line; only then is it capped at MAX_LINE bytes on a char
/// boundary, so no cut can separate a secret from what marks it.
pub fn redact(raw: &str) -> String {
    let line = CLASSES.iter().fold(raw.to_string(), |line, (_, rule)| apply(*rule, line));
    cut(&line, MAX_LINE).trim_end().to_string()
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

    /// A Write that tests can read back: the notice sink.
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    /// B2, pinned: an outage says so on stderr exactly once, however many
    /// lines it drops, and both that notice and the marker's reason go
    /// through `redact` like any log line. The log dir here holds a home
    /// path and a `token=` value, and the failed rotation names its paths.
    #[test]
    fn an_outage_is_noticed_once_and_its_reason_is_redacted() {
        let secret = "Zk3Qx9Lm2R";
        let dir = scratch("notice").join("Users").join("dayour").join(format!("token={secret}"));
        let notices = Captured::default();
        let mut log = RotatingLog::open(&dir, 256, FILES).unwrap().notice_to(notices.clone());
        log.write_line(&format!("first line {}", "x".repeat(200))).unwrap();
        fs::write(log_path(&dir, 1), "older\n").unwrap();
        fs::create_dir_all(log_path(&dir, 2).join("blocker")).unwrap();
        for i in 0..5 {
            assert!(log.write_line(&format!("lost {i} {}", "y".repeat(100))).is_err(), "rotation should fail");
        }
        let said = notices.text();
        assert_eq!(said.matches("sidecar log write failed").count(), 1, "one notice for 5 lost lines: {said:?}");
        fs::remove_dir_all(log_path(&dir, 2)).unwrap();
        log.write_line("after recovery").unwrap();
        // Force a rotation (200 more bytes on a 256-byte cap), so the marker is
        // read from a rotated file: reading only the current file misses it.
        log.write_line(&format!("after rotation {}", "w".repeat(185))).unwrap();
        let newest = fs::read_to_string(log_path(&dir, 0)).unwrap();
        assert!(!newest.contains("sidecar log dropped"), "the marker rotated out of the current file: {newest:?}");
        let current = log_text(&dir);
        let marker = current.lines().find(|line| line.contains("sidecar log dropped 5 line(s)")).unwrap_or_else(|| panic!("no drop marker in {current:?}"));
        for (what, text) in [("marker", marker), ("notice", said.as_str())] {
            assert!(text.contains("rotate failed") && (text.contains("Users/<user>/token=<redacted>") || text.contains("Users\\<user>\\token=<redacted>")), "{what} names the redacted path: {text:?}");
            assert!(!text.contains("dayour") && !text.contains(secret), "{what} leaks: {text:?}");
        }
        // A second outage is a new one: one more notice, not one per line.
        let _ = fs::remove_file(log_path(&dir, 2));
        fs::create_dir_all(log_path(&dir, 2).join("blocker")).unwrap();
        for _ in 0..3 {
            let _ = log.write_line(&"z".repeat(200));
        }
        assert_eq!(notices.text().matches("sidecar log write failed").count(), 2, "{:?}", notices.text());
        let _ = fs::remove_dir_all(scratch("notice"));
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

    /// One row per redaction class: (class, raw line, what must go, what
    /// must stay). Adding a class to CLASSES without a row here fails
    /// `every_redaction_class_has_a_row`.
    const ROWS: &[(&str, &str, &[&str], &str)] = &[
        ("control characters", "a\u{1b}[31mred\rb", &["\u{1b}", "\r"], "red"),
        ("request body", "rejected {\"params\":{\"x\":\"body-marker\"}} - 400", &["body-marker"], "rejected <body redacted>"),
        ("query string", "rejected POST /mcp?q=find-me 403", &["find-me"], "/mcp?<redacted> 403"),
        ("url userinfo", "fetch http://u:p4ss@localhost:8080/x failed", &["u:p4ss", "p4ss"], "http://<redacted>@localhost:8080/x failed"),
        ("token", "fetch failed token=t0k 403", &["t0k"], "token=<redacted> 403"),
        ("key", "fetch failed key=k1 status 403", &["k1"], "key=<redacted> status 403"),
        ("passwd", "login passwd=pw1 next", &["pw1"], "passwd=<redacted> next"),
        ("pwd", "login pwd=pw2 next", &["pw2"], "pwd=<redacted> next"),
        ("pass", "login pass=pw3 next", &["pw3"], "pass=<redacted> next"),
        ("auth", "proxy auth=au7 next", &["au7"], "auth=<redacted> next"),
        ("credential", "load credential=cr8 next", &["cr8"], "credential=<redacted> next"),
        ("session", "resume session=se9 next", &["se9"], "session=<redacted> next"),
        ("sig", "blob sig=sg0 next", &["sg0"], "sig=<redacted> next"),
        ("email", "error: notify dayour@example.com failed", &["dayour", "example.com"], "notify <email> failed"),
        ("home directory", r"open C:\Users\dayour\AppData\Local\x.wav failed", &["dayour"], r"C:\Users\<user>\AppData"),
        ("home directory", r#"open "C:\\Users\\dayour\\AppData\\x.wav" failed"#, &["dayour"], r"C:\\Users\\<user>\\AppData"),
        ("authorization", "Authorization: zz9", &["zz9"], "<redacted>"),
        ("cookie", "upstream said cookie=c00kie-v", &["c00kie"], "upstream said <redacted>"),
        ("bearer", "upstream said bearer q7", &["q7"], "upstream said <redacted>"),
        ("basic", "upstream said basic dTpw", &["dTpw"], "upstream said <redacted>"),
        ("x-api-key", "header x-api-key: xk1", &["xk1"], "header <redacted>"),
        ("api_key", "fetch failed api_key=k2&x=1 403", &["k2"], "fetch failed <redacted>"),
        ("password", "login password: hunter2 next", &["hunter2"], "login <redacted>"),
        ("secret", "client secret is s3cr", &["s3cr"], "client <redacted>"),
        ("token-shaped", "error: id ghp_16C7e42F292c6912E7710c838347Ae178B4a refused", &["ghp_16C7"], "error: id <redacted> refused"),
        ("token-shaped", "error: id Zk3Q/x9Lm2Rt7Vb4/NwY8pH6c refused", &["Zk3Q", "x9Lm", "NwY8"], "error: id <redacted> refused"),
    ];

    /// The forms every `name=value` class is caught in: colon and spaced
    /// separators, a quoted value with a second word, URL-encoded `%3D`
    /// (with `%26` ending the value), and a `#access_...=` fragment.
    const FORMS: &[&str] = &[
        "{n}=Qv7r next",
        "{n}: Qv7r next",
        "{n} = Qv7r next",
        "{n} : Qv7r next",
        "{n}=\"Qv7r Wy3z\" next",
        "{n}: 'Qv7r Wy3z' next",
        "cb%3Fx%3D1%26{n}%3DQv7r%26y%3D1 next",
        "https://h/cb#access_{n}=Qv7r&state=ok next",
    ];

    #[test]
    fn every_redaction_class_has_a_row() {
        for (class, _) in CLASSES {
            assert!(ROWS.iter().any(|(row, ..)| row == class), "class {class:?} has no row in ROWS");
        }
        for (row, ..) in ROWS {
            assert!(CLASSES.iter().any(|(class, _)| class == row), "row {row:?} names no class");
        }
    }

    #[test]
    fn each_redaction_class_row_goes_and_keeps_what_it_says() {
        let mut misses = Vec::new();
        for (class, raw, secrets, kept) in ROWS {
            let line = redact(raw);
            if secrets.iter().any(|secret| line.contains(secret)) || !line.contains(kept) {
                misses.push(format!("{class}: {raw:?} -> {line:?}"));
            }
        }
        let value_names = CLASSES.iter().filter_map(|(_, rule)| if let Rule::Value(name) = rule { Some(*name) } else { None });
        for name in value_names {
            for form in FORMS {
                let raw = form.replace("{n}", name);
                let line = redact(&raw);
                if line.contains("Qv7r") || line.contains("Wy3z") || !line.ends_with(" next") {
                    misses.push(format!("{name} form: {raw:?} -> {line:?}"));
                }
            }
        }
        assert!(misses.is_empty(), "{} miss(es):\n{}", misses.len(), misses.join("\n"));
        for plain in ["gen-audio-mcp http: rejected POST /mcp 403 origin peer=127.0.0.1:51234", "gen-audio-mcp: listening on 127.0.0.1:8765", "open /tmp/gen-audio/library/x.wav failed: not found"] {
            assert_eq!(redact(plain), plain, "kept whole");
        }
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
