//! xray_fast handler: pre-built file name index search.

use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};
use tracing::info;

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
    let end = pattern.find(['*', '?']).unwrap_or(pattern.len());
    &pattern[..end]
}

/// Parsed arguments for xray_fast.
struct FastParams {
    pattern: String,
    is_wildcard: bool,
    dir: String,
    /// File-extension filter. Empty = no filter. Multi-element = match ANY of
    /// the listed extensions (post 2026-04-25 list-params-to-arrays migration:
    /// `ext: array<string>`). Compared case-insensitive against the file's
    /// extension via `eq_ignore_ascii_case`.
    ext: Vec<String>,
    use_regex: bool,
    ignore_case: bool,
    dirs_only: bool,
    files_only: bool,
    count_only: bool,
    max_depth: Option<usize>,
    max_results: usize,
}

/// Compiled search patterns and ranking data.
struct SearchContext {
    search_terms: Vec<String>,
    ranking_terms: Vec<String>,
    re_list: Option<Vec<regex::Regex>>,
    is_wildcard: bool,
}

/// Parse and validate xray_fast arguments from JSON.
fn parse_fast_args(args: &Value, server_dir: &str) -> Result<FastParams, String> {
    // 2026-04-25 list-params-to-arrays migration:
    // `pattern` and `ext` are array<string> in the schema. Parser reads them via
    // read_string_array (rejects bare-string form with a migration-aware error).
    // `pattern` is then joined into a comma-string for `compile_search_patterns`,
    // which already does `.split(',')` to support OR-of-patterns. `ext` is kept
    // as a Vec because the matching loop in `handle_xray_fast` does ANY-of
    // comparison against `entry.path.extension()` and joining would break
    // multi-element filters (e.g. `["cs","rs"]` -> `"cs,rs"` matches nothing).
    //
    // `pattern` is REQUIRED — distinguish "key absent / null" from "empty array".
    let pattern_present = matches!(args.get("pattern"), Some(v) if !v.is_null());
    if !pattern_present {
        return Err("Missing required parameter: pattern".to_string());
    }
    let pattern_vec = super::utils::read_string_array(args, "pattern")?;

    let dir_provided = args.get("dir").and_then(|v| v.as_str()).is_some();

    // Determine if this is a "list all" (wildcard) request:
    //   pattern=["*"]            → wildcard
    //   pattern=[] + dir set     → wildcard (convenient shortcut)
    //   pattern=[] no dir        → error
    let is_wildcard = match pattern_vec.as_slice() {
        [s] if s == "*" => true,
        [] if dir_provided => true,
        [] => return Err(
            "Empty pattern without dir. Either provide a pattern to search for, \
             or specify dir to list all files in a directory (pattern=[\"*\"] or pattern=[]). \
             Do NOT fall back to built-in list_files or list_directory.".to_string()
        ),
        _ => false,
    };

    let pattern = pattern_vec.join(",");

    let dir = args.get("dir").and_then(|v| v.as_str())
        .map(|s| super::utils::resolve_dir_to_absolute(s, server_dir))
        .unwrap_or_else(|| server_dir.to_string());
    let ext_vec = super::utils::read_string_array(args, "ext")?;
    let ext = ext_vec;
    let use_regex = args.get("regex").and_then(|v| v.as_bool()).unwrap_or(false);
    let ignore_case = args.get("ignoreCase").and_then(|v| v.as_bool()).unwrap_or(false);
    let dirs_only = args.get("dirsOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let files_only = args.get("filesOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    // B1 fix: Reject mutually exclusive flags early
    if dirs_only && files_only {
        return Err(
            "filesOnly and dirsOnly are mutually exclusive. Use one or neither.".to_string()
        );
    }
    let count_only = args.get("countOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_depth = args.get("maxDepth").and_then(|v| v.as_u64()).map(|d| d as usize);
    let max_results = args.get("maxResults").and_then(|v| v.as_u64()).map(|d| d as usize).unwrap_or(0);

    Ok(FastParams {
        pattern, is_wildcard, dir, ext, use_regex, ignore_case,
        dirs_only, files_only, count_only, max_depth, max_results,
    })
}

/// Compile search patterns: split terms, auto-detect globs, build regexes, prepare ranking terms.
fn compile_search_patterns(params: &FastParams) -> Result<SearchContext, String> {
    // Split comma-separated patterns into multiple terms for OR matching
    let terms: Vec<String> = params.pattern
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    // Guard: if pattern is "*" and regex=true, treat as wildcard ("*" is invalid regex)
    let use_regex = if params.is_wildcard && params.use_regex { false } else { params.use_regex };

    // Save original terms for ranking before potential glob→regex conversion.
    // `extract_glob_literal` will pull the fixed prefix out later.
    let original_terms = terms.clone();

    // Auto-detect glob patterns (*, ?) and convert to regex
    // Only when not already in regex mode and not a wildcard-all request
    let (use_regex, terms) = if !use_regex && !params.is_wildcard {
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
    let search_terms: Vec<String> = if params.ignore_case {
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
            let pat = if params.ignore_case { format!("(?i){}", t) } else { t.clone() };
            match regex::Regex::new(&pat) {
                Ok(r) => regexes.push(r),
                Err(e) => return Err(format!("Invalid regex '{}': {}", t, e)),
            }
        }
        Some(regexes)
    } else {
        None
    };

    Ok(SearchContext {
        search_terms,
        ranking_terms,
        re_list,
        is_wildcard: params.is_wildcard,
    })
}

/// Sort, truncate, compute fileCount, and build the final JSON response.
fn format_and_sort_results(
    mut results: Vec<Value>,
    match_count: usize,
    params: &FastParams,
    search: &SearchContext,
    index_entries: &[crate::FileEntry],
    elapsed: std::time::Duration,
    ctx: &HandlerContext,
) -> Value {
    // ── Single-pass fileCount via HashMap (FAST-002) ──
    // The previous implementation rescanned `index_entries` once per matched
    // directory (O(matched × N)). For the documented use-case
    // `pattern='*' dirsOnly=true` on a 10k-dir / 100k-file repo that becomes
    // ~1G byte-comparisons and freezes the single-threaded MCP loop for
    // multiple seconds. We now build a single `dir → file_count` map in O(N)
    // (each file walks up its parents once) and look up each matched
    // directory in O(1). Memory: one usize per *unique* parent directory
    // (~10k dirs × 16 B ≈ 160 KB on a large repo).
    if params.dirs_only && !params.count_only && !results.is_empty() {
        let mut dir_file_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for entry in index_entries {
            if entry.is_dir {
                continue;
            }
            // Walk every ancestor directory, accumulating one file each.
            let mut path = entry.path.as_str();
            while let Some(slash) = path.rfind('/') {
                path = &path[..slash];
                if path.is_empty() {
                    break;
                }
                *dir_file_counts.entry(path).or_insert(0) += 1;
            }
        }
        for result in &mut results {
            if let Some(dir_path) = result["path"].as_str() {
                let count = dir_file_counts
                    .get(dir_path.trim_end_matches('/'))
                    .copied()
                    .unwrap_or(0);
                result["fileCount"] = json!(count);
            }
        }
    }

    // ── Sorting ──
    // For dirsOnly: sort by fileCount descending (largest modules first)
    if !params.count_only && params.dirs_only {
        results.sort_by(|a, b| {
            let fc_b = b["fileCount"].as_u64().unwrap_or(0);
            let fc_a = a["fileCount"].as_u64().unwrap_or(0);
            fc_b.cmp(&fc_a)
        });
    } else if !params.count_only && !search.is_wildcard {
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

            let tier_a = best_match_tier(stem_a, &search.ranking_terms);
            let tier_b = best_match_tier(stem_b, &search.ranking_terms);
            tier_a.cmp(&tier_b)
                .then_with(|| stem_a.len().cmp(&stem_b.len()))
                .then_with(|| path_a.cmp(path_b))
        });
    }

    // Apply maxResults truncation after sorting
    let truncated = if params.max_results > 0 && results.len() > params.max_results {
        results.truncate(params.max_results);
        true
    } else {
        false
    };

    let mut summary = json!({
        "totalMatches": match_count,
        "totalIndexed": index_entries.len(),
        "searchTimeMs": elapsed.as_secs_f64() * 1000.0,
    });
    // B2 fix: Use hints array to avoid overwriting
    let mut hints: Vec<String> = Vec::new();
    let ext_ignored_for_dirs = params.dirs_only && !params.ext.is_empty();
    if ext_ignored_for_dirs {
        hints.push("ext filter ignored when dirsOnly=true (directories have no file extension)".to_string());
    }
    // Hint when dirsOnly results are likely to be truncated
    if params.dirs_only && match_count > 150 && params.max_depth.is_none() {
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
        summary["maxResults"] = json!(params.max_results);
    }
    inject_branch_warning(&mut summary, ctx);
    json!({
        "files": results,
        "summary": summary
    })
}

pub(crate) fn handle_xray_fast(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let params = match parse_fast_args(args, &ctx.server_dir()) {
        Ok(p) => p,
        Err(e) => return ToolCallResult::error(e),
    };
    let search = match compile_search_patterns(&params) {
        Ok(s) => s,
        Err(e) => return ToolCallResult::error(e),
    };

    let start = Instant::now();

    // Load file index from in-memory cache (with dirty-flag invalidation).
    // If dirty or not yet built → rebuild from filesystem (~35ms for 100K files).
    // Otherwise use cached in-memory index (~0ms).
    let server_dir = ctx.server_dir();
    // Workspace-boundary security gate (MAJOR-14): reject any `dir` that points
    // outside the server's `--dir`. Without this check, `xray_fast` was the only
    // MCP tool that *built and saved* a fresh file-list index for any directory
    // the caller asked for — letting an agent enumerate arbitrary host paths
    // (e.g. `xray_fast { dir: "C:\\Users\\victim" }`) and persist the listing
    // to `%LOCALAPPDATA%\\xray`. All other tools (`xray_grep`, `xray_definitions`,
    // `xray_callers`) reject outside-dir requests via `validate_search_dir`;
    // this brings `xray_fast` to the same boundary.
    //
    // `code_xray::is_path_within` performs a logical-first comparison (matching
    // the indexer's `WalkBuilder::follow_links` view) and falls back to canonical
    // comparison when the input contains `..` segments — so symlinked
    // subdirectories inside the workspace (e.g. `docs/personal -> D:\\Personal\\…`)
    // remain searchable while genuine escapes are rejected.
    if !code_xray::is_path_within(&params.dir, &ctx.canonical_server_dir()) {
        return ToolCallResult::error(format!(
            "Server started with --dir {}. For other directories, start another server instance or use CLI.",
            server_dir
        ));
    }

    // ── Acquire file index without cloning (FAST-003) ──
    // The previous implementation cloned the entire ~100k-entry FileIndex
    // (~8 MB) on every request just so the read guard could be released
    // before the matching loop. The clone alone burned the L2/L3 cache and
    // dominated allocator pressure under a busy LLM client. We now hold the
    // read guard for the lifetime of the per-entry scan: read locks do not
    // block other readers, and the watcher's write-lock contention happens
    // only during rebuild — which we run BEFORE taking the read guard.
    //
    // ── PERF-08: single-flight rebuild via `ensure_file_index` ──
    // Previously this site checked `needs_rebuild` and ran `build_index`
    // inline with no mutual exclusion, so N concurrent cold-start
    // requests each performed a full filesystem walk + 8 MB allocation +
    // disk save in parallel. The helper coordinates exactly one in-flight
    // build via Mutex+Condvar; other waiters block until completion and
    // then read the freshly-built index. See
    // `HandlerContext.file_index_build_gate` for the contract.
    let server_dir_for_build = server_dir.clone();
    let respect_git_exclude = ctx.respect_git_exclude;
    let index_base = ctx.index_base.clone();
    if let Err(e) = super::utils::ensure_file_index(ctx, || {
        info!(dir = %server_dir_for_build, "Building file-list index (dirty or first use)");
        let new_index = crate::build_index(&crate::IndexArgs {
            dir: server_dir_for_build.clone(),
            max_age_hours: 24,
            hidden: false,
            no_ignore: false,
            respect_git_exclude,
            threads: 0,
        })
        .map_err(|e| format!("Failed to build file index: {}", e))?;
        // Save to disk for CLI/other consumers.
        let _ = crate::save_index(&new_index, &index_base);
        Ok(new_index)
    }) {
        return ToolCallResult::error(e);
    }

    let guard = match ctx.file_index.read() {
        Ok(g) => g,
        Err(e) => return ToolCallResult::error(format!("Failed to read file index: {}", e)),
    };
    let index = match guard.as_ref() {
        Some(idx) => idx,
        None => return ToolCallResult::error("File index not available after build".to_string()),
    };

    // When reusing a parent index for a subdirectory request, compute a path prefix
    // to filter entries. Without this, wildcard searches would return ALL entries in
    // the parent index, not just those under the requested dir.
    let subdir_entry_filter: Option<String> = {
        // Use LOGICAL paths (clean_path only, no canonicalize) so that symlinked
        // subdirectories like `docs/personal` → `D:\Personal\…` keep matching the
        // indexer's entries — which are recorded under `<server_dir>/personal/…`,
        // NOT under the symlink target. `params.dir` is already a logical absolute
        // path produced by `resolve_dir_to_absolute` (also symlink-safe).
        let dir_abs = code_xray::clean_path(&params.dir);
        let root_abs = code_xray::clean_path(&index.root);
        let dir_lower = dir_abs.to_lowercase();
        let root_lower = root_abs.to_lowercase();
        // Determine if the request targets the workspace root itself (no subdir
        // filter needed). First try logical equality. Then fall back to canonical
        // equality — only for the equivalence check, never for the filter prefix
        // — to handle the case where `dir` is expressed as `.` or a relative path
        // while `index.root` is an absolute path. Symlinked subdirectories still
        // fall through to the `else` branch (canonical paths differ) and use the
        // LOGICAL `dir_lower` as the filter, which is what matches indexed entries.
        let same_as_root = dir_lower.trim_end_matches('/') == root_lower.trim_end_matches('/')
            || {
                let dir_canon = std::fs::canonicalize(&params.dir).ok();
                let root_canon = std::fs::canonicalize(&index.root).ok();
                matches!((dir_canon, root_canon), (Some(d), Some(r)) if d == r)
            };
        if same_as_root {
            None // dir == root, no filtering needed
        } else {
            Some(format!("{}/", dir_lower.trim_end_matches('/')))
        }
    };

    // ext filter is meaningless for directories (they have no file extension),
    // so we skip it when dirsOnly=true and emit a hint in the response.
    // Empty Vec = no filter; multi-element Vec = match ANY of the listed
    // extensions (case-insensitive).
    let effective_ext: &[String] = if params.dirs_only { &[] } else { params.ext.as_slice() };

    // Compute base depth for maxDepth filtering.
    // When subdir_entry_filter is active (parent index reused for subdirectory),
    // base_depth must be relative to dir, not index.root. Otherwise maxDepth=1
    // would show entries 1 level below root instead of 1 level below dir.
    let base_depth = if params.max_depth.is_some() {
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
        if params.dirs_only && !entry.is_dir { continue; }
        if params.files_only && entry.is_dir { continue; }

        // maxDepth filtering
        if let Some(md) = params.max_depth {
            let entry_depth = entry.path.matches('/').count();
            if entry_depth.saturating_sub(base_depth) > md {
                continue;
            }
        }

        if !effective_ext.is_empty() {
            let path = Path::new(&entry.path);
            let matches_ext = path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|file_ext| {
                    effective_ext.iter().any(|wanted| file_ext.eq_ignore_ascii_case(wanted))
                });
            if !matches_ext { continue; }
        }

        let name = Path::new(&entry.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let search_name = if params.ignore_case { name.to_lowercase() } else { name.to_string() };

        let matched = if search.is_wildcard {
            true // wildcard → everything matches
        } else if let Some(ref regexes) = search.re_list {
            regexes.iter().any(|re| re.is_match(&search_name))
        } else {
            search.search_terms.iter().any(|term| search_name.contains(term.as_str()))
        };

        if matched {
            match_count += 1;
            if !params.count_only {
                if params.dirs_only {
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

    let elapsed = start.elapsed();
    let output = format_and_sort_results(results, match_count, &params, &search, &index.entries, elapsed, ctx);
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

    // ─── P2 Group C: SearchContext / is_wildcard plumbing tests ───
    // Regression tests for the SearchContext refactoring that introduced
    // is_wildcard as an explicit field instead of inferring at every call site.

    use super::{parse_fast_args, compile_search_patterns, format_and_sort_results,
                FastParams, SearchContext};
    use super::super::HandlerContext;
    use crate::FileEntry;
    use serde_json::json;

    /// C1: pattern='*' + dirsOnly=true must propagate is_wildcard through the
    /// FastParams → SearchContext pipeline. A regression here would re-introduce
    /// per-call-site wildcard detection (the very bug the SearchContext refactor
    /// was meant to eliminate).
    #[test]
    fn test_dirs_only_with_wildcard_via_context() {
        let server_dir = std::env::temp_dir().to_string_lossy().to_string();
        let args = json!({"pattern": ["*"], "dirsOnly": true, "dir": server_dir.clone()});
        let params = parse_fast_args(&args, &server_dir).expect("parse_fast_args should accept pattern='*'");
        assert!(params.is_wildcard, "FastParams.is_wildcard must be true for pattern='*'");
        assert!(params.dirs_only, "FastParams.dirs_only must be true");

        let search = compile_search_patterns(&params).expect("compile_search_patterns should succeed");
        assert!(search.is_wildcard,
            "SearchContext.is_wildcard must be true (plumbed from FastParams). \
             Regression: format_and_sort_results would attempt relevance ranking on '*' results.");
        assert!(search.re_list.is_none(),
            "Wildcard pattern must skip regex compilation (no terms to match)");
    }

    /// C2: Mutual exclusion of dirsOnly and filesOnly must be enforced at
    /// parse_fast_args entry. Regression of the B1 fix (added in this branch)
    /// would let both flags through and produce ambiguous results.
    #[test]
    fn test_dirs_only_and_files_only_mutually_exclusive() {
        let server_dir = std::env::temp_dir().to_string_lossy().to_string();
        let args = json!({"pattern": ["x"], "dirsOnly": true, "filesOnly": true});
        let result = parse_fast_args(&args, &server_dir);
        let err = match result {
            Ok(_) => panic!("dirsOnly + filesOnly must be rejected, got Ok"),
            Err(e) => e,
        };
        assert!(err.contains("mutually exclusive"),
            "Error must mention mutual exclusion; got: {}", err);
        assert!(err.contains("filesOnly") && err.contains("dirsOnly"),
            "Error must reference BOTH flag names so the LLM can self-correct; got: {}", err);
    }

    /// C3: maxResults=0 means unlimited. format_and_sort_results must NOT
    /// truncate when params.max_results == 0, and the response must NOT
    /// carry a "truncated" flag in summary.
    #[test]
    fn test_format_and_sort_results_max_results_zero_unlimited() {
        let params = FastParams {
            pattern: "x".to_string(),
            is_wildcard: false,
            dir: ".".to_string(),
            ext: Vec::new(),
            use_regex: false,
            ignore_case: false,
            dirs_only: false,
            files_only: false,
            count_only: false,
            max_depth: None,
            max_results: 0, // unlimited
        };
        let search = SearchContext {
            search_terms: vec!["x".to_string()],
            ranking_terms: vec!["x".to_string()],
            re_list: None,
            is_wildcard: false,
        };
        let results: Vec<serde_json::Value> = (0..100)
            .map(|i| json!({"path": format!("file{}.rs", i), "size": 100u64, "isDir": false}))
            .collect();
        let entries: Vec<FileEntry> = (0..100)
            .map(|i| FileEntry {
                path: format!("file{}.rs", i),
                size: 100,
                modified: 0,
                is_dir: false,
            })
            .collect();
        let ctx = HandlerContext::default();
        let output = format_and_sort_results(
            results, 100, &params, &search, &entries,
            std::time::Duration::from_millis(1), &ctx,
        );
        let files = output["files"].as_array().expect("output must contain files array");
        assert_eq!(files.len(), 100,
            "maxResults=0 must return ALL results without truncation; got {}", files.len());
        assert!(output["summary"].get("truncated").is_none(),
            "summary.truncated must be absent when nothing was truncated; got: {:?}",
            output["summary"]);
        assert!(output["summary"].get("maxResults").is_none(),
            "summary.maxResults must be absent when truncation did not happen");
    }
}