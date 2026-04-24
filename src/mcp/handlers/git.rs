//! MCP handlers for git history tools.
//!
//! Thin layer: parse JSON arguments → call core git functions → format JSON response.
//!
//! ## Cache-or-fallback routing (PR 2b)
//!
//! When `ctx.git_cache_ready` is true, handlers query the in-memory
//! [`GitHistoryCache`](crate::git::cache::GitHistoryCache) for sub-millisecond
//! responses. When the cache is not ready (building or disabled), handlers
//! fall back to the existing CLI-based `git log` calls.
//!
//! Exception: `xray_git_diff` always uses CLI (cache has no patch data).

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::git;
use crate::git::cache::GitHistoryCache;
use crate::mcp::protocol::ToolCallResult;

use super::HandlerContext;
use super::utils::json_to_string;

/// Emit a warning or info field on the response depending on the file's git history state.
///
/// - If the file was never tracked → warning (user likely typed wrong path).
/// - If the file was tracked but is not in current HEAD → info (file was deleted; historical
///   data may still be available from cache or CLI — this is NOT an error).
/// - If the file IS in current HEAD but returned 0 results → nothing added (genuine empty
///   result within the applied filters, e.g., date range).
///
/// Called from ALL 6 validation points across the three handlers (history, authors, activity)
/// to give consistent messaging. See user story 2026-04-17_git-deleted-files-support.md.
fn annotate_empty_git_result(output: &mut Value, repo: &str, path: &str, total_count_label: usize) {
    if path.is_empty() {
        return;
    }
    if git::file_exists_in_current_head(repo, path) {
        // File is tracked right now; empty result is just a filter miss, not a path problem.
        return;
    }
    if git::file_ever_existed_in_git(repo, path) {
        output["info"] = json!(format!(
            "File '{}' is not in current HEAD (deleted or moved). \
             Showing {} historical commit(s). This is NOT an error — xray_git_* tools \
             cover deleted files. Do NOT fall back to raw `git log --all --diff-filter=D`.",
            path, total_count_label
        ));
    } else {
        output["warning"] = json!(format!(
            "File never tracked in git: '{}'. Check the path spelling. \
             If the file was deleted long ago, the exact historical path may differ.",
            path
        ));
    }
}

/// Return tool definitions for all git history tools.
pub(crate) fn git_tool_definitions() -> Vec<crate::mcp::protocol::ToolDefinition> {
    vec![
        crate::mcp::protocol::ToolDefinition {
            name: "xray_git_history".to_string(),
            description: "Get commit history for a specific file in a git repository. Works for BOTH existing AND deleted files (cache covers full branch history; CLI fallback auto-retries without --follow for deleted files). Returns a list of commits that modified the file, with hash, date, author, and message. Use date filters to narrow results. Uses in-memory cache for sub-millisecond responses when available, falls back to git CLI. If the file was deleted from current HEAD, the response includes an 'info' field — this is NOT an error. NEVER fall back to raw `git log --all --diff-filter=D` — this tool covers deleted files directly.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Path to git repository" },
                    "file": { "type": "string", "description": "File path relative to repo root. Works for both currently-tracked AND deleted files." },
                    "from": { "type": "string", "description": "Start date (YYYY-MM-DD, inclusive)" },
                    "to": { "type": "string", "description": "End date (YYYY-MM-DD, inclusive)" },
                    "date": { "type": "string", "description": "Exact date (YYYY-MM-DD), overrides from/to" },
                    "maxResults": { "type": "integer", "description": "Max commits (default: 50, 0=unlimited)" },
                    "author": { "type": "string", "description": "Filter by author name/email (substring, case-insensitive)" },
                    "message": { "type": "string", "description": "Filter by commit message (substring, case-insensitive)" },
                    "noCache": { "type": "boolean", "description": "Bypass cache, query git CLI directly (default: false)" }
                },
                "required": ["repo", "file"]
            }),
        },
        crate::mcp::protocol::ToolDefinition {
            name: "xray_git_diff".to_string(),
            description: "Get commit history with full diff/patch for a specific file. Same as xray_git_history but includes added/removed lines for each commit. Patches are truncated to ~200 lines per commit to manage output size. Always uses git CLI (cache has no patch data).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Path to git repository" },
                    "file": { "type": "string", "description": "File path relative to repo root" },
                    "from": { "type": "string", "description": "Start date (YYYY-MM-DD, inclusive)" },
                    "to": { "type": "string", "description": "End date (YYYY-MM-DD, inclusive)" },
                    "date": { "type": "string", "description": "Exact date (YYYY-MM-DD), overrides from/to" },
                    "maxResults": { "type": "integer", "description": "Max commits (default: 50, 0=unlimited)" },
                    "author": { "type": "string", "description": "Filter by author name/email (substring, case-insensitive)" },
                    "message": { "type": "string", "description": "Filter by commit message (substring, case-insensitive)" }
                },
                "required": ["repo", "file"]
            }),
        },
        crate::mcp::protocol::ToolDefinition {
            name: "xray_git_authors".to_string(),
            description: "Get top authors/contributors for a file or directory, ranked by number of commits. Works for BOTH existing AND deleted files/directories. Shows who changed this path the most, with commit count and date range. For directories, aggregates across all files within (including files that have since been deleted). If no path specified, returns ownership for the entire repo.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Path to git repository" },
                    "path": { "type": "string", "description": "Path to file/directory. For directories, aggregates. If omitted, whole repo." },
                    "file": { "type": "string", "description": "Alias for 'path' (backward compatibility)" },
                    "from": { "type": "string", "description": "Start date (YYYY-MM-DD, inclusive)" },
                    "to": { "type": "string", "description": "End date (YYYY-MM-DD, inclusive)" },
                    "top": { "type": "integer", "description": "Top N authors (default: 10)" },
                    "message": { "type": "string", "description": "Filter by commit message (substring, case-insensitive)" },
                    "noCache": { "type": "boolean", "description": "Bypass cache, query git CLI directly (default: false)" }
                },
                "required": ["repo"]
            }),
        },
        crate::mcp::protocol::ToolDefinition {
            name: "xray_git_blame".to_string(),
            description: "Show author, date, and commit for each line in a given range of a file. Useful for finding who wrote specific code, when it was last changed, and which commit introduced it.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Path to git repository" },
                    "file": { "type": "string", "description": "File path relative to repo root" },
                    "startLine": { "type": "integer", "description": "Start line (1-based, inclusive)" },
                    "endLine": { "type": "integer", "description": "End line (1-based, inclusive). If omitted, only startLine." }
                },
                "required": ["repo", "file", "startLine"]
            }),
        },
        crate::mcp::protocol::ToolDefinition {
            name: "xray_git_activity".to_string(),
            description: "Get activity across files in a repository (or specific directory) for a date range. Returns a map of changed files with their commits. Useful for answering 'what changed this week?' Includes deleted files. Use includeDeleted=true to list ONLY files removed from current HEAD (great for 'what was removed from this module'). Date filters are recommended to keep results manageable.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Path to git repository" },
                    "path": { "type": "string", "description": "Filter by file or directory path. For directories, returns activity for all files within (including files since deleted). If omitted, returns whole-repo activity." },
                    "from": { "type": "string", "description": "Start date (YYYY-MM-DD, inclusive). Recommended." },
                    "to": { "type": "string", "description": "End date (YYYY-MM-DD, inclusive)" },
                    "date": { "type": "string", "description": "Exact date (YYYY-MM-DD), overrides from/to" },
                    "author": { "type": "string", "description": "Filter by author name/email (substring, case-insensitive)" },
                    "message": { "type": "string", "description": "Filter by commit message (substring, case-insensitive)" },
                    "noCache": { "type": "boolean", "description": "Bypass cache, query git CLI directly (default: false)" },
                    "includeDeleted": { "type": "boolean", "description": "If true, restrict results to files that are NOT in current HEAD (i.e., files that were deleted). Useful for 'find deleted files in <dir>'. Uses a single `git ls-files` call for efficiency. Default: false." }
                },
                "required": ["repo"]
            }),
        },
        crate::mcp::protocol::ToolDefinition {
            name: "xray_branch_status".to_string(),
            description: "Shows the current git branch status: branch name, whether it's main/master, how far behind/ahead of remote, uncommitted changes, and how fresh the last fetch is. Call this before investigating production bugs to ensure you're looking at the right code.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Path to git repository" }
                },
                "required": ["repo"]
            }),
        },
    ]
}

/// Dispatch a git tool call to the appropriate handler.
pub(crate) fn dispatch_git_tool(
    ctx: &HandlerContext,
    tool_name: &str,
    arguments: &Value,
) -> ToolCallResult {
    match tool_name {
        "xray_git_history" => handle_git_history(ctx, arguments, false),
        "xray_git_diff" => handle_git_history(ctx, arguments, true),
        "xray_git_authors" => handle_git_authors(ctx, arguments),
        "xray_git_activity" => handle_git_activity(ctx, arguments),
        "xray_git_blame" => handle_git_blame(ctx, arguments),
        "xray_branch_status" => handle_branch_status(ctx, arguments),
        _ => ToolCallResult::error(format!("Unknown git tool: {}", tool_name)),
    }
}

// ─── Date conversion helpers ────────────────────────────────────────

/// GIT-008: parse a positive integer argument with an explicit upper bound.
///
/// The previous pattern `args.get(key).as_u64().unwrap_or(default) as usize`
/// silently truncated on 32-bit targets and accepted absurd values like
/// `top: 10_000_000`, leading to OOM-class allocations downstream. This
/// helper enforces a sane cap and returns a structured error otherwise.
fn parse_bounded_usize(
    args: &Value,
    key: &str,
    default: usize,
    max: usize,
) -> Result<usize, String> {
    match args.get(key).and_then(|v| v.as_u64()) {
        Some(v) => {
            let v_usize = usize::try_from(v)
                .map_err(|_| format!("{key} must be 0..={} (got {v})", max))?;
            if v_usize > max {
                return Err(format!("{key} must be 0..={} (got {v})", max));
            }
            Ok(v_usize)
        }
        None => Ok(default),
    }
}

/// Convert YYYY-MM-DD to Unix timestamp (start of day, 00:00:00 UTC).
///
/// Uses Howard Hinnant's `days_from_civil` algorithm for correct date math.
fn date_str_to_timestamp_start(date: &str) -> Result<i64, String> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("Invalid date format '{}': expected YYYY-MM-DD", date));
    }
    let y: i64 = parts[0].parse().map_err(|_| format!("Invalid year in '{}'", date))?;
    let m: i64 = parts[1].parse().map_err(|_| format!("Invalid month in '{}'", date))?;
    let d: i64 = parts[2].parse().map_err(|_| format!("Invalid day in '{}'", date))?;

    // GIT-003: validate calendar ranges. Without this the Howard Hinnant
    // arithmetic happily accepts e.g. `2026-99-99` and produces a wild
    // timestamp; the date filter then "matches" zero commits and the user
    // sees an empty result with no idea why. Validate month, day, and
    // day-of-month against month length (handles Feb-29 correctly).
    if !(1..=12).contains(&m) {
        return Err(format!("Invalid month {} in '{}': expected 1..=12", m, date));
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in_month: i64 = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if leap { 29 } else { 28 },
        _ => unreachable!(),
    };
    if d < 1 || d > days_in_month {
        return Err(format!(
            "Invalid day {} in '{}': month {} has {} days",
            d, date, m, days_in_month
        ));
    }

    // Howard Hinnant's civil_from_days (days since 1970-01-01)
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = (y_adj - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as u32 + 2) / 5 + d as u32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe as i64 - 719468;

    Ok(days * 86400)
}

/// Convert YYYY-MM-DD to Unix timestamp (end of day, 23:59:59 UTC).
fn date_str_to_timestamp_end(date: &str) -> Result<i64, String> {
    date_str_to_timestamp_start(date).map(|ts| ts + 86399)
}

/// Parse from/to/date strings into Option<i64> timestamps for cache queries.
/// `date` overrides `from`/`to` (single-day filter).
/// Returns error if `from` date is after `to` date (BUG-4).
fn parse_cache_date_range(
    from: Option<&str>,
    to: Option<&str>,
    date: Option<&str>,
) -> Result<(Option<i64>, Option<i64>), String> {
    if let Some(d) = date {
        let start = date_str_to_timestamp_start(d)?;
        let end = date_str_to_timestamp_end(d)?;
        Ok((Some(start), Some(end)))
    } else {
        let from_ts = match from {
            Some(f) => Some(date_str_to_timestamp_start(f)?),
            None => None,
        };
        let to_ts = match to {
            Some(t) => Some(date_str_to_timestamp_end(t)?),
            None => None,
        };
        // Validate from <= to (BUG-4: reversed date range silently returned 0 results)
        if let (Some(f), Some(t)) = (from_ts, to_ts)
            && f > t {
                return Err(format!(
                    "'from' date ({}) is after 'to' date ({}). Swap them or correct the range.",
                    from.unwrap_or("?"), to.unwrap_or("?")
                ));
            }
        Ok((from_ts, to_ts))
    }
}

/// Format a Unix timestamp as "YYYY-MM-DD HH:MM:SS +0000" (UTC).
///
/// Matches git's `%ai` format for consistent output.
fn format_timestamp(ts: i64) -> String {
    let secs_per_day: i64 = 86400;
    let days = if ts >= 0 { ts / secs_per_day } else { (ts - secs_per_day + 1) / secs_per_day };
    let time_of_day = ts - days * secs_per_day;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Reverse of days_from_civil: convert days since epoch to YYYY-MM-DD
    let days = days + 719468; // shift to 0000-03-01 epoch
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} +0000", y, m, d, hours, minutes, seconds)
}

// ─── Individual handlers ────────────────────────────────────────────

/// Handle xray_git_history and xray_git_diff (shared logic, diff controlled by `include_diff`).
fn handle_git_history(ctx: &HandlerContext, args: &Value, include_diff: bool) -> ToolCallResult {
    let repo = match args.get("repo").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return ToolCallResult::error("Missing required parameter: repo".to_string()),
    };
    let file = match args.get("file").and_then(|v| v.as_str()) {
        Some(f) => f,
        None => return ToolCallResult::error("Missing required parameter: file".to_string()),
    };

    // Detect root-level queries and redirect to xray_git_activity
    if file == "." || file.is_empty() {
        return ToolCallResult::error(
            "xray_git_history requires a specific file path, not '.'. \
             Use xray_git_activity for repo-wide commit history across all files.".to_string()
        );
    }

    let from = args.get("from").and_then(|v| v.as_str());
    let to = args.get("to").and_then(|v| v.as_str());
    let date = args.get("date").and_then(|v| v.as_str());
    // GIT-008: cap maxResults at 1_000_000 (sane upper bound for git log output).
    let max_results = match parse_bounded_usize(args, "maxResults", 50, 1_000_000) {
        Ok(n) => n,
        Err(e) => return ToolCallResult::error(e),
    };
    let author_filter = args.get("author").and_then(|v| v.as_str());
    let message_filter = args.get("message").and_then(|v| v.as_str());
    let no_cache = args.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);

    // ── Cache path (history only, not diff — cache has no patch data) ──
    if !include_diff && !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed)
        && let Ok(cache_guard) = ctx.git_cache.read()
            && let Some(cache) = cache_guard.as_ref() {
                let start = Instant::now();
                let normalized = GitHistoryCache::normalize_path(file);

                let (from_ts, to_ts) = match parse_cache_date_range(from, to, date) {
                    Ok(range) => range,
                    Err(e) => return ToolCallResult::error(e),
                };

                let max = if max_results == 0 { None } else { Some(max_results) };
                let (commits, total_count) = cache.query_file_history(&normalized, max, from_ts, to_ts, author_filter, message_filter);
                let elapsed = start.elapsed();

                let commits_json: Vec<Value> = commits.iter().map(|c| {
                    json!({
                        "hash": c.hash,
                        "date": format_timestamp(c.timestamp),
                        "author": c.author_name,
                        "email": c.author_email,
                        "message": c.subject,
                    })
                }).collect();

                let hint = if total_count > commits_json.len() {
                    "More commits available. Use from/to date filters or increase maxResults. (from cache)"
                } else {
                    "(from cache)"
                };

                let mut output = json!({
                    "commits": commits_json,
                    "summary": {
                        "tool": "xray_git_history",
                        "totalCommits": total_count,
                        "returned": commits_json.len(),
                        "file": file,
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": hint,
                    }
                });

                // Empty results annotation: distinguish 'never existed' vs 'deleted from HEAD'.
                // Files deleted from HEAD may still be in the cache (build() uses --name-only which
                // traverses delete commits) — in that case total_count > 0 here. If total_count == 0,
                // check whether the file ever existed to emit info vs warning. See user story
                // 2026-04-17_git-deleted-files-support.md.
                if total_count == 0 {
                    annotate_empty_git_result(&mut output, repo, file, 0);
                }

                return ToolCallResult::success(json_to_string(&output));
            }

    // ── CLI fallback ──
    let filter = match git::parse_date_filter(from, to, date) {
        Ok(f) => f,
        Err(e) => return ToolCallResult::error(e),
    };

    let start = Instant::now();

    match git::file_history(repo, file, &filter, include_diff, max_results, author_filter, message_filter) {
        Ok((commits, total_count)) => {
            let elapsed = start.elapsed();

            let commits_json: Vec<Value> = commits.iter().map(|c| {
                let mut obj = json!({
                    "hash": c.hash,
                    "date": c.date,
                    "author": c.author_name,
                    "email": c.author_email,
                    "message": c.message,
                });
                if let Some(ref patch) = c.patch {
                    obj["patch"] = json!(patch);
                }
                obj
            }).collect();

            let tool_name = if include_diff { "xray_git_diff" } else { "xray_git_history" };

            let mut output = json!({
                "commits": commits_json,
                "summary": {
                    "tool": tool_name,
                    "totalCommits": total_count,
                    "returned": commits_json.len(),
                    "file": file,
                    "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                    "hint": if total_count > commits_json.len() {
                        "More commits available. Use from/to date filters or increase maxResults."
                    } else {
                        ""
                    }
                }
            });

            // Empty results annotation: distinguish 'never existed' vs 'deleted from HEAD'.
            // The CLI path now auto-retries without --follow for deleted files, so total_count
            // may be > 0 for deleted files — we still only annotate when total_count == 0.
            if total_count == 0 {
                annotate_empty_git_result(&mut output, repo, file, 0);
            }

            ToolCallResult::success(json_to_string(&output))
        }
        Err(e) => ToolCallResult::error(e),
    }
}

/// Handle xray_git_authors.
fn handle_git_authors(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let repo = match args.get("repo").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return ToolCallResult::error("Missing required parameter: repo".to_string()),
    };

    // path takes priority, file is backward-compatible alias
    let query_path = args.get("path").and_then(|v| v.as_str())
        .or_else(|| args.get("file").and_then(|v| v.as_str()))
        .unwrap_or("");

    let from = args.get("from").and_then(|v| v.as_str());
    let to = args.get("to").and_then(|v| v.as_str());
    // GIT-008: cap top at 10_000 (more than enough authors for any repo).
    let top = match parse_bounded_usize(args, "top", 10, 10_000) {
        Ok(n) => n,
        Err(e) => return ToolCallResult::error(e),
    };
    let message_filter = args.get("message").and_then(|v| v.as_str());
    let no_cache = args.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);

    // ── Cache path ──
    if !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed)
        && let Ok(cache_guard) = ctx.git_cache.read()
            && let Some(cache) = cache_guard.as_ref() {
                let start = Instant::now();
                let normalized = GitHistoryCache::normalize_path(query_path);

                let (from_ts, to_ts) = match parse_cache_date_range(from, to, None) {
                    Ok(range) => range,
                    Err(e) => return ToolCallResult::error(e),
                };

                let mut authors = cache.query_authors(&normalized, None, message_filter, from_ts, to_ts);
                let total_authors = authors.len();

                // Compute total commits across all authors
                let total_commits: usize = authors.iter().map(|a| a.commit_count).sum();

                // Truncate to top N
                authors.truncate(top);
                let elapsed = start.elapsed();

                let authors_json: Vec<Value> = authors.iter().enumerate().map(|(i, a)| {
                    json!({
                        "rank": i + 1,
                        "name": a.name,
                        "email": a.email,
                        "commits": a.commit_count,
                        "firstChange": format_timestamp(a.first_commit_timestamp),
                        "lastChange": format_timestamp(a.last_commit_timestamp),
                    })
                }).collect();

                let mut output = json!({
                    "authors": authors_json,
                    "summary": {
                        "tool": "xray_git_authors",
                        "totalCommits": total_commits,
                        "totalAuthors": total_authors,
                        "returned": authors_json.len(),
                        "path": query_path,
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": "(from cache)"
                    }
                });

                // Empty results annotation: distinguish 'never existed' vs 'deleted from HEAD'.
                if total_authors == 0 {
                    annotate_empty_git_result(&mut output, repo, query_path, 0);
                }

                return ToolCallResult::success(json_to_string(&output));
            }

    // ── CLI fallback ──
    let filter = match git::parse_date_filter(from, to, None) {
        Ok(f) => f,
        Err(e) => return ToolCallResult::error(e),
    };

    let start = Instant::now();

    match git::top_authors(repo, query_path, &filter, top, message_filter) {
        Ok((authors, total_commits, total_authors)) => {
            let elapsed = start.elapsed();

            let authors_json: Vec<Value> = authors.iter().enumerate().map(|(i, a)| {
                json!({
                    "rank": i + 1,
                    "name": a.name,
                    "email": a.email,
                    "commits": a.commit_count,
                    "firstChange": a.first_change,
                    "lastChange": a.last_change,
                })
            }).collect();

            let mut output = json!({
                "authors": authors_json,
                "summary": {
                    "tool": "xray_git_authors",
                    "totalCommits": total_commits,
                    "totalAuthors": total_authors,
                    "returned": authors_json.len(),
                    "path": query_path,
                    "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                }
            });

            // Empty results annotation: distinguish 'never existed' vs 'deleted from HEAD'.
            if total_authors == 0 {
                annotate_empty_git_result(&mut output, repo, query_path, 0);
            }

            ToolCallResult::success(json_to_string(&output))
        }
        Err(e) => ToolCallResult::error(e),
    }
}

/// Handle xray_git_activity.
fn handle_git_activity(ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let repo = match args.get("repo").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return ToolCallResult::error("Missing required parameter: repo".to_string()),
    };

    let from = args.get("from").and_then(|v| v.as_str());
    let to = args.get("to").and_then(|v| v.as_str());
    let date = args.get("date").and_then(|v| v.as_str());
    let author_filter = args.get("author").and_then(|v| v.as_str());
    let message_filter = args.get("message").and_then(|v| v.as_str());
    let no_cache = args.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);
    let include_deleted = args.get("includeDeleted").and_then(|v| v.as_bool()).unwrap_or(false);

    // ── Cache path ──
    if !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed)
        && let Ok(cache_guard) = ctx.git_cache.read()
            && let Some(cache) = cache_guard.as_ref() {
                let start = Instant::now();

                // For activity, use empty string for whole-repo scope
                let query_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let normalized = GitHistoryCache::normalize_path(query_path);

                let (from_ts, to_ts) = match parse_cache_date_range(from, to, date) {
                    Ok(range) => range,
                    Err(e) => return ToolCallResult::error(e),
                };

                let mut activities = cache.query_activity(&normalized, from_ts, to_ts, author_filter, message_filter);

                // includeDeleted filter: keep only files NOT in current HEAD.
                // MUST use single git ls-files call — see user story 2026-04-17 section on
                // performance invariant. A per-file `file_exists_in_current_head` on a 5000-file
                // directory in a 200K-file repo would take 75-225 seconds; the single ls-files
                // call runs in 200-700ms even on huge repos (reads only .git/index).
                if include_deleted {
                    let tracked = git::list_tracked_files_under(repo, query_path);
                    activities.retain(|a| !tracked.contains(&a.file_path));
                }

                let elapsed = start.elapsed();

                let total_files = activities.len();
                let total_entries: usize = activities.iter().map(|a| a.commit_count).sum();

                let files_array: Vec<Value> = activities.iter().map(|a| {
                    json!({
                        "path": a.file_path,
                        "commitCount": a.commit_count,
                        "lastModified": format_timestamp(a.last_modified),
                        "authors": a.authors,
                    })
                }).collect();

                let mut output = json!({
                    "activity": files_array,
                    "summary": {
                        "tool": "xray_git_activity",
                        "filesChanged": total_files,
                        "totalEntries": total_entries,
                        "commitsProcessed": cache.commits.len(),
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": if include_deleted {
                            "(from cache, filtered to files NOT in current HEAD)"
                        } else {
                            "(from cache)"
                        },
                        "includeDeleted": include_deleted
                    }
                });

                // Empty results annotation: distinguish 'never existed' vs 'deleted from HEAD'.
                if total_files == 0 {
                    annotate_empty_git_result(&mut output, repo, query_path, 0);
                }

                return ToolCallResult::success(json_to_string(&output));
            }

    // ── CLI fallback ──
    let filter = match git::parse_date_filter(from, to, date) {
        Ok(f) => f,
        Err(e) => return ToolCallResult::error(e),
    };

    let start = Instant::now();

    let activity_path = args.get("path").and_then(|v| v.as_str());

    match git::repo_activity(repo, &filter, author_filter, message_filter, activity_path) {
        Ok((file_map, commits_processed)) => {
            let elapsed = start.elapsed();

            // includeDeleted filter (CLI path): same performance invariant as cache path —
            // ONE git ls-files call, not N per-file checks. See user story 2026-04-17.
            let tracked_set = if include_deleted {
                Some(git::list_tracked_files_under(repo, activity_path.unwrap_or("")))
            } else {
                None
            };

            // Convert to array format for truncation compatibility
            let mut files_array: Vec<Value> = file_map.iter().filter_map(|(path, commits)| {
                if let Some(ref tracked) = tracked_set
                    && tracked.contains(path) {
                        return None; // file is in current HEAD, skip when includeDeleted=true
                    }
                Some((path, commits))
            }).map(|(path, commits)| {
                let commits_json: Vec<Value> = commits.iter().map(|c| {
                    json!({
                        "hash": &c.hash[..12.min(c.hash.len())],
                        "date": c.date,
                        "author": c.author_name,
                        "message": c.message,
                    })
                }).collect();
                json!({
                    "path": path,
                    "commits": commits_json,
                    "commitCount": commits_json.len(),
                })
            }).collect();

            // Sort by commit count descending (most active files first)
            files_array.sort_by(|a, b| {
                let ca = a["commitCount"].as_u64().unwrap_or(0);
                let cb = b["commitCount"].as_u64().unwrap_or(0);
                cb.cmp(&ca)
            });

            let total_files = files_array.len();
            // includeDeleted=true filters files_array down to deleted-only files; totalEntries
            // must be derived from the SAME filtered dataset, not the unfiltered file_map,
            // otherwise summary.totalEntries reports commits for files no longer in activity[]
            // (cache path already does this; CLI path was inconsistent).
            let total_entries: usize = files_array.iter()
                .map(|f| f["commitCount"].as_u64().unwrap_or(0) as usize)
                .sum();

            let activity_path_str = activity_path.unwrap_or("");

            let mut output = json!({
                "activity": files_array,
                "summary": {
                    "tool": "xray_git_activity",
                    "filesChanged": total_files,
                    "totalEntries": total_entries,
                    "commitsProcessed": commits_processed,
                    "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                    "hint": if include_deleted {
                        "Filtered to files NOT in current HEAD (includeDeleted=true)."
                    } else if from.is_none() && to.is_none() && date.is_none() {
                        "No date filter applied. Use from/to to narrow results for large repos."
                    } else {
                        ""
                    },
                    "includeDeleted": include_deleted
                }
            });

            // Empty results annotation: distinguish 'never existed' vs 'deleted from HEAD'.
            if total_files == 0 {
                annotate_empty_git_result(&mut output, repo, activity_path_str, 0);
            }

            ToolCallResult::success(json_to_string(&output))
        }
        Err(e) => ToolCallResult::error(e),
    }
}
/// Handle xray_git_blame — always uses CLI (no cache for blame data).
fn handle_git_blame(_ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let repo = match args.get("repo").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return ToolCallResult::error("Missing required parameter: repo".to_string()),
    };
    let file = match args.get("file").and_then(|v| v.as_str()) {
        Some(f) => f,
        None => return ToolCallResult::error("Missing required parameter: file".to_string()),
    };

    // Detect root-level queries — blame requires a specific file
    if file == "." || file.is_empty() {
        return ToolCallResult::error(
            "xray_git_blame requires a specific file path, not '.'. \
             Use xray_git_activity for repo-wide commit history.".to_string()
        );
    }
    // GIT-008: cap line numbers at 10_000_000 (10x the largest reasonable file).
    const MAX_BLAME_LINE: u64 = 10_000_000;
    let start_line = match args.get("startLine").and_then(|v| v.as_u64()) {
        Some(n) if (1..=MAX_BLAME_LINE).contains(&n) => n as usize,
        Some(n) if n < 1 => return ToolCallResult::error("startLine must be >= 1".to_string()),
        Some(n) => return ToolCallResult::error(format!("startLine must be <= {} (got {})", MAX_BLAME_LINE, n)),
        None => return ToolCallResult::error("Missing required parameter: startLine".to_string()),
    };
    let end_line = match args.get("endLine").and_then(|v| v.as_u64()) {
        Some(n) if n <= MAX_BLAME_LINE => Some(n as usize),
        Some(n) => return ToolCallResult::error(format!("endLine must be <= {} (got {})", MAX_BLAME_LINE, n)),
        None => None,
    };

    // Validate endLine >= startLine if provided
    if let Some(end) = end_line
        && end < start_line {
            return ToolCallResult::error(format!(
                "endLine ({}) must be >= startLine ({})", end, start_line
            ));
        }

    let start = Instant::now();

    match git::blame_lines(repo, file, start_line, end_line) {
        Ok(blame_lines) => {
            let elapsed = start.elapsed();
            let effective_end = end_line.unwrap_or(start_line);

            // Collect unique authors and commits
            let mut unique_authors: Vec<&str> = blame_lines.iter().map(|b| b.author_name.as_str()).collect();
            unique_authors.sort();
            unique_authors.dedup();

            let mut unique_commits: Vec<&str> = blame_lines.iter().map(|b| b.hash.as_str()).collect();
            unique_commits.sort();
            unique_commits.dedup();

            // Find oldest and newest dates
            let oldest = blame_lines.iter().map(|b| &b.date).min().cloned().unwrap_or_default();
            let newest = blame_lines.iter().map(|b| &b.date).max().cloned().unwrap_or_default();

            let blame_json: Vec<Value> = blame_lines.iter().map(|b| {
                json!({
                    "line": b.line,
                    "hash": b.hash,
                    "author": b.author_name,
                    "email": b.author_email,
                    "date": b.date,
                    "content": b.content,
                })
            }).collect();

            let output = json!({
                "blame": blame_json,
                "summary": {
                    "tool": "xray_git_blame",
                    "file": file,
                    "lineRange": format!("{}-{}", start_line, effective_end),
                    "uniqueAuthors": unique_authors.len(),
                    "uniqueCommits": unique_commits.len(),
                    "oldestLine": oldest.split(' ').next().unwrap_or(""),
                    "newestLine": newest.split(' ').next().unwrap_or(""),
                    "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                }
            });

            ToolCallResult::success(json_to_string(&output))
        }
        Err(e) => ToolCallResult::error(e),
    }
}

// ─── Branch status handler ──────────────────────────────────────────

/// Handle xray_branch_status — shows current branch, ahead/behind, dirty files, fetch age.
fn handle_branch_status(_ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let repo = match args.get("repo").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return ToolCallResult::error("Missing required parameter: repo".to_string()),
    };

    let start = Instant::now();

    // a. Current branch
    let current_branch = match run_git_command(repo, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(b) => b,
        Err(e) => return ToolCallResult::error(format!("Failed to get current branch: {}", e)),
    };

    // b. Is main branch
    let is_main = is_main_branch(&current_branch);

    // c. Determine main branch name
    let main_branch = detect_main_branch_name(repo);

    // d. Behind/ahead of main
    let (behind_main, ahead_of_main) = if let Some(ref mb) = main_branch {
        compute_behind_ahead(repo, mb)
    } else {
        (None, None)
    };

    // e. Dirty files
    let dirty_files = get_dirty_files(repo);

    // f. Last fetch time
    let (last_fetch_time, fetch_age, fetch_warning) = get_fetch_info(repo);

    // g. Warning
    let warning = build_warning(&current_branch, is_main, &main_branch, behind_main);

    let elapsed = start.elapsed();

    let output = json!({
        "currentBranch": current_branch,
        "isMainBranch": is_main,
        "mainBranch": main_branch,
        "behindMain": behind_main,
        "aheadOfMain": ahead_of_main,
        "dirtyFiles": dirty_files,
        "dirtyFileCount": dirty_files.len(),
        "lastFetchTime": last_fetch_time,
        "fetchAge": fetch_age,
        "fetchWarning": fetch_warning,
        "warning": warning,
        "summary": {
            "tool": "xray_branch_status",
            "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
        }
    });

    ToolCallResult::success(json_to_string(&output))
}

// ─── Branch status helper functions ─────────────────────────────────

/// Run a git command in the given repo directory and return trimmed stdout.
fn run_git_command(repo: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Check if a branch name is main or master.
pub(crate) fn is_main_branch(branch: &str) -> bool {
    branch == "main" || branch == "master"
}

/// Detect which main branch exists in the repo (main or master).
///
/// Falls back to remote refs (`origin/main`, `origin/master`) for fresh clones
/// and CI checkouts where the local branch hasn't been created yet.
fn detect_main_branch_name(repo: &str) -> Option<String> {
    if run_git_command(repo, &["rev-parse", "--verify", "main"]).is_ok()
        || run_git_command(repo, &["rev-parse", "--verify", "refs/remotes/origin/main"]).is_ok()
    {
        Some("main".to_string())
    } else if run_git_command(repo, &["rev-parse", "--verify", "master"]).is_ok()
        || run_git_command(repo, &["rev-parse", "--verify", "refs/remotes/origin/master"]).is_ok()
    {
        Some("master".to_string())
    } else {
        None
    }
}

/// Compute how far behind/ahead the current HEAD is relative to origin/<main_branch>.
/// Returns (behind, ahead). Both are None if the remote ref doesn't exist.
fn compute_behind_ahead(repo: &str, main_branch: &str) -> (Option<u64>, Option<u64>) {
    let remote_ref = format!("origin/{}", main_branch);
    match run_git_command(repo, &["rev-list", "--left-right", "--count", &format!("HEAD...{}", remote_ref)]) {
        Ok(output) => {
            // Output format: "3\t47" where 3=ahead, 47=behind
            let parts: Vec<&str> = output.split('\t').collect();
            if parts.len() == 2 {
                let ahead = parts[0].trim().parse::<u64>().ok();
                let behind = parts[1].trim().parse::<u64>().ok();
                (behind, ahead)
            } else {
                (None, None)
            }
        }
        Err(_) => (None, None),
    }
}

/// Get list of dirty (uncommitted) files via `git status --porcelain`.
fn get_dirty_files(repo: &str) -> Vec<String> {
    match run_git_command(repo, &["status", "--porcelain"]) {
        Ok(output) => {
            if output.is_empty() {
                Vec::new()
            } else {
                output
                    .lines()
                    .map(|line| {
                        // git status --porcelain format: "XY filename" (first 3 chars are status + space)
                        if line.len() > 3 { line[3..].to_string() } else { line.to_string() }
                    })
                    .collect()
            }
        }
        Err(_) => Vec::new(),
    }
}

/// Get fetch info: ISO timestamp, human-readable age, and warning if stale.
fn get_fetch_info(repo: &str) -> (Option<String>, Option<String>, Option<String>) {
    let fetch_head = Path::new(repo).join(".git").join("FETCH_HEAD");
    match std::fs::metadata(&fetch_head) {
        Ok(meta) => {
            match meta.modified() {
                Ok(modified) => {
                    let now = SystemTime::now();
                    let age_secs = now.duration_since(modified).unwrap_or_default().as_secs();

                    // ISO timestamp
                    let since_epoch = modified.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                    let iso_time = format_timestamp(since_epoch as i64);

                    // Human-readable age
                    let age_str = format_age(age_secs);

                    // Warning based on thresholds
                    let warning = compute_fetch_warning(age_secs, &age_str);

                    (Some(iso_time), Some(age_str), warning)
                }
                Err(_) => (None, None, None),
            }
        }
        Err(_) => (None, None, Some("No FETCH_HEAD found. Run: git fetch origin".to_string())),
    }
}

/// Format age in seconds as a human-readable string.
pub(crate) fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{} seconds ago", secs)
    } else if secs < 3600 {
        let mins = secs / 60;
        if mins == 1 { "1 minute ago".to_string() } else { format!("{} minutes ago", mins) }
    } else if secs < 86400 {
        let hours = secs / 3600;
        if hours == 1 { "1 hour ago".to_string() } else { format!("{} hours ago", hours) }
    } else {
        let days = secs / 86400;
        if days == 1 { "1 day ago".to_string() } else { format!("{} days ago", days) }
    }
}

/// Compute fetch warning based on age thresholds.
pub(crate) fn compute_fetch_warning(age_secs: u64, age_str: &str) -> Option<String> {
    if age_secs < 3600 {
        // < 1 hour
        None
    } else if age_secs < 86400 {
        // 1-24 hours
        Some(format!("Last fetch: {}", age_str))
    } else if age_secs < 604800 {
        // 1-7 days
        Some(format!("Last fetch: {}. Remote data may be outdated.", age_str))
    } else {
        // > 7 days
        Some(format!("Last fetch: {}! Recommend: git fetch origin", age_str))
    }
}

/// Build a human-readable warning string for the branch status.
pub(crate) fn build_warning(
    current_branch: &str,
    is_main: bool,
    main_branch: &Option<String>,
    behind_main: Option<u64>,
) -> Option<String> {
    if is_main {
        // On main/master — warn only if behind
        match behind_main {
            Some(behind) if behind > 0 => {
                Some(format!(
                    "Local {} is {} commits behind remote {}.",
                    current_branch, behind,
                    main_branch.as_deref().unwrap_or(current_branch)
                ))
            }
            _ => None,
        }
    } else {
        // Not on main — build warning
        let mut parts = vec![format!(
            "Index is built on '{}', not on {}.",
            current_branch,
            main_branch.as_deref().unwrap_or("main/master")
        )];
        if let Some(behind) = behind_main
            && behind > 0 {
                parts.push(format!(
                    "Local branch is {} commits behind remote {}.",
                    behind,
                    main_branch.as_deref().unwrap_or("main/master")
                ));
            }
        Some(parts.join(" "))
    }
}

// ─── Unit tests for date conversion and formatting ──────────────────

#[cfg(test)]
#[path = "git_handler_tests.rs"]
mod tests;

