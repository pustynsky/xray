//! MCP tool handler for `search_edit` — reliable file editing with two modes:
//! - Mode A (operations): line-range splice, applied bottom-up to avoid offset cascade
//! - Mode B (edits): text find-replace, literal or regex, with insert after/before support

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use super::utils::json_to_string;
use super::HandlerContext;

/// Maximum number of files for multi-file edit (protection against abuse).
const MAX_MULTI_FILE_PATHS: usize = 20;

/// Maximum file size (in bytes) for nearest-match hint computation.
/// Files larger than this skip the hint to avoid performance impact.
const NEAREST_MATCH_MAX_FILE_SIZE: usize = 512_000; // 500 KB

/// Minimum similarity ratio (0.0–1.0) for a nearest match to be reported.
/// Below this threshold the hint is suppressed as unhelpful.
const NEAREST_MATCH_MIN_SIMILARITY: f64 = 0.4;

/// Maximum length of search/match text shown in hint messages.
const NEAREST_MATCH_MAX_DISPLAY_LEN: usize = 150;

/// Handle `search_edit` tool call.
pub(crate) fn handle_search_edit(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    // ── Parse path/paths ──
    let single_path = args.get("path").and_then(|v| v.as_str());
    let multi_paths = args.get("paths").and_then(|v| v.as_array());

    // Validate: path XOR paths
    match (single_path, multi_paths) {
        (Some(_), Some(_)) => {
            return ToolCallResult::error(
                "Specify 'path' (single file) or 'paths' (multiple files), not both.".to_string(),
            );
        }
        (None, None) => {
            return ToolCallResult::error(
                "Missing required parameter: 'path' (single file) or 'paths' (array of files).".to_string(),
            );
        }
        _ => {}
    }

    // ── Parse common arguments ──
    let operations = args.get("operations").and_then(|v| v.as_array());
    let edits = args.get("edits").and_then(|v| v.as_array());
    let is_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let dry_run = args.get("dryRun").and_then(|v| v.as_bool()).unwrap_or(false);
    let expected_line_count = args.get("expectedLineCount").and_then(|v| v.as_u64()).map(|v| v as usize);

    // ── Validate mode ──
    match (operations, edits) {
        (None, None) => {
            return ToolCallResult::error(
                "Specify 'operations' (line-range) or 'edits' (text-match), not neither.".to_string(),
            );
        }
        (Some(_), Some(_)) => {
            return ToolCallResult::error(
                "Specify 'operations' or 'edits', not both.".to_string(),
            );
        }
        _ => {}
    }

    // ── Dispatch single vs multi-file ──
    if let Some(paths_array) = multi_paths {
        handle_multi_file_edit(ctx, paths_array, operations, edits, is_regex, dry_run, expected_line_count)
    } else {
        let path_str = single_path.unwrap(); // validated above
        handle_single_file_edit(ctx, path_str, operations, edits, is_regex, dry_run, expected_line_count)
    }
}

/// Detail about a single skipped edit (when `skipIfNotFound=true`).
struct SkippedEditDetail {
    /// 0-based index of the edit in the edits array.
    edit_index: usize,
    /// The search/anchor text that was not found (truncated for display).
    search_text: String,
    /// Human-readable reason why the edit was skipped.
    reason: String,
}

/// Result of editing a single file's content (in-memory, before writing).
struct EditResult {
    modified_content: String,
    applied: usize,
    total_replacements: usize,
    skipped_details: Vec<SkippedEditDetail>,
    diff: String,
    lines_added: i64,
    lines_removed: i64,
    new_line_count: usize,
}

/// Read and validate a file, returning its content and line ending style.
fn read_and_validate_file(server_dir: &str, path_str: &str) -> Result<(PathBuf, String, &'static str), String> {
    let resolved = resolve_path(server_dir, path_str);
    if !resolved.exists() {
        return Err(format!("File not found: {}", path_str));
    }
    if resolved.is_dir() {
        return Err(format!("Path is a directory, not a file: {}", path_str));
    }

    let raw_bytes = std::fs::read(&resolved)
        .map_err(|e| format!("Failed to read file '{}': {}", path_str, e))?;

    // Binary detection: check for null bytes in first 8KB
    let check_len = raw_bytes.len().min(8192);
    if raw_bytes[..check_len].contains(&0) {
        return Err(format!("Binary file detected, not editable: {}", path_str));
    }

    let content = match std::str::from_utf8(&raw_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(&raw_bytes).into_owned(),
    };

    let line_ending = detect_line_ending(&content);

    // Normalize to LF for processing
    let normalized = if line_ending == "\r\n" {
        content.replace("\r\n", "\n")
    } else {
        content
    };

    Ok((resolved, normalized, line_ending))
}

/// Apply edits/operations to file content and return results.
fn apply_edits_to_content(
    path_str: &str,
    normalized: &str,
    operations: Option<&Vec<Value>>,
    edits: Option<&Vec<Value>>,
    is_regex: bool,
    expected_line_count: Option<usize>,
) -> Result<EditResult, String> {
    let (modified_content, applied, total_replacements, skipped_details) = if let Some(ops_array) = operations {
        // Mode A: Line-range operations
        let ops = parse_line_operations(ops_array)?;

        let lines: Vec<&str> = normalized.split('\n').collect();

        // expectedLineCount check
        if let Some(expected) = expected_line_count {
            if lines.len() != expected {
                return Err(format!(
                    "Expected {} lines, file has {}. File may have changed.",
                    expected, lines.len()
                ));
            }
        }

        let (new_lines, applied_count) = apply_line_operations(&lines, ops)?;
        (new_lines.join("\n"), applied_count, 0, Vec::new())
    } else if let Some(edits_array) = edits {
        // Mode B: Text-match edits
        let text_edits = parse_text_edits(edits_array)?;

        let (new_content, replacements, skipped) = apply_text_edits(normalized, &text_edits, is_regex)?;
        let edit_count = text_edits.len();
        (new_content, edit_count, replacements, skipped)
    } else {
        unreachable!("Already validated that one of operations/edits is Some");
    };

    // Generate unified diff
    let diff = generate_unified_diff(path_str, normalized, &modified_content);

    // Count changes
    let original_line_count = normalized.split('\n').count();
    let new_line_count = modified_content.split('\n').count();
    let lines_delta = new_line_count as i64 - original_line_count as i64;
    let lines_removed = if lines_delta < 0 { -lines_delta } else { 0 };
    let lines_added = if lines_delta > 0 { lines_delta } else { 0 };

    Ok(EditResult {
        modified_content,
        applied,
        total_replacements,
        skipped_details,
        diff,
        lines_added,
        lines_removed,
        new_line_count,
    })
}

/// Write modified content back to file, restoring original line endings.
fn write_file_with_endings(resolved: &Path, content: &str, line_ending: &str) -> Result<(), String> {
    let output = if line_ending == "\r\n" {
        content.replace('\n', "\r\n")
    } else {
        content.to_string()
    };

    std::fs::write(resolved, output.as_bytes())
        .map_err(|e| format!("Failed to write file: {}", e))
}

/// Handle single-file edit (original behavior).
fn handle_single_file_edit(
    ctx: &HandlerContext,
    path_str: &str,
    operations: Option<&Vec<Value>>,
    edits: Option<&Vec<Value>>,
    is_regex: bool,
    dry_run: bool,
    expected_line_count: Option<usize>,
) -> ToolCallResult {
    // Read and validate
    let (resolved, normalized, line_ending) = match read_and_validate_file(&ctx.server_dir, path_str) {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };

    // Apply edits
    let edit_result = match apply_edits_to_content(path_str, &normalized, operations, edits, is_regex, expected_line_count) {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };

    // Write file (unless dryRun)
    if !dry_run {
        if let Err(e) = write_file_with_endings(&resolved, &edit_result.modified_content, line_ending) {
            return ToolCallResult::error(e);
        }
    }

    // Build response
    let mut response = json!({
        "path": path_str,
        "applied": edit_result.applied,
        "linesAdded": edit_result.lines_added,
        "linesRemoved": edit_result.lines_removed,
        "newLineCount": edit_result.new_line_count,
        "dryRun": dry_run,
    });

    if edit_result.total_replacements > 0 {
        response["totalReplacements"] = json!(edit_result.total_replacements);
    }

    if !edit_result.skipped_details.is_empty() {
        response["skippedEdits"] = json!(edit_result.skipped_details.len());
        response["skippedDetails"] = json!(edit_result.skipped_details.iter().map(|s| {
            json!({
                "editIndex": s.edit_index,
                "search": s.search_text,
                "reason": s.reason,
            })
        }).collect::<Vec<_>>());
    }

    if !edit_result.diff.is_empty() {
        response["diff"] = json!(edit_result.diff);
    } else {
        response["diff"] = json!("(no changes)");
    }

    ToolCallResult::success(json_to_string(&response))
}

/// Handle multi-file edit with transactional semantics (all-or-nothing).
fn handle_multi_file_edit(
    ctx: &HandlerContext,
    paths_array: &[Value],
    operations: Option<&Vec<Value>>,
    edits: Option<&Vec<Value>>,
    is_regex: bool,
    dry_run: bool,
    expected_line_count: Option<usize>,
) -> ToolCallResult {
    // Validate paths array
    if paths_array.is_empty() {
        return ToolCallResult::error("'paths' array must not be empty.".to_string());
    }
    if paths_array.len() > MAX_MULTI_FILE_PATHS {
        return ToolCallResult::error(format!(
            "'paths' array has {} entries, maximum is {}.",
            paths_array.len(), MAX_MULTI_FILE_PATHS
        ));
    }

    // Parse path strings
    let path_strings: Vec<&str> = match paths_array.iter()
        .enumerate()
        .map(|(i, v)| v.as_str().ok_or_else(|| format!("paths[{}]: expected string", i)))
        .collect::<Result<Vec<&str>, String>>() {
        Ok(ps) => ps,
        Err(e) => return ToolCallResult::error(e),
    };

    // Phase 1: Read all files
    let mut file_data: Vec<(&str, PathBuf, String, &'static str)> = Vec::with_capacity(path_strings.len());
    for path_str in &path_strings {
        match read_and_validate_file(&ctx.server_dir, path_str) {
            Ok((resolved, normalized, line_ending)) => {
                file_data.push((path_str, resolved, normalized, line_ending));
            }
            Err(e) => return ToolCallResult::error(format!("File '{}': {}", path_str, e)),
        }
    }

    // Phase 2: Apply edits to all (in memory)
    let mut edit_results: Vec<(&str, PathBuf, EditResult, &'static str)> = Vec::with_capacity(file_data.len());
    for (path_str, resolved, normalized, line_ending) in file_data {
        match apply_edits_to_content(path_str, &normalized, operations, edits, is_regex, expected_line_count) {
            Ok(result) => {
                edit_results.push((path_str, resolved, result, line_ending));
            }
            Err(e) => return ToolCallResult::error(format!("File '{}': {}", path_str, e)),
        }
    }

    // Phase 3: Write all (only if !dry_run)
    if !dry_run {
        for (path_str, resolved, result, line_ending) in &edit_results {
            if let Err(e) = write_file_with_endings(resolved, &result.modified_content, line_ending) {
                return ToolCallResult::error(format!("File '{}': {}", path_str, e));
            }
        }
    }

    // Phase 4: Build response with per-file results
    let mut total_applied: usize = 0;
    let mut results_array = Vec::new();
    for (path_str, _, result, _) in &edit_results {
        total_applied += result.applied;
        let mut file_result = json!({
            "path": path_str,
            "applied": result.applied,
            "linesAdded": result.lines_added,
            "linesRemoved": result.lines_removed,
            "newLineCount": result.new_line_count,
        });
        if result.total_replacements > 0 {
            file_result["totalReplacements"] = json!(result.total_replacements);
        }
        if !result.skipped_details.is_empty() {
            file_result["skippedEdits"] = json!(result.skipped_details.len());
            file_result["skippedDetails"] = json!(result.skipped_details.iter().map(|s| {
                json!({
                    "editIndex": s.edit_index,
                    "search": s.search_text,
                    "reason": s.reason,
                })
            }).collect::<Vec<_>>());
        }
        if !result.diff.is_empty() {
            file_result["diff"] = json!(result.diff);
        } else {
            file_result["diff"] = json!("(no changes)");
        }
        results_array.push(file_result);
    }

    let response = json!({
        "results": results_array,
        "summary": {
            "filesEdited": edit_results.len(),
            "totalApplied": total_applied,
            "dryRun": dry_run,
        }
    });

    ToolCallResult::success(json_to_string(&response))
}

// ─── Path resolution ─────────────────────────────────────────────────

/// Resolve a file path: if absolute, use as-is; if relative, resolve from server_dir.
fn resolve_path(server_dir: &str, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(server_dir).join(p)
    }
}

// ─── Line ending detection ───────────────────────────────────────────

/// Detect whether the file uses CRLF or LF line endings.
/// Returns "\r\n" for CRLF, "\n" for LF (default).
fn detect_line_ending(content: &str) -> &'static str {
    // Count CRLF vs bare LF
    let crlf_count = content.matches("\r\n").count();
    let lf_count = content.matches('\n').count();
    // bare LF = total LF - CRLF (each \r\n contains one \n)
    let bare_lf_count = lf_count - crlf_count;

    if crlf_count > bare_lf_count {
        "\r\n"
    } else {
        "\n"
    }
}

// ─── Mode A: Line-range operations ───────────────────────────────────

struct LineOperation {
    start_line: usize, // 1-based
    end_line: usize,   // 1-based, inclusive
    content: String,
}

fn parse_line_operations(ops_array: &[Value]) -> Result<Vec<LineOperation>, String> {
    let mut ops = Vec::with_capacity(ops_array.len());
    for (i, op) in ops_array.iter().enumerate() {
        let start_line = op.get("startLine")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("operations[{}]: missing or invalid 'startLine'", i))? as usize;
        let end_line = op.get("endLine")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("operations[{}]: missing or invalid 'endLine'", i))? as usize;
        let content = op.get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("operations[{}]: missing or invalid 'content'", i))?
            .to_string();

        if start_line == 0 {
            return Err(format!("operations[{}]: startLine must be >= 1", i));
        }

        ops.push(LineOperation { start_line, end_line, content });
    }
    Ok(ops)
}

/// Apply line-range operations bottom-up. Returns (new_lines, applied_count).
fn apply_line_operations(lines: &[&str], ops: Vec<LineOperation>) -> Result<(Vec<String>, usize), String> {
    let line_count = lines.len();

    // Validate ranges
    for op in &ops {
        // For insert mode (endLine < startLine), startLine can be line_count + 1 (append after last line)
        if op.end_line >= op.start_line {
            // Replace/delete mode
            if op.start_line > line_count {
                return Err(format!(
                    "startLine {} out of range (file has {} lines)",
                    op.start_line, line_count
                ));
            }
            if op.end_line > line_count {
                return Err(format!(
                    "endLine {} out of range (file has {} lines)",
                    op.end_line, line_count
                ));
            }
        } else {
            // Insert mode: startLine can be 1..=line_count+1
            if op.start_line > line_count + 1 {
                return Err(format!(
                    "startLine {} out of range for insert (file has {} lines, max insert position is {})",
                    op.start_line, line_count, line_count + 1
                ));
            }
        }
    }

    // Sort by startLine descending (bottom-up)
    let mut sorted_ops: Vec<&LineOperation> = ops.iter().collect();
    sorted_ops.sort_by(|a, b| b.start_line.cmp(&a.start_line));

    // Check overlaps (after sorting descending)
    // sorted_ops[0] has highest startLine, sorted_ops[last] has lowest
    for i in 0..sorted_ops.len().saturating_sub(1) {
        let higher = sorted_ops[i];   // higher startLine
        let lower = sorted_ops[i + 1]; // lower startLine

        // Skip overlap check for insert operations (endLine < startLine)
        if higher.end_line < higher.start_line || lower.end_line < lower.start_line {
            continue;
        }

        // Check: the lower operation's endLine must be < higher operation's startLine
        if lower.end_line >= higher.start_line {
            return Err(format!(
                "Operations overlap at lines {}-{}",
                lower.start_line, higher.end_line
            ));
        }
    }

    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    for op in &sorted_ops {
        let start = op.start_line - 1; // 0-based

        if op.end_line < op.start_line {
            // INSERT mode: insert content before startLine
            if op.content.is_empty() {
                continue; // empty insert = no-op
            }
            let new_lines: Vec<String> = op.content.split('\n').map(String::from).collect();
            for (i, line) in new_lines.iter().enumerate() {
                result.insert(start + i, line.clone());
            }
        } else if op.content.is_empty() {
            // DELETE mode: remove lines startLine..=endLine
            let end = op.end_line; // exclusive end for drain = endLine (1-based = 0-based + 1)
            result.drain(start..end);
        } else {
            // REPLACE mode: replace lines startLine..=endLine with content
            let end = op.end_line; // exclusive end for splice
            let new_lines: Vec<String> = op.content.split('\n').map(String::from).collect();
            result.splice(start..end, new_lines);
        }
    }

    Ok((result, ops.len()))
}

// ─── Mode B: Text-match edits ────────────────────────────────────────

/// Represents a single text edit operation.
/// Supports two modes:
/// - Search/replace: find `search` text and replace with `replace`
/// - Insert after/before: find anchor text and insert `content` after/before it
struct TextEdit {
    /// Text to search for (literal or regex). Used in search/replace mode.
    search: Option<String>,
    /// Replacement text. Used in search/replace mode.
    replace: Option<String>,
    /// Which occurrence to target. 0 = all occurrences.
    occurrence: usize,
    /// Anchor text to insert AFTER. Mutually exclusive with search/replace.
    insert_after: Option<String>,
    /// Anchor text to insert BEFORE. Mutually exclusive with search/replace.
    insert_before: Option<String>,
    /// Content to insert (used with insert_after/insert_before).
    content: Option<String>,
    /// Expected context near the search/anchor text (±5 lines). Safety check.
    expected_context: Option<String>,
    /// If true, skip this edit silently when search/anchor text is not found (instead of returning error).
    /// Useful with multi-file `paths` where not all files contain the target text.
    skip_if_not_found: bool,
}

fn parse_text_edits(edits_array: &[Value]) -> Result<Vec<TextEdit>, String> {
    let mut edits = Vec::with_capacity(edits_array.len());
    for (i, edit) in edits_array.iter().enumerate() {
        let search = edit.get("search").and_then(|v| v.as_str()).map(|s| s.to_string());
        let replace = edit.get("replace").and_then(|v| v.as_str()).map(|s| s.to_string());
        let insert_after = edit.get("insertAfter").and_then(|v| v.as_str()).map(|s| s.to_string());
        let insert_before = edit.get("insertBefore").and_then(|v| v.as_str()).map(|s| s.to_string());
        let content = edit.get("content").and_then(|v| v.as_str()).map(|s| s.to_string());
        let occurrence = edit.get("occurrence")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let expected_context = edit.get("expectedContext").and_then(|v| v.as_str()).map(|s| s.to_string());
        let skip_if_not_found = edit.get("skipIfNotFound").and_then(|v| v.as_bool()).unwrap_or(false);

        let has_search_replace = search.is_some() || replace.is_some();
        let has_insert = insert_after.is_some() || insert_before.is_some();

        // Validate mutual exclusivity
        if has_search_replace && has_insert {
            return Err(format!(
                "edits[{}]: 'search'/'replace' and 'insertAfter'/'insertBefore' are mutually exclusive",
                i
            ));
        }

        if has_insert {
            // Insert mode validation
            if insert_after.is_some() && insert_before.is_some() {
                return Err(format!(
                    "edits[{}]: 'insertAfter' and 'insertBefore' are mutually exclusive",
                    i
                ));
            }
            if content.is_none() {
                return Err(format!(
                    "edits[{}]: 'content' is required when using 'insertAfter' or 'insertBefore'",
                    i
                ));
            }
            let anchor = insert_after.as_deref().or(insert_before.as_deref()).unwrap();
            if anchor.is_empty() {
                return Err(format!(
                    "edits[{}]: anchor text must not be empty",
                    i
                ));
            }
        } else {
            // Search/replace mode validation
            let search_str = search.as_deref()
                .ok_or_else(|| format!("edits[{}]: missing or invalid 'search'", i))?;
            if replace.is_none() {
                return Err(format!("edits[{}]: missing or invalid 'replace'", i));
            }
            if search_str.is_empty() {
                return Err(format!("edits[{}]: 'search' must not be empty", i));
            }
        }

        edits.push(TextEdit {
            search,
            replace,
            occurrence,
            insert_after,
            insert_before,
            content,
            expected_context,
            skip_if_not_found,
        });
    }
    Ok(edits)
}

/// Suffix added to occurrence errors when the edit is not the first in the batch.
/// Explains that previous edits may have changed the content, reducing occurrence counts.
const SEQUENTIAL_EDIT_HINT: &str = ". Note: edits are applied sequentially — previous edits in the same request may have modified the content, reducing the occurrence count";

/// Apply text edits sequentially. Returns (new_content, total_replacements, skipped_details).
fn apply_text_edits(content: &str, edits: &[TextEdit], is_regex: bool) -> Result<(String, usize, Vec<SkippedEditDetail>), String> {
    let mut result = content.to_string();
    let mut total_replacements = 0;
    let mut skipped_details: Vec<SkippedEditDetail> = Vec::new();

    for (edit_index, edit) in edits.iter().enumerate() {
        if edit.insert_after.is_some() || edit.insert_before.is_some() {
            // Insert after/before mode
            let anchor = edit.insert_after.as_deref()
                .or(edit.insert_before.as_deref())
                .unwrap();
            let insert_content = edit.content.as_deref().unwrap(); // validated in parse
            let is_after = edit.insert_after.is_some();

            // Find the anchor text
            let matches: Vec<usize> = find_all_occurrences(&result, anchor);
            if matches.is_empty() {
                if edit.skip_if_not_found {
                    skipped_details.push(SkippedEditDetail {
                        edit_index,
                        search_text: truncate_for_display(anchor),
                        reason: "anchor text not found".to_string(),
                    });
                    continue;
                }
                let hint = nearest_match_hint(&result, anchor);
                return Err(format!("Anchor text not found: \"{}\"{}", truncate_for_display(anchor), hint));
            }

            // Determine which occurrence to use
            let target_pos = match edit.occurrence {
                0 => {
                    // Default: use first occurrence for insert
                    matches[0]
                }
                n => {
                    if n > matches.len() {
                        let hint = if edit_index > 0 { SEQUENTIAL_EDIT_HINT } else { "" };
                        return Err(format!(
                            "Occurrence {} requested but anchor \"{}\" found only {} time(s){}",
                            n, anchor, matches.len(), hint
                        ));
                    }
                    matches[n - 1]
                }
            };

            // Check expectedContext if present
            if let Some(ref ctx_text) = edit.expected_context {
                check_expected_context(&result, target_pos, anchor.len(), ctx_text)?;
            }

            // Find the line containing the anchor
            let anchor_end = target_pos + anchor.len();

            if is_after {
                // Insert after: find end of the line containing the anchor, insert on next line
                let line_end = result[anchor_end..].find('\n')
                    .map(|p| anchor_end + p)
                    .unwrap_or(result.len());
                let insert_text = format!("\n{}", insert_content);
                result.insert_str(line_end, &insert_text);
            } else {
                // Insert before: find start of the line containing the anchor, insert before it
                let line_start = result[..target_pos].rfind('\n')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let insert_text = format!("{}\n", insert_content);
                result.insert_str(line_start, &insert_text);
            }

            total_replacements += 1;
        } else {
            // Search/replace mode
            let search = edit.search.as_deref().unwrap();
            let replace = edit.replace.as_deref().unwrap();

            if is_regex {
                let re = Regex::new(search)
                    .map_err(|e| format!("Invalid regex '{}': {}", search, e))?;
                let count = re.find_iter(&result).count();
                if count == 0 {
                    if edit.skip_if_not_found {
                        skipped_details.push(SkippedEditDetail {
                            edit_index,
                            search_text: truncate_for_display(search),
                            reason: "regex pattern not found".to_string(),
                        });
                        continue;
                    }
                    let hint = nearest_match_hint(&result, search);
                    return Err(format!("Pattern not found: \"{}\"{}", truncate_for_display(search), hint));
                }

                // Check expectedContext on first match
                if let Some(ref ctx_text) = edit.expected_context {
                    if let Some(m) = re.find(&result) {
                        check_expected_context(&result, m.start(), m.len(), ctx_text)?;
                    }
                }

                match edit.occurrence {
                    0 => {
                        result = re.replace_all(&result, replace).to_string();
                        total_replacements += count;
                    }
                    n => {
                        if n > count {
                            let hint = if edit_index > 0 { SEQUENTIAL_EDIT_HINT } else { "" };
                            return Err(format!(
                                "Occurrence {} requested but pattern \"{}\" found only {} time(s){}",
                                n, search, count, hint
                            ));
                        }
                        let mut current = 0usize;
                        let replace_str = replace.to_string();
                        result = re.replace_all(&result, |caps: &regex::Captures| {
                            current += 1;
                            if current == n {
                                // Apply capture group substitution
                                let mut out = replace_str.clone();
                                for i in 0..caps.len() {
                                    if let Some(m) = caps.get(i) {
                                        out = out.replace(&format!("${}", i), m.as_str());
                                    }
                                }
                                out
                            } else {
                                caps[0].to_string()
                            }
                        }).to_string();
                        total_replacements += 1;
                    }
                }
            } else {
                // Literal search
                let count = result.matches(search).count();
                if count == 0 {
                    if edit.skip_if_not_found {
                        skipped_details.push(SkippedEditDetail {
                            edit_index,
                            search_text: truncate_for_display(search),
                            reason: "text not found".to_string(),
                        });
                        continue;
                    }
                    let hint = nearest_match_hint(&result, search);
                    return Err(format!("Text not found: \"{}\"{}", truncate_for_display(search), hint));
                }

                // Check expectedContext on first match
                if let Some(ref ctx_text) = edit.expected_context {
                    if let Some(pos) = result.find(search) {
                        check_expected_context(&result, pos, search.len(), ctx_text)?;
                    }
                }

                match edit.occurrence {
                    0 => {
                        result = result.replace(search, replace);
                        total_replacements += count;
                    }
                    n => {
                        if n > count {
                            let hint = if edit_index > 0 { SEQUENTIAL_EDIT_HINT } else { "" };
                            return Err(format!(
                                "Occurrence {} requested but text \"{}\" found only {} time(s){}",
                                n, search, count, hint
                            ));
                        }
                        let mut current = 0usize;
                        let mut new_result = String::new();
                        let mut remaining = result.as_str();
                        while let Some(pos) = remaining.find(search) {
                            current += 1;
                            new_result.push_str(&remaining[..pos]);
                            if current == n {
                                new_result.push_str(replace);
                            } else {
                                new_result.push_str(search);
                            }
                            remaining = &remaining[pos + search.len()..];
                        }
                        new_result.push_str(remaining);
                        result = new_result;
                        total_replacements += 1;
                    }
                }
            }
        }
    }

    Ok((result, total_replacements, skipped_details))
}

/// Find all occurrences of a literal string, returning their start positions.
fn find_all_occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        positions.push(start + pos);
        start += pos + needle.len();
    }
    positions
}

/// Check that expectedContext text exists within ±5 lines of the match position.
fn check_expected_context(content: &str, match_pos: usize, _match_len: usize, expected: &str) -> Result<(), String> {
    // Find line number of the match
    let match_line = content[..match_pos].matches('\n').count();

    // Collect all lines
    let lines: Vec<&str> = content.split('\n').collect();

    // Define context window: ±5 lines around the match
    let start_line = match_line.saturating_sub(5);
    let end_line = (match_line + 5).min(lines.len().saturating_sub(1));

    // Build context string from the window
    let context_window: String = lines[start_line..=end_line].join("\n");

    if !context_window.contains(expected) {
        return Err(format!(
            "Expected context \"{}\" not found near match at line {} (checked lines {}-{})",
            expected, match_line + 1, start_line + 1, end_line + 1
        ));
    }

    Ok(())
}

// ─── Nearest match hint ──────────────────────────────────────────────

/// Truncate a string for display in error messages.
fn truncate_for_display(s: &str) -> String {
    if s.len() <= NEAREST_MATCH_MAX_DISPLAY_LEN {
        s.to_string()
    } else {
        format!("{}…", &s[..s.floor_char_boundary(NEAREST_MATCH_MAX_DISPLAY_LEN)])
    }
}

/// Find the nearest matching line/window in `content` for the given `search_text`.
/// Returns a hint string to append to the error message, or empty string if no good match.
///
/// Algorithm:
/// - For single-line search: compare each line with char-level similarity
/// - For multi-line search: use sliding window of N lines, join and compare
/// - Uses `similar::TextDiff::ratio()` for similarity scoring
/// - Skips files > 500KB for performance
fn nearest_match_hint(content: &str, search_text: &str) -> String {
    // Skip for large files
    if content.len() > NEAREST_MATCH_MAX_FILE_SIZE {
        return String::new();
    }

    let search_lines: Vec<&str> = search_text.split('\n').collect();
    let search_line_count = search_lines.len();
    let content_lines: Vec<&str> = content.split('\n').collect();

    if content_lines.is_empty() || search_text.is_empty() {
        return String::new();
    }

    let mut best_similarity: f32 = 0.0;
    let mut best_line_num: usize = 0; // 1-based
    let mut best_text = String::new();

    if search_line_count <= 1 {
        // Single-line search: compare against each line
        for (i, line) in content_lines.iter().enumerate() {
            let ratio = similar::TextDiff::from_chars(search_text, line).ratio();
            if ratio > best_similarity {
                best_similarity = ratio;
                best_line_num = i + 1;
                best_text = line.to_string();
            }
        }
    } else {
        // Multi-line search: sliding window of search_line_count lines
        if content_lines.len() >= search_line_count {
            for i in 0..=(content_lines.len() - search_line_count) {
                let window = content_lines[i..i + search_line_count].join("\n");
                let ratio = similar::TextDiff::from_chars(search_text, &window).ratio();
                if ratio > best_similarity {
                    best_similarity = ratio;
                    best_line_num = i + 1;
                    best_text = window;
                }
            }
        }
    }

    if best_similarity < NEAREST_MATCH_MIN_SIMILARITY as f32 {
        return String::new();
    }

    let pct = (best_similarity * 100.0).round() as u32;
    let display_text = truncate_for_display(&best_text);
    format!(
        ". Nearest match at line {} (similarity {}%): \"{}\"",
        best_line_num, pct, display_text
    )
}

// ─── Diff generation ─────────────────────────────────────────────────

/// Generate a unified diff between original and modified content.
fn generate_unified_diff(path: &str, original: &str, modified: &str) -> String {
    if original == modified {
        return String::new();
    }

    similar::TextDiff::from_lines(original, modified)
        .unified_diff()
        .header(&format!("a/{}", path), &format!("b/{}", path))
        .to_string()
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;