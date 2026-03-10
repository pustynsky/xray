use std::io::{self, BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::mcp::handlers::{self, HandlerContext};
use crate::mcp::protocol::*;
use crate::save_content_index;
use crate::definitions;

/// Run the MCP server event loop over stdio.
///
/// Accepts a fully-constructed [`HandlerContext`] instead of 12 individual parameters.
/// The caller (`cmd_serve`) builds the context and passes it in.
pub fn run_server(ctx: HandlerContext) {

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
        // Use .take() to cap reading at MAX_REQUEST_SIZE + 1 bytes, preventing OOM
        // from a malicious/buggy client sending gigabytes without a newline.
        match reader.by_ref().take(MAX_REQUEST_SIZE as u64 + 1).read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                // Check if shutdown was signaled (belt-and-suspenders for signal between reads)
                if shutdown_flag.load(Ordering::SeqCst) {
                    info!("Shutdown flag set, exiting event loop");
                    break;
                }
                if line.len() > MAX_REQUEST_SIZE {
                    error!(size = line.len(), "Request too large (exceeded {} bytes), skipping", MAX_REQUEST_SIZE);
                    // Drain remaining bytes until newline to re-sync the stream.
                    // Use bounded reads in a loop to avoid OOM on the drain itself.
                    let mut discard = String::new();
                    loop {
                        discard.clear();
                        match reader.by_ref().take(8192).read_line(&mut discard) {
                            Ok(0) => break,                         // EOF
                            Ok(_) if discard.ends_with('\n') => break, // found newline
                            Ok(_) => continue,                      // keep draining
                            Err(_) => break,
                        }
                    }
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
                let Some(id) = request.id else {
                    debug!(method = %request.method, "Received notification");
                    continue;
                };

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
    crate::index::log_memory("shutdown: saving indexes");
    save_indexes_on_shutdown(&ctx);
    crate::index::log_memory("shutdown: complete");
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
            // Use ctx.def_extensions (computed once in serve.rs) to ensure
            // initialize instructions are consistent with tools/list descriptions.
            // Both must use the same def_extensions source.
            let def_ext_refs: Vec<&str> = ctx.def_extensions.iter().map(|s| s.as_str()).collect();
            let result = InitializeResult::new(&def_ext_refs);
            let result_val = safe_to_value(result, &id);
            safe_to_value(JsonRpcResponse::new(id, result_val), &Value::Null)
        }
        "tools/list" => {
            let tools = handlers::tool_definitions(&ctx.def_extensions);
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
#[path = "server_tests.rs"]
mod tests;
