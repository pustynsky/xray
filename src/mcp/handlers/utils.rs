//! Shared utility functions for MCP tool handlers.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;

use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use crate::clean_path;

use super::HandlerContext;

// ─── Branch warning ─────────────────────────────────────────────────

/// Returns a warning message if the current branch is not `main` or `master`.
/// Used by index-based tools (search_grep, search_definitions, search_callers,
/// search_fast) to alert the user that results may differ from production.
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
pub(crate) fn validate_search_dir(requested_dir: &str, server_dir: &str) -> Result<Option<String>, String> {
    let requested = std::fs::canonicalize(requested_dir)
        .map(|p| clean_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| requested_dir.to_string());
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

// ─── Extension filter helper ────────────────────────────────────────

/// Check if a file path's extension matches a filter string.
/// Supports comma-separated extensions: `"cs,sql"` matches both `.cs` and `.sql`.
/// Comparison is case-insensitive. Whitespace around extensions is trimmed.
pub(crate) fn matches_ext_filter(file_path: &str, ext_filter: &str) -> bool {
    std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ext_filter.split(',')
                .any(|allowed| e.eq_ignore_ascii_case(allowed.trim()))
        })
}

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

/// Build compact grouped lineContent for search_grep from raw file content.
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
#[allow(dead_code)]
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
pub(crate) fn truncate_large_response(mut output: Value, max_bytes: usize) -> Value {
    if max_bytes == 0 {
        return output;
    }
    let initial_size = serde_json::to_string(&output).map(|s| s.len()).unwrap_or(0);
    if initial_size <= max_bytes {
        return output;
    }

    let mut reasons: Vec<String> = Vec::new();

    // Phase 1: Cap `lines` arrays per file
    if let Some(files) = output.get_mut("files").and_then(|f| f.as_array_mut()) {
        for file_entry in files.iter_mut() {
            if let Some(lines) = file_entry.get_mut("lines").and_then(|l| l.as_array_mut()) {
                if lines.len() > MAX_LINES_PER_FILE {
                    let omitted = lines.len() - MAX_LINES_PER_FILE;
                    lines.truncate(MAX_LINES_PER_FILE);
                    file_entry["linesOmitted"] = json!(omitted);
                }
            }
            // Remove lineContent entirely if present — it's the biggest space consumer
            if file_entry.get("lineContent").is_some() {
                file_entry.as_object_mut().map(|o| o.remove("lineContent"));
                file_entry["lineContentOmitted"] = json!(true);
            }
        }
        reasons.push(format!("capped lines per file to {}, removed lineContent", MAX_LINES_PER_FILE));
    }

    // Check size after phase 1
    let size_after_p1 = serde_json::to_string(&output).map(|s| s.len()).unwrap_or(0);
    if size_after_p1 <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    // Phase 2: Cap `matchedTokens` in summary
    if let Some(summary) = output.get_mut("summary") {
        if let Some(tokens) = summary.get_mut("matchedTokens").and_then(|t| t.as_array_mut()) {
            if tokens.len() > MAX_MATCHED_TOKENS {
                let omitted = tokens.len() - MAX_MATCHED_TOKENS;
                tokens.truncate(MAX_MATCHED_TOKENS);
                summary["matchedTokensOmitted"] = json!(omitted);
                reasons.push(format!("capped matchedTokens to {}", MAX_MATCHED_TOKENS));
            }
        }
    }

    // Check size after phase 2
    let size_after_p2 = serde_json::to_string(&output).map(|s| s.len()).unwrap_or(0);
    if size_after_p2 <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    // Phase 3: Remove `lines` arrays entirely from file entries
    if let Some(files) = output.get_mut("files").and_then(|f| f.as_array_mut()) {
        for file_entry in files.iter_mut() {
            if file_entry.get("lines").is_some() {
                file_entry.as_object_mut().map(|o| o.remove("lines"));
            }
        }
        reasons.push("removed all lines arrays".to_string());
    }

    // Check size after phase 3
    let size_after_p3 = serde_json::to_string(&output).map(|s| s.len()).unwrap_or(0);
    if size_after_p3 <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    // Phase 4: Progressively remove file entries from the tail.
    // Estimate how many files to keep based on average file entry size.
    let size_p3 = serde_json::to_string(&output).map(|s| s.len()).unwrap_or(0);
    if let Some(files) = output.get_mut("files").and_then(|f| f.as_array_mut()) {
        let original_count = files.len();
        if original_count > 0 {
            let avg_file_size = size_p3 / original_count;
            let excess = size_p3.saturating_sub(max_bytes);
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

    // Check size after phase 4
    let size_after_p4 = serde_json::to_string(&output).map(|s| s.len()).unwrap_or(0);
    if size_after_p4 <= max_bytes {
        inject_truncation_metadata(&mut output, &reasons, initial_size);
        return output;
    }

    // Phase 5: Generic fallback — truncate any top-level array that isn't "files"
    // (already handled above). This covers "definitions", "containingDefinitions",
    // "callTree", or any future tool response format.
    let current_size = serde_json::to_string(&output).map(|s| s.len()).unwrap_or(0);
    if current_size > max_bytes {
        if let Some(obj) = output.as_object_mut() {
            // Find the largest top-level array (skip "files" — already handled)
            let largest_array_key = obj.iter()
                .filter(|(k, v)| *k != "files" && *k != "summary" && v.is_array())
                .max_by_key(|(_, v)| v.as_array().map(|a| a.len()).unwrap_or(0))
                .map(|(k, _)| k.clone());

            if let Some(key) = largest_array_key {
                if let Some(arr) = obj.get_mut(&key).and_then(|v| v.as_array_mut()) {
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
    }

    inject_truncation_metadata(&mut output, &reasons, initial_size);
    output
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
        ToolCallResult::success(serde_json::to_string(&truncated).unwrap())
    } else {
        result
    }
}

// ─── Metrics injection ──────────────────────────────────────────────

/// Inject performance metrics into a successful tool response.
/// Parses the JSON text, adds searchTimeMs/responseBytes/estimatedTokens/indexFiles/indexTokens
/// to the "summary" object (if present), then re-serializes.
/// Also applies response size truncation to keep output within LLM context budgets.
pub(crate) fn inject_metrics(result: ToolCallResult, ctx: &HandlerContext, start: Instant) -> ToolCallResult {
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Get the text from the first content item
    let text = match result.content.first() {
        Some(c) => &c.text,
        None => return result,
    };

    // Try to parse as JSON and inject metrics into "summary"
    if let Ok(mut output) = serde_json::from_str::<Value>(text) {
        if let Some(summary) = output.get_mut("summary") {
            summary["searchTimeMs"] = json!((elapsed_ms * 100.0).round() / 100.0);

            if let Ok(idx) = ctx.index.read() {
                summary["indexFiles"] = json!(idx.files.len());
                summary["indexTokens"] = json!(idx.index.len());
            }
        }

        // Apply response size truncation BEFORE measuring final bytes
        let max_bytes = if ctx.max_response_bytes > 0 { ctx.max_response_bytes } else { 0 };
        output = truncate_large_response(output, max_bytes);

        // Measure response size after truncation
        let json_str = serde_json::to_string(&output).unwrap();
        let bytes = json_str.len();
        if let Some(summary) = output.get_mut("summary") {
            summary["responseBytes"] = json!(bytes);
            summary["estimatedTokens"] = json!(bytes / 4);
        }

        ToolCallResult::success(serde_json::to_string(&output).unwrap())
    } else {
        // Not valid JSON or no summary -- return as-is
        result
    }
}

// ─── Body injection helper ──────────────────────────────────────────

pub(crate) fn inject_body_into_obj(
    obj: &mut Value,
    file_path: &str,
    line_start: u32,
    line_end: u32,
    file_cache: &mut HashMap<String, Option<String>>,
    total_body_lines_emitted: &mut usize,
    max_body_lines: usize,
    max_total_body_lines: usize,
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
            let start_idx = (line_start as usize).saturating_sub(1);
            let end_idx = (line_end as usize).min(total_file_lines);

            // Stale data check
            if line_end as usize > total_file_lines {
                obj["bodyWarning"] = json!(format!(
                    "definition claims line_end={} but file has only {} lines (stale index?)",
                    line_end, total_file_lines
                ));
            }

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

            if truncated {
                obj["bodyTruncated"] = json!(true);
                obj["totalBodyLines"] = json!(total_body_lines_in_def);
            }

            *total_body_lines_emitted += lines_to_emit;
        }
    }
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
mod tests {
    use super::*;

    #[test]
    fn test_sorted_intersect_empty_left() {
        assert_eq!(sorted_intersect(&[], &[1, 2, 3]), Vec::<u32>::new());
    }

    #[test]
    fn test_sorted_intersect_empty_right() {
        assert_eq!(sorted_intersect(&[1, 2, 3], &[]), Vec::<u32>::new());
    }

    #[test]
    fn test_sorted_intersect_both_empty() {
        assert_eq!(sorted_intersect(&[], &[]), Vec::<u32>::new());
    }

    #[test]
    fn test_sorted_intersect_disjoint() {
        assert_eq!(sorted_intersect(&[1, 3, 5], &[2, 4, 6]), Vec::<u32>::new());
    }

    #[test]
    fn test_normalize_path_sep() {
        assert_eq!(normalize_path_sep(r"C:\foo\bar"), "C:/foo/bar");
    }

    #[test]
    fn test_is_under_dir_basic() {
        assert!(is_under_dir("C:/Repos/MyProject/src/file.cs", "C:/Repos/MyProject"));
    }

    #[test]
    fn test_is_under_dir_case_insensitive() {
        assert!(is_under_dir("C:/repos/myproject/src/file.cs", "C:/Repos/MyProject"));
    }

    #[test]
    fn test_is_under_dir_not_prefix_of_different_dir() {
        assert!(!is_under_dir("C:/Repos/MainProjectExtra/file.cs", "C:/Repos/MainProject"));
    }

    #[test]
    fn test_is_under_dir_exact_match() {
        assert!(!is_under_dir("C:/Repos/MainProject", "C:/Repos/MainProject"));
    }

    #[test]
    fn test_validate_search_dir_exact_match() {
        // We can't easily test this without real directories, but we can test the logic
        // with paths that don't exist (canonicalize will fail, falling back to raw string)
        let result = validate_search_dir("/nonexistent/dir", "/nonexistent/dir");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_validate_search_dir_outside_rejects() {
        let result = validate_search_dir("/other/dir", "/my/dir");
        assert!(result.is_err());
    }

    #[test]
    fn test_grouped_line_content_single_group() {
        let lines = vec!["line0", "line1", "line2", "line3", "line4"];
        let mut to_show = BTreeSet::new();
        to_show.insert(1);
        to_show.insert(2);
        to_show.insert(3);
        let mut match_set = HashSet::new();
        match_set.insert(2);

        let result = build_grouped_line_content(&to_show, &lines, &match_set);
        let groups = result.as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["startLine"], 2);
        assert_eq!(groups[0]["lines"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_grouped_line_content_two_groups() {
        let lines = vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
        let mut to_show = BTreeSet::new();
        to_show.insert(1);
        to_show.insert(2);
        to_show.insert(7);
        to_show.insert(8);
        let mut match_set = HashSet::new();
        match_set.insert(1);
        match_set.insert(8);

        let result = build_grouped_line_content(&to_show, &lines, &match_set);
        let groups = result.as_array().unwrap();
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn test_grouped_line_content_no_matches() {
        let lines = vec!["a", "b", "c"];
        let mut to_show = BTreeSet::new();
        to_show.insert(0);
        let match_set = HashSet::new();

        let result = build_grouped_line_content(&to_show, &lines, &match_set);
        let groups = result.as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].get("matchIndices").is_none());
    }

    #[test]
    fn test_grouped_line_content_empty() {
        let lines: Vec<&str> = vec![];
        let to_show = BTreeSet::new();
        let match_set = HashSet::new();

        let result = build_grouped_line_content(&to_show, &lines, &match_set);
        let groups = result.as_array().unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn test_grouped_line_content_multiple_matches_in_group() {
        let lines = vec!["a", "b", "c", "d", "e"];
        let mut to_show = BTreeSet::new();
        for i in 0..5 { to_show.insert(i); }
        let mut match_set = HashSet::new();
        match_set.insert(1);
        match_set.insert(3);

        let result = build_grouped_line_content(&to_show, &lines, &match_set);
        let groups = result.as_array().unwrap();
        assert_eq!(groups.len(), 1);
        let indices = groups[0]["matchIndices"].as_array().unwrap();
        assert_eq!(indices.len(), 2);
    }

    #[test]
    fn test_context_lines_calculation() {
        let content = (0..20).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let match_lines = vec![10u32]; // line 10 (1-based)
        let result = build_line_content_from_matches(&content, &match_lines, 2);
        let groups = result.as_array().unwrap();
        assert_eq!(groups.len(), 1);
        // Should show lines 8-12 (5 lines: 2 before + match + 2 after)
        let lines = groups[0]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn test_context_lines_at_file_boundaries() {
        let content = "line1\nline2\nline3";
        let match_lines = vec![1u32];
        let result = build_line_content_from_matches(&content, &match_lines, 5);
        let groups = result.as_array().unwrap();
        assert_eq!(groups.len(), 1);
        let lines = groups[0]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3); // can't go before line 1
    }

    #[test]
    fn test_context_merges_overlapping_ranges() {
        let content = (0..20).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let match_lines = vec![5u32, 7u32]; // lines 5 and 7 with context 2 overlap
        let result = build_line_content_from_matches(&content, &match_lines, 2);
        let groups = result.as_array().unwrap();
        assert_eq!(groups.len(), 1); // should merge into single group
    }

    // ─── Response truncation tests ──────────────────────────────────

    #[test]
    fn test_truncate_small_response_unchanged() {
        let output = json!({
            "files": [{"path": "a.cs", "lines": [1, 2, 3]}],
            "summary": {"totalFiles": 1}
        });
        let result = truncate_large_response(output.clone(), DEFAULT_MAX_RESPONSE_BYTES);
        // Small response should be unchanged
        assert_eq!(result, output);
    }

    #[test]
    fn test_truncate_caps_lines_per_file() {
        // Build a response with files having many lines
        let many_lines: Vec<u32> = (1..=200).collect();
        let mut files = Vec::new();
        for i in 0..100 {
            files.push(json!({
                "path": format!("/some/very/long/path/to/file_{}.cs", i),
                "score": 0.5,
                "occurrences": 200,
                "lines": many_lines,
            }));
        }
        let output = json!({
            "files": files,
            "summary": {"totalFiles": 100, "totalOccurrences": 20000}
        });

        let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
        let result_str = serde_json::to_string(&result).unwrap();

        // Should be truncated
        assert!(result.get("summary").unwrap().get("responseTruncated").is_some(),
            "Expected responseTruncated in summary");

        // Lines per file should be capped
        if let Some(files) = result.get("files").and_then(|f| f.as_array()) {
            for file in files {
                if let Some(lines) = file.get("lines").and_then(|l| l.as_array()) {
                    assert!(lines.len() <= MAX_LINES_PER_FILE,
                        "Lines array should be capped to {}", MAX_LINES_PER_FILE);
                }
            }
        }

        // Final size should be under budget
        assert!(result_str.len() <= DEFAULT_MAX_RESPONSE_BYTES + 500, // small tolerance for metadata
            "Response {} bytes should be near budget {}", result_str.len(), DEFAULT_MAX_RESPONSE_BYTES);
    }

    #[test]
    fn test_truncate_caps_matched_tokens() {
        let many_tokens: Vec<String> = (0..500).map(|i| format!("token_{}", i)).collect();
        let mut files = Vec::new();
        for i in 0..50 {
            files.push(json!({
                "path": format!("/path/file_{}.cs", i),
                "lines": [1, 2, 3],
            }));
        }
        let output = json!({
            "files": files,
            "summary": {
                "totalFiles": 50,
                "matchedTokens": many_tokens,
            }
        });

        let initial_size = serde_json::to_string(&output).unwrap().len();
        if initial_size > DEFAULT_MAX_RESPONSE_BYTES {
            let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
            if let Some(tokens) = result.get("summary")
                .and_then(|s| s.get("matchedTokens"))
                .and_then(|t| t.as_array())
            {
                assert!(tokens.len() <= MAX_MATCHED_TOKENS,
                    "matchedTokens should be capped to {}", MAX_MATCHED_TOKENS);
            }
        }
    }

    #[test]
    fn test_truncate_removes_line_content() {
        // Build a response with lineContent (large)
        let mut files = Vec::new();
        for i in 0..50 {
            files.push(json!({
                "path": format!("/path/file_{}.cs", i),
                "lines": [1, 2, 3],
                "lineContent": [{
                    "startLine": 1,
                    "lines": (0..100).map(|j| format!("    some code line {} in file {}", j, i)).collect::<Vec<_>>(),
                }],
            }));
        }
        let output = json!({
            "files": files,
            "summary": {"totalFiles": 50}
        });

        let initial_size = serde_json::to_string(&output).unwrap().len();
        if initial_size > DEFAULT_MAX_RESPONSE_BYTES {
            let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
            // lineContent should be removed
            if let Some(files) = result.get("files").and_then(|f| f.as_array()) {
                for file in files {
                    assert!(file.get("lineContent").is_none(),
                        "lineContent should be removed during truncation");
                }
            }
        }
    }

    #[test]
    fn test_truncate_reduces_file_count() {
        // Build a response with 1000 files — way over budget
        let mut files = Vec::new();
        for i in 0..1000 {
            files.push(json!({
                "path": format!("/some/long/path/to/deeply/nested/file_number_{}.cs", i),
                "score": 0.001,
                "occurrences": 1,
                "lines": [1],
            }));
        }
        let output = json!({
            "files": files,
            "summary": {"totalFiles": 1000, "totalOccurrences": 1000}
        });

        let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
        let result_files = result.get("files").and_then(|f| f.as_array()).unwrap();
        assert!(result_files.len() < 1000,
            "File count should be reduced from 1000, got {}", result_files.len());

        // Summary should indicate truncation
        let summary = result.get("summary").unwrap();
        assert_eq!(summary.get("responseTruncated").and_then(|v| v.as_bool()), Some(true));
        assert!(summary.get("truncationReason").is_some());
        assert!(summary.get("hint").is_some());
    }

    #[test]
    fn test_truncate_response_if_needed_small() {
        let small = ToolCallResult::success(r#"{"files":[],"summary":{"totalFiles":0}}"#.to_string());
        let result = truncate_response_if_needed(small, DEFAULT_MAX_RESPONSE_BYTES);
        assert!(!result.is_error);
    }

    #[test]
    fn test_truncate_definitions_array() {
        // Build a search_definitions-style response with many definitions — way over budget
        let mut defs = Vec::new();
        for i in 0..5000 {
            defs.push(json!({
                "name": format!("SomeDefinitionName_{}", i),
                "kind": "property",
                "file": format!("/some/long/path/to/deeply/nested/file_{}.ts", i % 100),
                "lines": format!("{}-{}", i * 10, i * 10 + 5),
                "modifiers": ["public"],
                "parent": format!("SomeParentClass_{}", i % 50),
            }));
        }
        let output = json!({
            "definitions": defs,
            "summary": {
                "totalResults": 5000,
                "returned": 5000,
                "searchTimeMs": 1.23,
                "indexFiles": 500,
                "totalDefinitions": 50000,
            }
        });

        let initial_size = serde_json::to_string(&output).unwrap().len();
        assert!(initial_size > DEFAULT_MAX_RESPONSE_BYTES,
            "Test setup: definitions response should be over budget ({} bytes)", initial_size);

        let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
        let result_str = serde_json::to_string(&result).unwrap();

        // Definitions array should be truncated
        let result_defs = result.get("definitions").and_then(|d| d.as_array()).unwrap();
        assert!(result_defs.len() < 5000,
            "Definitions count should be reduced from 5000, got {}", result_defs.len());

        // Summary should indicate truncation
        let summary = result.get("summary").unwrap();
        assert_eq!(summary.get("responseTruncated").and_then(|v| v.as_bool()), Some(true));
        assert!(summary.get("truncationReason").is_some());
        let reason = summary["truncationReason"].as_str().unwrap();
        assert!(reason.contains("definitions"),
            "Truncation reason should mention 'definitions', got: {}", reason);

        // 'returned' in summary should reflect actual array length after truncation
        let returned = summary.get("returned").and_then(|v| v.as_u64()).unwrap() as usize;
        assert_eq!(returned, result_defs.len(),
            "summary.returned ({}) should match actual definitions array length ({})",
            returned, result_defs.len());

        // Hint should be definitions-specific (not grep-specific)
        let hint = summary.get("hint").and_then(|v| v.as_str()).unwrap();
        assert!(hint.contains("name") && hint.contains("kind") && hint.contains("file"),
            "Hint should mention definitions-specific filters, got: {}", hint);
        assert!(!hint.contains("countOnly"),
            "Hint should NOT mention countOnly (that's for grep), got: {}", hint);

        // Result should be reasonably close to budget
        assert!(result_str.len() <= DEFAULT_MAX_RESPONSE_BYTES * 2,
            "Response {} bytes should be near budget {}", result_str.len(), DEFAULT_MAX_RESPONSE_BYTES);
    }

    #[test]
    fn test_truncate_grep_hint_unchanged() {
        // Verify grep-style responses still get the grep-specific hint
        let mut files = Vec::new();
        for i in 0..1000 {
            files.push(json!({
                "path": format!("/some/long/path/to/file_{}.cs", i),
                "score": 0.001,
                "occurrences": 1,
                "lines": [1],
            }));
        }
        let output = json!({
            "files": files,
            "summary": {"totalFiles": 1000, "totalOccurrences": 1000}
        });

        let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
        let summary = result.get("summary").unwrap();
        let hint = summary.get("hint").and_then(|v| v.as_str()).unwrap();
        assert!(hint.contains("countOnly"),
            "Grep hint should mention countOnly, got: {}", hint);
        assert!(!hint.contains("kind"),
            "Grep hint should NOT mention definitions filters, got: {}", hint);
    }

    // ─── best_match_tier relevance ranking tests ─────────────────────

    #[test]
    fn test_best_match_tier_exact_match_returns_0() {
        let terms = vec!["userservice".to_string()];
        assert_eq!(best_match_tier("UserService", &terms), 0);
    }

    #[test]
    fn test_best_match_tier_exact_match_case_insensitive() {
        let terms = vec!["userservice".to_string()];
        assert_eq!(best_match_tier("USERSERVICE", &terms), 0);
        assert_eq!(best_match_tier("userservice", &terms), 0);
        assert_eq!(best_match_tier("UserService", &terms), 0);
    }

    #[test]
    fn test_best_match_tier_prefix_match_returns_1() {
        let terms = vec!["userservice".to_string()];
        assert_eq!(best_match_tier("UserServiceFactory", &terms), 1);
    }

    #[test]
    fn test_best_match_tier_contains_only_returns_2() {
        let terms = vec!["userservice".to_string()];
        assert_eq!(best_match_tier("IUserService", &terms), 2);
    }

    #[test]
    fn test_best_match_tier_no_match_returns_2() {
        // The function is called only on already-filtered results,
        // so a non-matching name still returns 2 (contains/default tier).
        let terms = vec!["userservice".to_string()];
        assert_eq!(best_match_tier("OrderProcessor", &terms), 2);
    }

    #[test]
    fn test_best_match_tier_multiple_terms_best_wins() {
        let terms = vec!["order".to_string(), "userservice".to_string()];
        // "UserService" is exact match for "userservice" → tier 0
        assert_eq!(best_match_tier("UserService", &terms), 0);
        // "OrderProcessor" is prefix match for "order" → tier 1
        assert_eq!(best_match_tier("OrderProcessor", &terms), 1);
        // "IUserService" contains "userservice" → tier 2
        assert_eq!(best_match_tier("IUserService", &terms), 2);
    }

    #[test]
    fn test_best_match_tier_empty_terms_returns_2() {
        let terms: Vec<String> = vec![];
        assert_eq!(best_match_tier("UserService", &terms), 2);
    }

    #[test]
    fn test_best_match_tier_exact_beats_prefix_with_multiple_terms() {
        // When one term is exact and another is prefix, exact wins (tier 0)
        let terms = vec!["iuserservice".to_string(), "userservice".to_string()];
        // "UserService" is exact for "userservice" → 0
        assert_eq!(best_match_tier("UserService", &terms), 0);
        // "IUserService" is exact for "iuserservice" → 0
        assert_eq!(best_match_tier("IUserService", &terms), 0);
    }

    // ─── matches_ext_filter tests ────────────────────────────────────

    #[test]
    fn test_matches_ext_filter_single() {
        assert!(matches_ext_filter("src/file.cs", "cs"));
        assert!(!matches_ext_filter("src/file.ts", "cs"));
    }

    #[test]
    fn test_matches_ext_filter_multi() {
        assert!(matches_ext_filter("src/file.cs", "cs,sql"));
        assert!(matches_ext_filter("src/file.sql", "cs,sql"));
        assert!(!matches_ext_filter("src/file.ts", "cs,sql"));
    }

    #[test]
    fn test_matches_ext_filter_case_insensitive() {
        assert!(matches_ext_filter("src/file.CS", "cs"));
        assert!(matches_ext_filter("src/file.cs", "CS"));
    }

    #[test]
    fn test_matches_ext_filter_with_spaces() {
        assert!(matches_ext_filter("src/file.cs", " cs , sql "));
        assert!(matches_ext_filter("src/file.sql", " cs , sql "));
    }

    #[test]
    fn test_matches_ext_filter_no_extension() {
        assert!(!matches_ext_filter("Makefile", "cs"));
    }

    #[test]
    fn test_best_match_tier_prefix_beats_contains() {
        let terms = vec!["user".to_string()];
        // "UserService" starts with "user" → tier 1
        assert_eq!(best_match_tier("UserService", &terms), 1);
        // "IUserService" contains "user" but doesn't start with it → tier 2
        assert_eq!(best_match_tier("IUserService", &terms), 2);
    }

    // ─── branch_warning tests ─────────────────────────────────────────

    /// Helper: create a minimal HandlerContext with a given current_branch.
    fn make_ctx_with_branch(branch: Option<&str>) -> HandlerContext {
        use std::sync::atomic::AtomicBool;
        use crate::{ContentIndex, TrigramIndex};

        let index = ContentIndex {
            root: ".".to_string(),
            ..Default::default()
        };
        HandlerContext {
            index: std::sync::Arc::new(std::sync::RwLock::new(index)),
            def_index: None,
            server_dir: ".".to_string(),
            server_ext: "cs".to_string(),
            metrics: false,
            index_base: std::path::PathBuf::from("."),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            content_ready: std::sync::Arc::new(AtomicBool::new(true)),
            def_ready: std::sync::Arc::new(AtomicBool::new(true)),
            git_cache: std::sync::Arc::new(std::sync::RwLock::new(None)),
            git_cache_ready: std::sync::Arc::new(AtomicBool::new(false)),
            current_branch: branch.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_branch_warning_feature_branch() {
        let ctx = make_ctx_with_branch(Some("feature/xyz"));
        let warning = branch_warning(&ctx);
        assert!(warning.is_some());
        let msg = warning.unwrap();
        assert!(msg.contains("feature/xyz"));
        assert!(msg.contains("not on main/master"));
    }

    #[test]
    fn test_branch_warning_main_branch() {
        let ctx = make_ctx_with_branch(Some("main"));
        assert!(branch_warning(&ctx).is_none());
    }

    #[test]
    fn test_branch_warning_master_branch() {
        let ctx = make_ctx_with_branch(Some("master"));
        assert!(branch_warning(&ctx).is_none());
    }

    #[test]
    fn test_branch_warning_none_branch() {
        let ctx = make_ctx_with_branch(None);
        assert!(branch_warning(&ctx).is_none());
    }

    #[test]
    fn test_inject_branch_warning_adds_field() {
        let ctx = make_ctx_with_branch(Some("users/dev/my-feature"));
        let mut summary = json!({"totalFiles": 5});
        inject_branch_warning(&mut summary, &ctx);
        assert!(summary.get("branchWarning").is_some());
        let warning = summary["branchWarning"].as_str().unwrap();
        assert!(warning.contains("users/dev/my-feature"));
    }

    #[test]
    fn test_inject_branch_warning_skips_main() {
        let ctx = make_ctx_with_branch(Some("main"));
        let mut summary = json!({"totalFiles": 5});
        inject_branch_warning(&mut summary, &ctx);
        assert!(summary.get("branchWarning").is_none());
    }

    #[test]
    fn test_inject_branch_warning_skips_none() {
        let ctx = make_ctx_with_branch(None);
        let mut summary = json!({"totalFiles": 5});
        inject_branch_warning(&mut summary, &ctx);
        assert!(summary.get("branchWarning").is_none());
    }
}