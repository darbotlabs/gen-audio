//! Isolated, offline model workers. Client arguments are data, never CLI options
//! or runtime paths. Readiness is not inference or transcript verification.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::bridge::wav_metadata;
use crate::paths::{self, read_bounded, Scratch};
use crate::{redact::redact_secrets, subprocess};
use crate::execution::Execution;

const MAX_TEXT_BYTES: usize = 32768;
const MAX_TEXT_CHARS: usize = 8192;
const TIMEOUT_SECONDS: u64 = 590;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelEngine {
    Vibevoice,
    Personaplex,
}

impl ModelEngine {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Vibevoice => "vibevoice",
            Self::Personaplex => "personaplex",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Vibevoice => "GEN_AUDIO_VIBEVOICE",
            Self::Personaplex => "GEN_AUDIO_PERSONAPLEX",
        }
    }

    fn task(self) -> &'static str {
        match self {
            Self::Vibevoice => "script_tts",
            Self::Personaplex => "speech_to_speech",
        }
    }

    fn semantics(self) -> &'static str {
        match self {
            Self::Vibevoice => "generated_script_speech",
            Self::Personaplex => "generated_conversation_speech",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    engine: ModelEngine,
    script_text: Option<String>,
    input_audio: Option<String>,
    persona: Option<String>,
    #[serde(default)]
    voice_refs: Vec<String>,
    #[serde(default)]
    agent_names: Vec<String>,
    output: Option<String>,
    #[serde(default = "default_tokens")]
    max_new_tokens: u32,
    #[serde(default = "default_seconds")]
    max_seconds: f64,
    #[serde(default)]
    seed: u32,
    execution: Option<Value>,
}

fn default_tokens() -> u32 { 256 }
fn default_seconds() -> f64 { 30.0 }

impl Request {
    fn parse(args: &Value, preflight: bool) -> Result<Self, String> {
        // Option<T> accepts null; this public protocol deliberately does not.
        let obj = args.as_object().ok_or("model arguments must be an object")?;
        if obj.values().any(Value::is_null) {
            return Err("model arguments must not contain null".into());
        }
        let request: Self = serde_json::from_value(args.clone())
            .map_err(|_| "invalid model arguments; use the typed model tool schema")?;
        if let Some(value) = &request.execution { Execution::parse(value)?; }
        crate::audio_metadata::for_agents(&request.agent_names)?;
        for text in [&request.script_text, &request.persona].into_iter().flatten() {
            if text.len() > MAX_TEXT_BYTES || text.chars().count() > MAX_TEXT_CHARS
                || text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
                return Err("text/persona must be at most 8192 characters / 32768 bytes without control codes".into());
            }
        }
        if !(1..=1024).contains(&request.max_new_tokens)
            || !request.max_seconds.is_finite() || !(1.0..=60.0).contains(&request.max_seconds) {
            return Err("maxNewTokens must be 1..1024 and maxSeconds must be 1..60".into());
        }
        if request.voice_refs.len() > if request.engine == ModelEngine::Vibevoice { 4 } else { 1 } {
            return Err("VibeVoice accepts at most four voiceRefs; PersonaPlex accepts one".into());
        }
        match request.engine {
            ModelEngine::Vibevoice if request.input_audio.is_some() || request.persona.is_some() => {
                return Err("VibeVoice takes scriptText and voiceRefs, not inputAudio or persona".into());
            }
            ModelEngine::Personaplex if request.script_text.is_some() => {
                return Err("PersonaPlex is speech-to-speech: provide inputAudio, persona and voiceRefs, not scriptText".into());
            }
            _ => {}
        }
        if preflight && request.output.is_some() {
            return Err("model_preflight does not accept output".into());
        }
        if request.output.as_ref().is_some_and(|s| s.len() > 173) {
            return Err("model output name must leave space for the .synth.json suffix (at most 173 bytes)".into());
        }
        if !preflight && !request.missing_fields().is_empty() {
            return Err(format!("missing model request fields: {}", request.missing_fields().join(", ")));
        }
        Ok(request)
    }

    fn missing_fields(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.engine == ModelEngine::Vibevoice
            && self.script_text.as_ref().is_none_or(|s| s.trim().is_empty()) {
            missing.push("scriptText");
        }
        if self.engine == ModelEngine::Personaplex {
            if self.input_audio.is_none() { missing.push("inputAudio"); }
            if self.persona.as_ref().is_none_or(|s| s.trim().is_empty()) { missing.push("persona"); }
        }
        if self.voice_refs.is_empty() { missing.push("voiceRefs"); }
        missing
    }
}

#[derive(Debug)]
struct Runtime {
    python: PathBuf,
    model: PathBuf,
    tokenizer: PathBuf,
    source: PathBuf,
}

impl Runtime {
    fn for_execution(repo: Option<&Path>, engine: ModelEngine, execution: &Execution) -> Result<Self, Vec<Value>> {
        let python_key = format!("{}_PYTHON", engine.prefix());
        let variant_key = format!("{}_{}", python_key, execution.backend.to_ascii_uppercase());
        Self::configured(repo, engine, |key| {
            if key == python_key {
                crate::runtime_config::setting(&variant_key).or_else(|| crate::runtime_config::setting(key))
            } else { crate::runtime_config::setting(key) }
        })
    }
    fn configured(
        repo: Option<&Path>,
        engine: ModelEngine,
        get: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, Vec<Value>> {
        let mut errors = Vec::new();
        let mut path = |suffix: &str, directory: bool| {
            let field = format!("{}_{suffix}", engine.prefix());
            let Some(raw) = get(&field).filter(|s| !s.is_empty()) else {
                errors.push(json!({"code":"missing_configuration","field":field,"message":format!("{field} is not configured")}));
                return None;
            };
            let path = PathBuf::from(raw);
            let valid = path.is_absolute()
                && !path.components().any(|c| matches!(c, Component::ParentDir))
                && paths::reject_quarantine(&path).is_ok()
                && if directory { path.is_dir() } else { path.is_file() };
            let python_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
            if !valid || (suffix == "PYTHON" && !matches!(python_name.as_str(),
                "python" | "python3" | "python3.12" | "python.exe" | "python3.exe" | "python3.12.exe")) {
                errors.push(json!({"code":"invalid_configuration","field":field,
                    "message":format!("{field} must name an existing absolute {}{}", if directory { "directory" } else { "file" },
                        if suffix == "PYTHON" { " for the isolated Python runtime (no launcher or shell)" } else { "" })}));
                return None;
            }
            // Resolving a venv's python symlink would accidentally select the
            // base interpreter. Preserve its configured absolute invocation.
            if suffix == "PYTHON" { Some(path) } else { path.canonicalize().ok() }
        };
        let python = path("PYTHON", false);
        let model = path("MODEL_DIR", true);
        let tokenizer = path("TOKENIZER", engine == ModelEngine::Vibevoice);
        let source = repo.and_then(|root| {
            let root = root.canonicalize().ok()?;
            for rel in [
                Path::new("src").join("gen_audio").join("__init__.py"),
                Path::new("src").join("gen_audio").join("model_workers").join("__main__.py"),
            ] {
                if paths::read_repo_relative(&root, rel.to_str()?).ok()? != root.join(rel) { return None; }
            }
            Some(root.join("src"))
        });
        if source.is_none() {
            errors.push(json!({"code":"missing_worker","message":"Repository model-worker entry point is missing or redirected"}));
        }
        match (python, model, tokenizer, source) {
            (Some(python), Some(model), Some(tokenizer), Some(source)) if errors.is_empty() => {
                Ok(Self { python, model, tokenizer, source })
            }
            _ => {
                if errors.is_empty() {
                    errors.push(json!({"code":"invalid_configuration","message":"Cannot resolve configured runtime paths"}));
                }
                Err(errors)
            }
        }
    }
}

struct Plan<'a> {
    scratch: &'a Scratch,
    request: Request,
    runtime: Runtime,
    voices: Vec<PathBuf>,
    input_audio: Option<PathBuf>,
    private_text: Option<PathBuf>,
    artifacts: Option<(PathBuf, PathBuf)>,
    adopted: bool,
    preflight: bool,
    execution: Execution,
}

impl Drop for Plan<'_> {
    fn drop(&mut self) {
        if let Some(path) = &self.private_text { let _ = std::fs::remove_file(path); }
        if !self.adopted {
            if let Some((wav, manifest)) = &self.artifacts {
                for path in [wav, manifest] {
                    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                        self.scratch.discard_reserved(name);
                    }
                }
            }
        }
    }
}

fn issued_wav(scratch: &Scratch, name: &str) -> Result<PathBuf, String> {
    paths::write_name(&scratch.dir, name)?;
    if !name.ends_with(".wav") { return Err("model audio inputs must be issued .wav names".into()); }
    scratch.open_input(name)
}

impl<'a> Plan<'a> {
    fn new(scratch: &'a Scratch, request: Request, runtime: Runtime, preflight: bool) -> Result<Self, String> {
        let execution = request.execution.as_ref().map(Execution::parse).unwrap_or_else(|| Execution::configured(request.engine.prefix()))?;
        let voices = request.voice_refs.iter().map(|name| issued_wav(scratch, name)).collect::<Result<_, _>>()?;
        let input_audio = request.input_audio.as_deref().map(|name| issued_wav(scratch, name)).transpose()?;
        let mut plan = Self {
            scratch, request, runtime, voices, input_audio,
            private_text: None, artifacts: None, adopted: false, preflight, execution,
        };
        if !preflight {
            let name = plan.request.output.clone().unwrap_or_else(|| scratch.unique_name(".wav"));
            plan.artifacts = Some(scratch.reserve_speech_with_manifest(&name, ".synth.json")?);
        }
        if let Some(text) = &plan.request.script_text {
            plan.private_text = Some(scratch.write_private_input(text.as_bytes(), ".txt")?);
        }
        Ok(plan)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.runtime.python);
        command.env_clear().current_dir(&self.scratch.dir);
        // No inherited Python paths, credentials, model options, or DLL search
        // PATH. Preserve Windows profile directories for accelerator caches:
        // MIOpen expands USERPROFILE when opening its kernel and tuning DBs.
        #[cfg(windows)]
        for key in ["USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
            if let Some(value) = std::env::var_os(key) {
                let directory = Path::new(&value);
                if directory.is_absolute() && directory.is_dir() {
                    command.env(key, value);
                }
            }
        }
        let mut path = vec![self.runtime.python.parent().unwrap().to_path_buf()];
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", &root).env("WINDIR", &root);
            path.push(PathBuf::from(root).join("System32"));
        }
        if let Ok(path) = std::env::join_paths(path) { command.env("PATH", path); }
        command.env("PYTHONPATH", &self.runtime.source)
            .env("PYTHONNOUSERSITE", "1").env("PYTHONSAFEPATH", "1")
            .env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1")
            .env("TEMP", &self.scratch.dir).env("TMP", &self.scratch.dir).env("TMPDIR", &self.scratch.dir)
            .env("HF_HUB_OFFLINE", "1").env("TRANSFORMERS_OFFLINE", "1").env("HF_DATASETS_OFFLINE", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1").env("DO_NOT_TRACK", "1")
            .env("TOKENIZERS_PARALLELISM", "false").env("GENAID_LOCAL_FILES_ONLY", "1")
            .env("OMP_NUM_THREADS", "8").env("MKL_NUM_THREADS", "8")
            .args(["-s", "-P", "-B", "-u", "-m", "gen_audio.model_workers", "--engine", self.request.engine.name()])
            .arg("--model-dir").arg(&self.runtime.model)
            .arg("--tokenizer").arg(&self.runtime.tokenizer)
            .args(["--device", "cpu", "--timeout-seconds", &TIMEOUT_SECONDS.to_string()])
            .arg("--execution").arg(serde_json::to_string(&self.execution).expect("execution serialization"))
            .arg("--max-new-tokens").arg(self.request.max_new_tokens.to_string())
            .arg("--max-seconds").arg(self.request.max_seconds.to_string())
            .arg("--seed").arg(self.request.seed.to_string());
        if self.preflight { command.arg("--preflight"); }
        if let Some(path) = &self.private_text { command.arg("--input-text").arg(path); }
        if let Some(path) = &self.input_audio { command.arg("--input-audio").arg(path); }
        if let Some(persona) = &self.request.persona {
            // Equals prevents a persona starting with "--" becoming an option.
            command.arg(format!("--persona={persona}"));
        }
        for path in &self.voices { command.arg("--voice-ref").arg(path); }
        for name in &self.request.agent_names { command.arg(format!("--agent-name={name}")); }
        if let Some((wav, _)) = &self.artifacts { command.arg("--output").arg(wav); }
        command
    }

    fn sanitize(&self, value: &mut Value) {
        match value {
            Value::String(text) => {
                for (path, label) in [
                    (&self.runtime.tokenizer, "[configured tokenizer]"),
                    (&self.runtime.model, "[configured model]"),
                    (&self.runtime.python, "[configured python]"),
                    (&self.scratch.dir, "[scratch]"),
                    (&self.runtime.source, "[repository src]"),
                ] {
                    let raw = path.to_string_lossy();
                    for raw in [raw.as_ref(), raw.trim_start_matches(r"\\?\")] {
                        *text = text.replace(raw, label).replace(&raw.replace('\\', "/"), label);
                    }
                }
                *text = redact_secrets(text);
            }
            Value::Array(items) => items.iter_mut().for_each(|item| self.sanitize(item)),
            Value::Object(obj) => {
                for (key, item) in obj {
                    if key == "message" {
                        if let Value::String(text) = item {
                            for input in [&self.request.script_text, &self.request.persona].into_iter().flatten() {
                                for part in std::iter::once(input.as_str()).chain(input.lines()).map(str::trim).filter(|s| !s.is_empty()) {
                                    *text = text.replace(part, "[private input]");
                                }
                            }
                        }
                    }
                    self.sanitize(item);
                }
            }
            _ => {}
        }
    }
}

fn unavailable(engine: ModelEngine, preflight: bool, errors: Vec<Value>, missing: Vec<&str>) -> Value {
    json!({
        "schema_version":"1.0","ok":false,"ready":false,"preflight":preflight,
        "engine":engine,"capability":engine.task(),"synthesizedSpeech":false,
        "status":"unavailable","phase":"unavailable","inference_validated":false,
        "transcript_verified":false,"missing_fields":missing,"errors":errors
    })
}

/// Check request-specific readiness without importing the heavy runtimes.
/// Missing request fields remain explicit blockers; no sample text is invented.
pub fn preflight(repo: Option<&Path>, scratch: &Scratch, args: &Value) -> Result<Value, String> {
    call(repo, scratch, args, true)
}

/// Generate only with configured isolated runtimes and issued scratch inputs.
/// `ok` is false for unavailability, verification failure, and capped output.
pub fn generate(repo: Option<&Path>, scratch: &Scratch, args: &Value) -> Result<Value, String> {
    call(repo, scratch, args, false)
}

pub(crate) fn resident_command(repo: &Path, scratch: &Scratch, engine: &str, execution: Option<&Value>) -> Result<Command, String> {
    let mut args = json!({"engine":engine});
    if let Some(value) = execution { args["execution"] = value.clone(); }
    let request = Request::parse(&args, true)?;
    let execution = request.execution.as_ref().map(Execution::parse).unwrap_or_else(|| Execution::configured(request.engine.prefix()))?;
    let runtime = Runtime::for_execution(Some(repo), request.engine, &execution)
        .map_err(|errors| errors.iter().filter_map(|e| e["message"].as_str()).collect::<Vec<_>>().join("; "))?;
    paths::read_repo_relative(repo, "src/gen_audio/runtime_owner.py")?;
    let model = runtime.model.clone();
    let tokenizer = runtime.tokenizer.clone();
    let plan = Plan::new(scratch, request, runtime, true)?;
    let configured = plan.command();
    let mut owner = Command::new(configured.get_program());
    owner.env_clear().current_dir(&scratch.dir);
    for (key, value) in configured.get_envs() { if let Some(value) = value { owner.env(key, value); } }
    owner.args(["-s", "-P", "-B", "-u", "-m", "gen_audio.runtime_owner", "--engine", engine])
        .arg("--model-dir").arg(model).arg("--tokenizer").arg(tokenizer)
        .arg("--execution").arg(serde_json::to_string(&execution).expect("execution serialization"));
    Ok(owner)
}

fn call(repo: Option<&Path>, scratch: &Scratch, args: &Value, preflight: bool) -> Result<Value, String> {
    let request = Request::parse(args, preflight)?;
    // Validate capability names even when the runtime is unconfigured.
    for name in request.voice_refs.iter().chain(request.input_audio.iter()) { issued_wav(scratch, name)?; }
    if let Some(name) = &request.output {
        paths::write_name(&scratch.dir, name)?;
        if !name.ends_with(".wav") { return Err("model output must end in .wav".into()); }
    }
    let execution = request.execution.as_ref().map(Execution::parse).unwrap_or_else(|| Execution::configured(request.engine.prefix()))?;
    let runtime = match Runtime::for_execution(repo, request.engine, &execution) {
        Ok(runtime) => runtime,
        Err(errors) => return Ok(unavailable(request.engine, preflight, errors, request.missing_fields())),
    };
    let mut plan = Plan::new(scratch, request, runtime, preflight)?;
    Ok(execute(&mut plan))
}

fn execute(plan: &mut Plan<'_>) -> Value {
    let mut command = plan.command();
    let argv: Vec<String> = command.get_args().skip(6).map(|arg| arg.to_string_lossy().to_string()).collect();
    let result = crate::runtime_owner::try_run(plan.request.engine.name(), "run", &command, &argv, &std::sync::atomic::AtomicBool::new(false)).and_then(|warm| match warm {
        Some(output) => Ok(output),
        None => subprocess::run(&mut command, b"", subprocess::Limits {
        deadline: Duration::from_secs(if plan.preflight { 30 } else { 600 }),
        ..subprocess::Limits::default()
        }),
    }).and_then(|output| process_output(plan, output));
    match result {
        Ok(value) => value,
        Err(message) => {
            let mut value = unavailable(plan.request.engine, plan.preflight,
                vec![json!({"code":"worker_failed","message":message})], plan.request.missing_fields());
            value["status"] = json!("failed");
            value["phase"] = json!("failed");
            plan.sanitize(&mut value);
            value
        }
    }
}

fn process_output(plan: &mut Plan<'_>, output: subprocess::Output) -> Result<Value, String> {
    // Accept one complete JSON document, not a success-looking trailing log line.
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|_| "worker returned invalid JSON")?;
    if value["schema_version"] != "1.0" || value["engine"] != plan.request.engine.name()
        || !value["ok"].is_boolean() || !value["synthesizedSpeech"].is_boolean() {
        return Err("worker receipt has an invalid schema or engine".into());
    }
    if value["ok"] == false {
        if output.status.code() != Some(2) || value["synthesizedSpeech"] != false
            || value.get("ready").is_some_and(|v| v != false)
            || !value["errors"].as_array().is_some_and(|e| !e.is_empty()
                && e.iter().all(|e| e["code"].is_string() && e["message"].is_string())) {
            return Err("worker blocker receipt disagrees with its exit status".into());
        }
        return Ok(readiness_result(plan, &value));
    }
    if !output.status.success() { return Err("worker success receipt disagrees with its exit status".into()); }
    if plan.preflight {
        if value["preflight"] != true || value["ready"] != true || value["synthesizedSpeech"] != false
            || value["inference_validated"] != false
            || value["errors"].as_array().is_none_or(|e| !e.is_empty())
            || !plan.request.missing_fields().is_empty() {
            return Err("worker claimed readiness without a complete valid preflight".into());
        }
        return Ok(readiness_result(plan, &value));
    }
    let verified = verify_artifacts(plan, &output.stdout)?;
    for field in ["wav", "manifest"] {
        plan.scratch.adopt_file(verified[field].as_str().ok_or("missing verified artifact name")?)?;
    }
    plan.adopted = true;
    Ok(verified)
}

fn readiness_result(plan: &Plan<'_>, report: &Value) -> Value {
    let mut result = unavailable(plan.request.engine, plan.preflight, vec![], plan.request.missing_fields());
    for key in ["runtime", "dependencies", "memory", "limits", "device", "dtype", "execution", "errors", "tokenizer_note", "voice_note"] {
        if let Some(value) = report.get(key) { result[key] = value.clone(); }
    }
    // Weight sizes and inspected WAV metadata are useful without reflecting
    // arbitrary model config documents or private text back to the client.
    result["assets"] = json!({});
    for key in ["weights", "voices", "input_audio", "tokenizer"] {
        if let Some(value) = report["assets"].get(key) { result["assets"][key] = value.clone(); }
    }
    let ready = report["ready"] == true && plan.request.missing_fields().is_empty();
    result["ok"] = json!(ready);
    result["ready"] = json!(ready);
    if ready {
        result["status"] = json!("ready");
        result["phase"] = json!("ready");
    }
    plan.sanitize(&mut result);
    result
}

#[derive(Debug, Deserialize, PartialEq)]
struct Receipt {
    schema_version: String,
    ok: bool,
    #[serde(rename = "synthesizedSpeech")]
    synthesized_speech: bool,
    engine: ModelEngine,
    output: PathBuf,
    manifest: PathBuf,
    sample_rate: u32,
    samples: u64,
    duration_s: f64,
    sha256: String,
    #[serde(default)]
    metadata: Value,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(flatten)]
    receipt: Receipt,
    task: String,
    output_semantics: String,
    transcript_verified: bool,
    provenance: Value,
}

fn verify_artifacts(plan: &Plan<'_>, stdout: &[u8]) -> Result<Value, String> {
    let receipt: Receipt = serde_json::from_slice(stdout).map_err(|_| "invalid model speech receipt")?;
    let (wav_path, manifest_path) = plan.artifacts.as_ref().ok_or("generation has no reserved artifacts")?;
    if receipt.schema_version != "1.0" || !receipt.ok || !receipt.synthesized_speech
        || receipt.engine != plan.request.engine || !receipt.output.is_absolute()
        || receipt.output != *wav_path || receipt.manifest != *manifest_path {
        return Err("model receipt does not match the reserved speech artifacts".into());
    }
    let bytes = read_bounded(wav_path, 24_000 * 60 * 2 + 65536)?;
    if !plan.request.agent_names.is_empty() || receipt.metadata.is_object() {
        crate::audio_metadata::verify(&bytes, &receipt.metadata, &plan.request.agent_names)?;
    }
    let (sample_rate, samples) = wav_metadata(&bytes)?;
    let duration = samples as f64 / sample_rate as f64;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if receipt.sample_rate != sample_rate || receipt.samples != samples
        || !receipt.duration_s.is_finite() || (receipt.duration_s - duration).abs() > 1e-6
        || duration > plan.request.max_seconds || receipt.sha256 != digest {
        return Err("model WAV hash, sample count, rate or duration disagrees with the receipt".into());
    }
    let manifest: Manifest = serde_json::from_slice(&read_bounded(manifest_path, 512 * 1024)?)
        .map_err(|_| "invalid model .synth.json manifest")?;
    if manifest.receipt != receipt || manifest.task != plan.request.engine.task()
        || manifest.output_semantics != plan.request.engine.semantics() || manifest.transcript_verified {
        return Err("model manifest disagrees with the receipt or falsely claims transcript verification".into());
    }
    let provenance = &manifest.provenance;
    let mut limit_reached = provenance["limit_reached"].as_bool().ok_or("model manifest must state limit_reached")?;
    let same_path = |value: &Value, expected: &Path| value.as_str().is_some_and(|s| Path::new(s) == expected);
    if !plan.execution.verify_result(provenance) || provenance["offline"] != true
        || provenance["seed"] != plan.request.seed
        || !same_path(&provenance["model_dir"], &plan.runtime.model)
        || !same_path(&provenance["tokenizer"], &plan.runtime.tokenizer)
        || provenance["limits"]["max_new_tokens"] != plan.request.max_new_tokens
        || provenance["limits"]["max_seconds"].as_f64() != Some(plan.request.max_seconds)
        || provenance["limits"]["timeout_seconds"].as_f64() != Some(TIMEOUT_SECONDS as f64)
        || !provenance["runtime_revision"].as_str().is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        || !provenance["voice_references"].as_array().is_some_and(|refs| refs.len() == plan.voices.len()
            && refs.iter().zip(&plan.voices).all(|(actual, expected)| same_path(actual, expected))) {
        return Err("model manifest provenance does not match the configured runtime and request".into());
    }
    match plan.request.engine {
        ModelEngine::Vibevoice => {
            let script = normalized_script(plan.request.script_text.as_deref().unwrap_or(""), plan.voices.len())?;
            if provenance["input_text_sha256"] != format!("{:x}", Sha256::digest(script.as_bytes())) {
                return Err("model manifest script provenance differs from requested text".into());
            }
            let token_cap = (plan.request.max_new_tokens as u64)
                .min((plan.request.max_seconds * 24_000.0 / 3200.0).floor() as u64);
            let generated = provenance["generated_tokens"].as_u64().ok_or("missing generated-token count")?;
            if provenance["effective_max_new_tokens"].as_u64() != Some(token_cap)
                || !provenance["input_tokens"].as_u64().is_some_and(|n| (1..=4096).contains(&n)) {
                return Err("model manifest token limits do not match the request".into());
            }
            // A false flag must not override evidence that the cap was reached.
            limit_reached |= generated >= token_cap;
        }
        ModelEngine::Personaplex => {
            let mut persona = plan.request.persona.as_deref().unwrap_or("").trim().to_string();
            if !(persona.starts_with("<system>") && persona.ends_with("<system>")) {
                persona = format!("<system> {persona} <system>");
            }
            if !same_path(&provenance["input_audio"], plan.input_audio.as_deref().ok_or("missing conversation input")?)
                || provenance["persona_sha256"] != format!("{:x}", Sha256::digest(persona.as_bytes()))
                || provenance["generated_text_semantics"] != "model_response_not_input_transcription" {
                return Err("model manifest conversation provenance differs from requested inputs".into());
            }
        }
    }
    let wav = wav_path.file_name().and_then(|s| s.to_str()).ok_or("invalid artifact name")?;
    let mut public_provenance = json!({});
    for key in ["runtime_revision", "runtime_repository", "runtime_profile", "runtime_patches_sha256", "runtime_wheel_sha256", "dependencies", "device", "dtype", "execution", "attention_policy", "timing", "offline", "seed",
        "limits", "input_text_sha256", "persona_sha256", "input_tokens", "generated_tokens", "effective_max_new_tokens",
        "stream_steps", "voice_format", "generated_text_semantics", "limit_reached",
        "ddpm_inference_steps", "generation_settings_source", "codec_precision", "codec_attention"] {
        if let Some(value) = provenance.get(key) { public_provenance[key] = value.clone(); }
    }
    public_provenance["voiceRefs"] = json!(plan.request.voice_refs);
    public_provenance["limit_reached"] = json!(limit_reached);
    if let Some(input) = &plan.request.input_audio { public_provenance["inputAudio"] = json!(input); }
    let mut result = json!({
        "schema_version":"1.0","ok":!limit_reached,"synthesizedSpeech":true,
        "engine":plan.request.engine,"task":manifest.task,"output_semantics":manifest.output_semantics,
        "status":if limit_reached {"incomplete"} else {"complete"},
        "phase":if limit_reached {"incomplete"} else {"complete"},
        "complete":!limit_reached,"limit_reached":limit_reached,"transcript_verified":false,
        "artifact_verified":true,"wav":wav,"output":wav,"manifest":manifest_path.file_name().unwrap().to_string_lossy(),
        "sample_rate":sample_rate,"samples":samples,"duration_s":duration,"sha256":digest,"bytes":bytes.len(),
        "provenance":public_provenance,"metadata":manifest.receipt.metadata
    });
    if limit_reached {
        result["errors"] = json!([{"code":"generation_limit_reached","message":"Verified audio is incomplete: the worker reached its generation limit."}]);
    }
    plan.sanitize(&mut result);
    Ok(result)
}

// Mirrors the worker's small, explicit Speaker 1..4 normalization only for
// request-hash comparison. This verifies provenance, not the audible transcript.
fn normalized_script(text: &str, voices: usize) -> Result<String, String> {
    let lines: Vec<_> = text.split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .map(str::trim).filter(|s| !s.is_empty()).collect();
    let mut turns = Vec::new();
    for line in &lines {
        let turn = line.split_once(':').and_then(|(head, tail)| {
            let words: Vec<_> = head.split_whitespace().collect();
            if words.len() != 2 || !words[0].eq_ignore_ascii_case("Speaker")
                || !matches!(words[1], "1" | "2" | "3" | "4")
                || head.ends_with(char::is_whitespace) || tail.trim_start().is_empty() { return None; }
            Some((words[1].parse::<usize>().ok()?, tail.trim_start()))
        });
        turns.push(turn);
    }
    if turns.iter().all(Option::is_none) && voices == 1 && !lines.is_empty() {
        if lines.iter().any(|line| line.split_once(':').is_some_and(|(head, _)| {
            let words: Vec<_> = head.split_whitespace().collect();
            words.len() == 2 && words[0].eq_ignore_ascii_case("Speaker")
                && words[1].chars().all(|c| c.is_ascii_digit())
        })) {
            return Err("invalid script speaker IDs".into());
        }
        return Ok(format!("Speaker 1: {}", lines.join(" ")));
    }
    let turns = turns.into_iter().collect::<Option<Vec<_>>>().ok_or("invalid script provenance")?;
    if turns.is_empty() || !(1..=voices).all(|id| turns.iter().any(|(speaker, _)| *speaker == id))
        || turns.iter().any(|(id, _)| *id > voices) {
        return Err("script speaker mapping differs from voiceRefs".into());
    }
    Ok(turns.into_iter().map(|(id, text)| format!("Speaker {id}: {text}")).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::OnceLock;

    static PROCESS_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Work(Scratch);
    impl Work {
        fn new() -> Self { Self(Scratch::create().unwrap()) }
    }
    impl Drop for Work {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0.dir); }
    }

    fn python() -> Option<PathBuf> {
        static PYTHON: OnceLock<Option<PathBuf>> = OnceLock::new();
        PYTHON.get_or_init(|| {
            for name in [if cfg!(windows) { "python" } else { "python3" }, "python"] {
                let output = subprocess::run(
                    Command::new(name).args(["-I", "-c", "import sys; print(sys.executable)"]),
                    b"", subprocess::Limits { deadline: Duration::from_secs(10), ..subprocess::Limits::default() },
                );
                if let Ok(output) = output {
                    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
                    if output.status.success() && path.is_absolute() && path.is_file() { return Some(path); }
                }
            }
            None
        }).clone()
    }

    fn fixture_wav() -> Vec<u8> {
        let mut wav = b"RIFF".to_vec();
        wav.extend(48036u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(24000u32.to_le_bytes());
        wav.extend(48000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(48000u32.to_le_bytes());
        wav.extend([1, 0].repeat(24000));
        wav
    }

    fn issue_reference(scratch: &Scratch) {
        fs::write(scratch.prepare_output("reference.wav").unwrap(), fixture_wav()).unwrap();
    }

    fn args(engine: ModelEngine) -> Value {
        match engine {
            ModelEngine::Vibevoice => json!({"engine":"vibevoice","scriptText":"Speaker 1: Private requested words.",
                "voiceRefs":["reference.wav"],"output":"result.wav"}),
            ModelEngine::Personaplex => json!({"engine":"personaplex","inputAudio":"reference.wav",
                "persona":"--Helpful private persona","voiceRefs":["reference.wav"],"output":"result.wav"}),
        }
    }

    const MOCK_WORKER: &str = r#"
import argparse, hashlib, json, os, pathlib, sys
p = argparse.ArgumentParser()
for key in ('engine','model-dir','tokenizer','device','execution','timeout-seconds','max-new-tokens',
            'max-seconds','seed','input-text','input-audio','persona','output'):
    p.add_argument('--' + key)
p.add_argument('--voice-ref', action='append', default=[])
p.add_argument('--preflight', action='store_true')
a = p.parse_args()
assert a.device == 'cpu'
assert os.environ['HF_HUB_OFFLINE'] == '1'
assert os.environ['PYTHONNOUSERSITE'] == '1'
assert not any(k in os.environ for k in ('PYTHONHOME','GEN_AUDIO_ARTIFACT_DIR','OPENAI_API_KEY','HF_TOKEN'))
mode = pathlib.Path(a.model_dir, 'mode.txt').read_text()
if mode == 'flood':
    print('x' * 300000)
    sys.exit(0)
base = dict(schema_version='1.0', engine=a.engine)
if a.preflight or mode in ('blocked', 'exit-mismatch'):
    ready = mode == 'ready'
    result = dict(base, ok=ready, ready=ready, preflight=True, synthesizedSpeech=False,
        inference_validated=False, capability='script_tts' if a.engine == 'vibevoice' else 'speech_to_speech',
        device='cpu', dtype='float32', memory=dict(sufficient=ready, available_bytes=8000000000, required_bytes=16000000000),
        dependencies={'torch':None}, assets={'voices':[{'path':a.voice_ref[0]}] if a.voice_ref else []},
        errors=[] if ready else [dict(code='insufficient_memory',message='Insufficient RAM; weights were not loaded'),
            dict(code='missing_dependency',message='Missing torch'),
            dict(code='missing_asset',message='Missing file at ' + a.model_dir)])
    print(json.dumps(result))
    sys.exit(0 if ready or mode == 'exit-mismatch' else 2)
output = pathlib.Path(a.output).resolve()
manifest = output.with_suffix('.synth.json')
raw = pathlib.Path(a.voice_ref[0]).read_bytes()
output.write_bytes(raw)
receipt = dict(base, ok=True, synthesizedSpeech=True, output=str(output), manifest=str(manifest),
    sample_rate=24000, samples=24000, duration_s=1.0, sha256=hashlib.sha256(raw).hexdigest())
limits = dict(max_new_tokens=int(a.max_new_tokens), max_seconds=float(a.max_seconds),
              timeout_seconds=float(a.timeout_seconds), max_input_tokens=4096)
provenance = dict(runtime_revision='a'*40, runtime_repository='mock-worker-test-only', dependencies={},
    model_dir=a.model_dir, tokenizer=a.tokenizer, voice_references=a.voice_ref, device='cpu', dtype='float32',
    offline=True, seed=int(a.seed), limits=limits, limit_reached=mode == 'limit')
if a.engine == 'vibevoice':
    text = pathlib.Path(a.input_text).read_text(encoding='utf-8')
    provenance['input_text_sha256'] = hashlib.sha256(text.encode()).hexdigest()
    token_cap = min(int(a.max_new_tokens), int(float(a.max_seconds) * 24000 / 3200))
    provenance.update(input_tokens=12, generated_tokens=token_cap if mode in ('limit','counter-limit') else 10,
                      effective_max_new_tokens=token_cap)
else:
    persona = a.persona.strip()
    if not (persona.startswith('<system>') and persona.endswith('<system>')):
        persona = '<system> ' + persona + ' <system>'
    provenance.update(input_audio=a.input_audio, persona_sha256=hashlib.sha256(persona.encode()).hexdigest(),
        generated_text_semantics='model_response_not_input_transcription', generated_text='DO NOT EXPOSE')
task = 'script_tts' if a.engine == 'vibevoice' else 'speech_to_speech'
semantics = 'generated_script_speech' if a.engine == 'vibevoice' else 'generated_conversation_speech'
record = dict(receipt, task=task, output_semantics=semantics, transcript_verified=False, provenance=provenance)
if mode == 'hash': receipt['sha256'] = '0'*64
if mode == 'samples': receipt['samples'] = 24001
if mode == 'rate': receipt['sample_rate'] = 16000
if mode == 'duration': receipt['duration_s'] = 2.0
if mode == 'path': receipt['output'] = str(output.with_name('unexpected.wav'))
if mode == 'manifest-mismatch': record['samples'] = 24001
if mode == 'task': record['task'] = 'script_tts' if task != 'script_tts' else 'speech_to_speech'
if mode == 'semantics': record['output_semantics'] = 'verbatim_transcript'
if mode == 'transcript': record['transcript_verified'] = True
if mode == 'provenance': provenance['tokenizer'] = 'arbitrary'
if mode == 'text-hash': provenance['input_text_sha256'] = '0'*64
if mode == 'persona-hash': provenance['persona_sha256'] = '0'*64
if mode == 'no-limit': del provenance['limit_reached']
if mode != 'missing-manifest': manifest.write_text(json.dumps(record), encoding='utf-8')
if mode == 'missing-wav': output.unlink()
if mode == 'corrupt-wav': output.write_bytes(b'not audio')
if mode == 'silence': output.write_bytes(raw[:44] + b'\0' * 48000)
if mode == 'trailing-log': print('not JSON')
print(json.dumps(receipt))
"#;

    fn mock_runtime(scratch: &Scratch, mode: &str) -> Option<Runtime> {
        let python = python()?;
        let source = scratch.dir.join("mock-src");
        let package = source.join("gen_audio");
        let workers = package.join("model_workers");
        fs::create_dir_all(&workers).unwrap();
        fs::write(package.join("__init__.py"), "").unwrap();
        fs::write(workers.join("__init__.py"), "").unwrap();
        fs::write(workers.join("__main__.py"), MOCK_WORKER).unwrap();
        let model = scratch.dir.join("mock-model");
        fs::create_dir_all(&model).unwrap();
        fs::write(model.join("mode.txt"), mode).unwrap();
        Some(Runtime { python, source, model: model.clone(), tokenizer: model })
    }

    #[test]
    fn typed_requests_and_config_never_accept_client_runtime_options() {
        for value in [
            json!({"engine":"vibevoice","python":"evil"}),
            json!({"engine":"personaplex","tokenizer":"evil"}),
            json!({"engine":"vibevoice","script":"arbitrary.txt"}),
            json!({"engine":"vibevoice","voiceRefs":null}),
            json!({"engine":"vibevoice","maxSeconds":true}),
            json!({"engine":"vibevoice","seed":-1}),
            json!({"engine":"vibevoice","scriptText":"x".repeat(8193)}),
            json!({"engine":"personaplex","voiceRefs":["a.wav","b.wav"]}),
        ] { assert!(Request::parse(&value, true).is_err(), "{value}"); }
        let error = Runtime::configured(None, ModelEngine::Vibevoice, |_| None).unwrap_err();
        assert_eq!(error.len(), 4);
        assert!(error.iter().any(|e| e["field"] == "GEN_AUDIO_VIBEVOICE_TOKENIZER"));
        let error = Runtime::configured(None, ModelEngine::Personaplex, |_| Some("python".into())).unwrap_err();
        assert_eq!(error.iter().filter(|e| e["code"] == "invalid_configuration").count(), 3);
        assert_eq!(normalized_script("plain\nwords", 1).unwrap(), "Speaker 1: plain words");
        assert_eq!(normalized_script(" speaker 2: Other\nSpeaker 1: First ", 2).unwrap(),
            "Speaker 2: Other\nSpeaker 1: First");
        assert!(normalized_script("Speaker 2: Missing one", 2).is_err());
    }

    #[test]
    fn only_issued_inputs_are_readable_even_without_configuration() {
        let work = Work::new();
        fs::write(work.0.dir.join("unissued.wav"), fixture_wav()).unwrap();
        for name in ["unissued.wav", "../escape.wav", "C:\\private.wav", "\\\\host\\share\\a.wav", "voice.pt"] {
            assert!(preflight(None, &work.0, &json!({"engine":"vibevoice","voiceRefs":[name]})).is_err());
        }
        issue_reference(&work.0);
        let report = preflight(None, &work.0, &json!({"engine":"vibevoice","voiceRefs":["reference.wav"]})).unwrap();
        assert_eq!(report["ready"], false);
        assert_eq!(report["missing_fields"], json!(["scriptText"]));
    }

    #[test]
    fn worker_manifest_reservations_never_overwrite_existing_files() {
        let work = Work::new();
        let manifest = work.0.dir.join("occupied.synth.json");
        fs::write(&manifest, b"private existing file").unwrap();
        assert!(work.0.reserve_speech_with_manifest("occupied.wav", ".synth.json").is_err());
        assert_eq!(fs::read(&manifest).unwrap(), b"private existing file");
        assert!(work.0.open_input("occupied.synth.json").is_err());
        let (wav, sidecar) = work.0.reserve_speech_with_manifest("fresh.wav", ".synth.json").unwrap();
        fs::write(&wav, fixture_wav()).unwrap();
        fs::write(&sidecar, b"{}").unwrap();
        work.0.adopt_new_files().unwrap();
        assert!(work.0.open_input("fresh.wav").is_err());
        assert!(work.0.open_input("fresh.synth.json").is_err());
        work.0.discard_reserved("fresh.wav");
        work.0.discard_reserved("fresh.synth.json");
        assert!(!wav.exists());
        assert!(!sidecar.exists());
        assert!(work.0.reserve_speech_with_manifest("fresh.wav", ".synth.json").is_ok());
    }

    #[test]
    fn plans_are_shell_free_isolated_and_keep_inputs_private() {
        let work = Work::new();
        issue_reference(&work.0);
        let Some(runtime) = mock_runtime(&work.0, "success") else { return };
        let request = Request::parse(&args(ModelEngine::Vibevoice), false).unwrap();
        let plan = Plan::new(&work.0, request, runtime, false).unwrap();
        let private = plan.private_text.clone().unwrap();
        assert!(work.0.open_input(private.file_name().unwrap().to_str().unwrap()).is_err());
        let command = plan.command();
        assert!(Path::new(command.get_program()).is_absolute());
        let argv: Vec<_> = command.get_args().map(|s| s.to_string_lossy().into_owned()).collect();
        assert!(argv.windows(2).any(|w| w == ["-m", "gen_audio.model_workers"]));
        assert!(!argv.iter().any(|s| s.contains("Private requested words")));
        let env: std::collections::HashMap<_, _> = command.get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        assert_eq!(env["PYTHONPATH"], Some(plan.runtime.source.to_string_lossy().into_owned()));
        assert_eq!(env["PYTHONNOUSERSITE"], Some("1".into()));
        assert!(!env.contains_key("PYTHONHOME"));
        assert!(!env.contains_key("OPENAI_API_KEY"));
        assert!(work.0.reserve_speech_with_manifest("result.wav", ".synth.json").is_err());
        drop(plan);
        assert!(!private.exists());
        assert!(work.0.reserve_speech_with_manifest("result.wav", ".synth.json").is_ok());
    }

    #[test]
    fn actual_mock_process_reports_exit_two_blockers_and_cannot_fake_ready() {
        let _lock = PROCESS_TEST.lock().unwrap();
        let work = Work::new();
        issue_reference(&work.0);
        for mode in ["blocked", "ready", "exit-mismatch"] {
            let Some(runtime) = mock_runtime(&work.0, mode) else { return };
            let mut request = args(ModelEngine::Vibevoice);
            request.as_object_mut().unwrap().remove("output");
            let mut plan = Plan::new(&work.0, Request::parse(&request, true).unwrap(), runtime, true).unwrap();
            let value = execute(&mut plan);
            assert_eq!(value["ready"], mode == "ready", "{value}");
            assert_eq!(value["synthesizedSpeech"], false);
            if mode == "blocked" {
                assert_eq!(value["status"], "unavailable");
                assert_eq!(value["errors"][0]["code"], "insufficient_memory");
                assert_eq!(value["dependencies"]["torch"], Value::Null);
                assert!(!value.to_string().contains("mock-model"));
            }
            if mode == "exit-mismatch" { assert_eq!(value["status"], "failed"); }
        }
        let runtime = mock_runtime(&work.0, "ready").unwrap();
        let mut plan = Plan::new(&work.0, Request::parse(&json!({"engine":"vibevoice"}), true).unwrap(), runtime, true).unwrap();
        let value = execute(&mut plan);
        assert_eq!(value["ready"], false);
        assert_eq!(value["missing_fields"], json!(["scriptText","voiceRefs"]));
    }

    #[test]
    fn actual_mock_generation_verifies_both_engines_and_flags_incomplete_audio() {
        let _lock = PROCESS_TEST.lock().unwrap();
        for engine in [ModelEngine::Vibevoice, ModelEngine::Personaplex] {
            for mode in ["success", "limit", "counter-limit"] {
                if engine == ModelEngine::Personaplex && mode == "counter-limit" { continue; }
                let work = Work::new();
                issue_reference(&work.0);
                let Some(runtime) = mock_runtime(&work.0, mode) else { return };
                let request = Request::parse(&args(engine), false).unwrap();
                let mut plan = Plan::new(&work.0, request, runtime, false).unwrap();
                let private = plan.private_text.clone();
                let value = execute(&mut plan);
                assert_eq!(value["synthesizedSpeech"], true, "{value}");
                assert_eq!(value["ok"], mode == "success");
                assert_eq!(value["limit_reached"], mode != "success");
                assert_eq!(value["complete"], mode == "success");
                assert_eq!(value["transcript_verified"], false);
                assert_eq!(value["task"], engine.task());
                assert_eq!(value["manifest"], "result.synth.json");
                assert_eq!(value["samples"], 24000);
                assert_eq!(value["duration_s"], 1.0);
                let text = value.to_string();
                assert!(!text.contains("Private requested words"));
                assert!(!text.contains("Helpful private persona"));
                assert!(!text.contains("DO NOT EXPOSE"));
                assert!(!text.contains(work.0.dir.to_str().unwrap()));
                assert!(work.0.open_input("result.wav").is_ok());
                assert!(work.0.open_input("result.synth.json").is_ok());
                assert!(work.0.reserve_speech_with_manifest("result.wav", ".synth.json").is_err());
                drop(plan);
                if let Some(path) = private { assert!(!path.exists()); }
                assert!(work.0.dir.join("result.wav").exists());
            }
        }
    }

    #[test]
    fn malformed_worker_outputs_never_issue_artifacts_and_cleanup_partial_files() {
        let _lock = PROCESS_TEST.lock().unwrap();
        let work = Work::new();
        issue_reference(&work.0);
        for mode in ["hash", "samples", "rate", "duration", "path", "manifest-mismatch",
            "task", "semantics", "transcript", "provenance", "text-hash", "no-limit",
            "missing-manifest", "missing-wav", "corrupt-wav", "silence", "trailing-log", "flood", "blocked", "exit-mismatch"] {
            let Some(runtime) = mock_runtime(&work.0, mode) else { return };
            let request = Request::parse(&args(ModelEngine::Vibevoice), false).unwrap();
            let mut plan = Plan::new(&work.0, request, runtime, false).unwrap();
            let private = plan.private_text.clone().unwrap();
            let value = execute(&mut plan);
            assert_eq!(value["ok"], false, "{mode}: {value}");
            assert_eq!(value["synthesizedSpeech"], false, "{mode}: {value}");
            assert!(work.0.open_input("result.wav").is_err(), "{mode}");
            work.0.adopt_new_files().unwrap();
            assert!(work.0.open_input("result.synth.json").is_err(), "{mode}");
            assert!(work.0.open_input(private.file_name().unwrap().to_str().unwrap()).is_err());
            drop(plan);
            assert!(!private.exists());
            assert!(!work.0.dir.join("result.wav").exists());
            assert!(!work.0.dir.join("result.synth.json").exists());
        }
        let runtime = mock_runtime(&work.0, "persona-hash").unwrap();
        let mut plan = Plan::new(&work.0, Request::parse(&args(ModelEngine::Personaplex), false).unwrap(), runtime, false).unwrap();
        assert_eq!(execute(&mut plan)["ok"], false);
    }

    #[test]
    #[ignore = "opt-in configured local model preflight; never imports torch or loads weights"]
    fn real_configured_preflight() {
        let work = Work::new();
        issue_reference(&work.0);
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let mut checked = 0;
        for engine in [ModelEngine::Vibevoice, ModelEngine::Personaplex] {
            if std::env::var_os(format!("{}_PYTHON", engine.prefix())).is_none() { continue; }
            let mut request = args(engine);
            request.as_object_mut().unwrap().remove("output");
            let value = preflight(Some(&repo), &work.0, &request).unwrap();
            println!("{}: {value}", engine.name());
            assert_eq!(value["synthesizedSpeech"], false);
            assert_eq!(value["inference_validated"], false);
            assert!(value.get("wav").is_none());
            assert!(!work.0.dir.join("result.wav").exists());
            checked += 1;
        }
        assert!(checked > 0, "Configure at least one GEN_AUDIO_<ENGINE>_PYTHON/MODEL_DIR/TOKENIZER");
    }
}
