use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use serde_json::{Value, json};

#[allow(clippy::disallowed_methods, reason = "Process-level cache tests need the public executable and native creation flags")]
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xray"));
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    command
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    content: PathBuf,
    definitions: PathBuf,
    debug: PathBuf,
    owned: Vec<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::Builder::new().prefix("cache467-smoke-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        let cache_dir = dirs::data_local_dir().unwrap().join("xray");
        fs::create_dir_all(&cache_dir).unwrap();
        let root_text = root.to_string_lossy();
        let prefix = code_xray::extract_semantic_prefix(&root);
        let base_hash = code_xray::stable_hash(&[root_text.as_bytes()]) as u32;
        let content_hash = code_xray::stable_hash(&[root_text.as_bytes(), b"rs"]) as u32;
        let definition_hash = code_xray::stable_hash(&[root_text.as_bytes(), b"rs", b"definitions"]) as u32;
        let path = |hash, suffix| cache_dir.join(format!("{prefix}_{hash:08x}.{suffix}"));
        let content = path(content_hash, "word-search");
        let definitions = path(definition_hash, "code-structure");
        let debug = path(base_hash, "debug.log");
        let mut owned = vec![
            content.clone(), definitions.clone(), debug.clone(),
            path(base_hash, "file-list"), path(base_hash, "git-history"),
        ];
        for index in [content.clone(), definitions.clone(), path(base_hash, "file-list")] {
            let mut meta = index.into_os_string();
            meta.push(".meta");
            owned.push(meta.into());
        }
        for file in &owned {
            assert!(!file.exists(), "fixture must not overwrite {}", file.display());
        }
        let fixture = Self { _temporary: temporary, root, content, definitions, debug, owned };
        fs::write(fixture.root.join("alpha.rs"), "pub fn cache467alpha() {}\n").unwrap();
        fs::write(fixture.root.join("beta.rs"), "pub fn cache467beta() {}\n").unwrap();
        for subcommand in ["content-index", "def-index"] {
            let output = command().arg(subcommand).arg("--dir").arg(&fixture.root).args(["--ext", "rs"])
                .output().unwrap();
            assert!(output.status.success(), "{subcommand}: {}", String::from_utf8_lossy(&output.stderr));
        }
        for file in [&fixture.content, &fixture.definitions] {
            assert_eq!(&fs::read(file).unwrap()[..4], b"XRYS");
        }
        fixture
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for path in &self.owned {
            if path.exists() {
                fs::remove_file(path).expect("remove owned cache smoke artifact");
            }
        }
    }
}

struct Server {
    child: Child,
    input: ChildStdin,
    replies: Receiver<Result<Value, String>>,
    next_id: u64,
}

impl Server {
    fn start(root: &Path) -> Self {
        let stderr = fs::File::create(root.join("stderr.log")).unwrap();
        let mut child = command().args(["serve", "--standalone", "--dir"]).arg(root)
            .args(["--ext", "rs", "--definitions", "--debug-log"])
            .env("XRAY_GUIDANCE_PREFIX", "0").current_dir(root)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let reply = line.map_err(|error| error.to_string())
                    .and_then(|line| serde_json::from_str(&line).map_err(|error| error.to_string()));
                if sender.send(reply).is_err() { break; }
            }
        });
        let mut server = Self { child, input, replies, next_id: 1 };
        server.request("initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "cache-snapshot-smoke", "version": "1"}}));
        writeln!(server.input, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
        server
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.input, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        self.input.flush().unwrap();
        loop {
            let reply = self.replies.recv_timeout(Duration::from_secs(60))
                .expect("cache smoke stdio response timeout").unwrap();
            if reply.get("id") == Some(&json!(id)) {
                assert!(reply.get("error").is_none(), "{reply}");
                return reply["result"].clone();
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        assert_ne!(result["isError"], json!(true), "{result}");
        serde_json::from_str(result["content"][0]["text"].as_str().expect("missing tool text")).unwrap()
    }

    fn wait_ready(&mut self) {
        let started = Instant::now();
        loop {
            let info = self.call("xray_info", json!({}));
            let indexes = info["indexes"].as_array().expect("missing indexes");
            let mut ready = true;
            for kind in ["content", "definition"] {
                let index = indexes.iter().find(|index| index["type"] == kind)
                    .unwrap_or_else(|| panic!("missing {kind} index: {info}"));
                if let Some(status) = index.get("status") {
                    if status == "building" {
                        ready = false;
                    } else {
                        assert_eq!(status, "ready", "{info}");
                    }
                }
            }
            if ready { return; }
            assert!(started.elapsed() < Duration::from_secs(60), "cache readiness timeout: {info}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn cache_467_public_cli_and_restarted_mcp_reuse_sharded_caches() {
    let fixture = Fixture::new();
    let content = fs::read(&fixture.content).unwrap();
    let definitions = fs::read(&fixture.definitions).unwrap();
    for (token, expected, excluded) in [
        ("cache467alpha", "alpha.rs", "beta.rs"),
        ("cache467beta", "beta.rs", "alpha.rs"),
    ] {
        let output = command().args(["grep", token, "--dir"]).arg(&fixture.root)
            .args(["--ext", "rs", "--exact"]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(expected), "{text}");
        assert!(!text.contains(excluded), "{text}");
    }
    let mut server = Server::start(&fixture.root);
    server.wait_ready();
    for (token, expected) in [("cache467alpha", "alpha.rs"), ("cache467beta", "beta.rs")] {
        let grep = server.call("xray_grep", json!({"terms": [token], "substring": false}));
        let files = grep["files"].as_array().unwrap();
        assert_eq!(files.len(), 1, "{grep}");
        assert!(files[0]["path"].as_str().unwrap().ends_with(expected), "{grep}");
        let defs = server.call("xray_definitions", json!({"name": [token], "exactNameOnly": true}));
        let entries = defs["definitions"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "{defs}");
        assert_eq!(entries[0]["name"], token);
        assert!(entries[0]["file"].as_str().unwrap().ends_with(expected), "{defs}");
    }
    let empty = server.call("xray_grep", json!({"terms": ["cache467absent"], "substring": false}));
    assert!(empty["files"].as_array().unwrap().is_empty(), "{empty}");
    let empty = server.call("xray_definitions", json!({"name": ["cache467absent"], "exactNameOnly": true}));
    assert!(empty["definitions"].as_array().unwrap().is_empty(), "{empty}");
    drop(server);
    assert_eq!(fs::read(&fixture.content).unwrap(), content, "content cache must not be rebuilt");
    assert_eq!(fs::read(&fixture.definitions).unwrap(), definitions, "definition cache must not be rebuilt");
    let log = fs::read_to_string(&fixture.debug).unwrap();
    for label in ["content-index", "definition-index"] {
        assert!(log.lines().any(|line| line.contains("| indexLoad |")
            && line.contains(&format!("indexLabel={label}")) && line.contains("indexStorage=lz4-sharded")), "{log}");
        assert!(!log.lines().any(|line| line.contains("| indexSave |")
            && line.contains(&format!("indexLabel={label}"))), "unexpected rebuild: {log}");
    }
}
