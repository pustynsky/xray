#![allow(clippy::disallowed_methods, reason = "Process-level tests exercise native creation flags and stdio directly")]

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
fn windows_backend_spawn_diagnostic() {
    let supported = windows_breakaway_supported();
    println!("XRAY_TEST_BREAKAWAY={}", if supported { "supported" } else { "unsupported" });
}

#[cfg(windows)]
fn windows_breakaway_supported() -> bool {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    let temporary = tempfile::tempdir().unwrap();
    let (mut allowed, mut denied) = (0, 0);
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
                    if flags & CREATE_BREAKAWAY_FROM_JOB != 0 { allowed += 1; }
                }
                Err(error) if flags & CREATE_BREAKAWAY_FROM_JOB != 0 && error.raw_os_error() == Some(5) => {
                    eprintln!("spawn {name}, append={append}: host denies breakaway: {error}");
                    denied += 1;
                }
                Err(error) => panic!("spawn {name}, append={append}: unexpected failure: {error}"),
            }
        }
    }
    assert!(matches!((allowed, denied), (4, 0) | (0, 4)), "inconsistent breakaway results: allowed={allowed}, denied={denied}");
    allowed == 4
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
    backend_pids: mpsc::Receiver<u32>,
}

impl Client {
    fn start(root: &Path) -> Self { Self::start_with_ext(root, "rs") }

    fn start_with_ext(root: &Path, ext: &str) -> Self {
        Self::start_command(command(root, ext))
    }

    fn start_command(mut command: Command) -> Self {
        Self::attach(command.spawn().unwrap())
    }

    fn attach(child: Child) -> Self {
        Self::attach_with_backend(child, true)
    }

    fn attach_with_backend(child: Child, shared: bool) -> Self {
        let mut client = Self::attach_uninitialized(child);
        if shared {
            client.backend_pid = client.backend_pids.recv_timeout(WAIT).unwrap_or_else(|error| {
                panic!("Backend connection failed: {error}; {}", client.stderr.lock().unwrap())
            });
        }
        let capabilities = if shared { json!({"roots": {"listChanged": true}}) } else { json!({}) };
        client.request(1, "initialize", json!({"protocolVersion": "2025-03-26",
            "capabilities": capabilities, "clientInfo": {"name": "shared-test", "version": "1"}}));
        client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        client
    }

    fn attach_uninitialized(mut child: Child) -> Self {
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
        Self { child, input, replies, stderr, stdout_closed, stderr_closed, backend_pid: 0, backend_pids: pids }
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

    fn wait_for_recovery(&mut self) {
        let old_pid = self.backend_pid;
        self.backend_pid = self.backend_pids.recv_timeout(WAIT).unwrap_or_else(|error| {
            panic!("recovery failed: {error}; {}", self.stderr.lock().unwrap())
        });
        assert_ne!(self.backend_pid, old_pid, "dead backend was not replaced");
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
fn missing_explicit_dir_uses_private_server_and_recovers() {
    let workspace = Workspace::new("missing_dir_recovery_marker");
    let missing = workspace.path().join("moved-repo");
    let mut client = Client::attach_with_backend(command(&missing, "rs").spawn().unwrap(), false);

    let result = client.call("xray_grep", json!({"terms": ["missing_dir_recovery_marker"], "countOnly": true}));

    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("WORKSPACE_UNRESOLVED"), "{text}");
    assert!(text.contains("does not exist"), "{text}");
    assert!(!text.contains("--dir ."), "{text}");

    let result = client.call("xray_reindex", json!({"dir": workspace.path().join("still-missing")}));

    assert_eq!(result["isError"], true, "{result}");
    let result = client.call("xray_grep", json!({"terms": ["missing_dir_recovery_marker"], "countOnly": true}));
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("WORKSPACE_UNRESOLVED"), "{text}");
    assert!(!text.contains("INDEX_BUILDING"), "{text}");

    let result = client.call("xray_reindex", json!({}));

    assert_eq!(result["isError"], true, "{result}");
    let result = client.call("xray_grep", json!({"terms": ["missing_dir_recovery_marker"], "countOnly": true}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(result["content"][0]["text"].as_str().unwrap().contains("WORKSPACE_UNRESOLVED"), "{result}");

    let result = client.call("xray_reindex", json!({"dir": workspace.path()}));

    let payload = Client::payload(&result);
    assert_eq!(payload["status"], "ok");
    assert!(payload["files"].as_u64().unwrap() > 0);
    client.wait_for_marker("missing_dir_recovery_marker");
    client.close();
    assert!(client.stderr.lock().unwrap().contains("workspace UNRESOLVED"));
    assert!(!client.stderr.lock().unwrap().contains("connected to workspace backend"));
}

#[test]
fn explicit_file_dir_uses_private_unresolved_server() {
    let workspace = Workspace::new("file_dir_marker");
    let file = workspace.path().join("sample.rs");
    let mut client = Client::attach_with_backend(command(&file, "rs").spawn().unwrap(), false);

    let result = client.call("xray_grep", json!({"terms": ["file_dir_marker"], "countOnly": true}));

    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("WORKSPACE_UNRESOLVED"), "{text}");
    assert!(text.contains("does not exist or is not a directory"), "{text}");
    client.close();
    assert!(!client.stderr.lock().unwrap().contains("connected to workspace backend"));
}

#[test]
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
fn shared_backend_idle_restart_applies_new_configuration() {
    let workspace = Workspace::new("config_restart_marker");
    let mut original_command = command_with_watch(workspace.path(), "rs", false);
    original_command.env("XRAY_BACKEND_IDLE_SECONDS", "120");
    let mut original = Client::start_command(original_command);
    original.wait_for_marker("config_restart_marker");
    let old_pid = original.backend_pid;
    // Do not use close() here: this test isolates idle reconfiguration from
    // the independent inherited-stdio regression.
    original.input.take();
    assert!(original.wait_for_exit().success());
    assert!(!workspace.owner_released(), "old backend must still own its idle workspace");
    let barrier = Arc::new(Barrier::new(3));
    let starts: Vec<_> = (0..3).map(|_| {
        let root = workspace.path().to_owned();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let mut replacement = command_with_watch(&root, "rs", false);
            replacement.args(["--max-response-kb", "64"]);
            barrier.wait();
            Client::start_command(replacement)
        })
    }).collect();
    let results: Vec<_> = starts.into_iter().map(|start| start.join()).collect();
    let mut clients: Vec<_> = results.into_iter().map(Result::unwrap).collect();
    let new_pid = clients[0].backend_pid;
    assert_ne!(new_pid, old_pid);
    for client in &mut clients {
        assert_eq!(client.backend_pid, new_pid, "restart elected multiple backends");
        client.wait_for_marker("config_restart_marker");
    }
    let log = fs::read_to_string(workspace.runtime.join(format!("{}.log", workspace.key))).unwrap();
    assert_eq!(log.matches("Starting MCP server").count(), 2, "old and replacement backends must initialize once each");
    // All clients have a 60-second connection deadline, shorter than the old
    // 120-second idle timeout. Check the actual retirement path, not CPU speed.
    assert_eq!(log.matches("Retiring idle workspace backend for configuration change").count(), 1,
        "replacement must retire the old backend, not wait for idle expiry");
    for client in &mut clients { client.close(); }
    workspace.wait_for_shutdown();
}

#[test]
fn shared_backend_idle_timeout_is_configurable_per_workspace() {
    let first = Workspace::new("idle_first_workspace");
    let second = Workspace::new("idle_second_workspace");
    let mut first_command = command_with_watch(first.path(), "rs", false);
    first_command.env("XRAY_BACKEND_IDLE_SECONDS", "1");
    let mut second_command = command_with_watch(second.path(), "rs", false);
    second_command.env("XRAY_BACKEND_IDLE_SECONDS", "3");
    let mut a = Client::start_command(first_command);
    let mut b = Client::start_command(second_command);
    assert_ne!(a.backend_pid, b.backend_pid);
    a.wait_for_marker("idle_first_workspace");
    b.wait_for_marker("idle_second_workspace");

    a.close();
    let waiting = Instant::now();
    while !first.owner_released() {
        b.request(42, "ping", json!({}));
        assert!(waiting.elapsed() < WAIT, "activity in another workspace prevented idle shutdown");
        thread::sleep(Duration::from_millis(50));
    }
    assert!(!second.owner_released(), "another workspace's shutdown stopped a connected backend");
    b.wait_for_marker("idle_second_workspace");

    let disconnected = Instant::now();
    b.close();
    second.wait_for_shutdown();
    assert!(disconnected.elapsed() >= Duration::from_secs(3),
        "second workspace did not honor its own configured idle timeout");
}

#[test]
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

fn workspace_profile_command(profiles: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xray"));
    command.args(["serve-workspace", "--profiles"]).arg(profiles)
        .current_dir(cwd)
        .env("XRAY_BACKEND_IDLE_SECONDS", "1")
        .env("XRAY_GUIDANCE_PREFIX", "0")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

fn write_workspace_profile(profiles: &Path, id: &str, root: &Path, ext: &str) {
    let directory = profiles.join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("config.json"), serde_json::to_vec(&json!({
        "version": 1, "workspaceRoot": root,
        "args": ["--ext", ext, "--definitions", "--debounce-ms", "20", "--no-periodic-rescan", "--watch"],
    })).unwrap()).unwrap();
}

#[test]
fn workspace_profiles_reuse_explicit_serve_backend_and_reject_unknown_cwd() {
    let workspace = Workspace::new("profile_shared_marker");
    let profiles = tempfile::tempdir().unwrap();
    write_workspace_profile(profiles.path(), "registered", workspace.path(), "rs");
    let nested = workspace.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let mut direct = Client::start(workspace.path());
    let mut selected = Client::start_command(workspace_profile_command(profiles.path(), &nested));
    assert_eq!(direct.backend_pid, selected.backend_pid);
    direct.close();
    selected.wait_for_marker("profile_shared_marker");
    selected.close();
    workspace.wait_for_shutdown();
    let unknown = tempfile::tempdir().unwrap();
    let rejected = workspace_profile_command(profiles.path(), unknown.path()).output().unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("No Xray profile is installed"));
    assert!(rejected.stdout.is_empty(), "an unregistered workspace must not start MCP");
}

#[test]
fn workspace_profiles_client_roots_from_unrelated_host_share_the_correct_backend() {
    let profiles = tempfile::tempdir().unwrap();
    let host = tempfile::tempdir().unwrap();
    let workspaces = [Workspace::new("roots_profile_first"), Workspace::new("roots_profile_second")];
    for (index, workspace) in workspaces.iter().enumerate() {
        write_workspace_profile(profiles.path(), &index.to_string(), workspace.path(), "rs");
    }
    for (index, workspace) in workspaces.iter().enumerate() {
        let mut direct = Client::start(workspace.path());
        let mut command = workspace_profile_command(profiles.path(), host.path());
        let mut selected = Client::attach_uninitialized(command.spawn().unwrap());
        let initialized = selected.request(1, "initialize", json!({"protocolVersion": "2025-03-26",
            "capabilities": {"roots": {}}, "clientInfo": {"name": "vscode-user-mcp", "version": "1"}}));
        assert_eq!(initialized["serverInfo"]["name"], "xray");
        selected.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let roots = selected.replies.recv_timeout(WAIT).unwrap();
        assert_eq!(roots["method"], "roots/list");
        // VS Code can ask for tools while its roots request is still in flight.
        selected.send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}}));
        selected.send(json!({"jsonrpc": "2.0", "id": roots["id"], "result": {"roots": [{
            "uri": url::Url::from_directory_path(workspace.path()).unwrap().as_str(),
        }]}}));
        selected.backend_pid = selected.backend_pids.recv_timeout(WAIT).unwrap_or_else(|error| {
            panic!("Root-selected backend failed: {error}; {}", selected.stderr.lock().unwrap())
        });
        assert_eq!(direct.backend_pid, selected.backend_pid);
        let tools = selected.response(3);
        assert!(tools["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "xray_info"));
        selected.wait_for_marker(["roots_profile_first", "roots_profile_second"][index]);
        let info = Client::payload(&selected.call("xray_info", json!({})));
        assert_eq!(fs::canonicalize(info["summary"]["serverDir"].as_str().unwrap()).unwrap(),
            fs::canonicalize(workspace.path()).unwrap());
        let marker = ["roots_profile_first", "roots_profile_second"][index];
        let start = Instant::now();
        let definitions = loop {
            let result = selected.call("xray_definitions", json!({"name": [marker], "includeBody": true}));
            if result["isError"] != true { break Client::payload(&result); }
            assert!(start.elapsed() < WAIT, "root-selected definitions did not become ready: {result}");
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(definitions["definitions"][0]["name"], marker);
        selected.close();
        direct.close();
        workspace.wait_for_shutdown();
    }
}

#[test]
fn workspace_profiles_three_workspaces_five_sessions_each() {
    let profiles = tempfile::tempdir().unwrap();
    let workspaces: Vec<_> = (0..3).map(|index| Workspace::new(&format!("profile_marker_{index}"))).collect();
    for (index, workspace) in workspaces.iter().enumerate() {
        write_workspace_profile(profiles.path(), &index.to_string(), workspace.path(), ["rs", "rs,md", "rs,txt"][index]);
        fs::create_dir(workspace.path().join("nested")).unwrap();
        fs::write(workspace.path().join("extra.md"), "profile_markdown_only").unwrap();
    }
    let barrier = Arc::new(Barrier::new(15));
    let mut starts = Vec::new();
    for (index, workspace) in workspaces.iter().enumerate() {
        for session in 0..5 {
            let cwd = if session % 2 == 0 { workspace.path().to_owned() } else { workspace.path().join("nested") };
            let profiles = profiles.path().to_owned();
            let barrier = Arc::clone(&barrier);
            starts.push((index, thread::spawn(move || {
                barrier.wait();
                Client::start_command(workspace_profile_command(&profiles, &cwd))
            })));
        }
    }
    let mut clients: Vec<_> = starts.into_iter().map(|(index, start)| (index, start.join().unwrap())).collect();
    let backends: std::collections::BTreeSet<_> = clients.iter().map(|(_, client)| client.backend_pid).collect();
    let proxies: std::collections::BTreeSet<_> = clients.iter().map(|(_, client)| client.child.id()).collect();
    assert_eq!(backends.len(), 3);
    assert_eq!(proxies.len(), 15);
    assert!(backends.is_disjoint(&proxies));
    let expected = [clients[0].1.backend_pid, clients[5].1.backend_pid, clients[10].1.backend_pid];
    for (index, client) in &mut clients {
        assert_eq!(client.backend_pid, expected[*index]);
        client.wait_for_marker(&format!("profile_marker_{index}"));
        let info = Client::payload(&client.call("xray_info", json!({})));
        assert_eq!(fs::canonicalize(info["summary"]["serverDir"].as_str().unwrap()).unwrap(),
            fs::canonicalize(workspaces[*index].path()).unwrap());
        let extra = Client::payload(&client.call("xray_grep", json!({
            "terms": ["profile_markdown_only"], "countOnly": true,
        })));
        assert_eq!(extra["summary"]["totalFiles"], usize::from(*index == 1));
        let start = Instant::now();
        loop {
            let info = Client::payload(&client.call("xray_info", json!({})));
            if info["indexes"].as_array().unwrap().iter().any(|entry| entry["type"] == "definition" && entry.get("status").is_none()) { break; }
            assert!(start.elapsed() < WAIT, "profile definitions did not become ready");
            thread::sleep(Duration::from_millis(20));
        }
        let definitions = Client::payload(&client.call("xray_definitions", json!({
            "name": [format!("profile_marker_{index}")], "file": ["sample.rs"], "includeBody": true,
        })));
        assert_eq!(definitions["definitions"][0]["name"], format!("profile_marker_{index}"));
        assert!(definitions["definitions"][0]["body"].as_array().unwrap().iter()
            .any(|line| line.as_str().unwrap().contains("pub fn")));
    }
    for index in 0..3 {
        clients[index * 5].1.close();
        clients[index * 5 + 1].1.wait_for_marker(&format!("profile_marker_{index}"));
        let workspace = &workspaces[index];
        let log = fs::read_to_string(workspace.runtime.join(format!("{}.log", workspace.key))).unwrap();
        assert_eq!(log.matches("Starting MCP server").count(), 1, "one index owner per workspace");
    }
    for (position, (_, client)) in clients.iter_mut().enumerate() {
        if position % 5 != 0 { client.close(); }
    }
    for workspace in &workspaces { workspace.wait_for_shutdown(); }
}

#[test]
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
struct RestrictedHostJob(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl RestrictedHostJob {
    fn new() -> Self {
        use std::{os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle}, ptr};
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            assert!(!handle.is_null(), "{}", std::io::Error::last_os_error());
            let job = OwnedHandle::from_raw_handle(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            assert_ne!(SetInformationJobObject(job.as_raw_handle(), JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32), 0);
            Self(job)
        }
    }

    fn spawn(&self, mut command: Command) -> Child {
        use std::os::windows::{io::{AsRawHandle, FromRawHandle, OwnedHandle}, process::CommandExt};
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            System::{Diagnostics::ToolHelp::*, JobObjects::AssignProcessToJobObject, Threading::*},
        };
        // Assign before any user code runs, so backend startup cannot escape the
        // test's per-client host job through a scheduling race.
        command.creation_flags(CREATE_SUSPENDED);
        let mut child = command.spawn().unwrap();
        let setup = || -> std::io::Result<()> {
            unsafe {
                if AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let raw = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if raw == INVALID_HANDLE_VALUE { return Err(std::io::Error::last_os_error()); }
                let snapshot = OwnedHandle::from_raw_handle(raw);
                let mut entry: THREADENTRY32 = std::mem::zeroed();
                entry.dwSize = size_of::<THREADENTRY32>() as u32;
                let mut available = Thread32First(snapshot.as_raw_handle(), &mut entry);
                while available != 0 {
                    if entry.th32OwnerProcessID == child.id() {
                        let raw_thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if raw_thread.is_null() { return Err(std::io::Error::last_os_error()); }
                        let thread = OwnedHandle::from_raw_handle(raw_thread);
                        if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                            return Err(std::io::Error::last_os_error());
                        }
                        return Ok(());
                    }
                    available = Thread32Next(snapshot.as_raw_handle(), &mut entry);
                }
                Err(std::io::Error::other("Suspended client primary thread not found"))
            }
        };
        if let Err(error) = setup() {
            let killed = child.kill();
            let waited = child.wait();
            panic!("host job setup: {error}; kill={killed:?}; wait={waited:?}");
        }
        child
    }

    fn contains(&self, pid: u32) -> bool {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::{JobObjects::IsProcessInJob, Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION}};
        unsafe {
            let raw = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            assert!(!raw.is_null());
            let process = OwnedHandle::from_raw_handle(raw);
            let mut member = 0;
            assert_ne!(IsProcessInJob(process.as_raw_handle(), self.0.as_raw_handle(), &mut member), 0);
            member != 0
        }
    }

    fn terminate(&self) {
        use std::os::windows::io::AsRawHandle;
        assert_ne!(unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0.as_raw_handle(), 99) }, 0);
    }
}

#[cfg(windows)]
#[test]
fn shared_backend_survives_first_clients_host_job_termination() {
    let workspace = Workspace::new("host_survivor_marker");
    let first_job = RestrictedHostJob::new();
    let second_job = RestrictedHostJob::new();
    let third_job = RestrictedHostJob::new();
    let mut first = Client::attach(first_job.spawn(command(workspace.path(), "rs")));
    let mut second = Client::attach(second_job.spawn(command(workspace.path(), "rs")));
    let mut third = Client::attach(third_job.spawn(command(workspace.path(), "rs")));
    assert_eq!(first.backend_pid, second.backend_pid);
    assert_eq!(first.backend_pid, third.backend_pid);
    assert!(first_job.contains(first.backend_pid), "backend must respect the first host's job boundary");
    first.wait_for_marker("host_survivor_marker");
    first_job.terminate();
    assert!(!first.wait_for_exit().success());
    second.wait_for_recovery();
    third.wait_for_recovery();
    assert_eq!(second.backend_pid, third.backend_pid, "survivors must elect one replacement");
    assert!(second_job.contains(second.backend_pid) || third_job.contains(second.backend_pid));
    second.wait_for_marker("host_survivor_marker");
    third.wait_for_marker("host_survivor_marker");
    let log = fs::read_to_string(workspace.runtime.join(format!("{}.log", workspace.key))).unwrap();
    assert_eq!(log.matches("Starting MCP server").count(), 2);
    second.close();
    third.close();
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
fn shared_backend_lost_edit_response_is_reported_and_never_replayed() {
    lost_edit_response("");
}

#[test]
fn shared_backend_discards_partial_json_before_recovery_error() {
    lost_edit_response("{\"jsonrpc\":\"2.0\",\"id\":72,\"res");
}

#[test]
fn shared_backend_discards_response_without_newline_before_recovery_error() {
    lost_edit_response("{\"jsonrpc\":\"2.0\",\"id\":72,\"result\":{}}");
}

fn lost_edit_response(fragment: &'static str) {
    use interprocess::local_socket::{prelude::*, ListenerOptions};
    use std::io::Read;
    let workspace = Workspace::new("apply_counter");
    fs::create_dir_all(&workspace.runtime).unwrap();
    let owner = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(workspace.runtime.join(format!("{}.owner.lock", workspace.key))).unwrap();
    owner.lock().unwrap();
    #[cfg(windows)]
    let name = {
        use interprocess::local_socket::{GenericNamespaced, ToNsName};
        let namespace = format!("{:x}", Sha256::digest(workspace.runtime.as_os_str().as_encoded_bytes()));
        format!("xray-{namespace}-{}", workspace.key).to_ns_name::<GenericNamespaced>().unwrap()
    };
    #[cfg(unix)]
    let name = {
        use interprocess::local_socket::{GenericFilePath, ToFsName};
        workspace.runtime.join(format!("{}.sock", &workspace.key[..24])).to_fs_name::<GenericFilePath>().unwrap()
    };
    let listener = ListenerOptions::new().name(name).create_sync().unwrap();
    let target = workspace.path().join("sample.rs");
    let edited = "pub fn apply_counter_apply_counter() {}\n";
    let write_target = target.clone();
    let server = thread::spawn(move || {
        let mut stream = listener.accept().unwrap();
        fn frame(stream: &mut impl Read) -> Value {
            let mut size = [0; 4];
            stream.read_exact(&mut size).unwrap();
            let size = u32::from_le_bytes(size) as usize;
            assert!(size <= 65536 && size > 0);
            let mut data = vec![0; size];
            stream.read_exact(&mut data).unwrap();
            serde_json::from_slice(&data).unwrap()
        }
        let hello = frame(&mut stream);
        let welcome = serde_json::to_vec(&json!({"protocol": hello["protocol"], "pid": std::process::id(), "error": null, "retry": false})).unwrap();
        stream.write_all(&(welcome.len() as u32).to_le_bytes()).unwrap();
        stream.write_all(&welcome).unwrap();
        let initialize = frame(&mut stream);
        assert_eq!(initialize["method"], "initialize");
        writeln!(stream, "{}", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"test-owner","version":"1"}}})).unwrap();
        let edit = loop {
            let request = frame(&mut stream);
            if request["method"] == "tools/call" { break request; }
            assert_eq!(request["method"], "notifications/initialized");
        };
        assert_eq!(edit["params"]["name"], "xray_edit");
        // Model the dangerous boundary: the edit committed but its response was lost.
        fs::write(write_target, edited).unwrap();
        stream.write_all(fragment.as_bytes()).unwrap();
        stream.flush().unwrap();
        drop(stream);
        drop(listener);
        drop(owner);
    });
    let mut client = Client::start(workspace.path());
    client.send(json!({"jsonrpc":"2.0","id":72,"method":"tools/call","params":{
        "name":"xray_edit","arguments":{"path":"sample.rs","edits":[{"search":"apply_counter","replace":"apply_counter_apply_counter"}]}
    }}));
    let response = client.replies.recv_timeout(WAIT).unwrap();
    assert_eq!(response["id"], 72);
    assert_eq!(response["error"]["code"], -32000);
    assert!(response["error"]["message"].as_str().unwrap().contains("NOT replayed"));
    assert!(response.get("result").is_none());
    server.join().unwrap();
    client.wait_for_recovery();
    client.wait_for_marker("apply_counter_apply_counter");
    assert_eq!(fs::read_to_string(target).unwrap(), edited, "edit was replayed after its response was lost");
    client.close();
    workspace.wait_for_shutdown();
}

#[test]
fn shared_backend_skips_oversized_and_null_id_input_without_losing_session() {
    let workspace = Workspace::new("input_survivor_marker");
    let mut client = Client::start(workspace.path());
    client.send(json!({"jsonrpc":"2.0","id":null,"method":"ping"}));
    let oversized = vec![b'x'; 10 * 1024 * 1024 + 1];
    client.input.as_mut().unwrap().write_all(&oversized).unwrap();
    client.input.as_mut().unwrap().write_all(b"\n").unwrap();
    client.input.as_mut().unwrap().flush().unwrap();
    let error = client.replies.recv_timeout(WAIT).unwrap();
    assert_eq!(error["id"], Value::Null);
    assert_eq!(error["error"]["code"], -32600);
    client.wait_for_marker("input_survivor_marker");
    client.close();
    workspace.wait_for_shutdown();
}

#[test]
fn shared_backend_crash_recovers_existing_connections_without_replaying_calls() {
    let workspace = Workspace::new("restart_shared_marker");
    let mut first = Client::start(workspace.path());
    let mut second = Client::start(workspace.path());
    first.wait_for_marker("restart_shared_marker");
    let old_pid = first.backend_pid;
    kill_backend(old_pid);
    first.wait_for_recovery();
    second.wait_for_recovery();
    assert_eq!(first.backend_pid, second.backend_pid, "survivors did not share one replacement");
    assert!(first.child.try_wait().unwrap().is_none());
    assert!(second.child.try_wait().unwrap().is_none());
    first.wait_for_marker("restart_shared_marker");
    second.wait_for_marker("restart_shared_marker");
    first.close();
    second.close();
    workspace.wait_for_shutdown();
}
