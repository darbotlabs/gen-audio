//! Build argv for the Python CLIs. The process is spawned with an argument
//! vector, never a shell string.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::paths::{read_trusted_script, read_user_repo_file, Scratch};
use crate::redact::redact_secrets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonTool {
    Improve,
    Spectrogram,
    Cube,
    Compare,
    Synth,
}

impl PythonTool {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "improve" => Some(Self::Improve),
            "spectrogram" => Some(Self::Spectrogram),
            "cube_revision" | "cube" => Some(Self::Cube),
            "compare" => Some(Self::Compare),
            "synth" => Some(Self::Synth),
            _ => None,
        }
    }

    fn script(self) -> &'static str {
        match self {
            Self::Improve => "scripts/improve.py",
            Self::Spectrogram => "scripts/spectrogram.py",
            Self::Cube => "scripts/cube_revision.py",
            Self::Compare => "scripts/compare_wavs.py",
            Self::Synth => "scripts/synth_kokoro_onnx.py",
        }
    }
}

#[derive(Debug)]
pub struct PythonPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

pub fn python_program() -> Result<PathBuf, String> {
    let fallback = if cfg!(windows) { "python" } else { "python3" };
    let raw = std::env::var("GEN_AUDIO_PYTHON").unwrap_or_else(|_| fallback.to_string());
    if raw.is_empty() || raw.contains('\0') || raw.contains("..") {
        return Err("GEN_AUDIO_PYTHON is empty or contains ..".into());
    }
    let path = PathBuf::from(&raw);
    let name = path
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or("");
    const ALLOWED: &[&str] = &["python", "python3", "python.exe", "python3.exe", "py", "py.exe"];
    if !ALLOWED.contains(&name) {
        return Err("GEN_AUDIO_PYTHON basename must be python, python3, or py".into());
    }
    Ok(path)
}

pub fn plan(
    tool: PythonTool,
    repo: &Path,
    scratch: &Scratch,
    args: &Value,
) -> Result<PythonPlan, String> {
    let script = read_trusted_script(repo, tool.script())?;
    let mut planned = vec![script.to_string_lossy().to_string()];
    match tool {
        PythonTool::Improve => {
            let input = scratch.open_input(req_name(args, "input")?)?;
            let output = scratch.prepare_output(req_name(args, "output")?)?;
            planned.push(input.to_string_lossy().to_string());
            planned.push("-o".into());
            planned.push(output.to_string_lossy().to_string());
        }
        PythonTool::Spectrogram => {
            let before = scratch.open_input(req_name(args, "before")?)?;
            planned.push("--before".into());
            planned.push(before.to_string_lossy().to_string());
            if let Some(after) = opt_name(args, "after")? {
                let after = scratch.open_input(after)?;
                planned.push("--after".into());
                planned.push(after.to_string_lossy().to_string());
            }
            planned.push("--out-dir".into());
            planned.push(scratch.dir.to_string_lossy().to_string());
            planned.push("--title".into());
            planned.push("gen-audio fixture".into());
        }
        PythonTool::Cube => {
            let input = scratch.open_input(req_name(args, "input")?)?;
            let output = scratch.prepare_output(req_name(args, "output")?)?;
            planned.push(input.to_string_lossy().to_string());
            planned.push("-o".into());
            planned.push(output.to_string_lossy().to_string());
            let steps = args.get("maxSteps").and_then(Value::as_u64).unwrap_or(4);
            if steps > 8 {
                return Err("maxSteps must be from 0 to 8".into());
            }
            planned.push("--max-steps".into());
            planned.push(steps.to_string());
        }
        PythonTool::Compare => {
            let engine = req_string(args, "engine")?;
            if !engine
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
                || engine.len() > 32
            {
                return Err("engine id must be a short lowercase slug".into());
            }
            let wav = scratch.open_input(req_name(args, "input")?)?;
            let output = scratch.prepare_output("compare-scores.json")?;
            planned.push(format!("{engine}={}", wav.to_string_lossy()));
            planned.push("-o".into());
            planned.push(output.to_string_lossy().to_string());
        }
        PythonTool::Synth => {
            let script_rel = args
                .get("script")
                .and_then(Value::as_str)
                .unwrap_or("examples/podcast_script_sample.txt");
            let cast_rel = args
                .get("castMap")
                .and_then(Value::as_str)
                .unwrap_or("voices/cast_map.example.json");
            let script_path = read_user_repo_file(repo, script_rel)?;
            let cast_path = read_user_repo_file(repo, cast_rel)?;
            let output = scratch.prepare_output(req_name(args, "output")?)?;
            let model = std::env::var("GEN_AUDIO_KOKORO_MODEL").unwrap_or_default();
            let voices = std::env::var("GEN_AUDIO_KOKORO_VOICES").unwrap_or_default();
            if model.is_empty() || voices.is_empty() {
                return Err(
                    "synth was not started: GEN_AUDIO_KOKORO_MODEL and GEN_AUDIO_KOKORO_VOICES are unset. No speech was invented."
                        .into(),
                );
            }
            let model_path = bounded_model_file(&model)?;
            let voices_path = bounded_model_file(&voices)?;
            planned.push(script_path.to_string_lossy().to_string());
            planned.push("--cast-map".into());
            planned.push(cast_path.to_string_lossy().to_string());
            planned.push("-o".into());
            planned.push(output.to_string_lossy().to_string());
            planned.push("--model".into());
            planned.push(model_path.to_string_lossy().to_string());
            planned.push("--voices".into());
            planned.push(voices_path.to_string_lossy().to_string());
        }
    }
    Ok(PythonPlan {
        program: python_program()?,
        args: planned,
        cwd: repo.to_path_buf(),
    })
}

pub fn run_plan(plan: &PythonPlan) -> Result<Value, String> {
    let mut command = Command::new(&plan.program);
    command.args(&plan.args).current_dir(&plan.cwd);
    for key in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "COPILOT_GITHUB_TOKEN",
    ] {
        command.env_remove(key);
    }
    let output = command
        .output()
        .map_err(|err| format!("failed to start python: {err}"))?;
    let stdout = redact_secrets(&String::from_utf8_lossy(&output.stdout));
    let stderr = redact_secrets(&String::from_utf8_lossy(&output.stderr));
    Ok(json!({
        "ok": output.status.success(),
        "code": output.status.code(),
        "stdout": tail(&stdout),
        "stderr": tail(&stderr),
        "synthesizedSpeech": false
    }))
}

fn bounded_model_file(raw: &str) -> Result<PathBuf, String> {
    let root = std::env::var("GEN_AUDIO_MODEL_DIR")
        .map_err(|_| "GEN_AUDIO_MODEL_DIR is required so tool arguments cannot point at arbitrary files".to_string())?;
    let root = PathBuf::from(root)
        .canonicalize()
        .map_err(|err| format!("GEN_AUDIO_MODEL_DIR: {err}"))?;
    let path = PathBuf::from(raw)
        .canonicalize()
        .map_err(|err| format!("model path: {err}"))?;
    if !path.starts_with(&root) {
        return Err("model path is outside GEN_AUDIO_MODEL_DIR".into());
    }
    if !path.is_file() {
        return Err("model path is not a file".into());
    }
    Ok(path)
}

fn req_name<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {key}"))
}

fn opt_name<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        _ => Err(format!("field {key} must be a string")),
    }
}

fn req_string<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    req_name(args, key)
}

fn tail(text: &str) -> String {
    let max = 2000;
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

/// The env vars a real synth reads.
#[cfg(any(test, feature = "test-support"))]
pub const KOKORO_ENV: [&str; 2] = ["GEN_AUDIO_KOKORO_MODEL", "GEN_AUDIO_KOKORO_VOICES"];
#[cfg(any(test, feature = "test-support"))]
static KOKORO_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test support only: clears the `GEN_AUDIO_KOKORO_*` vars for one test and
/// puts them back on drop, so a developer shell with real models set cannot
/// start a real synth from a unit test (PR #5 review fix 6; verification E3).
/// One process-wide lock, so every crate's tests in a binary share it.
///
/// Compiled only under `cfg(test)` or the `test-support` feature, which
/// gen-audio-mcp and gen-audio-acp enable in their `[dev-dependencies]`.
/// Release and normal builds of every crate never contain it (resolver 2
/// keeps dev-dependency features out of normal builds).
#[cfg(any(test, feature = "test-support"))]
pub struct NoKokoroEnv {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(any(test, feature = "test-support"))]
impl NoKokoroEnv {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let lock = KOKORO_ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = KOKORO_ENV.iter().map(|name| (*name, std::env::var_os(name))).collect();
        for name in KOKORO_ENV {
            std::env::remove_var(name);
        }
        Self { saved, _lock: lock }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for NoKokoroEnv {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// G: NoKokoroEnv and its env list exist only for tests (cfg(test) or the
    /// test-support feature), and no crate enables that feature outside
    /// [dev-dependencies].
    #[test]
    fn no_kokoro_env_is_test_support_only() {
        let source = include_str!("bridge.rs");
        let gate = "#[cfg(any(test, feature = \"test-support\"))]\n";
        for item in ["pub const KOKORO_ENV", "static KOKORO_ENV_LOCK", "pub struct NoKokoroEnv", "impl NoKokoroEnv", "impl Drop for NoKokoroEnv"] {
            let at = source.find(&format!("\n{item}")).unwrap_or_else(|| panic!("{item} not found"));
            assert!(source[..at + 1].ends_with(gate), "{item} must sit right under {gate}");
        }
        for manifest in [include_str!("../../gen-audio-mcp/Cargo.toml"), include_str!("../../gen-audio-acp/Cargo.toml")] {
            let (normal, dev) = manifest.split_once("[dev-dependencies]").expect("a [dev-dependencies] table");
            assert!(!normal.contains("test-support"), "test-support enabled outside [dev-dependencies]");
            assert!(dev.contains("features = [\"test-support\"]"));
        }
    }
    use crate::fixture::write_fixture_tone;
    use serde_json::json;

    #[test]
    fn improve_plan_rejects_paths_outside_work() {
        let scratch = Scratch::create().unwrap();
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        write_fixture_tone(&scratch).unwrap();
        let good = plan(
            PythonTool::Improve,
            &repo,
            &scratch,
            &json!({"input": "fixture-tone.wav", "output": "fixture-24k.wav"}),
        )
        .unwrap();
        let improve_py = Path::new("scripts").join("improve.py");
        assert!(good.args.iter().any(|arg| Path::new(arg).ends_with(&improve_py)));
        assert!(plan(
            PythonTool::Improve,
            &repo,
            &scratch,
            &json!({"input": "../secret.wav", "output": "out.wav"}),
        )
        .is_err());
        let script_err = plan(
            PythonTool::Synth,
            &repo,
            &scratch,
            &json!({"output": "nope.wav", "script": "README.md"}),
        )
        .unwrap_err();
        assert!(script_err.contains("examples/"));
        let _ = std::fs::remove_dir_all(&scratch.dir);
    }

    #[test]
    fn synth_does_not_invent_audio_without_models() {
        let scratch = Scratch::create().unwrap();
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let _env = NoKokoroEnv::new();
        let err = plan(
            PythonTool::Synth,
            &repo,
            &scratch,
            &json!({"output": "should-not-exist.wav"}),
        )
        .unwrap_err();
        assert!(err.contains("No speech was invented"));
        let _ = std::fs::remove_dir_all(&scratch.dir);
    }
}
