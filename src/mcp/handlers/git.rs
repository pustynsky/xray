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
//! Exception: `search_git_diff` always uses CLI (cache has no patch data).

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::git;
use crate::git::cache::GitHistoryCache;
use crate::mcp::protocol::ToolCallResult;

use super::HandlerContext;
use super::utils::json_to_string;

/// Return tool definitions for all git history tools.
pub(crate) fn git_tool_definitions() -> Vec<crate::mcp::protocol::ToolDefinition> {
    vec![
        crate::mcp::protocol::ToolDefinition {
            name: "search_git_history".to_string(),
            description: "Get commit history for a specific file in a git repository. Returns a list of commits that modified the file, with hash, date, author, and message. Use date filters to narrow results. Uses in-memory cache for sub-millisecond responses when available, falls back to git CLI.".to_string(),
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
                    "message": { "type": "string", "description": "Filter by commit message (substring, case-insensitive)" },
                    "noCache": { "type": "boolean", "description": "Bypass cache, query git CLI directly (default: false)" }
                },
                "required": ["repo", "file"]
            }),
        },
        crate::mcp::protocol::ToolDefinition {
            name: "search_git_diff".to_string(),
            description: "Get commit history with full diff/patch for a specific file. Same as search_git_history but includes added/removed lines for each commit. Patches are truncated to ~200 lines per commit to manage output size. Always uses git CLI (cache has no patch data).".to_string(),
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
            name: "search_git_authors".to_string(),
            description: "Get top authors/contributors for a file or directory, ranked by number of commits. Shows who changed this path the most, with commit count and date range. For directories, aggregates across all files within. If no path specified, returns ownership for the entire repo.".to_string(),
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
            name: "search_git_blame".to_string(),
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
            name: "search_git_activity".to_string(),
            description: "Get activity across ALL files in a repository for a date range. Returns a map of changed files with their commits. Useful for answering 'what changed this week?' Date filters are recommended to keep results manageable.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Path to git repository" },
                    "from": { "type": "string", "description": "Start date (YYYY-MM-DD, inclusive). Recommended." },
                    "to": { "type": "string", "description": "End date (YYYY-MM-DD, inclusive)" },
                    "date": { "type": "string", "description": "Exact date (YYYY-MM-DD), overrides from/to" },
                    "author": { "type": "string", "description": "Filter by author name/email (substring, case-insensitive)" },
                    "message": { "type": "string", "description": "Filter by commit message (substring, case-insensitive)" },
                    "noCache": { "type": "boolean", "description": "Bypass cache, query git CLI directly (default: false)" }
                },
                "required": ["repo"]
            }),
        },
        crate::mcp::protocol::ToolDefinition {
            name: "search_branch_status".to_string(),
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
        "search_git_history" => handle_git_history(ctx, arguments, false),
        "search_git_diff" => handle_git_history(ctx, arguments, true),
        "search_git_authors" => handle_git_authors(ctx, arguments),
        "search_git_activity" => handle_git_activity(ctx, arguments),
        "search_git_blame" => handle_git_blame(ctx, arguments),
        "search_branch_status" => handle_branch_status(ctx, arguments),
        _ => ToolCallResult::error(format!("Unknown git tool: {}", tool_name)),
    }
}

// ─── Date conversion helpers ────────────────────────────────────────

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
        if let (Some(f), Some(t)) = (from_ts, to_ts) {
            if f > t {
                return Err(format!(
                    "'from' date ({}) is after 'to' date ({}). Swap them or correct the range.",
                    from.unwrap_or("?"), to.unwrap_or("?")
                ));
            }
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

/// Handle search_git_history and search_git_diff (shared logic, diff controlled by `include_diff`).
fn handle_git_history(ctx: &HandlerContext, args: &Value, include_diff: bool) -> ToolCallResult {
    let repo = match args.get("repo").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return ToolCallResult::error("Missing required parameter: repo".to_string()),
    };
    let file = match args.get("file").and_then(|v| v.as_str()) {
        Some(f) => f,
        None => return ToolCallResult::error("Missing required parameter: file".to_string()),
    };

    let from = args.get("from").and_then(|v| v.as_str());
    let to = args.get("to").and_then(|v| v.as_str());
    let date = args.get("date").and_then(|v| v.as_str());
    let max_results = args.get("maxResults").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let author_filter = args.get("author").and_then(|v| v.as_str());
    let message_filter = args.get("message").and_then(|v| v.as_str());
    let no_cache = args.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);

    // ── Cache path (history only, not diff — cache has no patch data) ──
    if !include_diff && !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed) {
        if let Ok(cache_guard) = ctx.git_cache.read() {
            if let Some(cache) = cache_guard.as_ref() {
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

                let mut output = json!({
                    "commits": commits_json,
                    "summary": {
                        "tool": "search_git_history",
                        "totalCommits": total_count,
                        "returned": commits_json.len(),
                        "file": file,
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": format!("{} (from cache)",
                            if total_count > commits_json.len() {
                                "More commits available. Use from/to date filters or increase maxResults."
                            } else {
                                ""
                            }
                        ).trim().to_string()
                    }
                });

                // Empty results validation: warn if file doesn't exist in git
                if total_count == 0 {
                    let has_entries = cache.file_commits.contains_key(&normalized);
                    if !has_entries && !git::file_exists_in_git(repo, file) {
                        output["warning"] = json!(format!("File not found in git: {}. Check the path.", file));
                    }
                }

                return ToolCallResult::success(json_to_string(&output));
            }
        }
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

            let tool_name = if include_diff { "search_git_diff" } else { "search_git_history" };

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

            // Empty results validation: warn if file doesn't exist in git
            if total_count == 0 && !git::file_exists_in_git(repo, file) {
                output["warning"] = json!(format!("File not found in git: {}. Check the path.", file));
            }

            ToolCallResult::success(json_to_string(&output))
        }
        Err(e) => ToolCallResult::error(e),
    }
}

/// Handle search_git_authors.
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
    let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let message_filter = args.get("message").and_then(|v| v.as_str());
    let no_cache = args.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);

    // ── Cache path ──
    if !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed) {
        if let Ok(cache_guard) = ctx.git_cache.read() {
            if let Some(cache) = cache_guard.as_ref() {
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
                        "tool": "search_git_authors",
                        "totalCommits": total_commits,
                        "totalAuthors": total_authors,
                        "returned": authors_json.len(),
                        "path": query_path,
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": "(from cache)"
                    }
                });

                // Empty results validation: warn if file/path doesn't exist in git
                if total_authors == 0 && !query_path.is_empty() {
                    let has_entries = cache.file_commits.contains_key(&normalized);
                    if !has_entries && !git::file_exists_in_git(repo, query_path) {
                        output["warning"] = json!(format!("File not found in git: {}. Check the path.", query_path));
                    }
                }

                return ToolCallResult::success(json_to_string(&output));
            }
        }
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
                    "tool": "search_git_authors",
                    "totalCommits": total_commits,
                    "totalAuthors": total_authors,
                    "returned": authors_json.len(),
                    "path": query_path,
                    "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                }
            });

            // Empty results validation: warn if file/path doesn't exist in git
            if total_authors == 0 && !query_path.is_empty() && !git::file_exists_in_git(repo, query_path) {
                output["warning"] = json!(format!("File not found in git: {}. Check the path.", query_path));
            }

            ToolCallResult::success(json_to_string(&output))
        }
        Err(e) => ToolCallResult::error(e),
    }
}

/// Handle search_git_activity.
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

    // ── Cache path ──
    if !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed) {
        if let Ok(cache_guard) = ctx.git_cache.read() {
            if let Some(cache) = cache_guard.as_ref() {
                let start = Instant::now();

                // For activity, use empty string for whole-repo scope
                let query_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let normalized = GitHistoryCache::normalize_path(query_path);

                let (from_ts, to_ts) = match parse_cache_date_range(from, to, date) {
                    Ok(range) => range,
                    Err(e) => return ToolCallResult::error(e),
                };

                let activities = cache.query_activity(&normalized, from_ts, to_ts, author_filter, message_filter);
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
                        "tool": "search_git_activity",
                        "filesChanged": total_files,
                        "totalEntries": total_entries,
                        "commitsProcessed": cache.commits.len(),
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": "(from cache)"
                    }
                });

                // Empty results validation: warn if path doesn't exist in git
                if total_files == 0 && !query_path.is_empty() {
                    let has_entries = cache.file_commits.contains_key(&normalized);
                    if !has_entries && !git::file_exists_in_git(repo, query_path) {
                        output["warning"] = json!(format!("File not found in git: {}. Check the path.", query_path));
                    }
                }

                return ToolCallResult::success(json_to_string(&output));
            }
        }
    }

    // ── CLI fallback ──
    let filter = match git::parse_date_filter(from, to, date) {
        Ok(f) => f,
        Err(e) => return ToolCallResult::error(e),
    };

    let start = Instant::now();

    match git::repo_activity(repo, &filter, author_filter, message_filter) {
        Ok((file_map, commits_processed)) => {
            let elapsed = start.elapsed();

            // Convert to array format for truncation compatibility
            let mut files_array: Vec<Value> = file_map.iter().map(|(path, commits)| {
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
            let total_entries: usize = file_map.values().map(|v| v.len()).sum();

            // Check if a path filter was provided
            let activity_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");

            let mut output = json!({
                "activity": files_array,
                "summary": {
                    "tool": "search_git_activity",
                    "filesChanged": total_files,
                    "totalEntries": total_entries,
                    "commitsProcessed": commits_processed,
                    "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                    "hint": if from.is_none() && to.is_none() && date.is_none() {
                        "No date filter applied. Use from/to to narrow results for large repos."
                    } else {
                        ""
                    }
                }
            });

            // Empty results validation: warn if path doesn't exist in git
            if total_files == 0 && !activity_path.is_empty() && !git::file_exists_in_git(repo, activity_path) {
                output["warning"] = json!(format!("File not found in git: {}. Check the path.", activity_path));
            }

            ToolCallResult::success(json_to_string(&output))
        }
        Err(e) => ToolCallResult::error(e),
    }
}
/// Handle search_git_blame — always uses CLI (no cache for blame data).
fn handle_git_blame(_ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let repo = match args.get("repo").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return ToolCallResult::error("Missing required parameter: repo".to_string()),
    };
    let file = match args.get("file").and_then(|v| v.as_str()) {
        Some(f) => f,
        None => return ToolCallResult::error("Missing required parameter: file".to_string()),
    };
    let start_line = match args.get("startLine").and_then(|v| v.as_u64()) {
        Some(n) if n >= 1 => n as usize,
        Some(_) => return ToolCallResult::error("startLine must be >= 1".to_string()),
        None => return ToolCallResult::error("Missing required parameter: startLine".to_string()),
    };
    let end_line = args.get("endLine").and_then(|v| v.as_u64()).map(|n| n as usize);

    // Validate endLine >= startLine if provided
    if let Some(end) = end_line {
        if end < start_line {
            return ToolCallResult::error(format!(
                "endLine ({}) must be >= startLine ({})", end, start_line
            ));
        }
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
                    "tool": "search_git_blame",
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

/// Handle search_branch_status — shows current branch, ahead/behind, dirty files, fetch age.
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
            "tool": "search_branch_status",
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
fn detect_main_branch_name(repo: &str) -> Option<String> {
    if run_git_command(repo, &["rev-parse", "--verify", "main"]).is_ok() {
        Some("main".to_string())
    } else if run_git_command(repo, &["rev-parse", "--verify", "master"]).is_ok() {
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
        if let Some(behind) = behind_main {
            if behind > 0 {
                parts.push(format!(
                    "Local branch is {} commits behind remote {}.",
                    behind,
                    main_branch.as_deref().unwrap_or("main/master")
                ));
            }
        }
        Some(parts.join(" "))
    }
}

// ─── Unit tests for date conversion and formatting ──────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── date_str_to_timestamp_start tests ────────────────────────────

    #[test]
    fn test_date_epoch() {
        // 1970-01-01 00:00:00 UTC = 0
        assert_eq!(date_str_to_timestamp_start("1970-01-01").unwrap(), 0);
    }

    #[test]
    fn test_date_2024_12_16_start() {
        // 2024-12-16 00:00:00 UTC = 1734307200
        // Verified: 2024-01-01=1704067200, +350 days (leap year) = 1704067200 + 30240000 = 1734307200
        let ts = date_str_to_timestamp_start("2024-12-16").unwrap();
        assert_eq!(ts, 1734307200, "2024-12-16 start should be 1734307200, got {}", ts);
    }

    #[test]
    fn test_date_2024_12_16_end() {
        // 2024-12-16 23:59:59 UTC = 1734393599
        let ts = date_str_to_timestamp_end("2024-12-16").unwrap();
        assert_eq!(ts, 1734393599, "2024-12-16 end should be 1734393599, got {}", ts);
    }

    #[test]
    fn test_date_2025_12_16_start() {
        // 2025-12-16 00:00:00 UTC
        // 2025-01-01 = 1735689600, +349 days (non-leap) = 1735689600 + 30153600 = 1765843200
        let ts = date_str_to_timestamp_start("2025-12-16").unwrap();
        assert_eq!(ts, 1765843200, "2025-12-16 start should be 1765843200, got {}", ts);
    }

    #[test]
    fn test_date_2025_12_16_end() {
        let ts = date_str_to_timestamp_end("2025-12-16").unwrap();
        assert_eq!(ts, 1765929599, "2025-12-16 end should be 1765929599, got {}", ts);
    }

    #[test]
    fn test_date_2025_01_01() {
        // 2025-01-01 00:00:00 UTC = 1735689600
        let ts = date_str_to_timestamp_start("2025-01-01").unwrap();
        assert_eq!(ts, 1735689600, "2025-01-01 start should be 1735689600, got {}", ts);
    }

    #[test]
    fn test_date_2024_02_29_leap_year() {
        // 2024 is a leap year, Feb 29 should be valid
        // 2024-02-29: 2024-01-01=1704067200, +31 (Jan) + 28 (Feb 1-28) = 59 days = 1704067200 + 59*86400 = 1709164800
        let ts = date_str_to_timestamp_start("2024-02-29").unwrap();
        assert_eq!(ts, 1709164800, "2024-02-29 start should be 1709164800, got {}", ts);
    }

    #[test]
    fn test_date_various_known_dates() {
        // 2000-01-01 00:00:00 UTC = 946684800
        assert_eq!(date_str_to_timestamp_start("2000-01-01").unwrap(), 946684800);

        // 2020-03-15 00:00:00 UTC = 1584230400
        assert_eq!(date_str_to_timestamp_start("2020-03-15").unwrap(), 1584230400);
    }

    #[test]
    fn test_date_invalid_format() {
        assert!(date_str_to_timestamp_start("2025-12").is_err());
        assert!(date_str_to_timestamp_start("not-a-date").is_err());
        assert!(date_str_to_timestamp_start("").is_err());
    }

    // ── Commit at 1734370112 should fall within 2024-12-16, NOT 2025-12-16 ──

    #[test]
    fn test_commit_1734370112_is_2024_not_2025() {
        let commit_ts: i64 = 1734370112; // 2024-12-16 17:28:32 UTC

        let start_2024 = date_str_to_timestamp_start("2024-12-16").unwrap();
        let end_2024 = date_str_to_timestamp_end("2024-12-16").unwrap();
        assert!(
            commit_ts >= start_2024 && commit_ts <= end_2024,
            "Commit {} should fall within 2024-12-16 [{}, {}]",
            commit_ts, start_2024, end_2024
        );

        let start_2025 = date_str_to_timestamp_start("2025-12-16").unwrap();
        let end_2025 = date_str_to_timestamp_end("2025-12-16").unwrap();
        assert!(
            commit_ts < start_2025,
            "Commit {} should be BEFORE 2025-12-16 start {} (it's from 2024!)",
            commit_ts, start_2025
        );
        // This proves the commit is from 2024, not 2025
        let _ = end_2025; // suppress unused warning
    }

    // ── parse_cache_date_range tests ─────────────────────────────────

    #[test]
    fn test_parse_cache_date_range_with_date() {
        let (from, to) = parse_cache_date_range(None, None, Some("2024-12-16")).unwrap();
        assert_eq!(from, Some(1734307200));
        assert_eq!(to, Some(1734393599));
    }

    #[test]
    fn test_parse_cache_date_range_with_from_to() {
        let (from, to) = parse_cache_date_range(
            Some("2024-12-15"), Some("2024-12-17"), None
        ).unwrap();
        // from = start of 2024-12-15, to = end of 2024-12-17
        assert_eq!(from, Some(date_str_to_timestamp_start("2024-12-15").unwrap()));
        assert_eq!(to, Some(date_str_to_timestamp_end("2024-12-17").unwrap()));
    }

    #[test]
    fn test_parse_cache_date_range_date_overrides_from_to() {
        // When both date and from/to are provided, date takes precedence
        let (from, to) = parse_cache_date_range(
            Some("2020-01-01"), Some("2020-12-31"), Some("2024-12-16")
        ).unwrap();
        assert_eq!(from, Some(1734307200)); // 2024-12-16 start
        assert_eq!(to, Some(1734393599));   // 2024-12-16 end
    }

    #[test]
    fn test_parse_cache_date_range_no_filters() {
        let (from, to) = parse_cache_date_range(None, None, None).unwrap();
        assert_eq!(from, None);
        assert_eq!(to, None);
    }

    // ── format_timestamp tests ───────────────────────────────────────

    #[test]
    fn test_format_timestamp_epoch() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00 +0000");
    }

    #[test]
    fn test_format_timestamp_known_value() {
        // 1734370112 = 2024-12-16 17:28:32 UTC
        assert_eq!(format_timestamp(1734370112), "2024-12-16 17:28:32 +0000");
    }

    #[test]
    fn test_format_timestamp_start_of_day() {
        assert_eq!(format_timestamp(1734307200), "2024-12-16 00:00:00 +0000");
    }

    #[test]
    fn test_format_timestamp_end_of_day() {
        assert_eq!(format_timestamp(1734393599), "2024-12-16 23:59:59 +0000");
    }

    #[test]
    fn test_format_timestamp_roundtrip() {
        // Start of 2024-12-16 → format → should show 2024-12-16
        let ts = date_str_to_timestamp_start("2024-12-16").unwrap();
        let formatted = format_timestamp(ts);
        assert!(formatted.starts_with("2024-12-16"), "Expected 2024-12-16, got {}", formatted);
    }

    // ── Empty results validation (warning) tests ─────────────────────

    /// Helper: create a minimal HandlerContext for git handler tests.
    /// Uses the current repo directory (".") as the server dir.
    fn make_git_test_ctx() -> super::super::HandlerContext {
        use crate::mcp::handlers::handlers_test_utils::make_ctx_with_defs;
        make_ctx_with_defs()
    }

    #[test]
    fn test_git_history_cli_nonexistent_file_has_warning() {
        let ctx = make_git_test_ctx();
        let args = json!({
            "repo": ".",
            "file": "nonexistent_file_xyz_abc_123.rs"
        });
        let result = handle_git_history(&ctx, &args, false);
        assert!(!result.is_error, "Should succeed even for nonexistent file");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(
            output.get("warning").is_some(),
            "Should have warning for nonexistent file, got: {}",
            serde_json::to_string_pretty(&output).unwrap()
        );
        let warning = output["warning"].as_str().unwrap();
        assert!(
            warning.contains("File not found in git"),
            "Warning should mention 'File not found in git', got: {}",
            warning
        );
    }

    #[test]
    fn test_git_history_cli_existing_file_no_commits_no_warning() {
        // Query with an extremely narrow date range so result is 0 commits,
        // but the file IS tracked in git — no warning expected.
        let ctx = make_git_test_ctx();
        let args = json!({
            "repo": ".",
            "file": "Cargo.toml",
            "from": "1970-01-01",
            "to": "1970-01-02"
        });
        let result = handle_git_history(&ctx, &args, false);
        assert!(!result.is_error, "Should succeed");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(output["summary"]["totalCommits"].as_u64(), Some(0));
        assert!(
            output.get("warning").is_none(),
            "Should NOT have warning when file exists but has no commits in range"
        );
    }

    // ── Branch status tests ──────────────────────────────────────────

    #[test]
    fn test_branch_status_returns_current_branch() {
        let ctx = make_git_test_ctx();
        let args = json!({ "repo": "." });
        let result = handle_branch_status(&ctx, &args);
        assert!(!result.is_error, "Should succeed");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let branch = output["currentBranch"].as_str().unwrap();
        assert!(!branch.is_empty(), "Branch name should not be empty");
    }

    #[test]
    fn test_branch_status_detects_main_branch() {
        let ctx = make_git_test_ctx();
        let args = json!({ "repo": "." });
        let result = handle_branch_status(&ctx, &args);
        assert!(!result.is_error, "Should succeed");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        // mainBranch should be "main" or "master" (one must exist in this repo)
        let main = output["mainBranch"].as_str();
        assert!(
            main == Some("main") || main == Some("master"),
            "mainBranch should be 'main' or 'master', got {:?}",
            main
        );
    }

    #[test]
    fn test_branch_status_dirty_files() {
        let ctx = make_git_test_ctx();
        let args = json!({ "repo": "." });
        let result = handle_branch_status(&ctx, &args);
        assert!(!result.is_error, "Should succeed");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(output["dirtyFiles"].is_array(), "dirtyFiles should be an array");
        let count = output["dirtyFileCount"].as_u64().unwrap();
        let files = output["dirtyFiles"].as_array().unwrap();
        assert_eq!(count as usize, files.len(), "dirtyFileCount should match dirtyFiles length");
    }

    #[test]
    fn test_branch_status_missing_repo() {
        let ctx = make_git_test_ctx();
        let args = json!({});
        let result = handle_branch_status(&ctx, &args);
        assert!(result.is_error, "Should fail with missing repo");
        assert!(
            result.content[0].text.contains("Missing required parameter"),
            "Error should mention missing parameter"
        );
    }

    #[test]
    fn test_branch_status_bad_repo() {
        let ctx = make_git_test_ctx();
        let args = json!({ "repo": "/nonexistent/repo/path/xyz" });
        let result = handle_branch_status(&ctx, &args);
        assert!(result.is_error, "Should fail with bad repo path");
    }

    #[test]
    fn test_branch_status_has_summary() {
        let ctx = make_git_test_ctx();
        let args = json!({ "repo": "." });
        let result = handle_branch_status(&ctx, &args);
        assert!(!result.is_error, "Should succeed");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(output["summary"]["tool"].as_str(), Some("search_branch_status"));
        assert!(output["summary"]["elapsedMs"].as_f64().is_some());
    }

    // ── Helper function unit tests ───────────────────────────────────

    #[test]
    fn test_is_main_branch() {
        assert!(is_main_branch("main"));
        assert!(is_main_branch("master"));
        assert!(!is_main_branch("develop"));
        assert!(!is_main_branch("feature/my-feature"));
        assert!(!is_main_branch("users/dev/test"));
    }

    #[test]
    fn test_format_age() {
        assert_eq!(format_age(30), "30 seconds ago");
        assert_eq!(format_age(60), "1 minute ago");
        assert_eq!(format_age(120), "2 minutes ago");
        assert_eq!(format_age(3600), "1 hour ago");
        assert_eq!(format_age(7200), "2 hours ago");
        assert_eq!(format_age(86400), "1 day ago");
        assert_eq!(format_age(172800), "2 days ago");
    }

    #[test]
    fn test_compute_fetch_warning_thresholds() {
        // < 1 hour: no warning
        assert_eq!(compute_fetch_warning(1800, "30 minutes ago"), None);

        // 1-24 hours: simple message
        let w = compute_fetch_warning(7200, "2 hours ago");
        assert!(w.is_some());
        assert!(w.as_ref().unwrap().contains("Last fetch: 2 hours ago"));
        assert!(!w.as_ref().unwrap().contains("outdated"));

        // 1-7 days: outdated warning
        let w = compute_fetch_warning(259200, "3 days ago");
        assert!(w.is_some());
        assert!(w.as_ref().unwrap().contains("outdated"));

        // > 7 days: recommend fetch
        let w = compute_fetch_warning(1036800, "12 days ago");
        assert!(w.is_some());
        assert!(w.as_ref().unwrap().contains("git fetch origin"));
    }

    #[test]
    fn test_build_warning_on_main_up_to_date() {
        let w = build_warning("main", true, &Some("main".to_string()), Some(0));
        assert!(w.is_none(), "No warning when on main and up-to-date");
    }

    #[test]
    fn test_build_warning_on_main_behind() {
        let w = build_warning("main", true, &Some("main".to_string()), Some(5));
        assert!(w.is_some());
        assert!(w.as_ref().unwrap().contains("5 commits behind"));
    }

    #[test]
    fn test_build_warning_on_feature_branch() {
        let w = build_warning("dev/my-feature", false, &Some("master".to_string()), Some(47));
        assert!(w.is_some());
        let warning = w.unwrap();
        assert!(warning.contains("dev/my-feature"), "Warning should mention branch name");
        assert!(warning.contains("master"), "Warning should mention main branch");
        assert!(warning.contains("47 commits behind"), "Warning should mention behind count");
    }

    #[test]
    fn test_build_warning_on_feature_branch_no_behind() {
        let w = build_warning("dev/my-feature", false, &Some("main".to_string()), Some(0));
        assert!(w.is_some());
        let warning = w.unwrap();
        assert!(warning.contains("dev/my-feature"));
        assert!(!warning.contains("commits behind"), "Should not mention behind when 0");
    }

    #[test]
    fn test_build_warning_on_feature_branch_no_remote() {
        let w = build_warning("dev/my-feature", false, &Some("main".to_string()), None);
        assert!(w.is_some());
        let warning = w.unwrap();
        assert!(warning.contains("dev/my-feature"));
    }

    // ── git_authors file-not-found warning tests ──────────────────────

    #[test]
    fn test_git_authors_nonexistent_file_has_warning() {
        let ctx = make_git_test_ctx();
        let args = json!({
            "repo": ".",
            "file": "nonexistent_file_xyz_abc_123.rs"
        });
        let result = handle_git_authors(&ctx, &args);
        assert!(!result.is_error, "Should succeed even for nonexistent file");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(
            output.get("warning").is_some(),
            "Should have warning for nonexistent file, got: {}",
            serde_json::to_string_pretty(&output).unwrap()
        );
        let warning = output["warning"].as_str().unwrap();
        assert!(
            warning.contains("File not found in git"),
            "Warning should mention 'File not found in git', got: {}",
            warning
        );
    }

    #[test]
    fn test_git_authors_existing_file_no_warning() {
        let ctx = make_git_test_ctx();
        let args = json!({
            "repo": ".",
            "file": "Cargo.toml"
        });
        let result = handle_git_authors(&ctx, &args);
        assert!(!result.is_error, "Should succeed");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(
            output.get("warning").is_none(),
            "Should NOT have warning when file exists in git"
        );
    }

    // ── git_activity file-not-found warning tests ─────────────────────

    #[test]
    fn test_git_activity_nonexistent_path_has_warning() {
        let ctx = make_git_test_ctx();
        let args = json!({
            "repo": ".",
            "path": "nonexistent_dir_xyz_abc_123",
            "from": "1970-01-01",
            "to": "1970-01-02"
        });
        let result = handle_git_activity(&ctx, &args);
        assert!(!result.is_error, "Should succeed even for nonexistent path");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(
            output.get("warning").is_some(),
            "Should have warning for nonexistent path, got: {}",
            serde_json::to_string_pretty(&output).unwrap()
        );
        let warning = output["warning"].as_str().unwrap();
        assert!(
            warning.contains("File not found in git"),
            "Warning should mention 'File not found in git', got: {}",
            warning
        );
    }

    #[test]
    fn test_git_activity_no_path_no_warning() {
        // When no path filter is provided, no warning even if 0 results
        let ctx = make_git_test_ctx();
        let args = json!({
            "repo": ".",
            "from": "1970-01-01",
            "to": "1970-01-02"
        });
        let result = handle_git_activity(&ctx, &args);
        assert!(!result.is_error, "Should succeed");
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(
            output.get("warning").is_none(),
            "Should NOT have warning when no path filter is provided"
        );
    }
}
