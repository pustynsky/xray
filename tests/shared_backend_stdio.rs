use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions, TryLockError},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{Arc, Barrier, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(60);

#[cfg(windows)]
#[test]
#[ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway"]
fn windows_backend_spawn_diagnostic() {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    let temporary = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    for (name, flags) in [
        ("plain", 0),
        ("detached", DETACHED_PROCESS),
        ("breakaway", CREATE_BREAKAWAY_FROM_JOB),
        ("combined", CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS),
    ] {
        for append in [false, true] {
            let log = OpenOptions::new().create(true).write(true).append(append)
                .truncate(!append).open(temporary.path().join("spawn.log")).unwrap();
            let result = Command::new(env!("CARGO_BIN_EXE_xray"))
                .arg("--version").current_dir(temporary.path())
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log))
                .creation_flags(flags).spawn();
            match result {
                Ok(mut child) => {
                    let status = child.wait().unwrap();
                    eprintln!("spawn {name}, append={append}: {status}");
                    assert!(status.success());
                }
                Err(error) => {
                    eprintln!("spawn {name}, append={append}: {error}");
                    failures.push(format!("{name}, append={append}: {error}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}


struct Workspace {
    root: tempfile::TempDir,
    runtime: PathBuf,
    key: String,
}

impl Workspace {
    fn new(marker: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("sample.rs"), format!("pub fn {marker}() {{}}\n")).unwrap();
        let canonical = fs::canonicalize(root.path()).unwrap().to_string_lossy().into_owned();
        #[cfg(windows)]
        let canonical = canonical.to_lowercase();
        let key = format!("{:x}", Sha256::digest(canonical.as_bytes()));
        let runtime = dirs::data_local_dir().unwrap().join("xray").join("backends");
        Self { root, runtime, key }
    }

    fn path(&self) -> &Path { self.root.path() }

    fn owner_released(&self) -> bool {
        let path = self.runtime.join(format!("{}.owner.lock", self.key));
        if !path.exists() { return true; }
        let file = OpenOptions::new().read(true).write(true).open(path).unwrap();
        match file.try_lock() {
            Ok(()) => true,
            Err(TryLockError::WouldBlock) => false,
            Err(TryLockError::Error(error)) => panic!("owner lock: {error}"),
        }
    }

    fn wait_for_shutdown(&self) {
        let start = Instant::now();
        while !self.owner_released() {
            assert!(start.elapsed() < WAIT, "backend did not release its workspace");
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if thread::panicking() {
            let log_path = self.runtime.join(format!("{}.log", self.key));
            if let Ok(log) = fs::read_to_string(&log_path) {
                let lines: Vec<_> = log.lines().rev().take(20).collect();
                eprintln!("Backend log {}:\n{}", log_path.display(), lines.into_iter().rev().collect::<Vec<_>>().join("\n"));
            }
        }
        let start = Instant::now();
        while !self.owner_released() && start.elapsed() < WAIT {
            thread::sleep(Duration::from_millis(50));
        }
        if !self.owner_released() {
            eprintln!("Test backend still owns {}; retaining its diagnostic files", self.path().display());
            return;
        }
        let status = Command::new(env!("CARGO_BIN_EXE_xray"))
            .args(["cleanup", "--dir"]).arg(self.path()).status();
        if !matches!(status, Ok(status) if status.success()) {
            eprintln!("Failed to clean test indexes for {}", self.path().display());
        }
        for suffix in ["owner.lock", "launch.lock", "log"] {
            let path = self.runtime.join(format!("{}.{}", self.key, suffix));
            if let Err(error) = fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("Failed to remove {}: {error}", path.display());
                }
        }
    }
}

fn command(root: &Path, ext: &str) -> Command {
    command_with_watch(root, ext, true)
}

fn command_with_watch(root: &Path, ext: &str, watch: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xray"));
    command.args(["serve", "--dir"]).arg(root)
        .args(["--ext", ext, "--definitions", "--debounce-ms", "20", "--no-periodic-rescan"])
        .env("XRAY_BACKEND_IDLE_SECONDS", "1")
        .env("XRAY_GUIDANCE_PREFIX", "0")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if watch { command.arg("--watch"); }
    command
}

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<Value>,
    stderr: Arc<Mutex<String>>,
    stdout_closed: mpsc::Receiver<()>,
    stderr_closed: mpsc::Receiver<()>,
    backend_pid: u32,
}

impl Client {
    fn start(root: &Path) -> Self { Self::start_with_ext(root, "rs") }

    fn start_with_ext(root: &Path, ext: &str) -> Self {
        Self::start_command(command(root, ext))
    }

    fn start_command(mut command: Command) -> Self {
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let stderr_pipe = child.stderr.take().unwrap();
        let (sender, replies) = mpsc::channel();
        let (stdout_done, stdout_closed) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let value = serde_json::from_str(&line).unwrap_or_else(|error| panic!("Invalid MCP stdout: {error}: {line}"));
                if sender.send(value).is_err() { return; }
            }
            let _ = stdout_done.send(());
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let log = Arc::clone(&stderr);
        let (pid_sender, pids) = mpsc::channel();
        let (stderr_done, stderr_closed) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr_pipe).lines() {
                let line = line.unwrap();
                if let Some(pid) = line.strip_prefix("Xray: connected to workspace backend PID ") {
                    let _ = pid_sender.send(pid.parse::<u32>().unwrap());
                }
                log.lock().unwrap().push_str(&format!("{line}\n"));
            }
            let _ = stderr_done.send(());
        });
        let mut client = Self { child, input, replies, stderr, stdout_closed, stderr_closed, backend_pid: 0 };
        client.backend_pid = pids.recv_timeout(WAIT).unwrap_or_else(|error| {
            panic!("Backend connection failed: {error}; {}", client.stderr.lock().unwrap())
        });
        client.request(1, "initialize", json!({"protocolVersion": "2025-03-26",
            "capabilities": {"roots": {"listChanged": true}}, "clientInfo": {"name": "shared-test", "version": "1"}}));
        client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        client
    }

    fn send(&mut self, value: Value) {
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{value}").unwrap();
        input.flush().unwrap();
    }

    fn response(&mut self, id: u64) -> Value {
        let response = self.replies.recv_timeout(WAIT).unwrap_or_else(|error| {
            panic!("MCP response failed: {error}; {}", self.stderr.lock().unwrap())
        });
        assert_eq!(response["id"], id, "response from wrong MCP session: {response}");
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.response(id)
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request(7, "tools/call", json!({"name": name, "arguments": arguments}))
    }

    fn payload(result: &Value) -> Value {
        assert_ne!(result["isError"], true, "{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap_or_else(|error| panic!("{error}: {text}"))
    }

    fn wait_for_marker(&mut self, marker: &str) {
        let start = Instant::now();
        loop {
            let result = self.call("xray_grep", json!({"terms": [marker], "countOnly": true}));
            if result["isError"] != true {
                let payload = Self::payload(&result);
                if payload["summary"]["totalFiles"] == 1 { return; }
            }
            assert!(start.elapsed() < WAIT, "marker not indexed: {result}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for_exit(&mut self) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() { return status; }
            assert!(start.elapsed() < Duration::from_secs(10), "proxy did not exit");
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn close(&mut self) {
        self.input.take();
        assert!(self.wait_for_exit().success(), "{}", self.stderr.lock().unwrap());
        self.stdout_closed.recv_timeout(Duration::from_secs(2)).expect("exited proxy's stdout remains open");
        self.stderr_closed.recv_timeout(Duration::from_secs(2)).expect("exited proxy's stderr remains open");
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        if self.child.try_wait().ok().flatten().is_none() {
            if let Err(error) = self.child.kill() { eprintln!("Proxy cleanup failed: {error}"); }
            if let Err(error) = self.child.wait() { eprintln!("Proxy wait failed: {error}"); }
        }
    }
}

#[test]
#[cfg_attr(windows, ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway")]
fn shared_backend_simultaneous_clients_and_first_client_exit() {
    let workspace = Workspace::new("shared_backend_marker");
    let barrier = Arc::new(Barrier::new(5));
    let starts: Vec<_> = (0..5).map(|_| {
        let root = workspace.path().to_owned();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || { barrier.wait(); Client::start(&root) })
    }).collect();
    let mut clients: Vec<_> = starts.into_iter().map(|start| start.join().unwrap()).collect();
    let pid = clients[0].backend_pid;
    assert!(clients.iter().all(|client| client.backend_pid == pid));
    clients[0].wait_for_marker("shared_backend_marker");
    clients[0].send(json!({"jsonrpc": "2.0", "id": 42, "method": "tools/list", "params": {}}));
    clients[1].send(json!({"jsonrpc": "2.0", "id": 42, "method": "tools/call",
        "params": {"name": "xray_info", "arguments": {}}}));
    assert!(clients[0].response(42)["tools"].is_array());
    assert!(clients[1].response(42)["content"].is_array());
    clients[0].close();
    clients[1].wait_for_marker("shared_backend_marker");
    fs::write(workspace.path().join("new.rs"), "pub fn watcher_shared_marker() {}\n").unwrap();
    for client in clients.iter_mut().skip(1) { client.wait_for_marker("watcher_shared_marker"); }
    let log = fs::read_to_string(workspace.runtime.join(format!("{}.log", workspace.key))).unwrap();
    assert_eq!(log.matches("Starting MCP server").count(), 1, "indexes initialized more than once");
    for client in clients.iter_mut().skip(1) { client.close(); }
    workspace.wait_for_shutdown();
}

#[test]
#[cfg_attr(windows, ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway")]
fn shared_backend_first_client_streams_close_while_other_client_lives() {
    let workspace = Workspace::new("stdio_owner_marker");
    let mut first = Client::start(workspace.path());
    let mut second = Client::start(workspace.path());
    assert_eq!(first.backend_pid, second.backend_pid);
    first.close();
    second.wait_for_marker("stdio_owner_marker");
    second.close();
    workspace.wait_for_shutdown();
}

#[test]
#[cfg_attr(windows, ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway")]
fn shared_backend_idle_restart_applies_new_configuration() {
    let workspace = Workspace::new("config_restart_marker");
    let mut original_command = command_with_watch(workspace.path(), "rs", false);
    original_command.env("XRAY_BACKEND_IDLE_SECONDS", "20");
    let mut original = Client::start_command(original_command);
    original.wait_for_marker("config_restart_marker");
    let old_pid = original.backend_pid;
    // Do not use close() here: this test isolates idle reconfiguration from
    // the independent inherited-stdio regression.
    original.input.take();
    assert!(original.wait_for_exit().success());
    assert!(!workspace.owner_released(), "old backend must still own its idle workspace");
    let started = Instant::now();
    let barrier = Arc::new(Barrier::new(3));
    let starts: Vec<_> = (0..3).map(|_| {
        let root = workspace.path().to_owned();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let mut replacement = command_with_watch(&root, "rs", false);
            replacement.env("XRAY_BACKEND_IDLE_SECONDS", "20").args(["--max-response-kb", "64"]);
            barrier.wait();
            Client::start_command(replacement)
        })
    }).collect();
    let results: Vec<_> = starts.into_iter().map(|start| start.join()).collect();
    let mut clients: Vec<_> = results.into_iter().map(Result::unwrap).collect();
    assert!(started.elapsed() < Duration::from_secs(10), "restart waited for the old idle timeout");
    let new_pid = clients[0].backend_pid;
    assert_ne!(new_pid, old_pid);
    for client in &mut clients {
        assert_eq!(client.backend_pid, new_pid, "restart elected multiple backends");
        client.wait_for_marker("config_restart_marker");
    }
    let log = fs::read_to_string(workspace.runtime.join(format!("{}.log", workspace.key))).unwrap();
    assert_eq!(log.matches("Starting MCP server").count(), 2, "old and replacement backends must initialize once each");
    for client in &mut clients { client.close(); }
    workspace.wait_for_shutdown();
}

#[test]
#[cfg_attr(windows, ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway")]
fn shared_backend_isolates_workspaces_and_checks_configuration() {
    let first = Workspace::new("first_workspace_marker");
    let second = Workspace::new("second_workspace_marker");
    let mut a = Client::start(first.path());
    let mut b = Client::start(second.path());
    assert_ne!(a.backend_pid, b.backend_pid);
    a.wait_for_marker("first_workspace_marker");
    b.wait_for_marker("second_workspace_marker");
    let absent = Client::payload(&b.call("xray_grep", json!({"terms": ["first_workspace_marker"], "countOnly": true})));
    assert_eq!(absent["summary"]["totalFiles"], 0);
    let mut same = Client::start_with_ext(first.path(), "RS,rs");
    assert_eq!(a.backend_pid, same.backend_pid);
    same.close();
    let mismatch = command(first.path(), "rs").args(["--max-response-kb", "64"]).output().unwrap();
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("max_response_kb"));
    let mismatch = command(first.path(), "rs").env("XRAY_GUIDANCE_PREFIX", "1").output().unwrap();
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("env:XRAY_GUIDANCE_PREFIX"));
    let changed = a.call("xray_reindex", json!({"ext": ["md"]}));
    assert_eq!(changed["isError"], true);
    a.wait_for_marker("first_workspace_marker");
    a.close();
    b.close();
    first.wait_for_shutdown();
    second.wait_for_shutdown();
}

#[test]
#[cfg_attr(windows, ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway")]
fn shared_backend_edit_and_reindex_are_visible_to_every_client() {
    let workspace = Workspace::new("before_shared_edit");
    let mut a = Client::start(workspace.path());
    let mut b = Client::start(workspace.path());
    a.wait_for_marker("before_shared_edit");
    let edit = a.call("xray_edit", json!({"path": "sample.rs", "edits": [{"search": "before_shared_edit", "replace": "after_shared_edit"}]}));
    assert_ne!(edit["isError"], true, "{edit}");
    b.wait_for_marker("after_shared_edit");
    let rebuild = b.call("xray_reindex", json!({}));
    assert_ne!(rebuild["isError"], true, "{rebuild}");
    a.wait_for_marker("after_shared_edit");
    let other_root = tempfile::tempdir().unwrap();
    let switch = a.call("xray_reindex", json!({"dir": other_root.path()}));
    assert_eq!(switch["isError"], true);
    b.wait_for_marker("after_shared_edit");
    a.close();
    b.close();
    workspace.wait_for_shutdown();
}

#[test]
#[cfg_attr(windows, ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway")]
fn shared_backend_drains_responses_after_stdin_eof() {
    let workspace = Workspace::new("drain_shared_marker");
    let mut client = Client::start(workspace.path());
    for id in 100..110 {
        client.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {}}));
    }
    client.input.take();
    for id in 100..110 { assert!(client.response(id)["tools"].is_array()); }
    assert!(client.wait_for_exit().success());
    workspace.wait_for_shutdown();
}

#[cfg(windows)]
fn private_bytes(pid: u32) -> usize {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let raw = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        assert!(!raw.is_null());
        let process = OwnedHandle::from_raw_handle(raw);
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        assert_ne!(K32GetProcessMemoryInfo(process.as_raw_handle(), (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(), counters.cb), 0);
        counters.PrivateUsage
    }
}

#[cfg(windows)]
#[test]
#[ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway"]
fn shared_backend_memory_does_not_duplicate_indexes() {
    let workspace = Workspace::new("memory_shared_marker");
    for file in 0..1500 {
        let mut source = String::new();
        for method in 0..30 {
            source.push_str(&format!("pub fn memory_fixture_{file}_{method}() -> usize {{ {method} }}\n"));
        }
        fs::write(workspace.path().join(format!("fixture_{file}.rs")), source).unwrap();
    }
    // Watcher startup reconciliation is covered separately. Keep this measurement
    // focused on index sharing, not concurrent cold-reconcile allocations.
    let mut first = Client::start_command(command_with_watch(workspace.path(), "rs", false));
    first.wait_for_marker("memory_shared_marker");
    let start = Instant::now();
    loop {
        let info = Client::payload(&first.call("xray_info", json!({})));
        let ready = info["indexes"].as_array().unwrap().iter()
            .any(|index| index["type"] == "definition" && index.get("status").is_none());
        if ready { break; }
        assert!(start.elapsed() < WAIT, "definitions did not become ready");
        thread::sleep(Duration::from_millis(50));
    }
    first.wait_for_marker("memory_shared_marker");
    thread::sleep(Duration::from_secs(2));
    let backend_before = private_bytes(first.backend_pid);
    let proxy_before = private_bytes(first.child.id());
    let total_before = backend_before + proxy_before;
    let mut others: Vec<_> = (0..4).map(|_| Client::start_command(command_with_watch(workspace.path(), "rs", false))).collect();
    for client in &mut others {
        assert_eq!(client.backend_pid, first.backend_pid);
        client.wait_for_marker("memory_shared_marker");
    }
    let backend_after = private_bytes(first.backend_pid);
    let proxies: Vec<_> = std::iter::once(&first).chain(others.iter()).map(|client| private_bytes(client.child.id())).collect();
    let total_after = backend_after + proxies.iter().sum::<usize>();
    eprintln!("private commit: 1 client={:.1} MiB; 5 clients={:.1} MiB; backend {:.1}->{:.1} MiB; proxies={proxies:?} bytes",
        total_before as f64 / 1048576.0, total_after as f64 / 1048576.0,
        backend_before as f64 / 1048576.0, backend_after as f64 / 1048576.0);
    assert!(backend_after <= backend_before + 16 * 1024 * 1024, "backend duplicated state per client");
    // mimalloc's 64-bit segment is 32 MiB, so compare proxy growth with
    // the measured first proxy rather than assuming an absolute sub-32 MiB size.
    assert!(proxies.iter().all(|bytes| *bytes <= proxy_before + 4 * 1024 * 1024), "a proxy loaded heavy state");
    assert!(total_after <= total_before + 4 * proxy_before + 32 * 1024 * 1024,
        "additional clients consumed more than proxy overhead and backend headroom");
    first.close();
    others[0].wait_for_marker("memory_shared_marker");
    for client in &mut others { client.close(); }
    workspace.wait_for_shutdown();
}

#[cfg(windows)]
fn kill_backend(pid: u32) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    unsafe {
        let raw = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(!raw.is_null());
        let process = OwnedHandle::from_raw_handle(raw);
        assert_ne!(TerminateProcess(process.as_raw_handle(), 1), 0);
    }
}

#[cfg(unix)]
fn kill_backend(pid: u32) {
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) }, 0);
}

#[test]
#[cfg_attr(windows, ignore = "Run scripts/test-shared-backend.ps1: Cargo's Windows job prohibits breakaway")]
fn shared_backend_crash_fails_connections_and_next_connection_restarts() {
    let workspace = Workspace::new("restart_shared_marker");
    let mut first = Client::start(workspace.path());
    let mut second = Client::start(workspace.path());
    first.wait_for_marker("restart_shared_marker");
    let old_pid = first.backend_pid;
    kill_backend(old_pid);
    assert!(!first.wait_for_exit().success());
    assert!(!second.wait_for_exit().success());
    let mut replacement = Client::start(workspace.path());
    assert_ne!(replacement.backend_pid, old_pid);
    replacement.wait_for_marker("restart_shared_marker");
    replacement.close();
    workspace.wait_for_shutdown();
}
