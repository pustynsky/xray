//! MCP tool handler for `xray_edit` — reliable file editing with two modes:
//! - Mode A (operations): line-range splice, applied bottom-up to avoid offset cascade
//! - Mode B (edits): text find-replace, literal or regex, with insert after/before support

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use super::utils::json_to_string;
use super::HandlerContext;

/// Edit mode: either line-range operations (Mode A) or text-match edits (Mode B).
/// Using an enum makes invalid states (both None or both Some) unrepresentable at the type level.
enum EditMode<'a> {
    /// Mode A: line-range splice operations, applied bottom-up.
    Operations(&'a [Value]),
    /// Mode B: text find-replace or insert after/before, applied sequentially.
    Edits(&'a [Value]),
}

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

/// Handle `xray_edit` tool call.
pub(crate) fn handle_xray_edit(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
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

    // ── Validate mode and construct EditMode ──
    let mode = match (operations, edits) {
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
        (Some(ops), None) => EditMode::Operations(ops),
        (None, Some(eds)) => EditMode::Edits(eds),
    };

    // ── Dispatch single vs multi-file ──
    if let Some(paths_array) = multi_paths {
        handle_multi_file_edit(ctx, paths_array, &mode, is_regex, dry_run, expected_line_count)
    } else {
        let path_str = single_path.unwrap(); // validated above
        handle_single_file_edit(ctx, path_str, &mode, is_regex, dry_run, expected_line_count)
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
    warnings: Vec<String>,
    diff: String,
    lines_added: i64,
    lines_removed: i64,
    new_line_count: usize,
}

/// Read and validate a file, returning its content and line ending style.
fn read_and_validate_file(server_dir: &str, path_str: &str) -> Result<(PathBuf, String, &'static str), String> {
    let resolved = resolve_path(server_dir, path_str);
    if !resolved.exists() {
        // File doesn't exist — treat as empty (allows creation via insert operations)
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directories for '{}': {}", path_str, e))?;
        }
        return Ok((resolved, String::new(), "\n"));
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
    mode: &EditMode<'_>,
    is_regex: bool,
    expected_line_count: Option<usize>,
) -> Result<EditResult, String> {
    let (modified_content, applied, total_replacements, skipped_details, warnings) = match mode {
        EditMode::Operations(ops_array) => {
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
            (new_lines.join("\n"), applied_count, 0, Vec::new(), Vec::new())
        }
        EditMode::Edits(edits_array) => {
            // Mode B: Text-match edits
            let text_edits = parse_text_edits(edits_array)?;

            let (new_content, replacements, skipped, edit_warnings) = apply_text_edits(normalized, &text_edits, is_regex)?;
            let edit_count = text_edits.len();
            (new_content, edit_count, replacements, skipped, edit_warnings)
        }
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
        warnings,
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

/// Generate a temp file path in the same directory as `target` for atomic writes.
fn temp_path_for(target: &Path) -> PathBuf {
    let file_name = target.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    target.with_file_name(format!(".{}.xray_tmp", file_name))
}

/// Rename `src` to `dst`, replacing `dst` if it exists.
/// On Windows, std::fs::rename may fail if `dst` exists, so we remove first.
fn rename_replace(src: &Path, dst: &Path) -> Result<(), String> {
    // Try direct rename first (works on most platforms)
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Fallback: remove target first, then rename
            if dst.exists() {
                std::fs::remove_file(dst)
                    .map_err(|e2| format!("Cannot remove original '{}': {}", dst.display(), e2))?;
                std::fs::rename(src, dst)
                    .map_err(|e2| format!("Cannot rename temp to '{}': {} (original error: {})", dst.display(), e2, e))
            } else {
                Err(format!("Cannot rename temp to '{}': {}", dst.display(), e))
            }
        }
    }
}

/// Handle single-file edit (original behavior).
fn handle_single_file_edit(
    ctx: &HandlerContext,
    path_str: &str,
    mode: &EditMode<'_>,
    is_regex: bool,
    dry_run: bool,
    expected_line_count: Option<usize>,
) -> ToolCallResult {
    // Read and validate
    let (resolved, normalized, line_ending) = match read_and_validate_file(&ctx.server_dir(), path_str) {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };
    let file_created = normalized.is_empty() && !resolved.exists();

    // Apply edits
    let edit_result = match apply_edits_to_content(path_str, &normalized, mode, is_regex, expected_line_count) {
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

    if !edit_result.warnings.is_empty() {
        response["warnings"] = json!(edit_result.warnings);
    }

    if !edit_result.diff.is_empty() {
        response["diff"] = json!(edit_result.diff);
    } else {
        response["diff"] = json!("(no changes)");
    }

    if file_created {
        response["fileCreated"] = json!(true);
    }

    ToolCallResult::success(json_to_string(&response))
}

/// Handle multi-file edit with transactional semantics (all-or-nothing).
fn handle_multi_file_edit(
    ctx: &HandlerContext,
    paths_array: &[Value],
    mode: &EditMode<'_>,
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

    // Phase 1: Read all files (with duplicate path detection)
    let mut file_data: Vec<(&str, PathBuf, String, &'static str)> = Vec::with_capacity(path_strings.len());
    let mut seen_paths: HashSet<PathBuf> = HashSet::with_capacity(path_strings.len());
    for path_str in &path_strings {
        match read_and_validate_file(&ctx.server_dir(), path_str) {
            Ok((resolved, normalized, line_ending)) => {
                // Normalize path to handle ./file.txt vs file.txt
                let normalized_path: PathBuf = resolved.components().collect();
                if !seen_paths.insert(normalized_path.clone()) {
                    // Find the original path string that resolved to the same file
                    let original = file_data.iter()
                        .find(|(_, r, _, _)| {
                            let nr: PathBuf = r.components().collect();
                            nr == normalized_path
                        })
                        .map(|(p, _, _, _)| *p)
                        .unwrap_or("?");
                    return ToolCallResult::error(format!(
                        "Duplicate path: '{}' and '{}' resolve to the same file",
                        original, path_str
                    ));
                }
                file_data.push((path_str, resolved, normalized, line_ending));
            }
            Err(e) => return ToolCallResult::error(format!("File '{}': {}", path_str, e)),
        }
    }

    // Phase 2: Apply edits to all (in memory)
    let mut edit_results: Vec<(&str, PathBuf, EditResult, &'static str)> = Vec::with_capacity(file_data.len());
    for (path_str, resolved, normalized, line_ending) in file_data {
        match apply_edits_to_content(path_str, &normalized, mode, is_regex, expected_line_count) {
            Ok(result) => {
                edit_results.push((path_str, resolved, result, line_ending));
            }
            Err(e) => return ToolCallResult::error(format!("File '{}': {}", path_str, e)),
        }
    }

    // Phase 3: Write all (only if !dry_run) — atomic multi-file via temp+rename
    if !dry_run {
        // Phase 3a: Write to temp files (validates I/O before touching originals)
        let mut temp_files: Vec<(&str, PathBuf, PathBuf)> = Vec::with_capacity(edit_results.len());
        for (path_str, resolved, result, line_ending) in &edit_results {
            let temp = temp_path_for(resolved);
            if let Err(e) = write_file_with_endings(&temp, &result.modified_content, line_ending) {
                // Clean up temp files already written
                for (_, _, tp) in &temp_files {
                    let _ = std::fs::remove_file(tp);
                }
                return ToolCallResult::error(format!("File '{}': {}", path_str, e));
            }
            temp_files.push((path_str, resolved.clone(), temp));
        }

        // Phase 3b: Rename temp files to targets (fast, unlikely to fail)
        let mut renamed: usize = 0;
        for (path_str, resolved, temp) in &temp_files {
            if let Err(e) = rename_replace(temp, resolved) {
                // Best-effort cleanup of remaining temp files
                for (_, _, tp) in &temp_files[renamed..] {
                    let _ = std::fs::remove_file(tp);
                }
                return ToolCallResult::error(format!(
                    "File '{}': rename failed after {} of {} files committed: {}. \
                     Already-committed files cannot be rolled back.",
                    path_str, renamed, temp_files.len(), e
                ));
            }
            renamed += 1;
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
        if !result.warnings.is_empty() {
            file_result["warnings"] = json!(result.warnings);
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
        let content = normalize_crlf(&content);

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

/// Normalize CRLF line endings to LF in a string.
/// This ensures search text from JSON input matches LF-normalized file content.
fn normalize_crlf(s: &str) -> String {
    if s.contains("\r\n") {
        s.replace("\r\n", "\n")
    } else {
        s.to_string()
    }
}

/// Strip trailing whitespace from each line of a string.
/// Used for fuzzy-retry when exact match fails due to invisible trailing spaces.
fn strip_trailing_whitespace_per_line(s: &str) -> String {
    s.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip leading and trailing blank lines from a string.
/// Interior blank lines and content are preserved.
fn trim_blank_lines(s: &str) -> String {
    s.trim_matches('\n').to_string()
}

/// Collapse runs of horizontal whitespace (spaces/tabs) to a single space per line.
/// Used for flexible whitespace comparison in expectedContext checks.
fn collapse_spaces(s: &str) -> String {
    s.lines()
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert a literal search string to a flex-space regex pattern.
/// Each whitespace gap between non-whitespace tokens becomes `[ \t]+`,
/// and leading/trailing whitespace per line becomes `[ \t]*`.
/// This allows matching text with different amounts of horizontal whitespace.
///
/// Example: `"| Issue | Count |"` becomes a pattern matching `"| Issue       | Count     |"`
fn search_to_flex_pattern(search: &str) -> Option<String> {
    let lines: Vec<&str> = search.split('\n').collect();
    let mut pattern_parts: Vec<String> = Vec::new();
    let mut has_content = false;

    for line in &lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            // Empty line: match zero or more horizontal whitespace
            pattern_parts.push("[ \\t]*".to_string());
        } else {
            has_content = true;
            let escaped_parts: Vec<String> = parts.iter()
                .map(|p| regex::escape(p))
                .collect();
            let flexed = escaped_parts.join("[ \\t]+");
            pattern_parts.push(format!("[ \\t]*{}[ \\t]*", flexed));
        }
    }

    if !has_content {
        return None; // All-whitespace search — don't flex-match
    }

    Some(pattern_parts.join("\n"))
}

/// Describe a byte for diagnostic messages (hex + human-readable name).
fn describe_byte(b: u8) -> String {
    match b {
        b' ' => format!("0x{:02X} (space)", b),
        b'\t' => format!("0x{:02X} (tab)", b),
        b'\n' => format!("0x{:02X} (newline)", b),
        b'\r' => format!("0x{:02X} (carriage return)", b),
        0xC2 => format!("0x{:02X} (possible non-breaking space start)", b),
        b if b.is_ascii_graphic() => format!("0x{:02X} ('{}')", b, b as char),
        b => format!("0x{:02X}", b),
    }
}

fn parse_text_edits(edits_array: &[Value]) -> Result<Vec<TextEdit>, String> {
    let mut edits = Vec::with_capacity(edits_array.len());
    for (i, edit) in edits_array.iter().enumerate() {
        // Part A: Normalize CRLF in all text fields to match LF-normalized file content
        let search = edit.get("search").and_then(|v| v.as_str()).map(normalize_crlf);
        let replace = edit.get("replace").and_then(|v| v.as_str()).map(normalize_crlf);
        let insert_after = edit.get("insertAfter").and_then(|v| v.as_str()).map(normalize_crlf);
        let insert_before = edit.get("insertBefore").and_then(|v| v.as_str()).map(normalize_crlf);
        let content = edit.get("content").and_then(|v| v.as_str()).map(normalize_crlf);
        let occurrence = edit.get("occurrence")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let expected_context = edit.get("expectedContext").and_then(|v| v.as_str()).map(normalize_crlf);
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

/// Apply text edits sequentially. Returns (new_content, total_replacements, skipped_details, warnings).
fn apply_text_edits(content: &str, edits: &[TextEdit], is_regex: bool) -> Result<(String, usize, Vec<SkippedEditDetail>, Vec<String>), String> {
    let mut result = content.to_string();
    let mut total_replacements = 0;
    let mut skipped_details: Vec<SkippedEditDetail> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (edit_index, edit) in edits.iter().enumerate() {
        if edit.insert_after.is_some() || edit.insert_before.is_some() {
            // Insert after/before mode
            let anchor = edit.insert_after.as_deref()
                .or(edit.insert_before.as_deref())
                .unwrap();
            let insert_content = edit.content.as_deref().unwrap(); // validated in parse
            let is_after = edit.insert_after.is_some();

            // Find the anchor text (with auto-retry cascade)
            // Step 1: Exact match
            let mut matches = find_all_occurrences(&result, anchor);
            let mut actual_match_len = anchor.len();
            let mut effective_anchor_owned: Option<String> = None;
            let mut flex_match_lens: Option<Vec<usize>> = None;

            // Step 2: Strip trailing whitespace
            if matches.is_empty() {
                let trimmed = strip_trailing_whitespace_per_line(anchor);
                if trimmed != anchor && !trimmed.is_empty() {
                    let m = find_all_occurrences(&result, &trimmed);
                    if !m.is_empty() {
                        warnings.push(format!(
                            "edits[{}]: anchor matched after trimming trailing whitespace",
                            edit_index
                        ));
                        actual_match_len = trimmed.len();
                        effective_anchor_owned = Some(trimmed);
                        matches = m;
                    }
                }
            }

            // Step 3: Trim leading/trailing blank lines (+ strip trailing WS)
            if matches.is_empty() {
                let line_trimmed = strip_trailing_whitespace_per_line(&trim_blank_lines(anchor));
                if line_trimmed != anchor && !line_trimmed.is_empty() {
                    let m = find_all_occurrences(&result, &line_trimmed);
                    if !m.is_empty() {
                        warnings.push(format!(
                            "edits[{}]: anchor matched after trimming leading/trailing blank lines",
                            edit_index
                        ));
                        actual_match_len = line_trimmed.len();
                        effective_anchor_owned = Some(line_trimmed);
                        matches = m;
                    }
                }
            }

            // Step 4: Flex-space matching (collapse whitespace to regex)
            if matches.is_empty() {
                if let Some(pattern) = search_to_flex_pattern(anchor) {
                    if let Ok(re) = Regex::new(&pattern) {
                        let flex_results: Vec<(usize, usize)> = re.find_iter(&result)
                            .map(|m| (m.start(), m.end() - m.start()))
                            .collect();
                        if !flex_results.is_empty() {
                            warnings.push(format!(
                                "edits[{}]: anchor matched with flexible whitespace (spaces collapsed)",
                                edit_index
                            ));
                            matches = flex_results.iter().map(|&(s, _)| s).collect();
                            flex_match_lens = Some(flex_results.iter().map(|&(_, l)| l).collect());
                        }
                    }
                }
            }

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

            let effective_anchor = effective_anchor_owned.as_deref().unwrap_or(anchor);

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
                            n, effective_anchor, matches.len(), hint
                        ));
                    }
                    matches[n - 1]
                }
            };

            // Compute actual match length for this occurrence (may differ with flex-space)
            let selected_idx = match edit.occurrence { 0 => 0, n => n - 1 };
            let selected_match_len = if let Some(ref lens) = flex_match_lens {
                lens[selected_idx]
            } else {
                actual_match_len
            };

            // Check expectedContext if present
            if let Some(ref ctx_text) = edit.expected_context {
                check_expected_context(&result, target_pos, selected_match_len, ctx_text)?;
            }

            // Find the line containing the anchor
            let anchor_end = target_pos + selected_match_len;

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
                                // Use caps.expand() to avoid cascade bug where
                                // $0 expansion containing "$1" gets double-substituted
                                let mut out = String::new();
                                caps.expand(&replace_str, &mut out);
                                out
                            } else {
                                caps[0].to_string()
                            }
                        }).to_string();
                        total_replacements += 1;
                    }
                }
            } else {
                // Literal search (with auto-retry cascade)
                // Step 1: Exact match
                let mut effective_count = result.matches(search).count();
                let mut effective_search_owned: Option<String> = None;
                let mut flex_re: Option<Regex> = None;

                // Step 2: Strip trailing whitespace
                if effective_count == 0 {
                    let trimmed = strip_trailing_whitespace_per_line(search);
                    if trimmed != search && !trimmed.is_empty() {
                        let tc = result.matches(trimmed.as_str()).count();
                        if tc > 0 {
                            warnings.push(format!(
                                "edits[{}]: text matched after trimming trailing whitespace",
                                edit_index
                            ));
                            effective_search_owned = Some(trimmed);
                            effective_count = tc;
                        }
                    }
                }

                // Step 3: Trim leading/trailing blank lines (+ strip trailing WS)
                if effective_count == 0 {
                    let line_trimmed = strip_trailing_whitespace_per_line(&trim_blank_lines(search));
                    if line_trimmed != search && !line_trimmed.is_empty() {
                        let tc = result.matches(line_trimmed.as_str()).count();
                        if tc > 0 {
                            warnings.push(format!(
                                "edits[{}]: text matched after trimming leading/trailing blank lines",
                                edit_index
                            ));
                            effective_search_owned = Some(line_trimmed);
                            effective_count = tc;
                        }
                    }
                }

                // Step 4: Flex-space matching (collapse whitespace to regex)
                if effective_count == 0 {
                    if let Some(pattern) = search_to_flex_pattern(search) {
                        if let Ok(re) = Regex::new(&pattern) {
                            let fc = re.find_iter(&result).count();
                            if fc > 0 {
                                warnings.push(format!(
                                    "edits[{}]: text matched with flexible whitespace (spaces collapsed)",
                                    edit_index
                                ));
                                effective_count = fc;
                                flex_re = Some(re);
                            }
                        }
                    }
                }

                if effective_count == 0 {
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

                let effective_search = effective_search_owned.as_deref().unwrap_or(search);

                // Check expectedContext on first match
                if let Some(ref ctx_text) = edit.expected_context {
                    if let Some(ref re) = flex_re {
                        if let Some(m) = re.find(&result) {
                            check_expected_context(&result, m.start(), m.len(), ctx_text)?;
                        }
                    } else if let Some(pos) = result.find(effective_search) {
                        check_expected_context(&result, pos, effective_search.len(), ctx_text)?;
                    }
                }

                // Apply replacement
                if let Some(ref re) = flex_re {
                    // Flex-space: use regex replacement with NoExpand (literal replacement)
                    match edit.occurrence {
                        0 => {
                            result = re.replace_all(&result, regex::NoExpand(replace)).to_string();
                            total_replacements += effective_count;
                        }
                        n => {
                            if n > effective_count {
                                let hint = if edit_index > 0 { SEQUENTIAL_EDIT_HINT } else { "" };
                                return Err(format!(
                                    "Occurrence {} requested but text \"{}\" found only {} time(s){}",
                                    n, search, effective_count, hint
                                ));
                            }
                            let mut current = 0usize;
                            let replace_owned = replace.to_string();
                            result = re.replace_all(&result, |caps: &regex::Captures| {
                                current += 1;
                                if current == n {
                                    replace_owned.clone()
                                } else {
                                    caps[0].to_string()
                                }
                            }).to_string();
                            total_replacements += 1;
                        }
                    }
                } else {
                    // Literal replacement (steps 1-3)
                    match edit.occurrence {
                        0 => {
                            result = result.replace(effective_search, replace);
                            total_replacements += effective_count;
                        }
                        n => {
                            if n > effective_count {
                                let hint = if edit_index > 0 { SEQUENTIAL_EDIT_HINT } else { "" };
                                return Err(format!(
                                    "Occurrence {} requested but text \"{}\" found only {} time(s){}",
                                    n, effective_search, effective_count, hint
                                ));
                            }
                            let mut current = 0usize;
                            let mut new_result = String::new();
                            let mut remaining = result.as_str();
                            while let Some(pos) = remaining.find(effective_search) {
                                current += 1;
                                new_result.push_str(&remaining[..pos]);
                                if current == n {
                                    new_result.push_str(replace);
                                } else {
                                    new_result.push_str(effective_search);
                                }
                                remaining = &remaining[pos + effective_search.len()..];
                            }
                            new_result.push_str(remaining);
                            result = new_result;
                            total_replacements += 1;
                        }
                    }
                }
            }
        }
    }

    Ok((result, total_replacements, skipped_details, warnings))
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
        // Flex-space fallback: collapse whitespace in both and retry
        let collapsed_window = collapse_spaces(&context_window);
        let collapsed_expected = collapse_spaces(expected);
        if !collapsed_window.contains(&collapsed_expected) {
            return Err(format!(
                "Expected context \"{}\" not found near match at line {} (checked lines {}-{})",
                expected, match_line + 1, start_line + 1, end_line + 1
            ));
        }
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

    // Part C: When similarity is very high (≥99%), add byte-level diff diagnostic
    let byte_diff_hint = if best_similarity >= 0.99 {
        byte_level_diff_hint(search_text, &best_text)
    } else {
        String::new()
    };

    format!(
        ". Nearest match at line {} (similarity {}%): \"{}\"{}",
        best_line_num, pct, display_text, byte_diff_hint
    )
}

/// Generate a byte-level diff hint showing the first difference between two strings.
/// Used when similarity is ≥99% to help identify invisible whitespace differences.
fn byte_level_diff_hint(search: &str, found: &str) -> String {
    let search_bytes = search.as_bytes();
    let found_bytes = found.as_bytes();

    // Find first different byte
    for (i, (s, f)) in search_bytes.iter().zip(found_bytes.iter()).enumerate() {
        if s != f {
            return format!(
                ". First difference at byte {}: search has {}, file has {}",
                i, describe_byte(*s), describe_byte(*f)
            );
        }
    }

    // If one is a prefix of the other
    if search_bytes.len() != found_bytes.len() {
        let shorter = search_bytes.len().min(found_bytes.len());
        if search_bytes.len() > found_bytes.len() {
            let extra_byte = search_bytes[shorter];
            return format!(
                ". Search text is {} byte(s) longer than file text. Extra content starts with {}",
                search_bytes.len() - found_bytes.len(), describe_byte(extra_byte)
            );
        } else {
            let extra_byte = found_bytes[shorter];
            return format!(
                ". File text is {} byte(s) longer than search text. Extra content starts with {}",
                found_bytes.len() - search_bytes.len(), describe_byte(extra_byte)
            );
        }
    }

    // Identical bytes — shouldn't happen if we got here, but be safe
    String::new()
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