//! SQL regex-based parser: extracts definitions and call sites from T-SQL files.
//!
//! Supports: CREATE PROCEDURE/FUNCTION/TABLE/VIEW/TYPE/INDEX,
//! GO batch delimiters, column extraction, FK constraints, and call sites.

use std::collections::HashSet;

use regex::Regex;

#[allow(unused_imports)]
use super::types::*;

// ─── Main entry point ───────────────────────────────────────────────

pub(crate) fn parse_sql_definitions(
    source: &str,
    file_id: u32,
) -> (Vec<DefinitionEntry>, Vec<(usize, Vec<CallSite>)>, Vec<(usize, CodeStats)>) {
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
    let go_re = Regex::new(r"(?i)^\s*GO\s*$").unwrap();
    let mut batches = Vec::new();
    let mut batch_start = 0;

    for (i, line) in lines.iter().enumerate() {
        if go_re.is_match(line) {
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
    let batch_upper = batch_text.to_uppercase();

    // Try to match CREATE statements
    let create_re = Regex::new(
        r"(?i)^\s*CREATE\s+(OR\s+ALTER\s+)?(UNIQUE\s+)?(CLUSTERED\s+|NONCLUSTERED\s+)?(PROCEDURE|PROC|FUNCTION|TABLE|VIEW|TYPE|INDEX)\s+"
    ).unwrap();

    if let Some(m) = create_re.find(&batch_text) {
        let after_keyword = &batch_text[m.end()..];
        let keyword = extract_keyword_from_match(&batch_upper[m.start()..m.end()]);

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

    let caps = match re.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let (_schema, name) = if caps.get(2).is_some() {
        (strip_brackets(caps.get(2).unwrap().as_str()),
         strip_brackets(caps.get(3).unwrap().as_str()))
    } else {
        (String::new(), strip_brackets(caps.get(4).unwrap().as_str()))
    };

    if name.is_empty() { return; }

    // Build signature: first meaningful line (containing CREATE) + first ~5 parameters
    let first_line = batch.lines.iter()
        .map(|l| l.trim())
        .find(|l| {
            let upper = l.to_uppercase();
            upper.contains("CREATE") && (upper.contains("PROC") || upper.contains("FUNCTION"))
        })
        .unwrap_or_else(|| batch.lines.first().map(|l| l.trim()).unwrap_or(""));
    let mut sig = first_line.to_string();

    // Extract parameters (lines starting with @)
    let param_re = Regex::new(r"(?m)^\s*(@[\w]+)").unwrap();
    let params: Vec<String> = param_re.captures_iter(batch_text)
        .take(5)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .collect();

    if !params.is_empty() {
        let param_str = params.join(", ");
        if !sig.contains(&param_str) {
            sig = format!("{} ({})", sig.trim_end(), param_str);
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
        parent: None,
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
    let re = Regex::new(
        r"(?i)CREATE\s+TABLE\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    let caps = match re.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let (_schema, name) = if caps.get(2).is_some() {
        (strip_brackets(caps.get(2).unwrap().as_str()),
         strip_brackets(caps.get(3).unwrap().as_str()))
    } else {
        (String::new(), strip_brackets(caps.get(4).unwrap().as_str()))
    };

    if name.is_empty() { return; }

    // Build signature
    let sig_prefix = if _schema.is_empty() {
        format!("CREATE TABLE {}", name)
    } else {
        format!("CREATE TABLE [{}].[{}]", _schema, name)
    };

    // Extract FK constraints → base_types
    let fk_re = Regex::new(
        r"(?i)REFERENCES\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();
    let mut base_types: Vec<String> = Vec::new();
    let mut seen_fk: HashSet<String> = HashSet::new();
    for caps in fk_re.captures_iter(batch_text) {
        let ref_table = if caps.get(2).is_some() {
            strip_brackets(caps.get(3).unwrap().as_str())
        } else {
            strip_brackets(caps.get(4).unwrap().as_str())
        };
        if !ref_table.is_empty() && seen_fk.insert(ref_table.to_lowercase()) {
            base_types.push(ref_table);
        }
    }

    // Check for primary key
    let mut modifiers = Vec::new();
    let pk_re = Regex::new(r"(?i)PRIMARY\s+KEY").unwrap();
    if pk_re.is_match(batch_text) {
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
    let col_re = Regex::new(
        r"(?i)^\s*(\[?[A-Za-z_][\w]*\]?)\s+(BIGINT|INT|SMALLINT|TINYINT|BIT|DECIMAL|NUMERIC|FLOAT|REAL|MONEY|SMALLMONEY|DATETIME|DATETIME2|DATETIMEOFFSET|DATE|TIME|SMALLDATETIME|CHAR|VARCHAR|NCHAR|NVARCHAR|TEXT|NTEXT|BINARY|VARBINARY|IMAGE|UNIQUEIDENTIFIER|XML|SQL_VARIANT|HIERARCHYID|GEOGRAPHY|GEOMETRY|SYSNAME|TIMESTAMP|ROWVERSION)"
    ).unwrap();

    // Also match constraint lines to skip them
    let constraint_re = Regex::new(r"(?i)^\s*(CONSTRAINT|PRIMARY\s+KEY|UNIQUE|INDEX|CHECK|FOREIGN\s+KEY)").unwrap();

    for (i, line) in batch.lines.iter().enumerate() {
        let trimmed = line.trim();
        // Skip empty, comment, and constraint lines
        if trimmed.is_empty() || trimmed.starts_with("--") || trimmed == "(" || trimmed == ")" || trimmed.starts_with(")") {
            continue;
        }
        if constraint_re.is_match(trimmed) {
            continue;
        }
        // Skip CREATE TABLE line itself
        if Regex::new(r"(?i)CREATE\s+TABLE").unwrap().is_match(trimmed) {
            continue;
        }

        if let Some(caps) = col_re.captures(trimmed) {
            let col_name = strip_brackets(caps.get(1).unwrap().as_str());
            let col_type = caps.get(2).unwrap().as_str().to_uppercase();

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
    let re = Regex::new(
        r"(?i)CREATE\s+(?:OR\s+ALTER\s+)?VIEW\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    let caps = match re.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let (schema, name) = if caps.get(2).is_some() {
        (strip_brackets(caps.get(2).unwrap().as_str()),
         strip_brackets(caps.get(3).unwrap().as_str()))
    } else {
        (String::new(), strip_brackets(caps.get(4).unwrap().as_str()))
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
    let re = Regex::new(
        r"(?i)CREATE\s+TYPE\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    let caps = match re.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let (schema, name) = if caps.get(2).is_some() {
        (strip_brackets(caps.get(2).unwrap().as_str()),
         strip_brackets(caps.get(3).unwrap().as_str()))
    } else {
        (String::new(), strip_brackets(caps.get(4).unwrap().as_str()))
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
    let re = Regex::new(
        r"(?i)CREATE\s+(?:UNIQUE\s+)?(?:CLUSTERED\s+|NONCLUSTERED\s+)?INDEX\s+(\[?[\w]+\]?)"
    ).unwrap();

    let caps = match re.captures(batch_text) {
        Some(c) => c,
        None => return,
    };

    let index_name = strip_brackets(caps.get(1).unwrap().as_str());
    if index_name.is_empty() { return; }

    // Parse ON [schema].[table] to determine parent table
    let on_re = Regex::new(
        r"(?i)\bON\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    let parent = if let Some(on_caps) = on_re.captures(batch_text) {
        if on_caps.get(2).is_some() {
            Some(strip_brackets(on_caps.get(3).unwrap().as_str()))
        } else {
            Some(strip_brackets(on_caps.get(4).unwrap().as_str()))
        }
    } else {
        None
    };

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

    // EXEC [schema].[name] or EXECUTE [schema].[name]
    let exec_re = Regex::new(
        r"(?i)\bEXEC(?:UTE)?\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    // FROM [schema].[table]
    let from_re = Regex::new(
        r"(?i)\bFROM\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    // JOIN [schema].[table]
    let join_re = Regex::new(
        r"(?i)\bJOIN\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    // INSERT INTO [schema].[table]
    let insert_re = Regex::new(
        r"(?i)\bINSERT\s+INTO\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    // UPDATE [schema].[table]
    let update_re = Regex::new(
        r"(?i)\bUPDATE\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    // DELETE FROM [schema].[table]
    let delete_re = Regex::new(
        r"(?i)\bDELETE\s+FROM\s+((\[?[\w]+\]?)\s*\.\s*(\[?[\w]+\]?)|(\[?[\w]+\]?))"
    ).unwrap();

    // Temporary table/variable names to skip
    let skip_names: HashSet<&str> = ["#", "@", "INSERTED", "DELETED", "sys", "INFORMATION_SCHEMA"]
        .iter().copied().collect();

    let all_regexes: Vec<&Regex> = vec![&exec_re, &from_re, &join_re, &insert_re, &update_re, &delete_re];

    for (line_idx, line) in batch.lines.iter().enumerate() {
        let trimmed = line.trim();
        // Skip comment lines
        if trimmed.starts_with("--") {
            continue;
        }

        let line_num = (batch.start_line_offset + line_idx + 1) as u32;

        for regex in &all_regexes {
            for caps in regex.captures_iter(line) {
                let (receiver_type, method_name) = if caps.get(2).is_some() {
                    let schema = strip_brackets(caps.get(2).unwrap().as_str());
                    let name = strip_brackets(caps.get(3).unwrap().as_str());
                    (Some(schema), name)
                } else {
                    let name = strip_brackets(caps.get(4).unwrap().as_str());
                    (None, name)
                };

                if method_name.is_empty() { continue; }

                // Skip temp tables, table variables, and system objects
                if method_name.starts_with('#') || method_name.starts_with('@') {
                    continue;
                }
                if let Some(ref rt) = receiver_type {
                    if skip_names.contains(rt.as_str()) {
                        continue;
                    }
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