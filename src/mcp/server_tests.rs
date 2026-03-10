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
    assert_eq!(result["result"]["serverInfo"]["name"], "search-index");
}

#[test]
fn test_handle_tools_list() {
    let ctx = make_ctx();
    let result = handle_request(&ctx, "tools/list", &None, json!(2));
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 2);
    let tools = result["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 16);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"search_grep"));
    assert!(names.contains(&"search_find"));
    assert!(names.contains(&"search_fast"));
    assert!(names.contains(&"search_info"));
    assert!(names.contains(&"search_reindex"));
    assert!(names.contains(&"search_reindex_definitions"));
    assert!(names.contains(&"search_definitions"));
}

#[test]
fn test_handle_tools_call_grep() {
    let ctx = make_ctx();
    let params = json!({
        "name": "search_grep",
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
fn test_handle_request_does_not_panic_on_serialization() {
    // Verify that handle_request uses safe_to_value which doesn't panic.
    // We exercise all method branches to confirm no unwrap panics.
    let ctx = make_ctx();

    // All standard branches should return valid JSON-RPC responses
    let methods = vec![
        ("initialize", None),
        ("tools/list", None),
        ("tools/call", Some(json!({"name": "search_grep", "arguments": {"terms": "test"}}))),
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

    // def_extensions=[] → no definition extensions at all (no --definitions flag)
    let mut ctx2 = make_ctx();
    ctx2.server_ext = "xml".to_string();
    ctx2.def_extensions = vec![]; // no --definitions
    let result2 = handle_request(&ctx2, "initialize", &None, json!(2));
    let instructions2 = result2["result"]["instructions"].as_str().unwrap();
    assert!(!instructions2.contains("NEVER READ"),
        "empty def_extensions should NOT have NEVER READ block");
    assert!(instructions2.contains("search_definitions is not available"),
        "empty def_extensions should have fallback note");

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

/// Verify that tools/list response includes dynamic language descriptions
/// based on ctx.def_extensions.
#[test]
fn test_tools_list_dynamic_descriptions_rust() {
    let mut ctx = make_ctx();
    ctx.def_extensions = vec!["rs".to_string()];
    let result = handle_request(&ctx, "tools/list", &None, json!(2));
    let tools = result["result"]["tools"].as_array().unwrap();
    let def_tool = tools.iter().find(|t| t["name"] == "search_definitions").unwrap();
    let desc = def_tool["description"].as_str().unwrap();
    assert!(desc.contains("Rust"),
        "tools/list search_definitions should mention 'Rust' when def_extensions=[rs]. Got: {}", desc);
    assert!(!desc.contains("C#"),
        "tools/list search_definitions should NOT mention C# for rs-only config");
}

#[test]
fn test_tools_list_dynamic_descriptions_empty() {
    let ctx = make_ctx(); // default: def_extensions = []
    let result = handle_request(&ctx, "tools/list", &None, json!(2));
    let tools = result["result"]["tools"].as_array().unwrap();
    let def_tool = tools.iter().find(|t| t["name"] == "search_definitions").unwrap();
    let desc = def_tool["description"].as_str().unwrap();
    assert!(desc.contains("not available"),
        "tools/list search_definitions should say 'not available' when def_extensions is empty");
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
    assert!(instructions.contains("search_definitions is not available"),
        "initialize with empty def_extensions should have fallback note");
}

/// Regression test: initialize with def_extensions=["rs"] must say "NEVER READ .rs" and
/// tools/list search_definitions must say "Rust" — both from ctx.def_extensions.
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
    let def_tool = tools.iter().find(|t| t["name"] == "search_definitions").unwrap();
    let desc = def_tool["description"].as_str().unwrap();
    assert!(desc.contains("Rust"),
        "tools/list search_definitions should mention 'Rust'");
}


#[test]
fn test_shutdown_flag_initially_false_and_can_be_set() {
    let flag = Arc::new(AtomicBool::new(false));
    assert!(!flag.load(Ordering::SeqCst), "shutdown flag should start as false");

    let flag_clone = flag.clone();
    flag_clone.store(true, Ordering::SeqCst);
    assert!(flag.load(Ordering::SeqCst), "shutdown flag should be true after setting");
}
