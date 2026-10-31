//! Shared utility functions for MCP tool handlers.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mcp::protocol::ToolCallResult;
use crate::clean_path;

use super::HandlerContext;

/// TTL for the live current-branch probe used by [`branch_warning`].
/// Short enough that `git checkout main` is reflected within a few seconds;
/// long enough that repeated tool calls don't fork a `git` subprocess each
/// time. See `bug-report_xray-index-freshness-reindex-cache-and-stale-branch-warning_2026-04-28.md`.
const BRANCH_PROBE_TTL: Duration = Duration::from_secs(5);

/// Serialize a JSON Value to a string, returning a fallback error JSON
/// if serialization fails (e.g., due to NaN/Infinity float values).
/// This prevents panics in MCP handlers from crashing the long-lived server process.
///
/// MINOR-29: the fallback path builds its JSON through `serde_json::json!`
/// so the error message is properly escaped. The previous implementation
/// used `format!` with the raw `Display` of `serde_json::Error`, which
/// could contain `"`, `\`, or control characters and therefore emit
/// invalid JSON for the client.
pub(crate) fn json_to_string(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| {
        let fallback = serde_json::json!({
            "error": format!("serialization failed: {}", e)
        });
        serde_json::to_string(&fallback)
            .unwrap_or_else(|_| String::from(r#"{"error":"serialization fallback failed"}"#))
    })
}

// ─── Branch warning ─────────────────────────────────────────────────

/// Provenance of the value returned by [`current_branch_resolved`].
pub(crate) enum BranchSource {
    /// Value came from a successful live `git rev-parse` probe (or its TTL
    /// cache) within the current workspace.
    Live,
    /// Value came from `HandlerContext::current_branch` (the snapshot taken
    /// at server startup) because the live probe failed or is disabled.
    Startup,
    /// No branch could be determined (no startup value and probe failed).
    Unknown,
}

/// Resolve the current branch for `branch_warning`.
///
/// When `ctx.live_branch_probe_enabled` is false (tests, CLI subcommands),
/// returns the static value captured at server startup. When true (long-running
/// MCP server), performs a `git rev-parse --abbrev-ref HEAD` probe in the
/// current workspace directory and caches the result for [`BRANCH_PROBE_TTL`].
///
/// Probe failure (no `git`, not a repo, detached HEAD with empty output) falls
/// back to `ctx.current_branch` so behaviour is never worse than the legacy
/// startup-only path.
///
/// Returns the branch source alongside the value so callers can surface
/// `branchSource` for diagnostics if they want to.
pub(crate) fn current_branch_resolved(ctx: &HandlerContext) -> (Option<String>, BranchSource) {
    if !ctx.live_branch_probe_enabled {
        return (
            ctx.current_branch.clone(),
            if ctx.current_branch.is_some() { BranchSource::Startup } else { BranchSource::Unknown },
        );
    }

    let dir = ctx.server_dir();

    // Read-lock fast path: serve cached value if still fresh.
    if let Ok(cache) = ctx.current_branch_cache.read()
        && let Some((ts, val)) = cache.get(&dir)
        && ts.elapsed() < BRANCH_PROBE_TTL
    {
        return match val {
            Some(_) => (val.clone(), BranchSource::Live),
            None => (
                ctx.current_branch.clone(),
                if ctx.current_branch.is_some() { BranchSource::Startup } else { BranchSource::Unknown },
            ),
        };
    }


    // Cold or expired entry — probe live. Note: a brief race where N concurrent
    // requests all probe at once is possible (the read-lock check above is not
    // atomic with the write below). N is bounded by handler concurrency and the
    // probe is cheap (~5–20 ms on Windows), so we accept it instead of adding a
    // single-flight gate.
    let probed = crate::background_command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Ok(mut cache) = ctx.current_branch_cache.write() {
        cache.insert(dir, (Instant::now(), probed.clone()));
    }

    match probed {
        Some(b) => (Some(b), BranchSource::Live),
        None => (
            ctx.current_branch.clone(),
            if ctx.current_branch.is_some() { BranchSource::Startup } else { BranchSource::Unknown },
        ),
    }
}

/// Returns a warning message if the current branch is not `main` or `master`.
/// Used by index-based tools (xray_grep, xray_definitions, xray_callers,
/// xray_fast) to alert the user that results may differ from production.
///
/// Resolves the branch via [`current_branch_resolved`] so a long-running MCP
/// server reflects `git checkout` within seconds (bug-report 2026-04-28).
pub(crate) fn branch_warning(ctx: &HandlerContext) -> Option<String> {
    let (branch, _src) = current_branch_resolved(ctx);
    branch.and_then(|b| {
        if b == "main" || b == "master" || b == "HEAD" {
            None
        } else {
            Some(format!(
                "Index is built on branch '{}', not on main/master. Results may differ from production.",
                b
            ))
        }
    })
}

/// Inject branchWarning into a summary JSON object if needed.
pub(crate) fn inject_branch_warning(summary: &mut Value, ctx: &HandlerContext) {
    if let Some(warning) = branch_warning(ctx) {
        summary["branchWarning"] = serde_json::Value::String(warning);
    }
}

/// DEF-S-006: surface a degraded definition-index state in handler responses.
///
/// When `definitions::build_definition_index` could not join one or more parser
/// worker threads (panic / OOM / tree-sitter abort), `worker_panics` is non-zero
/// and the index is missing a chunk of files. Without this signal, callers see
/// suspiciously empty results from `xray_definitions` / `xray_callers` and have
/// no way to distinguish "no matches" from "index is incomplete". The flag
/// hints the user to rerun `xray_reindex_definitions`.
///
/// `worker_panics` MUST be read from a `DefinitionIndex` guard already held by
/// the caller. Re-acquiring `ctx.def_index.read()` here would re-enter the
/// same `RwLock` on the same thread, which is undefined for `std::sync::RwLock`
/// and on Windows can stall for minutes if a writer (watcher / reindex) is
/// waiting between the outer and inner read. See
/// [docs/concurrency.md](docs/concurrency.md) §"Lock Ordering Contract" and
/// the 2026-05-03 incident on a large workspace (321 s stall on a single
/// `xray_definitions` request) recorded in the local debug log.
pub(crate) fn inject_index_degraded(summary: &mut Value, worker_panics: usize) {
    if worker_panics > 0 {
        summary["indexDegraded"] = json!(true);
        summary["indexDegradedHint"] = json!(format!(
            "{} parser worker(s) panicked during index build — run xray_reindex_definitions to recover",
            worker_panics
        ));
    }
}

pub(crate) fn build_result_status(
    status: &str,
    complete: bool,
    safe_for_exhaustive_claims: bool,
    safe_for_exact_semantics: bool,
    evidence_level: &str,
    reasons: Vec<String>,
) -> Value {
    json!({
        "status": status,
        "complete": complete,
        "safeForExhaustiveClaims": safe_for_exhaustive_claims,
        "safeForExactSemantics": safe_for_exact_semantics,
        "evidenceLevel": evidence_level,
        "reasons": reasons,
    })
}

pub(crate) fn attach_result_status(output: Value, result_status: Value) -> Value {
    let Value::Object(map) = output else {
        return output;
    };

    let mut ordered = serde_json::Map::new();
    ordered.insert("resultStatus".to_string(), result_status);
    for (key, value) in map {
        if key != "resultStatus" {
            ordered.insert(key, value);
        }
    }
    Value::Object(ordered)
}

pub(crate) fn set_result_status(output: &mut Value, result_status: Value) {
    let current = std::mem::take(output);
    *output = attach_result_status(current, result_status);
}

pub(crate) fn count_tree_nodes(nodes: &[Value]) -> usize {
    fn count_nested(node: &Value) -> usize {
        let mut total = 1usize;
        for child_key in ["callers", "callees", "children", "parents"] {
            if let Some(children) = node.get(child_key).and_then(|value| value.as_array()) {
                total += children.iter().map(count_nested).sum::<usize>();
            }
        }
        total
    }

    nodes.iter().map(count_nested).sum()
}

pub(crate) fn add_collection_accounting(result_status: &mut Value, shown: Value, total: Value) {
    let omitted = match (&shown, &total) {
        (Value::Object(shown_obj), Value::Object(total_obj)) => {
            let mut omitted_obj = serde_json::Map::new();
            for (key, total_value) in total_obj {
                if let (Some(total_u64), Some(shown_u64)) = (
                    total_value.as_u64(),
                    shown_obj.get(key).and_then(Value::as_u64),
                ) {
                    omitted_obj.insert(key.clone(), json!(total_u64.saturating_sub(shown_u64)));
                }
            }
            Value::Object(omitted_obj)
        }
        _ => Value::Null,
    };
    result_status["shown"] = shown;
    result_status["total"] = total;
    if !omitted.is_null() {
        result_status["omitted"] = omitted;
    }
    result_status["totalKnown"] = json!(true);
}

#[derive(Debug, Clone)]
pub(crate) struct PageRequest {
    pub offset: usize,
    pub node_offset: Option<usize>,
    pub requested: bool,
    pub workspace_generation: u64,
    pub index_epoch: u64,
    pub query_fingerprint: String,
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect(),
            )
        }
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonicalize_json).collect())
        }
        other => other,
    }
}

fn pagination_query_fingerprint(tool_name: &str, args: &Value) -> String {
    let mut normalized_args = args.clone();
    if let Some(object) = normalized_args.as_object_mut() {
        object.remove("offset");
        object.remove("continuationToken");
        #[cfg(test)]
        object.remove("__testTokenRegexStrategy");
    }
    let payload = canonicalize_json(json!({
        "tool": tool_name,
        "arguments": normalized_args,
    }));
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn record_content_index_publish(
    index_epoch: &std::sync::atomic::AtomicU64,
) {
    index_epoch.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
}


pub(crate) fn continuation_token(
    index_epoch: u64,
    next_offset: usize,
    query_fingerprint: &str,
) -> String {
    format!(
        "xrp1.{index_epoch}.{next_offset}.{query_fingerprint}"
    )
}

pub(crate) fn read_non_negative_usize(
    args: &Value,
    key: &str,
    default: usize,
) -> Result<usize, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or_else(|| format!("{key} must be a non-negative integer"))?;
            usize::try_from(raw).map_err(|_| format!("{key} is too large"))
        }
    }
}


pub(crate) fn parse_page_request(
    ctx: &HandlerContext,
    tool_name: &str,
    args: &Value,
) -> Result<PageRequest, String> {
    let explicit_offset = match args.get("offset") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let raw = value
                .as_u64()
                .ok_or_else(|| "offset must be a non-negative integer".to_string())?;
            Some(usize::try_from(raw).map_err(|_| "offset is too large".to_string())?)
        }
    };
    let token = match args.get("continuationToken") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.as_str()),
        Some(_) => return Err("continuationToken must be a non-empty string".to_string()),
    };
    if explicit_offset.is_some() && token.is_some() {
        return Err("offset cannot be combined with continuationToken".to_string());
    }

    let workspace_generation = ctx
        .workspace
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .generation;
    let index_epoch = ctx.index_epoch.load(std::sync::atomic::Ordering::Acquire);
    let query_fingerprint = pagination_query_fingerprint(tool_name, args);
    let (offset, node_offset) = if let Some(token) = token {
        let parts: Vec<&str> = token.splitn(6, '.').collect();
        if parts.first() == Some(&"xrt1") && tool_name != "xray_callers" {
            return Err("call-tree continuationToken is only valid for xray_callers".to_string());
        }
        let (generation, offset, node_offset, fingerprint) = match parts.as_slice() {
            ["xrp1", generation, offset, fingerprint] => (
                generation.parse::<u64>().ok(),
                offset.parse::<usize>().ok(),
                None,
                Some(*fingerprint),
            ),
            ["xrt1", generation, root_offset, node_offset, fingerprint] => (
                generation.parse::<u64>().ok(),
                root_offset.parse::<usize>().ok(),
                node_offset.parse::<usize>().ok(),
                Some(*fingerprint),
            ),
            _ => (None, None, None, None),
        };
        if generation.is_none()
            || offset.is_none()
            || fingerprint.is_none()
            || (parts.first() == Some(&"xrt1") && node_offset.is_none())
        {
            return Err("continuationToken is malformed".to_string());
        }
        if generation != Some(index_epoch) {
            return Err(format!(
                "stale continuationToken: index epoch changed from {} to {}",
                generation.unwrap_or_default(),
                index_epoch,
            ));
        }
        if fingerprint != Some(query_fingerprint.as_str()) {
            return Err("continuationToken does not match this query".to_string());
        }
        (offset.unwrap_or_default(), node_offset)
    } else {
        (explicit_offset.unwrap_or(0), None)
    };

    Ok(PageRequest {
        offset,
        node_offset,
        requested: explicit_offset.is_some() || token.is_some(),
        workspace_generation,
        index_epoch,
        query_fingerprint,
    })
}

pub(crate) fn attach_page_status(
    result_status: &mut Value,
    unit: &str,
    page_request: &PageRequest,
    returned: usize,
    total: usize,
) {
    let next_offset = page_request.offset.saturating_add(returned);
    let total_known = result_status
        .get("totalKnown")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut page = json!({
        "unit": unit,
        "offset": page_request.offset,
        "returned": returned,
        "total": total,
        "workspaceGeneration": page_request.workspace_generation,
        "indexEpoch": page_request.index_epoch,
        "queryFingerprint": page_request.query_fingerprint,
        "totalKnown": total_known,
    });
    if let Some(node_offset) = page_request.node_offset {
        page["nodeOffset"] = json!(node_offset);
    }
    if returned > 0 && next_offset < total {
        page["nextOffset"] = json!(next_offset);
        page["continuationToken"] = json!(continuation_token(
            page_request.index_epoch,
            next_offset,
            &page_request.query_fingerprint,
        ));
    }
    result_status["page"] = page;
    if page_request.offset > 0 || next_offset < total {
        result_status["status"] = json!("partial");
        result_status["complete"] = json!(false);
        result_status["safeForExhaustiveClaims"] = json!(false);
        let reasons = result_status["reasons"].as_array_mut();
        if let Some(reasons) = reasons
            && !reasons.iter().any(|reason| reason == "pagination")
        {
            reasons.push(json!("pagination"));
        }
    }
}

fn add_generic_accounting_from_output(result_status: &mut Value, output: &Value) {
    let Some(summary) = output.get("summary") else {
        result_status["totalKnown"] = json!(false);
        result_status["omittedReason"] = json!("response_size_cap");
        return;
    };

    if let Some(files) = output.get("files").and_then(Value::as_array) {
        let shown_occurrences = files
            .iter()
            .filter_map(|file| file.get("occurrences").and_then(Value::as_u64))
            .sum::<u64>();
        let total_files = summary.get("totalFiles").and_then(Value::as_u64);
        let total_occurrences = summary.get("totalOccurrences").and_then(Value::as_u64);
        if let (Some(total_files), Some(total_occurrences)) = (total_files, total_occurrences) {
            add_collection_accounting(
                result_status,
                json!({ "files": files.len(), "occurrences": shown_occurrences }),
                json!({ "files": total_files, "occurrences": total_occurrences }),
            );
            return;
        }
    }

    if let Some(definitions) = output.get("definitions").and_then(Value::as_array)
        && let Some(total_results) = summary.get("totalResults").and_then(Value::as_u64)
    {
        add_collection_accounting(
            result_status,
            json!({ "definitions": definitions.len() }),
            json!({ "definitions": total_results }),
        );
        return;
    }

    if let Some(call_tree) = output.get("callTree").and_then(Value::as_array)
        && let Some(total_nodes) = summary.get("totalNodes").and_then(Value::as_u64)
    {
        add_collection_accounting(
            result_status,
            json!({ "nodes": count_tree_nodes(call_tree) }),
            json!({ "nodes": total_nodes }),
        );
        return;
    }

    result_status["totalKnown"] = json!(false);
    result_status["omittedReason"] = json!("response_size_cap");
}

pub(crate) fn mark_result_status_response_truncated(output: &mut Value, reasons: &[String]) {
    let evidence_level = output
        .get("resultStatus")
        .and_then(|status| status.get("evidenceLevel"))
        .and_then(Value::as_str)
        .unwrap_or("snippet")
        .to_string();

    let mut status = output
        .get("resultStatus")
        .cloned()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| {
            build_result_status("partial", false, false, false, &evidence_level, Vec::new())
        });

    status["status"] = json!("partial");
    status["complete"] = json!(false);
    status["safeForExhaustiveClaims"] = json!(false);
    status["safeForExactSemantics"] = json!(false);
    status["evidenceLevel"] = json!(evidence_level);

    let mut status_reasons = status
        .get("reasons")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !status_reasons.iter().any(|reason| reason == "response_truncated") {
        status_reasons.push("response_truncated".to_string());
    }
    status["reasons"] = json!(status_reasons);

    if !reasons.is_empty() {
        status["truncationDetails"] = json!(reasons);
    }
    let page_owns_accounting = status
        .get("page")
        .is_some_and(|page| {
            page.get("unit").and_then(Value::as_str) == Some("callTreeNodes")
                || page.get("totalKnown").and_then(Value::as_bool) == Some(false)
        });
    if !page_owns_accounting {
        add_generic_accounting_from_output(&mut status, output);
    }
    set_result_status(output, status);
}


// ─── Dir validation ─────────────────────────────────────────────────

/// Normalize path separators to forward slashes for cross-platform comparison.
pub(crate) fn normalize_path_sep(p: &str) -> String {
    p.replace('\\', "/")
}

/// Resolve `dir` to a logical absolute path under `server_dir`, mirroring the
/// indexer's `WalkBuilder::follow_links(true)` view: a file reached via
/// `<server_dir>/<symlinked_subdir>/foo` keeps that logical path even when the
/// underlying directory is a symlink to somewhere outside the workspace.
///
/// Behavior:
/// - Absolute input → cleaned path-as-given (separator normalization only).
/// - `"."` → `clean_path(server_dir)`.
/// - Relative input → `<clean_path(server_dir)>/<input>` (joined as text).
///
/// **Never calls `canonicalize`**, so it is safe for security-critical
/// comparisons that must remain consistent with what the indexer recorded
/// (e.g. boundary checks via [`code_xray::is_path_within`], path-prefix filters
/// on cached index entries). Symlink resolution would silently break those
/// comparisons because the indexer keeps logical paths but `canonicalize`
/// returns the symlink target.
pub(crate) fn resolve_dir_to_absolute(dir: &str, server_dir: &str) -> String {
    let normalized = dir.replace('\\', "/");
    if std::path::Path::new(dir).is_absolute() {
        clean_path(&normalized)
    } else if dir == "." {
        clean_path(server_dir)
    } else {
        format!(
            "{}/{}",
            clean_path(server_dir).trim_end_matches('/'),
            normalized.trim_matches('/'),
        )
    }
}

/// Validate that `requested_dir` is inside the workspace and decide whether the
/// caller needs a subdirectory filter on the cached index entries.
///
/// Returns:
/// - `Ok(None)` — request targets the workspace root itself; no filter needed.
/// - `Ok(Some(logical_abs))` — request targets a subdirectory; downstream uses
///   the returned LOGICAL absolute path as a path-prefix filter on indexed
///   entries (so it correctly matches files reached via symlinked subdirs).
/// - `Err(message)` — request targets a path outside the workspace.
pub(crate) fn validate_search_dir(requested_dir: &str, server_dir: &str) -> Result<Option<String>, String> {
    // Step 1: Build a *logical* absolute form of the requested path — i.e. the
    // path as the indexer would see it via `WalkBuilder::follow_links`, NOT the
    // symlink target. We do not call `canonicalize` here, otherwise a symlinked
    // subdirectory like `docs/personal` would be resolved to its real target
    // (e.g. `D:\Personal\…`) and:
    //   (a) the validation below would reject it as outside the workspace, and
    //   (b) any returned subdir filter would no longer match the indexed entries.
    let logical_abs = resolve_dir_to_absolute(requested_dir, server_dir);

    // Step 2: Workspace boundary check via `code_xray::is_path_within`, which
    // performs logical-path comparison first (matching the indexer) and falls
    // back to canonicalize-based comparison only when needed (8.3 short names,
    // path-traversal validation for inputs containing `..`).
    if !code_xray::is_path_within(&logical_abs, server_dir) {
        return Err(format!(
            "Server started with --dir {}. For other directories, start another server instance or use CLI.",
            server_dir
        ));
    }

    // Step 3: Decide whether the request targets the workspace root itself
    // (return `None` — no subdir filter needed) or a subdirectory (return
    // `Some(logical_abs)` — downstream uses it as a path-prefix filter).
    let req_norm = normalize_path_sep(&logical_abs).to_lowercase();
    let srv_norm = normalize_path_sep(server_dir).to_lowercase();
    let req_trim = req_norm.trim_end_matches('/');
    let srv_trim = srv_norm.trim_end_matches('/');

    if req_trim == srv_trim {
        return Ok(None);
    }

    let server_prefix = format!("{}/", srv_trim);
    if req_norm.starts_with(&server_prefix) {
        return Ok(Some(logical_abs));
    }

    let canonical = std::fs::canonicalize(&logical_abs)
        .map(|path| crate::clean_path(&path.to_string_lossy()))
        .unwrap_or(logical_abs);
    Ok(Some(canonical))
}

/// Check if a file path is under the given directory prefix (case-insensitive, separator-normalized).
/// Ensures proper boundary check: `C:\Repos\Shared` won't match `C:\Repos\SharedExtra\file.cs`.
pub(crate) fn is_under_dir(file_path: &str, dir_prefix: &str) -> bool {
    let file_norm = normalize_path_sep(file_path).to_lowercase();
    let mut dir_norm = normalize_path_sep(dir_prefix).to_lowercase();
    let file_trim = file_norm.trim_end_matches('/');
    let dir_trim = dir_norm.trim_end_matches('/');
    if file_trim == dir_trim {
        return false;
    }

    // Ensure dir prefix ends with '/' for proper boundary matching.
    if !dir_norm.ends_with('/') {
        dir_norm.push('/');
    }
    file_norm.starts_with(&dir_norm)
}

/// Heuristic: if path ends with a known source/config extension, it's likely a file path.
/// Used as fallback when `Path::is_file()` returns false (e.g., non-existent paths).
pub(crate) fn looks_like_file_path(path: &str) -> bool {
    let known_exts = [
        "rs", "cs", "ts", "tsx", "js", "jsx", "py", "java",
        "go", "rb", "sql", "xml", "json", "yaml", "yml",
        "md", "txt", "toml", "cfg", "ini", "html", "css",
        "scss", "less", "vue", "svelte", "swift", "kt",
        "c", "cpp", "h", "hpp", "csproj", "sln", "props",
        "targets", "config", "csv", "log",
    ];
    if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
        known_exts.iter().any(|k| k.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

// ─── Extension filter helper ────────────────────────────────────────

/// Check if a file path's extension matches a filter string.
/// Supports comma-separated extensions: `"cs,sql"` matches both `.cs` and `.sql`.
/// Comparison is case-insensitive. Whitespace around extensions is trimmed.
/// Pre-computed exclude-directory patterns for zero-allocation per-file matching.
/// Create once per query via `from_dirs()`, then call `matches()` per file.
#[derive(Debug, Clone)]
pub(crate) struct ExcludePatterns {
    /// Pre-lowercased segment patterns: [("/test/", "test/"), ...]
    segments: Vec<(String, String)>,
    original: Vec<String>,
}

impl ExcludePatterns {
    /// Build from raw exclude_dir strings (e.g., ["test", "Mock"]).
    /// Lowercases and formats patterns once.
    pub fn from_dirs(exclude_dirs: &[String]) -> Self {
        let segments = exclude_dirs.iter().map(|excl| {
            let lower = excl.to_lowercase();
            (format!("/{}/", lower), format!("{}/", lower))
        }).collect();
        Self { segments, original: exclude_dirs.to_vec() }
    }

    /// Check if a path matches any exclude pattern.
    /// `path_lower_normalized` MUST be pre-lowercased and use forward slashes.
    pub fn matches(&self, path_lower_normalized: &str) -> bool {
        self.matching_patterns(path_lower_normalized).next().is_some()
    }

    pub(crate) fn matching_patterns<'a>(&'a self, path_lower_normalized: &'a str) -> impl Iterator<Item = &'a str> {
        self.segments.iter().zip(&self.original).filter(move |((segment, at_start), _)| {
            path_lower_normalized.contains(segment.as_str())
                || path_lower_normalized.starts_with(at_start.as_str())
        }).map(|(_, original)| original.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// Pre-split a comma-separated extension filter string.
pub(crate) fn prepare_ext_filter(ext_filter: &str) -> Vec<String> {
    ext_filter.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}


pub(crate) fn matches_ext_filter(file_path: &str, ext_filter: &str) -> bool {
    std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ext_filter.split(',')
                .any(|allowed| e.eq_ignore_ascii_case(allowed.trim()))
        })
}

// Check if a file path matches any of the exclude directory filters.
// Uses segment-based matching: `excludeDir=["test"]` excludes `src/test/file.rs`
// but NOT `src/contest/file.rs`. Normalizes backslashes to forward slashes.
// This is the canonical implementation — all tools (grep, definitions, callers)

// ─── Set operations ─────────────────────────────────────────────────

/// Merge-intersect two sorted u32 slices. Returns sorted intersection.
pub(crate) fn sorted_intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => { result.push(a[i]); i += 1; j += 1; }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    result
}

pub(crate) fn ordered_trigram_posting_lists<'a>(
    term: &str,
    trigram_idx: &'a crate::TrigramIndex,
) -> Option<Vec<&'a [u32]>> {
    let mut trigrams = code_xray::generate_trigrams(term);
    trigrams.sort_unstable();
    trigrams.dedup();
    if trigrams.is_empty() {
        return None;
    }

    let mut posting_lists: Vec<&[u32]> = trigrams.iter()
        .map(|trigram| trigram_idx.trigram_map.get(trigram).map(Vec::as_slice))
        .collect::<Option<_>>()?;
    posting_lists.sort_unstable_by_key(|posting_list| posting_list.len());
    Some(posting_lists)
}

/// Returns `None` when the term has no trigrams or any trigram has no posting list.
pub(crate) fn intersect_trigram_postings(
    term: &str,
    trigram_idx: &crate::TrigramIndex,
) -> Option<Vec<u32>> {
    let mut posting_lists = ordered_trigram_posting_lists(term, trigram_idx)?.into_iter();
    let mut candidates = posting_lists.next()?.to_vec();
    for posting_list in posting_lists {
        candidates = sorted_intersect(&candidates, posting_list);
        if candidates.is_empty() {
            break;
        }
    }
    Some(candidates)
}

// ─── Line content helpers ───────────────────────────────────────────

/// Build compact grouped lineContent for xray_grep from raw file content.
/// Computes context windows around match lines, then groups consecutive lines
/// into `[{startLine, lines[], matchIndices[]}]`.
pub(crate) fn build_line_content_from_matches(
    content: &str,
    match_lines: &[u32],
    context_lines: usize,
) -> Value {
    let lines_vec: Vec<&str> = content.lines().collect();
    let total_lines = lines_vec.len();

    let mut lines_to_show = BTreeSet::new();
    let mut match_lines_set = HashSet::new();

    for &ln in match_lines {
        let idx = (ln as usize).saturating_sub(1);
        if idx < total_lines {
            match_lines_set.insert(idx);
            let s = idx.saturating_sub(context_lines);
            let e = (idx + context_lines).min(total_lines - 1);
            for i in s..=e { lines_to_show.insert(i); }
        }
    }

    build_grouped_line_content(&lines_to_show, &lines_vec, &match_lines_set)
}

/// Split index posting line numbers into those still corroborated by the
/// freshly-read file content, plus a count of stale ones that no longer match.
///
/// The inverted index records physical line numbers captured at index time,
/// but `lineContent` is rendered against content read fresh from disk. When the
/// file changed after indexing (lines inserted/removed), a posting line can
/// point at content that no longer contains the search term. Keeping only
/// corroborated lines guarantees the rendered `lineContent` never shows a
/// stale, non-matching line.
///
/// `terms_lower` MUST already be lowercased; each candidate line is matched
/// case-insensitively as a raw substring, mirroring token/substring search.
pub(crate) fn partition_verified_lines(
    content: &str,
    candidate_lines: &[u32],
    terms_lower: &[String],
) -> (Vec<u32>, usize) {
    let lines_vec: Vec<&str> = content.lines().collect();
    let total_lines = lines_vec.len();
    let mut verified = Vec::with_capacity(candidate_lines.len());
    let mut stale = 0usize;
    for &ln in candidate_lines {
        let idx = (ln as usize).saturating_sub(1);
        let corroborated = idx < total_lines && {
            let lower = lines_vec[idx].to_lowercase();
            terms_lower.iter().any(|t| lower.contains(t.as_str()))
        };
        if corroborated {
            verified.push(ln);
        } else {
            stale += 1;
        }
    }
    (verified, stale)
}

/// Warning surfaced when `partition_verified_lines` dropped stale posting lines
/// from `lineContent` for one or more files (index stale vs. on-disk content).
pub(crate) fn stale_lines_warning(file_count: usize) -> String {
    format!(
        "{} file(s): some indexed line numbers no longer match on-disk content \
         (file changed since indexing). lineContent shows only lines that still \
         contain the search term; stale line numbers are omitted. Reindex \
         (xray_reindex) for up-to-date positions.",
        file_count
    )
}

/// Warning surfaced when phrase search found candidate lines whose fresh content
/// no longer contains the phrase tokens (index stale vs. on-disk content) — the
/// phrase may still exist at a shifted position the stale index cannot point to.
pub(crate) fn stale_phrase_warning(file_count: usize) -> String {
    format!(
        "{} file(s): the index pointed phrase candidates at lines that no longer \
         contain the phrase tokens (file changed since indexing); matches may be \
         missed. Reindex (xray_reindex), or use lineRegex=true to scan raw lines.",
        file_count
    )
}

/// Groups consecutive lines into compact chunks: `[{startLine, lines[], matchIndices[]}]`.
pub(crate) fn build_grouped_line_content(
    lines_to_show: &BTreeSet<usize>,
    lines_vec: &[&str],
    match_lines_set: &HashSet<usize>,
) -> Value {
    let mut groups: Vec<Value> = Vec::new();
    let mut current_group_start: Option<usize> = None;
    let mut current_group_lines: Vec<&str> = Vec::new();
    let mut current_group_matches: Vec<usize> = Vec::new();

    let ordered_lines: Vec<usize> = lines_to_show.iter().cloned().collect();

    for (i, &idx) in ordered_lines.iter().enumerate() {
        let is_consecutive = i > 0 && idx == ordered_lines[i - 1] + 1;

        if !is_consecutive && !current_group_lines.is_empty() {
            let mut group = json!({
                "startLine": current_group_start.unwrap() + 1,
                "lines": current_group_lines,
            });
            if !current_group_matches.is_empty() {
                group["matchIndices"] = json!(current_group_matches);
            }
            groups.push(group);
            current_group_lines = Vec::new();
            current_group_matches = Vec::new();
        }

        if current_group_lines.is_empty() {
            current_group_start = Some(idx);
        }

        if match_lines_set.contains(&idx) {
            current_group_matches.push(current_group_lines.len());
        }
        current_group_lines.push(lines_vec[idx]);
    }

    if !current_group_lines.is_empty() {
        let mut group = json!({
            "startLine": current_group_start.unwrap() + 1,
            "lines": current_group_lines,
        });
        if !current_group_matches.is_empty() {
            group["matchIndices"] = json!(current_group_matches);
        }
        groups.push(group);
    }

    json!(groups)
}

// ─── Response size truncation ───────────────────────────────────────

/// Default maximum response size in bytes before truncation kicks in.
/// 16KB ≈ 4K tokens — keeps LLM context budget reasonable.
/// Can be overridden via `--max-response-kb` CLI flag.
/// Used by tests and as the fallback when no explicit budget is configured.
pub(crate) const DEFAULT_MAX_RESPONSE_BYTES: usize = 16_384;

// ─── PERF-08: single-flight gate for the file-list rebuild ──────────

/// Single-flight gate that ensures at most one in-flight build of
/// `HandlerContext.file_index` at any time, regardless of how many
/// concurrent `xray_fast` requests trigger a rebuild simultaneously.
///
/// See the field doc on `HandlerContext.file_index_build_gate` for the
/// motivation. Implementation: `Mutex<bool> + Condvar`. The bool is
/// `true` while a thread is inside `build_index`. Other threads observe
/// it under the mutex and either (a) return immediately if the index is
/// already fresh, or (b) block on the condvar until the in-flight build
/// signals completion, then re-check.
///
/// Held lock scope is intentionally tiny (state inspection only). The
/// expensive `build_index` call runs **outside** the mutex, so unrelated
/// `xray_fast` requests against an already-built / non-dirty index never
/// touch this gate at all.
pub struct FileIndexBuildGate {
    /// `true` while exactly one thread is inside the build closure.
    pub(crate) building: std::sync::Mutex<bool>,
    /// Signalled when a build finishes (success, error, or panic).
    pub(crate) done: std::sync::Condvar,
}

impl FileIndexBuildGate {
    pub fn new() -> Self {
        Self {
            building: std::sync::Mutex::new(false),
            done: std::sync::Condvar::new(),
        }
    }
}

impl Default for FileIndexBuildGate {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrigramReadiness {
    Ready,
    Dirty,
    Building,
    Busy,
}

impl TrigramReadiness {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Dirty => "dirty",
            Self::Building => "building",
            Self::Busy => "busy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrigramDirtyTrigger {
    Startup,
    Watcher,
    Edit,
    Unknown,
}

impl TrigramDirtyTrigger {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Watcher => "watcher",
            Self::Edit => "edit",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrigramBuildTrigger {
    Startup,
    Edit,
    Query,
    Unknown,
}

impl TrigramBuildTrigger {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Edit => "edit",
            Self::Query => "query",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TrigramTelemetry {
    last_dirty_trigger: TrigramDirtyTrigger,
    last_build_trigger: TrigramBuildTrigger,
    last_build_ms: Option<f64>,
    last_ready_at_ms: Option<u64>,
}

impl Default for TrigramTelemetry {
    fn default() -> Self {
        Self {
            last_dirty_trigger: TrigramDirtyTrigger::Unknown,
            last_build_trigger: TrigramBuildTrigger::Unknown,
            last_build_ms: None,
            last_ready_at_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TrigramRuntimeSnapshot {
    pub(crate) status: TrigramReadiness,
    pub(crate) last_dirty_trigger: TrigramDirtyTrigger,
    pub(crate) last_build_trigger: TrigramBuildTrigger,
    pub(crate) last_build_ms: Option<f64>,
    pub(crate) last_ready_at_ms: Option<u64>,
}

/// Single-flight gate for trigram rebuilds.
pub struct TrigramRebuildGate {
    pub(crate) building: std::sync::Mutex<bool>,
    pub(crate) done: std::sync::Condvar,
    telemetry: std::sync::Mutex<TrigramTelemetry>,
}

impl TrigramRebuildGate {
    pub fn new() -> Self {
        Self {
            building: std::sync::Mutex::new(false),
            done: std::sync::Condvar::new(),
            telemetry: std::sync::Mutex::new(TrigramTelemetry::default()),
        }
    }

    pub fn is_building(&self) -> bool {
        self.building.lock().map(|building| *building).unwrap_or(false)
    }

    pub(crate) fn snapshot(
        &self,
        index: &std::sync::RwLock<crate::ContentIndex>,
    ) -> TrigramRuntimeSnapshot {
        let index = match index.try_read() {
            Ok(index) => Some(index),
            Err(std::sync::TryLockError::Poisoned(error)) => Some(error.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        let building = self.building.lock().unwrap_or_else(|error| error.into_inner());
        let status = if *building {
            TrigramReadiness::Building
        } else {
            match index {
                Some(ref index) if index.trigram_dirty => TrigramReadiness::Dirty,
                Some(_) => TrigramReadiness::Ready,
                None => TrigramReadiness::Busy,
            }
        };
        let telemetry = self.telemetry.lock().unwrap_or_else(|error| error.into_inner());
        TrigramRuntimeSnapshot {
            status,
            last_dirty_trigger: telemetry.last_dirty_trigger,
            last_build_trigger: telemetry.last_build_trigger,
            last_build_ms: telemetry.last_build_ms,
            last_ready_at_ms: telemetry.last_ready_at_ms,
        }
    }

    pub(crate) fn mark_dirty(&self, trigger: TrigramDirtyTrigger) {
        self.telemetry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last_dirty_trigger = trigger;
    }

    pub(crate) fn record_build_completed(&self, trigger: TrigramBuildTrigger, build_ms: f64) {
        let ready_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        let mut telemetry = self.telemetry.lock().unwrap_or_else(|error| error.into_inner());
        telemetry.last_build_trigger = trigger;
        telemetry.last_build_ms = Some(build_ms);
        telemetry.last_ready_at_ms = ready_at_ms;
    }
}

impl Default for TrigramRebuildGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Single-flight wrapper around the `xray_fast` file-index rebuild.
///
/// Contract:
/// - Returns `Ok(())` once `ctx.file_index` is populated **and** is
///   not stale (i.e. `file_index_dirty == false`).
/// - At most one caller at a time runs `build_fn`. All other callers
///   that arrive while a build is in flight wait on the condvar and
///   re-check after wake-up; if the in-flight build succeeded the
///   waiter returns `Ok(())` without invoking `build_fn`. If the
///   in-flight build panicked or set the index to `None`, exactly
///   one waiter takes over as the new builder.
/// - On `Err` from `build_fn`, the gate is released (other waiters
///   unblocked) and the error is propagated to this caller. Other
///   waiters re-check on wake-up and one of them retries the build.
/// - **Panic safety:** the build slot is held via an RAII guard; if
///   `build_fn` unwinds, the guard's `Drop` clears `building=false`
///   and notifies all waiters before unwinding propagates.
pub fn ensure_file_index<F>(
    ctx: &HandlerContext,
    build_fn: F,
) -> Result<(), String>
where
    F: FnOnce() -> Result<crate::FileIndex, String>,
{
    use std::sync::atomic::Ordering;

    /// RAII guard: clears the building flag and wakes waiters even on
    /// panic. Constructed with the flag already set to `true` by the
    /// caller, so `Drop` is the *only* code path that resets it.
    struct BuildSlotGuard<'a> {
        gate: &'a FileIndexBuildGate,
    }
    impl Drop for BuildSlotGuard<'_> {
        fn drop(&mut self) {
            let mut b = self.gate.building.lock().unwrap_or_else(|e| e.into_inner());
            *b = false;
            // Wake every waiter — they all need to re-check whether
            // the index is now ready or whether a retry is required.
            self.gate.done.notify_all();
        }
    }

    /// RAII guard: restores `file_index_dirty=true` if the build does
    /// NOT complete successfully (early return via `?`, panic, etc.).
    /// We cleared `dirty` *before* the build (single-flight guarantee
    /// against lost watcher invalidations — see `pre_build_dirty` swap
    /// below) so a failed build must put it back, otherwise the next
    /// caller would skip the rebuild on a still-stale signal.
    struct DirtyRestoreGuard<'a> {
        ctx: &'a HandlerContext,
        pre_build_dirty: bool,
        armed: bool,
    }
    impl Drop for DirtyRestoreGuard<'_> {
        fn drop(&mut self) {
            if self.armed && self.pre_build_dirty {
                self.ctx
                    .file_index_dirty
                    .store(true, Ordering::Relaxed);
            }
        }
    }

    // Single-flight loop: spin only on logical state transitions
    // (Idle→Building→Idle), never on raw timing. Each iteration either
    // returns or sleeps on the condvar.
    loop {
        let mut building = ctx
            .file_index_build_gate
            .building
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Re-evaluate `needs_rebuild` under the gate's mutex so that a
        // builder that just released the slot can publish its result
        // before we decide. (The `file_index` RwLock is taken briefly
        // inside this expression — that read lock is not contended
        // here because the only writer is the build path itself, which
        // we are coordinating.)
        let needs_rebuild = ctx.file_index_dirty.load(Ordering::Relaxed)
            || ctx
                .file_index
                .read()
                .map(|fi| fi.is_none())
                .unwrap_or(true);

        if !needs_rebuild {
            return Ok(());
        }

        if *building {
            // Another thread owns the slot. Block on the condvar; the
            // RAII guard in the builder will notify_all on completion
            // (success, error, or panic). After wake-up we loop and
            // re-check, which is the correct response to spurious
            // wake-ups too.
            while *building {
                building = ctx
                    .file_index_build_gate
                    .done
                    .wait(building)
                    .unwrap_or_else(|e| e.into_inner());
            }
            // Loop continues — re-checks `needs_rebuild` and either
            // returns `Ok(())` (the previous build succeeded and is
            // visible now) or takes the build slot ourselves (the
            // previous build failed/panicked; index is still missing).
            continue;
        }

        // We become the builder. Flip the flag while still holding the
        // lock so no other thread can also enter this branch in the
        // same instant. Then drop the lock so the actual build runs
        // unsynchronised.
        *building = true;
        drop(building);

        // From here until function return / unwind, _slot is alive and
        // its Drop will reset `building=false` + `notify_all` waiters.
        let _slot = BuildSlotGuard {
            gate: &ctx.file_index_build_gate,
        };

        // Clear the dirty flag BEFORE running `build_fn` (not after) so
        // that any watcher signal arriving DURING the build is preserved
        // — the next caller will then rebuild against the new fs state.
        // Pre-fix this used an unconditional `store(false)` *after*
        // publishing the index, which silently erased mid-build
        // invalidations and left the cache stale until the next watcher
        // event after the build finished.
        let pre_build_dirty = ctx.file_index_dirty.swap(false, Ordering::Relaxed);
        let mut restore = DirtyRestoreGuard {
            ctx,
            pre_build_dirty,
            armed: true,
        };

        let new_index = build_fn()?; // ← `?` propagates; restore + _slot drop on the way out

        // Publish the freshly-built index. We do NOT touch
        // `file_index_dirty` here — it's either still `false` (no
        // watcher signal during build, our pre-build swap is the
        // current state) or has been re-set to `true` by a watcher
        // mid-build (the next caller will rebuild, exactly as
        // intended). Disarm the restore guard now that build succeeded.
        if let Ok(mut fi) = ctx.file_index.write() {
            *fi = Some(new_index);
        }
        restore.armed = false;
        return Ok(());
    }
}

pub fn ensure_workspace_file_index(ctx: &HandlerContext) -> Result<(), String> {
    let server_dir = ctx.server_dir();
    let respect_git_exclude = ctx.respect_git_exclude;
    let index_base = ctx.index_base.clone();
    ensure_file_index(ctx, || {
        tracing::info!(dir = %server_dir, "Building file-list index (dirty or first use)");
        let new_index = crate::build_index(&crate::IndexArgs {
            dir: server_dir,
            max_age_hours: 24,
            hidden: false,
            no_ignore: false,
            respect_git_exclude,
            threads: 0,
        })
        .map_err(|error| format!("Failed to build file index: {}", error))?;
        let _ = crate::save_index(&new_index, &index_base);
        Ok(new_index)
    })
}


/// Maximum number of line numbers to include per file entry.
const MAX_LINES_PER_FILE: usize = 10;

/// Maximum number of preview source lines to keep per file before dropping `lineContent`.
const MAX_LINE_CONTENT_LINES_PER_FILE: usize = MAX_LINES_PER_FILE * 5;

/// Maximum number of matched tokens to include in substring search summary.
const MAX_MATCHED_TOKENS: usize = 20;

/// Truncate a JSON response to fit within the response size budget.
///
/// Progressive truncation strategy:
/// 1. Cap per-file `lines` arrays and bounded `lineContent` previews
/// 2. Cap `matchedTokens` in summary (keep first N, add `matchedTokensOmitted`)
/// 3. Remove file entries from the tail until under budget
/// 4. Remove `lines` arrays entirely from file entries
/// 5. Strip large body fields while preserving signatures and metadata
/// 6. Remove `lineContent` previews only if capped previews still exceed budget
/// 7. **Generic fallback**: truncate any top-level array (e.g. `definitions`,
///    `containingDefinitions`, `callTree`) — covers non-grep response formats
///
/// `max_bytes` = 0 disables truncation entirely.
/// Returns the (possibly truncated) JSON value.
/// Build the policy reminder string emitted according to the configured cadence.
/// Uses **imperative enforcement framing** (REQUIRED, NO EXCEPTIONS, STOP,
/// protocol error) rather than passive advice ("Prefer xray").
///
/// When `indexed_ext` is non-empty, appends a VIOLATION clause that explicitly
/// lists the configured indexed extensions and names the required xray tool for
/// each operation (read/search/edit). This is designed to close rationalization
/// lanes ("habit", "familiarity", "just this once") that tolerate built-in tool
/// fallback.
///
/// The wording is covered by `test_build_policy_reminder_is_imperative` which
/// guards against regression to passive phrasing.
fn build_policy_reminder(indexed_ext: &str) -> String {
    let ext_line = if indexed_ext.trim().is_empty() {
        String::new()
    } else {
        format!(
            " VIOLATION = calling built-in read_file/search_files/apply_diff on files with extensions [{}]. REQUIRED: xray_definitions (read), xray_grep (search), xray_edit (edit).",
            indexed_ext
        )
    };
    format!(
        "=== XRAY_POLICY - ENFORCEMENT === \
         REQUIRED: use xray_* MCP tools for all read/search/edit operations on indexed files. \
         NO EXCEPTIONS for 'familiarity', 'habit', 'quick check', or 'just this once'. \
         If about to call a built-in on an indexed file -> STOP, use the xray equivalent. \
         Built-in calls when xray covers the case = protocol error.{} \
         INTENT->TOOL: context-around-match->xray_grep showLines | read-method-body->xray_definitions includeBody | stack-trace (file:line)->xray_definitions containsLine | replace-in-files->xray_edit | list-dir->xray_fast dirsOnly | find-callers->xray_callers. \
         ================================",
        ext_line
    )
}

struct ResponseGuidanceContext<'a> {
    tool_name: &'a str,
    output: &'a Value,
    handler_ctx: &'a HandlerContext,
    invocation_args: Option<&'a Value>,
}

fn next_step_hint(context: &ResponseGuidanceContext<'_>) -> Option<String> {
    if matches!(context.output.pointer("/summary/scopeDiagnostic/reason").and_then(Value::as_str),
        Some("fully_excluded" | "file_not_found")) {
        return Some("Resolve summary.scopeDiagnostic within the original restrictions before choosing another query.".to_string());
    }
    match context.tool_name {
        "xray_grep" => grep_or_fast_next_step(context, "grep")
            .or_else(|| Some("Next: narrow content results with xray_grep filters, or use xray_definitions when a parser-active target file is known".to_string())),
        "xray_fast" => grep_or_fast_next_step(context, "fast")
            .or_else(|| Some("Next: use xray_definitions file=[\"<match>\"] to inspect parser-active files, or xray_grep for content".to_string())),
        "xray_definitions" => Some("Next: use xray_callers for call chains or xray_grep for text patterns".to_string()),
        "xray_callers" => Some("Next: use xray_definitions includeBody=true for source or xray_grep for text refs".to_string()),
        "xray_edit" => Some("Next: use xray_definitions to verify or xray_grep to check related files".to_string()),
        "xray_git_history" | "xray_git_diff" | "xray_git_authors" | "xray_git_activity" | "xray_git_blame" | "xray_branch_status" => {
            Some("Next: use xray_definitions for code context or xray_callers for impact".to_string())
        }
        _ => None,
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSymbolQuery {
    file: [String; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    exclude_dir: Option<Vec<String>>,
    include_body: bool,
    auto_correct: bool,
}

fn selected_file_symbol_query(context: &ResponseGuidanceContext<'_>) -> Result<Option<Value>, &'static str> {
    let Some(path) = top_result_path(context.output) else { return Ok(None); };
    let Some(extension) = extension_for_path(path) else { return Ok(None); };
    if !definition_parser_active(context.handler_ctx, &extension) {
        if extension == "html" && has_typescript_parser(context.handler_ctx)
            && selector_from_invocation_args(context.invocation_args).is_some() {
            return Err("No equivalent xray_callers query can retain the selected template file scope; no template-navigation query was generated.");
        }
        return Ok(None);
    }
    let Some(args) = context.invocation_args else {
        return Err("No equivalent query was generated because the original restrictions are unavailable.");
    };
    if super::arg_validation::check_unknown_args(context.tool_name, args).is_some()
        || args.get("exclude").and_then(Value::as_array).is_some_and(|items| !items.is_empty())
        || args.get("invert").and_then(Value::as_bool) == Some(true) {
        return Err("No equivalent xray_definitions query can retain these search restrictions; no cross-tool query was generated.");
    }
    let exclude_dir = args.get("excludeDir").and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let path = std::path::Path::new(path);
    let file = if path.is_absolute() { path.to_path_buf() } else {
        std::path::Path::new(&context.handler_ctx.server_dir()).join(path)
    };
    let query = FileSymbolQuery {
        file: [crate::clean_path(&file.to_string_lossy())], exclude_dir,
        include_body: false, auto_correct: false,
    };
    Ok(Some(json!({
        "tool": "xray_definitions", "args": query,
        "operation": "inspectSelectedFile", "semanticsChanged": true,
        "reason": "List symbols in this selected file, not content matches or file-name matches. Select a definition before reading its body.",
    })))
}

fn grep_or_fast_next_step(context: &ResponseGuidanceContext<'_>, _source: &str) -> Option<String> {
    let path = top_result_path(context.output)?;
    match selected_file_symbol_query(context) {
        Ok(Some(_)) => Some(format!(
            "Next: summary.nextStepQuery lists symbols in '{}'; select a definition before reading its body.",
            workspace_relative_hint_path(path, context.handler_ctx),
        )),
        Ok(None) => None,
        Err(reason) => Some(reason.to_string()),
    }
}

fn top_result_path(output: &Value) -> Option<&str> {
    output
        .get("files")?
        .as_array()?
        .first()?
        .get("path")?
        .as_str()
}

fn extension_for_path(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

fn definition_parser_active(ctx: &HandlerContext, extension: &str) -> bool {
    ctx.def_extensions
        .iter()
        .any(|ext| ext.eq_ignore_ascii_case(extension))
}

fn has_typescript_parser(ctx: &HandlerContext) -> bool {
    ctx.def_extensions
        .iter()
        .any(|ext| ext.eq_ignore_ascii_case("ts") || ext.eq_ignore_ascii_case("tsx"))
}

fn workspace_relative_hint_path(path: &str, ctx: &HandlerContext) -> String {
    let normalized = normalize_path_sep(path);
    if !(std::path::Path::new(path).is_absolute() || normalized.contains(':')) {
        return normalized;
    }

    let server_dir = normalize_path_sep(&ctx.server_dir());
    let server_dir = server_dir.trim_end_matches('/');
    let normalized_lower = normalized.to_ascii_lowercase();
    let server_lower = server_dir.to_ascii_lowercase();
    let prefix = format!("{}/", server_lower);
    if normalized_lower.starts_with(&prefix) {
        normalized[server_dir.len() + 1..].to_string()
    } else {
        normalized
    }
}

fn selector_from_invocation_args(invocation_args: Option<&Value>) -> Option<String> {
    let terms = invocation_args?.get("terms")?.as_array()?;
    terms
        .iter()
        .filter_map(|value| value.as_str())
        .find_map(selector_from_text)
}

fn selector_from_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let tag_body = trimmed
        .strip_prefix("</")
        .or_else(|| trimmed.strip_prefix('<'))?;
    let selector: String = tag_body
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect();
    if selector.contains('-')
        && selector
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
    {
        Some(selector)
    } else {
        None
    }
}

#[cfg(test)]
pub(crate) fn inject_response_guidance(result: ToolCallResult, tool_name: &str, indexed_ext: &str, ctx: &super::HandlerContext) -> ToolCallResult {
    inject_response_guidance_with_args(result, tool_name, indexed_ext, ctx, None)
}

fn is_index_backed_data_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "xray_grep" | "xray_definitions" | "xray_callers" | "xray_fast"
    )
}

fn branch_source_name(source: BranchSource) -> &'static str {
    match source {
        BranchSource::Live => "live_or_cached",
        BranchSource::Startup => "startup",
        BranchSource::Unknown => "unknown",
    }
}

fn build_analysis_context(ctx: &HandlerContext, allow_live_probe: bool) -> Value {
    let (workspace_root, workspace_status, workspace_mode, workspace_generation, workspace_resolved) = {
        let workspace = ctx.workspace.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            if workspace.canonical_dir.is_empty() {
                workspace.dir.clone()
            } else {
                workspace.canonical_dir.clone()
            },
            workspace.status.to_string(),
            workspace.mode.to_string(),
            workspace.generation,
            workspace.status == super::WorkspaceStatus::Resolved,
        )
    };
    // Error/unresolved paths use startup state instead of probing a possibly unrelated CWD.
    let (branch, branch_source) = if allow_live_probe && workspace_resolved {
        current_branch_resolved(ctx)
    } else {
        (
            ctx.current_branch.clone(),
            if ctx.current_branch.is_some() { BranchSource::Startup } else { BranchSource::Unknown },
        )
    };
    json!({
        "schemaVersion": 1,
        "source": {
            "targetKind": "local_worktree",
            "workspaceRoot": workspace_root,
            "workspaceStatus": workspace_status,
            "workspaceBindingMode": workspace_mode,
            "workspaceBindingGeneration": workspace_generation,
            "branch": {
                "name": branch,
                "source": branch_source_name(branch_source),
            },
            "revisionEvidence": "not_verified",
        }
    })
}


pub(crate) fn tool_result_output(result: &ToolCallResult) -> Option<Value> {
    let text = &result.content.first()?.text;
    match serde_json::from_str::<Value>(text) {
        Ok(output) => Some(output),
        // Plain-text errors need an envelope for diagnostics; plain successes
        // remain unchanged.
        Err(_) if result.is_error => Some(json!({ "error": text, "summary": {} })),
        Err(_) => None,
    }
}

pub(crate) fn inject_response_guidance_with_args(
    result: ToolCallResult,
    tool_name: &str,
    indexed_ext: &str,
    ctx: &super::HandlerContext,
    invocation_args: Option<&Value>,
) -> ToolCallResult {
    let was_error = result.is_error;
    let mut output = match tool_result_output(&result) {
        Some(output) => output,
        None => return result,
    };

    {
        let Some(obj) = output.as_object_mut() else {
            return result;
        };

        if tool_name == "xray_info" {
            obj.insert("softwareUpdate".to_string(), ctx.updates.snapshot());
        }
        if !obj.contains_key("summary") {
            obj.insert("summary".to_string(), json!({}));
        }
    }

    let index_backed_data_tool = is_index_backed_data_tool(tool_name);
    if index_backed_data_tool
        && let Some(object) = output.as_object_mut()
    {
        object.insert(
            "analysisContext".to_string(),
            build_analysis_context(ctx, !was_error),
        );
    }

    let mut context_query = None;
    let context_hint = if output.pointer("/summary/nextStepHint").is_none() {
        let guidance_context = ResponseGuidanceContext {
            tool_name,
            output: &output,
            handler_ctx: ctx,
            invocation_args,
        };
        if !was_error && matches!(tool_name, "xray_grep" | "xray_fast") {
            context_query = selected_file_symbol_query(&guidance_context).ok().flatten();
        }
        next_step_hint(&guidance_context)
    } else {
        None
    };

    if let Some(summary) = output.get_mut("summary").and_then(|v| v.as_object_mut()) {
        if let Some(query) = context_query { summary.insert("nextStepQuery".to_string(), query); }
        summary.insert("policyReminder".to_string(), json!(build_policy_reminder(indexed_ext)));
        if let Some(hint) = context_hint {
            // Tool handlers may set a more specific nextStepHint; use the generic
            // guidance only when the response did not already provide one.
            summary.entry("nextStepHint".to_string()).or_insert(json!(hint));
        }
        if index_backed_data_tool {
            summary.remove("serverDir");
            summary.remove("workspaceStatus");
            summary.remove("workspaceSource");
            summary.remove("workspaceGeneration");
        } else if let Ok(ws) = ctx.workspace.read() {
            summary.insert("serverDir".to_string(), json!(ws.dir));
            summary.insert("workspaceStatus".to_string(), json!(ws.status.to_string()));
            summary.insert("workspaceSource".to_string(), json!(ws.mode.to_string()));
            summary.insert("workspaceGeneration".to_string(), json!(ws.generation));
        }
    }

    ToolCallResult::success(json_to_string(&output))
}


pub(crate) const POLICY_REMINDER_INTERVAL_RESPONSES: u64 = 25;
pub(crate) const POLICY_REMINDER_IDLE_SECS: u64 = 30 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PolicyReminderMode {
    Adaptive,
    Always,
    Off,
}

#[cfg(test)]
thread_local! {
    static POLICY_REMINDER_MODE_TEST_OVERRIDE: std::cell::Cell<Option<PolicyReminderMode>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_policy_reminder_mode_override_for_test(
    value: Option<PolicyReminderMode>,
) -> Option<PolicyReminderMode> {
    POLICY_REMINDER_MODE_TEST_OVERRIDE.with(|override_value| {
        let previous = override_value.get();
        override_value.set(value);
        previous
    })
}

pub(crate) fn policy_reminder_mode_from_env_value(value: Option<&str>) -> PolicyReminderMode {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("always" | "1" | "true" | "yes" | "on") => PolicyReminderMode::Always,
        Some("off" | "0" | "false" | "no") => PolicyReminderMode::Off,
        _ => PolicyReminderMode::Adaptive,
    }
}

pub(crate) fn policy_reminder_mode() -> PolicyReminderMode {
    #[cfg(test)]
    {
        POLICY_REMINDER_MODE_TEST_OVERRIDE
            .with(|override_value| override_value.get().unwrap_or(PolicyReminderMode::Adaptive))
    }

    #[cfg(not(test))]
    {
        policy_reminder_mode_from_env_value(
            std::env::var("XRAY_POLICY_REMINDER").ok().as_deref(),
        )
    }
}

pub(crate) fn current_unix_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Production MCP dispatch is sequential; atomics keep shared test contexts lock-free.
#[derive(Debug, Default)]
pub(crate) struct PolicyReminderState {
    response_count: std::sync::atomic::AtomicU64,
    last_response_secs: std::sync::atomic::AtomicU64,
}

impl PolicyReminderState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn should_emit(&self, mode: PolicyReminderMode, now_secs: u64) -> bool {
        use std::sync::atomic::Ordering;

        let response_count = self.response_count.fetch_add(1, Ordering::Relaxed) + 1;
        let previous_response_secs = self.last_response_secs.swap(now_secs, Ordering::Relaxed);
        match mode {
            PolicyReminderMode::Always => true,
            PolicyReminderMode::Off => false,
            PolicyReminderMode::Adaptive => {
                response_count == 1
                    || response_count.is_multiple_of(POLICY_REMINDER_INTERVAL_RESPONSES)
                    || (previous_response_secs != 0
                        && now_secs.saturating_sub(previous_response_secs)
                            >= POLICY_REMINDER_IDLE_SECS)
            }
        }
    }
}

const GUIDANCE_PREFIX_HEADER: &str = "=== XRAY AGENT GUIDANCE ===";
const GUIDANCE_PREFIX_FOOTER: &str = "===============================";
const COMPACT_POLICY_GUIDANCE: &str = concat!(
    "Use xray_* MCP tools for indexed files; ",
    "built-in read/search/edit equivalents are policy violations."
);

fn guidance_prefix_env_value_enabled(value: Option<&str>) -> bool {
    value
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
thread_local! {
    static GUIDANCE_PREFIX_TEST_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_guidance_prefix_override_for_test(value: Option<bool>) -> Option<bool> {
    GUIDANCE_PREFIX_TEST_OVERRIDE.with(|override_value| {
        let previous = override_value.get();
        override_value.set(value);
        previous
    })
}

pub(crate) fn guidance_prefix_enabled() -> bool {
    #[cfg(test)]
    {
        GUIDANCE_PREFIX_TEST_OVERRIDE
            .with(|override_value| override_value.get().unwrap_or(false))
    }

    #[cfg(not(test))]
    {
        // 2026-05-04: prefix mode (present guidance fields as raw text before
        // the JSON suffix) is now the default. Set
        // XRAY_GUIDANCE_PREFIX=0|false|no|off to opt back into the legacy
        // mode where guidance fields stay inside `summary` of the JSON body.
        match std::env::var("XRAY_GUIDANCE_PREFIX").ok() {
            None => true,
            Some(v) => guidance_prefix_env_value_enabled(Some(&v)),
        }
    }
}

#[cfg(test)]
pub(crate) fn render_guidance_prefix_if_enabled(
    result: ToolCallResult,
    tool_name: &str,
) -> ToolCallResult {
    render_guidance_prefix_for_policy(result, tool_name, true)
}

fn merge_truncation_details(output: &Value, details: &mut Vec<String>) {
    let Some(values) = output["resultStatus"]["truncationDetails"].as_array() else {
        return;
    };
    for value in values.iter().filter_map(Value::as_str) {
        if !details.iter().any(|detail| detail == value) {
            details.push(value.to_string());
        }
    }
}

fn restore_truncation_provenance(
    output: &mut Value,
    details: &[String],
    original_bytes: usize,
) {
    if details.is_empty() {
        return;
    }
    if let Some(summary) = output.get_mut("summary") {
        summary["originalResponseBytes"] = json!(original_bytes);
        summary["truncationReason"] = json!(details.join("; "));
    }
    output["resultStatus"]["truncationDetails"] = json!(details);
}

fn fit_guidance_output(
    mut output: Value,
    max_bytes: usize,
    prefix_bytes: usize,
    has_response_bytes: bool,
    has_estimated_tokens: bool,
) -> (Value, bool) {
    if max_bytes == 0 {
        return (output, false);
    }
    let suffix_budget = max_bytes.saturating_sub(prefix_bytes).max(1);
    let original_bytes = output["summary"]["originalResponseBytes"]
        .as_u64()
        .map(|bytes| bytes as usize)
        .unwrap_or_else(|| measure_json_size(&output));
    if let Some(summary) = output.get_mut("summary") {
        if has_response_bytes {
            summary["responseBytes"] = json!(max_bytes);
        }
        if has_estimated_tokens {
            summary["estimatedTokens"] = json!(max_bytes / 4);
        }
    }
    let truncation_applied = measure_json_size(&output) > suffix_budget;
    let mut truncation_details = Vec::new();
    merge_truncation_details(&output, &mut truncation_details);
    let mut fit_budget = suffix_budget;
    for _ in 0..8 {
        output = truncate_large_response(output, fit_budget);
        merge_truncation_details(&output, &mut truncation_details);
        restore_truncation_provenance(&mut output, &truncation_details, original_bytes);
        let fitted_size = measure_json_size(&output);
        if fitted_size <= suffix_budget {
            break;
        }
        let overflow = fitted_size.saturating_sub(suffix_budget).max(1);
        let next_budget = fit_budget.saturating_sub(overflow).max(1);
        if next_budget == fit_budget {
            break;
        }
        fit_budget = next_budget;
    }
    let structured_error = is_response_truncation_error(&output, truncation_applied);
    let residual_size_error = measure_json_size(&output) > suffix_budget;
    (output, structured_error || residual_size_error)
}

#[cfg(test)]
pub(crate) fn render_guidance_prefix_for_policy(
    result: ToolCallResult,
    tool_name: &str,
    include_policy_reminder: bool,
) -> ToolCallResult {
    render_guidance_prefix_for_policy_with_budget(
        result,
        tool_name,
        include_policy_reminder,
        0,
    )
}

pub(crate) fn render_guidance_prefix_for_policy_with_budget(
    result: ToolCallResult,
    _tool_name: &str,
    include_policy_reminder: bool,
    max_bytes: usize,
) -> ToolCallResult {
    // Prefix presentation and policy cadence are independent. Every response is
    // budgeted with the policy present; off-cycle rendering removes it only from
    // the final wire text, keeping result truncation deterministic.
    let prefix_enabled = guidance_prefix_enabled();

    let was_error = result.is_error;
    let text = match result.content.first() {
        Some(c) => c.text.clone(),
        None => return result,
    };

    let mut output = match serde_json::from_str::<Value>(&text) {
        Ok(v) => v,
        Err(_) => return result,
    };

    let Some(summary) = output.get_mut("summary").and_then(|v| v.as_object_mut()) else {
        return if prefix_enabled {
            truncate_response_if_needed(result, max_bytes)
        } else {
            result
        };
    };

    let next_step_hint = summary
        .get("nextStepHint")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(str::to_string);
    let has_policy_reminder = include_policy_reminder
        && summary
            .get("policyReminder")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|reminder| !reminder.is_empty());
    // 2026-05-05: also promote `unknownArgsWarning` into the prefix so the
    // hint about a silently-ignored argument is visible BEFORE the JSON body.
    // The previous summary-only placement was easy to miss when the LLM
    // skimmed top-level fields and reported wrong-scope results (e.g.
    // `topN=15` ignored → "top 1 author" reported as if 15 was honored).
    // The field is intentionally NOT removed from `summary` — programmatic
    // clients (and existing tests) keep reading `summary.unknownArgsWarning`
    // unchanged. The prefix carries a short, human-friendly mirror.
    let unknown_args_warning = summary
        .get("unknownArgsWarning")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|warning| !warning.is_empty())
        .map(str::to_string);

    let has_response_bytes = summary.contains_key("responseBytes");
    let has_estimated_tokens = summary.contains_key("estimatedTokens");
    if !include_policy_reminder {
        summary.remove("policyReminder");
    }

    if !prefix_enabled {
        let (output, force_error) = if has_response_bytes || has_estimated_tokens
            || super::body_delivery::has_body_reads(&output)
        {
            fit_guidance_output(output, max_bytes, 0, has_response_bytes, has_estimated_tokens)
        } else {
            (output, false)
        };
        let text = serialize_guidance_response(
            output,
            None,
            has_response_bytes,
            has_estimated_tokens,
        );
        let rendered = ToolCallResult::success(text);
        return if was_error || force_error {
            ToolCallResult { is_error: true, ..rendered }
        } else {
            rendered
        };
    }

    if next_step_hint.is_none() && !has_policy_reminder && unknown_args_warning.is_none() {
        let (output, force_error) = fit_guidance_output(
            output,
            max_bytes,
            0,
            has_response_bytes,
            has_estimated_tokens,
        );
        let text = serialize_guidance_response(
            output,
            None,
            has_response_bytes,
            has_estimated_tokens,
        );
        let rendered = ToolCallResult::success(text);
        return if was_error || force_error {
            ToolCallResult { is_error: true, ..rendered }
        } else {
            rendered
        };
    }

    summary.remove("nextStepHint");
    summary.remove("policyReminder");

    let build_guidance_prefix = |include_warning: bool, include_next_step: bool| {
        let mut guidance_lines = Vec::new();
        if include_warning
            && let Some(warning) = unknown_args_warning.as_deref()
        {
            guidance_lines.push(format!("⚠ {}", warning));
        }
        if include_next_step
            && let Some(hint) = next_step_hint.as_deref()
        {
            guidance_lines.push(format!("→ {}", hint));
        }
        if has_policy_reminder {
            guidance_lines.push(COMPACT_POLICY_GUIDANCE.to_string());
            format!(
                "{}\n{}\n{}",
                GUIDANCE_PREFIX_HEADER,
                guidance_lines.join("\n"),
                GUIDANCE_PREFIX_FOOTER
            )
        } else {
            guidance_lines.join("\n")
        }
    };
    let fit_with_prefix = |guidance_prefix: &str| {
        fit_guidance_output(
            output.clone(),
            max_bytes,
            if guidance_prefix.is_empty() {
                0
            } else {
                guidance_prefix.len().saturating_add(2)
            },
            has_response_bytes,
            has_estimated_tokens,
        )
    };

    let exceeds_wire_budget = |output: &Value, guidance_prefix: &str| {
        max_bytes > 0
            && measure_json_size(output)
                .saturating_add(if guidance_prefix.is_empty() {
                    0
                } else {
                    guidance_prefix.len().saturating_add(2)
                })
                > max_bytes
    };
    let mut guidance_prefix = build_guidance_prefix(true, true);
    let (mut output, mut force_error) = fit_with_prefix(&guidance_prefix);
    if exceeds_wire_budget(&output, &guidance_prefix) && unknown_args_warning.is_some() {
        guidance_prefix = build_guidance_prefix(false, true);
        (output, force_error) = fit_with_prefix(&guidance_prefix);
    }
    if exceeds_wire_budget(&output, &guidance_prefix) && next_step_hint.is_some() {
        guidance_prefix = build_guidance_prefix(false, false);
        (output, force_error) = fit_with_prefix(&guidance_prefix);
    }
    let prefix = (!guidance_prefix.is_empty()).then_some(guidance_prefix.as_str());
    let text = serialize_guidance_response(
        output,
        prefix,
        has_response_bytes,
        has_estimated_tokens,
    );
    let rendered = ToolCallResult::success(text);
    if was_error || force_error {
        ToolCallResult {
            is_error: true,
            ..rendered
        }
    } else {
        rendered
    }
}

fn serialize_guidance_response(
    mut output: Value,
    guidance_prefix: Option<&str>,
    has_response_bytes: bool,
    has_estimated_tokens: bool,
) -> String {
    super::body_delivery::finalize_metadata(&mut output);
    let render = |output: &Value| {
        let json_suffix = json_to_string(output);
        guidance_prefix.map_or_else(
            || json_suffix.clone(),
            |prefix| format!("{}\n\n{}", prefix, json_suffix),
        )
    };

    for _ in 0..8 {
        let final_text = render(&output);
        let final_bytes = final_text.len() as u64;
        let estimated_tokens = final_bytes / 4;

        let Some(summary) = output.get_mut("summary").and_then(|v| v.as_object_mut()) else {
            return final_text;
        };

        let mut changed = false;
        if has_response_bytes
            && summary.get("responseBytes").and_then(Value::as_u64) != Some(final_bytes)
        {
            summary.insert("responseBytes".to_string(), json!(final_bytes));
            changed = true;
        }
        if has_estimated_tokens
            && summary.get("estimatedTokens").and_then(Value::as_u64) != Some(estimated_tokens)
        {
            summary.insert("estimatedTokens".to_string(), json!(estimated_tokens));
            changed = true;
        }

        if !changed {
            return final_text;
        }
    }

    render(&output)
}

/// Measure the JSON-serialized size of a Value in bytes.
fn measure_json_size(output: &Value) -> usize {
    super::body_delivery::delivered_size(output)
}

/// Phase 1: Cap per-file line metadata while preserving a bounded `lineContent` preview.
fn phase_cap_lines_per_file(output: &mut Value, reasons: &mut Vec<String>) {
    let mut capped_lines = false;
    let mut capped_line_content = false;

    if let Some(files) = output.get_mut("files").and_then(|f| f.as_array_mut()) {
        for file_entry in files.iter_mut() {
            if let Some(lines) = file_entry.get_mut("lines").and_then(|l| l.as_array_mut())
                && lines.len() > MAX_LINES_PER_FILE {
                    let omitted = lines.len() - MAX_LINES_PER_FILE;
                    lines.truncate(MAX_LINES_PER_FILE);
                    file_entry["linesOmitted"] = json!(omitted);
                    capped_lines = true;
                }
            capped_line_content |= cap_line_content_preview(file_entry);
        }
    }

    if capped_lines {
        reasons.push(format!("capped lines per file to {}", MAX_LINES_PER_FILE));
    }
    if capped_line_content {
        reasons.push(format!(
            "capped lineContent preview per file to {} source lines",
            MAX_LINE_CONTENT_LINES_PER_FILE
        ));
    }
}

fn cap_line_content_preview(file_entry: &mut Value) -> bool {
    let omitted_source_lines = {
        let Some(groups) = file_entry.get_mut("lineContent").and_then(|v| v.as_array_mut()) else {
            return false;
        };

        let mut remaining_source_lines = MAX_LINE_CONTENT_LINES_PER_FILE;
        let mut remaining_match_indices = MAX_LINES_PER_FILE;
        let mut groups_to_keep = groups.len();
        let mut omitted_source_lines = 0usize;

        for (group_index, group) in groups.iter_mut().enumerate() {
            let group_line_count = group
                .get("lines")
                .and_then(|v| v.as_array())
                .map(|lines| lines.len())
                .unwrap_or(0);

            if remaining_source_lines == 0 {
                omitted_source_lines += group_line_count;
                groups_to_keep = groups_to_keep.min(group_index);
                continue;
            }

            let keep_line_count = group_line_count.min(remaining_source_lines);
            if keep_line_count < group_line_count {
                omitted_source_lines += group_line_count - keep_line_count;
                groups_to_keep = groups_to_keep.min(group_index + 1);
            }

            if let Some(lines) = group.get_mut("lines").and_then(|v| v.as_array_mut()) {
                lines.truncate(keep_line_count);
            }
            remaining_source_lines = remaining_source_lines.saturating_sub(keep_line_count);

            let remove_match_indices = if let Some(group_obj) = group.as_object_mut() {
                if let Some(match_indices) = group_obj.get_mut("matchIndices").and_then(|v| v.as_array_mut()) {
                    match_indices.retain(|index| {
                        let Some(index) = index.as_u64().map(|idx| idx as usize) else {
                            return false;
                        };
                        if index >= keep_line_count || remaining_match_indices == 0 {
                            return false;
                        }
                        remaining_match_indices -= 1;
                        true
                    });
                    match_indices.is_empty()
                } else {
                    false
                }
            } else {
                false
            };

            if remove_match_indices
                && let Some(group_obj) = group.as_object_mut() {
                    group_obj.remove("matchIndices");
                }
        }

        if groups_to_keep < groups.len() {
            groups.truncate(groups_to_keep);
        }

        omitted_source_lines
    };

    if omitted_source_lines > 0 {
        file_entry["lineContentLinesOmitted"] = json!(omitted_source_lines);
        true
    } else {
        false
    }
}

/// Phase 2: Cap `matchedTokens` array in summary to MAX_MATCHED_TOKENS.
fn phase_cap_matched_tokens(output: &mut Value, reasons: &mut Vec<String>) {
    if let Some(summary) = output.get_mut("summary")
        && let Some(tokens) = summary.get_mut("matchedTokens").and_then(|t| t.as_array_mut())
            && tokens.len() > MAX_MATCHED_TOKENS {
                let omitted = tokens.len() - MAX_MATCHED_TOKENS;
                tokens.truncate(MAX_MATCHED_TOKENS);
                summary["matchedTokensOmitted"] = json!(omitted);
                reasons.push(format!("capped matchedTokens to {}", MAX_MATCHED_TOKENS));
            }
}

/// Phase 3: Remove `lines` arrays entirely from file entries.
fn phase_remove_lines_arrays(output: &mut Value, reasons: &mut Vec<String>) {
    if let Some(files) = output.get_mut("files").and_then(|f| f.as_array_mut()) {
        for file_entry in files.iter_mut() {
            if file_entry.get("lines").is_some() {
                file_entry.as_object_mut().map(|o| o.remove("lines"));
            }
        }
        reasons.push("removed all lines arrays".to_string());
    }
}

/// Phase 4: Progressively remove file entries from the tail based on average file entry size.
fn phase_reduce_file_count(output: &mut Value, max_bytes: usize, reasons: &mut Vec<String>) {
    let current_size = measure_json_size(output);
    if let Some(files) = output.get_mut("files").and_then(|f| f.as_array_mut()) {
        let original_count = files.len();
        if let Some(avg_file_size) = current_size.checked_div(original_count) {
            let excess = current_size.saturating_sub(max_bytes);
            let files_to_remove = if let Some(div) = excess.checked_div(avg_file_size) {
                div + 1 // +1 to be safe
            } else {
                original_count / 2
            };
            let keep = original_count.saturating_sub(files_to_remove).max(1);
            files.truncate(keep);
            let removed = original_count - files.len();
            if removed > 0 {
                reasons.push(format!("reduced files from {} to {}", original_count, files.len()));
            }
        }
    }
}

/// Phase 5a: Strip body fields from array entries to preserve signatures/metadata.
fn phase_strip_body_fields(output: &mut Value, reasons: &mut Vec<String>) {
    let body_fields = &["body", "bodyStartLine", "bodyTruncated", "totalBodyLines", "docCommentLines"];
    if let Some(obj) = output.as_object_mut() {
        let mut stripped = false;
        // Strip bodies from top-level arrays
        for key in &["definitions", "callTree", "containingDefinitions"] {
            if let Some(arr) = obj.get_mut(*key).and_then(|v| v.as_array_mut()) {
                for entry in arr.iter_mut() {
                    if let Some(entry_obj) = entry.as_object_mut() {
                        for field in body_fields {
                            if entry_obj.remove(*field).is_some() {
                                stripped = true;
                            }
                        }
                        // Also strip body from nested callers/callees arrays
                        strip_bodies_recursive(entry_obj, body_fields);
                    }
                }
            }
        }
        if let Some(root_method) = obj.get_mut("rootMethod")
            && let Some(root_object) = root_method.as_object_mut()
        {
            for field in body_fields {
                if root_object.remove(*field).is_some() {
                    stripped = true;
                }
            }
        }
        // Strip bodies from multi-method batch results: results[].callTree[]
        if let Some(results_arr) = obj.get_mut("results").and_then(|v| v.as_array_mut()) {
            for result_entry in results_arr.iter_mut() {
                if let Some(result_obj) = result_entry.as_object_mut() {
                    // Strip rootMethod body
                    if let Some(root_method) = result_obj.get_mut("rootMethod")
                        && let Some(rm_obj) = root_method.as_object_mut() {
                            for field in body_fields {
                                if rm_obj.remove(*field).is_some() {
                                    stripped = true;
                                }
                            }
                        }
                    // Strip bodies from callTree entries
                    if let Some(call_tree) = result_obj.get_mut("callTree").and_then(|v| v.as_array_mut()) {
                        for entry in call_tree.iter_mut() {
                            if let Some(entry_obj) = entry.as_object_mut() {
                                for field in body_fields {
                                    if entry_obj.remove(*field).is_some() {
                                        stripped = true;
                                    }
                                }
                                strip_bodies_recursive(entry_obj, body_fields);
                            }
                        }
                    }
                }
            }
        }
        if stripped {
            reasons.push("stripped body fields to preserve signatures".to_string());
            if let Some(summary) = obj.get_mut("summary") {
                summary["bodiesStrippedForSize"] = json!(true);
                if summary.get("totalBodyLinesReturned").is_some() {
                    summary["totalBodyLinesReturned"] = json!(0);
                }
            }
            let evidence_level = if obj.contains_key("definitions") {
                "index_map"
            } else {
                "ast_call_graph"
            };
            if let Some(result_status) = obj.get_mut("resultStatus") {
                result_status["evidenceLevel"] = json!(evidence_level);
                result_status["safeForExactSemantics"] = json!(false);
                let total_body_lines = result_status["total"]["bodyLines"].as_u64();
                if result_status["shown"]["bodyLines"].is_number() {
                    result_status["shown"]["bodyLines"] = json!(0);
                }
                if let Some(total_body_lines) = total_body_lines {
                    result_status["omitted"]["bodyLines"] = json!(total_body_lines);
                }
            }
        }
    }
}

/// Phase 5b: Remove lineContent previews only after cheaper caps still exceed the budget.
fn phase_remove_line_content(output: &mut Value, reasons: &mut Vec<String>) {
    let mut removed = false;
    if let Some(files) = output.get_mut("files").and_then(|f| f.as_array_mut()) {
        for file_entry in files.iter_mut() {
            if let Some(file_obj) = file_entry.as_object_mut()
                && file_obj.remove("lineContent").is_some() {
                    file_obj.insert("lineContentOmitted".to_string(), json!(true));
                    removed = true;
                }
        }
    }
    if removed {
        reasons.push("removed lineContent previews".to_string());
    }
}

/// Child relations preserved by the reconstructable caller-tree page format.
const CALL_TREE_CHILD_KEYS: [&str; 4] = ["callers", "callees", "children", "parents"];

fn call_tree_node_continuation_token(
    index_epoch: u64,
    root_offset: usize,
    node_offset: usize,
    query_fingerprint: &str,
) -> String {
    format!(
        "xrt1.{index_epoch}.{root_offset}.{node_offset}.{query_fingerprint}"
    )
}

fn flatten_call_tree_root(root: &Value, root_offset: usize) -> Vec<Value> {
    fn visit(
        node: &Value,
        parent_index: Option<usize>,
        relation: Option<&str>,
        ordinal: usize,
        root_offset: usize,
        records: &mut Vec<Value>,
    ) {
        let node_index = records.len();
        let node_id = format!("r{root_offset}:n{node_index}");
        let parent_node_id = parent_index.map(|index| format!("r{root_offset}:n{index}"));
        let node_data = if let Some(object) = node.as_object() {
            Value::Object(
                object
                    .iter()
                    .filter(|(key, _)| !CALL_TREE_CHILD_KEYS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        } else {
            node.clone()
        };
        records.push(json!({
            "nodeId": node_id,
            "parentNodeId": parent_node_id,
            "relation": relation,
            "ordinal": ordinal,
            "node": node_data,
        }));
        for child_key in CALL_TREE_CHILD_KEYS {
            if let Some(children) = node.get(child_key).and_then(Value::as_array) {
                for (child_ordinal, child) in children.iter().enumerate() {
                    visit(
                        child,
                        Some(node_index),
                        Some(child_key),
                        child_ordinal,
                        root_offset,
                        records,
                    );
                }
            }
        }
    }

    let mut records = Vec::new();
    visit(root, None, None, 0, root_offset, &mut records);
    records
}

fn call_tree_node_page_skeleton(output: &Value) -> Value {
    let mut skeleton = serde_json::Map::new();
    for key in ["query", "summary", "resultStatus"] {
        if let Some(value) = output.get(key) {
            skeleton.insert(key.to_string(), value.clone());
        }
    }
    const RETAINED_FIELDS: &[&str] = &["query", "summary", "resultStatus", "callTree"];
    let mut omitted_fields: Vec<&str> = output
        .as_object()
        .map(|object| {
            object
                .keys()
                .map(String::as_str)
                .filter(|key| !RETAINED_FIELDS.contains(key))
                .collect()
        })
        .unwrap_or_default();
    omitted_fields.sort_unstable();
    if !omitted_fields.is_empty() {
        if let Some(summary) = skeleton.get_mut("summary") {
            summary["nodePageOmittedFields"] = json!(omitted_fields);
        }
        if let Some(status) = skeleton.get_mut("resultStatus") {
            status["safeForExhaustiveClaims"] = json!(false);
            if let Some(reasons) = status["reasons"].as_array_mut()
                && !reasons
                    .iter()
                    .any(|reason| reason == "node_page_omits_auxiliary_fields")
            {
                reasons.push(json!("node_page_omits_auxiliary_fields"));
            }
        }
    }
    Value::Object(skeleton)
}


#[allow(clippy::too_many_arguments)]
fn call_tree_node_page_error(
    code: &str,
    message: &str,
    max_bytes: usize,
    root_offset: usize,
    root_total: usize,
    node_offset: usize,
    node_total: usize,
    workspace_generation: u64,
    index_epoch: u64,
    query_fingerprint: &str,
    total_known: bool,
) -> Value {
    let mut result = json!({
        "error": {
            "code": code,
            "message": message,
            "rootOffset": root_offset,
            "nodeOffset": node_offset,
            "maxResponseBytes": max_bytes,
            "unit": "callTreeNodes",
        },
        "summary": {
            "responseTruncated": true,
            "hint": "Continue with the returned token unchanged, or reduce included auxiliary data if one node record itself exceeds the budget.",
        },
        "resultStatus": {
            "status": "partial",
            "complete": false,
            "safeForExhaustiveClaims": false,
            "safeForExactSemantics": false,
            "totalKnown": total_known,
            "evidenceLevel": "metadata",
            "reasons": [code],
            "page": {
                "unit": "callTreeNodes",
                "rootOffset": root_offset,
                "rootTotal": root_total,
                "offset": node_offset,
                "returned": 0,
                "total": node_total,
                "totalKnown": total_known,
                "workspaceGeneration": workspace_generation,
                "indexEpoch": index_epoch,
                "queryFingerprint": query_fingerprint,
            },
        },
    });
    if measure_json_size(&result) > max_bytes {
        result = json!({
            "error": {
                "code": code,
                "rootOffset": root_offset,
                "nodeOffset": node_offset,
            }
        });
    }
    if measure_json_size(&result) > max_bytes {
        result = json!({ "error": 1 });
    }
    if measure_json_size(&result) > max_bytes {
        result = if max_bytes >= 2 { json!({}) } else { json!(0) };
    }
    result
}


fn fit_call_tree_node_page(
    output: &Value,
    max_bytes: usize,
    reasons: &[String],
    original_bytes: usize,
) -> Option<Value> {
    let root = output.get("callTree")?.as_array()?.first()?;
    let page = output.get("resultStatus")?.get("page")?;
    let root_offset = page.get("offset")?.as_u64()? as usize;
    let root_total = page.get("total")?.as_u64()? as usize;
    let node_offset = page
        .get("nodeOffset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let workspace_generation = page.get("workspaceGeneration")?.as_u64()?;
    let index_epoch = page.get("indexEpoch")?.as_u64()?;
    let query_fingerprint = page.get("queryFingerprint")?.as_str()?;
    let total_known = output
        .get("resultStatus")?
        .get("totalKnown")?
        .as_bool()?;
    let records = flatten_call_tree_root(root, root_offset);
    if node_offset >= records.len() {
        return Some(call_tree_node_page_error(
            "continuation_offset_out_of_range",
            "The call-tree continuation node offset is outside the rebuilt root.",
            max_bytes,
            root_offset,
            root_total,
            node_offset,
            records.len(),
            workspace_generation,
            index_epoch,
            query_fingerprint,
            total_known,
        ));
    }
    let skeleton = call_tree_node_page_skeleton(output);

    let build_candidate = |keep: usize, measurement: bool| {
        let end = node_offset.saturating_add(keep).min(records.len());
        let mut candidate = skeleton.clone();
        if let Some(object) = candidate.as_object_mut() {
            object.insert(
                "callTreeNodes".to_string(),
                Value::Array(records[node_offset..end].to_vec()),
            );
        }
        if let Some(status) = candidate.get_mut("resultStatus") {
            status["status"] = json!("partial");
            status["complete"] = json!(false);
            status["safeForExhaustiveClaims"] = json!(false);
            status["shown"]["nodes"] = json!(keep);
            if let Some(total_nodes) = status["total"]["nodes"].as_u64() {
                status["omitted"]["nodes"] = json!(total_nodes.saturating_sub(keep as u64));
            }
            status["page"] = json!({
                "unit": "callTreeNodes",
                "rootOffset": root_offset,
                "rootTotal": root_total,
                "offset": node_offset,
                "returned": keep,
                "total": records.len(),
                "totalKnown": total_known,
                "workspaceGeneration": workspace_generation,
                "indexEpoch": index_epoch,
                "queryFingerprint": query_fingerprint,
            });
            let page = &mut status["page"];
            if let Some(query) = output.pointer("/resultStatus/page/queryArgs")
                .or_else(|| output.pointer("/resultStatus/page/nextArgs"))
            {
                page["queryArgs"] = query.clone();
            }
            if end < records.len() {
                page["nextOffset"] = json!(end);
                page["continuationToken"] = json!(call_tree_node_continuation_token(
                    index_epoch,
                    root_offset,
                    end,
                    query_fingerprint,
                ));
            } else if root_offset.saturating_add(1) < root_total {
                page["nextRootOffset"] = json!(root_offset + 1);
                page["continuationToken"] = json!(continuation_token(
                    index_epoch,
                    root_offset + 1,
                    query_fingerprint,
                ));
            }
            if measurement && page.get("continuationToken").is_none() {
                page["continuationToken"] = json!(call_tree_node_continuation_token(
                    index_epoch,
                    root_offset,
                    end,
                    query_fingerprint,
                ));
            }
        }
        let mut candidate_reasons = reasons.to_vec();
        candidate_reasons.push(format!(
            "byte-fitted call tree root {} into node records {}..{} of {}",
            root_offset,
            node_offset,
            end,
            records.len(),
        ));
        inject_truncation_metadata(&mut candidate, &candidate_reasons, original_bytes);
        candidate
    };

    let mut low = 1usize;
    let mut high = records.len() - node_offset;
    let mut best_keep = None;
    while low <= high {
        let midpoint = low + (high - low) / 2;
        let candidate = build_candidate(midpoint, true);
        if measure_json_size(&candidate) <= max_bytes {
            best_keep = Some(midpoint);
            low = midpoint.saturating_add(1);
        } else {
            high = midpoint.saturating_sub(1);
        }
    }

    Some(if let Some(keep) = best_keep {
        build_candidate(keep, false)
    } else {
        call_tree_node_page_error(
            "single_item_exceeds_response_budget",
            "The next complete call-tree node record exceeds the response byte budget.",
            max_bytes,
            root_offset,
            root_total,
            node_offset,
            records.len(),
            workspace_generation,
            index_epoch,
            query_fingerprint,
            total_known,
        )
    })
}


fn recoverable_page_array_key(output: &Value) -> Option<&'static str> {
    let unit = output
        .get("resultStatus")?
        .get("page")?
        .get("unit")?
        .as_str()?;
    let key = match unit {
        "definitions" => "definitions",
        "topLevelRoots" => "callTree",
        "methods" => "results",
        "files" => "files",
        _ => return None,
    };
    output.get(key).and_then(Value::as_array).map(|_| key)
}

fn update_recoverable_page(output: &mut Value, array_key: &str, keep: usize) {
    if let Some(array) = output.get_mut(array_key).and_then(Value::as_array_mut) {
        array.truncate(keep);
    }

    let (offset, total, generation, fingerprint) = {
        let page = &output["resultStatus"]["page"];
        (
            page.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
            page.get("total").and_then(Value::as_u64).unwrap_or(keep as u64) as usize,
            page.get("indexEpoch").and_then(Value::as_u64).unwrap_or(0),
            page.get("queryFingerprint").and_then(Value::as_str).unwrap_or("").to_string(),
        )
    };
    let next_offset = offset.saturating_add(keep);
    if let Some(page) = output
        .get_mut("resultStatus")
        .and_then(|status| status.get_mut("page"))
        .and_then(Value::as_object_mut)
    {
        page.insert("returned".to_string(), json!(keep));
        page.remove("nextOffset");
        page.remove("continuationToken");
        if keep > 0 && next_offset < total {
            page.insert("nextOffset".to_string(), json!(next_offset));
            page.insert(
                "continuationToken".to_string(),
                json!(continuation_token(generation, next_offset, &fingerprint)),
            );
        }
    }

    if let Some(summary) = output.get_mut("summary")
        && summary.get("returned").is_some()
    {
        summary["returned"] = json!(keep);
    }

    let shown_file_occurrences = (array_key == "files").then(|| {
        output
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|file| file.get("occurrences").and_then(Value::as_u64))
            .sum::<u64>()
    });
    let shown_nodes = match array_key {
        "callTree" => output
            .get("callTree")
            .and_then(Value::as_array)
            .map(|nodes| count_tree_nodes(nodes) as u64),
        "results" => output
            .get("results")
            .and_then(Value::as_array)
            .map(|results| {
                results
                    .iter()
                    .filter_map(|result| result.get("callTree").and_then(Value::as_array))
                    .map(|nodes| count_tree_nodes(nodes) as u64)
                    .sum()
            }),
        _ => None,
    };
    if let Some(status) = output.get_mut("resultStatus") {
        if array_key == "definitions" {
            status["shown"]["definitions"] = json!(keep);
            if let Some(total_definitions) = status["total"]["definitions"].as_u64() {
                status["omitted"]["definitions"] =
                    json!(total_definitions.saturating_sub(keep as u64));
            }
        } else if let Some(shown_occurrences) = shown_file_occurrences {
            let total = status.get("total").cloned().unwrap_or_else(|| {
                json!({ "files": total, "occurrences": shown_occurrences })
            });
            add_collection_accounting(
                status,
                json!({ "files": keep, "occurrences": shown_occurrences }),
                total,
            );
        } else if let Some(shown_nodes) = shown_nodes {
            status["shown"]["nodes"] = json!(shown_nodes);
            if let Some(total_nodes) = status["total"]["nodes"].as_u64() {
                status["omitted"]["nodes"] = json!(total_nodes.saturating_sub(shown_nodes));
            }
        }
    }
}

fn oversized_page_item_error(output: &Value, max_bytes: usize) -> Value {
    let page = &output["resultStatus"]["page"];
    let item_offset = page.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let unit = page.get("unit").and_then(Value::as_str).unwrap_or("items");
    let total = page.get("total").and_then(Value::as_u64).unwrap_or(0);
    let mut result = json!({
        "error": {
            "code": "single_item_exceeds_response_budget",
            "message": "The next complete result item exceeds the response byte budget.",
            "itemOffset": item_offset,
            "maxResponseBytes": max_bytes,
            "unit": unit,
        },
        "summary": {
            "responseTruncated": true,
            "hint": "Narrow the query, reduce depth/body limits, or disable body inclusion before retrying the same offset.",
        },
        "resultStatus": {
            "status": "partial",
            "complete": false,
            "safeForExhaustiveClaims": false,
            "safeForExactSemantics": false,
            "totalKnown": true,
            "evidenceLevel": "metadata",
            "reasons": ["single_item_exceeds_response_budget"],
            "page": {
                "unit": unit,
                "offset": item_offset,
                "returned": 0,
                "total": total,
            },
        },
    });
    if measure_json_size(&result) > max_bytes {
        result = json!({
            "error": {
                "code": "single_item_exceeds_response_budget",
                "itemOffset": item_offset,
            }
        });
    }
    if measure_json_size(&result) > max_bytes {
        result = json!({ "error": 1 });
    }
    if measure_json_size(&result) > max_bytes {
        result = if max_bytes >= 2 { json!({}) } else { json!(0) };
    }
    result
}

fn compact_response_text(text: &str, max_chars: usize) -> Option<String> {
    let mut chars = text.chars();
    let compact: String = chars.by_ref().take(max_chars).collect();
    chars.next().map(|_| format!("{compact}..."))
}

fn try_fit_recoverable_page(
    output: &Value,
    array_key: &str,
    max_bytes: usize,
    reasons: &[String],
    original_bytes: usize,
) -> Option<Value> {
    let original_count = output
        .get(array_key)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if original_count == 0 {
        return Some(output.clone());
    }

    let build_candidate = |keep: usize, compact_hint: bool| {
        let mut candidate = output.clone();
        update_recoverable_page(&mut candidate, array_key, keep);
        let mut candidate_reasons = reasons.to_vec();
        candidate_reasons.push(format!(
            "byte-fitted '{}' page from {} to {} complete entries",
            array_key, original_count, keep,
        ));
        let supports_continuation = candidate["resultStatus"]["page"]["unit"] == "files"
            && candidate["resultStatus"]["page"]["queryFingerprint"].is_string()
            && candidate["resultStatus"]["page"]["continuationToken"].is_string();
        if compact_hint {
            let removed_queries = candidate.as_object_mut()
                .and_then(|object| object.remove("recommendedNextQueries")).is_some();
            let removed_navigation = candidate.get_mut("summary").and_then(Value::as_object_mut)
                .is_some_and(|summary| {
                    let removed = summary.remove("nextStepQuery").is_some();
                    if removed { summary.remove("nextStepHint"); }
                    removed
                });
            if removed_queries || removed_navigation {
                candidate_reasons.push("omitted optional follow-up queries".to_string());
            }
        }
        if array_key == "files" && compact_hint {
            let removed_execution = candidate
                .as_object_mut()
                .and_then(|object| object.remove("execution"))
                .is_some();
            let removed_analysis_context = candidate
                .as_object_mut()
                .and_then(|object| object.remove("analysisContext"))
                .is_some();
            let removed_next_queries = candidate
                .as_object_mut()
                .and_then(|object| object.remove("recommendedNextQueries"))
                .is_some();
            let adjusted_summary_hints = candidate
                .get_mut("summary")
                .and_then(Value::as_object_mut)
                .is_some_and(|summary| {
                    let compacted_warning = summary
                        .get("unknownArgsWarning")
                        .and_then(Value::as_str)
                        .and_then(|warning| compact_response_text(warning, 64));
                    let warning_changed = if let Some(compacted_warning) = compacted_warning {
                        summary.insert("unknownArgsWarning".to_string(), json!(compacted_warning));
                        true
                    } else {
                        false
                    };
                    let unknown_args_changed = summary
                        .get_mut("unknownArgs")
                        .and_then(Value::as_array_mut)
                        .is_some_and(|unknown_args| {
                            let mut changed = false;
                            for unknown_arg in unknown_args {
                                let Some(object) = unknown_arg.as_object_mut() else {
                                    continue;
                                };
                                for field in ["key", "hint"] {
                                    if let Some(value) = object.get(field).and_then(Value::as_str)
                                        && let Some(compact) = compact_response_text(value, 64)
                                    {
                                        object.insert(field.to_string(), json!(compact));
                                        changed = true;
                                    }
                                }
                            }
                            changed
                        });
                    let guidance_changed = if guidance_prefix_enabled() {
                        let compacted_next_step = summary
                            .get("nextStepHint")
                            .and_then(Value::as_str)
                            .and_then(|hint| {
                                if supports_continuation {
                                    Some("Use continuationToken.".to_string())
                                } else {
                                    compact_response_text(hint, 96)
                                }
                            });
                        if let Some(compacted_next_step) = compacted_next_step {
                            summary.insert("nextStepHint".to_string(), json!(compacted_next_step));
                            true
                        } else {
                            false
                        }
                    } else {
                        let removed_policy = summary.remove("policyReminder").is_some();
                        let removed_next_step = summary.remove("nextStepHint").is_some();
                        removed_policy || removed_next_step
                    };
                    warning_changed || unknown_args_changed || guidance_changed
                });
            if removed_execution
                || removed_analysis_context
                || removed_next_queries
                || adjusted_summary_hints
            {
                candidate_reasons.push("compacted response metadata".to_string());
            }
            if supports_continuation && candidate_reasons.len() > 3 {
                let first_reason = candidate_reasons.first().cloned();
                let page_reason = candidate_reasons
                    .iter()
                    .rev()
                    .find(|reason| reason.starts_with("byte-fitted"))
                    .cloned();
                candidate_reasons.clear();
                candidate_reasons.extend(first_reason);
                candidate_reasons.extend(page_reason);
                candidate_reasons.push("compacted response metadata".to_string());
            }
        }
        inject_truncation_metadata(&mut candidate, &candidate_reasons, original_bytes);
        if array_key == "files" && compact_hint && supports_continuation
            && let Some(summary) = candidate
                .get_mut("summary")
                .and_then(Value::as_object_mut)
        {
            summary.remove("hint");
        }
        candidate
    };

    let find_best = |compact_hint: bool| {
        let mut low = 1usize;
        let mut high = original_count;
        let mut best_keep: Option<usize> = None;
        while low <= high {
            let midpoint = low + (high - low) / 2;
            let candidate = build_candidate(midpoint, compact_hint);
            if measure_json_size(&candidate) <= max_bytes {
                best_keep = Some(midpoint);
                low = midpoint.saturating_add(1);
            } else {
                high = midpoint.saturating_sub(1);
            }
        }
        best_keep
    };

    let hinted_keep = find_best(false);
    let compact_keep = find_best(true);
    let best_keep = match (hinted_keep, compact_keep) {
        (Some(hinted), Some(compact)) if compact > hinted => Some((compact, true)),
        (Some(hinted), _) => Some((hinted, false)),
        (None, Some(compact)) => Some((compact, true)),
        (None, None) => None,
    };
    best_keep.map(|(keep, compact_hint)| build_candidate(keep, compact_hint))
}


fn phase_truncate_largest_array(output: &mut Value, max_bytes: usize, reasons: &mut Vec<String>) {
    let current_size = measure_json_size(output);
    if current_size <= max_bytes {
        return;
    }
    if let Some(obj) = output.as_object_mut() {
        // Find the largest top-level array (skip "files" — already handled)
        let largest_array_key = obj.iter()
            .filter(|(k, v)| *k != "files" && *k != "summary" && v.is_array())
            .max_by_key(|(_, v)| v.as_array().map(|a| a.len()).unwrap_or(0))
            .map(|(k, _)| k.clone());

        if let Some(key) = largest_array_key
            && let Some(arr) = obj.get_mut(&key).and_then(|v| v.as_array_mut()) {
                let original_count = arr.len();
                if original_count > 0 {
                    // Estimate how many entries to keep
                    let avg_entry_size = current_size / original_count.max(1);
                    let target_entries = if let Some(div) = max_bytes.checked_div(avg_entry_size) {
                        div
                    } else {
                        original_count / 2
                    };
                    let keep = target_entries.max(1).min(original_count);
                    let actual_kept = keep; // capture before truncate
                    arr.truncate(keep);
                    let removed = original_count - arr.len();
                    if removed > 0 {
                        reasons.push(format!(
                            "truncated '{}' array from {} to {} entries",
                            key, original_count, arr.len()
                        ));
                        // Update 'returned' in summary to reflect actual array length
                        if let Some(summary) = obj.get_mut("summary") {
                            summary["returned"] = json!(actual_kept);
                        }
                    }
                }
            }
    }
}

fn fit_recoverable_page(
    output: Value,
    array_key: &str,
    max_bytes: usize,
    reasons: &[String],
    original_bytes: usize,
) -> Value {
    if let Some(fitted) = try_fit_recoverable_page(
        &output,
        array_key,
        max_bytes,
        reasons,
        original_bytes,
    ) {
        return fitted;
    }
    if array_key == "callTree" {
        fit_call_tree_node_page(&output, max_bytes, reasons, original_bytes)
            .unwrap_or_else(|| oversized_page_item_error(&output, max_bytes))
    } else {
        oversized_page_item_error(&output, max_bytes)
    }
}


pub(crate) fn truncate_large_response(mut output: Value, max_bytes: usize) -> Value {
    if max_bytes == 0 {
        return output;
    }
    let initial_size = measure_json_size(&output);
    let node_page_requested = output
        .get("resultStatus")
        .and_then(|status| status.get("page"))
        .and_then(|page| page.get("nodeOffset"))
        .is_some_and(Value::is_number);
    let recoverable_array_key = recoverable_page_array_key(&output);
    let recoverable_array_has_items = recoverable_array_key
        .and_then(|key| output.get(key))
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());
    if initial_size <= max_bytes && !node_page_requested {
        return output;
    }

    let mut reasons: Vec<String> = Vec::new();

    phase_cap_lines_per_file(&mut output, &mut reasons);
    if measure_json_size(&output) <= max_bytes && !node_page_requested {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    phase_cap_matched_tokens(&mut output, &mut reasons);
    if measure_json_size(&output) <= max_bytes && !node_page_requested {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    if recoverable_array_key == Some("files") {
        let compacted_policy_marker = guidance_prefix_enabled()
            && output
                .get_mut("summary")
                .and_then(Value::as_object_mut)
                .is_some_and(|summary| {
                    if summary.get("policyReminder").is_some() {
                        summary.insert("policyReminder".to_string(), json!("1"));
                        true
                    } else {
                        false
                    }
                });
        if compacted_policy_marker {
            reasons.push("compacted policy reminder marker".to_string());
        }
        if measure_json_size(&output) <= max_bytes && !node_page_requested {
            inject_truncation_metadata(&mut output, &reasons, initial_size);
            return output;
        }
        if recoverable_array_has_items
            && let Some(fitted) = try_fit_recoverable_page(
                &output,
                "files",
                max_bytes,
                &reasons,
                initial_size,
            )
        {
            return fitted;
        }
    } else {
        phase_reduce_file_count(&mut output, max_bytes, &mut reasons);
        if measure_json_size(&output) <= max_bytes && !node_page_requested {
            inject_truncation_metadata(&mut output, &reasons, initial_size);
            return output;
        }
    }

    phase_remove_lines_arrays(&mut output, &mut reasons);
    if measure_json_size(&output) <= max_bytes && !node_page_requested {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    if super::body_delivery::has_body_reads(&output) {
        super::body_delivery::fit_body_fragments(&mut output, max_bytes);
        reasons.push("byte-fitted body fragments".to_string());
    } else {
        phase_strip_body_fields(&mut output, &mut reasons);
    }
    if measure_json_size(&output) <= max_bytes && !node_page_requested {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    phase_remove_line_content(&mut output, &mut reasons);
    if measure_json_size(&output) <= max_bytes && !node_page_requested {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    if node_page_requested {
        return fit_call_tree_node_page(&output, max_bytes, &reasons, initial_size)
            .unwrap_or_else(|| oversized_page_item_error(&output, max_bytes));
    }

    if let Some(array_key) = recoverable_array_key
        && (array_key != "files" || recoverable_array_has_items)
    {
        return fit_recoverable_page(output, array_key, max_bytes, &reasons, initial_size);
    }

    phase_truncate_largest_array(&mut output, max_bytes, &mut reasons);
    inject_truncation_metadata(&mut output, &reasons, initial_size);
    output
}

/// Recursively strip body fields from nested `callers`/`callees`/`children` arrays.
fn strip_bodies_recursive(obj: &mut serde_json::Map<String, Value>, body_fields: &[&str]) {
    for nested_key in &["callers", "callees", "children"] {
        if let Some(arr) = obj.get_mut(*nested_key).and_then(|v| v.as_array_mut()) {
            for entry in arr.iter_mut() {
                if let Some(entry_obj) = entry.as_object_mut() {
                    for field in body_fields {
                        entry_obj.remove(*field);
                    }
                    strip_bodies_recursive(entry_obj, body_fields);
                }
            }
        }
    }
}

/// Add truncation metadata to the summary object.
/// Adapts the hint message based on the response format (grep vs definitions).
fn inject_truncation_metadata(output: &mut Value, reasons: &[String], original_bytes: usize) {
    if reasons.is_empty() {
        return;
    }

    // Detect response type to provide a relevant hint
    let has_files = output.get("files").is_some();
    let has_definitions = output.get("definitions").is_some();
    let hint = if has_definitions {
        "Response truncated. Narrow your search with more specific name, kind, file, or parent filters, or reduce maxResults."
    } else if has_files {
        "Use countOnly=true for aggregate totals, filesOnly=true to keep the file list without line bodies, or narrow with dir/ext/exclude filters"
    } else {
        "Response truncated. Use more specific filters to reduce result size."
    };

    if let Some(summary) = output.get_mut("summary") {
        summary["responseTruncated"] = json!(true);
        summary["truncationReason"] = json!(reasons.join("; "));
        summary["originalResponseBytes"] = json!(original_bytes);
        summary["hint"] = json!(hint);
    }
    mark_result_status_response_truncated(output, reasons);
    super::body_delivery::refresh_page_args(output);
    super::body_delivery::refresh_accounting(output);
}

/// Apply response size truncation to a ToolCallResult (no metrics injection).
/// Used when metrics are disabled but we still need to cap response size.
fn is_response_truncation_error(output: &Value, truncation_applied: bool) -> bool {
    let structured_error = output
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .is_some_and(|code| {
            matches!(
                code,
                "single_item_exceeds_response_budget" | "continuation_offset_out_of_range"
            )
        });
    structured_error
        || (truncation_applied
            && match output {
                Value::Object(object) => {
                    object.is_empty() || object.get("error").and_then(Value::as_u64) == Some(1)
                }
                Value::Number(number) => number.as_u64() == Some(0),
                _ => false,
            })
}


pub(crate) fn truncate_response_if_needed(result: ToolCallResult, max_bytes: usize) -> ToolCallResult {
    if max_bytes == 0 {
        return result;
    }

    let text = match result.content.first() {
        Some(c) => &c.text,
        None => return result,
    };

    if text.len() <= max_bytes {
        return result;
    }

    if let Ok(output) = serde_json::from_str::<Value>(text) {
        let truncated = truncate_large_response(output, max_bytes);
        let is_error = result.is_error || is_response_truncation_error(&truncated, true);
        ToolCallResult {
            is_error,
            ..ToolCallResult::success(json_to_string(&truncated))
        }
    } else {
        result
    }
}

// ─── Metrics injection ──────────────────────────────────────────────

/// Inject performance metrics into a successful tool response.
/// Parses the JSON text, adds searchTimeMs/responseBytes/estimatedTokens/indexFiles/indexTokens
/// to the "summary" object (if present), then re-serializes. The final response
/// renderer applies the exact wire budget after moving guidance fields.
pub(crate) fn inject_metrics(result: ToolCallResult, ctx: &HandlerContext, start: Instant) -> ToolCallResult {
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    let was_error = result.is_error;

    // Get the text from the first content item
    let text = match result.content.first() {
        Some(c) => &c.text,
        None => return result,
    };

    // Try to parse as JSON and inject metrics into "summary"
    if let Ok(mut output) = serde_json::from_str::<Value>(text) {
        if let Some(summary) = output.get_mut("summary") {
            // B4 fix: Don't overwrite handler-specific searchTimeMs.
            // Many handlers (grep, definitions, callers) set precise search time
            // without serialization/truncation overhead. Preserve it and add
            // totalTimeMs for the full dispatch-to-response time.
            let total_time = (elapsed_ms * 100.0).round() / 100.0;
            if summary.get("searchTimeMs").is_none() {
                summary["searchTimeMs"] = json!(total_time);
            }
            summary["totalTimeMs"] = json!(total_time);

            if let Ok(idx) = ctx.index.try_read() {
                summary["indexFiles"] = json!(idx.live_file_count());
                summary["indexTokens"] = json!(idx.index.len());
            }
        }

        // Measure response size before the final guidance-aware fit.
        // The renderer applies the exact wire budget after policy fields move.
        let bytes = json_to_string(&output).len();
        if let Some(summary) = output.get_mut("summary") {
            summary["responseBytes"] = json!(bytes);
            summary["estimatedTokens"] = json!(bytes / 4);
        }

        ToolCallResult {
            is_error: was_error,
            ..ToolCallResult::success(json_to_string(&output))
        }
    } else {
        // Not valid JSON or no summary -- return as-is
        result
    }
}

// ─── Body injection helper ──────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct BodySource {
    content: String,
    source_hash: String,
}

impl BodySource {
    pub(crate) fn hash_for_range(&self, start: u32, end: u32) -> Option<String> {
        (start > 0 && end >= start && self.content.lines().nth(end as usize - 1).is_some())
            .then(|| self.source_hash.clone())
    }

    pub(crate) fn new(content: String) -> Self {
        let source_hash = format!("sha256:{:x}", Sha256::digest(content.as_bytes()));
        Self { content, source_hash }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn inject_body_into_obj(
    obj: &mut Value,
    file_path: &str,
    line_start: u32,
    line_end: u32,
    file_cache: &mut HashMap<String, Option<BodySource>>,
    total_body_lines_emitted: &mut usize,
    max_body_lines: usize,
    max_total_body_lines: usize,
    include_doc_comments: bool,
    body_line_start: Option<u32>,
    body_line_end: Option<u32>,
) {
    obj["bodyComplete"] = json!(false);
    obj["bodyRangeComplete"] = json!(false);
    obj["bodyFile"] = json!(file_path);
    if obj.get("bodyAnchorLine").is_some() {
        obj["bodyAnchorVisible"] = json!(false);
        obj["bodyAnchorReason"] = json!("body_unavailable");
    }
    let content_opt = file_cache.entry(file_path.to_string()).or_insert_with(|| {
        crate::read_file_lossy(std::path::Path::new(file_path)).ok().map(|(content, _)| BodySource::new(content))
    });
    let Some(source) = content_opt.as_ref() else {
        obj["bodyError"] = json!("failed to read file");
        return;
    };
    let lines: Vec<&str> = source.content.lines().collect();
    let definition_start = (line_start as usize).saturating_sub(1);
    let definition_end = line_end as usize;
    let available_start = if include_doc_comments && definition_start <= lines.len() {
        find_doc_comment_start(&lines, definition_start)
    } else {
        definition_start
    };
    let requested_start = body_line_start.map(|line| (line as usize).saturating_sub(1))
        .unwrap_or(available_start).max(available_start);
    let requested_end = body_line_end.map(|line| line as usize).unwrap_or(definition_end).min(definition_end);
    obj["bodyDefinitionStartLine"] = json!(line_start);
    obj["bodyDefinitionEndLine"] = json!(line_end);
    obj["bodyAvailableStartLine"] = json!(available_start + 1);
    obj["bodyRequestedStartLine"] = json!(requested_start + 1);
    obj["bodyRequestedEndLine"] = json!(requested_end);
    obj["bodySourceHash"] = json!(source.source_hash);
    let explicit_range = body_line_start.is_some() || body_line_end.is_some();
    obj["bodyExplicitRange"] = json!(explicit_range);
    if definition_end > lines.len() {
        obj["bodyWarning"] = json!(format!(
            "definition claims line_end={} but file has only {} lines (stale index?)", line_end, lines.len(),
        ));
    }
    let range_end = requested_end.min(lines.len());
    let available = range_end.saturating_sub(requested_start);
    let remaining = if max_total_body_lines == 0 { usize::MAX } else {
        max_total_body_lines.saturating_sub(*total_body_lines_emitted)
    };
    let limit = if max_body_lines == 0 { remaining } else { max_body_lines.min(remaining) };
    let count = available.min(limit);
    let anchor = obj.get("bodyAnchorLine").and_then(Value::as_u64).map(|line| line as usize);
    let offset = if !explicit_range && count > 0 {
        anchor.filter(|line| *line > requested_start && *line <= range_end)
            .map(|line| line.saturating_sub(requested_start + 1)
                .saturating_sub((count - 1) / 2).min(available - count))
            .unwrap_or(0)
    } else { 0 };
    let start = requested_start + offset;
    obj["bodyStartLine"] = json!(start + 1);
    obj["body"] = if count == 0 { json!([]) } else { json!(lines[start..start + count]) };
    if available == 0 {
        obj["bodyOmitted"] = json!("requested range has no available body lines");
    } else if count == 0 {
        obj["bodyOmitted"] = json!("total body lines budget exceeded");
    }
    refresh_body_metadata(obj);
    *total_body_lines_emitted += count;
}

pub(crate) fn refresh_body_metadata(obj: &mut Value) {
    let Some(body) = obj.get("body").and_then(Value::as_array) else { return; };
    let count = body.len() as u64;
    let start = obj["bodyStartLine"].as_u64().unwrap_or(1);
    let end = start.saturating_add(count).saturating_sub(1);
    let definition_start = obj["bodyDefinitionStartLine"].as_u64().unwrap_or(start);
    let definition_end = obj["bodyDefinitionEndLine"].as_u64().unwrap_or(end);
    let available_start = obj["bodyAvailableStartLine"].as_u64().unwrap_or(definition_start);
    let requested_start = obj["bodyRequestedStartLine"].as_u64().unwrap_or(start);
    let requested_end = obj["bodyRequestedEndLine"].as_u64().unwrap_or(end);
    let complete = count > 0 && start <= available_start && end >= definition_end;
    let range_complete = count > 0 && start <= requested_start && end >= requested_end;
    obj["bodyEndLine"] = if count > 0 { json!(end) } else { Value::Null };
    obj["bodyComplete"] = json!(complete);
    obj["bodyRangeComplete"] = json!(range_complete);
    if range_complete {
        obj.as_object_mut().unwrap().remove("bodyTruncated");
        obj.as_object_mut().unwrap().remove("totalBodyLines");
    } else {
        obj["bodyTruncated"] = json!(true);
        obj["totalBodyLines"] = json!(requested_end.saturating_add(1).saturating_sub(requested_start));
    }
    let doc_lines = definition_start.saturating_sub(start).min(count);
    if doc_lines > 0 {
        obj["docCommentLines"] = json!(doc_lines);
    } else {
        obj.as_object_mut().unwrap().remove("docCommentLines");
    }
    if let Some(anchor) = obj.get("bodyAnchorLine").and_then(Value::as_u64) {
        let visible = count > 0 && anchor >= start && anchor <= end;
        obj["bodyAnchorVisible"] = json!(visible);
        if visible {
            obj.as_object_mut().unwrap().remove("bodyAnchorReason");
        } else {
            obj["bodyAnchorReason"] = json!(if obj["bodyExplicitRange"] == true && (anchor < requested_start || anchor > requested_end) {
                "outside_requested_range"
            } else if anchor < available_start || anchor > definition_end {
                "outside_definition"
            } else {
                "body_line_budget"
            });
        }
    }
}

/// Scan upward from `decl_start_idx` (0-based) to find the first line of a
/// contiguous doc-comment block. Returns the 0-based index of the first
/// doc-comment line, or `decl_start_idx` if no doc-comment is found.
///
/// Supports:
/// - C#/Rust: `///` XML doc comments
/// - TypeScript/JavaScript: `/** ... */` JSDoc blocks
///
/// Skips blank lines between the declaration and the comment block.
/// Stops at the first non-comment, non-blank line.
pub(crate) fn find_doc_comment_start(lines: &[&str], decl_start_idx: usize) -> usize {
    if decl_start_idx == 0 {
        return decl_start_idx;
    }

    let mut scan = decl_start_idx - 1;

    // Phase 1: skip blank lines between declaration and potential comment
    while scan > 0 && lines[scan].trim().is_empty() {
        scan -= 1;
    }
    // If the line after skipping blanks is blank too (scan == 0 and blank), no comment
    if lines[scan].trim().is_empty() {
        return decl_start_idx;
    }

    // Phase 2: check if we're at a doc-comment line
    if !is_doc_comment_line(lines[scan]) {
        return decl_start_idx; // no doc-comment above
    }

    // Phase 3: scan upward through contiguous doc-comment lines
    let comment_end = scan;
    while scan > 0 {
        let above = lines[scan - 1].trim();
        if above.is_empty() {
            // Allow at most one blank line within a JSDoc block (between /** and */)
            // but not for /// style comments
            break;
        }
        if is_doc_comment_line(lines[scan - 1]) {
            scan -= 1;
        } else {
            break;
        }
    }

    // Verify we actually found comment lines
    if scan <= comment_end {
        scan
    } else {
        decl_start_idx
    }
}

/// Check if a line is a doc-comment line (trimmed).
/// Matches: `///`, `/**`, ` * `, ` */`, `*` (JSDoc continuation)
fn is_doc_comment_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("///")          // C#, Rust doc comments
        || trimmed.starts_with("/**")   // JSDoc block start
        || trimmed.starts_with("*/")    // JSDoc block end
        || (trimmed.starts_with('*') && !trimmed.starts_with("**") || trimmed == "*") // JSDoc continuation: `* text` or bare `*`
}

// ─── Name similarity helper ─────────────────────────────────────────

/// Compute similarity ratio between two strings (0.0 – 1.0).
/// Uses Jaro-Winkler distance — optimized for short identifiers and typo detection.
/// Gives bonus for matching prefixes, which aligns with typical LLM errors
/// (e.g., `GetUsr` → `GetUser`, `hndl_search` → `handle_search`).
/// Useful for fuzzy name matching when xray_definitions returns 0 results.
pub(crate) fn name_similarity(a: &str, b: &str) -> f64 {
    strsim::jaro_winkler(a, b)
}

// ─── Relevance ranking helpers ──────────────────────────────────────

/// Returns the best (lowest) match tier for a name against a list of search terms.
/// - 0 = exact match (name equals one of the terms, case-insensitive)
/// - 1 = prefix match (name starts with one of the terms)
/// - 2 = contains match (name contains one of the terms — already filtered)
pub(crate) fn best_match_tier(name: &str, terms: &[String]) -> u8 {
    let name_lower = name.to_lowercase();
    let mut best = 2u8;
    for term in terms {
        let term_lower = term.to_lowercase();
        if name_lower == term_lower {
            return 0; // exact — can't do better
        }
        if best > 1 && name_lower.starts_with(&term_lower) {
            best = 1;
        }
    }
    best
}

/// Resolve the closed enum of valid `kind` values for `xray_definitions`.
/// Derived at runtime from
/// [`crate::definitions::types::DefinitionKind::ALL_KINDS`] so the schema
/// validator and the parser-side enum cannot drift. Computed once per
/// process via [`std::sync::OnceLock`].
///
/// Drift guards live next to the enum itself (see
/// `definitions::types::kinds_drift_tests::all_kinds_lists_every_variant`).
fn known_kinds() -> &'static [&'static str] {
    use crate::definitions::DefinitionKind;
    use std::sync::OnceLock;
    static CACHE: OnceLock<Vec<&'static str>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            DefinitionKind::ALL_KINDS
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<&'static str>>()
        })
        .as_slice()
}

/// Read a JSON array-of-strings argument from MCP tool call args.
///
/// Returns an empty `Vec` when the param is absent or `null`. Trims each entry
/// and skips empty ones (matches the prior comma-split behaviour for empty
/// segments). Returns an `Err` with an actionable message when the param is
/// present but is not an array of strings — the canonical schema-mismatch path
/// after the 2026-04-25 list-params-to-arrays migration. The error message
/// always includes a concrete `["a","b"]` example so LLM callers can self-correct.
pub(crate) fn read_string_array(
    args: &Value,
    key: &str,
) -> Result<Vec<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for (i, v) in arr.iter().enumerate() {
                match v.as_str() {
                    Some(s) => {
                        // Skip entirely-whitespace entries, but PRESERVE leading/
                        // trailing whitespace inside non-empty entries — it is
                        // significant for regex patterns (e.g. `"^## "` differs
                        // semantically from `"^##"`).
                        if !s.trim().is_empty() {
                            out.push(s.to_string());
                        }
                    }
                    None => {
                        return Err(format!(
                            "'{key}' must be an array of strings; element [{i}] is {} (got {}). Example: '{key}': [\"a\",\"b\"].",
                            type_label(v),
                            v
                        ));
                    }
                }
            }
            Ok(out)
        }
        Some(other) => Err(format!(
            "'{key}' must be an array of strings (got {}). The string-with-comma form was removed in the 2026-04-25 migration. Example: '{key}': [\"a\",\"b\"].",
            type_label(other)
        )),
    }
}

/// Strict reader for an OPTIONAL single-string MCP parameter.
///
/// Symmetric to [`read_string_array`] for the single-value case. Returns:
/// - `Ok(None)` when the key is missing or `Value::Null`.
/// - `Ok(Some(s))` when the value is a `Value::String` with non-whitespace content.
/// - `Err(msg)` when the value is the wrong type (`Array`, `Number`, `Bool`,
///   `Object`) OR an empty/whitespace-only string. Both failure modes
///   are explicit so wrong-type input never silently degrades into
///   "filter dropped, query proceeded against unfiltered superset".
///
/// The empty-string-is-Err policy is intentional and DIFFERS from the per-element
/// whitespace-skip behaviour of [`read_string_array`] (which preserves whitespace
/// inside non-empty array entries because it can be regex-significant). For a
/// single-value param, an empty string is unambiguous user error: the caller
/// either meant to pass a value or meant to omit the key. Silently treating it
/// as absent would re-introduce the same "filter silently skipped" failure mode
/// that the 2026-04-25 strict-typing migration is closing.
pub(crate) fn read_string(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                Err(format!(
                    "'{key}' must be a non-empty string (got empty / whitespace-only). \
                     Pass a non-empty string or omit the key. \
                     Single-value params reject empty strings since the 2026-04-25 strict-typing migration. \
                     Example: '{key}': \"value\"."
                ))
            } else {
                Ok(Some(s.clone()))
            }
        }
        Some(other) => Err(format!(
            "'{key}' must be a string (got {}). \
             Single-value params reject arrays/numbers/bools/objects since the 2026-04-25 strict-typing migration. \
             Example: '{key}': \"value\".",
            type_label(other)
        )),
    }
}

/// Strict reader for a REQUIRED single-string MCP parameter.
///
/// Distinguishes "missing" from "wrong type" in the error message — fixes the
/// misleading `Missing required parameter: <key>` error that the legacy
/// `args.get(K).and_then(|v| v.as_str())` pattern produced when the caller
/// passed an array/number/bool/object instead of a string.
pub(crate) fn read_required_string(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key) {
        None | Some(Value::Null) => Err(format!("Missing required parameter: '{key}'.")),
        _ => match read_string(args, key)? {
            Some(s) => Ok(s),
            // `read_string` returned `Ok(None)` only on missing/null, both already
            // matched above; an empty/whitespace string surfaced as `Err` upstream.
            // This arm is unreachable but kept exhaustive for future maintenance.
            None => Err(format!("Missing required parameter: '{key}'.")),
        },
    }
}

/// Strict reader for an OPTIONAL enum-typed string MCP parameter (closed set,
/// with a default). Returns the default when the key is missing/null; errors on
/// wrong type, empty string, or value outside `allowed`.
///
/// Used for `direction` (default `"up"`), `mode` (default `"or"`), etc. Pair
/// each call site with a `pub const ALL_X: &[&str] = &[...]` next to the enum
/// definition + an exhaustive-match drift-guard test (mirror of `ALL_KINDS`).
pub(crate) fn read_enum_string_with_default(
    args: &Value,
    key: &str,
    allowed: &[&str],
    default: &str,
) -> Result<String, String> {
    match read_string(args, key)? {
        None => Ok(default.to_string()),
        Some(s) => {
            if allowed.iter().any(|a| a.eq_ignore_ascii_case(&s)) {
                Ok(s)
            } else {
                Err(format!(
                    "'{key}' must be one of {:?} (got {:?}).",
                    allowed, s
                ))
            }
        }
    }
}

/// Strict reader for an OPTIONAL enum-typed string MCP parameter (closed set,
/// no default — absence yields `None`). Used for `sortBy` where unset means
/// "do not sort".
pub(crate) fn read_enum_string_opt(
    args: &Value,
    key: &str,
    allowed: &[&str],
) -> Result<Option<String>, String> {
    match read_string(args, key)? {
        None => Ok(None),
        Some(s) => {
            if allowed.iter().any(|a| a.eq_ignore_ascii_case(&s)) {
                Ok(Some(s))
            } else {
                Err(format!(
                    "'{key}' must be one of {:?} (got {:?}).",
                    allowed, s
                ))
            }
        }
    }
}


/// Like [`read_string_array`] but additionally validates each element against
/// the closed [`known_kinds`] enum. Used by `xray_definitions` for the `kind`
/// parameter.
pub(crate) fn read_kind_array(args: &Value) -> Result<Vec<String>, String> {
    let raw = read_string_array(args, "kind")?;
    let valid = known_kinds();
    for k in &raw {
        if !valid.contains(&k.as_str()) {
            return Err(format!(
                "'kind' contains unknown value '{k}'. Valid values: {}.",
                valid.join(", ")
            ));
        }
    }
    Ok(raw)
}

/// Human-readable JSON value type label for diagnostic messages.
fn type_label(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
#[path = "utils_tests.rs"]
mod tests;
