//! xray_fast handler: pre-built file name index search.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};
use tracing::info;

use crate::mcp::protocol::ToolCallResult;

use super::HandlerContext;
use super::utils::{best_match_tier, inject_branch_warning, json_to_string};

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

    let dir = args.get("dir").and_then(|v| v.as_str()).unwrap_or(&ctx.server_dir).to_string();
    let ext = args.get("ext").and_then(|v| v.as_str()).map(|s| s.to_string());
    let use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let ignore_case = args.get("ignoreCase").and_then(|v| v.as_bool()).unwrap_or(false);
    let dirs_only = args.get("dirsOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let files_only = args.get("filesOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let count_only = args.get("countOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_depth = args.get("maxDepth").and_then(|v| v.as_u64()).map(|d| d as usize);

    let start = Instant::now();

    // Load file index.
    // Strategy: try exact dir first, then fall back to server_dir's index if dir is
    // a subdirectory. This prevents creating orphan file-list indexes for every
    // subdirectory the LLM explores (bug: each subdir call created a separate index file).
    let index = match crate::load_index(&dir, &ctx.index_base) {
        Ok(idx) => idx,
        Err(_) => {
            // Fallback: if dir is a subdirectory of server_dir, reuse the server_dir's index.
            // The server_dir index contains ALL files including subdirectories.
            let fallback = if dir != ctx.server_dir {
                crate::load_index(&ctx.server_dir, &ctx.index_base).ok().filter(|idx| {
                    let dir_canon = std::fs::canonicalize(&dir)
                        .map(|p| code_xray::clean_path(&p.to_string_lossy()).to_lowercase())
                        .unwrap_or_else(|_| dir.replace('\\', "/").to_lowercase());
                    let root_canon = std::fs::canonicalize(&idx.root)
                        .map(|p| code_xray::clean_path(&p.to_string_lossy()).to_lowercase())
                        .unwrap_or_else(|_| idx.root.replace('\\', "/").to_lowercase());
                    let root_prefix = format!("{}/", root_canon.trim_end_matches('/'));
                    dir_canon.starts_with(&root_prefix) || dir_canon == root_canon
                })
            } else {
                None
            };

            if let Some(parent_idx) = fallback {
                info!(dir = %dir, root = %parent_idx.root, "Reusing server_dir file index for subdirectory");
                parent_idx
            } else {
                // Auto-build (only when no parent index is available)
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
        }
    };

    // When reusing a parent index for a subdirectory request, compute a path prefix
    // to filter entries. Without this, wildcard searches would return ALL entries in
    // the parent index, not just those under the requested dir.
    let subdir_entry_filter: Option<String> = {
        let root_norm = index.root.replace('\\', "/");
        // Resolve dir to absolute path consistent with index entry paths.
        // Must use clean_path to strip Windows \\?\ prefix from canonicalize.
        let dir_abs = if std::path::Path::new(&dir).is_absolute() {
            std::fs::canonicalize(&dir)
                .map(|p| code_xray::clean_path(&p.to_string_lossy()))
                .unwrap_or_else(|_| dir.replace('\\', "/"))
        } else {
            // Relative path — resolve against index root
            let full = format!("{}/{}", root_norm.trim_end_matches('/'), dir.replace('\\', "/").trim_matches('/'));
            std::fs::canonicalize(&full)
                .map(|p| code_xray::clean_path(&p.to_string_lossy()))
                .unwrap_or(full)
        };
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

    // Build file-count-per-directory map (only when dirsOnly + wildcard, not count_only)
    let file_counts: HashMap<&str, usize> = if dirs_only && is_wildcard && !count_only {
        let root_normalized = index.root.replace('\\', "/");
        let dir_normalized = dir.replace('\\', "/");
        let server_dir_normalized = ctx.server_dir.replace('\\', "/");
        // Resolve dir_prefix to match absolute paths in the index.
        // Bug fix: raw `dir` can be relative (e.g. "src") while entry paths are
        // absolute (e.g. "C:/Repos/project/src/..."). Also handles the case
        // where load_index built an index FOR the subdir (root == resolved dir).
        let dir_trimmed = dir_normalized.trim_matches('/');
        let dir_prefix = if dir_normalized == root_normalized
            || dir_normalized == server_dir_normalized
            || dir_normalized == "."
            || root_normalized.ends_with(&format!("/{}", dir_trimmed))
        {
            // dir IS the root of this index (or equivalent) — no filtering
            String::new()
        } else if dir_normalized.starts_with(&root_normalized) {
            // Already absolute path within root
            format!("{}/", dir_normalized)
        } else {
            // Relative path — resolve against index root
            format!(
                "{}/{}/",
                root_normalized.trim_end_matches('/'),
                dir_trimmed
            )
        };
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for entry in &index.entries {
            if entry.is_dir { continue; }
            let path = entry.path.as_str();
            // Only count files under the requested dir
            if !dir_prefix.is_empty() && !path.starts_with(dir_prefix.as_str()) {
                continue;
            }
            // Walk up all ancestor directories and increment their counts
            let mut pos = path.len();
            while let Some(slash) = path[..pos].rfind('/') {
                let ancestor = &path[..slash];
                *counts.entry(ancestor).or_insert(0) += 1;
                pos = slash;
            }
        }
        counts
    } else {
        HashMap::new()
    };

    // Compute base depth for maxDepth filtering.
    // When subdir_entry_filter is active (parent index reused for subdirectory),
    // base_depth must be relative to dir, not index.root. Otherwise maxDepth=1
    // would show entries 1 level below root instead of 1 level below dir.
    let base_depth = if max_depth.is_some() {
        if let Some(ref filter) = subdir_entry_filter {
            // filter = "c:/repos/shared/src/" — count slashes in the dir path (without trailing /)
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
                if dirs_only && is_wildcard {
                    let fc = file_counts.get(entry.path.as_str()).copied().unwrap_or(0);
                    results.push(json!({
                        "path": entry.path,
                        "size": entry.size,
                        "isDir": true,
                        "fileCount": fc,
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

    // ── Sorting ──
    // For wildcard + dirsOnly: sort by fileCount descending (largest modules first)
    if !count_only && is_wildcard && dirs_only {
        results.sort_by(|a, b| {
            let fc_b = b["fileCount"].as_u64().unwrap_or(0);
            let fc_a = a["fileCount"].as_u64().unwrap_or(0);
            fc_b.cmp(&fc_a)
        });
    }
    // Relevance ranking: exact match first, then prefix, then contains
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
    // Hint when dirsOnly results are likely to be truncated
    if dirs_only && match_count > 150 && max_depth.is_none() {
        summary["hint"] = json!(
            "Too many directories. Use maxDepth=1 for immediate children only, \
             or use xray_definitions file='<dir>' for code-level module overview with autoSummary."
        );
    }
    inject_branch_warning(&mut summary, ctx);
    let output = json!({
        "files": results,
        "summary": summary
    });

    ToolCallResult::success(json_to_string(&output))
}