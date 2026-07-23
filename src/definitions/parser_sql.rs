//! SQL regex-based parser: extracts definitions and call sites from T-SQL files.
//!
//! Supports: CREATE PROCEDURE/FUNCTION/TABLE/VIEW/TYPE/INDEX,
//! GO batch delimiters, column extraction, FK constraints, and call sites.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

// ─── Lazy-compiled regex statics ────────────────────────────────────
// All regex patterns are compiled once on first use instead of per-call.

static GO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*GO\s*$").unwrap()
});

static CREATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)^\s*CREATE\s+(OR\s+ALTER\s+)?(UNIQUE\s+)?(CLUSTERED\s+|NONCLUSTERED\s+)?(PROCEDURE|PROC|FUNCTION|TABLE|VIEW|TYPE|INDEX)\s+"
    ).unwrap()
});

static DECL_PARAM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)(?:^|[\s,(])(@[\w]+)\b").unwrap()
});

static CREATE_TABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+TABLE\s+").unwrap()
});

static FK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)REFERENCES\s+").unwrap()
});

static PK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)PRIMARY\s+KEY").unwrap()
});

static COL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(\[?[A-Za-z_][\w]*\]?)\s+(BIGINT|INT|SMALLINT|TINYINT|BIT|DECIMAL|NUMERIC|FLOAT|REAL|MONEY|SMALLMONEY|DATETIME|DATETIME2|DATETIMEOFFSET|DATE|TIME|SMALLDATETIME|CHAR|VARCHAR|NCHAR|NVARCHAR|TEXT|NTEXT|BINARY|VARBINARY|IMAGE|UNIQUEIDENTIFIER|XML|SQL_VARIANT|HIERARCHYID|GEOGRAPHY|GEOMETRY|SYSNAME|TIMESTAMP|ROWVERSION)"
    ).unwrap()
});

static CONSTRAINT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(CONSTRAINT|PRIMARY\s+KEY|UNIQUE|INDEX|CHECK|FOREIGN\s+KEY)").unwrap()
});

static CREATE_TABLE_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+TABLE").unwrap()
});

static CREATE_VIEW_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+(?:OR\s+ALTER\s+)?VIEW\s+").unwrap()
});

static CREATE_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+TYPE\s+").unwrap()
});

static CREATE_INDEX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+(?:UNIQUE\s+)?(?:CLUSTERED\s+|NONCLUSTERED\s+)?INDEX\s+").unwrap()
});

static ON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bON\s+").unwrap()
});

static EXEC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bEXEC(?:UTE)?\s+").unwrap()
});

static FROM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bFROM\s+").unwrap()
});

static JOIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bJOIN\s+").unwrap()
});

static INSERT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bINSERT\s+INTO\s+").unwrap()
});

static UPDATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bUPDATE\s+").unwrap()
});

static DELETE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bDELETE\s+FROM\s+").unwrap()
});


static CREATE_FUNCTION_DECL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bCREATE\s+(?:OR\s+ALTER\s+)?FUNCTION\s+").unwrap()
});

#[allow(unused_imports)]
use super::types::*;

// ─── Main entry point ───────────────────────────────────────────────

pub(crate) fn parse_sql_definitions(
    source: &str,
    file_id: u32,
) -> ParseResult {
    if source.trim().is_empty() {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    // Check if file is comments-only
    let has_non_comment_content = source.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with("--") && !trimmed.starts_with("/*") && !trimmed.starts_with("*")
    });
    if !has_non_comment_content {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let lines: Vec<&str> = source.lines().collect();

    // Split into GO-separated batches
    let batches = split_go_batches(&lines);

    let mut defs: Vec<DefinitionEntry> = Vec::new();
    let mut call_sites: Vec<(usize, Vec<CallSite>)> = Vec::new();
    let mut code_stats: Vec<(usize, CodeStats)> = Vec::new();

    for batch in &batches {
        parse_batch(batch, file_id, &mut defs, &mut call_sites, &mut code_stats);
    }

    (defs, call_sites, code_stats)
}

// ─── Batch splitting ────────────────────────────────────────────────

/// A batch of lines from a GO-separated SQL file.
struct Batch<'a> {
    lines: &'a [&'a str],
    /// 0-based line offset in the original file
    start_line_offset: usize,
}

/// Split lines by GO delimiter. Each GO on its own line (with optional whitespace)
/// starts a new batch.
fn split_go_batches<'a>(lines: &'a [&'a str]) -> Vec<Batch<'a>> {
    let mut batches = Vec::new();
    let mut batch_start = 0;

    for (i, line) in lines.iter().enumerate() {
        if GO_RE.is_match(line) {
            if i > batch_start {
                batches.push(Batch {
                    lines: &lines[batch_start..i],
                    start_line_offset: batch_start,
                });
            }
            batch_start = i + 1;
        }
    }

    // Last batch (after last GO or entire file if no GO)
    if batch_start < lines.len() {
        batches.push(Batch {
            lines: &lines[batch_start..],
            start_line_offset: batch_start,
        });
    }

    batches
}

// ─── Batch parsing ──────────────────────────────────────────────────

fn parse_batch(
    batch: &Batch,
    file_id: u32,
    defs: &mut Vec<DefinitionEntry>,
    call_sites: &mut Vec<(usize, Vec<CallSite>)>,
    code_stats: &mut Vec<(usize, CodeStats)>,
) {
    let batch_text = batch.lines.join("\n");
    let search_text = mask_sql_comments_preserve_offsets(&batch_text);
    let search_upper = search_text.to_uppercase();

    // Try to match CREATE statements
    if let Some(m) = CREATE_RE.find(&search_text) {
        let after_keyword = &batch_text[m.end()..];
        let keyword = extract_keyword_from_match(&search_upper[m.start()..m.end()]);

        match keyword.as_str() {
            "PROCEDURE" | "PROC" => {
                parse_procedure_or_function(
                    batch, file_id, &batch_text, DefinitionKind::StoredProcedure,
                    defs, call_sites, code_stats,
                );
            }
            "FUNCTION" => {
                parse_procedure_or_function(
                    batch, file_id, &batch_text, DefinitionKind::SqlFunction,
                    defs, call_sites, code_stats,
                );
            }
            "TABLE" => {
                parse_table(batch, file_id, &batch_text, after_keyword, defs, code_stats);
            }
            "VIEW" => {
                parse_view(batch, file_id, &batch_text, defs, code_stats);
            }
            "TYPE" => {
                parse_type(batch, file_id, &batch_text, defs, code_stats);
            }
            "INDEX" => {
                parse_index(batch, file_id, &batch_text, defs, code_stats);
            }
            _ => {}
        }
    }
}

/// Extract the DDL keyword (PROCEDURE, TABLE, etc.) from the matched CREATE text.
fn extract_keyword_from_match(matched_upper: &str) -> String {
    // The keyword is the last word before trailing whitespace
    let keywords = ["PROCEDURE", "PROC", "FUNCTION", "TABLE", "VIEW", "TYPE", "INDEX"];
    for kw in &keywords {
        if matched_upper.contains(kw) {
            // Return the most specific match
            if *kw == "PROC" && matched_upper.contains("PROCEDURE") {
                continue;
            }
            return kw.to_string();
        }
    }
    String::new()
}

// ─── Name parsing helpers ───────────────────────────────────────────

fn strip_brackets(name: &str) -> String {
    name.trim_start_matches('[').trim_end_matches(']').to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SqlMultipartIdentifier {
    segments: Vec<String>,
    start: usize,
    end: usize,
    last_segment_delimited: bool,
}

fn scan_sql_multipart_identifier(text: &str, start: usize) -> Option<SqlMultipartIdentifier> {
    let mut cursor = skip_sql_identifier_whitespace(text, start);
    let identifier_start = cursor;
    let mut segments = Vec::with_capacity(2);

    loop {
        if segments.len() == 4 {
            return None;
        }

        let (segment, segment_end, delimited) = scan_sql_identifier_segment(text, cursor)?;
        segments.push(segment);

        let after_segment = skip_sql_identifier_whitespace(text, segment_end);
        if text.as_bytes().get(after_segment) != Some(&b'.') {
            return Some(SqlMultipartIdentifier {
                segments,
                start: identifier_start,
                end: segment_end,
                last_segment_delimited: delimited,
            });
        }

        cursor = skip_sql_identifier_whitespace(text, after_segment + 1);
    }
}

fn scan_sql_identifier_segment(text: &str, start: usize) -> Option<(String, usize, bool)> {
    match text.as_bytes().get(start).copied()? {
        b'[' => scan_sql_delimited_identifier(text, start, b']')
            .map(|(value, end)| (value, end, true)),
        b'"' => scan_sql_delimited_identifier(text, start, b'"')
            .map(|(value, end)| (value, end, true)),
        _ => scan_sql_regular_identifier(text, start)
            .map(|(value, end)| (value, end, false)),
    }
}

fn scan_sql_delimited_identifier(text: &str, start: usize, closing: u8) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let mut cursor = start + 1;
    let mut chunk_start = cursor;
    let mut value = String::new();

    while cursor < bytes.len() {
        if bytes[cursor] != closing {
            cursor += 1;
            continue;
        }

        value.push_str(&text[chunk_start..cursor]);
        if bytes.get(cursor + 1) == Some(&closing) {
            value.push(closing as char);
            cursor += 2;
            chunk_start = cursor;
            continue;
        }

        if value.is_empty() {
            return None;
        }
        return Some((value, cursor + 1));
    }

    None
}

fn scan_sql_regular_identifier(text: &str, start: usize) -> Option<(String, usize)> {
    let mut chars = text.get(start..)?.char_indices();
    let (_, first) = chars.next()?;
    if !is_sql_regular_identifier_start(first) {
        return None;
    }

    let mut end = start + first.len_utf8();
    for (offset, ch) in chars {
        if !is_sql_regular_identifier_continue(ch) {
            break;
        }
        end = start + offset + ch.len_utf8();
    }

    Some((text[start..end].to_string(), end))
}

fn skip_sql_identifier_whitespace(text: &str, mut cursor: usize) -> usize {
    while let Some(ch) = text.get(cursor..).and_then(|rest| rest.chars().next()) {
        if !ch.is_whitespace() {
            break;
        }
        cursor += ch.len_utf8();
    }
    cursor
}

fn is_sql_regular_identifier_start(ch: char) -> bool {
    ch == '_' || ch == '@' || ch == '#' || ch.is_alphabetic()
}

fn is_sql_regular_identifier_continue(ch: char) -> bool {
    is_sql_regular_identifier_start(ch) || ch == '$' || ch.is_numeric()
}

fn canonical_sql_identifier_segment(segment: &str) -> String {
    let mut chars = segment.chars();
    let is_regular = chars.next().is_some_and(is_sql_regular_identifier_start)
        && chars.all(is_sql_regular_identifier_continue);
    if is_regular {
        segment.to_string()
    } else {
        format!("[{}]", segment.replace(']', "]]"))
    }
}

fn canonical_sql_multipart_identifier(identifier: &SqlMultipartIdentifier) -> String {
    identifier.segments
        .iter()
        .map(|segment| canonical_sql_identifier_segment(segment))
        .collect::<Vec<_>>()
        .join(".")
}

fn sql_identifier_parts(identifier: &SqlMultipartIdentifier) -> (Option<String>, String) {
    let name = identifier.segments.last().cloned().unwrap_or_default();
    let parent = if identifier.segments.len() > 1 {
        Some(identifier.segments[..identifier.segments.len() - 1]
            .iter()
            .map(|segment| canonical_sql_identifier_segment(segment))
            .collect::<Vec<_>>()
            .join("."))
    } else {
        None
    };

    (parent, name)
}

fn scan_sql_function_call_candidates(text: &str) -> Vec<SqlMultipartIdentifier> {
    let mut candidates = Vec::new();
    let mut cursor = 0;

    while cursor < text.len() {
        let ch = match text[cursor..].chars().next() {
            Some(ch) => ch,
            None => break,
        };
        let previous_is_identifier = text[..cursor].chars().next_back()
            .is_some_and(is_sql_regular_identifier_continue);
        let can_start = ch == '[' || ch == '"' || is_sql_regular_identifier_start(ch);

        if can_start && !previous_is_identifier
            && let Some(identifier) = scan_sql_multipart_identifier(text, cursor) {
                let after_identifier = skip_sql_identifier_whitespace(text, identifier.end);
                let is_qualified_call = identifier.segments.len() > 1
                    && text.as_bytes().get(after_identifier) == Some(&b'(');
                cursor = identifier.end;
                if is_qualified_call {
                    candidates.push(identifier);
                }
                continue;
            }

        cursor += ch.len_utf8();
    }

    candidates
}

// ─── Line counting helpers ──────────────────────────────────────────

fn compute_code_stats_for_lines(lines: &[&str]) -> CodeStats {
    let code_lines = lines.iter().filter(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with("--")
    }).count() as u16;

    CodeStats {
        cyclomatic_complexity: code_lines,  // rough proxy for SQL
        cognitive_complexity: 0,
        max_nesting_depth: 0,
        param_count: 0,
        return_count: 0,
        call_count: 0,
        lambda_count: 0,
    }
}

// ─── CREATE PROCEDURE / FUNCTION parsing ────────────────────────────

fn extract_signature_params(
    batch_text: &str,
    declaration_start: usize,
    kind: DefinitionKind,
) -> Vec<String> {
    // Body statements can start with @ too, so signature params must come from
    // the declaration only.
    let declaration_text = declaration_text_before_body(&batch_text[declaration_start..], kind);
    let declaration_for_params = mask_sql_comments_and_literals_preserve_offsets(declaration_text);
    let mut seen = HashSet::new();
    DECL_PARAM_RE.captures_iter(&declaration_for_params)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .filter(|param| seen.insert(param.to_ascii_lowercase()))
        .take(5)
        .collect()
}

fn mask_sql_comments_preserve_offsets(text: &str) -> String {
    mask_sql_non_code_preserve_offsets(text, false)
}

fn mask_sql_comments_and_literals_preserve_offsets(text: &str) -> String {
    mask_sql_non_code_preserve_offsets(text, true)
}

fn mask_sql_non_code_preserve_offsets(text: &str, mask_literals: bool) -> String {
    let mut bytes = text.as_bytes().to_vec();
    let mut idx = 0;

    while idx < bytes.len() {
        if bytes[idx] == b'\'' {
            idx = skip_sql_single_quoted_literal(&mut bytes, idx, mask_literals);
            continue;
        }

        if bytes[idx] == b'[' {
            idx = skip_sql_bracketed_identifier(&bytes, idx);
            continue;
        }

        if bytes[idx] == b'"' {
            idx = skip_sql_double_quoted_identifier(&bytes, idx);
            continue;
        }

        if bytes[idx] == b'-' && bytes.get(idx + 1) == Some(&b'-') {
            mask_sql_byte(&mut bytes, idx);
            mask_sql_byte(&mut bytes, idx + 1);
            idx += 2;

            while idx < bytes.len() && bytes[idx] != b'\n' {
                mask_sql_byte(&mut bytes, idx);
                idx += 1;
            }
            continue;
        }

        if bytes[idx] == b'/' && bytes.get(idx + 1) == Some(&b'*') {
            mask_sql_byte(&mut bytes, idx);
            mask_sql_byte(&mut bytes, idx + 1);
            idx += 2;

            while idx < bytes.len() {
                let closes_comment = bytes[idx] == b'*' && bytes.get(idx + 1) == Some(&b'/');
                mask_sql_byte(&mut bytes, idx);
                if closes_comment {
                    mask_sql_byte(&mut bytes, idx + 1);
                    idx += 2;
                    break;
                }
                idx += 1;
            }
            continue;
        }

        idx += 1;
    }

    String::from_utf8(bytes).unwrap()
}

fn skip_sql_single_quoted_literal(bytes: &mut [u8], start: usize, mask_literal: bool) -> usize {
    if mask_literal {
        mask_sql_byte(bytes, start);
    }

    let mut idx = start + 1;
    while idx < bytes.len() {
        let current = bytes[idx];
        if mask_literal {
            mask_sql_byte(bytes, idx);
        }
        idx += 1;

        if current == b'\'' {
            if bytes.get(idx) == Some(&b'\'') {
                if mask_literal {
                    mask_sql_byte(bytes, idx);
                }
                idx += 1;
            } else {
                break;
            }
        }
    }

    idx
}

fn skip_sql_bracketed_identifier(bytes: &[u8], start: usize) -> usize {
    skip_sql_delimited_identifier_bytes(bytes, start, b']')
}

fn skip_sql_double_quoted_identifier(bytes: &[u8], start: usize) -> usize {
    skip_sql_delimited_identifier_bytes(bytes, start, b'"')
}

fn skip_sql_delimited_identifier_bytes(bytes: &[u8], start: usize, closing: u8) -> usize {
    let mut idx = start + 1;
    while idx < bytes.len() {
        if bytes[idx] == closing {
            if bytes.get(idx + 1) == Some(&closing) {
                idx += 2;
            } else {
                return idx + 1;
            }
        } else {
            idx += 1;
        }
    }

    idx
}

fn mask_sql_delimited_identifiers_preserve_offsets(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    let mut idx = 0;

    while idx < bytes.len() {
        let (end, closing) = match bytes[idx] {
            b'[' => (skip_sql_bracketed_identifier(&bytes, idx), b']'),
            b'"' => (skip_sql_double_quoted_identifier(&bytes, idx), b'"'),
            _ => {
                idx += 1;
                continue;
            }
        };
        let content_end = if bytes.get(end.saturating_sub(1)) == Some(&closing) {
            end - 1
        } else {
            end
        };
        for mask_idx in idx + 1..content_end {
            mask_sql_byte(&mut bytes, mask_idx);
        }
        idx = end;
    }

    String::from_utf8(bytes).unwrap()
}

fn mask_sql_byte(bytes: &mut [u8], idx: usize) {
    if bytes[idx] != b'\r' && bytes[idx] != b'\n' {
        bytes[idx] = b' ';
    }
}

fn declaration_text_before_body(text: &str, kind: DefinitionKind) -> &str {
    let boundary_text = mask_sql_comments_and_literals_preserve_offsets(text);
    let mut offset = 0;

    for line_with_ending in boundary_text.split_inclusive('\n') {
        let line = strip_line_ending(line_with_ending);
        if let Some(boundary) = find_signature_body_boundary(line, kind) {
            return &text[..offset + boundary];
        }
        offset += line_with_ending.len();
    }

    text
}

fn strip_line_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn find_signature_body_boundary(line: &str, kind: DefinitionKind) -> Option<usize> {
    let leading_whitespace = line.len() - line.trim_start().len();
    let trimmed = &line[leading_whitespace..];

    if kind == DefinitionKind::SqlFunction
        && let Some(returns_idx) = find_sql_keyword(trimmed, "RETURNS")
    {
        return Some(leading_whitespace + returns_idx);
    }

    if let Some(as_idx) = find_proc_body_as(trimmed) {
        return Some(leading_whitespace + as_idx);
    }

    let upper_trimmed = trimmed.to_ascii_uppercase();
    if starts_with_body_keyword(&upper_trimmed) {
        return Some(leading_whitespace);
    }

    None
}

fn find_proc_body_as(line: &str) -> Option<usize> {
    let upper = line.to_ascii_uppercase();

    for (idx, _) in upper.match_indices("AS") {
        if !is_keyword_boundary(&upper, idx, "AS") {
            continue;
        }

        let after = upper[idx + "AS".len()..].trim_start();
        if after.is_empty()
            || after.starts_with("--")
            || after.starts_with("/*")
            || after.starts_with(';')
            || starts_with_body_keyword(after)
        {
            return Some(idx);
        }
    }

    None
}

fn find_sql_keyword(line: &str, keyword: &str) -> Option<usize> {
    let upper = line.to_ascii_uppercase();
    upper.match_indices(keyword)
        .find(|(idx, _)| is_keyword_boundary(&upper, *idx, keyword))
        .map(|(idx, _)| idx)
}

fn is_keyword_boundary(upper: &str, idx: usize, keyword: &str) -> bool {
    let before_ok = match upper[..idx].chars().next_back() {
        Some(ch) => !is_sql_identifier_char(ch),
        None => true,
    };
    let after_idx = idx + keyword.len();
    let after_ok = match upper[after_idx..].chars().next() {
        Some(ch) => !is_sql_identifier_char(ch),
        None => true,
    };

    before_ok && after_ok
}

fn starts_with_body_keyword(upper_trimmed: &str) -> bool {
    [
        "BEGIN", "DECLARE", "DELETE", "EXEC", "EXECUTE", "INSERT", "MERGE",
        "RETURN", "SELECT", "SET", "UPDATE", "WITH",
    ].iter().any(|keyword| starts_with_sql_keyword(upper_trimmed, keyword))
}

fn starts_with_sql_keyword(upper: &str, keyword: &str) -> bool {
    let Some(rest) = upper.strip_prefix(keyword) else {
        return false;
    };

    match rest.chars().next() {
        Some(ch) => !is_sql_identifier_char(ch),
        None => true,
    }
}

fn is_sql_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '@'
}

fn first_signature_line(
    batch_text: &str,
    create_start: usize,
    kind: DefinitionKind,
) -> String {
    let line = batch_text[create_start..]
        .lines()
        .next()
        .map(str::trim)
        .unwrap_or("");
    let boundary_line = mask_sql_comments_and_literals_preserve_offsets(line);

    let end = match find_signature_body_boundary(&boundary_line, kind) {
        Some(boundary) => boundary,
        None => mask_sql_comments_preserve_offsets(line).trim_end().len(),
    };
    line[..end].trim_end().to_string()
}

fn parse_procedure_or_function(
    batch: &Batch,
    file_id: u32,
    batch_text: &str,
    kind: DefinitionKind,
    defs: &mut Vec<DefinitionEntry>,
    call_sites: &mut Vec<(usize, Vec<CallSite>)>,
    code_stats: &mut Vec<(usize, CodeStats)>,
) {
    let kw = if kind == DefinitionKind::StoredProcedure { "PROC(?:EDURE)?" } else { "FUNCTION" };
    let re = Regex::new(&format!(
        r"(?i)CREATE\s+(?:OR\s+ALTER\s+)?{}\s+",
        kw
    )).unwrap();

    let search_text = mask_sql_comments_and_literals_preserve_offsets(batch_text);
    let anchor_text = mask_sql_delimited_identifiers_preserve_offsets(&search_text);
    let create_match = match re.find(&anchor_text) {
        Some(matched) => matched,
        None => return,
    };
    let identifier = match scan_sql_multipart_identifier(&search_text, create_match.end()) {
        Some(identifier) => identifier,
        None => return,
    };
    let (parent, name) = sql_identifier_parts(&identifier);

    if name.is_empty() { return; }

    let mut sig = first_signature_line(batch_text, create_match.start(), kind);

    let params = extract_signature_params(batch_text, identifier.end, kind);

    if !params.is_empty() {
        let sig_for_params = mask_sql_comments_and_literals_preserve_offsets(&sig);
        let mut existing_params = HashSet::new();
        let mut first_existing_param_start = None;
        for capture in DECL_PARAM_RE.captures_iter(&sig_for_params) {
            if let Some(param) = capture.get(1) {
                first_existing_param_start.get_or_insert(param.start());
                existing_params.insert(param.as_str().to_ascii_lowercase());
            }
        }

        let missing_params: Vec<&str> = params.iter()
            .map(String::as_str)
            .filter(|param| !existing_params.contains(&param.to_ascii_lowercase()))
            .collect();

        if !missing_params.is_empty() {
            let trimmed_sig = sig.trim_end();
            let (prefix, params_to_append) = if let Some(param_start) = first_existing_param_start {
                (sig[..param_start].trim_end(), params.join(", "))
            } else {
                (trimmed_sig, missing_params.join(", "))
            };

            if prefix.ends_with('(') {
                sig = format!("{}{})", prefix, params_to_append);
            } else {
                sig = format!("{} ({})", prefix, params_to_append);
            }
        }
    }

    let line_start = (batch.start_line_offset + 1) as u32; // 1-based
    let line_end = (batch.start_line_offset + batch.lines.len()) as u32;

    let def_idx = defs.len();
    defs.push(DefinitionEntry {
        file_id,
        name: name.clone(),
        kind,
        line_start,
        line_end,
        parent,
        signature: Some(sig.split_whitespace().collect::<Vec<_>>().join(" ")),
        modifiers: Vec::new(),
        attributes: Vec::new(),
        base_types: Vec::new(),
    });

    // Extract call sites from SP/function body
    let calls = extract_call_sites_from_body(batch_text, batch, &name);
    if !calls.is_empty() {
        call_sites.push((def_idx, calls));
    }

    // Code stats
    let stats = compute_code_stats_for_lines(batch.lines);
    code_stats.push((def_idx, stats));
}

// ─── CREATE TABLE parsing ───────────────────────────────────────────

fn parse_table(
    batch: &Batch,
    file_id: u32,
    batch_text: &str,
    _after_keyword: &str,
    defs: &mut Vec<DefinitionEntry>,
    code_stats: &mut Vec<(usize, CodeStats)>,
) {
    let search_text = mask_sql_comments_and_literals_preserve_offsets(batch_text);
    let anchor_text = mask_sql_delimited_identifiers_preserve_offsets(&search_text);
    let anchor = match CREATE_TABLE_RE.find(&anchor_text) {
        Some(anchor) => anchor,
        None => return,
    };
    let identifier = match scan_sql_multipart_identifier(&search_text, anchor.end()) {
        Some(identifier) => identifier,
        None => return,
    };
    let (parent, name) = sql_identifier_parts(&identifier);

    if name.is_empty() { return; }

    let sig_prefix = format!("CREATE TABLE {}", canonical_sql_multipart_identifier(&identifier));

    // Extract FK constraints → base_types
    let mut base_types: Vec<String> = Vec::new();
    let mut seen_fk: HashSet<String> = HashSet::new();
    for anchor in FK_RE.find_iter(&anchor_text) {
        let ref_table = match scan_sql_multipart_identifier(&search_text, anchor.end()) {
            Some(identifier) => sql_identifier_parts(&identifier).1,
            None => continue,
        };
        if !ref_table.is_empty() && seen_fk.insert(ref_table.to_lowercase()) {
            base_types.push(ref_table);
        }
    }

    // Check for primary key
    let mut modifiers = Vec::new();
    if PK_RE.is_match(batch_text) {
        modifiers.push("primaryKey".to_string());
    }

    let line_start = (batch.start_line_offset + 1) as u32;
    let line_end = (batch.start_line_offset + batch.lines.len()) as u32;

    let table_def_idx = defs.len();
    defs.push(DefinitionEntry {
        file_id,
        name: name.clone(),
        kind: DefinitionKind::Table,
        line_start,
        line_end,
        parent,
        signature: Some(sig_prefix),
        modifiers,
        attributes: Vec::new(),
        base_types,
    });

    // Code stats for table
    let stats = compute_code_stats_for_lines(batch.lines);
    code_stats.push((table_def_idx, stats));

    // Extract columns as child definitions
    extract_columns(batch, file_id, &name, defs);
}

/// Extract column definitions from CREATE TABLE body.
/// Columns are lines inside the parentheses that look like: `[ColumnName] DataType ...`
fn extract_columns(
    batch: &Batch,
    file_id: u32,
    table_name: &str,
    defs: &mut Vec<DefinitionEntry>,
) {
    for (i, line) in batch.lines.iter().enumerate() {
        let trimmed = line.trim();
        // Skip empty, comment, and constraint lines
        if trimmed.is_empty() || trimmed.starts_with("--") || trimmed == "(" || trimmed == ")" || trimmed.starts_with(")") {
            continue;
        }
        if CONSTRAINT_RE.is_match(trimmed) {
            continue;
        }
        // Skip CREATE TABLE line itself
        if CREATE_TABLE_LINE_RE.is_match(trimmed) {
            continue;
        }

        if let Some(caps) = COL_RE.captures(trimmed) {
            let col_name = match caps.get(1) {
                Some(m) => strip_brackets(m.as_str()),
                None => continue,
            };
            let col_type = match caps.get(2) {
                Some(m) => m.as_str().to_uppercase(),
                None => continue,
            };

            // Skip if column name is a SQL keyword
            let keywords = ["CONSTRAINT", "PRIMARY", "UNIQUE", "INDEX", "CHECK", "FOREIGN", "KEY", "ON", "CREATE", "ALTER", "DROP"];
            if keywords.contains(&col_name.to_uppercase().as_str()) {
                continue;
            }

            let line_num = (batch.start_line_offset + i + 1) as u32;

            defs.push(DefinitionEntry {
                file_id,
                name: col_name.clone(),
                kind: DefinitionKind::Column,
                line_start: line_num,
                line_end: line_num,
                parent: Some(table_name.to_string()),
                signature: Some(format!("{} {}", col_name, col_type)),
                modifiers: Vec::new(),
                attributes: Vec::new(),
                base_types: Vec::new(),
            });
        }
    }
}

// ─── CREATE VIEW parsing ────────────────────────────────────────────

fn parse_view(
    batch: &Batch,
    file_id: u32,
    batch_text: &str,
    defs: &mut Vec<DefinitionEntry>,
    code_stats: &mut Vec<(usize, CodeStats)>,
) {
    let search_text = mask_sql_comments_and_literals_preserve_offsets(batch_text);
    let anchor_text = mask_sql_delimited_identifiers_preserve_offsets(&search_text);
    let anchor = match CREATE_VIEW_RE.find(&anchor_text) {
        Some(anchor) => anchor,
        None => return,
    };
    let identifier = match scan_sql_multipart_identifier(&search_text, anchor.end()) {
        Some(identifier) => identifier,
        None => return,
    };
    let (parent, name) = sql_identifier_parts(&identifier);

    if name.is_empty() { return; }

    let sig = format!("CREATE VIEW {}", canonical_sql_multipart_identifier(&identifier));

    let line_start = (batch.start_line_offset + 1) as u32;
    let line_end = (batch.start_line_offset + batch.lines.len()) as u32;

    let def_idx = defs.len();
    defs.push(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::View,
        line_start,
        line_end,
        parent,
        signature: Some(sig),
        modifiers: Vec::new(),
        attributes: Vec::new(),
        base_types: Vec::new(),
    });

    let stats = compute_code_stats_for_lines(batch.lines);
    code_stats.push((def_idx, stats));
}

// ─── CREATE TYPE parsing ────────────────────────────────────────────

fn parse_type(
    batch: &Batch,
    file_id: u32,
    batch_text: &str,
    defs: &mut Vec<DefinitionEntry>,
    code_stats: &mut Vec<(usize, CodeStats)>,
) {
    let search_text = mask_sql_comments_and_literals_preserve_offsets(batch_text);
    let anchor_text = mask_sql_delimited_identifiers_preserve_offsets(&search_text);
    let anchor = match CREATE_TYPE_RE.find(&anchor_text) {
        Some(anchor) => anchor,
        None => return,
    };
    let identifier = match scan_sql_multipart_identifier(&search_text, anchor.end()) {
        Some(identifier) => identifier,
        None => return,
    };
    let (parent, name) = sql_identifier_parts(&identifier);

    if name.is_empty() { return; }

    let sig = format!("CREATE TYPE {}", canonical_sql_multipart_identifier(&identifier));

    let line_start = (batch.start_line_offset + 1) as u32;
    let line_end = (batch.start_line_offset + batch.lines.len()) as u32;

    let def_idx = defs.len();
    defs.push(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::UserDefinedType,
        line_start,
        line_end,
        parent,
        signature: Some(sig),
        modifiers: Vec::new(),
        attributes: Vec::new(),
        base_types: Vec::new(),
    });

    let stats = compute_code_stats_for_lines(batch.lines);
    code_stats.push((def_idx, stats));
}

// ─── CREATE INDEX parsing ───────────────────────────────────────────

fn parse_index(
    batch: &Batch,
    file_id: u32,
    batch_text: &str,
    defs: &mut Vec<DefinitionEntry>,
    code_stats: &mut Vec<(usize, CodeStats)>,
) {
    let search_text = mask_sql_comments_and_literals_preserve_offsets(batch_text);
    let anchor_text = mask_sql_delimited_identifiers_preserve_offsets(&search_text);
    let anchor = match CREATE_INDEX_RE.find(&anchor_text) {
        Some(anchor) => anchor,
        None => return,
    };
    let index_identifier = match scan_sql_multipart_identifier(&search_text, anchor.end()) {
        Some(identifier) => identifier,
        None => return,
    };
    let index_name = sql_identifier_parts(&index_identifier).1;
    if index_name.is_empty() { return; }

    // Index parent remains the table's display name for backward compatibility.
    let parent = ON_RE.find(&anchor_text)
        .and_then(|on_anchor| scan_sql_multipart_identifier(&search_text, on_anchor.end()))
        .map(|identifier| sql_identifier_parts(&identifier).1);

    // Build signature
    let first_line = batch.lines.first().map(|l| l.trim()).unwrap_or("");
    let sig = first_line.split_whitespace().collect::<Vec<_>>().join(" ");

    let line_start = (batch.start_line_offset + 1) as u32;
    let line_end = (batch.start_line_offset + batch.lines.len()) as u32;

    let def_idx = defs.len();
    defs.push(DefinitionEntry {
        file_id,
        name: index_name,
        kind: DefinitionKind::SqlIndex,
        line_start,
        line_end,
        parent,
        signature: Some(sig),
        modifiers: Vec::new(),
        attributes: Vec::new(),
        base_types: Vec::new(),
    });

    let stats = compute_code_stats_for_lines(batch.lines);
    code_stats.push((def_idx, stats));
}

// ─── Call site extraction ───────────────────────────────────────────

fn extract_call_sites_from_body(
    body_text: &str,
    batch: &Batch,
    _current_name: &str,
) -> Vec<CallSite> {
    let mut calls: Vec<CallSite> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    // Temporary table/variable names to skip
    let skip_names: HashSet<&str> = ["#", "@", "INSERTED", "DELETED", "sys", "INFORMATION_SCHEMA"]
        .iter().copied().collect();

    let all_regexes: Vec<&Regex> = vec![&EXEC_RE, &FROM_RE, &JOIN_RE, &INSERT_RE, &UPDATE_RE, &DELETE_RE];
    let masked_body = mask_sql_comments_and_literals_preserve_offsets(body_text);
    let anchor_body = mask_sql_delimited_identifiers_preserve_offsets(&masked_body);
    let mut anchors: Vec<_> = all_regexes
        .iter()
        .flat_map(|regex| regex.find_iter(&anchor_body).map(|anchor| (anchor.start(), anchor.end())))
        .collect();
    anchors.sort_unstable();

    for (anchor_start, anchor_end) in anchors {
        let identifier = match scan_sql_multipart_identifier(&masked_body, anchor_end) {
            Some(identifier) => identifier,
            None => continue,
        };
        let (receiver_type, method_name) = sql_identifier_parts(&identifier);

        if method_name.is_empty() { continue; }

        // Skip temp tables, table variables, and system objects
        if method_name.starts_with('#') || method_name.starts_with('@') {
            continue;
        }
        if let Some(ref rt) = receiver_type
            && skip_names.contains(rt.as_str()) {
                continue;
            }
        if skip_names.contains(method_name.as_str()) {
            continue;
        }

        let key = (
            receiver_type.clone().unwrap_or_default().to_lowercase(),
            method_name.to_lowercase(),
        );
        if seen.insert(key) {
            let line_offset = masked_body[..anchor_start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            calls.push(CallSite {
                method_name,
                receiver_type,
                line: (batch.start_line_offset + line_offset + 1) as u32,
                receiver_is_generic: false,
            });
        }
    }

    let declaration_ranges: Vec<_> = CREATE_FUNCTION_DECL_RE
        .find_iter(&anchor_body)
        .filter_map(|anchor| scan_sql_multipart_identifier(&masked_body, anchor.end()))
        .map(|identifier| identifier.start..identifier.end)
        .collect();

    for identifier in scan_sql_function_call_candidates(&masked_body) {
        if declaration_ranges
            .iter()
            .any(|range| range.contains(&identifier.start))
        {
            continue;
        }

        let (receiver_type, method_name) = sql_identifier_parts(&identifier);
        let schema = match receiver_type {
            Some(schema) => schema,
            None => continue,
        };
        if skip_names.contains(schema.as_str()) || skip_names.contains(method_name.as_str()) {
            continue;
        }
        let method_name_lower = method_name.to_ascii_lowercase();
        // Until typed scalar call sites land (D12), keep this conservative: broad
        // schema.name(...) matching would misclassify XML/spatial member calls.
        let has_explicit_function_name = identifier.last_segment_delimited
            || method_name_lower.starts_with("fn_")
            || method_name_lower.starts_with("ufn_");
        if !has_explicit_function_name {
            continue;
        }

        let key = (schema.to_lowercase(), method_name.to_lowercase());
        if seen.insert(key) {
            let line_offset = masked_body[..identifier.start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            calls.push(CallSite {
                method_name,
                receiver_type: Some(schema),
                line: (batch.start_line_offset + line_offset + 1) as u32,
                receiver_is_generic: false,
            });
        }
    }

    calls
}