//! Vendor and local connector surfaces.
//!
//! MCP, ACP, and the harness are local. Copilot, Claude, GPT, and Gemini
//! stay in mock mode unless a credential is set AND `GEN_AUDIO_CONNECTOR_LIVE=1`.
//! Mock completions do not call the network and are not model output.

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
        check_prompt(prompt)?;
        let token = first_env(&["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"]);
        if !live_enabled() || token.is_none() {
            return Ok(mock_completion("copilot", "github-models", prompt));
        }
        let token = token.unwrap();
        if let Ok(studio) = std::env::var("COPILOT_STUDIO_ENDPOINT") {
            let url = require_https_or_loopback(&studio)?;
            let body = json!({"prompt": prompt, "source": "gen-audio"});
            let text = send_json(&url, &[("authorization", &format!("Bearer {token}"))], &body)?;
            return Ok(Completion {
                connector_id: "copilot".into(),
                text,
                mock: false,
                model: "copilot-studio-endpoint".into(),
            });
        }
        let model = std::env::var("COPILOT_GITHUB_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        check_model(&model)?;
        let body = chat_body(&model, prompt);
        let text = send_json(
            "https://models.github.ai/inference/chat/completions",
            &[
                ("authorization", &format!("Bearer {token}")),
                ("accept", "application/vnd.github+json"),
            ],
            &body,
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
        vendor_health("claude", std::env::var("ANTHROPIC_API_KEY").is_ok(), &detail)
    }
    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError> {
        check_prompt(prompt)?;
        if std::env::var("GEN_AUDIO_CLAUDE_CODE_CLI").ok().as_deref() == Some("1") {
            return run_claude_code(prompt);
        }
        let Some(key) = std::env::var("ANTHROPIC_API_KEY").ok() else {
            return Ok(mock_completion("claude", "claude-api", prompt));
        };
        if !live_enabled() {
            return Ok(mock_completion("claude", "claude-api", prompt));
        }
        let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-3-5-haiku-latest".into());
        check_model(&model)?;
        let body = json!({
            "model": model,
            "max_tokens": 256,
            "messages": [{"role": "user", "content": prompt}]
        });
        let text = send_json(
            "https://api.anthropic.com/v1/messages",
            &[
                ("x-api-key", &key),
                ("anthropic-version", "2023-06-01"),
            ],
            &body,
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
            std::env::var("OPENAI_API_KEY").is_ok(),
            "OpenAI chat completions. Base URL must be https, or http loopback.",
        )
    }
    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError> {
        check_prompt(prompt)?;
        let Some(key) = std::env::var("OPENAI_API_KEY").ok() else {
            return Ok(mock_completion("gpt", "openai", prompt));
        };
        if !live_enabled() {
            return Ok(mock_completion("gpt", "openai", prompt));
        }
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        check_model(&model)?;
        let base = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let base = require_https_or_loopback(base.trim_end_matches('/'))?;
        let body = chat_body(&model, prompt);
        let text = send_json(
            &format!("{base}/chat/completions"),
            &[("authorization", &format!("Bearer {key}"))],
            &body,
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
            std::env::var("GEMINI_API_KEY").is_ok() || std::env::var("GOOGLE_API_KEY").is_ok(),
            "Gemini generateContent. The key is sent as the x-goog-api-key header, not the query string.",
        )
    }
    fn complete(&self, prompt: &str) -> Result<Completion, ConnectorError> {
        check_prompt(prompt)?;
        let key = std::env::var("GEMINI_API_KEY")
            .ok()
            .or_else(|| std::env::var("GOOGLE_API_KEY").ok());
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
            "contents": [{"role": "user", "parts": [{"text": prompt}]}]
        });
        let text = send_json(&url, &[("x-goog-api-key", &key)], &body)?;
        Ok(Completion {
            connector_id: "gemini".into(),
            text,
            mock: false,
            model,
        })
    }
}

pub fn chat_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 256
    })
}

pub fn claude_code_args(bin: &str, prompt: &str) -> Vec<String> {
    vec![
        bin.to_string(),
        "-p".into(),
        prompt.to_string(),
        "--output-format".into(),
        "text".into(),
    ]
}

fn run_claude_code(prompt: &str) -> Result<Completion, ConnectorError> {
    let bin = claude_bin().ok_or_else(|| {
        ConnectorError::new("GEN_AUDIO_CLAUDE_CODE_CLI=1 but no claude binary was found on PATH")
    })?;
    let args = claude_code_args(&bin.to_string_lossy(), prompt);
    let output = std::process::Command::new(&args[0])
        .args(&args[1..])
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .map_err(|err| ConnectorError::new(format!("claude binary failed to start: {err}")))?;
    if !output.status.success() {
        return Err(ConnectorError::new(format!(
            "claude binary exited {}: {}",
            output.status,
            tail(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    Ok(Completion {
        connector_id: "claude".into(),
        text: redact_secrets(&tail(&String::from_utf8_lossy(&output.stdout))),
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

fn live_enabled() -> bool {
    std::env::var("GEN_AUDIO_CONNECTOR_LIVE").ok().as_deref() == Some("1")
}

fn token_present(names: &[&str]) -> bool {
    names.iter().any(|name| std::env::var(name).ok().filter(|v| !v.is_empty()).is_some())
}

fn first_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    })
}

fn check_prompt(prompt: &str) -> Result<(), ConnectorError> {
    let len = prompt.chars().count();
    if len == 0 || len > 8_000 {
        return Err(ConnectorError::new("prompt must be 1 to 8000 characters"));
    }
    Ok(())
}

fn check_model(model: &str) -> Result<(), ConnectorError> {
    if model.is_empty()
        || model.len() > 80
        || !model
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/'))
        || model.contains("..")
        || model.starts_with('/')
    {
        return Err(ConnectorError::new("model id has unsupported characters"));
    }
    Ok(())
}

pub fn require_https_or_loopback(url: &str) -> Result<String, ConnectorError> {
    let ok = url.starts_with("https://")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("http://localhost");
    if !ok || url.contains("://") && url.chars().filter(|ch| *ch == ':').count() > 2 {
        return Err(ConnectorError::new(
            "endpoint must be https, or http on 127.0.0.1 / localhost",
        ));
    }
    Ok(url.to_string())
}

fn send_json(url: &str, headers: &[(&str, &str)], body: &Value) -> Result<String, ConnectorError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(25))
        .timeout_write(Duration::from_secs(8))
        .redirects(0)
        .build();
    let mut request = agent.request("POST", url);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = request
        .send_json(body.clone())
        .map_err(|err| ConnectorError::new(format!("vendor request failed: {err}")))?;
    let text = response
        .into_string()
        .map_err(|err| ConnectorError::new(format!("vendor response: {err}")))?;
    Ok(redact_secrets(&tail(&text)))
}

fn tail(text: &str) -> String {
    text.chars().take(1500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_connectors_and_vendor_mocks_do_not_pretend_to_be_models() {
        let reports = health_all();
        assert_eq!(reports.len(), 7);
        for id in ["copilot", "claude", "gpt", "gemini"] {
            std::env::remove_var(match id {
                "copilot" => "GITHUB_TOKEN",
                "claude" => "ANTHROPIC_API_KEY",
                "gpt" => "OPENAI_API_KEY",
                "gemini" => "GEMINI_API_KEY",
                _ => "UNUSED",
            });
        }
        std::env::remove_var("COPILOT_GITHUB_TOKEN");
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("GOOGLE_API_KEY");
        std::env::remove_var("GEN_AUDIO_CONNECTOR_LIVE");
        std::env::remove_var("GEN_AUDIO_CLAUDE_CODE_CLI");
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
    fn token_present_does_not_send() {
        std::env::set_var("OPENAI_API_KEY", "sk-testtoken123456");
        std::env::remove_var("GEN_AUDIO_CONNECTOR_LIVE");
        let health = health_one("gpt").unwrap();
        assert_eq!(health.mode, Mode::TokenPresent);
        assert!(!health.authenticated);
        let done = complete_one("gpt", "hello").unwrap();
        assert!(done.mock);
        assert!(!done.text.contains("sk-test"));
        std::env::remove_var("OPENAI_API_KEY");
    }

    #[test]
    fn chat_body_and_endpoint_rules() {
        let body = chat_body("gpt-4o-mini", "hi");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(require_https_or_loopback("http://example.com").is_err());
        assert!(require_https_or_loopback("https://api.openai.com/v1").is_ok());
        assert!(check_model("../etc").is_err());
        let args = claude_code_args("claude", "ping");
        assert_eq!(args[1], "-p");
        assert!(!args.iter().any(|arg| arg.contains("sh -c")));
    }
}
