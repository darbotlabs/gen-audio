//! Vendor and local connector surfaces.
//!
//! MCP, ACP, and the harness are local. Copilot, Claude, GPT, and Gemini
//! stay in mock mode unless a credential is set AND `GEN_AUDIO_CONNECTOR_LIVE=1`.
//! Mock completions do not call the network and are not model output.

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::collections::BTreeMap;
use std::net::ToSocketAddrs;
use std::time::Duration;

use gen_audio_core::redact::redact_secrets;
use serde::Serialize;
use serde_json::{json, Value};

pub const CONNECTOR_IDS: &[&str] = &[
    "mcp", "acp", "harness", "copilot", "claude", "gpt", "gemini",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Mock,
    Live,
    Local,
    TokenPresent,
    Misconfigured,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub connector_id: String,
    pub mode: Mode,
    pub authenticated: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Completion {
    pub connector_id: String,
    pub text: String,
    pub mock: bool,
    pub model: String,
}

#[derive(Debug)]
pub struct ConnectorError {
    pub message: String,
}

impl ConnectorError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: redact_secrets(&message.into()),
        }
    }
}

impl std::fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub trait Connector {
    fn id(&self) -> &'static str;
    fn health(&self) -> HealthReport;
    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError>;
    fn complete_with_limit(&self, prompt: &str, max_tokens: u32) -> Result<Completion, ConnectorError> {
        if max_tokens != 256 {
            return Err(ConnectorError::new("connector does not support adjustable output limits"));
        }
        self.complete(prompt)
    }
}

pub fn roster() -> Vec<Box<dyn Connector>> {
    vec![
        Box::new(LocalConnector::new(
            "mcp",
            "Local stateless MCP server (stdio and optional loopback HTTP). No vendor credential.",
        )),
        Box::new(LocalConnector::new(
            "acp",
            "Local Agent Client Protocol agent. Sessions exist only because ACP requires sessionId.",
        )),
        Box::new(LocalConnector::new(
            "harness",
            "Local harness plans tools and writes JSONL traces. It does not invent speech audio.",
        )),
        Box::new(CopilotConnector),
        Box::new(ClaudeConnector),
        Box::new(GptConnector),
        Box::new(GeminiConnector),
    ]
}

pub fn health_all() -> Vec<HealthReport> {
    roster().iter().map(|item| item.health()).collect()
}

pub fn health_one(id: &str) -> Result<HealthReport, ConnectorError> {
    roster()
        .into_iter()
        .find(|item| item.id() == id)
        .map(|item| item.health())
        .ok_or_else(|| ConnectorError::new(format!("unknown connector {id}")))
}

pub fn complete_one(id: &str, prompt: &str) -> Result<Completion, ConnectorError> {
    let connector = roster()
        .into_iter()
        .find(|item| item.id() == id)
        .ok_or_else(|| ConnectorError::new(format!("unknown connector {id}")))?;
    connector.complete(prompt)
}

pub fn complete_one_with_limit(id: &str, prompt: &str, max_tokens: u32) -> Result<Completion, ConnectorError> {
    check_completion_limit(max_tokens)?;
    let connector = roster().into_iter().find(|item| item.id() == id)
        .ok_or_else(|| ConnectorError::new(format!("unknown connector {id}")))?;
    connector.complete_with_limit(prompt, max_tokens)
}

struct LocalConnector {
    id: &'static str,
    detail: &'static str,
}

impl LocalConnector {
    fn new(id: &'static str, detail: &'static str) -> Self {
        Self { id, detail }
    }
}

impl Connector for LocalConnector {
    fn id(&self) -> &'static str {
        self.id
    }

    fn health(&self) -> HealthReport {
        HealthReport {
            connector_id: self.id.to_string(),
            mode: Mode::Local,
            authenticated: false,
            detail: self.detail.to_string(),
        }
    }

    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError> {
        check_prompt(prompt)?;
        Ok(Completion {
            connector_id: self.id.to_string(),
            text: format!(
                "local connector '{}' does not call a vendor model. Prompt length: {}.",
                self.id,
                prompt.chars().count()
            ),
            mock: false,
            model: "local".into(),
        })
    }
}

struct CopilotConnector;
struct ClaudeConnector;
struct GptConnector;
struct GeminiConnector;

impl Connector for CopilotConnector {
    fn id(&self) -> &'static str {
        "copilot"
    }
    fn health(&self) -> HealthReport {
        vendor_health(
            "copilot",
            token_present(&["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"]),
            "GitHub Models / optional Copilot Studio endpoint. Mock until GEN_AUDIO_CONNECTOR_LIVE=1.",
        )
    }
    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError> {
        self.complete_with_limit(prompt, 256)
    }
    fn complete_with_limit(&self, prompt: &str, max_tokens: u32) -> Result<Completion, ConnectorError> {
        check_completion_limit(max_tokens)?;
        check_prompt(prompt)?;
        let token = first_env(&["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"]);
        if !live_enabled() || token.is_none() {
            return Ok(mock_completion("copilot", "github-models", prompt));
        }
        let token = token.unwrap();
        if let Ok(studio) = std::env::var("COPILOT_STUDIO_ENDPOINT") {
            let url = require_https_or_loopback(&studio)?;
            let body = json!({"prompt": prompt, "source": "gen-audio", "max_tokens": max_tokens});
            let text = send_json(&url, &[("authorization", &bearer(&token)?)], &body, Provider::Studio)?;
            return Ok(Completion {
                connector_id: "copilot".into(),
                text,
                mock: false,
                model: "copilot-studio-endpoint".into(),
            });
        }
        let model = std::env::var("COPILOT_GITHUB_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        check_model(&model)?;
        let body = chat_body_with_limit(&model, prompt, max_tokens);
        let text = send_json(
            "https://models.github.ai/inference/chat/completions",
            &[
                ("authorization", &bearer(&token)?),
                ("accept", "application/vnd.github+json"),
            ],
            &body,
            Provider::OpenAi,
        )?;
        Ok(Completion {
            connector_id: "copilot".into(),
            text,
            mock: false,
            model,
        })
    }
}

impl Connector for ClaudeConnector {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn health(&self) -> HealthReport {
        let mut detail = "Anthropic Messages API. Claude Code CLI is not spawned unless GEN_AUDIO_CLAUDE_CODE_CLI=1.".to_string();
        if claude_bin().is_some() {
            detail.push_str(" claude binary is on PATH.");
        }
        vendor_health("claude", env_var("ANTHROPIC_API_KEY").is_ok(), &detail)
    }
    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError> {
        self.complete_with_limit(prompt, 256)
    }
    fn complete_with_limit(&self, prompt: &str, max_tokens: u32) -> Result<Completion, ConnectorError> {
        check_completion_limit(max_tokens)?;
        check_prompt(prompt)?;
        if env_var("GEN_AUDIO_CLAUDE_CODE_CLI").ok().as_deref() == Some("1") {
            if max_tokens != 256 {
                return Err(ConnectorError::new("drafting requires the Anthropic API; Claude CLI does not expose the requested token budget"));
            }
            return run_claude_code(prompt);
        }
        let Some(key) = env_var("ANTHROPIC_API_KEY").ok() else {
            return Ok(mock_completion("claude", "claude-api", prompt));
        };
        if !live_enabled() {
            return Ok(mock_completion("claude", "claude-api", prompt));
        }
        let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-3-5-haiku-latest".into());
        check_model(&model)?;
        let body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": [{"role": "user", "content": prompt}]
        });
        let text = send_json(
            "https://api.anthropic.com/v1/messages",
            &[
                ("x-api-key", &key),
                ("anthropic-version", "2023-06-01"),
            ],
            &body,
            Provider::Anthropic,
        )?;
        Ok(Completion {
            connector_id: "claude".into(),
            text,
            mock: false,
            model,
        })
    }
}

impl Connector for GptConnector {
    fn id(&self) -> &'static str {
        "gpt"
    }
    fn health(&self) -> HealthReport {
        vendor_health(
            "gpt",
            env_var("OPENAI_API_KEY").is_ok(),
            "OpenAI chat completions. Base URL must be https, or http loopback.",
        )
    }
    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError> {
        self.complete_with_limit(prompt, 256)
    }
    fn complete_with_limit(&self, prompt: &str, max_tokens: u32) -> Result<Completion, ConnectorError> {
        check_completion_limit(max_tokens)?;
        check_prompt(prompt)?;
        let Some(key) = env_var("OPENAI_API_KEY").ok() else {
            return Ok(mock_completion("gpt", "openai", prompt));
        };
        if !live_enabled() {
            return Ok(mock_completion("gpt", "openai", prompt));
        }
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        check_model(&model)?;
        let base = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let base = require_https_or_loopback(base.trim_end_matches('/'))?;
        let body = chat_body_with_limit(&model, prompt, max_tokens);
        let text = send_json(
            &format!("{base}/chat/completions"),
            &[("authorization", &bearer(&key)?)],
            &body,
            Provider::OpenAi,
        )?;
        Ok(Completion {
            connector_id: "gpt".into(),
            text,
            mock: false,
            model,
        })
    }
}

impl Connector for GeminiConnector {
    fn id(&self) -> &'static str {
        "gemini"
    }
    fn health(&self) -> HealthReport {
        vendor_health(
            "gemini",
            env_var("GEMINI_API_KEY").is_ok() || env_var("GOOGLE_API_KEY").is_ok(),
            "Gemini generateContent. The key is sent as the x-goog-api-key header, not the query string.",
        )
    }
    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError> {
        self.complete_with_limit(prompt, 256)
    }
    fn complete_with_limit(&self, prompt: &str, max_tokens: u32) -> Result<Completion, ConnectorError> {
        check_completion_limit(max_tokens)?;
        check_prompt(prompt)?;
        let key = env_var("GEMINI_API_KEY")
            .ok()
            .or_else(|| env_var("GOOGLE_API_KEY").ok());
        let Some(key) = key else {
            return Ok(mock_completion("gemini", "gemini", prompt));
        };
        if !live_enabled() {
            return Ok(mock_completion("gemini", "gemini", prompt));
        }
        let model = std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.0-flash".into());
        check_model(&model)?;
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
        );
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": prompt}]}],
            "generationConfig": {"maxOutputTokens": max_tokens}
        });
        let text = send_json(&url, &[("x-goog-api-key", &key)], &body, Provider::Gemini)?;
        Ok(Completion {
            connector_id: "gemini".into(),
            text,
            mock: false,
            model,
        })
    }
}

pub fn chat_body(model: &str, prompt: &str) -> Value {
    chat_body_with_limit(model, prompt, 256)
}

fn chat_body_with_limit(model: &str, prompt: &str, max_tokens: u32) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens
    })
}

pub fn claude_code_args(bin: &str) -> Vec<String> {
    vec![
        bin.to_string(),
        "-p".into(),
        "--output-format".into(),
        "text".into(),
    ]
}

fn run_claude_code(prompt: &str) -> Result<Completion, ConnectorError> {
    let bin = claude_bin().ok_or_else(|| {
        ConnectorError::new("GEN_AUDIO_CLAUDE_CODE_CLI=1 but no claude binary was found on PATH")
    })?;
    let name = bin.file_name().and_then(|part| part.to_str()).unwrap_or("");
    if name != "claude" && name != "claude.exe" {
        return Err(ConnectorError::new(
            "CLAUDE_CODE_BIN must point at a file named claude",
        ));
    }
    let mut child = std::process::Command::new(&bin);
    child
        .args(["-p", "--output-format", "text"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for key in [
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "COPILOT_GITHUB_TOKEN",
    ] {
        child.env_remove(key);
    }
    let output = gen_audio_core::subprocess::run(
        &mut child, prompt.as_bytes(), gen_audio_core::subprocess::Limits::default(),
    ).map_err(ConnectorError::new)?;
    if !output.status.success() {
        return Err(ConnectorError::new(format!(
            "claude binary exited {}",
            output.status
        )));
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| ConnectorError::new("claude returned invalid UTF-8"))?.trim();
    if text.is_empty() { return Err(ConnectorError::new("claude returned no completion text")); }
    Ok(Completion {
        connector_id: "claude".into(),
        text: redact_secrets(text),
        mock: false,
        model: "claude-code-cli".into(),
    })
}

fn claude_bin() -> Option<std::path::PathBuf> {
    let configured = std::env::var("CLAUDE_CODE_BIN").ok();
    let name = configured.as_deref().unwrap_or("claude");
    which(name)
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    if name.contains('\0') || name.contains("..") {
        return None;
    }
    if name.contains('/') || name.contains('\\') {
        let path = std::path::PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn vendor_health(id: &str, token: bool, detail: &str) -> HealthReport {
    let mode = if !token {
        Mode::Mock
    } else if live_enabled() {
        Mode::Live
    } else {
        Mode::TokenPresent
    };
    HealthReport {
        connector_id: id.to_string(),
        mode,
        authenticated: token && live_enabled(),
        detail: if token && !live_enabled() {
            format!("{detail} A credential is set, but live calls stay off until GEN_AUDIO_CONNECTOR_LIVE=1.")
        } else {
            detail.to_string()
        },
    }
}

fn mock_completion(id: &str, model: &str, prompt: &str) -> Completion {
    Completion {
        connector_id: id.into(),
        text: format!(
            "mock connector '{id}' did not call an API and this text is not model output. Prompt length: {}.",
            prompt.chars().count()
        ),
        mock: true,
        model: model.into(),
    }
}

// The process environment is one table. Tests that need a credential present
// or absent install an overlay for this thread instead of `set_var`, so a
// neighbour cannot observe the mutation. Unlisted keys still read the process.
#[cfg(test)]
thread_local! {
    static ENV_OVERLAY: RefCell<Option<BTreeMap<String, Option<String>>>> = const { RefCell::new(None) };
}

#[cfg(test)]
struct EnvGuard {
    previous: Option<BTreeMap<String, Option<String>>>,
}

#[cfg(test)]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ENV_OVERLAY.with(|slot| *slot.borrow_mut() = previous);
    }
}

#[cfg(test)]
fn bind_env(values: BTreeMap<String, Option<String>>) -> EnvGuard {
    ENV_OVERLAY.with(|slot| {
        let previous = slot.borrow_mut().replace(values);
        EnvGuard { previous }
    })
}

fn env_var(name: &str) -> Result<String, std::env::VarError> {
    #[cfg(test)]
    {
        let over = ENV_OVERLAY.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|map| map.get(name).cloned())
        });
        if let Some(value) = over {
            return match value {
                Some(text) => Ok(text),
                None => Err(std::env::VarError::NotPresent),
            };
        }
    }
    std::env::var(name)
}

fn live_enabled() -> bool {
    env_var("GEN_AUDIO_CONNECTOR_LIVE").ok().as_deref() == Some("1")
}

fn token_present(names: &[&str]) -> bool {
    names.iter().any(|name| env_var(name).ok().filter(|v| !v.is_empty()).is_some())
}

fn first_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        env_var(name).ok().filter(|value| !value.is_empty())
    })
}

fn check_prompt(prompt: &str) -> Result<(), ConnectorError> {
    let len = prompt.chars().count();
    if len == 0 || len > 8_000 {
        return Err(ConnectorError::new("prompt must be 1 to 8000 characters"));
    }
    Ok(())
}

fn check_completion_limit(max_tokens: u32) -> Result<(), ConnectorError> {
    if !(256..=4096).contains(&max_tokens) {
        return Err(ConnectorError::new("completion token budget must be 256 to 4096"));
    }
    Ok(())
}

fn check_model(model: &str) -> Result<(), ConnectorError> {
    if model.is_empty()
        || model.len() > 80
        || !model
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        || model.contains("..")
    {
        return Err(ConnectorError::new("model id has unsupported characters"));
    }
    Ok(())
}

fn header_value_ok(value: &str) -> Result<(), ConnectorError> {
    if value.is_empty() || value.len() > 8192 || value.chars().any(|ch| ch.is_control()) {
        return Err(ConnectorError::new("header value contains control characters"));
    }
    Ok(())
}

fn bearer(token: &str) -> Result<String, ConnectorError> {
    header_value_ok(token)?;
    Ok(format!("Bearer {token}"))
}

pub fn require_https_or_loopback(url: &str) -> Result<String, ConnectorError> {
    parse_endpoint(url).map_err(ConnectorError::new)?;
    if url.starts_with("https://") && !loopback_endpoint(&url) {
        assert_public_destination(&url).map_err(ConnectorError::new)?;
    }
    Ok(url.to_string())
}

fn loopback_endpoint(url: &str) -> bool {
    let Ok((scheme, host, _, _)) = parse_endpoint(url) else {
        return false;
    };
    scheme == "http" || host == "127.0.0.1" || host == "localhost" || host == "::1"
}

fn parse_endpoint(url: &str) -> Result<(String, String, Option<u16>, String), String> {
    if url.len() < 8 || url.len() > 300 {
        return Err("endpoint length is out of range".into());
    }
    if url.chars().any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '\\' | '@' | '?' | '#')) {
        return Err("endpoint contains credentials, whitespace, or a query string".into());
    }
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| "endpoint is missing a scheme".to_string())?;
    if scheme != "https" && scheme != "http" {
        return Err("endpoint scheme must be https or http".into());
    }
    let (hostport, path) = match rest.split_once('/') {
        Some((hostport, path)) => (hostport, format!("/{path}")),
        None => (rest, String::new()),
    };
    if hostport.is_empty() || path.contains("//") {
        return Err("endpoint host is empty".into());
    }
    let (host, port) = split_host_port(hostport)?;
    if scheme == "http" && host != "127.0.0.1" && host != "localhost" {
        return Err("http is only allowed for 127.0.0.1 and localhost".into());
    }
    if host.contains("..") || host.starts_with('.') || host.ends_with('.') {
        return Err("endpoint host is not a single name".into());
    }
    Ok((scheme.to_string(), host, port, path))
}

fn split_host_port(hostport: &str) -> Result<(String, Option<u16>), String> {
    if let Some(inner) = hostport.strip_prefix('[') {
        let (host, rest) = inner
            .split_once(']')
            .ok_or_else(|| "bad ipv6 host".to_string())?;
        let port = if let Some(raw) = rest.strip_prefix(':') {
            Some(parse_port(raw)?)
        } else if rest.is_empty() {
            None
        } else {
            return Err("bad ipv6 host".into());
        };
        if host != "::1" {
            return Err("ipv6 http endpoints must be [::1]".into());
        }
        return Ok((host.to_string(), port));
    }
    let (host, port) = match hostport.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) => {
            (host, Some(parse_port(port)?))
        }
        _ => (hostport, None),
    };
    if host.is_empty() || host.contains(':') {
        return Err("endpoint host is invalid".into());
    }
    Ok((host.to_string(), port))
}

fn parse_port(raw: &str) -> Result<u16, String> {
    let port: u16 = raw.parse().map_err(|_| "bad port".to_string())?;
    if port == 0 {
        return Err("port must be between 1 and 65535".into());
    }
    Ok(port)
}

fn assert_public_destination(url: &str) -> Result<(), String> {
    let (_, host, port, _) = parse_endpoint(url)?;
    if is_blocked_name(&host) {
        return Err("endpoint host is not a public name".into());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip_blocked(ip) {
            return Err("endpoint address is not public".into());
        }
        return Ok(());
    }
    let allow_private = std::env::var("GEN_AUDIO_CONNECTOR_ALLOW_PRIVATE").ok().as_deref() == Some("1");
    if allow_private {
        return Ok(());
    }
    let socket_port = port.unwrap_or(443);
    let looked_up = (host.as_str(), socket_port)
        .to_socket_addrs()
        .map_err(|_| "endpoint host did not resolve".to_string())?;
    let mut saw = false;
    for addr in looked_up {
        saw = true;
        if ip_blocked(addr.ip()) {
            return Err("endpoint resolved to a non-public address".into());
        }
    }
    if !saw {
        return Err("endpoint host did not resolve".into());
    }
    Ok(())
}

fn is_blocked_name(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "metadata.google.internal"
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".localhost")
}

fn ip_blocked(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (b & 0b1100_0000) == 64)
                || (a == 192 && b == 0 && v4.octets()[2] == 2)
                || (a == 198 && b == 51 && v4.octets()[2] == 100)
                || (a == 203 && b == 0 && v4.octets()[2] == 113)
        }
        std::net::IpAddr::V6(v6) => {
            v6.to_ipv4_mapped().is_some_and(|ip| ip_blocked(std::net::IpAddr::V4(ip)))
                || v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

#[derive(Clone, Copy)]
enum Provider { OpenAi, Anthropic, Gemini, Studio }

fn completion_text(provider: Provider, value: &Value) -> Result<String, ConnectorError> {
    let truncated = match provider {
        Provider::OpenAi => value.pointer("/choices/0/finish_reason") == Some(&json!("length")),
        Provider::Anthropic => value.get("stop_reason") == Some(&json!("max_tokens")),
        Provider::Gemini => value.pointer("/candidates/0/finishReason") == Some(&json!("MAX_TOKENS")),
        Provider::Studio => value.get("truncated") == Some(&json!(true)),
    };
    if truncated {
        return Err(ConnectorError::new("provider reached its output limit; incomplete text was not accepted"));
    }
    let text = match provider {
        Provider::OpenAi => value.pointer("/choices/0/message/content").and_then(Value::as_str).map(str::to_owned),
        Provider::Studio => value.get("text").and_then(Value::as_str).map(str::to_owned),
        Provider::Anthropic => value["content"].as_array().map(|blocks| blocks.iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n")),
        Provider::Gemini => value.pointer("/candidates/0/content/parts").and_then(Value::as_array).map(|parts| parts.iter()
            .filter(|part| part.get("thought") != Some(&Value::Bool(true)))
            .filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n")),
    }.filter(|s| !s.trim().is_empty()).ok_or_else(|| ConnectorError::new("vendor returned no completion text"))?;
    if text.len() > 64 * 1024 { return Err(ConnectorError::new("vendor completion exceeds byte limit")); }
    Ok(redact_secrets(&text))
}

fn send_json(url: &str, headers: &[(&str, &str)], body: &Value, provider: Provider) -> Result<String, ConnectorError> {
    use std::io::Read;
    require_https_or_loopback(url)?;
    for (_, value) in headers {
        header_value_ok(value)?;
    }
    let allow_loopback = loopback_endpoint(url);
    let allow_private = std::env::var("GEN_AUDIO_CONNECTOR_ALLOW_PRIVATE").ok().as_deref() == Some("1");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(25))
        .timeout_write(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .redirects(0)
        .resolver(move |address: &str| {
            let addresses: Vec<_> = address.to_socket_addrs()?.collect();
            if addresses.is_empty() || addresses.iter().any(|a| {
                if allow_loopback { !a.ip().is_loopback() } else { !allow_private && ip_blocked(a.ip()) }
            }) {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "blocked connector destination"));
            }
            Ok(addresses)
        })
        .build();
    let mut request = agent.request("POST", url);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    if serde_json::to_vec(body).map_err(|_| ConnectorError::new("invalid request JSON"))?.len() > 64 * 1024 {
        return Err(ConnectorError::new("vendor request exceeds byte limit"));
    }
    let response = request
        .send_json(body.clone())
        .map_err(|err| match err {
            ureq::Error::Status(status, _) => ConnectorError::new(format!("vendor request failed (HTTP {status})")),
            _ => ConnectorError::new("vendor transport failed"),
        })?;
    if !(200..300).contains(&response.status()) {
        return Err(ConnectorError::new("vendor returned a non-success status"));
    }
    let mut bytes = Vec::new();
    response.into_reader().take(128 * 1024 + 1).read_to_end(&mut bytes)
        .map_err(|_| ConnectorError::new("vendor response read failed"))?;
    if bytes.len() > 128 * 1024 { return Err(ConnectorError::new("vendor response exceeds byte limit")); }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ConnectorError::new("vendor returned invalid JSON"))?;
    completion_text(provider, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn credentials_absent() -> BTreeMap<String, Option<String>> {
        [
            "COPILOT_GITHUB_TOKEN",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GEN_AUDIO_CONNECTOR_LIVE",
            "GEN_AUDIO_CLAUDE_CODE_CLI",
        ]
        .into_iter()
        .map(|name| (name.to_string(), None))
        .collect()
    }

    #[test]
    fn draft_token_budgets_are_explicit_and_truncated_completions_fail() {
        assert!(check_completion_limit(256).is_ok());
        assert!(check_completion_limit(4096).is_ok());
        assert!(check_completion_limit(4097).is_err());
        assert!(check_completion_limit(0).is_err());
        assert_eq!(chat_body_with_limit("test-model", "dialogue", 2048)["max_tokens"], 2048);
        for (provider, response) in [
            (Provider::OpenAi, json!({"choices":[{"finish_reason":"length","message":{"content":"Speaker 1: Truncated"}}]})),
            (Provider::Anthropic, json!({"stop_reason":"max_tokens","content":[{"type":"text","text":"Speaker 1: Truncated"}]})),
            (Provider::Gemini, json!({"candidates":[{"finishReason":"MAX_TOKENS","content":{"parts":[{"text":"Speaker 1: Truncated"}]}}]})),
            (Provider::Studio, json!({"truncated":true,"text":"Speaker 1: Truncated"})),
        ] {
            assert!(completion_text(provider, &response).unwrap_err().message.contains("incomplete"));
        }
    }

    #[test]
    fn seven_connectors_and_vendor_mocks_do_not_pretend_to_be_models() {
        let reports = health_all();
        assert_eq!(reports.len(), 7);
        let _env = bind_env(credentials_absent());
        for id in ["copilot", "claude", "gpt", "gemini"] {
            let health = health_one(id).unwrap();
            assert_eq!(health.mode, Mode::Mock);
            let done = complete_one(id, "status please").unwrap();
            assert!(done.mock);
            assert!(done.text.contains("not model output"));
        }
        assert_eq!(health_one("mcp").unwrap().mode, Mode::Local);
    }

    #[test]
    fn empty_overlay_matches_a_real_empty_environment_variable() {
        // std::env::var returns Ok("") for a variable that is set and empty.
        // gpt health uses that is_ok() check, so an empty key is TokenPresent
        // and live stays off. Treating "" as absent would prove the wrong thing.
        let mut env = credentials_absent();
        env.insert("OPENAI_API_KEY".into(), Some(String::new()));
        let _env = bind_env(env);
        let health = health_one("gpt").unwrap();
        assert_eq!(health.mode, Mode::TokenPresent);
        assert!(!health.authenticated);
        let done = complete_one("gpt", "hello").unwrap();
        assert!(done.mock);
    }

    #[test]
    fn token_present_does_not_send() {
        let mut env = credentials_absent();
        env.insert("OPENAI_API_KEY".into(), Some("sk-testtoken123456".into()));
        let _env = bind_env(env);
        let health = health_one("gpt").unwrap();
        assert_eq!(health.mode, Mode::TokenPresent);
        assert!(!health.authenticated);
        let done = complete_one("gpt", "hello").unwrap();
        assert!(done.mock);
        assert!(!done.text.contains("sk-test"));
    }

    #[test]
    fn chat_body_and_endpoint_rules() {
        let body = chat_body("gpt-4o-mini", "hi");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(require_https_or_loopback("http://example.com").is_err());
        assert!(require_https_or_loopback("http://127.0.0.1.evil.com").is_err());
        assert!(require_https_or_loopback("http://localhost.attacker.com").is_err());
        assert!(require_https_or_loopback("https://user:token@api.openai.com/v1").is_err());
        assert!(require_https_or_loopback("https://169.254.169.254/latest").is_err());
        assert!(require_https_or_loopback("http://127.0.0.1:9/v1").is_ok());
        assert!(parse_endpoint("https://api.openai.com/v1").is_ok());
        assert!(check_model("../etc").is_err());
        assert!(check_model("gemini/flash").is_err());
        let args = claude_code_args("claude");
        assert_eq!(args[1], "-p");
        assert!(!args.iter().any(|arg| arg.contains("ping") || arg.contains("sh -c")));
    }

    fn mock_request(status: u16, body: String, provider: Provider, headers: &[(&str, &str)])
        -> (Result<String, ConnectorError>, String)
    {
        use std::io::{BufRead, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/completion", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream,_)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(10)),
                    Err(e) => panic!("mock accept failed: {e}"),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut captured = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" { break; }
                if let Some((key,value)) = line.split_once(':') {
                    if key.eq_ignore_ascii_case("content-length") { length = value.trim().parse::<usize>().unwrap(); }
                }
                captured.push_str(&line);
                assert!(captured.len() <= 16384);
            }
            assert!(length < 65536);
            let mut request_body = vec![0; length];
            reader.read_exact(&mut request_body).unwrap();
            captured.push_str(std::str::from_utf8(&request_body).unwrap());
            let response = format!("HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            let _ = stream.write_all(response.as_bytes());
            captured
        });
        let result = send_json(&url,headers,&chat_body("mock-model","only a loopback test"),provider);
        (result, worker.join().unwrap())
    }

    #[test]
    fn mocked_http_extracts_provider_text_and_sends_real_header_values() {
        let auth = bearer("local-test-token").unwrap();
        for (provider, body, header, value, expected) in [
            (Provider::OpenAi, json!({"choices":[{"message":{"content":"openai text"}}]}), "authorization", auth.as_str(), "openai text"),
            (Provider::Studio, json!({"text":"studio text"}), "authorization", auth.as_str(), "studio text"),
            (Provider::Anthropic, json!({"content":[{"type":"thinking","thinking":"hidden"},{"type":"text","text":"anthropic text"}]}), "x-api-key", "local-anthropic-test", "anthropic text"),
            (Provider::Gemini, json!({"candidates":[{"content":{"parts":[{"text":"hidden","thought":true},{"text":"gemini text"}]}}]}), "x-goog-api-key", "local-gemini-test", "gemini text"),
        ] {
            let (result,request) = mock_request(200, body.to_string(),provider,&[(header,value)]);
            assert_eq!(result.unwrap(),expected);
            assert!(request.to_ascii_lowercase().contains(&format!("{header}: {}",value.to_ascii_lowercase())));
            assert!(!request.contains("******"));
        }
    }

    #[test]
    fn mocked_http_rejects_invalid_oversized_and_error_bodies_safely() {
        for (status, body, hint) in [
            (401, "{\"key\":\"must-not-leak\"}".to_string(), "HTTP 401"),
            (200, "must-not-leak".to_string(), "invalid JSON"),
            (200, "{}".to_string(), "no completion"),
            (200, "x".repeat(128*1024+1), "byte limit"),
            (302, "{}".to_string(), "non-success"),
        ] {
            let (result,_) = mock_request(status,body,Provider::OpenAi,&[]);
            let error = result.unwrap_err().message;
            assert!(error.contains(hint),"{error}");
            assert!(!error.contains("must-not-leak"));
        }
        assert!(bearer("a\r\nInjected: secret").is_err());
        assert!(ip_blocked("::ffff:127.0.0.1".parse().unwrap()));
        assert!(parse_endpoint("http://127.0.0.1:65536").is_err());
    }
}
