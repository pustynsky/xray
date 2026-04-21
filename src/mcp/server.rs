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
/// MCP server event loop state — tracks protocol handshake and pending requests.
struct ServerState {
    /// Whether the client advertised `capabilities.roots`.
    client_supports_roots: bool,
    /// Whether the client advertised `capabilities.roots.listChanged`.
    client_supports_roots_list_changed: bool,
    /// ID of the pending `roots/list` request (if any).
    pending_roots_request_id: Option<u64>,
    /// Next request ID for server-initiated requests.
    next_id: u64,
}

impl ServerState {
    fn new() -> Self {
        Self {
            client_supports_roots: false,
            client_supports_roots_list_changed: false,
            pending_roots_request_id: None,
            next_id: 1,
        }
    }
}

/// Convert a file:// URI to a local path string using the `url` crate.
/// Handles percent-encoding, drive letters, Unicode, and UNC paths.
pub(crate) fn uri_to_path(uri: &str) -> Option<String> {
    let parsed = url::Url::parse(uri).ok()?;
    if parsed.scheme() != "file" { return None; }
    let path = parsed.to_file_path().ok()?;
    Some(crate::clean_path(&path.to_string_lossy()))
}

/// Send a JSON-RPC request to the client (server-initiated).
fn send_request(writer: &mut impl io::Write, state: &mut ServerState, method: &str, params: Value) -> io::Result<u64> {
    let id = state.next_id;
    state.next_id += 1;
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let s = serde_json::to_string(&request).unwrap();
    debug!(request = %s, "Sending server request");
    writeln!(writer, "{}", s)?;
    writer.flush()?;
    Ok(id)
}

/// Handle a response to a server-initiated request (e.g., roots/list).
fn handle_pending_response(raw: &Value, state: &mut ServerState, ctx: &HandlerContext) {
    let id = raw.get("id").and_then(|v| v.as_u64());

    // Check if this is the roots/list response
    if let Some(pending_id) = state.pending_roots_request_id
        && id == Some(pending_id) {
            state.pending_roots_request_id = None;

            if let Some(result) = raw.get("result") {
                if let Some(roots) = result.get("roots").and_then(|r| r.as_array()) {
                    if let Some(first_root) = roots.first()
                        && let Some(uri) = first_root.get("uri").and_then(|u| u.as_str())
                            && let Some(path) = uri_to_path(uri) {
                                let mut ws = ctx.workspace.write().unwrap_or_else(|e| e.into_inner());
                                let old_dir = ws.dir.clone();
                                if !path.eq_ignore_ascii_case(&old_dir) {
                                    ws.set_dir(path.clone());
                                    ws.mode = handlers::WorkspaceBindingMode::ClientRoots;
                                    ws.generation += 1;
                                    ws.status = handlers::WorkspaceStatus::Reindexing;
                                    // Reset ready flags — old indexes are for the wrong workspace.
                                    // This prevents tools from returning stale results from old indexes.
                                    ctx.content_ready.store(false, std::sync::atomic::Ordering::Release);
                                    if ctx.def_index.is_some() {
                                        ctx.def_ready.store(false, std::sync::atomic::Ordering::Release);
                                    }
                                    info!(dir = %path, previous = %old_dir, generation = ws.generation,
                                        roots_count = roots.len(), "Workspace set from roots/list");
                                    // Note: reindexing will happen on next tool call or via xray_reindex.
                                    // For now, set status to Reindexing — tools will see this and
                                    // the LLM can call xray_reindex to complete the switch.
                                } else {
                                    // Same directory — just update mode
                                    ws.mode = handlers::WorkspaceBindingMode::ClientRoots;
                                    info!(dir = %path, "Workspace confirmed by roots/list (same dir)");
                                }
                            }
                    if roots.len() > 1 {
                        warn!(count = roots.len(), "Multiple roots received, using first one");
                    }
                }
            } else if let Some(error) = raw.get("error") {
                warn!(error = %error, "roots/list request failed");
            }
            return;
        }

    debug!(id = ?id, "Received response to unknown server request");
}

pub fn run_server(ctx: HandlerContext) {

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    let mut state = ServerState::new();

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
                    // MINOR-16: cap the drain itself so a client that keeps streaming
                    // without a newline cannot make us loop forever. 64 MB is 6× the
                    // max legitimate request and still well below any realistic OOM
                    // threshold; once exceeded we give up and exit the event loop
                    // rather than risk an unbounded read.
                    const DRAIN_BYTE_CAP: usize = 64 * 1024 * 1024;
                    let mut drained: usize = 0;
                    let mut discard = String::new();
                    let exceeded = loop {
                        discard.clear();
                        match reader.by_ref().take(8192).read_line(&mut discard) {
                            Ok(0) => break false,
                            Ok(n) => {
                                drained = drained.saturating_add(n);
                                if discard.ends_with('\n') {
                                    break false;
                                }
                                if drained >= DRAIN_BYTE_CAP {
                                    break true;
                                }
                            }
                            Err(_) => break false,
                        }
                    };
                    if exceeded {
                        warn!(
                            drained_bytes = drained,
                            cap = DRAIN_BYTE_CAP,
                            "Oversized-request drain exceeded cap; terminating event loop to avoid unbounded read"
                        );
                        break;
                    }
                    continue;
                }
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }

                debug!(request = %trimmed, "Incoming JSON-RPC");

                // Parse as generic Value first to route requests vs responses
                let raw: Value = match serde_json::from_str(&trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "Failed to parse JSON-RPC message");
                        let err = JsonRpcErrorResponse::new(
                            Value::Null, -32700, format!("Parse error: {}", e),
                        );
                        if let Ok(resp) = serde_json::to_string(&err) {
                            let _ = writeln!(writer, "{}", resp);
                            let _ = writer.flush();
                        }
                        continue;
                    }
                };

                // Route: if has "method" → request or notification; else → response
                if raw.get("method").is_some() {
                    // Request or notification
                    let request: JsonRpcRequest = match serde_json::from_value(raw.clone()) {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(error = %e, "Failed to parse JSON-RPC request structure");
                            continue;
                        }
                    };

                    // MINOR-4 / MINOR-25: validate JSON-RPC version. Per JSON-RPC 2.0
                    // §4, `jsonrpc` MUST be exactly "2.0"; any other value is an
                    // Invalid Request (-32600). We previously accepted any string
                    // (field parsed but not checked) which made the server violate
                    // the spec silently.
                    if request.jsonrpc != "2.0" {
                        warn!(
                            jsonrpc = %request.jsonrpc,
                            method = %request.method,
                            "Rejecting request with unsupported JSON-RPC version"
                        );
                        let reply_id = request.id.clone().unwrap_or(Value::Null);
                        let err = JsonRpcErrorResponse::new(
                            reply_id,
                            -32600,
                            format!(
                                "Invalid Request: jsonrpc must be \"2.0\", got {:?}",
                                request.jsonrpc
                            ),
                        );
                        if let Ok(resp) = serde_json::to_string(&err) {
                            let _ = writeln!(writer, "{}", resp);
                            let _ = writer.flush();
                        }
                        continue;
                    }

                    // Handle notifications (no id)
                    if request.id.is_none() {
                        debug!(method = %request.method, "Received notification");
                        // Handle initialized notification — trigger roots/list
                        if request.method == "notifications/initialized" {
                            let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
                            let is_pinned = ws.mode == handlers::WorkspaceBindingMode::PinnedCli;
                            drop(ws);
                            if state.client_supports_roots && !is_pinned {
                                match send_request(&mut writer, &mut state, "roots/list", json!({})) {
                                    Ok(id) => {
                                        state.pending_roots_request_id = Some(id);
                                        info!(request_id = id, "Requesting roots/list from client");
                                    }
                                    Err(e) => warn!(error = %e, "Failed to send roots/list request"),
                                }
                            }
                        }
                        // Handle roots/list_changed notification
                        if request.method == "notifications/roots/list_changed" {
                            let ws = ctx.workspace.read().unwrap_or_else(|e| e.into_inner());
                            match ws.mode {
                                handlers::WorkspaceBindingMode::PinnedCli => {
                                    debug!("roots/list_changed ignored — workspace is PinnedCli");
                                }
                                handlers::WorkspaceBindingMode::ManualOverride => {
                                    info!("roots/list_changed received but workspace was manually overridden");
                                }
                                _ => {
                                    drop(ws); // release read lock
                                    if state.client_supports_roots {
                                        match send_request(&mut writer, &mut state, "roots/list", json!({})) {
                                            Ok(id) => {
                                                state.pending_roots_request_id = Some(id);
                                                info!(request_id = id, "Re-requesting roots/list after roots_changed");
                                            }
                                            Err(e) => warn!(error = %e, "Failed to send roots/list request"),
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    let id = request.id.unwrap();

                    // Special handling for initialize — parse client capabilities
                    if request.method == "initialize"
                        && let Some(ref params) = request.params {
                            let has_roots = params
                                .pointer("/capabilities/roots")
                                .is_some();
                            let has_list_changed = params
                                .pointer("/capabilities/roots/listChanged")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            state.client_supports_roots = has_roots;
                            state.client_supports_roots_list_changed = has_list_changed;
                            info!(roots = has_roots, list_changed = has_list_changed,
                                "Client capabilities parsed");
                        }

                    // After initialized notification handling is not needed here —
                    // it's handled above in the notification branch.
                    // But we need to trigger roots/list after initialize response.
                    // We do this after sending the initialize response below.

                    let response = handle_request(&ctx, &request.method, &request.params, id.clone());

                    let resp_str = match serde_json::to_string(&response) {
                        Ok(s) => s,
                        Err(ser_err) => {
                            error!(error = %ser_err, "Failed to serialize JSON-RPC response");
                            let fallback = JsonRpcErrorResponse::new(
                                id, -32603,
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
                } else if raw.get("result").is_some() || raw.get("error").is_some() {
                    // Response to a server-initiated request
                    handle_pending_response(&raw, &mut state, &ctx);
                } else {
                    warn!("Received message with no 'method', 'result', or 'error' field");
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
///
/// **Skip heuristic (MINOR-17):** the original implementation skipped save
/// when `files.is_empty()` to avoid overwriting a good on-disk index with
/// an empty one when the server started and shut down before the
/// background builder finished. That guard has a false-negative: a user
/// who legitimately removed every indexed file would see the stale
/// populated index persist on disk.
///
/// The corrected heuristic gates on `content_ready` / `def_ready`
/// (Acquire load), which is the authoritative "build finished" signal.
/// If the index is not ready, the builder is either running or failed —
/// either way, the on-disk copy (if any) is more trustworthy than our
/// partial in-memory state. If it is ready, we save unconditionally
/// (including the empty case, which represents a legitimate "all files
/// removed" state).
fn save_indexes_on_shutdown(ctx: &HandlerContext) {
    use std::sync::atomic::Ordering;

    // Save content index
    if !ctx.content_ready.load(Ordering::Acquire) {
        info!("Content index not ready (builder still running or failed), skipping save");
    } else {
        match ctx.index.read() {
            Ok(idx) => {
                if let Err(e) = save_content_index(&idx, &ctx.index_base) {
                    warn!(error = %e, "Failed to save content index on shutdown");
                } else {
                    info!(files = idx.files.len(), "Content index saved on shutdown");
                }
            }
            Err(e) => warn!(error = %e, "Failed to read content index for shutdown save"),
        }
    }

    // Save definition index
    if let Some(ref def) = ctx.def_index {
        if !ctx.def_ready.load(Ordering::Acquire) {
            info!("Definition index not ready (builder still running or failed), skipping save");
        } else {
            match def.read() {
                Ok(idx) => {
                    if let Err(e) = definitions::save_definition_index(&idx, &ctx.index_base) {
                        warn!(error = %e, "Failed to save definition index on shutdown");
                    } else {
                        info!(definitions = idx.definitions.len(), "Definition index saved on shutdown");
                    }
                }
                Err(e) => warn!(error = %e, "Failed to read definition index for shutdown save"),
            }
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
