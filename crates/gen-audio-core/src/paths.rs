//! Path rules for untrusted tool arguments.
//!
//! Writes stay as a single file name inside the process work directory.
//! A scratch directory only opens files this process created, and it refuses
//! symlinks. Tool-supplied repo reads are limited to `examples/` and `voices/`.
//! Python entry points are an exact trusted list, not a caller-chosen path.

use std::collections::HashSet;
use std::fs;
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
        let dir = std::env::temp_dir().join(format!("gen-audio-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir)?;
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

impl Scratch {
    pub fn create() -> Result<Self, String> {
        let owned = TempWorkDir::create().map_err(|err| err.to_string())?;
        let mut scratch = Self::new(owned.path().to_path_buf())?;
        scratch.cleanup = Some(owned);
        Ok(scratch)
    }

    pub fn new(dir: PathBuf) -> Result<Self, String> {
        let dir = dir
            .canonicalize()
            .map_err(|err| format!("work directory: {err}"))?;
        if !dir.is_dir() {
            return Err("work directory is not a directory".into());
        }
        reject_sensitive_scratch(&dir)?;
        let mut preexisting = HashSet::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    preexisting.insert(name.to_string());
                }
            }
        }
        Ok(Self {
            dir,
            issued: Mutex::new(HashSet::new()),
            preexisting,
            cleanup: None,
        })
    }

    /// Mark files created after this scratch was opened. Names that were
    /// already in the directory stay unreadable.
    pub fn adopt_new_files(&self) -> Result<(), String> {
        let mut issued = self.issued.lock().map_err(|_| "scratch lock".to_string())?;
        let entries = fs::read_dir(&self.dir).map_err(|err| err.to_string())?;
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if self.preexisting.contains(&name) || issued.contains(&name) {
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
}

fn reject_sensitive_scratch(dir: &Path) -> Result<(), String> {
    if let Some(repo) = find_repo_root() {
        if dir.starts_with(&repo) || repo.starts_with(dir) {
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
            eprintln!("{}", mcp_addr_write_failure_line(&err));
            Err(err)
        }
    }
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
    if let Ok(raw) = std::env::var("GEN_AUDIO_REPO") {
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
    read_repo_relative(repo, rel)
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

fn reject_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("refusing to follow a symlink in the work directory".into())
        }
        _ => Ok(()),
    }
}

fn single_component(name: &str) -> Result<String, String> {
    if name.is_empty()
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
        reap_stale_mcp_addr();
        assert!(
            !addr_file.exists(),
            "a legacy file with a dead port is removed"
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
        let _addr = bind_mcp_addr_file(target);
        let err = publish_mcp_addr("127.0.0.1:9").expect_err("parent is a file, not a directory");
        let line = mcp_addr_write_failure_line(&err);
        assert!(line.starts_with("gen-audio-mcp: mcp.addr write failed:"), "{line}");
        assert!(line.contains(&err), "{line}");
        drop(_addr);
        let _ = fs::remove_file(&blocker);
    }
}
