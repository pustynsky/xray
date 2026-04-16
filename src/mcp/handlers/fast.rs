//! xray_fast handler: pre-built file name index search.

use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};
use tracing::info;

use std::sync::atomic::Ordering;

use crate::mcp::protocol::ToolCallResult;

use super::HandlerContext;
use super::utils::{best_match_tier, inject_branch_warning, json_to_string};

/// Convert a simple glob pattern (containing * or ?) to a regex string.
/// Returns None if the pattern has no glob characters.
/// Only applies to filename matching — anchored to match the full name.
fn maybe_glob_to_regex(pattern: &str) -> Option<String> {
    if pattern == "*" {
        return None; // Special wildcard-all case, handled separately
    }
    if !pattern.contains('*') && !pattern.contains('?') {
        return None; // No glob chars — use normal substring matching
    }
    let mut regex = String::with_capacity(pattern.len() * 2);
    regex.push('^');
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            // Escape regex metacharacters
            '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            _ => regex.push(ch),
        }
    }
    regex.push('$');
    Some(regex)
}

/// Extract the literal (fixed) prefix from a glob pattern — the part before
/// the first glob character (`*` or `?`). Used for ranking results when glob
/// patterns are converted to regex (the regex string itself is useless for
/// `best_match_tier` which does literal `contains`/`starts_with` checks).
/// For non-glob patterns returns the full string unchanged.
fn extract_glob_literal(pattern: &str) -> &str {
    let end = pattern.find(|c: char| c == '*' || c == '?').unwrap_or(pattern.len());
    &pattern[..end]
}

pub(crate) fn handle_xray_fast(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
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

    let dir = args.get("dir").and_then(|v| v.as_str())
        .map(|s| super::utils::resolve_dir_to_absolute(s, &ctx.server_dir()))
        .unwrap_or_else(|| ctx.server_dir());
    let ext = args.get("ext").and_then(|v| v.as_str()).map(|s| s.to_string());
    let use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let ignore_case = args.get("ignoreCase").and_then(|v| v.as_bool()).unwrap_or(false);
    let dirs_only = args.get("dirsOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let files_only = args.get("filesOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    // B1 fix: Reject mutually exclusive flags early
    if dirs_only && files_only {
        return ToolCallResult::error(
            "filesOnly and dirsOnly are mutually exclusive. Use one or neither.".to_string()
        );
    }
    let count_only = args.get("countOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_depth = args.get("maxDepth").and_then(|v| v.as_u64()).map(|d| d as usize);
    let max_results = args.get("maxResults").and_then(|v| v.as_u64()).map(|d| d as usize).unwrap_or(0);

    let start = Instant::now();

    // Load file index from in-memory cache (with dirty-flag invalidation).
    // If dirty or not yet built → rebuild from filesystem (~35ms for 100K files).
    // Otherwise use cached in-memory index (~0ms).
    //
    // Special case: when dir is outside server_dir, build a one-off index
    // for that directory (not cached in memory).
    let server_dir = ctx.server_dir();
    let dir_is_outside = {
        let dir_canon = std::fs::canonicalize(&dir)
            .map(|p| code_xray::clean_path(&p.to_string_lossy()).to_lowercase())
            .unwrap_or_else(|_| dir.replace('\\', "/").to_lowercase());
        // Use cached canonical server_dir (avoids ~1-5ms syscall per request)
        let srv_canon = ctx.canonical_server_dir().to_lowercase();
        let srv_prefix = format!("{}/", srv_canon.trim_end_matches('/'));
        dir_canon != srv_canon && !dir_canon.starts_with(&srv_prefix)
    };

    let index = if dir_is_outside {
        // Outside server_dir: try loading from disk, fall back to build+save.
        // Not cached in memory (it's for a different directory), but saved to disk
        // so repeated calls to the same outside dir are fast.
        match crate::load_index(&dir, &ctx.index_base) {
            Ok(idx) => idx,
            Err(_) => {
                info!(dir = %dir, "Building file-list index for outside directory");
                let new_index = match crate::build_index(&crate::IndexArgs {
                    dir: dir.clone(),
                    max_age_hours: 24,
                    hidden: false,
                    no_ignore: false, respect_git_exclude: false,
                    threads: 0,
                }) {
                    Ok(idx) => idx,
                    Err(e) => return ToolCallResult::error(format!("Failed to build file index: {}", e)),
                };
                let _ = crate::save_index(&new_index, &ctx.index_base);
                new_index
            }
        }
    } else {
        // Inside server_dir (or same): use in-memory cache
        let needs_rebuild = ctx.file_index_dirty.load(Ordering::Relaxed)
            || ctx.file_index.read().map(|fi| fi.is_none()).unwrap_or(true);

        if needs_rebuild {
            info!(dir = %server_dir, "Building file-list index (dirty or first use)");
            let new_index = match crate::build_index(&crate::IndexArgs {
                dir: server_dir.clone(),
                max_age_hours: 24,
                hidden: false,
                no_ignore: false, respect_git_exclude: false,
                threads: 0,
            }) {
                Ok(idx) => idx,
                Err(e) => return ToolCallResult::error(format!("Failed to build file index: {}", e)),
            };
            // Save to disk for CLI/other consumers
            let _ = crate::save_index(&new_index, &ctx.index_base);
            // Store in memory and reset dirty flag
            if let Ok(mut fi) = ctx.file_index.write() {
                *fi = Some(new_index);
            }
            ctx.file_index_dirty.store(false, Ordering::Relaxed);
        }

        // Read from in-memory cache
        let guard = match ctx.file_index.read() {
            Ok(g) => g,
            Err(e) => return ToolCallResult::error(format!("Failed to read file index: {}", e)),
        };
        match guard.as_ref() {
            Some(idx) => idx.clone(),
            None => return ToolCallResult::error("File index not available after build".to_string()),
        }
    };

    // When reusing a parent index for a subdirectory request, compute a path prefix
    // to filter entries. Without this, wildcard searches would return ALL entries in
    // the parent index, not just those under the requested dir.
    let subdir_entry_filter: Option<String> = {
        let root_norm = index.root.replace('\\', "/");
        // dir is already absolute (resolved by resolve_dir_to_absolute at the top).
        // Canonicalize to normalize path separators and resolve symlinks.
        let dir_abs = std::fs::canonicalize(&dir)
            .map(|p| code_xray::clean_path(&p.to_string_lossy()))
            .unwrap_or_else(|_| dir.replace('\\', "/"));
        let root_abs = std::fs::canonicalize(&index.root)
            .map(|p| code_xray::clean_path(&p.to_string_lossy()))
            .unwrap_or(root_norm);
        let dir_lower = dir_abs.to_lowercase();
        let root_lower = root_abs.to_lowercase();
        if dir_lower.trim_end_matches('/') == root_lower.trim_end_matches('/') {
            None // dir == root, no filtering needed
        } else {
            Some(format!("{}/", dir_lower.trim_end_matches('/')))
        }
    };

    // Split comma-separated patterns into multiple terms for OR matching
    let terms: Vec<String> = pattern
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    // Guard: if pattern is "*" and regex=true, treat as wildcard ("*" is invalid regex)
    let use_regex = if is_wildcard && use_regex { false } else { use_regex };

    // Save original terms for ranking before potential glob→regex conversion.
    // `extract_glob_literal` will pull the fixed prefix out later.
    let original_terms = terms.clone();

    // Auto-detect glob patterns (*, ?) and convert to regex
    // Only when not already in regex mode and not a wildcard-all request
    let (use_regex, terms) = if !use_regex && !is_wildcard {
        let mut any_glob = false;
        let converted: Vec<String> = terms.iter().map(|t| {
            if let Some(re) = maybe_glob_to_regex(t) {
                any_glob = true;
                re
            } else {
                t.clone()
            }
        }).collect();
        if any_glob { (true, converted) } else { (use_regex, terms) }
    } else {
        (use_regex, terms)
    };

    // Recompute search_terms after potential glob conversion
    let search_terms: Vec<String> = if ignore_case {
        terms.iter().map(|t| t.to_lowercase()).collect()
    } else {
        terms.clone()
    };

    // Ranking terms: extract literal parts from original patterns (before glob→regex
    // conversion). For non-glob "Order", extract_glob_literal returns "Order" (unchanged).
    // For glob "Order*", it returns "Order" — enabling proper tier ranking.
    // For "*Helper*", it returns "" which is filtered out → tier-2 fallback (length sort).
    let ranking_terms: Vec<String> = original_terms.iter()
        .map(|t| extract_glob_literal(t).to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();

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

    // fileCount is computed AFTER the main loop (two-pass approach).
    // This avoids O(N × depth) HashMap operations for ALL directories when only
    // ~29 matched directories actually need counts. See Finding 1 in performance audit.

    // Compute base depth for maxDepth filtering.
    // When subdir_entry_filter is active (parent index reused for subdirectory),
    // base_depth must be relative to dir, not index.root. Otherwise maxDepth=1
    // would show entries 1 level below root instead of 1 level below dir.
    let base_depth = if max_depth.is_some() {
        if let Some(ref filter) = subdir_entry_filter {
            // filter = "c:/projects/myapp/src/" — count slashes in the dir path (without trailing /)
            filter.trim_end_matches('/').matches('/').count()
        } else {
            index.root.replace('\\', "/").matches('/').count()
        }
    } else {
        0
    };

    let mut results: Vec<Value> = Vec::new();
    let mut match_count = 0usize;

    for entry in &index.entries {
        // Filter by subdirectory when parent index is reused for a subdir request
        if let Some(ref prefix) = subdir_entry_filter {
            let entry_lower = entry.path.to_lowercase();
            // Entry must be under the subdirectory OR be the subdirectory itself
            if !entry_lower.starts_with(prefix)
                && entry_lower != prefix.trim_end_matches('/') {
                continue;
            }
        }
        if dirs_only && !entry.is_dir { continue; }
        if files_only && entry.is_dir { continue; }

        // maxDepth filtering
        if let Some(md) = max_depth {
            let entry_depth = entry.path.matches('/').count();
            if entry_depth.saturating_sub(base_depth) > md {
                continue;
            }
        }

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
                if dirs_only {
                    results.push(json!({
                        "path": entry.path,
                        "size": entry.size,
                        "isDir": true,
                        "fileCount": 0,
                    }));
                } else {
                    results.push(json!({
                        "path": entry.path,
                        "size": entry.size,
                        "isDir": entry.is_dir,
                    }));
                }
            }
        }
    }

    // ── maxResults truncation (before sorting for efficiency, but we sort first for quality) ──

    // ── Two-pass fileCount: count files only for matched directories ──
    // Instead of building a HashMap for ALL ~10K directories (O(N × depth)),
    // we only count files for the ~29 matched directories (O(matched × N)).
    // This reduces ~435ms to ~30ms on 100K-file repos.
    if dirs_only && !count_only && !results.is_empty() {
        for result in &mut results {
            if let Some(dir_path) = result["path"].as_str() {
                let prefix = format!("{}/", dir_path);
                let count = index.entries.iter()
                    .filter(|e| !e.is_dir && e.path.starts_with(&prefix))
                    .count();
                result["fileCount"] = json!(count);
            }
        }
    }

    // ── Sorting ──
    // For dirsOnly: sort by fileCount descending (largest modules first)
    if !count_only && dirs_only {
        results.sort_by(|a, b| {
            let fc_b = b["fileCount"].as_u64().unwrap_or(0);
            let fc_a = a["fileCount"].as_u64().unwrap_or(0);
            fc_b.cmp(&fc_a)
        });
    } else if !count_only && !is_wildcard {
    // Relevance ranking: exact match first, then prefix, then contains
    // Skip ranking for wildcard (no search terms to rank against)
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

            let tier_a = best_match_tier(stem_a, &ranking_terms);
            let tier_b = best_match_tier(stem_b, &ranking_terms);
            tier_a.cmp(&tier_b)
                .then_with(|| stem_a.len().cmp(&stem_b.len()))
                .then_with(|| path_a.cmp(path_b))
        });
    }

    // Apply maxResults truncation after sorting
    let truncated = if max_results > 0 && results.len() > max_results {
        results.truncate(max_results);
        true
    } else {
        false
    };

    let elapsed = start.elapsed();

    let mut summary = json!({
        "totalMatches": match_count,
        "totalIndexed": index.entries.len(),
        "searchTimeMs": elapsed.as_secs_f64() * 1000.0,
    });
    // B2 fix: Use hints array to avoid overwriting
    let mut hints: Vec<String> = Vec::new();
    if ext_ignored_for_dirs {
        hints.push("ext filter ignored when dirsOnly=true (directories have no file extension)".to_string());
    }
    // Hint when dirsOnly results are likely to be truncated
    if dirs_only && match_count > 150 && max_depth.is_none() {
        hints.push(
            "Too many directories. Use maxDepth=1 for immediate children only, \
             or use xray_definitions file='<dir>' for code-level module overview with autoSummary.".to_string()
        );
    }
    if !hints.is_empty() {
        summary["hint"] = json!(hints.join(". "));
    }
    if truncated {
        summary["truncated"] = json!(true);
        summary["maxResults"] = json!(max_results);
    }
    inject_branch_warning(&mut summary, ctx);
    let output = json!({
        "files": results,
        "summary": summary
    });

    ToolCallResult::success(json_to_string(&output))
}

#[cfg(test)]
mod fast_unit_tests {
    use super::extract_glob_literal;

    #[test]
    fn test_extract_glob_literal_no_glob() {
        assert_eq!(extract_glob_literal("Order"), "Order");
        assert_eq!(extract_glob_literal("UserService.cs"), "UserService.cs");
        assert_eq!(extract_glob_literal(""), "");
    }

    #[test]
    fn test_extract_glob_literal_star_suffix() {
        assert_eq!(extract_glob_literal("Order*"), "Order");
        assert_eq!(extract_glob_literal("User*"), "User");
    }

    #[test]
    fn test_extract_glob_literal_star_prefix() {
        assert_eq!(extract_glob_literal("*Helper"), "");
        assert_eq!(extract_glob_literal("*Helper*"), "");
    }

    #[test]
    fn test_extract_glob_literal_question_mark() {
        assert_eq!(extract_glob_literal("Use?Service"), "Use");
        assert_eq!(extract_glob_literal("?Service"), "");
    }

    #[test]
    fn test_extract_glob_literal_mixed() {
        assert_eq!(extract_glob_literal("Order*.cs"), "Order");
        assert_eq!(extract_glob_literal("Config*Helper?"), "Config");
    }
}