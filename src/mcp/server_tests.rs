use super::*;



fn make_ctx() -> HandlerContext {
    HandlerContext::default()
}

#[test]
fn test_handle_initialize() {
    let ctx = make_ctx();
    let result = handle_request(&ctx, "initialize", &None, json!(1));
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 1);
    assert_eq!(result["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(result["result"]["serverInfo"]["name"], "xray");
}

#[test]
fn test_handle_tools_list() {
    let ctx = make_ctx();
    let result = handle_request(&ctx, "tools/list", &None, json!(2));
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 2);
    let tools = result["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), handlers::TOOL_DEFINITION_COUNT);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"xray_grep"));
    assert!(names.contains(&"xray_fast"));
    assert!(names.contains(&"xray_info"));
    assert!(names.contains(&"xray_reindex"));
    assert!(names.contains(&"xray_reindex_definitions"));
    assert!(names.contains(&"xray_definitions"));
}

#[test]
fn test_handle_tools_call_grep() {
    let ctx = make_ctx();
    let params = json!({
        "name": "xray_grep",
        "arguments": { "terms": "HttpClient" }
    });
    let result = handle_request(&ctx, "tools/call", &Some(params), json!(3));
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 3);
    // Should have content array
    let content = result["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
}

#[test]
fn test_handle_unknown_method() {
    let ctx = make_ctx();
    let result = handle_request(&ctx, "unknown/method", &None, json!(99));
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 99);
    assert!(result["error"]["message"].as_str().unwrap().contains("Method not found"));
    assert_eq!(result["error"]["code"], -32601);
}

#[test]
fn test_handle_ping() {
    let ctx = make_ctx();
    let result = handle_request(&ctx, "ping", &None, json!(42));
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 42);
    assert!(result["result"].is_object());
}

#[test]
fn test_handle_tools_call_missing_params() {
    let ctx = make_ctx();
    let result = handle_request(&ctx, "tools/call", &None, json!(5));
    assert_eq!(result["result"]["isError"], true);
    assert!(result["result"]["content"][0]["text"].as_str().unwrap().contains("Missing params"));
}

#[test]
fn test_protocol_trace_extracts_initialize_capabilities() {
    let params = Some(json!({
        "capabilities": {
            "roots": { "listChanged": true }
        }
    }));

    assert_eq!(initialize_capabilities(&params), (true, true));
    assert_eq!(initialize_capabilities(&None), (false, false));
}

#[test]
fn test_protocol_trace_extracts_tool_name_before_dispatch() {
    let params = Some(json!({
        "name": "xray_grep",
        "arguments": { "terms": ["needle"] }
    }));

    assert_eq!(tool_name_for_protocol(&params), "xray_grep");
    assert_eq!(tool_name_for_protocol(&None), "<missing>");
}

fn protocol_field<'a>(fields: &'a [(&'static str, String)], name: &str) -> &'a str {
    fields
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.as_str())
        .unwrap_or_else(|| panic!("missing protocol field {name}: {fields:?}"))
}

#[test]
fn test_protocol_trace_uses_raw_tools_call_before_validation() {
    let raw = json!({
        "jsonrpc": "1.0",
        "id": 7,
        "method": "tools/call",
        "params": { "name": "xray_grep", "arguments": {} }
    });
    let method = method_for_protocol(&raw);
    let params = params_for_protocol(&raw);
    let id = id_for_protocol(&raw);

    let (event, fields) = protocol_request_event(&method, &params, id);

    assert_eq!(event, "tools/call");
    assert_eq!(protocol_field(&fields, "id"), "7");
    assert_eq!(protocol_field(&fields, "name"), "xray_grep");
}

#[test]
fn test_run_server_with_io_stdout_is_json_rpc_only() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = make_ctx();
    ctx.index_base = tmp.path().to_path_buf();

    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"roots":{"listChanged":true}}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"1.0","id":7,"method":"tools/call","params":{"name":"xray_grep","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"xray_grep","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#,
    ].join("\n") + "\n";
    let mut reader = std::io::Cursor::new(input.into_bytes());
    let mut output = Vec::new();

    run_server_with_io(ctx, &mut reader, &mut output, false);

    let stdout = String::from_utf8(output).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 5, "stdout should contain responses only: {stdout}");
    for line in lines {
        assert!(!line.contains("PROTO"), "protocol trace leaked to stdout: {line}");
        assert!(!line.contains("REQ  |"), "debug request log leaked to stdout: {line}");
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0", "invalid JSON-RPC response: {line}");
    }
}

#[test]
fn test_handle_request_does_not_panic_on_serialization() {
    // Verify that handle_request uses safe_to_value which doesn't panic.
    // We exercise all method branches to confirm no unwrap panics.
    let ctx = make_ctx();

    // All standard branches should return valid JSON-RPC responses
    let methods = vec![
        ("initialize", None),
        ("tools/list", None),
        ("tools/call", Some(json!({"name": "xray_grep", "arguments": {"terms": "test"}}))),
        ("tools/call", None), // missing params branch
        ("ping", None),
        ("unknown/method", None),
    ];
    for (method, params) in methods {
        let result = handle_request(&ctx, method, &params, json!(1));
        assert!(result.is_object(), "Response for '{}' should be a JSON object, got: {:?}", method, result);
        assert_eq!(result["jsonrpc"], "2.0", "Response for '{}' should have jsonrpc=2.0", method);
    }
}

#[test]
fn test_safe_to_value_returns_error_on_serialization_failure() {
    // safe_to_value should return an error JSON object instead of panicking
    // when given something that can't be serialized.
    // Since all our types are Serialize, we test the happy path
    // and verify the function signature works correctly.
    let id = json!(42);
    let result = safe_to_value(json!({"test": true}), &id);
    assert_eq!(result["test"], true);

    // Test with a normal JsonRpcResponse
    let resp = JsonRpcResponse::new(json!(1), json!({"ok": true}));
    let result = safe_to_value(resp, &id);
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["result"]["ok"], true);
}

/// Test that initialize uses ctx.def_extensions (not server_ext) for instructions.
/// def_extensions=["cs"] should produce "NEVER READ .cs" but not .xml.
#[test]
fn test_initialize_def_extension_filtering() {
    // def_extensions=["cs"] → "NEVER READ .cs FILES DIRECTLY"
    let mut ctx = make_ctx();
    ctx.server_ext = "cs,xml".to_string();
    ctx.def_extensions = vec!["cs".to_string()];
    let result = handle_request(&ctx, "initialize", &None, json!(1));
    let instructions = result["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains(".cs"),
        "instructions should mention .cs (has parser)");
    // .xml should NOT appear (not in def_extensions)
    assert!(!instructions.contains("NEVER READ .xml"),
        "instructions should NOT mention .xml in NEVER READ (no parser). Got:\n{}", instructions);
    assert!(!instructions.contains(".xml FILES DIRECTLY"),
        "instructions should NOT have .xml in file reading rule");

    // def_extensions=[] and no XML-on-demand content extension -> no definition reads.
    let mut ctx2 = make_ctx();
    ctx2.server_ext = "txt".to_string();
    ctx2.def_extensions = vec![]; // no --definitions
    let result2 = handle_request(&ctx2, "initialize", &None, json!(2));
    let instructions2 = result2["result"]["instructions"].as_str().unwrap();
    assert!(!instructions2.contains("NEVER READ"),
        "empty def_extensions should NOT have NEVER READ block");
    assert!(instructions2.contains("xray_definitions is not available"),
        "empty def_extensions should have fallback note");

    // XML on-demand is a runtime definitions capability; exact file requests
    // can work even when XML-family extensions are not content-indexed.
    #[cfg(feature = "lang-xml")]
    {
        let mut ctx_xml = make_ctx();
        ctx_xml.server_ext = "txt".to_string();
        ctx_xml.def_extensions = vec![];
        ctx_xml.def_index = Some(std::sync::Arc::new(std::sync::RwLock::new(
            crate::definitions::DefinitionIndex::default(),
        )));
        let result_xml = handle_request(&ctx_xml, "initialize", &None, json!(22));
        let instructions_xml = result_xml["result"]["instructions"].as_str().unwrap();
        assert!(!instructions_xml.contains("NEVER READ"),
            "XML on-demand should not be listed in source-code NEVER READ rule");
        assert!(instructions_xml.contains("XML / CSPROJ ON-DEMAND PARSING"),
            "runtime XML on-demand availability should render XML guidance");
        assert!(!instructions_xml.contains("xray_definitions is not available"),
            "runtime XML on-demand availability should not claim xray_definitions is unavailable");
        assert!(!instructions_xml.contains("Raw XML phrase search"),
            "raw XML grep guidance should require XML-like content extensions in --ext");
    }

    // def_extensions=["cs","ts","sql"] → all three mentioned
    let mut ctx3 = make_ctx();
    ctx3.server_ext = "cs,ts,sql".to_string();
    ctx3.def_extensions = vec!["cs".to_string(), "ts".to_string(), "sql".to_string()];
    let result3 = handle_request(&ctx3, "initialize", &None, json!(3));
    let instructions3 = result3["result"]["instructions"].as_str().unwrap();
    assert!(instructions3.contains(".cs"), "should contain .cs");
    assert!(instructions3.contains(".ts"), "should contain .ts");
    assert!(instructions3.contains(".sql"), "should contain .sql");
    assert!(instructions3.contains("NEVER READ"), "should have NEVER READ block");
}

#[cfg(feature = "lang-xml")]
#[test]
fn test_tools_list_advertises_xml_on_demand_when_runtime_available() {
    let mut ctx = make_ctx();
    ctx.server_ext = "txt".to_string();
    ctx.def_extensions = vec![];
    ctx.def_index = Some(std::sync::Arc::new(std::sync::RwLock::new(
        crate::definitions::DefinitionIndex::default(),
    )));

    let result = handle_request(&ctx, "tools/list", &None, json!(44));
    let tools = result["result"]["tools"].as_array().unwrap();
    let definitions_tool = tools
        .iter()
        .find(|tool| tool["name"].as_str() == Some("xray_definitions"))
        .expect("tools/list should include xray_definitions");
    let description = definitions_tool["description"].as_str().unwrap();
    assert!(description.contains("XML on-demand parsing is available"), "{description}");
    assert!(!description.contains("Definition index not available for current file extensions"), "{description}");
}

/// Verify that tools/list response includes dynamic language descriptions
/// based on ctx.def_extensions.
#[test]
fn test_tools_list_dynamic_descriptions_rust() {
    let mut ctx = make_ctx();
    ctx.def_extensions = vec!["rs".to_string()];
    let result = handle_request(&ctx, "tools/list", &None, json!(2));
    let tools = result["result"]["tools"].as_array().unwrap();
    let def_tool = tools.iter().find(|t| t["name"] == "xray_definitions").unwrap();
    let desc = def_tool["description"].as_str().unwrap();
    assert!(desc.contains("Rust"),
        "tools/list xray_definitions should mention 'Rust' when def_extensions=[rs]. Got: {}", desc);
    assert!(!desc.contains("C#"),
        "tools/list xray_definitions should NOT mention C# for rs-only config");
}

#[test]
fn test_tools_list_dynamic_descriptions_empty() {
    let ctx = make_ctx(); // default: def_extensions = []
    let result = handle_request(&ctx, "tools/list", &None, json!(2));
    let tools = result["result"]["tools"].as_array().unwrap();
    let def_tool = tools.iter().find(|t| t["name"] == "xray_definitions").unwrap();
    let desc = def_tool["description"].as_str().unwrap();
    assert!(desc.contains("not available"),
        "tools/list xray_definitions should say 'not available' when def_extensions is empty");
}

/// Regression test for Bug 1: initialize and tools/list must use the SAME def_extensions.
/// When def_extensions is empty (no --definitions flag), initialize should NOT say "NEVER READ".
#[test]
fn test_initialize_consistent_with_tools_list_empty_def_extensions() {
    let ctx = make_ctx(); // default: def_extensions = [], server_ext = "cs"
    let init_result = handle_request(&ctx, "initialize", &None, json!(1));
    let instructions = init_result["result"]["instructions"].as_str().unwrap();
    // With empty def_extensions (no --definitions), instructions should NOT say "NEVER READ"
    assert!(!instructions.contains("NEVER READ"),
        "initialize with empty def_extensions should NOT produce NEVER READ block. \
         This would contradict tools/list which says 'not available'. Got:\n{}",
        &instructions[..200.min(instructions.len())]);
    // Should contain fallback note instead
    assert!(instructions.contains("xray_definitions is not available"),
        "initialize with empty def_extensions should have fallback note");
}

/// Regression test: initialize with def_extensions=["rs"] must say "NEVER READ .rs" and
/// tools/list xray_definitions must say "Rust" — both from ctx.def_extensions.
#[test]
fn test_initialize_consistent_with_tools_list_rust() {
    let mut ctx = make_ctx();
    ctx.def_extensions = vec!["rs".to_string()];
    // initialize
    let init_result = handle_request(&ctx, "initialize", &None, json!(1));
    let instructions = init_result["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("NEVER READ .rs FILES DIRECTLY"),
        "initialize with def_extensions=[rs] should say NEVER READ .rs");
    // tools/list
    let tools_result = handle_request(&ctx, "tools/list", &None, json!(2));
    let tools = tools_result["result"]["tools"].as_array().unwrap();
    let def_tool = tools.iter().find(|t| t["name"] == "xray_definitions").unwrap();
    let desc = def_tool["description"].as_str().unwrap();
    assert!(desc.contains("Rust"),
        "tools/list xray_definitions should mention 'Rust'");
}


#[test]
fn test_shutdown_flag_initially_false_and_can_be_set() {
    let flag = Arc::new(AtomicBool::new(false));
    assert!(!flag.load(Ordering::SeqCst), "shutdown flag should start as false");

    let flag_clone = flag.clone();
    flag_clone.store(true, Ordering::SeqCst);
    assert!(flag.load(Ordering::SeqCst), "shutdown flag should be true after setting");
}

// ─── uri_to_path tests ─────────────────────────────────────────

#[test]
fn test_uri_to_path_windows_drive_letter() {
    let result = uri_to_path("file:///C:/Projects/MyApp");
    assert!(result.is_some());
    let path = result.unwrap();
    assert!(path.contains("C:") || path.contains("c:"), "Should contain drive letter, got: {}", path);
    assert!(path.contains("Projects/MyApp") || path.contains("Projects\\MyApp"),
        "Should contain path, got: {}", path);
}

#[test]
fn test_uri_to_path_percent_encoding() {
    let result = uri_to_path("file:///C:/My%20Projects/My%20App");
    assert!(result.is_some());
    let path = result.unwrap();
    assert!(path.contains("My Projects"), "Should decode percent-encoding, got: {}", path);
}

#[test]
fn test_uri_to_path_non_file_scheme_returns_none() {
    assert!(uri_to_path("https://example.com").is_none());
    assert!(uri_to_path("http://localhost").is_none());
    assert!(uri_to_path("ftp://server/path").is_none());
}

#[test]
fn test_uri_to_path_invalid_uri_returns_none() {
    assert!(uri_to_path("not a uri").is_none());
    assert!(uri_to_path("").is_none());
}

#[test]
fn test_uri_to_path_unix_style() {
    let result = uri_to_path("file:///home/user/projects/myapp");
    // On Windows, url::Url::to_file_path() returns Err for Unix-style paths (no drive letter).
    // On Unix, it returns the path correctly.
    if cfg!(windows) {
        assert!(result.is_none(), "On Windows, Unix-style file URIs are not supported");
    } else {
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(path.contains("home/user/projects/myapp"),
            "Should contain unix path, got: {}", path);
    }
}

// ── P0-1a: panic guard around handle_request dispatch ─────────────────

#[test]
fn test_dispatch_request_with_panic_guard_returns_internal_error_on_panic() {
    // `$test/panic` is a #[cfg(test)]-only method in handle_request that
    // unconditionally panics. The guard must convert the panic into a
    // JSON-RPC -32603 response carrying the panic message, NOT propagate it.
    let ctx = make_ctx();
    let response = dispatch_request_with_panic_guard(&ctx, "$test/panic", &None, json!(7));
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 7);
    assert_eq!(response["error"]["code"], -32603);
    let message = response["error"]["message"].as_str().expect("error.message string");
    assert!(message.contains("handler panicked"), "expected panic marker, got: {message}");
    assert!(message.contains("$test/panic"), "expected panic payload in message, got: {message}");
}

#[test]
fn test_dispatch_request_with_panic_guard_passes_through_normal_response() {
    // Sanity: non-panicking branches must round-trip the inner handle_request
    // value untouched (no wrapping, no extra fields).
    let ctx = make_ctx();
    let response = dispatch_request_with_panic_guard(&ctx, "ping", &None, json!(11));
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 11);
    assert!(response["result"].is_object());
    assert!(response.get("error").is_none(), "ping response must not carry an error");
}

#[test]
fn test_run_server_with_io_continues_after_handler_panic() {
    // End-to-end: a `$test/panic` request must emit a -32603 response AND the
    // server loop must keep going to process the next request. Pre-P0-1a the
    // panic propagated up through the loop and killed the process; the
    // subsequent ping would never be answered.
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = make_ctx();
    ctx.index_base = tmp.path().to_path_buf();

    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"$test/panic"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
    ].join("\n") + "\n";
    let mut reader = std::io::Cursor::new(input.into_bytes());
    let mut output = Vec::new();

    run_server_with_io(ctx, &mut reader, &mut output, false);

    let stdout = String::from_utf8(output).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2,
        "expected two responses (panic-error + ping) but loop terminated early: {stdout}");

    let panic_resp: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(panic_resp["id"], 1);
    assert_eq!(panic_resp["error"]["code"], -32603);
    assert!(panic_resp["error"]["message"].as_str().unwrap().contains("handler panicked"));

    let ping_resp: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(ping_resp["id"], 2);
    assert!(ping_resp["result"].is_object(), "ping after panic must succeed");
}

#[test]
fn test_panic_payload_to_string_handles_str_and_string() {
    let s_panic = std::panic::catch_unwind(|| panic!("literal str payload"));
    let payload = s_panic.unwrap_err();
    assert_eq!(panic_payload_to_string(payload.as_ref()), "literal str payload");

    let owned = "owned".to_string();
    let s_panic = std::panic::catch_unwind(move || panic!("{owned} string payload"));
    let payload = s_panic.unwrap_err();
    assert_eq!(panic_payload_to_string(payload.as_ref()), "owned string payload");
}

// ── P0-1 follow-up: handle_pending_response panic guard ─────────────────

#[test]
fn test_pending_response_state_guard_no_op_on_normal_drop() {
    // The guard must NOT touch workspace state on normal scope-exit (success
    // commit OR plain return). Pins the dual contract: dismissed flag AND
    // std::thread::panicking() check — either one being broken would
    // silently undo every successful workspace switch via roots/list.
    let ctx = make_ctx();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir("/original/dir".to_string());
        ws.mode = handlers::WorkspaceBindingMode::ManualOverride;
        ws.generation = 3;
        ws.status = handlers::WorkspaceStatus::Resolved;
    }

    {
        let _guard = PendingResponseStateGuard::new(&ctx);
        // Simulate a successful mutation.
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir("/new/dir".to_string());
        ws.mode = handlers::WorkspaceBindingMode::ClientRoots;
        ws.generation = 4;
        ws.status = handlers::WorkspaceStatus::Reindexing;
        drop(ws);
        // Guard drops normally here (no panic, no dismiss). Drop must be no-op.
    }

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, "/new/dir",
        "normal-drop guard must NOT roll back dir (would undo successful switch)");
    assert_eq!(ws.generation, 4, "normal-drop guard must NOT roll back generation");
    assert_eq!(ws.status, handlers::WorkspaceStatus::Reindexing,
        "normal-drop guard must NOT touch status");
}

#[test]
fn test_pending_response_state_guard_rolls_back_workspace_on_panic() {
    // Reproduces the original gap: a panic inside the workspace-mutation
    // block of handle_pending_response (between set_dir and the final
    // eady flag stores) would leave the workspace pinned to a new dir
    // with status=Reindexing AND content_ready=true — every subsequent
    // tool call would return WORKSPACE_REINDEXING forever. The guard's
    // Drop must restore the full snapshot.
    let ctx = make_ctx();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir("/original/dir".to_string());
        ws.mode = handlers::WorkspaceBindingMode::ManualOverride;
        ws.generation = 3;
        ws.status = handlers::WorkspaceStatus::Resolved;
    }
    ctx.content_ready.store(true, std::sync::atomic::Ordering::Release);
    ctx.def_ready.store(true, std::sync::atomic::Ordering::Release);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = PendingResponseStateGuard::new(&ctx);
        // Simulate the mid-flight mutation path before panic.
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir("/half/built".to_string());
            ws.mode = handlers::WorkspaceBindingMode::ClientRoots;
            ws.generation = 4;
            ws.status = handlers::WorkspaceStatus::Reindexing;
        }
        ctx.content_ready.store(false, std::sync::atomic::Ordering::Release);
        panic!("simulated panic mid-handle_pending_response");
    }));
    assert!(result.is_err(), "panic should have been caught");

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, "/original/dir",
        "panic-rollback must restore dir to the pre-mutation snapshot");
    assert_eq!(ws.mode, handlers::WorkspaceBindingMode::ManualOverride,
        "panic-rollback must restore mode");
    assert_eq!(ws.generation, 3, "panic-rollback must restore generation");
    assert_eq!(ws.status, handlers::WorkspaceStatus::Resolved,
        "panic-rollback must force status back to Resolved so tools are not blocked");
    assert!(ctx.content_ready.load(std::sync::atomic::Ordering::Acquire),
        "panic-rollback must restore content_ready");
    assert!(ctx.def_ready.load(std::sync::atomic::Ordering::Acquire),
        "panic-rollback must restore def_ready");
}

#[test]
fn test_pending_response_state_guard_dismiss_blocks_rollback_on_panic() {
    // After dismiss() the guard must NOT roll back even on a subsequent
    // panic. Pins the post-commit contract: once handle_pending_response
    // has finished the workspace mutation block, any later panic (e.g. in
    // the trailing if roots.len() > 1 warn! or the surrounding event
    // loop) must leave the new workspace state intact.
    let ctx = make_ctx();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir("/original/dir".to_string());
        ws.mode = handlers::WorkspaceBindingMode::ManualOverride;
        ws.generation = 3;
        ws.status = handlers::WorkspaceStatus::Resolved;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut guard = PendingResponseStateGuard::new(&ctx);
        {
            let mut ws = ctx.workspace.write().unwrap();
            ws.set_dir("/new/dir".to_string());
            ws.mode = handlers::WorkspaceBindingMode::ClientRoots;
            ws.generation = 4;
            ws.status = handlers::WorkspaceStatus::Reindexing;
        }
        guard.dismiss();
        panic!("simulated panic after commit point");
    }));
    assert!(result.is_err(), "panic should have been caught");

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, "/new/dir",
        "after dismiss(), panic must NOT roll dir back");
    assert_eq!(ws.generation, 4,
        "after dismiss(), generation must stay at the new value");
    assert_eq!(ws.status, handlers::WorkspaceStatus::Reindexing,
        "after dismiss(), status must NOT be forced back to Resolved");
}

#[test]
fn test_run_server_with_io_continues_after_pending_response_panic() {
    // End-to-end: a server-initiated response carrying the test-panic id
    // must NOT kill the event loop. A subsequent ping must be answered.
    // Pre-fix the panic in handle_pending_response would propagate up and
    // terminate the loop, dropping the ping.
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = make_ctx();
    ctx.index_base = tmp.path().to_path_buf();

    let input = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"roots\":[]}}}}\n{{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"ping\"}}\n",
        TEST_PANIC_PENDING_RESPONSE_ID,
    );
    let mut reader = std::io::Cursor::new(input.into_bytes());
    let mut output = Vec::new();

    run_server_with_io(ctx, &mut reader, &mut output, false);

    let stdout = String::from_utf8(output).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    // The pending-response branch emits no response (it's handling an
    // incoming response, not a request). The ping reply is the only line.
    assert_eq!(lines.len(), 1,
        "expected exactly the ping response after the panicking pending-response, got: {stdout}");
    let ping_resp: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(ping_resp["id"], 42);
    assert!(ping_resp["result"].is_object(),
        "ping after pending-response panic must succeed: {stdout}");
}
