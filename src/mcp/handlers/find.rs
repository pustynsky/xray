//! search_find handler: live filesystem walk for file name/content search.

use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use crate::mcp::protocol::ToolCallResult;

use super::utils::{json_to_string, validate_search_dir};
use super::HandlerContext;

pub(crate) fn handle_search_find(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => return ToolCallResult::error("Missing required parameter: pattern".to_string()),
    };

    let dir = args.get("dir").and_then(|v| v.as_str()).unwrap_or(&ctx.server_dir).to_string();

    // Validate dir parameter -- must match server dir or be a subdirectory
    if let Err(msg) = validate_search_dir(&dir, &ctx.server_dir) {
        return ToolCallResult::error(msg);
    }

    let ext = args.get("ext").and_then(|v| v.as_str()).map(|s| s.to_string());
    let contents = args.get("contents").and_then(|v| v.as_bool()).unwrap_or(false);
    let use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let ignore_case = args.get("ignoreCase").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_depth = args.get("maxDepth").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let count_only = args.get("countOnly").and_then(|v| v.as_bool()).unwrap_or(false);

    let start = Instant::now();

    let search_pattern = if ignore_case {
        pattern.to_lowercase()
    } else {
        pattern.clone()
    };

    let re = if use_regex {
        match regex::Regex::new(&if ignore_case {
            format!("(?i){}", &pattern)
        } else {
            pattern.clone()
        }) {
            Ok(r) => Some(r),
            Err(e) => return ToolCallResult::error(format!("Invalid regex: {}", e)),
        }
    } else {
        None
    };

    let root = Path::new(&dir);
    if !root.exists() {
        return ToolCallResult::error(format!("Directory does not exist: {}", dir));
    }

    let mut results: Vec<Value> = Vec::new();
    let mut match_count = 0usize;
    let mut file_count = 0usize;

    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false);
    if max_depth > 0 {
        builder.max_depth(Some(max_depth));
    }

    if contents {
        for entry in builder.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) { continue; }
            if let Some(ref ext_f) = ext {
                let matches_ext = entry.path().extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext_f));
                if !matches_ext { continue; }
            }
            file_count += 1;
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let matched = if let Some(ref re) = re {
                re.is_match(&content)
            } else if ignore_case {
                content.to_lowercase().contains(&search_pattern)
            } else {
                content.contains(&search_pattern)
            };
            if matched {
                match_count += 1;
                if !count_only {
                    let mut lines = Vec::new();
                    for (line_num, line) in content.lines().enumerate() {
                        let line_matched = if let Some(ref re) = re {
                            re.is_match(line)
                        } else if ignore_case {
                            line.to_lowercase().contains(&search_pattern)
                        } else {
                            line.contains(&search_pattern)
                        };
                        if line_matched {
                            lines.push(json!({
                                "line": line_num + 1,
                                "text": line.trim(),
                            }));
                        }
                    }
                    results.push(json!({
                        "path": entry.path().display().to_string(),
                        "matches": lines,
                    }));
                }
            }
        }
    } else {
        for entry in builder.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            file_count += 1;
            let name = match entry.file_name().to_str() {
                Some(n) => n.to_string(),
                None => continue,
            };
            if let Some(ref ext_f) = ext {
                let matches_ext = entry.path().extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext_f));
                if !matches_ext { continue; }
            }
            let search_name = if ignore_case { name.to_lowercase() } else { name.clone() };
            let matched = if let Some(ref re) = re {
                re.is_match(&search_name)
            } else {
                search_name.contains(&search_pattern)
            };
            if matched {
                match_count += 1;
                if !count_only {
                    results.push(json!({
                        "path": entry.path().display().to_string(),
                    }));
                }
            }
        }
    }

    let elapsed = start.elapsed();

    let output = json!({
        "files": results,
        "summary": {
            "totalMatches": match_count,
            "totalFilesScanned": file_count,
            "searchTimeMs": elapsed.as_secs_f64() * 1000.0,
        }
    });

    ToolCallResult::success(json_to_string(&output))
}