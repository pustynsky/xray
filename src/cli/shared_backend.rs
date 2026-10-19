mod platform;
mod proxy;

use super::{args::ServeArgs, serve};
use crate::mcp::{handlers::HandlerContext, server};
use interprocess::{
    ConnectWaitMode,
    local_socket::{
        ConnectOptions, Listener, ListenerNonblockingMode,
        ListenerOptions, Name, Stream, prelude::*,
    },
};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    thread,
    time::{Duration, Instant},
};

const PROTOCOL: u32 = 1;
const POLL: Duration = Duration::from_millis(25);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HANDSHAKE: usize = 64 * 1024;
const MAX_SESSIONS: usize = 64;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Configuration(BTreeMap<String, String>);

impl Configuration {
    fn capture(args: &ServeArgs) -> io::Result<Self> {
        let mut args = args.clone();
        args.internal_backend = false;
        args.standalone = false;
        let value = serde_json::to_value(args)?;
        let mut fields = BTreeMap::new();
        let object = value.as_object().ok_or_else(|| io::Error::other("Invalid serve configuration"))?;
        for (key, value) in object {
            fields.insert(key.clone(), value.to_string());
        }
        // Hash environment values rather than sending/logging potentially sensitive
        // Git configuration. Presence and value both participate in compatibility.
        for (key, value) in std::env::vars_os() {
            let key = key.to_string_lossy();
            if key.starts_with("XRAY_") || key.starts_with("GIT_") {
                fields.insert(format!("env:{key}"), digest(value.as_encoded_bytes()));
            }
        }
        let mut executable = File::open(std::env::current_exe()?)?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let size = executable.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            hash.update(&buffer[..size]);
        }
        fields.insert("executable".into(), format!("{:x}", hash.finalize()));
        Ok(Self(fields))
    }

    fn differences<'a>(&'a self, other: &'a Self) -> Vec<&'a str> {
        let mut keys: Vec<_> = self.0.keys().chain(other.0.keys()).map(String::as_str).collect();
        keys.sort_unstable();
        keys.dedup();
        keys.retain(|key| self.0.get(*key) != other.0.get(*key));
        keys
    }
}

#[derive(Serialize, Deserialize)]
struct Hello {
    protocol: u32,
    configuration: Configuration,
}

#[derive(Serialize, Deserialize)]
struct Welcome {
    protocol: u32,
    pid: u32,
    error: Option<String>,
    #[serde(default)]
    retry: bool,
}

#[derive(Default)]
struct SessionRegistry {
    active: usize,
    ever_accepted: bool,
    retiring: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Accepted,
    Conflict,
    Retry,
}

impl SessionRegistry {
    fn admit(&mut self, compatible: bool) -> Admission {
        if self.retiring {
            return Admission::Retry;
        }
        if compatible {
            self.active += 1;
            self.ever_accepted = true;
            return Admission::Accepted;
        }
        if self.active == 0 && self.ever_accepted {
            // Admission and retirement share a lock: a compatible connection
            // cannot slip in after an idle replacement has been requested.
            self.retiring = true;
            tracing::info!("Retiring idle workspace backend for configuration change");
            Admission::Retry
        } else {
            Admission::Conflict
        }
    }
}

struct ActiveSession<'a>(&'a Mutex<SessionRegistry>);

impl Drop for ActiveSession<'_> {
    fn drop(&mut self) {
        let mut sessions = self.0.lock().unwrap_or_else(|error| {
            tracing::warn!("Recovering poisoned backend lifecycle during session teardown");
            error.into_inner()
        });
        sessions.active -= 1;
    }
}

struct Workspace {
    runtime: PathBuf,
    key: String,
}

impl Workspace {
    fn new(dir: &str) -> io::Result<Self> {
        let runtime = crate::index_dir().join("backends");
        fs::create_dir_all(&runtime)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = fs::symlink_metadata(&runtime)?;
            if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Invalid backend runtime directory"));
            }
            fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        }
        let root = fs::canonicalize(dir)?.to_string_lossy().into_owned();
        #[cfg(windows)]
        let root = root.to_lowercase();
        Ok(Self { runtime, key: digest(root.as_bytes()) })
    }

    fn path(&self, suffix: &str) -> PathBuf {
        self.runtime.join(format!("{}.{}", self.key, suffix))
    }

    fn name(&self) -> io::Result<Name<'static>> {
        #[cfg(windows)]
        {
            let user_namespace = digest(self.runtime.as_os_str().as_encoded_bytes());
            format!("xray-{user_namespace}-{}", self.key).to_ns_name::<GenericNamespaced>()
        }
        #[cfg(unix)]
        {
            self.socket_path().to_fs_name::<GenericFilePath>()
        }
    }

    #[cfg(unix)]
    fn socket_path(&self) -> PathBuf {
        // Unix socket names have a much smaller limit than regular file paths.
        self.runtime.join(format!("{}.sock", &self.key[..self.key.len().min(24)]))
    }

    fn lock(&self, suffix: &str) -> io::Result<Option<File>> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(self.path(suffix))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    fn listener(&self) -> io::Result<Listener> {
        // The lifetime lock is already held. Only its owner may remove a stale
        // Unix socket; never overwrite another live listener.
        #[cfg(unix)]
        match fs::remove_file(self.socket_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        platform::restrict_listener(
            ListenerOptions::new().name(self.name()?).nonblocking(ListenerNonblockingMode::Accept),
        )?.create_sync()
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn normalize(args: &mut ServeArgs) -> io::Result<()> {
    args.dir = crate::clean_path(&fs::canonicalize(&args.dir).map_err(|error| {
        io::Error::new(error.kind(), format!("Cannot resolve workspace --dir {:?}: {error}", args.dir))
    })?.to_string_lossy());
    args.ext = args.ext.iter().flat_map(|s| s.split(','))
        .map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    args.ext.sort();
    args.ext.dedup();
    if args.ext.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "At least one indexed extension is required"));
    }
    args.rescan_interval_sec = args.rescan_interval_sec.max(10);
    Ok(())
}

pub(super) fn run(mut args: ServeArgs) -> io::Result<()> {
    // The existing roots-based mode can change workspaces after initialize.
    // Keep it private rather than pinning it to an accidental startup directory.
    if matches!(args.dir.as_str(), "." | "./" | ".\\") && !args.internal_backend {
        eprintln!("Xray: dynamic workspace mode uses a private server; pass an explicit --dir to share indexes.");
        serve::cmd_serve(args);
        return Ok(());
    }
    normalize(&mut args)?;
    let workspace = Workspace::new(&args.dir)?;
    if args.standalone {
        let _owner = workspace.lock("owner.lock")?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::AlreadyExists, "This workspace already has an index owner; close its connections before using --standalone")
        })?;
        serve::cmd_serve(args);
        return Ok(());
    }
    let configuration = Configuration::capture(&args)?;
    if args.internal_backend {
        run_backend(args, workspace, configuration)
    } else {
        let stream = connect_or_start(&args, &workspace, &configuration)?;
        proxy::run(stream, &args, &workspace, &configuration)
    }
}

fn connect_or_start(args: &ServeArgs, workspace: &Workspace, configuration: &Configuration) -> io::Result<Stream> {
    connect_or_start_until(args, workspace, configuration, Instant::now() + STARTUP_TIMEOUT)
}

fn connect_or_start_until(args: &ServeArgs, workspace: &Workspace, configuration: &Configuration, deadline: Instant) -> io::Result<Stream> {
    let mut launcher = None;
    let mut child: Option<Child> = None;
    let mut replacement_requested = false;
    loop {
        let connection = try_connect(workspace, configuration)?;
        if matches!(&connection, ConnectionAttempt::Retry) {
            replacement_requested = true;
        }
        if let ConnectionAttempt::Connected(stream) = connection {
            // Reap our child if it eventually exits while this proxy is still alive.
            if let Some(mut child) = child.take() {
                thread::spawn(move || {
                    if let Err(error) = child.wait() {
                        eprintln!("Xray: failed to wait for backend process: {error}");
                    }
                });
            }
            return Ok(stream);
        }
        let child_status = child.as_mut().map(Child::try_wait).transpose()?.flatten();
        if let Some(status) = child_status {
                if status.success() && replacement_requested {
                    child = None;
                    replacement_requested = false;
                    continue;
                }
                return Err(io::Error::other(format!(
                    "Workspace backend exited during startup ({status}). See {}", workspace.path("log").display()
                )));
            }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!(
                "Timed out connecting to workspace backend. A standalone server may own the workspace. See {}",
                workspace.path("log").display()
            )));
        }
        if launcher.is_none() {
            launcher = workspace.lock("launch.lock")?;
        }
        if launcher.is_some() && child.is_none()
            && let Some(owner) = workspace.lock("owner.lock")? {
                drop(owner);
                child = Some(spawn_backend(args, workspace)?);
            }
        thread::sleep(POLL);
    }
}

#[allow(clippy::disallowed_methods, reason = "Backend launcher applies its own detach and host-job flags in platform::spawn_backend")]
fn spawn_backend(args: &ServeArgs, workspace: &Workspace) -> io::Result<Child> {
    let log = OpenOptions::new().create(true).append(true).open(workspace.path("log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["serve", "--internal-backend", "--dir"]).arg(&args.dir)
        .arg("--ext").arg(args.ext.join(","))
        .arg("--debounce-ms").arg(args.debounce_ms.to_string())
        .arg("--log-level").arg(&args.log_level)
        .arg("--max-response-kb").arg(args.max_response_kb.to_string())
        .arg("--rescan-interval-sec").arg(args.rescan_interval_sec.to_string())
        .current_dir(&args.dir)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log));
    for (enabled, flag) in [
        (args.watch, "--watch"), (args.definitions, "--definitions"),
        (args.metrics, "--metrics"), (args.debug_log, "--debug-log"),
        (args.respect_git_exclude, "--respect-git-exclude"),
        (args.no_periodic_rescan, "--no-periodic-rescan"),
    ] {
        if enabled {
            command.arg(flag);
        }
    }
    platform::spawn_backend(&mut command).map_err(|error| io::Error::new(error.kind(), format!(
        "Cannot launch workspace backend: {error}"
    )))
}

fn retryable(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof | io::ErrorKind::WouldBlock)
        || cfg!(windows) && error.raw_os_error() == Some(231) // ERROR_PIPE_BUSY
}

enum ConnectionAttempt {
    Connected(Stream),
    Retry,
    Unavailable,
}

fn try_connect(workspace: &Workspace, configuration: &Configuration) -> io::Result<ConnectionAttempt> {
    let mut stream = match ConnectOptions::new().name(workspace.name()?)
        .wait_mode(ConnectWaitMode::Timeout(POLL)).nonblocking_stream(true).connect_sync() {
        Ok(stream) => stream,
        Err(error) if retryable(&error) => return Ok(ConnectionAttempt::Unavailable),
        Err(error) => return Err(error),
    };
    platform::verify_peer(&stream)?;
    let handshake = (|| {
        send_frame(&mut stream, &Hello { protocol: PROTOCOL, configuration: Configuration(configuration.0.clone()) })?;
        receive_frame::<Welcome>(&mut stream)
    })();
    let welcome = match handshake {
        Ok(welcome) => welcome,
        Err(error) if retryable(&error) => return Ok(ConnectionAttempt::Unavailable),
        Err(error) => return Err(error),
    };
    if welcome.protocol != PROTOCOL {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Incompatible workspace backend protocol"));
    }
    if welcome.retry {
        return Ok(ConnectionAttempt::Retry);
    }
    if let Some(error) = welcome.error {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, error));
    }
    stream.set_nonblocking(false)?;
    eprintln!("Xray: connected to workspace backend PID {}", welcome.pid);
    Ok(ConnectionAttempt::Connected(stream))
}

fn backend_idle_timeout() -> io::Result<Duration> {
    match std::env::var("XRAY_BACKEND_IDLE_SECONDS") {
        Err(std::env::VarError::NotPresent) => Ok(Duration::from_secs(60)),
        Ok(value) => value.parse::<u64>().ok().filter(|value| (1..=3600).contains(value))
            .map(Duration::from_secs).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput,
                "XRAY_BACKEND_IDLE_SECONDS must be an integer between 1 and 3600")),
        Err(error) => Err(io::Error::new(io::ErrorKind::InvalidInput, error)),
    }
}

fn run_backend(args: ServeArgs, workspace: Workspace, configuration: Configuration) -> io::Result<()> {
    let _owner = workspace.lock("owner.lock")?.ok_or_else(|| {
        io::Error::new(io::ErrorKind::AlreadyExists, "A backend already owns this workspace")
    })?;
    let idle_timeout = backend_idle_timeout()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&shutdown);
    ctrlc::set_handler(move || signal.store(true, Ordering::Release)).map_err(io::Error::other)?;
    let ctx = Arc::new(serve::build_context(args));
    let configuration = Arc::new(configuration);
    let request_gate = Arc::new(Mutex::new(()));
    let lifecycle = Arc::new(Mutex::new(SessionRegistry::default()));
    let listener = workspace.listener()?;
    let mut sessions: Vec<thread::JoinHandle<()>> = Vec::new();
    let mut idle_since = Instant::now();
    let mut accept_error = None;
    while !shutdown.load(Ordering::Acquire) {
        let mut i = 0;
        while i < sessions.len() {
            if sessions[i].is_finished() {
                if sessions.swap_remove(i).join().is_err() {
                    tracing::error!("MCP connection thread panicked");
                }
                idle_since = Instant::now();
            } else {
                i += 1;
            }
        }
        if lifecycle.lock().map_err(|_| io::Error::other("Backend lifecycle lock poisoned"))?.retiring {
            break;
        }
        match listener.accept() {
            Ok(stream) => {
                if sessions.len() >= MAX_SESSIONS {
                    tracing::warn!("Rejecting connection: workspace backend has {MAX_SESSIONS} sessions");
                    drop(stream);
                    continue;
                }
                let ctx = Arc::clone(&ctx);
                let configuration = Arc::clone(&configuration);
                let request_gate = Arc::clone(&request_gate);
                let lifecycle = Arc::clone(&lifecycle);
                sessions.push(thread::spawn(move || {
                    if let Err(error) = serve_connection(stream, &ctx, &configuration, &request_gate, &lifecycle) {
                        tracing::warn!(%error, "MCP backend connection closed with an error");
                    }
                }));
            }
            Err(error) if retryable(&error) => {}
            Err(error) => {
                accept_error = Some(error);
                break;
            }
        }
        if sessions.is_empty() && idle_since.elapsed() >= idle_timeout {
            break;
        }
        thread::sleep(POLL);
    }
    drop(listener);
    if lifecycle.lock().map_err(|_| io::Error::other("Backend lifecycle lock poisoned"))?.retiring {
        // No accepted sessions remain. Let bounded handshakes deliver their
        // retry replies before saving and releasing ownership for the next owner.
        for session in sessions {
            if session.join().is_err() {
                tracing::error!("MCP connection thread panicked during backend replacement");
            }
        }
    }
    let _requests = request_gate.lock().map_err(|_| io::Error::other("Shared request gate poisoned at shutdown"))?;
    server::save_indexes_on_shutdown(&ctx);
    tracing::info!("Workspace backend stopped");
    match accept_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn serve_connection(
    mut stream: Stream,
    ctx: &HandlerContext,
    configuration: &Configuration,
    gate: &Mutex<()>,
    lifecycle: &Mutex<SessionRegistry>,
) -> io::Result<()> {
    platform::verify_peer(&stream)?;
    stream.set_nonblocking(true)?;
    let hello: Hello = receive_frame(&mut stream)?;
    if hello.protocol != PROTOCOL {
        send_frame(&mut stream, &Welcome {
            protocol: PROTOCOL,
            pid: std::process::id(),
            error: Some("Incompatible workspace backend protocol. Restart all Xray connections.".to_string()),
            retry: false,
        })?;
        return Ok(());
    }
    let differences = configuration.differences(&hello.configuration);
    let admission = lifecycle.lock().map_err(|_| io::Error::other("Backend lifecycle lock poisoned"))?
        .admit(differences.is_empty());
    let _session = (admission == Admission::Accepted).then(|| ActiveSession(lifecycle));
    let error = match admission {
        Admission::Accepted => None,
        Admission::Conflict => Some(format!(
            "Workspace backend configuration differs: {}. Use the same configuration or restart all Xray connections.",
            differences.join(", "),
        )),
        Admission::Retry => Some("Idle workspace backend is restarting with a new configuration.".to_string()),
    };
    send_frame(&mut stream, &Welcome {
        protocol: PROTOCOL, pid: std::process::id(), error, retry: admission == Admission::Retry,
    })?;
    if admission != Admission::Accepted {
        return Ok(());
    }
    stream.set_nonblocking(false)?;
    let mut reader = BufReader::new(ClientInput::new(&stream));
    let mut writer = &stream;
    server::run_shared_session(ctx.clone(), &mut reader, &mut writer, gate);
    Ok(())
}

fn send_frame<T: Serialize>(stream: &mut Stream, message: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(message)?;
    send_payload(stream, &bytes)
}

fn send_payload(stream: &mut Stream, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_HANDSHAKE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Backend handshake exceeds size limit"));
    }
    let mut frame = (bytes.len() as u32).to_le_bytes().to_vec();
    frame.extend_from_slice(bytes);
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut offset = 0;
    while offset < frame.len() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Backend handshake write timed out"));
        }
        // Keep writes within the transport's 512-byte named-pipe buffer.
        let end = (offset + 512).min(frame.len());
        match stream.write(&frame[offset..end]) {
            // PIPE_NOWAIT byte pipes report a full write buffer as a successful
            // zero-byte write. The peer can drain it before the handshake deadline.
            #[cfg(windows)]
            Ok(0) => thread::sleep(POLL),
            #[cfg(not(windows))]
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(size) => offset += size,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn receive_frame<T: DeserializeOwned>(stream: &mut Stream) -> io::Result<T> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut length = [0; 4];
    read_until(stream, &mut length, deadline)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_HANDSHAKE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Backend handshake exceeds size limit"));
    }
    let mut bytes = vec![0; length];
    read_until(stream, &mut bytes, deadline)?;
    serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn read_until(stream: &mut Stream, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Backend handshake read timed out"));
        }
        match platform::read_handshake(stream, bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(size) => bytes = &mut bytes[size..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

const MAX_INPUT_CHUNK: usize = 64 * 1024;

// Named pipes cannot half-close. Input frames preserve stdin EOF without
// dropping the connection before the backend has written its final responses.
struct ClientInput<R> {
    reader: R,
    remaining: usize,
    ended: bool,
}

impl<R: Read> ClientInput<R> {
    fn new(reader: R) -> Self { Self { reader, remaining: 0, ended: false } }
}

impl<R: Read> Read for ClientInput<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() || self.ended { return Ok(0); }
        if self.remaining == 0 {
            let mut length = [0; 4];
            self.reader.read_exact(&mut length)?;
            self.remaining = u32::from_le_bytes(length) as usize;
            if self.remaining == 0 {
                self.ended = true;
                return Ok(0);
            }
            if self.remaining > MAX_INPUT_CHUNK {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "MCP input frame exceeds size limit"));
            }
        }
        let count = bytes.len().min(self.remaining);
        let size = self.reader.read(&mut bytes[..count])?;
        if size == 0 { return Err(io::ErrorKind::UnexpectedEof.into()); }
        self.remaining -= size;
        Ok(size)
    }
}

#[cfg(test)]
fn forward_input(reader: &mut impl Read, writer: &mut impl Write, closing: &AtomicBool) -> io::Result<()> {
    let mut bytes = [0; 8192];
    loop {
        let size = match reader.read(&mut bytes) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if size == 0 { closing.store(true, Ordering::Release); }
        writer.write_all(&(size as u32).to_le_bytes())?;
        writer.write_all(&bytes[..size])?;
        if size == 0 { return Ok(()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_registry_preserves_fresh_backend_for_its_launcher() {
        let mut lifecycle = SessionRegistry::default();
        assert_eq!(lifecycle.admit(false), Admission::Conflict);
        assert!(!lifecycle.retiring);
        assert_eq!(lifecycle.admit(true), Admission::Accepted);
    }

    #[test]
    fn shared_registry_never_retires_an_active_session() {
        let lifecycle = Mutex::new(SessionRegistry::default());
        assert_eq!(lifecycle.lock().unwrap().admit(true), Admission::Accepted);
        let session = ActiveSession(&lifecycle);
        assert_eq!(lifecycle.lock().unwrap().admit(false), Admission::Conflict);
        assert!(!lifecycle.lock().unwrap().retiring);
        drop(session);
        assert_eq!(lifecycle.lock().unwrap().active, 0);
        assert_eq!(lifecycle.lock().unwrap().admit(false), Admission::Retry);
        assert_eq!(lifecycle.lock().unwrap().admit(true), Admission::Retry);
        assert_eq!(lifecycle.lock().unwrap().active, 0);
    }

    #[test]
    fn shared_registry_admission_and_retirement_are_atomic() {
        for _ in 0..100 {
            let lifecycle = Arc::new(Mutex::new(SessionRegistry { ever_accepted: true, ..SessionRegistry::default() }));
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let first = Arc::clone(&lifecycle);
            let ready = Arc::clone(&barrier);
            let compatible = thread::spawn(move || {
                ready.wait();
                first.lock().unwrap().admit(true)
            });
            barrier.wait();
            let incompatible = lifecycle.lock().unwrap().admit(false);
            let compatible = compatible.join().unwrap();
            let state = lifecycle.lock().unwrap();
            match (compatible, incompatible) {
                (Admission::Accepted, Admission::Conflict) => {
                    assert_eq!(state.active, 1);
                    assert!(!state.retiring);
                }
                (Admission::Retry, Admission::Retry) => {
                    assert_eq!(state.active, 0);
                    assert!(state.retiring);
                }
                outcome => panic!("invalid admission outcome: {outcome:?}"),
            }
        }
    }


    #[test]
    fn shared_input_frames_preserve_eof_and_reject_oversized_chunks() {
        let source = "payload".repeat(5000);
        let closing = AtomicBool::new(false);
        let mut encoded = Vec::new();
        forward_input(&mut source.as_bytes(), &mut encoded, &closing).unwrap();
        assert!(closing.load(Ordering::Acquire));
        let mut reader = ClientInput::new(encoded.as_slice());
        let mut decoded = String::new();
        reader.read_to_string(&mut decoded).unwrap();
        assert_eq!(decoded, source);
        let oversized = ((MAX_INPUT_CHUNK + 1) as u32).to_le_bytes();
        let mut reader = ClientInput::new(oversized.as_slice());
        assert_eq!(reader.read(&mut [0; 1]).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let partial = [2, 0, 0, 0, b'x'];
        assert_eq!(ClientInput::new(partial.as_slice()).read_to_end(&mut Vec::new()).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }


    #[test]
    fn shared_handshake_waits_for_data_and_handles_backpressure() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = Workspace { runtime: temp.path().to_owned(), key: "handshake".into() };
        let listener = workspace.listener().unwrap();
        let name = workspace.name().unwrap();
        let server = thread::spawn(move || {
            let start = Instant::now();
            let mut connection = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if retryable(&error) && start.elapsed() < HANDSHAKE_TIMEOUT => thread::sleep(POLL),
                    Err(error) => panic!("accept: {error}"),
                }
            };
            connection.set_nonblocking(true).unwrap();
            let payload: String = receive_frame(&mut connection).unwrap();
            assert_eq!(payload.len(), 16384);
            thread::sleep(POLL * 2);
            send_frame(&mut connection, &payload).unwrap();
        });
        let mut client = ConnectOptions::new().name(name).nonblocking_stream(true).connect_sync().unwrap();
        thread::sleep(POLL * 2);
        let payload = "a".repeat(16384);
        send_frame(&mut client, &payload).unwrap();
        assert_eq!(receive_frame::<String>(&mut client).unwrap(), payload);
        server.join().unwrap();
    }


    #[test]
    fn shared_configuration_reports_only_different_keys() {
        let first = Configuration(BTreeMap::from([("ext".into(), "rs".into()), ("env:GIT_CONFIG_VALUE_0".into(), "hash-a".into())]));
        let second = Configuration(BTreeMap::from([("ext".into(), "rs".into()), ("env:GIT_CONFIG_VALUE_0".into(), "hash-b".into())]));
        assert_eq!(first.differences(&second), ["env:GIT_CONFIG_VALUE_0"]);
    }

    #[test]
    fn shared_configuration_normalizes_extensions() {
        let temp = tempfile::tempdir().unwrap();
        let mut args = ServeArgs { dir: temp.path().to_string_lossy().into_owned(),
            ext: vec!["RS, md".into(), "rs".into()], ..ServeArgs::default() };
        normalize(&mut args).unwrap();
        assert_eq!(args.ext, ["md", "rs"]);
    }
}
