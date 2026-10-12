//! Bounded, shell-free child execution shared by local adapters.

use std::io::{BufRead, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct Limits {
    pub deadline: Duration,
    pub stdout: usize,
    pub stderr: usize,
    pub stdin: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(120),
            stdout: 256 * 1024,
            stderr: 64 * 1024,
            stdin: 256 * 1024,
        }
    }
}

pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
const CAPACITY: usize = 4;
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn run(command: &mut Command, input: &[u8], limits: Limits) -> Result<Output, String> {
    run_cancellable(command, input, limits, &AtomicBool::new(false))
}

pub fn run_cancellable(command: &mut Command, input: &[u8], limits: Limits, cancel: &AtomicBool) -> Result<Output, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("subprocess cancelled".into());
    }
    if input.len() > limits.stdin {
        return Err("subprocess stdin limit exceeded".into());
    }
    ACTIVE
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < CAPACITY).then_some(n + 1))
        .map_err(|_| "subprocess capacity exhausted".to_string())?;
    let _permit = Permit;
    command.env_remove("GEN_AUDIO_MCP_TOKEN").env_remove("GEN_AUDIO_MCP_BOOT_NONCE");
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    tree::configure(command);
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| format!("subprocess failed to start: {e}"))?;
    let tree = match tree::Tree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let (send, receive) = mpsc::channel::<Result<(u8, Vec<u8>), String>>();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut stdin = child.stdin.take().expect("piped stdin");
    #[cfg(unix)]
    if let Err(error) = tree::nonblocking(&stdout)
        .and_then(|_| tree::nonblocking(&stderr))
        .and_then(|_| tree::nonblocking(&stdin))
    {
        tree.terminate();
        let _ = child.wait();
        return Err(format!("subprocess pipe setup failed: {error}"));
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let reader = |mut pipe: Box<dyn Read + Send>, cap: usize, id: u8, send: mpsc::Sender<_>, cancelled: Arc<AtomicBool>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut block = [0u8; 8192];
            let result = loop {
                if cancelled.load(Ordering::Acquire) {
                    break Err("subprocess pipe cancelled".into());
                }
                match pipe.read(&mut block) {
                    Ok(0) => break Ok((id, bytes)),
                    Ok(n) => {
                        if n > cap.saturating_sub(bytes.len()) {
                            break Err(format!("subprocess {} limit exceeded", if id == 1 { "stdout" } else { "stderr" }));
                        }
                        bytes.extend_from_slice(&block[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break Err("subprocess output read failed".into()),
                }
            };
            let _ = send.send(result);
        })
    };
    let out_thread = reader(Box::new(stdout), limits.stdout, 1, send.clone(), Arc::clone(&cancelled));
    let err_thread = reader(Box::new(stderr), limits.stderr, 2, send.clone(), Arc::clone(&cancelled));
    let input = input.to_vec();
    let writer_cancelled = Arc::clone(&cancelled);
    let in_thread = std::thread::spawn(move || {
        let mut remaining = input.as_slice();
        let result = loop {
            if remaining.is_empty() {
                break Ok((0, Vec::new()));
            }
            if writer_cancelled.load(Ordering::Acquire) {
                break Err("subprocess stdin cancelled".to_string());
            }
            match stdin.write(remaining) {
                Ok(0) => break Err("subprocess stdin closed".to_string()),
                Ok(n) => remaining = &remaining[n..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break Err("subprocess stdin write failed".to_string()),
            }
        };
        drop(stdin); // EOF is required by CLI adapters that consume stdin.
        let _ = send.send(result);
    });
    let mut stdout = None;
    let mut stderr = None;
    let mut wrote = false;
    let mut status = None;
    let result = loop {
        if cancel.load(Ordering::Acquire) {
            break Err("subprocess cancelled".into());
        }
        if start.elapsed() >= limits.deadline {
            break Err("subprocess deadline exceeded".into());
        }
        match receive.recv_timeout(Duration::from_millis(10)) {
            Ok(Ok((0, _))) => wrote = true,
            Ok(Ok((1, bytes))) => stdout = Some(bytes),
            Ok(Ok((2, bytes))) => stderr = Some(bytes),
            Ok(Err(error)) => break Err(error),
            _ => {}
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(found) => status = found,
                Err(_) => break Err("subprocess wait failed".into()),
            }
        }
        if let (Some(status), Some(stdout), Some(stderr), true) = (status, stdout.as_ref(), stderr.as_ref(), wrote) {
            break Ok(Output { status, stdout: stdout.clone(), stderr: stderr.clone() });
        }
    };
    // Kill the process group/job even after the direct child exits: descendants
    // must not retain the pipes or outlive a bounded tool call.
    cancelled.store(true, Ordering::Release);
    tree.terminate();
    if result.is_err() {
        let _ = child.kill();
    }
    let reaped = child.wait();
    for thread in [out_thread, err_thread, in_thread] {
        if thread.join().is_err() {
            return Err("subprocess pipe worker failed".into());
        }
    }
    reaped.map_err(|_| "subprocess reap failed".to_string())?;
    result
}

/// Stop the connection on an oversized line instead of allocating/draining an
/// unbounded stream from an untrusted peer.
pub fn read_line_bounded(reader: &mut impl BufRead, cap: usize) -> Result<Option<String>, String> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|_| "request read failed")?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let n = available.iter().position(|b| *b == b'\n').map_or(available.len(), |n| n + 1);
        if n > cap.saturating_sub(line.len()) {
            return Err("request line exceeds byte limit".into());
        }
        let done = available[n - 1] == b'\n';
        line.extend_from_slice(&available[..n]);
        reader.consume(n);
        if done { break; }
    }
    String::from_utf8(line).map(Some).map_err(|_| "request is not UTF-8".into())
}

pub fn valid_rpc_id(id: &serde_json::Value) -> bool {
    match id {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control),
        serde_json::Value::Number(n) => n.as_i64().is_some_and(|n| n.unsigned_abs() <= 9_007_199_254_740_991),
        _ => false,
    }
}

#[cfg(unix)]
pub(crate) mod tree {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    unsafe extern "C" {
        fn setpgid(pid: i32, pgid: i32) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
        fn getpgid(pid: i32) -> i32;
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    pub fn nonblocking(pipe: &impl AsRawFd) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        const NONBLOCK: i32 = 0x800;
        #[cfg(not(target_os = "linux"))]
        const NONBLOCK: i32 = 0x4;
        let fd = pipe.as_raw_fd();
        let flags = unsafe { fcntl(fd, 3) }; // F_GETFL
        if flags < 0 || unsafe { fcntl(fd, 4, flags | NONBLOCK) } < 0 { // F_SETFL
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn configure(command: &mut std::process::Command) {
        unsafe {
            command.pre_exec(|| {
                if setpgid(0, 0) != 0 { return Err(std::io::Error::last_os_error()); }
                Ok(())
            });
        }
    }
    pub struct Tree(i32);
    impl Tree {
        pub fn attach(child: &std::process::Child) -> Result<Self, String> { Ok(Self(child.id() as i32)) }
        pub fn terminate(&self) { unsafe { kill(-self.0, 9); } }
        pub fn contains_pid(&self, pid: u32) -> bool { pid <= i32::MAX as u32 && unsafe { getpgid(pid as i32) } == self.0 }
    }
}

#[cfg(windows)]
pub(crate) mod tree {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    type Handle = *mut c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn SetInformationJobObject(job: Handle, class: i32, information: *const c_void, length: u32) -> i32;
        fn QueryInformationJobObject(job: Handle, class: i32, information: *mut c_void, length: u32, returned: *mut u32) -> i32;
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtResumeProcess(process: Handle) -> i32;
    }
    pub fn configure(command: &mut std::process::Command) {
        // Assign the job before any child code can start descendants.
        command.creation_flags(0x08000004); // CREATE_NO_WINDOW | CREATE_SUSPENDED
    }
    pub struct Tree(Handle);
    // Kernel job handles have no thread affinity; ownership remains exclusive.
    unsafe impl Send for Tree {}
    #[repr(C)]
    struct BasicLimits {
        process_time: i64, job_time: i64, flags: u32, minimum_working_set: usize,
        maximum_working_set: usize, active_processes: u32, affinity: usize,
        priority: u32, scheduling: u32,
    }
    #[repr(C)]
    struct ExtendedLimits {
        basic: BasicLimits, io: [u64; 6], process_memory: usize, job_memory: usize,
        peak_process_memory: usize, peak_job_memory: usize,
    }
    #[repr(C)]
    struct ProcessList { assigned: u32, count: u32, pids: [usize; 64] }
    impl Tree {
        pub fn attach(child: &std::process::Child) -> Result<Self, String> {
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() { return Err("could not create subprocess job".into()); }
            let tree = Self(job);
            let mut limits: ExtendedLimits = unsafe { std::mem::zeroed() };
            limits.basic.flags = 0x00002000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            if unsafe { SetInformationJobObject(job, 9, &limits as *const _ as *const c_void, std::mem::size_of::<ExtendedLimits>() as u32) } == 0 {
                return Err("could not configure subprocess lifetime containment".into());
            }
            if unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0 {
                return Err("could not contain subprocess in job".into());
            }
            if unsafe { NtResumeProcess(child.as_raw_handle()) } < 0 {
                return Err("could not resume contained subprocess".into());
            }
            Ok(tree)
        }
        pub fn terminate(&self) { unsafe { TerminateJobObject(self.0, 1); } }
        pub fn contains_pid(&self, pid: u32) -> bool {
            let mut processes: ProcessList = unsafe { std::mem::zeroed() };
            if unsafe { QueryInformationJobObject(self.0, 3, &mut processes as *mut _ as *mut c_void, std::mem::size_of::<ProcessList>() as u32, std::ptr::null_mut()) } == 0 { return false; }
            processes.pids[..(processes.count as usize).min(64)].contains(&(pid as usize))
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            self.terminate();
            unsafe { CloseHandle(self.0); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_RUNNER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    #[ignore]
    fn child_helper() {
        match std::env::var("GEN_AUDIO_RUNNER_TEST").unwrap().as_str() {
            "wait" => std::thread::sleep(Duration::from_secs(30)),
            "descendant" => {
                let _descendant = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "subprocess::tests::child_helper", "--ignored", "--nocapture"])
                    .env("GEN_AUDIO_RUNNER_TEST", "wait").spawn().unwrap();
                std::process::exit(0);
            }
            #[cfg(unix)]
            "escaped" => {
                use std::os::unix::process::CommandExt;
                let _descendant = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "subprocess::tests::child_helper", "--ignored", "--nocapture"])
                    .env("GEN_AUDIO_RUNNER_TEST", "brief-wait")
                    .process_group(0).spawn().unwrap();
                std::process::exit(0);
            }
            "brief-wait" => std::thread::sleep(Duration::from_secs(2)),
            "flood" => { let _ = std::io::stdout().write_all(&vec![b'x'; 1_000_000]); }
            "stderr" => { let _ = std::io::stderr().write_all(&vec![b'x'; 1_000_000]); }
            "exit" => std::process::exit(7),
            "env" => {
                assert!(std::env::var_os("GEN_AUDIO_MCP_TOKEN").is_none());
                assert!(std::env::var_os("GEN_AUDIO_MCP_BOOT_NONCE").is_none());
            }
            _ => {
                let mut data = String::new();
                std::io::stdin().read_to_string(&mut data).unwrap();
                println!("EOF:{data}");
            }
        }
    }

    #[test]
    fn deadlines_limits_stdin_and_status() {
        let _serial = TEST_RUNNER.lock().unwrap();
        for (mode, error) in [("wait", Some("deadline")), ("descendant", Some("deadline")), ("flood", Some("stdout")), ("stderr", Some("stderr")), ("echo", None), ("exit", None)] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "subprocess::tests::child_helper", "--ignored", "--nocapture"])
                .env("GEN_AUDIO_RUNNER_TEST", mode);
            let result = run(&mut command, b"hello", Limits {
                deadline: Duration::from_secs(2), stdout: 4096, stderr: 4096, stdin: 100,
            });
            if let Some(error) = error {
                let reason = result.err().unwrap();
                assert!(reason.contains(error), "{mode}: {reason}");
            } else {
                let output = result.unwrap();
                if mode == "echo" {
                    assert!(output.status.success());
                    assert!(String::from_utf8_lossy(&output.stdout).contains("EOF:hello"));
                } else {
                    assert_eq!(output.status.code(), Some(7));
                }
            }
        }
        assert!(run(&mut Command::new("does-not-exist"), &[0; 101], Limits { stdin: 100, ..Limits::default() }).is_err());
    }

    #[test]
    fn cancellation_is_bounded_and_credentials_are_not_inherited() {
        let _serial = TEST_RUNNER.lock().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::Release);
        });
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "subprocess::tests::child_helper", "--ignored", "--nocapture"])
            .env("GEN_AUDIO_RUNNER_TEST", "wait");
        let started = Instant::now();
        let error = run_cancellable(&mut command, b"", Limits::default(), &cancel).err().unwrap();
        setter.join().unwrap();
        assert!(error.contains("cancelled"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        command.env("GEN_AUDIO_RUNNER_TEST", "env")
            .env("GEN_AUDIO_MCP_TOKEN", "synthetic-not-a-credential")
            .env("GEN_AUDIO_MCP_BOOT_NONCE", "synthetic-not-a-credential");
        assert!(run(&mut command, b"", Limits::default()).unwrap().status.success());
    }

    #[test]
    fn bounded_lines() {
        let mut input = std::io::Cursor::new(b"{}\nsecond\n".as_slice());
        assert_eq!(read_line_bounded(&mut input, 3).unwrap(), Some("{}\n".into()));
        assert!(read_line_bounded(&mut input, 3).is_err());
        assert!(read_line_bounded(&mut std::io::Cursor::new([255]), 4).is_err());
    }

    #[test]
    fn child_that_never_reads_stdin_cannot_hang_the_writer() {
        let _serial = TEST_RUNNER.lock().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "subprocess::tests::child_helper", "--ignored", "--nocapture"])
            .env("GEN_AUDIO_RUNNER_TEST", "wait");
        let start = Instant::now();
        let error = run(&mut command, &vec![0; 256 * 1024], Limits {
            deadline: Duration::from_secs(1), ..Limits::default()
        }).err().unwrap();
        assert!(error.contains("deadline"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn escaped_descendant_cannot_hold_pipe_cleanup_open() {
        let _serial = TEST_RUNNER.lock().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "subprocess::tests::child_helper", "--ignored", "--nocapture"])
            .env("GEN_AUDIO_RUNNER_TEST", "escaped");
        let start = Instant::now();
        let error = run(&mut command, b"", Limits {
            deadline: Duration::from_millis(150), ..Limits::default()
        }).err().unwrap();
        assert!(error.contains("deadline"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
