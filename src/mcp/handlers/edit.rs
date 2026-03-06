//! MCP tool handler for `search_edit` — reliable file editing with two modes:
//! - Mode A (operations): line-range splice, applied bottom-up to avoid offset cascade
//! - Mode B (edits): text find-replace, literal or regex

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;
use super::utils::json_to_string;
use super::HandlerContext;

/// Handle `search_edit` tool call.
pub(crate) fn handle_search_edit(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    // ── Parse arguments ──
    let path_str = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ToolCallResult::error("Missing required parameter: 'path'".to_string()),
    };

    let operations = args.get("operations").and_then(|v| v.as_array());
    let edits = args.get("edits").and_then(|v| v.as_array());
    let is_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let dry_run = args.get("dryRun").and_then(|v| v.as_bool()).unwrap_or(false);
    let expected_line_count = args.get("expectedLineCount").and_then(|v| v.as_u64()).map(|v| v as usize);

    // ── Validate mode ──
    match (operations, edits) {
        (None, None) => {
            return ToolCallResult::error("Specify 'operations' (line-range) or 'edits' (text-match), not neither.".to_string());
        }
        (Some(_), Some(_)) => {
            return ToolCallResult::error("Specify 'operations' or 'edits', not both.".to_string());
        }
        _ => {}
    }

    // ── Resolve path ──
    let resolved = resolve_path(&ctx.server_dir, path_str);
    if !resolved.exists() {
        return ToolCallResult::error(format!("File not found: {}", path_str));
    }
    if resolved.is_dir() {
        return ToolCallResult::error(format!("Path is a directory, not a file: {}", path_str));
    }

    // ── Read file ──
    let raw_bytes = match std::fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => return ToolCallResult::error(format!("Failed to read file: {}", e)),
    };

    // Binary detection: check for null bytes in first 8KB
    let check_len = raw_bytes.len().min(8192);
    if raw_bytes[..check_len].contains(&0) {
        return ToolCallResult::error("Binary file detected, not editable.".to_string());
    }

    let content = match std::str::from_utf8(&raw_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(&raw_bytes).into_owned(),
    };

    // ── Detect line ending style ──
    let line_ending = detect_line_ending(&content);

    // ── Normalize to LF for processing ──
    let normalized = if line_ending == "\r\n" {
        content.replace("\r\n", "\n")
    } else {
        content
    };

    // ── Dispatch to mode ──
    let (modified_content, applied, total_replacements) = if let Some(ops_array) = operations {
        // Mode A: Line-range operations
        let ops = match parse_line_operations(ops_array) {
            Ok(ops) => ops,
            Err(e) => return ToolCallResult::error(e),
        };

        let lines: Vec<&str> = normalized.split('\n').collect();

        // expectedLineCount check
        if let Some(expected) = expected_line_count {
            if lines.len() != expected {
                return ToolCallResult::error(format!(
                    "Expected {} lines, file has {}. File may have changed.",
                    expected, lines.len()
                ));
            }
        }

        match apply_line_operations(&lines, ops) {
            Ok(new_lines) => {
                let applied_count = new_lines.1;
                (new_lines.0.join("\n"), applied_count, 0)
            }
            Err(e) => return ToolCallResult::error(e),
        }
    } else if let Some(edits_array) = edits {
        // Mode B: Text-match edits
        let text_edits = match parse_text_edits(edits_array) {
            Ok(edits) => edits,
            Err(e) => return ToolCallResult::error(e),
        };

        match apply_text_edits(&normalized, &text_edits, is_regex) {
            Ok((new_content, replacements)) => {
                let edit_count = text_edits.len();
                (new_content, edit_count, replacements)
            }
            Err(e) => return ToolCallResult::error(e),
        }
    } else {
        unreachable!("Already validated that one of operations/edits is Some");
    };

    // ── Generate unified diff ──
    let diff = generate_unified_diff(path_str, &normalized, &modified_content);

    // ── Count changes ──
    let original_line_count = normalized.split('\n').count();
    let new_line_count = modified_content.split('\n').count();
    let lines_added = new_line_count as i64 - original_line_count as i64;
    let lines_removed = if lines_added < 0 { -lines_added } else { 0 };
    let lines_added_positive = if lines_added > 0 { lines_added } else { 0 };

    // ── Write file (unless dryRun) ──
    if !dry_run {
        // Restore original line endings
        let output = if line_ending == "\r\n" {
            modified_content.replace('\n', "\r\n")
        } else {
            modified_content
        };

        if let Err(e) = std::fs::write(&resolved, output.as_bytes()) {
            return ToolCallResult::error(format!("Failed to write file: {}", e));
        }
    }

    // ── Build response ──
    let mut response = json!({
        "path": path_str,
        "applied": applied,
        "linesAdded": lines_added_positive,
        "linesRemoved": lines_removed,
        "newLineCount": new_line_count,
        "dryRun": dry_run,
    });

    if total_replacements > 0 {
        response["totalReplacements"] = json!(total_replacements);
    }

    if !diff.is_empty() {
        response["diff"] = json!(diff);
    } else {
        response["diff"] = json!("(no changes)");
    }

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

struct TextEdit {
    search: String,
    replace: String,
    occurrence: usize, // 0 = all
}

fn parse_text_edits(edits_array: &[Value]) -> Result<Vec<TextEdit>, String> {
    let mut edits = Vec::with_capacity(edits_array.len());
    for (i, edit) in edits_array.iter().enumerate() {
        let search = edit.get("search")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("edits[{}]: missing or invalid 'search'", i))?
            .to_string();
        let replace = edit.get("replace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("edits[{}]: missing or invalid 'replace'", i))?
            .to_string();
        let occurrence = edit.get("occurrence")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        if search.is_empty() {
            return Err(format!("edits[{}]: 'search' must not be empty", i));
        }

        edits.push(TextEdit { search, replace, occurrence });
    }
    Ok(edits)
}

/// Apply text edits sequentially. Returns (new_content, total_replacements).
fn apply_text_edits(content: &str, edits: &[TextEdit], is_regex: bool) -> Result<(String, usize), String> {
    let mut result = content.to_string();
    let mut total_replacements = 0;

    for edit in edits {
        if is_regex {
            let re = Regex::new(&edit.search)
                .map_err(|e| format!("Invalid regex '{}': {}", edit.search, e))?;
            let count = re.find_iter(&result).count();
            if count == 0 {
                return Err(format!("Pattern not found: \"{}\"", edit.search));
            }
            match edit.occurrence {
                0 => {
                    result = re.replace_all(&result, edit.replace.as_str()).to_string();
                    total_replacements += count;
                }
                n => {
                    if n > count {
                        return Err(format!(
                            "Occurrence {} requested but pattern \"{}\" found only {} time(s)",
                            n, edit.search, count
                        ));
                    }
                    let mut current = 0usize;
                    let replace_str = edit.replace.clone();
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
            let count = result.matches(&edit.search).count();
            if count == 0 {
                return Err(format!("Text not found: \"{}\"", edit.search));
            }
            match edit.occurrence {
                0 => {
                    result = result.replace(&edit.search, &edit.replace);
                    total_replacements += count;
                }
                n => {
                    if n > count {
                        return Err(format!(
                            "Occurrence {} requested but text \"{}\" found only {} time(s)",
                            n, edit.search, count
                        ));
                    }
                    let mut current = 0usize;
                    let mut new_result = String::new();
                    let mut remaining = result.as_str();
                    while let Some(pos) = remaining.find(&edit.search) {
                        current += 1;
                        new_result.push_str(&remaining[..pos]);
                        if current == n {
                            new_result.push_str(&edit.replace);
                        } else {
                            new_result.push_str(&edit.search);
                        }
                        remaining = &remaining[pos + edit.search.len()..];
                    }
                    new_result.push_str(remaining);
                    result = new_result;
                    total_replacements += 1;
                }
            }
        }
    }

    Ok((result, total_replacements))
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