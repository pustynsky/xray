#![allow(clippy::disallowed_methods, reason = "Process-level tests exercise native creation flags and stdio directly")]

use std::collections::{BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use serde_json::{Value, json};

struct Server {
    child: Child,
    input: ChildStdin,
    replies: Receiver<Result<Value, String>>,
    next_id: u64,
    max_seen: usize,
}

impl Server {
    fn start(root: &std::path::Path, limit: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_xray"))
            .args(["serve", "--standalone", "--dir"]).arg(root).args(["--ext", "rs", "--definitions"])
            .current_dir(root)
            .env("XRAY_TRANSPORT_MAX_BYTES", limit)
            .env("XRAY_CHECK_UPDATES", "0")
            .env("XRAY_GUIDANCE_PREFIX", "1")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().unwrap();
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
        let mut server = Self { child, input, replies, next_id: 1, max_seen: 0 };
        server.request("initialize", json!({"protocolVersion": "2025-03-26",
            "capabilities": {}, "clientInfo": {"name": "delivery-smoke", "version": "1"}}));
        writeln!(server.input, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
        server
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.input, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        self.input.flush().unwrap();
        loop {
            let reply = self.replies.recv_timeout(Duration::from_secs(60)).expect("stdio response timeout").unwrap();
            if reply.get("id") == Some(&json!(id)) {
                assert!(reply.get("error").is_none(), "{reply}");
                return reply["result"].clone();
            }
        }
    }

    fn wait_for_definitions(&mut self, limit: usize) {
        let started = Instant::now();
        loop {
            let info = self.call("xray_info", json!({}), limit, false);
            let indexes = info["indexes"].as_array().expect("missing indexes");
            let definition = indexes.iter().find(|index| index["type"] == "definition")
                .unwrap_or_else(|| panic!("missing definition index: {info}"));
            if definition.get("status").is_none() {
                return;
            }
            assert_eq!(definition["status"], "building", "{info}");
            assert!(started.elapsed() < Duration::from_secs(60), "definition index timeout: {info}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn call(&mut self, tool: &str, args: Value, limit: usize, error: bool) -> Value {
        let result = self.request("tools/call", json!({"name": tool, "arguments": args}));
        assert_eq!(result["isError"].as_bool().unwrap_or(false), error, "{result}");
        let bytes: usize = result["content"].as_array().unwrap().iter()
            .map(|content| content["text"].as_str().unwrap().len()).sum();
        self.max_seen = self.max_seen.max(bytes);
        assert!(bytes <= limit, "{bytes} exceeds {limit}");
        let text = result["content"][0]["text"].as_str().unwrap();
        let json = text.split_once("\n\n").map(|(_, suffix)| suffix).unwrap_or(text);
        let output: Value = serde_json::from_str(json).unwrap();
        if let Some(measured) = output["summary"]["responseBytes"].as_u64() {
            assert_eq!(measured, bytes as u64);
        }
        output
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn collect_fragment(entry: &Value, source: &[String], seen: &mut BTreeSet<usize>, pending: &mut VecDeque<Value>) {
    let start = entry["bodyStartLine"].as_u64().unwrap() as usize;
    let end = entry["bodyEndLine"].as_u64().unwrap() as usize;
    assert_eq!(entry["body"], json!(source[start - 1..end]));
    for line in start..=end { assert!(seen.insert(line), "duplicate line {line}"); }
    assert_eq!(entry["bodyComplete"], start == 2 && end == source.len());
    for field in ["bodyRead", "bodySourceHash", "bodyFile", "bodyDefinitionStartLine",
        "bodyDefinitionEndLine", "bodyAvailableStartLine", "bodyRequestedStartLine",
        "bodyRequestedEndLine", "bodyExplicitRange"]
    {
        assert!(entry.get(field).is_none(), "leaked {field}");
    }
    for field in ["beforeArgs", "nextArgs"] {
        if let Some(args) = entry["bodyContinuation"].get(field) {
            assert_eq!(entry["bodyContinuation"]["tool"], "xray_definitions");
            assert!(args["bodyTarget"]["sourceHash"].as_str().unwrap().starts_with("sha256:"));
            pending.push_back(args.clone());
        }
    }
}

#[test]
fn body_delivery_stdio_env_budget_anchor_and_continuation() {
    let temp = tempfile::tempdir().unwrap();
    let root = code_xray::canonicalize_test_root(temp.path());
    let path = root.join("sample.rs");
    let mut source = vec!["fn small() {}".to_string(), "fn deliver() {".to_string()];
    source.extend((3..=182).map(|line| format!("    // line {line}: {}", "\u{03bb}\"\\".repeat(40))));
    source.push("}".to_string());
    std::fs::write(&path, source.join("\n")).unwrap();
    let limit = 8192;
    let mut server = Server::start(&root, &limit.to_string());
    server.wait_for_definitions(limit);
    let first = server.call("xray_definitions", json!({"file": [path], "containsLine": 90,
        "includeBody": true, "maxBodyLines": 0, "maxTotalBodyLines": 0}), limit, false);
    let anchor = &first["containingDefinitions"][0];
    assert_eq!(anchor["bodyAnchorLine"], 90, "{first}");
    assert_eq!(anchor["bodyAnchorVisible"], true);
    assert_eq!(anchor["bodyComplete"], false);
    assert_eq!(anchor["bodyRangeComplete"], false);
    assert!(anchor["bodyContinuation"]["beforeArgs"].is_object());
    assert!(anchor["bodyContinuation"]["nextArgs"].is_object());
    let mut seen = BTreeSet::new();
    let mut pending = VecDeque::new();
    collect_fragment(anchor, &source, &mut seen, &mut pending);
    let mut pages = 1;
    while let Some(args) = pending.pop_front() {
        assert!(pages < source.len(), "continuation did not advance");
        let output = server.call("xray_definitions", args.clone(), limit, false);
        assert_eq!(output["definitions"].as_array().unwrap().len(), 1);
        let entry = &output["definitions"][0];
        assert_eq!(entry["bodyRangeComplete"], entry["bodyStartLine"] == args["bodyLineStart"]
            && entry["bodyEndLine"] == args["bodyLineEnd"]);
        collect_fragment(entry, &source, &mut seen, &mut pending);
        pages += 1;
    }
    assert_eq!(seen, (2..=source.len()).collect());
    let small = server.call("xray_definitions", json!({"file": [path], "name": ["small"],
        "includeBody": true}), limit, false);
    let entry = &small["definitions"][0];
    assert_eq!(entry["body"], json!([source[0]]));
    assert_eq!(entry["bodyComplete"], true);
    assert_eq!(entry["bodyRangeComplete"], true);
    assert!(entry.get("bodyContinuation").is_none());
    println!("stdio: {pages} body pages, {} unique lines, peak {} / {limit} UTF-8 text bytes", seen.len(), server.max_seen);
}

#[test]
fn body_delivery_stdio_invalid_env_is_structured() {
    let temp = tempfile::tempdir().unwrap();
    let root = code_xray::canonicalize_test_root(temp.path());
    let mut server = Server::start(&root, "511");
    let output = server.call("xray_help", json!({}), 512, true);
    assert_eq!(output["error"]["code"], "invalid_transport_max_bytes");
    assert_eq!(output["error"]["parameter"], "XRAY_TRANSPORT_MAX_BYTES");
    assert_eq!(output["resultStatus"]["complete"], false);
}
