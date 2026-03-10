//! search_fast handler: pre-built file name index search.

use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};
use tracing::info;

use crate::mcp::protocol::ToolCallResult;

use super::HandlerContext;
use super::utils::{best_match_tier, inject_branch_warning, json_to_string};

pub(crate) fn handle_search_fast(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let raw_pattern = args.get("pattern").and_then(|v| v.as_str());
    let dir_provided = args.get("dir").and_then(|v| v.as_str()).is_some();

    // Determine if this is a "list all" (wildcard) request:
    //   pattern="*"          → wildcard
    //   pattern="" + dir set → wildcard (convenient shortcut)
    //   pattern="" no dir    → error
    let is_wildcard = match raw_pattern {
        Some("*") => true,
        Some("") if dir_provided => true,
        Some("") => return ToolCallResult::error(
            "Empty pattern without dir. Either provide a pattern to search for, \
             or specify dir to list all files in a directory (pattern='' or pattern='*'). \
             Do NOT fall back to built-in list_files or list_directory.".to_string()
        ),
        None => return ToolCallResult::error("Missing required parameter: pattern".to_string()),
        _ => false,
    };

    let pattern = raw_pattern.unwrap_or("").to_string();

    let dir = args.get("dir").and_then(|v| v.as_str()).unwrap_or(&ctx.server_dir).to_string();
    let ext = args.get("ext").and_then(|v| v.as_str()).map(|s| s.to_string());
    let use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let ignore_case = args.get("ignoreCase").and_then(|v| v.as_bool()).unwrap_or(false);
    let dirs_only = args.get("dirsOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let files_only = args.get("filesOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let count_only = args.get("countOnly").and_then(|v| v.as_bool()).unwrap_or(false);

    let start = Instant::now();

    // Load file index
    let index = match crate::load_index(&dir, &ctx.index_base) {
        Ok(idx) => idx,
        Err(_) => {
            // Auto-build
            info!(dir = %dir, "No file index found, building automatically");
            let new_index = match crate::build_index(&crate::IndexArgs {
                dir: dir.clone(),
                max_age_hours: 24,
                hidden: false,
                no_ignore: false,
                threads: 0,
            }) {
                Ok(idx) => idx,
                Err(e) => return ToolCallResult::error(format!("Failed to build file index: {}", e)),
            };
            let _ = crate::save_index(&new_index, &ctx.index_base);
            new_index
        }
    };

    // Split comma-separated patterns into multiple terms for OR matching
    let terms: Vec<String> = pattern
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    let search_terms: Vec<String> = if ignore_case {
        terms.iter().map(|t| t.to_lowercase()).collect()
    } else {
        terms.clone()
    };

    let re_list: Option<Vec<regex::Regex>> = if use_regex {
        let mut regexes = Vec::with_capacity(terms.len());
        for t in &terms {
            let pat = if ignore_case { format!("(?i){}", t) } else { t.clone() };
            match regex::Regex::new(&pat) {
                Ok(r) => regexes.push(r),
                Err(e) => return ToolCallResult::error(format!("Invalid regex '{}': {}", t, e)),
            }
        }
        Some(regexes)
    } else {
        None
    };

    // ext filter is meaningless for directories (they have no file extension),
    // so we skip it when dirsOnly=true and emit a hint in the response.
    let ext_ignored_for_dirs = dirs_only && ext.is_some();
    let effective_ext = if dirs_only { &None } else { &ext };

    let mut results: Vec<Value> = Vec::new();
    let mut match_count = 0usize;

    for entry in &index.entries {
        if dirs_only && !entry.is_dir { continue; }
        if files_only && entry.is_dir { continue; }

        if let Some(ext_f) = effective_ext {
            let path = Path::new(&entry.path);
            let matches_ext = path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(ext_f));
            if !matches_ext { continue; }
        }

        let name = Path::new(&entry.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let search_name = if ignore_case { name.to_lowercase() } else { name.to_string() };

        let matched = if is_wildcard {
            true // wildcard → everything matches
        } else if let Some(ref regexes) = re_list {
            regexes.iter().any(|re| re.is_match(&search_name))
        } else {
            search_terms.iter().any(|term| search_name.contains(term.as_str()))
        };

        if matched {
            match_count += 1;
            if !count_only {
                results.push(json!({
                    "path": entry.path,
                    "size": entry.size,
                    "isDir": entry.is_dir,
                }));
            }
        }
    }

    // ── Relevance ranking: exact match first, then prefix, then contains ──
    // Skip ranking for wildcard (no search terms to rank against)
    if !count_only && !is_wildcard {
        results.sort_by(|a, b| {
            let path_a = a["path"].as_str().unwrap_or("");
            let path_b = b["path"].as_str().unwrap_or("");
            // Extract filename stem (without extension) for matching
            let stem_a = std::path::Path::new(path_a)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let stem_b = std::path::Path::new(path_b)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");

            let tier_a = best_match_tier(stem_a, &search_terms);
            let tier_b = best_match_tier(stem_b, &search_terms);
            tier_a.cmp(&tier_b)
                .then_with(|| stem_a.len().cmp(&stem_b.len()))
                .then_with(|| path_a.cmp(path_b))
        });
    }

    let elapsed = start.elapsed();

    let mut summary = json!({
        "totalMatches": match_count,
        "totalIndexed": index.entries.len(),
        "searchTimeMs": elapsed.as_secs_f64() * 1000.0,
    });
    if ext_ignored_for_dirs {
        summary["hint"] = json!("ext filter ignored when dirsOnly=true (directories have no file extension)");
    }
    inject_branch_warning(&mut summary, ctx);
    let output = json!({
        "files": results,
        "summary": summary
    });

    ToolCallResult::success(json_to_string(&output))
}