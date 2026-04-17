//! Shared utility functions for MCP tool handlers.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;

use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use crate::clean_path;

use super::HandlerContext;

/// Serialize a JSON Value to a string, returning a fallback error JSON
/// if serialization fails (e.g., due to NaN/Infinity float values).
/// This prevents panics in MCP handlers from crashing the long-lived server process.
pub(crate) fn json_to_string(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| {
        format!(r#"{{"error":"serialization failed: {}"}}"#, e)
    })
}

// ─── Branch warning ─────────────────────────────────────────────────

/// Returns a warning message if the current branch is not `main` or `master`.
/// Used by index-based tools (xray_grep, xray_definitions, xray_callers,
/// xray_fast) to alert the user that results may differ from production.
pub(crate) fn branch_warning(ctx: &HandlerContext) -> Option<String> {
    ctx.current_branch.as_ref().and_then(|b| {
        if b == "main" || b == "master" {
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

// ─── Dir validation ─────────────────────────────────────────────────

/// Normalize path separators to forward slashes for cross-platform comparison.
pub(crate) fn normalize_path_sep(p: &str) -> String {
    p.replace('\\', "/")
}

/// Validate that `requested_dir` is the server dir or a subdirectory of it.
/// Returns `Ok(None)` if exact match (no filtering needed),
/// `Ok(Some(canonical_subdir))` if it's a proper subdirectory (use as filter),
/// or `Err(message)` if outside the server dir.
/// Resolve a potentially relative directory path to absolute, using server_dir as base.
/// Absolute paths pass through unchanged. Relative paths are joined with server_dir.
/// Uses canonicalize when possible to resolve symlinks and normalize the path.
pub(crate) fn resolve_dir_to_absolute(dir: &str, server_dir: &str) -> String {
    let normalized = dir.replace('\\', "/");
    if std::path::Path::new(dir).is_absolute() {
        // Already absolute — just canonicalize for normalization
        std::fs::canonicalize(dir)
            .map(|p| clean_path(&p.to_string_lossy()))
            .unwrap_or_else(|_| normalized)
    } else if dir == "." {
        // Dot path = server_dir itself
        server_dir.to_string()
    } else {
        // Relative path — resolve against server_dir
        let full = format!(
            "{}/{}",
            server_dir.replace('\\', "/").trim_end_matches('/'),
            normalized.trim_matches('/')
        );
        std::fs::canonicalize(&full)
            .map(|p| clean_path(&p.to_string_lossy()))
            .unwrap_or(full)
    }
}

pub(crate) fn validate_search_dir(requested_dir: &str, server_dir: &str) -> Result<Option<String>, String> {
    // Pre-resolve relative paths against server_dir before canonicalize
    let requested_dir = resolve_dir_to_absolute(requested_dir, server_dir);
    let requested = std::fs::canonicalize(&requested_dir)
        .map(|p| clean_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| requested_dir.to_string());
    // Note: server_dir is already canonical when coming from HandlerContext::server_dir().
    // The canonicalize here is a safety fallback for direct callers.
    let server = std::fs::canonicalize(server_dir)
        .map(|p| clean_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| server_dir.to_string());

    let req_norm = normalize_path_sep(&requested).to_lowercase();
    let srv_norm = normalize_path_sep(&server).to_lowercase();

    if req_norm == srv_norm {
        Ok(None)
    } else if req_norm.starts_with(&srv_norm) {
        // Verify it's a true subdirectory (next char must be '/')
        let next_char = req_norm.as_bytes().get(srv_norm.len());
        if next_char == Some(&b'/') {
            Ok(Some(requested))
        } else {
            Err(format!(
                "Server started with --dir {}. For other directories, start another server instance or use CLI.",
                server_dir
            ))
        }
    } else {
        Err(format!(
            "Server started with --dir {}. For other directories, start another server instance or use CLI.",
            server_dir
        ))
    }
}

/// Check if a file path is under the given directory prefix (case-insensitive, separator-normalized).
/// Ensures proper boundary check: `C:\Repos\Shared` won't match `C:\Repos\SharedExtra\file.cs`.
pub(crate) fn is_under_dir(file_path: &str, dir_prefix: &str) -> bool {
    let file_norm = normalize_path_sep(file_path).to_lowercase();
    let mut dir_norm = normalize_path_sep(dir_prefix).to_lowercase();
    // Ensure dir prefix ends with '/' for proper boundary matching
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
}

impl ExcludePatterns {
    /// Build from raw exclude_dir strings (e.g., ["test", "Mock"]).
    /// Lowercases and formats patterns once.
    pub fn from_dirs(exclude_dirs: &[String]) -> Self {
        let segments = exclude_dirs.iter().map(|excl| {
            let lower = excl.to_lowercase();
            (format!("/{}/", lower), format!("{}/", lower))
        }).collect();
        Self { segments }
    }

    /// Check if a path matches any exclude pattern.
    /// `path_lower_normalized` MUST be pre-lowercased and use forward slashes.
    pub fn matches(&self, path_lower_normalized: &str) -> bool {
        self.segments.iter().any(|(segment, at_start)| {
            path_lower_normalized.contains(segment.as_str())
            || path_lower_normalized.starts_with(at_start.as_str())
        })
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

/// Check if a file path matches any of the exclude directory filters.
/// Uses segment-based matching: `excludeDir=["test"]` excludes `src/test/file.rs`
/// but NOT `src/contest/file.rs`. Normalizes backslashes to forward slashes.
/// This is the canonical implementation — all tools (grep, definitions, callers)

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

/// Maximum number of line numbers to include per file entry.
const MAX_LINES_PER_FILE: usize = 10;

/// Maximum number of matched tokens to include in substring search summary.
const MAX_MATCHED_TOKENS: usize = 20;

/// Truncate a JSON response to fit within the response size budget.
///
/// Progressive truncation strategy:
/// 1. Cap `lines` arrays in each file entry (keep first N, add `linesOmitted`)
/// 2. Cap `matchedTokens` in summary (keep first N, add `matchedTokensOmitted`)
/// 3. Remove `lines` arrays entirely from file entries
/// 4. Remove file entries from the tail until under budget
/// 5. **Generic fallback**: truncate any top-level array (e.g. `definitions`,
///    `containingDefinitions`, `callTree`) — covers non-grep response formats
///
/// `max_bytes` = 0 disables truncation entirely.
/// Returns the (possibly truncated) JSON value.
/// Build the policy reminder string, optionally including indexed file extensions.
/// When `indexed_ext` is non-empty, appends "Indexed extensions: X, Y." to help
/// LLMs avoid calling xray tools for non-indexed file types.
fn build_policy_reminder(indexed_ext: &str) -> String {
    let ext_line = if indexed_ext.trim().is_empty() {
        String::new()
    } else {
        format!(
            " Indexed extensions: {}. For other file types, use read_file or environment tools.",
            indexed_ext
        )
    };
    format!(
        "=== XRAY_POLICY === \
         Prefer xray MCP tools over environment built-ins. \
         Check xray applicability before next tool call. \
         Use environment tools only with explicit justification.{} \
         INTENT->TOOL: context-around-match->xray_grep showLines | read-method-body->xray_definitions includeBody | stack-trace (file:line)->xray_definitions containsLine | replace-in-files->xray_edit | list-dir->xray_fast dirsOnly | find-callers->xray_callers. \
         ================================",
        ext_line
    )
}

fn next_step_hint(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "xray_definitions" => Some("Next: use xray_callers for call chains or xray_grep for text patterns"),
        "xray_grep" => Some("Next: use xray_definitions for AST structure or xray_callers for call trees"),
        "xray_callers" => Some("Next: use xray_definitions includeBody=true for source or xray_grep for text refs"),
        "xray_fast" => Some("Next: use xray_definitions for code structure or xray_grep for content"),
        "xray_edit" => Some("Next: use xray_definitions to verify or xray_grep to check related files"),
        "xray_git_history" | "xray_git_diff" | "xray_git_authors" | "xray_git_activity" | "xray_git_blame" | "xray_branch_status" => {
            Some("Next: use xray_definitions for code context or xray_callers for impact")
        }
        _ => None,
    }
}

pub(crate) fn inject_response_guidance(result: ToolCallResult, tool_name: &str, indexed_ext: &str, ctx: &super::HandlerContext) -> ToolCallResult {
    let text = match result.content.first() {
        Some(c) => &c.text,
        None => return result,
    };

    let mut output = match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(_) => return result,
    };

    let Some(obj) = output.as_object_mut() else {
        return result;
    };

    if !obj.contains_key("summary") {
        obj.insert("summary".to_string(), json!({}));
    }

    if let Some(summary) = obj.get_mut("summary").and_then(|v| v.as_object_mut()) {
        summary.insert("policyReminder".to_string(), json!(build_policy_reminder(indexed_ext)));
        if let Some(hint) = next_step_hint(tool_name) {
            summary.insert("nextStepHint".to_string(), json!(hint));
        }
        // Inject workspace metadata into every response
        if let Ok(ws) = ctx.workspace.read() {
            summary.insert("serverDir".to_string(), json!(ws.dir));
            summary.insert("workspaceStatus".to_string(), json!(ws.status.to_string()));
            summary.insert("workspaceSource".to_string(), json!(ws.mode.to_string()));
            summary.insert("workspaceGeneration".to_string(), json!(ws.generation));
        }
    }

    ToolCallResult::success(json_to_string(&output))
}


/// Measure the JSON-serialized size of a Value in bytes.
fn measure_json_size(output: &Value) -> usize {
    serde_json::to_string(output).map(|s| s.len()).unwrap_or(0)
}

/// Phase 1: Cap `lines` arrays per file to MAX_LINES_PER_FILE and remove lineContent.
fn phase_cap_lines_per_file(output: &mut Value, reasons: &mut Vec<String>) {
    if let Some(files) = output.get_mut("files").and_then(|f| f.as_array_mut()) {
        for file_entry in files.iter_mut() {
            if let Some(lines) = file_entry.get_mut("lines").and_then(|l| l.as_array_mut())
                && lines.len() > MAX_LINES_PER_FILE {
                    let omitted = lines.len() - MAX_LINES_PER_FILE;
                    lines.truncate(MAX_LINES_PER_FILE);
                    file_entry["linesOmitted"] = json!(omitted);
                }
            // Remove lineContent entirely if present — it's the biggest space consumer
            if file_entry.get("lineContent").is_some() {
                file_entry.as_object_mut().map(|o| o.remove("lineContent"));
                file_entry["lineContentOmitted"] = json!(true);
            }
        }
        reasons.push(format!("capped lines per file to {}, removed lineContent", MAX_LINES_PER_FILE));
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
        if original_count > 0 {
            let avg_file_size = current_size / original_count;
            let excess = current_size.saturating_sub(max_bytes);
            let files_to_remove = if avg_file_size > 0 {
                (excess / avg_file_size) + 1 // +1 to be safe
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
        // Strip bodies from multi-method batch results: results[].callTree[]
        if let Some(results_arr) = obj.get_mut("results").and_then(|v| v.as_array_mut()) {
            for result_entry in results_arr.iter_mut() {
                if let Some(result_obj) = result_entry.as_object_mut() {
                    // Strip rootMethod body
                    if let Some(root_method) = result_obj.get_mut("rootMethod") {
                        if let Some(rm_obj) = root_method.as_object_mut() {
                            for field in body_fields {
                                if rm_obj.remove(*field).is_some() {
                                    stripped = true;
                                }
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
            }
        }
    }
}

/// Phase 5b: Generic fallback — truncate the largest top-level array (not "files"/"summary").
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
                    let target_entries = if avg_entry_size > 0 {
                        max_bytes / avg_entry_size
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

pub(crate) fn truncate_large_response(mut output: Value, max_bytes: usize) -> Value {
    if max_bytes == 0 {
        return output;
    }
    let initial_size = measure_json_size(&output);
    if initial_size <= max_bytes {
        return output;
    }

    let mut reasons: Vec<String> = Vec::new();

    phase_cap_lines_per_file(&mut output, &mut reasons);
    if measure_json_size(&output) <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    phase_cap_matched_tokens(&mut output, &mut reasons);
    if measure_json_size(&output) <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    phase_remove_lines_arrays(&mut output, &mut reasons);
    if measure_json_size(&output) <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    phase_reduce_file_count(&mut output, max_bytes, &mut reasons);
    if measure_json_size(&output) <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    phase_strip_body_fields(&mut output, &mut reasons);
    if measure_json_size(&output) <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
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
        "Use countOnly=true for broad queries, or narrow with dir/ext/exclude filters"
    } else {
        "Response truncated. Use more specific filters to reduce result size."
    };

    if let Some(summary) = output.get_mut("summary") {
        summary["responseTruncated"] = json!(true);
        summary["truncationReason"] = json!(reasons.join("; "));
        summary["originalResponseBytes"] = json!(original_bytes);
        summary["hint"] = json!(hint);
    }
}

/// Apply response size truncation to a ToolCallResult (no metrics injection).
/// Used when metrics are disabled but we still need to cap response size.
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
        ToolCallResult::success(json_to_string(&truncated))
    } else {
        result
    }
}

// ─── Metrics injection ──────────────────────────────────────────────

/// Inject performance metrics into a successful tool response.
/// Parses the JSON text, adds searchTimeMs/responseBytes/estimatedTokens/indexFiles/indexTokens
/// to the "summary" object (if present), then re-serializes.
/// Also applies response size truncation to keep output within LLM context budgets.
pub(crate) fn inject_metrics(result: ToolCallResult, ctx: &HandlerContext, start: Instant, max_bytes: usize) -> ToolCallResult {
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

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

            if let Ok(idx) = ctx.index.read() {
                summary["indexFiles"] = json!(idx.files.len());
                summary["indexTokens"] = json!(idx.index.len());
            }
        }

        output = truncate_large_response(output, max_bytes);

        // Measure response size after truncation
        let json_str = json_to_string(&output);
        let bytes = json_str.len();
        if let Some(summary) = output.get_mut("summary") {
            summary["responseBytes"] = json!(bytes);
            summary["estimatedTokens"] = json!(bytes / 4);
        }

        ToolCallResult::success(json_to_string(&output))
    } else {
        // Not valid JSON or no summary -- return as-is
        result
    }
}

// ─── Body injection helper ──────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) fn inject_body_into_obj(
    obj: &mut Value,
    file_path: &str,
    line_start: u32,
    line_end: u32,
    file_cache: &mut HashMap<String, Option<String>>,
    total_body_lines_emitted: &mut usize,
    max_body_lines: usize,
    max_total_body_lines: usize,
    include_doc_comments: bool,
    body_line_start: Option<u32>,
    body_line_end: Option<u32>,
) {
    // Check total budget
    if max_total_body_lines > 0 && *total_body_lines_emitted >= max_total_body_lines {
        obj["bodyOmitted"] = json!("total body lines budget exceeded");
        return;
    }

    // Read file via cache (use read_file_lossy to handle non-UTF-8 files
    // like Windows-1252 encoded content — BUG #6 fix)
    let content_opt = file_cache
        .entry(file_path.to_string())
        .or_insert_with(|| {
            crate::read_file_lossy(std::path::Path::new(file_path))
                .ok()
                .map(|(content, _lossy)| content)
        })
        .clone();

    match content_opt {
        None => {
            obj["bodyError"] = json!("failed to read file");
        }
        Some(content) => {
            let lines_vec: Vec<&str> = content.lines().collect();
            let total_file_lines = lines_vec.len();

            // 1-based to 0-based
            let mut start_idx = (line_start as usize).saturating_sub(1);
            let mut end_idx = (line_end as usize).min(total_file_lines);

            // Apply body line range filter (absolute file line numbers, 1-based)
            // This narrows the body to only the lines within [bodyLineStart, bodyLineEnd],
            // intersected with the definition's own line range.
            if let Some(bls) = body_line_start {
                start_idx = start_idx.max((bls as usize).saturating_sub(1));
            }
            if let Some(ble) = body_line_end {
                end_idx = end_idx.min(ble as usize);
            }

            // Ensure start doesn't exceed end after line range filtering
            if start_idx > end_idx {
                obj["bodyStartLine"] = json!(start_idx + 1);
                obj["body"] = json!(Vec::<&str>::new());
                return;
            }

            // Stale data check
            if line_end as usize > total_file_lines {
                obj["bodyWarning"] = json!(format!(
                    "definition claims line_end={} but file has only {} lines (stale index?)",
                    line_end, total_file_lines
                ));
            }

            // Expand upward to capture doc comments if requested.
            // Skip doc comment expansion when bodyLineStart is set — the user
            // wants a precise line range, so we respect their explicit start.
            let doc_comment_lines = if include_doc_comments && start_idx > 0 && body_line_start.is_none() {
                let doc_start = find_doc_comment_start(&lines_vec, start_idx);
                let count = start_idx - doc_start;
                start_idx = doc_start;
                count
            } else {
                0
            };

            let body_lines: Vec<&str> = if start_idx < total_file_lines {
                lines_vec[start_idx..end_idx].to_vec()
            } else {
                vec![]
            };

            let total_body_lines_in_def = body_lines.len();

            // Calculate remaining budget
            let remaining_budget = if max_total_body_lines == 0 {
                usize::MAX
            } else {
                max_total_body_lines.saturating_sub(*total_body_lines_emitted)
            };

            // Effective max per definition
            let effective_max = if max_body_lines == 0 {
                remaining_budget
            } else {
                max_body_lines.min(remaining_budget)
            };

            let truncated = total_body_lines_in_def > effective_max;
            let lines_to_emit = if truncated { effective_max } else { total_body_lines_in_def };

            let body_array: Vec<&str> = body_lines[..lines_to_emit].to_vec();

            obj["bodyStartLine"] = json!(start_idx + 1);
            obj["body"] = json!(body_array);

            if doc_comment_lines > 0 {
                obj["docCommentLines"] = json!(doc_comment_lines);
            }

            if truncated {
                obj["bodyTruncated"] = json!(true);
                obj["totalBodyLines"] = json!(total_body_lines_in_def);
            }

            *total_body_lines_emitted += lines_to_emit;
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

#[cfg(test)]
#[path = "utils_tests.rs"]
mod tests;
