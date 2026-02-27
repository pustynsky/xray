use super::*;

#[test]
fn test_parse_initialize_request() {
    let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
    let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.method, "initialize");
    assert_eq!(req.id, Some(serde_json::json!(1)));
    assert!(req.params.is_some());
}

#[test]
fn test_parse_notification() {
    let json = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.method, "notifications/initialized");
    assert!(req.id.is_none());
}

#[test]
fn test_parse_tools_list_request() {
    let json = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.method, "tools/list");
    assert_eq!(req.id, Some(serde_json::json!(2)));
}

#[test]
fn test_parse_tools_call_request() {
    let json = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_grep","arguments":{"terms":"HttpClient","mode":"or"}}}"#;
    let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.method, "tools/call");
    let params = req.params.unwrap();
    assert_eq!(params["name"], "search_grep");
    assert_eq!(params["arguments"]["terms"], "HttpClient");
    assert_eq!(params["arguments"]["mode"], "or");
}

#[test]
fn test_initialize_response_format() {
    let result = InitializeResult::new(crate::definitions::definition_extensions());
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["protocolVersion"], "2025-03-26");
    assert_eq!(json["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(json["serverInfo"]["name"], "search-index");
    assert_eq!(json["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn test_initialize_includes_instructions() {
    let result = InitializeResult::new(crate::definitions::definition_extensions());
    let json = serde_json::to_value(&result).unwrap();
    let instructions = json["instructions"].as_str().unwrap();
    assert!(instructions.contains("search_fast"), "instructions should mention search_fast");
    assert!(instructions.contains("search_find"), "instructions should mention search_find");
    assert!(instructions.contains("substring"), "instructions should mention substring search");
    assert!(instructions.contains("search_callers"), "instructions should mention search_callers");
    assert!(instructions.contains("class"), "instructions should mention class parameter");
    assert!(instructions.contains("includeBody"), "instructions should mention includeBody");
    assert!(instructions.contains("countOnly"), "instructions should mention countOnly");
    // Verify all definition extensions appear in instructions
    for ext in crate::definitions::definition_extensions() {
        assert!(instructions.contains(&format!(".{}", ext)),
            "instructions should mention .{} extension", ext);
    }
}

#[test]
fn test_jsonrpc_response_format() {
    let resp = JsonRpcResponse::new(
        serde_json::json!(1),
        serde_json::to_value(InitializeResult::new(crate::definitions::definition_extensions())).unwrap(),
    );
    let json = serde_json::to_string(&resp).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 1);
    assert!(parsed["result"]["protocolVersion"].is_string());
}

#[test]
fn test_tool_call_success_result() {
    let result = ToolCallResult::success("hello".to_string());
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["content"][0]["type"], "text");
    assert_eq!(json["content"][0]["text"], "hello");
    // isError should not appear when false
    assert!(json.get("isError").is_none());
}

#[test]
fn test_tool_call_error_result() {
    let result = ToolCallResult::error("something failed".to_string());
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["content"][0]["text"], "something failed");
    assert_eq!(json["isError"], true);
}

#[test]
fn test_jsonrpc_error_response() {
    let resp = JsonRpcErrorResponse::new(
        serde_json::json!(5),
        -32601,
        "Method not found".to_string(),
    );
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 5);
    assert_eq!(json["error"]["code"], -32601);
    assert_eq!(json["error"]["message"], "Method not found");
}
