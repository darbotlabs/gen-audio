//! Path rules for untrusted tool arguments.
//!
//! Writes stay as a single file name inside the process work directory.
//! A scratch directory only opens files this process created, and it refuses
//! symlinks. Tool-supplied repo reads are limited to `examples/` and `voices/`.
//! Python entry points are an exact trusted list, not a caller-chosen path.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

const WRITE_SUFFIXES: &[&str] = &[".wav", ".json", ".png"];
const USER_REPO_PREFIXES: &[&str] = &["examples/", "voices/"];
const TRUSTED_SCRIPTS: &[&str] = &[
    "scripts/improve.py",
    "scripts/spectrogram.py",
    "scripts/cube_revision.py",
    "scripts/compare_wavs.py",
    "scripts/synth_kokoro_onnx.py",
    "scripts/generate.py",
];

/// Process-local scratch. The issued set is how later tool calls name files
/// written earlier. It is not an MCP session id.
pub struct Scratch {
    pub dir: PathBuf,
    issued: Mutex<HashSet<String>>,
    reserved: Mutex<HashSet<String>>,
    private: Mutex<HashSet<String>>,
    preexisting: HashSet<String>,
    /// Set when this scratch created the directory. Dropped with the scratch,
    /// including on panic, so the temp directory does not stay behind.
    cleanup: Option<TempWorkDir>,
}

/// A directory under the temp root. Removed when this value drops, including
/// when the owner panics.
pub struct TempWorkDir {
    path: PathBuf,
}

impl TempWorkDir {
    fn create() -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        // Installed SDK resources are read-only. Scratch belongs to the user,
        // independently of the checkout or the executable's installation path.
        let root = std::env::temp_dir().join("gen-audio-work");
        fs::create_dir_all(&root)?;
        reject_symlink(&root).map_err(std::io::Error::other)?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = root.join(format!("gen-audio-{}-{nanos}-{serial}", std::process::id()));
        fs::create_dir(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        let path = dir.canonicalize()?;
        // A listener thread can outlive every test and be killed without Drop.
        // The exit hook removes any directory still registered.
        remember_work_dir(&path);
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for TempWorkDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkDir {
    fn drop(&mut self) {
        forget_work_dir(&self.path);
        let _ = fs::remove_dir_all(&self.path);
    }
}

static LIVE_WORK_DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn remember_work_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            extern "C" {
                fn atexit(cb: extern "C" fn()) -> i32;
            }
            extern "C" fn reap_work_dirs() {
                let Ok(mut live) = LIVE_WORK_DIRS.lock() else {
                    return;
                };
                for path in live.drain(..) {
                    let _ = fs::remove_dir_all(path);
                }
            }
            let _ = atexit(reap_work_dirs);
        });
    }
    if let Ok(mut live) = LIVE_WORK_DIRS.lock() {
        live.push(path.to_path_buf());
    }
}

fn forget_work_dir(path: &Path) {
    if let Ok(mut live) = LIVE_WORK_DIRS.lock() {
        live.retain(|item| item != path);
    }
}

pub struct SpeechReservation<'a> {
    pub output: PathBuf,
    pub manifest: PathBuf,
    scratch: &'a Scratch,
}

impl std::fmt::Debug for SpeechReservation<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeechReservation").field("output", &self.output).finish()
    }
}

impl Drop for SpeechReservation<'_> {
    fn drop(&mut self) {
        for path in [&self.output, &self.manifest] {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                self.scratch.discard_reserved(name);
            }
        }
    }
}

impl Scratch {
    pub fn create() -> Result<Self, String> {
        let owned = TempWorkDir::create().map_err(|err| err.to_string())?;
        let mut scratch = Self::new(owned.path().to_path_buf())?;
        scratch.cleanup = Some(owned);
        Ok(scratch)
    }

    pub fn new(dir: PathBuf) -> Result<Self, String> {
        reject_symlink(&dir)?;
        let dir = dir
            .canonicalize()
            .map_err(|err| format!("work directory: {err}"))?;
        if !dir.is_dir() {
            return Err("work directory is not a directory".into());
        }
        reject_sensitive_scratch(&dir)?;
        let mut preexisting = HashSet::new();
        {
            for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if let Some(name) = entry.file_name().to_str() {
                    preexisting.insert(name.to_string());
                }
            }
        }
        Ok(Self {
            dir,
            issued: Mutex::new(HashSet::new()),
            reserved: Mutex::new(HashSet::new()),
            private: Mutex::new(HashSet::new()),
            preexisting,
            cleanup: None,
        })
    }

    /// Mark files created after this scratch was opened. Names that were
    /// already in the directory stay unreadable.
    pub fn adopt_new_files(&self) -> Result<(), String> {
        let mut issued = self.issued.lock().map_err(|_| "scratch lock".to_string())?;
        let entries = fs::read_dir(&self.dir).map_err(|err| err.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if self.preexisting.contains(&name) || issued.contains(&name)
                || self.reserved.lock().map_err(|_| "scratch lock")?.contains(&name)
                || self.private.lock().map_err(|_| "scratch lock")?.contains(&name) {
                continue;
            }
            let path = self.dir.join(&name);
            if reject_symlink(&path).is_err() {
                continue;
            }
            if write_name(&self.dir, &name).is_ok() && path.is_file() {
                issued.insert(name);
            }
        }
        Ok(())
    }

    pub fn prepare_output(&self, name: &str) -> Result<PathBuf, String> {
        let path = write_name(&self.dir, name)?;
        reject_symlink(&path)?;
        if self.reserved.lock().map_err(|_| "scratch lock")?.contains(name) {
            return Err("output name is reserved for synthesis".into());
        }
        let mut issued = self.issued.lock().map_err(|_| "scratch lock".to_string())?;
        if path.exists() && !issued.contains(name) {
            return Err("refusing to overwrite a file this process did not create".into());
        }
        issued.insert(name.to_string());
        Ok(path)
    }

    pub fn open_input(&self, name: &str) -> Result<PathBuf, String> {
        let issued = self.issued.lock().map_err(|_| "scratch lock".to_string())?;
        if !issued.contains(name) {
            return Err("refusing to read a file this process did not create".into());
        }
        drop(issued);
        let path = read_name_in_work(&self.dir, name)?;
        reject_symlink(&path)?;
        let canon = path
            .canonicalize()
            .map_err(|err| format!("work file: {err}"))?;
        if !canon.starts_with(&self.dir) {
            return Err("path escaped the work directory".into());
        }
        Ok(canon)
    }

    /// Issue only a validated output, never sibling files merely discovered in
    /// the work directory. Used after synthesis integrity verification.
    pub fn adopt_file(&self, name: &str) -> Result<(), String> {
        let path = read_name_in_work(&self.dir, name)?;
        reject_symlink(&path)?;
        if self.preexisting.contains(name) || self.private.lock().map_err(|_| "scratch lock")?.contains(name) {
            return Err("cannot adopt a preexisting file".into());
        }
        self.issued.lock().map_err(|_| "scratch lock")?.insert(name.into());
        self.reserved.lock().map_err(|_| "scratch lock")?.remove(name);
        Ok(())
    }

    /// Reserve the WAV and its manifest together, without issuing unverified files.
    pub fn reserve_speech(&self, name: &str) -> Result<(PathBuf, PathBuf), String> {
        self.reserve_speech_with_manifest(name, ".json")
    }

    pub fn reserve_speech_guard(&self, name: &str) -> Result<SpeechReservation<'_>, String> {
        let (output, manifest) = self.reserve_speech(name)?;
        Ok(SpeechReservation { output, manifest, scratch: self })
    }

    pub(crate) fn reserve_speech_with_manifest(&self, name: &str, suffix: &str) -> Result<(PathBuf, PathBuf), String> {
        if !matches!(suffix, ".json" | ".synth.json") {
            return Err("unsupported speech manifest suffix".into());
        }
        if !name.ends_with(".wav") {
            return Err("speech output must end in .wav".into());
        }
        let wav = write_name(&self.dir, name)?;
        let manifest_name = format!("{}{suffix}", name.strip_suffix(".wav").unwrap());
        let manifest = write_name(&self.dir, &manifest_name)?;
        let mut reserved = self.reserved.lock().map_err(|_| "scratch lock")?;
        for (name, path) in [(name, &wav), (manifest_name.as_str(), &manifest)] {
            reject_symlink(path)?;
            if path.exists() || reserved.contains(name) || self.preexisting.contains(name) {
                return Err("speech output must have a unique, unused name".into());
            }
        }
        reserved.insert(name.into());
        reserved.insert(manifest_name);
        Ok((wav, manifest))
    }

    /// Failed model-worker publications must never become readable via a later
    /// directory-wide adoption. Keep the reservation if removal fails.
    pub(crate) fn discard_reserved(&self, name: &str) {
        let path = match write_name(&self.dir, name) {
            Ok(path) => path,
            Err(_) => {
                eprintln!("speech reservation cleanup rejected an invalid name");
                return;
            }
        };
        let mut reserved = match self.reserved.lock() {
            Ok(reserved) => reserved,
            Err(_) => {
                eprintln!("speech reservation cleanup could not acquire its lock");
                return;
            }
        };
        if reserved.contains(name) {
            match fs::remove_file(path) {
                Ok(()) => { reserved.remove(name); }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => { reserved.remove(name); }
                Err(error) => {
                    eprintln!("speech reservation cleanup failed; output remains inaccessible: {}", error.kind());
                }
            }
        }
    }

    pub fn unique_name(&self, suffix: &str) -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        format!("speech-{}-{}{suffix}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }

    /// Internal inputs are create-new, bounded and deliberately never issued
    /// through the public file capability set.
    pub fn write_private_input(&self, bytes: &[u8], suffix: &str) -> Result<PathBuf, String> {
        if bytes.len() > 256 * 1024 || !matches!(suffix, ".txt" | ".json") {
            return Err("private input exceeds limit or has an invalid suffix".into());
        }
        let path = self.dir.join(self.unique_name(suffix));
        self.private.lock().map_err(|_| "scratch lock")?
            .insert(path.file_name().unwrap().to_string_lossy().into_owned());
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|e| format!("private input: {e}"))?;
        if let Err(e) = file.write_all(bytes) {
            drop(file);
            fs::remove_file(&path).map_err(|cleanup| format!("private input cleanup: {cleanup}"))?;
            return Err(format!("private input write: {e}"));
        }
        Ok(path)
    }
}

fn reject_sensitive_scratch(dir: &Path) -> Result<(), String> {
    if let Some(repo) = find_repo_root() {
        let generated = repo.join("target").join("gen-audio-work");
        let is_generated_child = dir.parent() == Some(generated.as_path());
        if (dir.starts_with(&repo) && !is_generated_child) || repo.starts_with(dir) {
            return Err("work directory must not contain or sit inside the repository".into());
        }
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        if dir == Path::new(&home) {
            return Err("work directory must not be the home directory".into());
        }
    }
    if dir == std::env::temp_dir() {
        return Err("work directory must be a dedicated directory, not the temp root".into());
    }
    Ok(())
}

pub fn make_work_dir() -> std::io::Result<TempWorkDir> {
    TempWorkDir::create()
}

// Tests point `mcp.addr` at their own file without mutating the process
// environment. The signal handler and every other thread keep the real path:
// there is one addr file per process, and a signal can arrive on any thread.
#[cfg(test)]
thread_local! {
    static MCP_ADDR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
struct McpAddrFileGuard {
    previous: Option<PathBuf>,
}

#[cfg(test)]
impl Drop for McpAddrFileGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        MCP_ADDR_OVERRIDE.with(|slot| *slot.borrow_mut() = previous);
    }
}

#[cfg(test)]
fn bind_mcp_addr_file(path: PathBuf) -> McpAddrFileGuard {
    MCP_ADDR_OVERRIDE.with(|slot| {
        let previous = slot.borrow_mut().replace(path);
        McpAddrFileGuard { previous }
    })
}

/// Loopback address the desktop MCP listener bound, one line, no secrets.
pub fn mcp_addr_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = MCP_ADDR_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return path;
    }
    if let Some(path) = std::env::var_os("GEN_AUDIO_MCP_ADDR_FILE") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("gen-audio").join("mcp.addr")
}

pub fn write_mcp_addr(addr: &str) -> Result<(), String> {
    let path = mcp_addr_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    arm_mcp_addr_signals();
    fs::write(&path, format!("{addr}\n{}\n", std::process::id())).map_err(|err| err.to_string())
}

/// The one line written when `mcp.addr` cannot be saved.
pub fn mcp_addr_write_failure_line(err: &str) -> String {
    format!("gen-audio-mcp: mcp.addr write failed: {err}")
}

/// Write `mcp.addr` and log when it fails. Callers must use this instead of
/// discarding `write_mcp_addr`: a silent failure leaves the desktop and the
/// listener disagreeing about the port.
pub fn publish_mcp_addr(addr: &str) -> Result<(), String> {
    match write_mcp_addr(addr) {
        Ok(()) => Ok(()),
        Err(err) => {
            // fd 2, not `eprintln`: libtest captures the macro, so a parent
            // waiting on this process would not see the failure.
            log_stderr(&mcp_addr_write_failure_line(&err));
            Err(err)
        }
    }
}

fn log_stderr(line: &str) {
    #[cfg(unix)]
    let text = format!("{line}\n");
    #[cfg(unix)]
    {
        let _ = unsafe { write(2, text.as_ptr() as *const std::ffi::c_void, text.len()) };
    }
    #[cfg(not(unix))]
    {
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }
}

#[cfg(unix)]
extern "C" {
    fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
}

/// Remove `mcp.addr` when this process receives SIGINT or SIGTERM.
/// [`reap_stale_mcp_addr`] still drops a file whose pid is already dead,
/// which covers SIGKILL and a handler that never ran.
pub fn arm_mcp_addr_signals() {
    #[cfg(unix)]
    {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            signal(2, on_mcp_stop as *const std::ffi::c_void);
            signal(15, on_mcp_stop as *const std::ffi::c_void);
        });
    }
}

#[cfg(unix)]
extern "C" {
    fn signal(sig: i32, handler: *const std::ffi::c_void) -> *const std::ffi::c_void;
}

#[cfg(unix)]
extern "C" fn on_mcp_stop(_sig: i32) {
    let _ = fs::remove_file(mcp_addr_path());
    std::process::exit(0);
}

pub fn delete_mcp_addr() {
    let _ = fs::remove_file(mcp_addr_path());
}

pub fn read_mcp_addr() -> Option<String> {
    let text = fs::read_to_string(mcp_addr_path()).ok()?;
    let addr = text.lines().next().unwrap_or("").trim();
    if addr.is_empty() {
        None
    } else {
        Some(addr.to_string())
    }
}

/// Drop `mcp.addr` when the recorded pid is dead or the port is not accepting.
/// A legacy one-line file is kept while its port still accepts connections.
pub fn reap_stale_mcp_addr() {
    let path = mcp_addr_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let mut lines = text.lines();
    let Some(addr) = lines.next().map(str::trim).filter(|line| !line.is_empty()) else {
        let _ = fs::remove_file(&path);
        return;
    };
    let pid = lines
        .next()
        .and_then(|line| line.trim().parse::<u32>().ok());
    let stale = match pid {
        Some(pid) => !pid_alive(pid) || !port_accepts(addr),
        None => !port_accepts(addr),
    };
    if stale {
        let _ = fs::remove_file(&path);
    }
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn port_accepts(addr: &str) -> bool {
    let Ok(socket) = addr.parse::<std::net::SocketAddr>() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&socket, std::time::Duration::from_millis(200)).is_ok()
}

pub fn find_repo_root() -> Option<PathBuf> {
    if let Some(raw) = crate::runtime_config::setting("GEN_AUDIO_REPO") {
        let path = PathBuf::from(raw);
        if is_repo(&path) {
            return path.canonicalize().ok();
        }
    }
    let mut cur = std::env::current_dir().ok()?;
    loop {
        if is_repo(&cur) {
            return cur.canonicalize().ok();
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn is_repo(path: &Path) -> bool {
    path.join("pyproject.toml").is_file() && path.join("src").join("gen_audio").is_dir()
}

pub fn write_name(work: &Path, name: &str) -> Result<PathBuf, String> {
    let file_name = single_component(name)?;
    if !WRITE_SUFFIXES
        .iter()
        .any(|suffix| file_name.ends_with(suffix))
    {
        return Err("output name must end in .wav, .json, or .png".into());
    }
    let root = work
        .canonicalize()
        .map_err(|err| format!("work directory: {err}"))?;
    let path = root.join(&file_name);
    if path.parent().and_then(|p| p.canonicalize().ok()).as_deref() != Some(root.as_path()) {
        return Err("output escaped the work directory".into());
    }
    Ok(path)
}

pub fn read_name_in_work(work: &Path, name: &str) -> Result<PathBuf, String> {
    let path = write_name(work, name)?;
    if !path.is_file() {
        return Err(format!("file is not in the work directory: {name}"));
    }
    Ok(path)
}

pub fn read_user_repo_file(repo: &Path, raw: &str) -> Result<PathBuf, String> {
    let normalized = raw.replace('\\', "/");
    if !USER_REPO_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return Err("tool arguments may only read files under examples/ or voices/".into());
    }
    if normalized
        .split('/')
        .any(|part| part.is_empty() || part.starts_with('.'))
    {
        return Err("hidden or empty path components are not accepted".into());
    }
    let path = read_repo_relative(repo, &normalized)?;
    let meta = fs::metadata(&path).map_err(|err| err.to_string())?;
    if meta.len() > 256 * 1024 {
        return Err("repo file is larger than 256 KiB".into());
    }
    Ok(path)
}

pub fn read_trusted_script(repo: &Path, rel: &str) -> Result<PathBuf, String> {
    if !TRUSTED_SCRIPTS.contains(&rel) {
        return Err("script is not in the trusted list".into());
    }
    let script = read_repo_relative(repo, rel)?;
    if script != repo.canonicalize().map_err(|e| e.to_string())?.join(rel) {
        return Err("trusted entry point must not be redirected".into());
    }
    Ok(script)
}

pub fn read_repo_relative(repo: &Path, raw: &str) -> Result<PathBuf, String> {
    let root = repo.canonicalize().map_err(|err| format!("repo: {err}"))?;
    let joined = push_relative(&root, raw)?;
    let canon = joined
        .canonicalize()
        .map_err(|_| format!("repo path does not exist: {raw}"))?;
    if !canon.starts_with(&root) {
        return Err("repo path escapes the repository".into());
    }
    reject_quarantine(&canon)?;
    if !canon.is_file() {
        return Err(format!("not a file: {raw}"));
    }
    Ok(canon)
}

fn push_relative(base: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.contains('\0') {
        return Err("path is empty".into());
    }
    let rel = Path::new(raw);
    if rel.is_absolute() || is_foreign_absolute(raw) {
        return Err("absolute paths are not accepted from tool arguments".into());
    }
    let mut out = base.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("path must not contain ..".into()),
            _ => return Err("unsupported path component".into()),
        }
    }
    Ok(out)
}

fn is_foreign_absolute(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.starts_with("\\\\") || trimmed.starts_with("//") {
        return true;
    }
    let bytes = trimmed.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

pub fn reject_quarantine(path: &Path) -> Result<(), String> {
    if path.components().any(|part| part.as_os_str().to_string_lossy().to_ascii_lowercase().contains("quarantine")) {
        return Err("quarantined files must not be used".into());
    }
    Ok(())
}

pub fn read_bounded(path: &Path, cap: usize) -> Result<Vec<u8>, String> {
    reject_symlink(path)?;
    if !path.is_file() {
        return Err("expected a regular file".into());
    }
    let mut data = Vec::new();
    fs::File::open(path).map_err(|_| "cannot open bounded file")?
        .take(cap as u64 + 1).read_to_end(&mut data).map_err(|_| "cannot read bounded file")?;
    if data.len() > cap { return Err("file exceeds byte limit".into()); }
    Ok(data)
}

pub(crate) fn reject_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("refusing to follow a symlink in the work directory".into())
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("cannot inspect file metadata".into()),
    }
}

fn single_component(name: &str) -> Result<String, String> {
    if name.is_empty() || name.len() > 180
        || name.contains('\0')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return Err("file name must be a single path component".into());
    }
    if name.starts_with('.') {
        return Err("file name must not be hidden or relative".into());
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9')) {
        return Err("reserved device file names are not accepted".into());
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err("file name has unsupported characters".into());
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_work_is_writable_outside_installed_resources_and_is_reaped() {
        let work = make_work_dir().unwrap();
        let path = work.path().to_path_buf();
        assert!(path.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        if let Some(repo) = find_repo_root() { assert!(!path.starts_with(repo)); }
        fs::write(path.join("probe.txt"), b"private runtime probe").unwrap();
        drop(work);
        assert!(!path.exists());
    }

    /// TCP port held without `listen()`. `connect` is refused immediately, and
    /// another process cannot bind the same address. Non-Linux uses TEST-NET-1
    /// (RFC 5737), which is not a host that accepts connections.
    struct RefusingPort {
        addr: std::net::SocketAddr,
        #[cfg(target_os = "linux")]
        fd: i32,
    }

    #[cfg(target_os = "linux")]
    #[repr(C)]
    struct SockAddrIn {
        sin_family: u16,
        sin_port: u16,
        sin_addr: u32,
        sin_zero: [u8; 8],
    }

    #[cfg(target_os = "linux")]
    extern "C" {
        fn socket(domain: i32, ty: i32, protocol: i32) -> i32;
        fn bind(fd: i32, addr: *const SockAddrIn, len: u32) -> i32;
        fn getsockname(fd: i32, addr: *mut SockAddrIn, len: *mut u32) -> i32;
        fn close(fd: i32) -> i32;
    }

    impl RefusingPort {
        fn bind() -> Self {
            #[cfg(target_os = "linux")]
            {
                // AF_INET, SOCK_STREAM, IPPROTO_TCP. Bind only — no listen().
                let fd = unsafe { socket(2, 1, 6) };
                assert!(fd >= 0, "socket");
                let mut raw = SockAddrIn {
                    sin_family: 2,
                    sin_port: 0,
                    sin_addr: u32::from_be(0x7f00_0001),
                    sin_zero: [0; 8],
                };
                let rc = unsafe { self::bind(fd, &raw, 16) };
                assert_eq!(rc, 0, "bind");
                let mut len: u32 = 16;
                let rc = unsafe { getsockname(fd, &mut raw, &mut len) };
                assert_eq!(rc, 0, "getsockname");
                let port = u16::from_be(raw.sin_port);
                assert_ne!(port, 0, "kernel assigned a port");
                Self {
                    addr: std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                    fd,
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                Self {
                    addr: "192.0.2.1:9".parse().unwrap(),
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for RefusingPort {
        fn drop(&mut self) {
            unsafe { close(self.fd) };
        }
    }

    #[test]
    fn work_dir_is_removed_when_its_owner_drops_or_panics() {
        let scratch = Scratch::create().unwrap();
        let dir = scratch.dir.clone();
        assert!(dir.exists());
        drop(scratch);
        assert!(!dir.exists(), "a dropped scratch left {dir:?}");

        let probe = std::sync::Mutex::new(PathBuf::new());
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let scratch = Scratch::create().unwrap();
            *probe.lock().unwrap() = scratch.dir.clone();
            panic!("owner panicked");
        }));
        assert!(caught.is_err());
        let dir = probe.lock().unwrap().clone();
        assert!(!dir.exists(), "a panicked scratch left {dir:?}");
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        let work = make_work_dir().unwrap();
        assert!(write_name(&work, "../x.wav").is_err());
        assert!(write_name(&work, "a/b.wav").is_err());
        assert!(write_name(&work, "ok.wav").is_ok());
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let repo = repo.canonicalize().unwrap();
        assert!(read_repo_relative(&repo, "examples/podcast_script_sample.txt").is_ok());
        assert!(read_repo_relative(&repo, "../Cargo.toml").is_err());
        assert!(read_repo_relative(&repo, "/etc/passwd").is_err());
        assert!(read_user_repo_file(&repo, "examples/podcast_script_sample.txt").is_ok());
        assert!(read_user_repo_file(&repo, "voices/cast_map.example.json").is_ok());
        assert!(read_user_repo_file(&repo, "README.md").is_err());
        assert!(read_user_repo_file(&repo, "examples/../README.md").is_err());
        assert!(read_user_repo_file(&repo, r"C:\Windows\system.ini").is_err());
        assert!(read_user_repo_file(&repo, r"\\server\share\script.txt").is_err());
        assert!(write_name(&work, r"C:\out.wav").is_err());
        assert!(read_trusted_script(&repo, "scripts/improve.py").is_ok());
        assert!(read_trusted_script(&repo, "scripts/not-real.py").is_err());
        let addr_file = std::env::temp_dir().join(format!(
            "gen-audio-mcp-addr-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _addr = bind_mcp_addr_file(addr_file.clone());
        fs::write(&addr_file, "127.0.0.1:9\n4294967294\n").unwrap();
        reap_stale_mcp_addr();
        assert!(!addr_file.exists(), "a dead pid must remove mcp.addr");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let live = listener.local_addr().unwrap().to_string();
        fs::write(&addr_file, format!("{live}\n")).unwrap();
        reap_stale_mcp_addr();
        assert!(addr_file.exists(), "a legacy file with a live port stays");
        drop(listener);
        // Closing the listener and expecting the port to stay free races with
        // any other process that binds it before the connect check. Hold a
        // socket that is bound but not listening: connect is refused, and
        // the port cannot be taken.
        let dead = RefusingPort::bind();
        fs::write(&addr_file, format!("{}\n", dead.addr)).unwrap();
        reap_stale_mcp_addr();
        assert!(
            !addr_file.exists(),
            "a legacy file with a dead port is removed"
        );
        assert!(
            !port_accepts(&dead.addr.to_string()),
            "the held port still refuses connections"
        );
        drop(_addr);
        let scratch = Scratch::new(work.path().to_path_buf()).unwrap();
        fs::write(work.join("secret.json"), b"{\"token\":\"sekret\"}").unwrap();
        assert!(scratch.open_input("secret.json").is_err());
        scratch.prepare_output("out.wav").unwrap();
        fs::write(work.join("out.wav"), b"RIFFdemo").unwrap();
        assert!(scratch.open_input("out.wav").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn sigterm_and_sigint_remove_mcp_addr() {
        for (signal, name) in [(15, "TERM"), (2, "INT")] {
            let path = std::env::temp_dir().join(format!(
                "gen-audio-mcp-addr-signal-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0)
            ));
            let _ = fs::remove_file(&path);
            let exe = std::env::current_exe().expect("test exe");
            let mut child = std::process::Command::new(&exe)
                .arg("mcp_addr_sleeps_until_signalled")
                .arg("--test-threads=1")
                .env("GEN_AUDIO_MCP_ADDR_HOLD", "1")
                .env("GEN_AUDIO_MCP_ADDR_FILE", &path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn addr holder");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !path.exists() {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{name}: mcp.addr was not written");
                }
                if child.try_wait().ok().flatten().is_some() {
                    panic!("{name}: holder exited before writing mcp.addr");
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            let killed = std::process::Command::new("kill")
                .args([&format!("-{signal}"), &child.id().to_string()])
                .status()
                .expect("kill");
            assert!(killed.success(), "{name}: kill failed");
            let started = std::time::Instant::now();
            loop {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                if started.elapsed() > std::time::Duration::from_secs(5) {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{name}: process ignored the signal");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert!(!path.exists(), "{name} must remove mcp.addr");
        }
    }

    #[test]
    fn mcp_addr_sleeps_until_signalled() {
        if std::env::var("GEN_AUDIO_MCP_ADDR_HOLD").ok().as_deref() != Some("1") {
            return;
        }
        write_mcp_addr("127.0.0.1:9").expect("write mcp.addr");
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[test]
    fn mcp_addr_write_error_is_logged() {
        if std::env::var("GEN_AUDIO_MCP_ADDR_LOG").ok().as_deref() == Some("1") {
            let _ = publish_mcp_addr("127.0.0.1:9");
            return;
        }
        let blocker = std::env::temp_dir().join(format!(
            "gen-audio-mcp-addr-blocker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::write(&blocker, b"not-a-directory").unwrap();
        let target = blocker.join("mcp.addr");
        let output = std::process::Command::new(std::env::current_exe().expect("test exe"))
            .arg("mcp_addr_write_error_is_logged")
            .arg("--test-threads=1")
            .env("GEN_AUDIO_MCP_ADDR_LOG", "1")
            .env("GEN_AUDIO_MCP_ADDR_FILE", &target)
            .output()
            .expect("spawn addr writer");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("gen-audio-mcp: mcp.addr write failed:"),
            "stderr did not report the write failure:\n{stderr}"
        );
        let _ = fs::remove_file(&blocker);
    }

    #[test]
    fn speech_reservations_and_private_inputs_are_not_public() {
        let scratch = Scratch::create().unwrap();
        let (wav, manifest) = scratch.reserve_speech("fresh.wav").unwrap();
        assert!(scratch.reserve_speech("fresh.wav").is_err());
        assert!(scratch.reserve_speech("../escape.wav").is_err());
        assert!(scratch.reserve_speech("bad.json").is_err());
        fs::write(&wav, b"test").unwrap();
        fs::write(&manifest, b"{}").unwrap();
        let input = scratch.write_private_input(b"{}", ".json").unwrap();
        assert!(scratch.write_private_input(&vec![0; 256 * 1024 + 1], ".txt").is_err());
        scratch.adopt_new_files().unwrap();
        assert!(scratch.open_input("fresh.wav").is_err());
        assert!(scratch.open_input(input.file_name().unwrap().to_str().unwrap()).is_err());
        scratch.adopt_file("fresh.wav").unwrap();
        assert!(scratch.open_input("fresh.wav").is_ok());
        assert!(scratch.reserve_speech("fresh.wav").is_err());
        fs::remove_dir_all(&scratch.dir).unwrap();
    }
}
