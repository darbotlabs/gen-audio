//! Build argv for the Python CLIs. The process is spawned with an argument
//! vector, never a shell string.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::paths::{read_bounded, read_trusted_script, read_user_repo_file, Scratch, SpeechReservation};
use crate::redact::redact_secrets;
use crate::{catalog, subprocess};

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
pub struct PythonPlan<'a> {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    speech: Option<SpeechExpectation>,
    private_inputs: Vec<PathBuf>,
    _reservation: Option<SpeechReservation<'a>>,
}

impl Drop for PythonPlan<'_> {
    fn drop(&mut self) {
        for path in &self.private_inputs {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Explicit routing seam for additional real adapters; never alias an engine.
pub fn synth_engine(id: &str) -> Result<PythonTool, String> {
    match id {
        "kokoro_onnx" => Ok(PythonTool::Synth),
        "personaplex" | "vibevoice" => Err(format!("{id} synthesis adapter is not installed")),
        _ => Err(format!("unsupported synthesis engine {id}")),
    }
}

#[derive(Debug)]
struct SpeechExpectation {
    output: PathBuf,
    manifest: PathBuf,
    turns: Vec<ExpectedTurn>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
struct ExpectedTurn {
    speaker_id: String,
    name: String,
    voice: String,
    lang: String,
    speed: f64,
    text: String,
}

#[derive(Deserialize)]
struct ManifestTurn {
    speaker_id: String,
    name: String,
    voice: String,
    lang: String,
    speed: f64,
    text_sha256: String,
    text_chars: usize,
    samples: u64,
    sample_rate: u32,
}

#[derive(Deserialize)]
struct SpeechManifest {
    schema_version: String,
    #[serde(rename = "synthesizedSpeech")]
    synthesized_speech: bool,
    engine: String,
    output: PathBuf,
    manifest: PathBuf,
    sample_rate: u32,
    samples: u64,
    duration_s: f64,
    sha256: String,
    bytes: usize,
    improved: bool,
    wav_subtype: String,
    turn_count: usize,
    turns: Vec<ManifestTurn>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechReceipt {
    schema_version: String,
    ok: bool,
    #[serde(rename = "synthesizedSpeech")]
    synthesized_speech: bool,
    engine: String,
    output: PathBuf,
    manifest: PathBuf,
    sample_rate: u32,
    samples: u64,
    duration_s: f64,
    sha256: String,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Serialize)]
pub struct VerifiedSpeech {
    pub schema_version: &'static str,
    pub ok: bool,
    #[serde(rename = "synthesizedSpeech")]
    pub synthesized_speech: bool,
    pub engine: &'static str,
    pub wav: String,
    pub output: String,
    pub manifest: String,
    pub sample_rate: u32,
    pub samples: u64,
    pub duration_s: f64,
    pub sha256: String,
    pub turns: usize,
    pub metadata: Value,
}

pub fn python_program() -> Result<PathBuf, String> {
    let fallback = if cfg!(windows) { "python" } else { "python3" };
    let raw = crate::runtime_config::setting("GEN_AUDIO_PYTHON").and_then(|s|s.into_string().ok()).unwrap_or_else(|| fallback.to_string());
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

pub fn plan<'a>(
    tool: PythonTool,
    repo: &Path,
    scratch: &'a Scratch,
    args: &Value,
) -> Result<PythonPlan<'a>, String> {
    let program = python_program()?;
    let script = read_trusted_script(repo, tool.script())?;
    let mut planned = vec![script.to_string_lossy().to_string()];
    let mut reservation = None;
    let mut speech = None;
    let mut private_inputs = Vec::new();
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
            let steps = match args.get("maxSteps") {
                None => 4,
                Some(value) => value.as_u64().ok_or("maxSteps must be an integer")?,
            };
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
            let (text, cast, turns) = synth_inputs(repo, args)?;
            let model = crate::runtime_config::setting("GEN_AUDIO_KOKORO_MODEL").and_then(|s|s.into_string().ok()).unwrap_or_default();
            let voices = crate::runtime_config::setting("GEN_AUDIO_KOKORO_VOICES").and_then(|s|s.into_string().ok()).unwrap_or_default();
            if model.is_empty() || voices.is_empty() {
                return Err(
                    "synth was not started: GEN_AUDIO_KOKORO_MODEL and GEN_AUDIO_KOKORO_VOICES are unset. No speech was invented."
                        .into(),
                );
            }
            let model_path = bounded_model_file(&model)?;
            let voices_path = bounded_model_file(&voices)?;
            let output_name = opt_name(args, "output")?.map(str::to_owned)
                .unwrap_or_else(|| scratch.unique_name(".wav"));
            let reserved = scratch.reserve_speech_guard(&output_name)?;
            let output = reserved.output.clone();
            let manifest = reserved.manifest.clone();
            reservation = Some(reserved);
            let script_path = scratch.write_private_input(text.as_bytes(), ".txt")?;
            let cast_bytes = serde_json::to_vec(&cast).map_err(|_| "cast serialization failed")?;
            let cast_path = match scratch.write_private_input(&cast_bytes, ".json") {
                Ok(path) => path,
                Err(error) => {
                    let _ = std::fs::remove_file(script_path);
                    return Err(error);
                }
            };
            private_inputs.extend([script_path.clone(), cast_path.clone()]);
            planned.push(script_path.to_string_lossy().to_string());
            planned.push("--cast-map".into());
            planned.push(cast_path.to_string_lossy().to_string());
            planned.push("-o".into());
            planned.push(output.to_string_lossy().to_string());
            planned.push("--model".into());
            planned.push(model_path.to_string_lossy().to_string());
            planned.push("--voices".into());
            planned.push(voices_path.to_string_lossy().to_string());
            planned.extend(["--json".into(), "--improve".into()]);
            speech = Some(SpeechExpectation { output, manifest, turns });
        }
    }
    Ok(PythonPlan {
        program,
        args: planned,
        cwd: repo.to_path_buf(),
        speech,
        private_inputs,
        _reservation: reservation,
    })
}

/// Argv for `gen_audio.cli.generate` via the trusted shim. The prompt stays in the work file.
pub fn plan_generate(
    repo: &Path,
    work: &Path,
    personas: &[String],
    engine: &str,
    duration_s: f64,
) -> Result<PythonPlan<'static>, String> {
    let script = read_trusted_script(repo, "scripts/generate.py")?;
    if personas.is_empty() || personas.len() > 8 {
        return Err("personas must be 1 to 8 ids".into());
    }
    for id in personas {
        if id.len() > 32 || !id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_') {
            return Err("persona id is invalid".into());
        }
    }
    if engine.len() > 32 || !engine.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_') {
        return Err("engine id must be a short lowercase slug".into());
    }
    if !(0.5..=1800.0).contains(&duration_s) {
        return Err("duration must be from 0.5 to 1800 seconds".into());
    }
    let prompt = work.join("scripts").join("prompt.txt");
    let prompt = prompt.canonicalize().map_err(|err| format!("prompt file: {err}"))?;
    let work_canon = work.canonicalize().map_err(|err| format!("work directory: {err}"))?;
    if !prompt.starts_with(&work_canon) {
        return Err("prompt file is outside the work directory".into());
    }
    Ok(PythonPlan {
        program: python_program()?,
        args: vec![
            script.to_string_lossy().to_string(),
            "--prompt-file".into(),
            prompt.to_string_lossy().to_string(),
            "--personas".into(),
            personas.join(","),
            "--engine".into(),
            engine.to_string(),
            "--duration-s".into(),
            format!("{duration_s}"),
            "--out-dir".into(),
            work_canon.to_string_lossy().to_string(),
        ],
        cwd: repo.to_path_buf(),
        speech: None,
        private_inputs: Vec::new(),
        _reservation: None,
    })
}

pub(crate) fn command(plan: &PythonPlan<'_>) -> Command {
    let mut command = Command::new(&plan.program);
    command.args(&plan.args).current_dir(&plan.cwd)
        .env_remove("GEN_AUDIO_ARTIFACT_DIR")
        .env("GENAID_LOCAL_FILES_ONLY", "1");
    for key in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "COPILOT_GITHUB_TOKEN",
        "GEN_AUDIO_MCP_TOKEN",
        "GEN_AUDIO_MCP_BOOT_NONCE",
    ] {
        command.env_remove(key);
    }
    for key in ["GEN_AUDIO_KOKORO_MODEL", "GEN_AUDIO_KOKORO_VOICES", "GENAID_MODEL_BASE"] {
        if let Some(value) = crate::runtime_config::setting(key) {
            command.env(key, value);
        }
    }
    command
}

pub(crate) fn resident_command(repo: &Path, scratch: &Scratch) -> Result<Command, String> {
    crate::paths::read_repo_relative(repo, "src/gen_audio/runtime_owner.py")?;
    let plan = PythonPlan { program: python_program()?, args: vec![], cwd: scratch.dir.clone(),
        speech: None, private_inputs: vec![], _reservation: None };
    let mut result = command(&plan);
    result.env("PYTHONPATH", repo.join("src")).env("PYTHONNOUSERSITE", "1")
        .env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1")
        .args(["-u", "-m", "gen_audio.runtime_owner", "--engine", "kokoro_onnx"]);
    Ok(result)
}

pub fn start_plan(plan: &PythonPlan) -> Result<std::process::Child, String> {
    let mut command = command(plan);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Own process group so a timeout or cancel can signal the python child
    // and anything it spawned.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .spawn()
        .map_err(|err| format!("failed to start python: {err}"))
}

pub fn run_plan(plan: &PythonPlan<'_>) -> Result<Value, String> {
    let output = if plan.speech.is_some() {
        match crate::runtime_owner::try_run("kokoro_onnx", "run", &command(plan), &plan.args[1..], &std::sync::atomic::AtomicBool::new(false))? {
            Some(output) => output,
            None => subprocess::run(&mut command(plan), b"", subprocess::Limits::default())?,
        }
    } else { subprocess::run(&mut command(plan), b"", subprocess::Limits::default())? };
    if let Some(expected) = &plan.speech {
        if !output.status.success() {
            return Err(format!("synthesis process failed (exit {:?}); no verified speech", output.status.code()));
        }
        return serde_json::to_value(verify_speech(expected, &output.stdout)?)
            .map_err(|_| "speech result serialization failed".into());
    }
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

pub fn run_generate_plan(
    plan: &PythonPlan<'_>,
    deadline: std::time::Duration,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<subprocess::Output, String> {
    if let Some(output) = crate::runtime_owner::try_run("kokoro_onnx", "run_generate", &command(plan), &plan.args[1..], cancel)? { return Ok(output); }
    subprocess::run_cancellable(&mut command(plan), b"", subprocess::Limits {
        deadline,
        stdout: 1024 * 1024,
        stderr: 1024 * 1024,
        ..subprocess::Limits::default()
    }, cancel)
}

fn synth_inputs(repo: &Path, args: &Value) -> Result<(String, Value, Vec<ExpectedTurn>), String> {
    let obj = args.as_object().ok_or("synth arguments must be an object")?;
    for key in obj.keys() {
        if !["script", "scriptText", "castMap", "agents", "voice", "output"].contains(&key.as_str()) {
            return Err(format!("unexpected synth argument {key}"));
        }
    }
    let script = opt_name(args, "script")?;
    let inline = opt_name(args, "scriptText")?;
    let text = match (script, inline) {
        (Some(path), None) => String::from_utf8(read_bounded(&read_user_repo_file(repo, path)?, 256 * 1024)?)
            .map_err(|_| "script must be UTF-8")?,
        (None, Some(text)) if text.len() <= 256 * 1024 => text.to_owned(),
        _ => return Err("provide exactly one of script or scriptText (at most 256 KiB)".into()),
    };
    let text = text.trim_start_matches('\u{feff}');
    if text.contains('\0') { return Err("script contains NUL".into()); }
    // A single-persona request may supply prose. Multiple personas need explicit turns.
    let has_header = text.lines().any(|line| {
        let line = line.trim_start();
        line.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("Speaker"))
            && line.get(7..).and_then(|rest| rest.chars().next()).is_some_and(char::is_whitespace)
    });
    let text = if !has_header && args.get("agents").and_then(Value::as_array).is_some_and(|a| a.len() == 1) {
        format!("Speaker 1: {text}")
    } else { text.to_owned() };
    let turns = crate::script::parse_script(&text)?;
    let cast = if let Some(cast_path) = opt_name(args, "castMap")? {
        if args.get("agents").is_some() || args.get("voice").is_some() {
            return Err("castMap is mutually exclusive with agents and voice".into());
        }
        serde_json::from_slice::<Value>(&read_bounded(&read_user_repo_file(repo, cast_path)?, 256 * 1024)?)
            .map_err(|_| "castMap must contain JSON")?
    } else {
        synth_engine(req_string(args, "voice")?)?;
        let agents = args.get("agents").and_then(Value::as_array).ok_or("agents must be an array")?;
        if agents.is_empty() || agents.len() > catalog::MAX_AGENTS_PER_TRACK {
            return Err("agents must contain 1 to 8 mapped personas".into());
        }
        let mut seen = std::collections::HashSet::new();
        let mut speakers = serde_json::Map::new();
        for (i, agent) in agents.iter().enumerate() {
            let id = agent.as_str().ok_or("agent id must be a string")?;
            if !seen.insert(id) { return Err("duplicate persona".into()); }
            let persona = catalog::persona(id).ok_or_else(|| format!("unknown persona {id}"))?;
            let voice = catalog::persona_kokoro_pack(id)
                .ok_or_else(|| format!("persona {id} has no mapped Kokoro voice"))?;
            speakers.insert((i + 1).to_string(), json!({"name":persona.name,"voice":voice}));
        }
        json!({"engine":"kokoro_onnx","default_lang":"en-us","default_speed":1.0,"speakers":speakers})
    };
    synth_engine(cast.get("engine").and_then(Value::as_str).ok_or("cast engine must be a string")?)?;
    let speakers = cast.get("speakers").and_then(Value::as_object).ok_or("cast speakers must be an object")?;
    if speakers.is_empty() || speakers.len() > 8 { return Err("cast must contain 1 to 8 speakers".into()); }
    let default_lang = opt_name(&cast, "default_lang")?.unwrap_or("en-us");
    let default_speed = speed(&cast, "default_speed", 1.0)?;
    let mut normalized = serde_json::Map::new();
    for (id, assignment) in speakers {
        let number = id.parse::<u32>().map_err(|_| "invalid cast speaker id")?.to_string();
        if !assignment.is_object() { return Err("cast assignment must be an object".into()); }
        let voice = req_string(assignment, "voice")?;
        if !catalog::personas().iter().any(|p| catalog::persona_kokoro_pack(p.id) == Some(voice)) {
            return Err(format!("unmapped Kokoro voice {voice}"));
        }
        let lang = opt_name(assignment, "lang")?.unwrap_or(default_lang);
        if lang.len() > 16 || lang.is_empty() || !lang.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
            return Err("invalid cast language".into());
        }
        let name = opt_name(assignment, "name")?.unwrap_or("Speaker");
        if name.len() > 128 || name.chars().any(char::is_control) { return Err("invalid cast speaker name".into()); }
        if normalized.insert(number, json!({"name":name,"voice":voice,"lang":lang,"speed":speed(assignment,"speed",default_speed)?})).is_some() {
            return Err("duplicate normalized cast speaker id".into());
        }
    }
    let expected = turns.into_iter().map(|turn| {
        let a = normalized.get(&turn.speaker_id).ok_or_else(|| format!("Speaker {} is not in the cast", turn.speaker_id))?;
        Ok(ExpectedTurn {
            speaker_id: turn.speaker_id, name: a["name"].as_str().unwrap().into(),
            voice: a["voice"].as_str().unwrap().into(), lang: a["lang"].as_str().unwrap().into(),
            speed: a["speed"].as_f64().unwrap(), text: turn.text,
        })
    }).collect::<Result<Vec<_>, String>>()?;
    Ok((text, json!({"engine":"kokoro_onnx","speakers":normalized}), expected))
}

fn speed(args: &Value, key: &str, default: f64) -> Result<f64, String> {
    let value = match args.get(key) {
        None => default,
        Some(value) => value.as_f64().ok_or("cast speed must be a number")?,
    };
    if !(0.5..=2.0).contains(&value) { return Err("cast speed must be from 0.5 to 2".into()); }
    Ok(value)
}

fn verify_speech(expected: &SpeechExpectation, stdout: &[u8]) -> Result<VerifiedSpeech, String> {
    let line = std::str::from_utf8(stdout).map_err(|_| "synth output must be UTF-8")?
        .lines().rev().find(|line| !line.trim().is_empty()).ok_or("missing synth JSON receipt")?;
    let receipt: SpeechReceipt = serde_json::from_str(line).map_err(|_| "invalid typed synth JSON receipt")?;
    if receipt.schema_version != "1.0" || !receipt.ok || !receipt.synthesized_speech || receipt.engine != "kokoro_onnx" {
        return Err("synth JSON receipt does not claim successful Kokoro speech".into());
    }
    for (actual, planned) in [(&receipt.output, &expected.output), (&receipt.manifest, &expected.manifest)] {
        if !actual.is_absolute() || actual != planned {
            return Err("synth returned an unexpected artifact path".into());
        }
    }
    let wav = read_bounded(&expected.output, 192 * 1024 * 1024)?;
    let (sample_rate, samples) = wav_metadata(&wav)?;
    let duration_s = samples as f64 / sample_rate as f64;
    let digest = format!("{:x}", Sha256::digest(&wav));
    if receipt.sample_rate != sample_rate || receipt.samples != samples
        || !receipt.duration_s.is_finite() || (receipt.duration_s - duration_s).abs() > 1e-6
        || receipt.sha256 != digest {
        return Err("synth WAV does not match receipt hash or metadata".into());
    }
    let manifest: SpeechManifest = serde_json::from_slice(&read_bounded(&expected.manifest, 512 * 1024)?)
        .map_err(|_| "invalid synth manifest")?;
    if manifest.schema_version != "1.0" || !manifest.synthesized_speech
        || manifest.engine != "kokoro_onnx" || manifest.sample_rate != sample_rate
        || manifest.samples != samples || !manifest.improved || manifest.wav_subtype != "PCM_16"
        || !manifest.duration_s.is_finite() || (manifest.duration_s - duration_s).abs() > 1e-6
        || manifest.sha256 != digest || manifest.bytes != wav.len()
        || manifest.output != expected.output || manifest.manifest != expected.manifest {
        return Err("synth manifest metadata disagrees with published WAV".into());
    }
    if manifest.turns.len() != expected.turns.len() || manifest.turn_count != expected.turns.len()
        || manifest.turns.iter().zip(&expected.turns).any(|(actual, expected)| {
            actual.speaker_id != expected.speaker_id || actual.name != expected.name
                || actual.voice != expected.voice || actual.lang != expected.lang || actual.speed != expected.speed
                || actual.text_sha256 != format!("{:x}", Sha256::digest(expected.text.as_bytes()))
                || actual.text_chars != expected.text.chars().count() || actual.samples == 0 || actual.sample_rate == 0
        }) {
        return Err("synth manifest does not match requested text and cast".into());
    }
    let wav_name = expected.output.file_name().unwrap().to_string_lossy().into_owned();
    let mut names = Vec::new();
    for turn in &expected.turns { if !names.contains(&turn.name) { names.push(turn.name.clone()); } }
    crate::audio_metadata::verify(&wav, &receipt.metadata, &names)?;
    let manifest_value: Value = serde_json::from_slice(&read_bounded(&expected.manifest, 512 * 1024)?).map_err(|_| "invalid synth manifest")?;
    if manifest_value["metadata"] != receipt.metadata { return Err("Artist manifest and receipt disagree".into()); }
    Ok(VerifiedSpeech {
        schema_version: "1.0", ok: true, synthesized_speech: true, engine: "kokoro_onnx",
        output: wav_name.clone(), wav: wav_name,
        manifest: expected.manifest.file_name().unwrap().to_string_lossy().into_owned(),
        sample_rate, samples, duration_s, sha256: digest, turns: expected.turns.len(), metadata: receipt.metadata,
    })
}

pub(crate) fn wav_metadata(bytes: &[u8]) -> Result<(u32, u64), String> {
    let invalid = || "invalid or unsupported published WAV".to_string();
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" { return Err(invalid()); }
    let u32_at = |i| u32::from_le_bytes(bytes[i..i+4].try_into().unwrap());
    if u32_at(4) as usize + 8 != bytes.len() { return Err(invalid()); }
    let mut offset = 12;
    let mut rate = None;
    let mut data_size = None;
    while offset + 8 <= bytes.len() {
        let size = u32_at(offset+4) as usize;
        let start = offset + 8;
        let end = start.checked_add(size).filter(|end| *end <= bytes.len()).ok_or_else(invalid)?;
        match &bytes[offset..offset+4] {
            b"fmt " => {
                if rate.is_some() || size < 16 { return Err(invalid()); }
                let u16_at = |i| u16::from_le_bytes(bytes[i..i+2].try_into().unwrap());
                let sample_rate = u32_at(start+4);
                if u16_at(start) != 1 || u16_at(start+2) != 1 || u16_at(start+14) != 16
                    || u16_at(start+12) != 2 || sample_rate != 24_000 || u32_at(start+8) != sample_rate*2 {
                    return Err(invalid());
                }
                rate = Some(sample_rate);
            }
            b"data" => {
                if data_size.is_some() || size == 0 || size % 2 != 0 || bytes[start..end].iter().all(|b| *b == 0) {
                    return Err(invalid());
                }
                data_size = Some(size);
            }
            _ => {}
        }
        offset = end + (size % 2);
    }
    if offset != bytes.len() { return Err(invalid()); }
    Ok((rate.ok_or_else(invalid)?, (data_size.ok_or_else(invalid)? / 2) as u64))
}

fn bounded_model_file(raw: &str) -> Result<PathBuf, String> {
    let root = crate::runtime_config::setting("GEN_AUDIO_MODEL_DIR")
        .ok_or("GEN_AUDIO_MODEL_DIR is required so tool arguments cannot point at arbitrary files".to_string())?;
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
        None => Ok(None),
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
        let mut saved: Vec<_> = KOKORO_ENV.iter().map(|name| (*name, std::env::var_os(name))).collect();
        saved.push(("GEN_AUDIO_CONFIG", std::env::var_os("GEN_AUDIO_CONFIG")));
        std::env::set_var("GEN_AUDIO_CONFIG", PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));
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

    /// Viewport binding and the library-root override are the same kind of seam
    /// as NoKokoroEnv: other crates' tests reach them through `test-support`,
    /// and a release build of this crate does not export them.
    #[test]
    fn viewport_and_library_test_seams_are_test_support_only() {
        let gate = "#[cfg(any(test, feature = \"test-support\"))]\n";
        let files = [
            (include_str!("viewport.rs"), &["pub struct ViewportHandle", "pub struct ViewportGuard", "pub fn bind_viewport", "thread_local!"][..]),
            (include_str!("library_store.rs"), &["pub fn set_root_override_for_test", "thread_local!"][..]),
        ];
        for (source, items) in files {
            for item in items {
                let at = source.find(&format!("\n{item}")).unwrap_or_else(|| panic!("{item} not found"));
                assert!(
                    source[..at + 1].ends_with(gate),
                    "{item} must sit right under the test-support gate"
                );
            }
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
        struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                for (name, value) in &self.0 {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
        let _restore = RestoreEnv(["GEN_AUDIO_KOKORO_MODEL", "GEN_AUDIO_KOKORO_VOICES", "GEN_AUDIO_CONFIG"]
            .into_iter().map(|name| (name, std::env::var_os(name))).collect());
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
            &json!({"output": "should-not-exist.wav", "scriptText":"Only requested words.", "agents":["alice"], "voice":"kokoro_onnx"}),
        )
        .unwrap_err();
        assert!(err.contains("No speech was invented"));
        let _ = std::fs::remove_dir_all(&scratch.dir);
    }

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
    }

    #[test]
    fn synthesis_requires_actual_content_and_mapped_cast() {
        let request = json!({"scriptText":"Requested words, not an example.", "agents":["alice"],"voice":"kokoro_onnx"});
        let (text, cast, expected) = synth_inputs(&repo(), &request).unwrap();
        assert_eq!(text, "Speaker 1: Requested words, not an example.");
        assert_eq!(cast["speakers"]["1"]["voice"], "af_heart");
        assert_eq!(expected[0].text, "Requested words, not an example.");
        let (_, cast, turns) = synth_inputs(&repo(), &json!({
            "scriptText":"Speaker 1: Alice says this.\nSpeaker 2: Frank says something else.",
            "agents":["alice","frank"],"voice":"kokoro_onnx"
        })).unwrap();
        assert_eq!(cast["speakers"]["2"]["voice"], "am_michael");
        assert_eq!(turns.len(), 2);
        let (_, _, turns) = synth_inputs(&repo(), &json!({
            "scriptText":"SPEAKER\t1: First.\nsPeAkEr 2: Second.",
            "agents":["alice","frank"],"voice":"kokoro_onnx"
        })).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].text, "Second.");
        for invalid in [
            json!({}),
            json!({"script":null,"scriptText":"hello","agents":["alice"],"voice":"kokoro_onnx"}),
            json!({"script":"examples/podcast_script_sample.txt","scriptText":"x","castMap":"voices/cast_map.example.json"}),
            json!({"scriptText":"x","castMap":"voices/cast_map.example.json","agents":["alice"],"voice":"kokoro_onnx"}),
            json!({"scriptText":"hello","agents":["anton"],"voice":"kokoro_onnx"}),
            json!({"scriptText":"hello","agents":["alice"],"voice":"vibevoice"}),
            json!({"scriptText":"hello","agents":["alice"],"voice":"personaplex"}),
            json!({"scriptText":"Speaker 2: Uncast.","agents":["alice"],"voice":"kokoro_onnx"}),
            json!({"scriptText":"unassigned prose","agents":["alice","frank"],"voice":"kokoro_onnx"}),
        ] { assert!(synth_inputs(&repo(), &invalid).is_err(), "{invalid}"); }
    }

    #[test]
    fn abandoned_and_failed_plans_release_unissued_speech() {
        let scratch = Scratch::create().unwrap();
        let reservation = scratch.reserve_speech_guard("retry.wav").unwrap();
        let output = reservation.output.clone();
        let manifest = reservation.manifest.clone();
        let abandoned = PythonPlan {
            program: PathBuf::from("does-not-exist"),
            args: Vec::new(),
            cwd: repo(),
            speech: None,
            private_inputs: Vec::new(),
            _reservation: Some(reservation),
        };
        assert!(scratch.reserve_speech("retry.wav").is_err());
        assert!(run_plan(&abandoned).is_err());
        std::fs::write(&output, b"unverified output").unwrap();
        std::fs::write(&manifest, b"unverified manifest").unwrap();
        drop(abandoned);
        assert!(!output.exists() && !manifest.exists());
        let next = scratch.reserve_speech_guard("retry.wav").unwrap();
        std::fs::write(&next.output, b"verified by caller").unwrap();
        scratch.adopt_file("retry.wav").unwrap();
        drop(next);
        assert!(scratch.open_input("retry.wav").is_ok());
        std::fs::remove_dir_all(scratch.dir).unwrap();
    }

    fn mock_speech(scratch: &Scratch) -> (SpeechExpectation, Value, Value) {
        let fixture = write_fixture_tone(scratch).unwrap();
        let output = scratch.dir.join("verified.wav");
        let mut fixture_bytes = std::fs::read(fixture.wav).unwrap();
        fixture_bytes[24..28].copy_from_slice(&24_000u32.to_le_bytes());
        fixture_bytes[28..32].copy_from_slice(&48_000u32.to_le_bytes());
        std::fs::write(&output, crate::audio_metadata::with_artist(&fixture_bytes, "Alice").unwrap()).unwrap();
        let manifest_path = scratch.dir.join("verified.json");
        let bytes = std::fs::read(&output).unwrap();
        let (rate, samples) = wav_metadata(&bytes).unwrap();
        let (_, _, turns) = synth_inputs(&repo(), &json!({"scriptText":"Contract test only.", "agents":["alice"],"voice":"kokoro_onnx"})).unwrap();
        let receipt = json!({
            "schema_version":"1.0","ok":true,"synthesizedSpeech":true,"engine":"kokoro_onnx",
            "output":output,"manifest":manifest_path,"sample_rate":rate,"samples":samples,
            "duration_s":samples as f64 / rate as f64,"sha256":format!("{:x}",Sha256::digest(&bytes)),
            "metadata":crate::audio_metadata::for_agents(&["Alice".into()]).unwrap()
        });
        let mut manifest = receipt.clone();
        manifest["improved"] = json!(true);
        manifest["wav_subtype"] = json!("PCM_16");
        manifest["bytes"] = json!(bytes.len());
        manifest["turn_count"] = json!(1);
        manifest["turns"] = json!([{
            "speaker_id":"1","name":"Alice","voice":"af_heart","lang":"en-us","speed":1.0,
            "text_sha256":format!("{:x}",Sha256::digest(turns[0].text.as_bytes())),
            "text_chars":turns[0].text.chars().count(),"samples":samples,"sample_rate":rate
        }]);
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        (SpeechExpectation { output, manifest:manifest_path, turns }, receipt, manifest)
    }

    #[test]
    fn receipt_wav_manifest_and_requested_content_must_agree() {
        // A fixture exercises the receipt verifier only, never an actual synthesis adapter.
        let scratch = Scratch::create().unwrap();
        let (expected, receipt, manifest) = mock_speech(&scratch);
        let verified = verify_speech(&expected, receipt.to_string().as_bytes()).unwrap();
        assert_eq!(verified.wav, "verified.wav");
        let schema: Value = serde_json::from_str(include_str!("../../../schemas/connectors/speech-result.schema.json")).unwrap();
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&serde_json::to_value(&verified).unwrap()));
        assert!(scratch.open_input("verified.wav").is_err());
        for (field, bad) in [
            ("schema_version",json!("2.0")), ("ok",json!(false)), ("synthesizedSpeech",json!(false)),
            ("engine",json!("vibevoice")), ("sha256",json!("0".repeat(64))),
            ("samples",json!(1)), ("sample_rate",json!(48000)), ("duration_s",json!(0)),
            ("output",json!("verified.wav")), ("manifest",json!(scratch.dir.join("wrong.json"))),
        ] {
            let mut value = receipt.clone(); value[field] = bad;
            assert!(verify_speech(&expected, value.to_string().as_bytes()).is_err(), "{field}");
        }
        for bad in [b"not json".as_slice(), b"{\"ok\":true}", b"[]"] {
            assert!(verify_speech(&expected, bad).is_err());
        }
        for (field, bad) in [("sha256",json!("wrong")), ("bytes",json!(0)), ("improved",json!(false)), ("turn_count",json!(9))] {
            let mut value = manifest.clone(); value[field] = bad;
            std::fs::write(&expected.manifest, value.to_string()).unwrap();
            assert!(verify_speech(&expected, receipt.to_string().as_bytes()).is_err(), "{field}");
        }
        let mut tampered = manifest.clone();
        tampered["turns"][0]["text_sha256"] = json!("unrequested text");
        std::fs::write(&expected.manifest, tampered.to_string()).unwrap();
        assert!(verify_speech(&expected, receipt.to_string().as_bytes()).is_err());
        std::fs::write(&expected.manifest, manifest.to_string()).unwrap();
        let mut wav = std::fs::read(&expected.output).unwrap();
        let end = wav.len()-1;
        wav[end] ^= 1;
        std::fs::write(&expected.output, wav).unwrap();
        assert!(verify_speech(&expected, receipt.to_string().as_bytes()).is_err());
        std::fs::remove_dir_all(&scratch.dir).unwrap();
    }

    #[test]
    fn connector_config_schemas_enforce_types_and_limits() {
        for (source, key, good, bad) in [
            (include_str!("../../../schemas/connectors/gpt.schema.json"),"OPENAI_MODEL",json!("gpt-test"),json!("../invalid")),
            (include_str!("../../../schemas/connectors/claude.schema.json"),"GEN_AUDIO_CLAUDE_CODE_CLI",json!("0"),json!(true)),
            (include_str!("../../../schemas/connectors/copilot.schema.json"),"COPILOT_STUDIO_ENDPOINT",json!("http://127.0.0.1:8000/mock"),json!("http://127.0.0.1.evil/mock")),
            (include_str!("../../../schemas/connectors/gemini.schema.json"),"GEMINI_API_KEY",json!("mock-key"),json!("mock\r\ninjected")),
        ] {
            let schema: Value = serde_json::from_str(source).unwrap();
            let validator = jsonschema::validator_for(&schema).unwrap();
            assert!(validator.is_valid(&json!({key:good})));
            assert!(!validator.is_valid(&json!({key:bad})));
            assert!(!validator.is_valid(&json!({"unexpected":true})));
        }
    }
}
