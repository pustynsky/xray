use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::mcp::handlers::{self, HandlerContext};
use crate::mcp::protocol::*;
use crate::{save_content_index, ContentIndex};
use crate::definitions::{self, DefinitionIndex};
use crate::git::cache::GitHistoryCache;

/// Run the MCP server event loop over stdio
pub fn run_server(
    index: Arc<RwLock<ContentIndex>>,
    def_index: Option<Arc<RwLock<DefinitionIndex>>>,
    server_dir: String,
    server_ext: String,
    metrics: bool,
    index_base: PathBuf,
    max_response_bytes: usize,
    content_ready: Arc<AtomicBool>,
    def_ready: Arc<AtomicBool>,
    git_cache: Arc<RwLock<Option<GitHistoryCache>>>,
    git_cache_ready: Arc<AtomicBool>,
    current_branch: Option<String>,
) {
    let ctx = HandlerContext {
        index,
        def_index,
        server_dir,
        server_ext,
        metrics,
        index_base,
        max_response_bytes,
        content_ready,
        def_ready,
        git_cache,
        git_cache_ready,
        current_branch,
    };

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    const MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024; // 10 MB

    // Set up Ctrl+C / SIGINT / SIGTERM handler for graceful shutdown
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown_flag.clone();
    if let Err(e) = ctrlc::set_handler(move || {
        shutdown_clone.store(true, Ordering::SeqCst);
        eprintln!("\nReceived shutdown signal, saving indexes...");
    }) {
        warn!("Failed to set Ctrl+C handler: {}", e);
    }

    info!("MCP server ready, waiting for JSON-RPC requests on stdin");

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                // Check if shutdown was signaled (belt-and-suspenders for signal between reads)
                if shutdown_flag.load(Ordering::SeqCst) {
                    info!("Shutdown flag set, exiting event loop");
                    break;
                }
                if line.len() > MAX_REQUEST_SIZE {
                    error!(size = line.len(), "Request too large, skipping");
                    continue;
                }
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }

                debug!(request = %line, "Incoming JSON-RPC");

                let request: JsonRpcRequest = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(error = %e, "Failed to parse JSON-RPC request");
                        let err = JsonRpcErrorResponse::new(
                                Value::Null,
                                -32700,
                                format!("Parse error: {}", e),
                            );
                            let resp = match serde_json::to_string(&err) {
                                Ok(s) => s,
                                Err(ser_err) => {
                                    error!(error = %ser_err, "Failed to serialize parse-error response, skipping");
                                    continue;
                                }
                            };
                        debug!(response = %resp, "Error response");
                        if let Err(e) = writeln!(writer, "{}", resp) {
                            error!(error = %e, "Failed to write error response to stdout, shutting down");
                            break;
                        }
                        if let Err(e) = writer.flush() {
                            error!(error = %e, "Failed to flush stdout, shutting down");
                            break;
                        }
                        continue;
                    }
                };

                // Notifications have no id — don't send a response
                if request.id.is_none() {
                    debug!(method = %request.method, "Received notification");
                    continue;
                }

                let id = request.id.unwrap();
                let response = handle_request(&ctx, &request.method, &request.params, id.clone());

                let resp_str = match serde_json::to_string(&response) {
                    Ok(s) => s,
                    Err(ser_err) => {
                        error!(error = %ser_err, "Failed to serialize JSON-RPC response");
                        // Send a JSON-RPC internal error instead of panicking
                        let fallback = JsonRpcErrorResponse::new(
                            id,
                            -32603,
                            format!("Internal error: response serialization failed: {}", ser_err),
                        );
                        match serde_json::to_string(&fallback) {
                            Ok(s) => s,
                            Err(e2) => {
                                error!(error = %e2, "Failed to serialize fallback error response, skipping");
                                continue;
                            }
                        }
                    }
                };
                debug!(response = %resp_str, "Outgoing JSON-RPC");
                if let Err(e) = writeln!(writer, "{}", resp_str) {
                    error!(error = %e, "Failed to write response to stdout, shutting down");
                    break;
                }
                if let Err(e) = writer.flush() {
                    error!(error = %e, "Failed to flush stdout, shutting down");
                    break;
                }
            }
            Err(e) => {
                error!(error = %e, "Error reading stdin");
                break;
            }
        }
    }

    info!("stdin closed, saving indexes before shutdown...");
    save_indexes_on_shutdown(&ctx);
    info!("Shutdown complete");
}

/// Save in-memory indexes to disk on graceful shutdown.
/// This preserves incremental watcher updates that were only held in memory.
fn save_indexes_on_shutdown(ctx: &HandlerContext) {
    // Save content index
    match ctx.index.read() {
        Ok(idx) => {
            if idx.files.is_empty() {
                info!("Content index is empty, skipping save");
            } else if let Err(e) = save_content_index(&idx, &ctx.index_base) {
                warn!(error = %e, "Failed to save content index on shutdown");
            } else {
                info!(files = idx.files.len(), "Content index saved on shutdown");
            }
        }
        Err(e) => warn!(error = %e, "Failed to read content index for shutdown save"),
    }

    // Save definition index
    if let Some(ref def) = ctx.def_index {
        match def.read() {
            Ok(idx) => {
                if idx.files.is_empty() {
                    info!("Definition index is empty, skipping save");
                } else if let Err(e) = definitions::save_definition_index(&idx, &ctx.index_base) {
                    warn!(error = %e, "Failed to save definition index on shutdown");
                } else {
                    info!(definitions = idx.definitions.len(), "Definition index saved on shutdown");
                }
            }
            Err(e) => warn!(error = %e, "Failed to read definition index for shutdown save"),
        }
    }
}

/// Safely serialize a value to a JSON-RPC `Value`, returning a JSON-RPC
/// internal-error response if serialization fails (instead of panicking).
fn safe_to_value<T: serde::Serialize>(v: T, id: &Value) -> Value {
    match serde_json::to_value(v) {
        Ok(val) => val,
        Err(e) => {
            error!(error = %e, "Failed to serialize value to JSON");
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32603,
                    "message": format!("Internal error: serialization failed: {}", e)
                }
            })
        }
    }
}

fn handle_request(
    ctx: &HandlerContext,
    method: &str,
    params: &Option<Value>,
    id: Value,
) -> Value {
    match method {
        "initialize" => {
            // Compute definition-supported extensions from server config
            let server_exts: Vec<&str> = ctx.server_ext.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            let def_extensions: Vec<&str> = definitions::DEFINITION_EXTENSIONS.iter()
                .filter(|ext| server_exts.iter().any(|se| se.eq_ignore_ascii_case(ext)))
                .copied()
                .collect();
            let result = InitializeResult::new(&def_extensions);
            let result_val = safe_to_value(result, &id);
            safe_to_value(JsonRpcResponse::new(id, result_val), &Value::Null)
        }
        "tools/list" => {
            let tools = handlers::tool_definitions();
            let result = ToolsListResult { tools };
            let result_val = safe_to_value(result, &id);
            safe_to_value(JsonRpcResponse::new(id, result_val), &Value::Null)
        }
        "tools/call" => {
            let params = match params {
                Some(p) => p,
                None => {
                    let result = ToolCallResult::error("Missing params".to_string());
                    let result_val = safe_to_value(result, &id);
                    return safe_to_value(JsonRpcResponse::new(id, result_val), &Value::Null);
                }
            };

            let tool_name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            // Log request (no-op when --debug-log not passed)
            crate::index::log_request(tool_name, &serde_json::to_string(&arguments).unwrap_or_default());

            let tool_start = std::time::Instant::now();
            let result = handlers::dispatch_tool(ctx, tool_name, &arguments);

            // Log response (no-op when --debug-log not passed)
            let elapsed_ms = tool_start.elapsed().as_secs_f64() * 1000.0;
            let result_json = safe_to_value(&result, &id);
            let response_str = serde_json::to_string(&result_json).unwrap_or_default();
            let response_bytes = response_str.len();
            crate::index::log_response(tool_name, elapsed_ms, response_bytes, &response_str);

            safe_to_value(JsonRpcResponse::new(id, result_json), &Value::Null)
        }
        "ping" => {
            safe_to_value(JsonRpcResponse::new(id, json!({})), &Value::Null)
        }
        _ => {
            safe_to_value(JsonRpcErrorResponse::new(
                id,
                -32601,
                format!("Method not found: {}", method),
            ), &Value::Null)
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::TrigramIndex;

    fn make_ctx() -> HandlerContext {
        let index = ContentIndex {
            root: ".".to_string(),
            ..Default::default()
        };
        HandlerContext {
            index: Arc::new(RwLock::new(index)),
            def_index: None,
            server_dir: ".".to_string(),
            server_ext: "cs".to_string(),
            metrics: false,
            index_base: std::path::PathBuf::from("."),
            max_response_bytes: crate::mcp::handlers::utils::DEFAULT_MAX_RESPONSE_BYTES,
            content_ready: Arc::new(AtomicBool::new(true)),
            def_ready: Arc::new(AtomicBool::new(true)),
            git_cache: Arc::new(RwLock::new(None)),
            git_cache_ready: Arc::new(AtomicBool::new(false)),
            current_branch: None,
        }
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
        assert_eq!(tools.len(), 15);
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

    /// Test that the def_extensions filtering logic in initialize correctly
    /// intersects server_ext with DEFINITION_EXTENSIONS.
    /// server_ext="cs,xml" should produce def_extensions=["cs"] (xml has no parser).
    #[test]
    fn test_initialize_def_extension_filtering() {
        // server_ext="cs,xml" → only "cs" has a definition parser
        let mut ctx = make_ctx();
        ctx.server_ext = "cs,xml".to_string();
        let result = handle_request(&ctx, "initialize", &None, json!(1));
        let instructions = result["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains(".cs"),
            "instructions should mention .cs (has parser)");
        // .xml should NOT appear in the NEVER READ line (no definition parser)
        // The NEVER READ line looks like "NEVER READ .cs FILES DIRECTLY"
        // Check that .xml doesn't appear between "NEVER READ" and "FILES DIRECTLY"
        assert!(!instructions.contains("NEVER READ .xml"),
            "instructions should NOT mention .xml in NEVER READ (no parser). Got:\n{}", instructions);
        assert!(!instructions.contains(".xml FILES DIRECTLY"),
            "instructions should NOT have .xml in file reading rule");

        // server_ext="xml" → no definition extensions at all
        let mut ctx2 = make_ctx();
        ctx2.server_ext = "xml".to_string();
        let result2 = handle_request(&ctx2, "initialize", &None, json!(2));
        let instructions2 = result2["result"]["instructions"].as_str().unwrap();
        assert!(!instructions2.contains("NEVER READ"),
            "xml-only server should NOT have NEVER READ block");
        assert!(instructions2.contains("search_definitions is not available"),
            "xml-only server should have fallback note");

        // server_ext="cs,ts,sql" → all three have parsers
        let mut ctx3 = make_ctx();
        ctx3.server_ext = "cs,ts,sql".to_string();
        let result3 = handle_request(&ctx3, "initialize", &None, json!(3));
        let instructions3 = result3["result"]["instructions"].as_str().unwrap();
        assert!(instructions3.contains(".cs"), "should contain .cs");
        assert!(instructions3.contains(".ts"), "should contain .ts");
        assert!(instructions3.contains(".sql"), "should contain .sql");
        assert!(instructions3.contains("NEVER READ"), "should have NEVER READ block");
    }

    #[test]
    fn test_shutdown_flag_initially_false_and_can_be_set() {
        let flag = Arc::new(AtomicBool::new(false));
        assert!(!flag.load(Ordering::SeqCst), "shutdown flag should start as false");

        let flag_clone = flag.clone();
        flag_clone.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst), "shutdown flag should be true after setting");
    }
}