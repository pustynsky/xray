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
use crate::git::cache::{validate_git_ref, GitHistoryCache};
use crate::mcp::protocol::ToolCallResult;

use super::HandlerContext;
use super::utils::{json_to_string, read_required_string, read_string};

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
             If the file was deleted long ago, the exact historical path may differ. \
             Cache validity for empty results is HEAD-pinned: if the cache's snapshot HEAD \
             differs from the live HEAD, the empty result auto-falls-through to a fresh git CLI query.",
            path
        ));
    }
}

/// HEAD-pinning check for cached empty results (user story 2026-05-10).
///
/// Returns `true` when the cache's snapshot HEAD differs from the repo's live
/// `git rev-parse HEAD`. Callers use this only when the cache returned an
/// empty result for the underlying `query_*` call (BEFORE any handler-side
/// post-filtering such as `includeDeleted`). An empty result is only
/// authoritative relative to the HEAD it was built from — if HEAD has moved,
/// new commits may match the query (e.g. file just committed, author's first
/// commit landed, message-pattern's first match landed), so the handler
/// falls through to the CLI fallback for an authoritative answer.
///
/// Returns `false` (i.e. "empty is trustworthy") when:
/// - live HEAD matches the cache snapshot HEAD, OR
/// - `git rev-parse HEAD` fails (bare repo / no commits) — we cannot prove
///   staleness, so we do not invalidate (preserves prior behavior).
///
/// Takes the cache HEAD as `&str` (not `&GitHistoryCache`) so callers can
/// snapshot it and drop the cache read-lock before spawning git.
fn cache_head_stale(cache_head: &str, repo: &str) -> bool {
    match git::current_head_hash(repo) {
        Some(live) => live != cache_head,
        None => false,
    }
}

/// Return tool definitions for all git history tools.
pub(crate) fn git_tool_definitions() -> Vec<crate::mcp::protocol::ToolDefinition> {
    vec![
        crate::mcp::protocol::ToolDefinition {
            name: "xray_git_history".to_string(),
            description: "Get commit history for a specific file in a git repository. Works for BOTH existing AND deleted files (cache covers full branch history; CLI fallback auto-retries without --follow for deleted files). Returns a list of commits that modified the file, with hash, date, author, and message. Result list is unbounded — for files with long history narrow with `from=`/`to=`/`author=`/`message=` or cap with `maxResults=`. Uses in-memory cache for sub-millisecond responses when available, falls back to git CLI. If the file was deleted from current HEAD, the response includes an 'info' field — this is NOT an error. NEVER fall back to raw `git log --all --diff-filter=D` — this tool covers deleted files directly. Set firstCommit=true to return only the commit that introduced the file (uses --follow --diff-filter=A --max-count=1, with no-follow fallback; correct across renames; works for deleted files; bypasses cache and other filters).".to_string(),
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
                    "noCache": { "type": "boolean", "description": "Bypass cache, query git CLI directly (default: false)" },
                    "firstCommit": { "type": "boolean", "description": "If true, return only the commit that CREATED this file (git log --follow --diff-filter=A --max-count=1, with no-follow fallback for deleted files). Correct across renames; works for deleted files. Ignores cache and date/author/message/maxResults filters. Response shape: {firstCommit: {hash,date,author,email,message} | null, summary: {...}}. Default: false." }
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
            description: "Shows branch, exact HEAD, ahead/behind, dirty state, and fetch age. With expectedRef, resolves a local Git object and reports match status. Never fetches or changes the worktree. Use before production investigations or remote reviews.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Path to git repository" },
                    "expectedRef": { "type": "string", "description": "Optional local Git ref, branch, tag, or commit to compare with HEAD. No fetch or checkout." }
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
    let mut result = match tool_name {
        "xray_git_history" => handle_git_history(ctx, arguments, false),
        "xray_git_diff" => handle_git_history(ctx, arguments, true),
        "xray_git_authors" => handle_git_authors(ctx, arguments),
        "xray_git_activity" => handle_git_activity(ctx, arguments),
        "xray_git_blame" => handle_git_blame(ctx, arguments),
        "xray_branch_status" => handle_branch_status(ctx, arguments),
        _ => return ToolCallResult::error(format!("Unknown git tool: {}", tool_name)),
    };
    annotate_shallow_clone(&mut result, arguments);
    result
}

/// Inject a `shallowClone` field into successful git tool responses so
/// downstream LLM users notice when their local git view is truncated.
///
/// Pattern: post-process at the dispatch boundary instead of threading a
/// `repo` argument through every handler's `json!` site. The `repo` arg is
/// always the first parameter of every git tool, so we can extract it from
/// `arguments` without parsing handler internals.
///
/// Behaviour:
/// - No-op when the response is an error or not parseable as JSON.
/// - No-op when the repo is not shallow.
/// - Adds `output["shallowClone"] = { boundaries: [...], warning: "..." }`.
/// - Escalation: if the response carries a `firstCommit` whose hash matches
///   one of the shallow boundaries, marks `shallowClone.firstCommitAtBoundary
///   = true` so callers know the "first commit" is almost certainly NOT the
///   real one (real history is below the graft).
fn annotate_shallow_clone(result: &mut ToolCallResult, arguments: &Value) {
    if result.is_error {
        return;
    }
    let Some(repo) = arguments.get("repo").and_then(|v| v.as_str()) else {
        return;
    };
    let Some(info) = git::detect_shallow(repo) else {
        return;
    };
    let Some(content) = result.content.first_mut() else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<Value>(&content.text) else {
        return;
    };

    let mut shallow = json!({
        "isShallow": true,
        "boundaries": info.boundaries,
        "warning": info.warning_text(),
    });

    // Escalation: firstCommit equals a shallow boundary => almost certainly
    // not the real first commit.
    if let Some(first_hash) = value
        .get("firstCommit")
        .and_then(|fc| fc.get("hash"))
        .and_then(|h| h.as_str())
        && info.boundaries.iter().any(|b| b == first_hash)
    {
        shallow["firstCommitAtBoundary"] = json!(true);
        shallow["warning"] = json!(format!(
            "{} The reported `firstCommit` IS the graft boundary — it is NOT the \
             real first commit, just the oldest one your local clone can see.",
            info.warning_text()
        ));
    }

    if let Value::Object(map) = &mut value {
        map.insert("shallowClone".to_string(), shallow);
    }
    content.text = json_to_string(&value);
}


// ─── Date conversion helpers ────────────────────────────────────────

/// Per-request freshness gate for the in-memory git cache.
///
/// The cache stores a `shallow_fingerprint` snapshot from when it was built.
/// If the live repo state diverges (most commonly: `git fetch --unshallow`
/// removes `.git/shallow` while the server is running), this returns
/// `false` and forces the calling handler to fall through to the `git log`
/// CLI path — which sees current reality by construction.
///
/// HEAD movement is intentionally NOT checked here: the cache's existing
/// HEAD-pinning logic for empty results plus the background watcher-driven
/// rebuild path already cover HEAD drift. Adding a HEAD `rev-parse` per
/// request would cost an extra subprocess on the hot path. Shallow drift
/// detection costs one stat + one read of `.git/shallow` per request
/// (typically <1 KB, ~10–50 µs on Windows NTFS, ~2–10 µs on Linux);
/// `git::shallow_fingerprint` memoises only the resolved file path, never
/// the file contents — see its docstring for the coherency rationale.
fn cache_is_fresh_for_shallow(
    cache: &crate::git::cache::GitHistoryCache,
    repo: &str,
) -> bool {
    let current = crate::git::shallow_fingerprint(repo);
    cache.shallow_fingerprint.as_deref() == current.as_deref()
}


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
    let repo = match read_required_string(args, "repo") {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };
    let file = match read_required_string(args, "file") {
        Ok(f) => f,
        Err(e) => return ToolCallResult::error(e),
    };
    let repo = repo.as_str();
    let file = file.as_str();

    // Detect root-level queries and redirect to xray_git_activity
    if file == "." || file.is_empty() {
        return ToolCallResult::error(
            "xray_git_history requires a specific file path, not '.'. \
             Use xray_git_activity for repo-wide commit history across all files.".to_string()
        );
    }

    // ── firstCommit short-circuit ─────────────────────────────────────────
    // Returns the single commit that introduced the file. Bypasses cache and
    // ALL other filters (date/author/message/maxResults) — semantics is
    // "creation commit, definitionally unique". See git::file_first_commit
    // for the rationale (--diff-filter=A required to disambiguate from
    // "oldest reachable modification").
    let first_commit = args.get("firstCommit").and_then(|v| v.as_bool()).unwrap_or(false);
    if first_commit {
        if include_diff {
            return ToolCallResult::error(
                "firstCommit is not supported by xray_git_diff (no patch is returned). \
                 Use xray_git_history with firstCommit=true.".to_string()
            );
        }
        let start = Instant::now();
        return match git::file_first_commit(repo, file) {
            Ok(Some(c)) => {
                let elapsed = start.elapsed();
                let output = json!({
                    "firstCommit": {
                        "hash": c.hash,
                        "date": c.date,
                        "author": c.author_name,
                        "email": c.author_email,
                        "message": c.message,
                    },
                    "summary": {
                        "tool": "xray_git_history",
                        "mode": "firstCommit",
                        "file": file,
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": "Creation commit (git log --follow --diff-filter=A --max-count=1, with no-follow fallback for deleted files). Other filters (from/to/date/author/message/maxResults/noCache) are ignored in firstCommit mode.",
                    }
                });
                ToolCallResult::success(json_to_string(&output))
            }
            Ok(None) => {
                let elapsed = start.elapsed();
                let mut output = json!({
                    "firstCommit": Value::Null,
                    "summary": {
                        "tool": "xray_git_history",
                        "mode": "firstCommit",
                        "file": file,
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": "No creation commit found. The file may never have existed in git, or the path is filtered by .gitignore.",
                    }
                });
                annotate_empty_git_result(&mut output, repo, file, 0);
                ToolCallResult::success(json_to_string(&output))
            }
            Err(e) => ToolCallResult::error(e),
        };
    }

    let from_owned = match read_string(args, "from") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let to_owned = match read_string(args, "to") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let date_owned = match read_string(args, "date") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let from = from_owned.as_deref();
    let to = to_owned.as_deref();
    let date = date_owned.as_deref();
    // GIT-008: cap maxResults at 1_000_000 (sane upper bound for git log output).
    let max_results = match parse_bounded_usize(args, "maxResults", 50, 1_000_000) {
        Ok(n) => n,
        Err(e) => return ToolCallResult::error(e),
    };
    let author_filter_owned = match read_string(args, "author") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let message_filter_owned = match read_string(args, "message") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let author_filter = author_filter_owned.as_deref();
    let message_filter = message_filter_owned.as_deref();
    let no_cache = args.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);

    // ── Cache path (history only, not diff — cache has no patch data) ──
    if !include_diff && !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed)
        && let Ok(cache_guard) = ctx.git_cache.read()
            && let Some(cache) = cache_guard.as_ref()
            && cache_is_fresh_for_shallow(cache, repo) {
                let start = Instant::now();
                let normalized = GitHistoryCache::normalize_path(file);

                let (from_ts, to_ts) = match parse_cache_date_range(from, to, date) {
                    Ok(range) => range,
                    Err(e) => return ToolCallResult::error(e),
                };

                let max = if max_results == 0 { None } else { Some(max_results) };
                let (commits, total_count) = cache.query_file_history(&normalized, max, from_ts, to_ts, author_filter, message_filter);
                // Snapshot what we need before deciding the fall-through, so we can drop the
                // cache read-lock before spawning `git rev-parse HEAD` (lock-order hygiene:
                // never hold git_cache.read while doing IO that could outlive the request).
                let cache_head_snapshot = cache.head_hash.clone();
                drop(cache_guard);
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

                // HEAD-pinning (user story 2026-05-10): an empty cached result is only
                // authoritative relative to the cache's snapshot HEAD. If HEAD has moved,
                // new commits may now match (file just committed; author's first commit;
                // message pattern's first match; date range that just became reachable),
                // so fall through to CLI fallback. Non-empty results are NEVER re-checked
                // (cache is monotonic for files+commits already known).
                if total_count == 0 && cache_head_stale(&cache_head_snapshot, repo) {
                    // intentional fall-through to CLI fallback below
                } else {
                    if total_count == 0 {
                        annotate_empty_git_result(&mut output, repo, file, 0);
                    }
                    return ToolCallResult::success(json_to_string(&output));
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
    let repo = match read_required_string(args, "repo") {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };
    let repo = repo.as_str();

    // path takes priority, file is backward-compatible alias
    let query_path_owned = {
        let from_path = match read_string(args, "path") {
            Ok(v) => v,
            Err(e) => return ToolCallResult::error(e),
        };
        match from_path {
            Some(p) => Some(p),
            None => match read_string(args, "file") {
                Ok(v) => v,
                Err(e) => return ToolCallResult::error(e),
            },
        }
    };
    let query_path = query_path_owned.as_deref().unwrap_or("");

    let from_owned = match read_string(args, "from") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let to_owned = match read_string(args, "to") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let from = from_owned.as_deref();
    let to = to_owned.as_deref();
    // GIT-008: cap top at 10_000 (more than enough authors for any repo).
    let top = match parse_bounded_usize(args, "top", 10, 10_000) {
        Ok(n) => n,
        Err(e) => return ToolCallResult::error(e),
    };
    let message_filter_owned = match read_string(args, "message") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let message_filter = message_filter_owned.as_deref();
    let no_cache = args.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);

    // ── Cache path ──
    if !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed)
        && let Ok(cache_guard) = ctx.git_cache.read()
            && let Some(cache) = cache_guard.as_ref()
            && cache_is_fresh_for_shallow(cache, repo) {
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
                // Snapshot head + drop guard before spawning git (lock-order hygiene).
                let cache_head_snapshot = cache.head_hash.clone();
                drop(cache_guard);
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

                // HEAD-pinning (user story 2026-05-10) — see handle_git_history for rationale.
                if total_authors == 0 && cache_head_stale(&cache_head_snapshot, repo) {
                    // fall through to CLI
                } else {
                    // Empty results annotation: distinguish 'never existed' vs 'deleted from HEAD'.
                    if total_authors == 0 {
                        annotate_empty_git_result(&mut output, repo, query_path, 0);
                    }
                    return ToolCallResult::success(json_to_string(&output));
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
    let repo = match read_required_string(args, "repo") {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };
    let repo = repo.as_str();

    let from_owned = match read_string(args, "from") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let to_owned = match read_string(args, "to") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let date_owned = match read_string(args, "date") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let author_filter_owned = match read_string(args, "author") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let message_filter_owned = match read_string(args, "message") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let path_owned = match read_string(args, "path") {
        Ok(v) => v,
        Err(e) => return ToolCallResult::error(e),
    };
    let from = from_owned.as_deref();
    let to = to_owned.as_deref();
    let date = date_owned.as_deref();
    let author_filter = author_filter_owned.as_deref();
    let message_filter = message_filter_owned.as_deref();
    let no_cache = args.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);
    let include_deleted = args.get("includeDeleted").and_then(|v| v.as_bool()).unwrap_or(false);

    // ── Cache path ──
    if !no_cache && ctx.git_cache_ready.load(Ordering::Relaxed)
        && let Ok(cache_guard) = ctx.git_cache.read()
            && let Some(cache) = cache_guard.as_ref()
            && cache_is_fresh_for_shallow(cache, repo) {
                let start = Instant::now();

                // For activity, use empty string for whole-repo scope
                let query_path = path_owned.as_deref().unwrap_or("");
                let normalized = GitHistoryCache::normalize_path(query_path);

                let (from_ts, to_ts) = match parse_cache_date_range(from, to, date) {
                    Ok(range) => range,
                    Err(e) => return ToolCallResult::error(e),
                };

                let mut activities = cache.query_activity(&normalized, from_ts, to_ts, author_filter, message_filter);
                // Pre-`includeDeleted`-filter count. HEAD-pinning gates on THIS count, not
                // post-filter `total_files`: a known path that `query_activity` returns N>0
                // for but `includeDeleted=true` post-trims to 0 is an authoritative empty
                // (CLI would compute the same), whereas a true zero-results-from-cache may
                // be a stale false-negative if HEAD has moved.
                let cache_pre_filter_count = activities.len();

                // Snapshot all needed cache fields and DROP the read-guard before any git IO
                // (lock-order hygiene: never hold git_cache.read() across `git ls-files` /
                // `git rev-parse` subprocesses, which can briefly block cache writers).
                let cache_head_snapshot = cache.head_hash.clone();
                let cache_commits_count = cache.commits.len();
                drop(cache_guard);

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
                        "commitsProcessed": cache_commits_count,
                        "elapsedMs": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
                        "hint": if include_deleted {
                            "(from cache, filtered to files NOT in current HEAD)"
                        } else {
                            "(from cache)"
                        },
                        "includeDeleted": include_deleted
                    }
                });

                // HEAD-pinning (user story 2026-05-10) — see handle_git_history for rationale.
                // Gate on the PRE-`includeDeleted`-filter cache result count: a known path
                // post-trimmed to 0 by includeDeleted is authoritative, but a true empty
                // from `query_activity` may be stale if HEAD has moved.
                if cache_pre_filter_count == 0 && cache_head_stale(&cache_head_snapshot, repo) {
                    // fall through to CLI
                } else {
                    // Empty results annotation: distinguish 'never existed' vs 'deleted from HEAD'.
                    if total_files == 0 {
                        annotate_empty_git_result(&mut output, repo, query_path, 0);
                    }
                    return ToolCallResult::success(json_to_string(&output));
                }
            }

    // ── CLI fallback ──
    let filter = match git::parse_date_filter(from, to, date) {
        Ok(f) => f,
        Err(e) => return ToolCallResult::error(e),
    };

    let start = Instant::now();

    let activity_path = path_owned.as_deref();

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
    let repo = match read_required_string(args, "repo") {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };
    let file = match read_required_string(args, "file") {
        Ok(f) => f,
        Err(e) => return ToolCallResult::error(e),
    };
    let repo = repo.as_str();
    let file = file.as_str();

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
fn is_full_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn ancestry_base(revision: &str) -> Option<&str> {
    let operator = revision.find(['~', '^'])?;
    let base = revision.get(..operator)?;
    if base.is_empty() || base.contains("..") {
        return None;
    }

    let suffix = revision.get(operator..)?;
    let mut chars = suffix.chars().peekable();
    while let Some(operator) = chars.next() {
        if !matches!(operator, '~' | '^') {
            return None;
        }
        while chars.next_if(|ch| ch.is_ascii_digit()).is_some() {}
        if chars.peek().is_some_and(|ch| !matches!(ch, '~' | '^')) {
            return None;
        }
    }
    Some(base)
}

fn revision_may_be_hidden_by_shallow(repo: &str, expected_ref: &str) -> bool {
    if is_full_object_id(expected_ref) {
        return true;
    }
    let Some(base) = ancestry_base(expected_ref) else {
        return false;
    };
    if is_full_object_id(base) {
        return true;
    }
    let base_commit = format!("{}^{{commit}}", base);
    run_git_command(repo, &["rev-parse", "--verify", &base_commit]).is_ok()
}


fn handle_branch_status(_ctx: &HandlerContext, args: &Value) -> ToolCallResult {
    let repo = match read_required_string(args, "repo") {
        Ok(r) => r,
        Err(e) => return ToolCallResult::error(e),
    };
    let repo = repo.as_str();
    let expected_ref = match read_string(args, "expectedRef") {
        Ok(value) => value,
        Err(error) => return ToolCallResult::error(error),
    };
    if let Some(expected_ref) = expected_ref.as_deref()
        && let Err(error) = validate_git_ref(expected_ref)
    {
        return ToolCallResult::error(error);
    }

    let start = Instant::now();

    // a. Current branch
    let t = Instant::now();
    let current_branch = match run_git_command(repo, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(b) => b,
        Err(e) => return ToolCallResult::error(format!("Failed to get current branch: {}", e)),
    };
    let current_branch_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let actual_head = match run_git_command(repo, &["rev-parse", "--verify", "HEAD^{commit}"]) {
        Ok(head) => head,
        Err(e) => return ToolCallResult::error(format!("Failed to resolve HEAD: {}", e)),
    };
    let actual_head_ms = t.elapsed().as_secs_f64() * 1000.0;

    // b. Is main branch
    let is_main = is_main_branch(&current_branch);

    // c. Determine main branch name
    let t = Instant::now();
    let main_branch = detect_main_branch_name(_ctx, repo);
    let main_branch_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Shallow detection runs before behind/ahead: a shallow graft can truncate a
    // real common ancestor and make `git merge-base` look empty, which must NOT
    // be misread as "unrelated histories". Computed once and reused for output.
    let t = Instant::now();
    let shallow_info = git::detect_shallow(repo);
    let shallow_ms = t.elapsed().as_secs_f64() * 1000.0;

    // d. Behind/ahead of main — history-aware trunk selection.
    //
    // `main_branch` is only the *nominal* trunk name (priority main→master). A
    // repo can carry BOTH `main` and `master` where one is an unrelated orphan
    // (stale GitHub default, migration leftover). Comparing HEAD against an
    // unrelated trunk yields a meaningless symmetric-difference count (tens of
    // thousands "behind") that never shrinks on pull. So we compare against the
    // trunk whose remote actually shares history with HEAD and surface the
    // *effective* trunk + an `unrelatedHistories` flag instead of a bogus count.
    let t = Instant::now();
    let (behind_main, ahead_of_main, effective_main, unrelated_histories) =
        if let Some(ref mb) = main_branch {
            resolve_behind_ahead(repo, mb, shallow_info.is_some())
        } else {
            (None, None, None, false)
        };
    let behind_ahead_ms = t.elapsed().as_secs_f64() * 1000.0;

    // e. Dirty files
    let t = Instant::now();
    let dirty_files = get_dirty_files(repo);
    let dirty_files_ms = t.elapsed().as_secs_f64() * 1000.0;
    let worktree_dirty = !dirty_files.is_empty();

    let t = Instant::now();
    let (expected_head, revision_matches, revision_status, revision_warning) =
        match expected_ref.as_deref() {
            None => (None, None, "not_requested", None),
            Some(expected_ref) => {
                let revision = format!("{}^{{commit}}", expected_ref);
                match run_git_command(repo, &["rev-parse", "--verify", &revision]) {
                    Ok(expected_head) => {
                        let matches = actual_head == expected_head;
                        let status = if matches { "match" } else { "mismatch" };
                        let warning = (!matches).then(|| format!(
                            "Expected ref '{}' resolves to {}, but the local checkout is at {}.",
                            expected_ref, expected_head, actual_head,
                        ));
                        (Some(expected_head), Some(matches), status, warning)
                    }
                    Err(_) if shallow_info.is_some()
                        && revision_may_be_hidden_by_shallow(repo, expected_ref) => (
                            None,
                            None,
                            "shallow_history",
                            Some(format!(
                                "Expected revision '{}' is not available locally; shallow history may hide the required ancestor or object.",
                                expected_ref,
                            )),
                        ),
                    Err(_) => (
                        None,
                        None,
                        "unresolved_ref",
                        Some(format!(
                            "Expected ref '{}' is not available in the local Git object database.",
                            expected_ref,
                        )),
                    ),
                }
            }
        };
    let expected_ref_ms = t.elapsed().as_secs_f64() * 1000.0;
    let revision_ms = actual_head_ms + expected_ref_ms;

    // f. Last fetch time
    let t = Instant::now();
    let (last_fetch_time, fetch_age, fetch_warning) = get_fetch_info(repo);
    let fetch_info_ms = t.elapsed().as_secs_f64() * 1000.0;

    // g. Warning
    let warning = build_warning(
        &current_branch,
        is_main,
        &effective_main,
        behind_main,
        unrelated_histories,
    );

    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;

    crate::index::log_phase("branchStatus", &[
        ("elapsedMs", format!("{:.1}", elapsed_ms)),
        ("currentBranchMs", format!("{:.1}", current_branch_ms)),
        ("revisionMs", format!("{:.1}", revision_ms)),
        ("mainBranchMs", format!("{:.1}", main_branch_ms)),
        ("behindAheadMs", format!("{:.1}", behind_ahead_ms)),
        ("dirtyFilesMs", format!("{:.1}", dirty_files_ms)),
        ("fetchInfoMs", format!("{:.1}", fetch_info_ms)),
        ("shallowMs", format!("{:.1}", shallow_ms)),
        ("dirtyFileCount", dirty_files.len().to_string()),
        ("repo", repo.to_string()),
    ]);

    let output = json!({
        "currentBranch": current_branch,
        "actualHead": actual_head,
        "expectedRef": expected_ref,
        "expectedHead": expected_head,
        "revisionMatches": revision_matches,
        "revisionStatus": revision_status,
        "revisionWarning": revision_warning,
        "worktreeDirty": worktree_dirty,
        "isMainBranch": is_main,
        "mainBranch": effective_main,
        "behindMain": behind_main,
        "aheadOfMain": ahead_of_main,
        "unrelatedHistories": unrelated_histories,
        "dirtyFiles": dirty_files,
        "dirtyFileCount": dirty_files.len(),
        "lastFetchTime": last_fetch_time,
        "fetchAge": fetch_age,
        "fetchWarning": fetch_warning,
        "warning": warning,
        "isShallow": shallow_info.is_some(),
        "shallowBoundaries": shallow_info.as_ref().map(|s| s.boundaries.clone()),
        "summary": {
            "tool": "xray_branch_status",
            "elapsedMs": (elapsed_ms * 100.0).round() / 100.0,
            "subTimings": {
                "currentBranchMs": (current_branch_ms * 100.0).round() / 100.0,
                "revisionMs": (revision_ms * 100.0).round() / 100.0,
                "mainBranchMs": (main_branch_ms * 100.0).round() / 100.0,
                "behindAheadMs": (behind_ahead_ms * 100.0).round() / 100.0,
                "dirtyFilesMs": (dirty_files_ms * 100.0).round() / 100.0,
                "fetchInfoMs": (fetch_info_ms * 100.0).round() / 100.0,
                "shallowMs": (shallow_ms * 100.0).round() / 100.0,
            }
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
///
/// **PERF-02:** result is cached per repo path on `ctx.branch_name_cache`,
/// and the cold probe collapses 4 sequential `git rev-parse` invocations
/// into a single `git for-each-ref` (≈20–80 ms saved per
/// `xray_branch_status` on Windows where each spawn costs 5–20 ms).
/// See [`HandlerContext::branch_name_cache`] for the invalidation contract.
fn detect_main_branch_name(ctx: &HandlerContext, repo: &str) -> Option<String> {
    // Cache hit: return immediately. Read-lock so concurrent branch_status
    // requests don't serialise on a write-locked mutex.
    if let Ok(cache) = ctx.branch_name_cache.read()
        && let Some(cached) = cache.get(repo) {
            return cached.clone();
        }

    let resolved = probe_main_branch_name(repo);

    // Best-effort cache write. If the lock is poisoned we silently re-probe
    // next call — strictly worse than caching but never worse than the old
    // uncached path, so swallowing the error is safe.
    //
    // **PERF-02 follow-up**: only cache positive results. Negative caching
    // (`Some(None)`) created a permanent-poisoning bug: a path that gets
    // probed before its repo exists (e.g. handler called against an
    // empty workspace, then user runs `git init` + creates a `main`
    // branch) would forever return None until server restart, even though
    // a re-probe would now succeed. Re-probing a hopeless path costs one
    // `git for-each-ref` per request — ≈5-20 ms on Windows, paid only by
    // the (rare) bad-path case; the common positive path still benefits
    // from the cache and remains zero-spawn on warm calls.
    if resolved.is_some()
        && let Ok(mut cache) = ctx.branch_name_cache.write()
    {
        cache.insert(repo.to_string(), resolved.clone());
    }
    resolved
}

/// PERF-02 cold probe: ask `git for-each-ref` for all four candidate refs in
/// a single spawn instead of running up to 4 sequential `git rev-parse`.
///
/// `for-each-ref` prints one line per ref that **exists**, silent for refs
/// that don't exist. **Output is sorted by refname**, NOT by argument order,
/// so a repo with both `refs/heads/master` and `refs/remotes/origin/main`
/// emits `master` BEFORE `origin/main`. We must therefore enumerate which
/// candidates exist and apply explicit priority — matching the legacy
/// 4-probe sequence: `main` (local-or-remote) is always preferred over
/// `master` (local-or-remote). Falls back to the legacy probe sequence if
/// the combined call fails for any reason (very old git, hardened sandbox
/// without `for-each-ref`, etc.) so we never regress the resolution itself.
fn probe_main_branch_name(repo: &str) -> Option<String> {
    if let Ok(out) = run_git_command(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/main",
            "refs/heads/master",
            "refs/remotes/origin/main",
            "refs/remotes/origin/master",
        ],
    ) {
        let mut has_main = false;
        let mut has_master = false;
        for line in out.lines() {
            match line.trim() {
                "main" | "origin/main" => has_main = true,
                "master" | "origin/master" => has_master = true,
                _ => {}
            }
        }
        if has_main {
            return Some("main".to_string());
        }
        if has_master {
            return Some("master".to_string());
        }
        // Combined probe ran but found nothing — don't fall back; the legacy
        // probe would just confirm the same answer at 4× the cost.
        return None;
    }

    // Combined probe failed (very rare). Fall back to the legacy 4-probe
    // sequence so behaviour is identical to the pre-PERF-02 implementation.
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

/// Resolve how far HEAD is behind/ahead of the repository trunk, choosing the
/// trunk remote that actually shares history with HEAD.
///
/// Returns `(behind, ahead, effectiveTrunk, unrelatedHistories)`:
/// - `behind`/`ahead`: symmetric-difference counts vs `origin/<effectiveTrunk>`,
///   or `None` when no related trunk remote exists.
/// - `effectiveTrunk`: the trunk actually compared against — may differ from the
///   nominal `primary` when `primary` is an unrelated orphan ref.
/// - `unrelatedHistories`: `true` ONLY with positive evidence of genuinely
///   unrelated histories — a trunk remote exists, HEAD resolves to a commit, the
///   repo is not shallow, yet no candidate shares a merge-base with HEAD. The
///   count is suppressed because it would otherwise be a meaningless "entire
///   other history" number that never shrinks on pull.
///
/// `repo_is_shallow` gates the flag: a shallow graft can truncate a real common
/// ancestor and make `merge-base` look empty, so we never claim "unrelated" on a
/// shallow clone (the count is still suppressed, just without the orphan claim).
/// An unborn HEAD likewise yields no merge-base but is not "unrelated", so we
/// require HEAD to resolve before flagging (the handler errors earlier on unborn
/// HEAD today, but this keeps the helper self-correct).
///
/// Candidate order: the detected `primary` first, then the alternate well-known
/// trunk (`main`↔`master`). This recovers the real trunk when a repo carries
/// both refs and the detected one is an orphan.
fn resolve_behind_ahead(
    repo: &str,
    primary: &str,
    repo_is_shallow: bool,
) -> (Option<u64>, Option<u64>, Option<String>, bool) {
    let alternate = match primary {
        "main" => Some("master"),
        "master" => Some("main"),
        _ => None,
    };
    let candidates = std::iter::once(primary).chain(alternate);

    let mut remote_exists_no_merge_base = false;
    for cand in candidates {
        let remote_ref = format!("origin/{}", cand);
        // `git merge-base HEAD <ref>` succeeds (non-empty) iff the two share a
        // common ancestor. It exits non-zero with no output for unrelated
        // histories, a missing ref, AND an unborn/invalid HEAD, so a failure
        // alone cannot tell those apart — we disambiguate after the loop.
        match run_git_command(repo, &["merge-base", "HEAD", &remote_ref]) {
            Ok(mb) if !mb.is_empty() => {
                let (behind, ahead) = count_left_right(repo, &remote_ref);
                return (behind, ahead, Some(cand.to_string()), false);
            }
            _ => {
                if ref_exists(repo, &remote_ref) {
                    remote_exists_no_merge_base = true;
                }
            }
        }
    }

    // Claim "unrelated histories" only with positive evidence: a trunk remote
    // exists, the repo is not shallow (a graft can hide the real ancestor), and
    // HEAD is a real commit (not unborn). Otherwise the count is simply "cannot
    // compute", not an orphan trunk — suppress it WITHOUT the misleading claim.
    let unrelated =
        remote_exists_no_merge_base && !repo_is_shallow && ref_exists(repo, "HEAD");

    (None, None, Some(primary.to_string()), unrelated)
}

/// True if `git_ref` (e.g. `origin/main` or `HEAD`) resolves to a commit in `repo`.
fn ref_exists(repo: &str, git_ref: &str) -> bool {
    run_git_command(repo, &["rev-parse", "--verify", "--quiet", git_ref])
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

/// Count how far HEAD is behind/ahead of `remote_ref` via the symmetric
/// difference `HEAD...<remote_ref>`. Returns `(behind, ahead)`.
fn count_left_right(repo: &str, remote_ref: &str) -> (Option<u64>, Option<u64>) {
    match run_git_command(repo, &["rev-list", "--left-right", "--count", &format!("HEAD...{}", remote_ref)]) {
        Ok(output) => {
            // Output format: "3\t47" where 3=ahead (left, HEAD), 47=behind (right).
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
    unrelated_histories: bool,
) -> Option<String> {
    // Unrelated histories take precedence: behind/ahead were suppressed, so any
    // "N commits behind" message would be both absent and misleading.
    if unrelated_histories {
        return Some(format!(
            "HEAD and origin/{} have unrelated histories (no common ancestor). \
             behind/ahead suppressed — the repo likely carries an orphan trunk ref.",
            main_branch.as_deref().unwrap_or("main/master")
        ));
    }
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

