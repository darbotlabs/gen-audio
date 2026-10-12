//! One resident model, owned by the native backend. No caller-controlled paths.
use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use serde_json::{json, Value};
use crate::{bridge, model_worker, paths::Scratch, redact::redact_secrets, subprocess};

const DEADLINE: Duration = Duration::from_secs(120);
const CAP: usize = 256 * 1024;

struct Resident {
    engine: String,
    program: std::path::PathBuf,
    child: Child,
    writes: mpsc::SyncSender<Vec<u8>>,
    written: mpsc::Receiver<Result<(), String>>,
    replies: mpsc::Receiver<Result<String, String>>,
    tree: subprocess::tree::Tree,
    next: u64,
    _scratch: Scratch,
    metadata: Value,
}

impl Drop for Resident {
    fn drop(&mut self) { self.tree.terminate(); let _ = self.child.kill(); let _ = self.child.wait(); }
}

fn slot() -> &'static Mutex<Option<Resident>> {
    static SLOT: OnceLock<Mutex<Option<Resident>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

impl Resident {
    fn spawn(engine: &str, repo: &Path, execution: Option<&Value>) -> Result<Self, String> {
        let scratch = Scratch::create()?;
        let mut command = match engine {
            "kokoro_onnx" => bridge::resident_command(repo, &scratch)?,
            "vibevoice" | "personaplex" => model_worker::resident_command(repo, &scratch, engine, execution)?,
            _ => return Err(format!("No model-loading implementation exists for {engine}")),
        };
        let program = command.get_program().into();
        command.env_remove("GEN_AUDIO_MCP_TOKEN").env_remove("GEN_AUDIO_MCP_BOOT_NONCE")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        subprocess::tree::configure(&mut command);
        let mut child = command.spawn().map_err(|e| format!("Runtime failed to start: {e}"))?;
        let tree = match subprocess::tree::Tree::attach(&child) {
            Ok(tree) => tree,
            Err(error) => { let _ = child.kill(); let _ = child.wait(); return Err(error); }
        };
        let mut stdin = child.stdin.take().ok_or("Runtime stdin is missing")?;
        let stdout = child.stdout.take().ok_or("Runtime stdout is missing")?;
        let mut stderr = child.stderr.take().ok_or("Runtime stderr is missing")?;
        let (send, replies) = mpsc::sync_channel(1);
        let (writes, input) = mpsc::sync_channel::<Vec<u8>>(1);
        let (ack, written) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            while let Ok(message) = input.recv() {
                let result = stdin.write_all(&message).and_then(|_| stdin.flush()).map_err(|_| "Resident runtime input failed".to_string());
                let failed = result.is_err();
                if ack.send(result).is_err() || failed { break; }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let line = match subprocess::read_line_bounded(&mut reader, CAP) {
                    Ok(Some(line)) => Ok(line),
                    Ok(None) => Err("Resident runtime closed its output".into()),
                    Err(error) => Err(error),
                };
                let failed = line.is_err();
                if send.send(line).is_err() || failed { break; }
            }
        });
        // Drain diagnostics with constant memory; private speech and credentials
        // never enter a log or a frontend response.
        std::thread::spawn(move || { let _ = std::io::copy(&mut stderr, &mut std::io::sink()); });
        Ok(Self { engine: engine.into(), program, child, writes, written, replies, tree, next: 0,
            _scratch: scratch, metadata: Value::Null })
    }

    fn request(&mut self, operation: &str, args: &[String], cancel: &AtomicBool) -> Result<Value, String> {
        if self.child.try_wait().map_err(|_| "Could not inspect runtime process")?.is_some() {
            return Err("Resident model process has exited".into());
        }
        self.next = self.next.checked_add(1).ok_or("Runtime request id exhausted")?;
        let mut input = serde_json::to_vec(&json!({"id":self.next,"operation":operation,"args":args})).map_err(|_| "Invalid runtime request")?;
        if input.len() > 32768 { return Err("Runtime request exceeds the bounded argv limit".into()); }
        input.push(b'\n');
        self.writes.try_send(input).map_err(|_| "Resident runtime input queue is unavailable")?;
        let start = Instant::now();
        let deadline = if operation == "run" { Duration::from_secs(600) } else { DEADLINE };
        loop {
            if cancel.load(Ordering::Acquire) { return Err("Resident generation cancelled".into()); }
            if start.elapsed() >= DEADLINE { return Err("Resident runtime input exceeded its deadline".into()); }
            match self.written.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => { result?; break; },
                Err(mpsc::RecvTimeoutError::Timeout) => {},
                Err(_) => return Err("Resident runtime input channel closed".into()),
            }
        }
        let line = loop {
            if cancel.load(Ordering::Acquire) { return Err("Resident generation cancelled".into()); }
            if start.elapsed() >= deadline { return Err("Resident runtime exceeded its deadline".into()); }
            match self.replies.recv_timeout(Duration::from_millis(20)) {
                Ok(line) => break line?,
                Err(mpsc::RecvTimeoutError::Timeout) => {},
                Err(_) => return Err("Resident runtime response channel closed".into()),
            }
        };
        let reply: Value = serde_json::from_str(&line).map_err(|_| "Resident runtime returned invalid JSON")?;
        if reply["id"].as_u64() != Some(self.next) { return Err("Resident runtime returned the wrong request id".into()); }
        if reply["ok"] != true { return Err(redact_secrets(reply["error"].as_str().unwrap_or("Resident runtime operation failed"))); }
        let result = reply["result"].clone();
        let pid = result["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0);
        if result["engine"] != self.engine || !pid.is_some_and(|pid| self.tree.contains_pid(pid))
            || self.metadata["pid"].as_u64().is_some_and(|previous| Some(previous) != result["pid"].as_u64())
            || !result["loaded"].is_boolean() { return Err("Resident runtime ownership receipt is invalid".into()); }
        self.metadata = result.clone();
        if let Some(metadata) = self.metadata.as_object_mut() {
            for field in ["code", "stdout", "stderr"] { metadata.remove(field); }
        }
        Ok(result)
    }
}

pub fn load(repo: &Path, engine: &str) -> Result<Value, String> {
    load_with_execution(repo, engine, None)
}

pub fn load_with_execution(repo: &Path, engine: &str, execution: Option<&Value>) -> Result<Value, String> {
    if !matches!(engine, "kokoro_onnx" | "vibevoice" | "personaplex") { return Err(format!("No model-loading implementation exists for {engine}")); }
    if let Some(value) = execution {
        crate::execution::Execution::parse(value)?;
        if engine == "kokoro_onnx" { return Err("Explicit Kokoro provider selection is not yet configured".into()); }
    }
    let mut resident = slot().try_lock().map_err(|_| "Model owner is busy; retry after the active load or generation finishes")?;
    // Release the previous model before loading another; its RAM cannot be
    // counted as available while both checkpoints overlap.
    *resident = None;
    let mut worker = Resident::spawn(engine, repo, execution)?;
    let result = worker.request("load", &[], &AtomicBool::new(false))?;
    if result["loaded"] == true { *resident = Some(worker); }
    Ok(result)
}

pub fn status() -> Value {
    let Ok(mut resident) = slot().try_lock() else { return json!({"phase":"busy","loaded":null}); };
    if resident.as_mut().is_some_and(|worker| worker.child.try_wait().map_or(true, |status| status.is_some())) { *resident = None; }
    resident.as_ref().map(|worker| worker.metadata.clone()).unwrap_or_else(|| json!({"phase":"unloaded","loaded":false}))
}

pub fn unload() -> Result<Value, String> {
    let mut resident = slot().try_lock().map_err(|_| "Model owner is busy; cancel generation before unloading")?;
    *resident = None;
    Ok(json!({"loaded":false,"phase":"unloaded"}))
}

pub(crate) fn try_run(engine: &str, operation: &str, command: &Command, argv: &[String], cancel: &AtomicBool) -> Result<Option<subprocess::Output>, String> {
    let mut resident = slot().try_lock().map_err(|_| "Model owner is busy; retry after the active operation finishes")?;
    let Some(worker) = resident.as_mut() else { return Ok(None); };
    if worker.engine != engine { return Ok(None); }
    if worker.program != Path::new(command.get_program()) { *resident = None; return Err("Configured Python runtime changed; reload the engine".into()); }
    let result = match worker.request(operation, argv, cancel) {
        Ok(result) => result,
        Err(error) => { *resident = None; return Err(error); }
    };
    let code = result["code"].as_i64().filter(|value| (0..=255).contains(value)).ok_or("Runtime exit code is invalid")?;
    let stdout = result["stdout"].as_str().filter(|value| value.len() <= CAP).ok_or("Runtime speech receipt is invalid")?.as_bytes().to_vec();
    #[cfg(windows)]
    let status = { use std::os::windows::process::ExitStatusExt; std::process::ExitStatus::from_raw(code as u32) };
    #[cfg(unix)]
    let status = { use std::os::unix::process::ExitStatusExt; std::process::ExitStatus::from_raw((code as i32) << 8) };
    Ok(Some(subprocess::Output { status, stdout, stderr: vec![] }))
}
