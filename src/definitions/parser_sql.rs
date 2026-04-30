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
    Regex::new(
        r"(?i)CREATE\s+TABLE\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static FK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)REFERENCES\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
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
    Regex::new(
        r"(?i)CREATE\s+(?:OR\s+ALTER\s+)?VIEW\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static CREATE_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)CREATE\s+TYPE\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static CREATE_INDEX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)CREATE\s+(?:UNIQUE\s+)?(?:CLUSTERED\s+|NONCLUSTERED\s+)?INDEX\s+(\[?[\w]+\]?)"
    ).unwrap()
});

static ON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bON\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static EXEC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bEXEC(?:UTE)?\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static FROM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bFROM\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static JOIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bJOIN\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static INSERT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bINSERT\s+INTO\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static UPDATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bUPDATE\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
});

static DELETE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bDELETE\s+FROM\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap()
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

/// Strip square brackets from a SQL identifier: `[dbo]` → `dbo`
fn strip_brackets(name: &str) -> String {
    name.trim_start_matches('[').trim_end_matches(']').to_string()
}

/// Extract (schema, name) from a regex match with the common SQL pattern:
/// `((\[?schema\]?)\s*\.\s*(\[?name\]?)|(\[?name\]?))`.
///
/// `schema_group` / `name_group` are the group indices for the schema-qualified form,
/// `simple_group` is the group index for the standalone name form.
/// Returns `None` if no valid name can be extracted (defensive against corrupted SQL).
fn extract_schema_name(
    caps: &regex::Captures,
    schema_group: usize,
    name_group: usize,
    simple_group: usize,
) -> Option<(String, String)> {
    if caps.get(schema_group).is_some() {
        let schema = caps.get(schema_group)?.as_str();
        let name = caps.get(name_group)?.as_str();
        Some((strip_brackets(schema), strip_brackets(name)))
    } else {
        let name = caps.get(simple_group)?.as_str();
        Some((String::new(), strip_brackets(name)))
    }
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
    let mut idx = start + 1;
    while idx < bytes.len() {
        if bytes[idx] == b']' {
            if bytes.get(idx + 1) == Some(&b']') {
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
        r"(?i)CREATE\s+(?:OR\s+ALTER\s+)?{}\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))",
        kw
    )).unwrap();

    let search_text = mask_sql_comments_preserve_offsets(batch_text);
    let caps = match re.captures(&search_text) {
        Some(c) => c,
        None => return,
    };
    let create_match = caps.get(0).unwrap();

    let (schema, name) = match extract_schema_name(&caps, 2, 3, 4) {
        Some(pair) => pair,
        None => return,
    };

    if name.is_empty() { return; }

    let mut sig = first_signature_line(batch_text, create_match.start(), kind);

    let params = extract_signature_params(batch_text, create_match.end(), kind);

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
        parent: if schema.is_empty() { None } else { Some(schema) },
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
    let caps = match CREATE_TABLE_RE.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let (_schema, name) = match extract_schema_name(&caps, 2, 3, 4) {
        Some(pair) => pair,
        None => return,
    };

    if name.is_empty() { return; }

    // Build signature
    let sig_prefix = if _schema.is_empty() {
        format!("CREATE TABLE {}", name)
    } else {
        format!("CREATE TABLE [{}].[{}]", _schema, name)
    };

    // Extract FK constraints → base_types
    let mut base_types: Vec<String> = Vec::new();
    let mut seen_fk: HashSet<String> = HashSet::new();
    for caps in FK_RE.captures_iter(batch_text) {
        let ref_table = if let Some((_, name)) = extract_schema_name(&caps, 2, 3, 4) {
            name
        } else {
            continue;
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
        parent: None,
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
    let caps = match CREATE_VIEW_RE.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let (schema, name) = match extract_schema_name(&caps, 2, 3, 4) {
        Some(pair) => pair,
        None => return,
    };

    if name.is_empty() { return; }

    let sig = if schema.is_empty() {
        format!("CREATE VIEW {}", name)
    } else {
        format!("CREATE VIEW [{}].[{}]", schema, name)
    };

    let line_start = (batch.start_line_offset + 1) as u32;
    let line_end = (batch.start_line_offset + batch.lines.len()) as u32;

    let def_idx = defs.len();
    defs.push(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::View,
        line_start,
        line_end,
        parent: None,
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
    let caps = match CREATE_TYPE_RE.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let (schema, name) = match extract_schema_name(&caps, 2, 3, 4) {
        Some(pair) => pair,
        None => return,
    };

    if name.is_empty() { return; }

    let sig = if schema.is_empty() {
        format!("CREATE TYPE {}", name)
    } else {
        format!("CREATE TYPE [{}].[{}]", schema, name)
    };

    let line_start = (batch.start_line_offset + 1) as u32;
    let line_end = (batch.start_line_offset + batch.lines.len()) as u32;

    let def_idx = defs.len();
    defs.push(DefinitionEntry {
        file_id,
        name,
        kind: DefinitionKind::UserDefinedType,
        line_start,
        line_end,
        parent: None,
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
    let caps = match CREATE_INDEX_RE.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let index_name = match caps.get(1) {
        Some(m) => strip_brackets(m.as_str()),
        None => return,
    };
    if index_name.is_empty() { return; }

    // Parse ON [schema].[table] to determine parent table
    let parent = ON_RE.captures(batch_text)
        .and_then(|on_caps| extract_schema_name(&on_caps, 2, 3, 4))
        .map(|(_, name)| name);

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
    _body_text: &str,
    batch: &Batch,
    _current_name: &str,
) -> Vec<CallSite> {
    let mut calls: Vec<CallSite> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    // Temporary table/variable names to skip
    let skip_names: HashSet<&str> = ["#", "@", "INSERTED", "DELETED", "sys", "INFORMATION_SCHEMA"]
        .iter().copied().collect();

    let all_regexes: Vec<&Regex> = vec![&EXEC_RE, &FROM_RE, &JOIN_RE, &INSERT_RE, &UPDATE_RE, &DELETE_RE];

    for (line_idx, line) in batch.lines.iter().enumerate() {
        let trimmed = line.trim();
        // Skip comment lines
        if trimmed.starts_with("--") {
            continue;
        }

        let line_num = (batch.start_line_offset + line_idx + 1) as u32;

        for regex in &all_regexes {
            for caps in regex.captures_iter(line) {
                let (receiver_type, method_name) = match extract_schema_name(&caps, 2, 3, 4) {
                    Some((schema, name)) if !schema.is_empty() => (Some(schema), name),
                    Some((_, name)) => (None, name),
                    None => continue,
                };

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
                    calls.push(CallSite {
                        method_name,
                        receiver_type,
                        line: line_num,
                        receiver_is_generic: false,
                    });
                }
            }
        }
    }

    calls
}