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
                        let resp = serde_json::to_string(&err).unwrap();
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

                let resp_str = serde_json::to_string(&response).unwrap();
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

fn handle_request(
    ctx: &HandlerContext,
    method: &str,
    params: &Option<Value>,
    id: Value,
) -> Value {
    match method {
        "initialize" => {
            let result = InitializeResult::new();
            serde_json::to_value(JsonRpcResponse::new(
                id,
                serde_json::to_value(result).unwrap(),
            ))
            .unwrap()
        }
        "tools/list" => {
            let tools = handlers::tool_definitions();
            let result = ToolsListResult { tools };
            serde_json::to_value(JsonRpcResponse::new(
                id,
                serde_json::to_value(result).unwrap(),
            ))
            .unwrap()
        }
        "tools/call" => {
            let params = match params {
                Some(p) => p,
                None => {
                    let result = ToolCallResult::error("Missing params".to_string());
                    return serde_json::to_value(JsonRpcResponse::new(
                        id,
                        serde_json::to_value(result).unwrap(),
                    ))
                    .unwrap();
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
            let result_json = serde_json::to_value(&result).unwrap();
            let response_str = serde_json::to_string(&result_json).unwrap_or_default();
            let response_bytes = response_str.len();
            crate::index::log_response(tool_name, elapsed_ms, response_bytes, &response_str);

            serde_json::to_value(JsonRpcResponse::new(
                id,
                result_json,
            ))
            .unwrap()
        }
        "ping" => {
            serde_json::to_value(JsonRpcResponse::new(id, json!({}))).unwrap()
        }
        _ => {
            serde_json::to_value(JsonRpcErrorResponse::new(
                id,
                -32601,
                format!("Method not found: {}", method),
            ))
            .unwrap()
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
            created_at: 0,
            max_age_secs: 3600,
            files: vec![],
            index: HashMap::new(),
            total_tokens: 0,
            extensions: vec![],
            file_token_counts: vec![],
            trigram: TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: None,
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
    fn test_shutdown_flag_initially_false_and_can_be_set() {
        let flag = Arc::new(AtomicBool::new(false));
        assert!(!flag.load(Ordering::SeqCst), "shutdown flag should start as false");

        let flag_clone = flag.clone();
        flag_clone.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst), "shutdown flag should be true after setting");
    }
}