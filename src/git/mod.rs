//! Git history query module — calls `git` CLI for optimal performance.
//!
//! Uses `git log` CLI with commit-graph and bloom filter optimizations for
//! path-limited queries. On-demand fallback when in-memory cache is not available.
//! See `cache.rs` for the pre-built in-memory cache path (sub-millisecond queries).

use std::collections::HashMap;
use std::process::Command;

// ─── Types ──────────────────────────────────────────────────────────

/// Information about a single commit that touched a file.
#[derive(Clone, Debug)]
pub struct CommitInfo {
    pub hash: String,
    pub date: String,
    pub author_name: String,
    pub author_email: String,
    pub message: String,
    pub patch: Option<String>,
}

/// Aggregated author statistics for a file.
#[derive(Clone, Debug)]
pub struct AuthorStats {
    pub name: String,
    pub email: String,
    pub commit_count: usize,
    pub first_change: String,
    pub last_change: String,
}

/// Date range filter for git queries.
#[derive(Clone, Debug)]
pub struct DateFilter {
    /// Start date string (YYYY-MM-DD), inclusive
    pub from_date: Option<String>,
    /// End date string (YYYY-MM-DD), inclusive (converted to next day for git --before)
    pub to_date: Option<String>,
}

/// Information about a single blamed line.
#[derive(Clone, Debug)]
pub struct BlameLine {
    pub line: usize,
    pub hash: String,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    pub content: String,
}

// ─── Date helpers ───────────────────────────────────────────────────

/// Validate a YYYY-MM-DD date string. Returns Ok(()) or Err with message.
pub fn validate_date(s: &str) -> Result<(), String> {
    // Simple validation: must be YYYY-MM-DD format
    if s.len() != 10 {
        return Err(format!("Invalid date '{}': expected YYYY-MM-DD format", s));
    }
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("Invalid date '{}': expected YYYY-MM-DD format", s));
    }
    let year: u32 = parts[0].parse().map_err(|_| format!("Invalid year in '{}'", s))?;
    let month: u32 = parts[1].parse().map_err(|_| format!("Invalid month in '{}'", s))?;
    let day: u32 = parts[2].parse().map_err(|_| format!("Invalid day in '{}'", s))?;

    if !(1970..=2100).contains(&year) {
        return Err(format!("Year {} out of range (1970-2100)", year));
    }
    if !(1..=12).contains(&month) {
        return Err(format!("Month {} out of range (1-12)", month));
    }
    if !(1..=31).contains(&day) {
        return Err(format!("Day {} out of range (1-31)", day));
    }

    Ok(())
}

/// Increment a YYYY-MM-DD date by one day for --before filter.
/// Simple implementation that handles month/year boundaries.
fn next_day(date: &str) -> String {
    let parts: Vec<u32> = date.split('-').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 3 {
        // This branch should be unreachable — validate_date() is always called before next_day().
        // If reached, return original date; git will either handle it or return a clear error.
        eprintln!("[WARN] next_day called with unparseable date: {}", date);
        return date.to_string();
    }
    let (year, month, day) = (parts[0], parts[1], parts[2]);

    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 29 } else { 28 },
        _ => 31,
    };

    if day < days_in_month {
        format!("{:04}-{:02}-{:02}", year, month, day + 1)
    } else if month < 12 {
        format!("{:04}-{:02}-01", year, month + 1)
    } else {
        format!("{:04}-01-01", year + 1)
    }
}

/// Build a DateFilter from optional from/to/date parameters.
///
/// If `date` is provided, it overrides `from` and `to` (single-day filter).
pub fn parse_date_filter(
    from: Option<&str>,
    to: Option<&str>,
    date: Option<&str>,
) -> Result<DateFilter, String> {
    if let Some(d) = date {
        validate_date(d)?;
        Ok(DateFilter {
            from_date: Some(d.to_string()),
            to_date: Some(d.to_string()),
        })
    } else {
        if let Some(f) = from {
            validate_date(f)?;
        }
        if let Some(t) = to {
            validate_date(t)?;
        }
        // Validate from <= to (BUG-4: reversed date range silently returned 0 results)
        if let (Some(f), Some(t)) = (from, to)
            && f > t {
                return Err(format!(
                    "'from' date ({}) is after 'to' date ({}). Swap them or correct the range.",
                    f, t
                ));
            }
        Ok(DateFilter {
            from_date: from.map(|s| s.to_string()),
            to_date: to.map(|s| s.to_string()),
        })
    }
}

// ─── Git CLI helpers ────────────────────────────────────────────────

/// Separator used in git log --format to split fields.
/// Using a rare Unicode character to avoid collision with commit messages.
const FIELD_SEP: &str = "␞";
/// Separator between records in git log output.
const RECORD_SEP: &str = "␟";

/// Build common git log arguments for date filtering.
///
/// Appends `T00:00:00Z` to force UTC interpretation, matching the cache path
/// which uses UTC timestamps. Without this, git interprets bare YYYY-MM-DD
/// dates in the local timezone, causing mismatches on non-UTC systems.
fn add_date_args(cmd: &mut Command, filter: &DateFilter) {
    if let Some(ref from) = filter.from_date {
        cmd.arg(format!("--after={}T00:00:00Z", from));
    }
    if let Some(ref to) = filter.to_date {
        // git --before is exclusive, so we need the next day for inclusive behavior
        let next = next_day(to);
        cmd.arg(format!("--before={}T00:00:00Z", next));
    }
}

/// Run a git command and return stdout as String.
fn run_git(cmd: &mut Command) -> Result<String, String> {
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to execute git: {}. Is git installed and in PATH?", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git command failed: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse a git log record (using FIELD_SEP-separated fields) into CommitInfo.
fn parse_commit_record(record: &str) -> Option<CommitInfo> {
    let fields: Vec<&str> = record.split(FIELD_SEP).collect();
    if fields.len() < 5 {
        return None;
    }

    Some(CommitInfo {
        hash: fields[0].trim().to_string(),
        date: fields[1].trim().to_string(),
        author_name: fields[2].trim().to_string(),
        author_email: fields[3].trim().to_string(),
        message: fields[4..].join(FIELD_SEP).trim().to_string(),
        patch: None,
    })
}

// ─── Core query functions ───────────────────────────────────────────

/// Maximum number of patch lines per commit to prevent context overflow.
const MAX_PATCH_LINES: usize = 200;

/// Get commit history for a single file.
///
/// If `include_diff` is true, each commit includes the patch text.
/// `max_results` limits the number of commits returned (0 = unlimited).
///
/// Returns `(commits, total_count)` where total_count may exceed commits.len()
/// when max_results limits the output.
pub fn file_history(
    repo_path: &str,
    file: &str,
    filter: &DateFilter,
    include_diff: bool,
    max_results: usize,
    author_filter: Option<&str>,
    message_filter: Option<&str>,
) -> Result<(Vec<CommitInfo>, usize), String> {
    // Try WITH --follow first (default behavior — follows renames)
    let (mut commits, mut total_count) = run_file_history_query(
        repo_path, file, filter, max_results, author_filter, message_filter, true,
    )?;

    // Fallback for DELETED files: if --follow returned 0 results, retry WITHOUT --follow.
    // `git log --follow` is known to return empty for files that were deleted and never
    // renamed — removing --follow makes git traverse the delete commit.
    // See user story 2026-04-17_git-deleted-files-support.md for details.
    //
    // Bug 7 (consolidated plan 2026-04-23): we used to gate this retry on a separate
    // `file_ever_existed_in_git` probe (one extra `git log --all` spawn). That gate is
    // redundant — the no-follow query itself returns 0 results when the file truly never
    // existed (or has no commits in the active filter), with the same correctness signal
    // and one fewer process spawn on the deleted-file cold path. Other call sites that
    // need an explicit "existed?" boolean (e.g. `annotate_empty_git_result`) still use
    // `file_ever_existed_in_git` directly.
    if total_count == 0 {
        let (no_follow_commits, no_follow_total) = run_file_history_query(
            repo_path, file, filter, max_results, author_filter, message_filter, false,
        )?;
        if no_follow_total > 0 {
            commits = no_follow_commits;
            total_count = no_follow_total;
        }
    }

    // If diff requested, get patch for each commit
    if include_diff {
        for commit in &mut commits {
            let patch = get_commit_diff(repo_path, &commit.hash, file)?;
            commit.patch = Some(patch);
        }
    }

    Ok((commits, total_count))
}

/// Internal helper for `file_history`: run one `git log` query with or without `--follow`.
/// Returns `(commits, total_count_before_truncation)`.
fn run_file_history_query(
    repo_path: &str,
    file: &str,
    filter: &DateFilter,
    max_results: usize,
    author_filter: Option<&str>,
    message_filter: Option<&str>,
    follow: bool,
) -> Result<(Vec<CommitInfo>, usize), String> {
    let format = format!("{}%H{}%ai{}%an{}%ae{}%s{}", RECORD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP);

    let mut cmd = Command::new("git");
    cmd.current_dir(repo_path)
        .arg("log")
        .arg(format!("--format={}", format));

    // GIT-005: bound git output via --max-count instead of pulling the entire
    // history and truncating in-process. Without this, a query like
    // `xray_git_history file=X maxResults=50` on a file with 100k commits
    // streams ALL 100k commit records out of git, parses each into a
    // CommitInfo, then throws 99 950 away. Request `max_results + 1` so the
    // handler's "more commits available" hint still fires correctly when the
    // cap is hit (total_count > returned).
    if max_results > 0 {
        cmd.arg(format!("--max-count={}", max_results.saturating_add(1)));
    }

    if follow {
        cmd.arg("--follow");
    }

    add_date_args(&mut cmd, filter);

    if let Some(author) = author_filter {
        cmd.arg(format!("--author={}", author));
    }
    if let Some(message) = message_filter {
        cmd.arg(format!("--grep={}", message));
    }

    cmd.arg("--").arg(file);

    let output = run_git(&mut cmd)?;

    let mut commits: Vec<CommitInfo> = output
        .split(RECORD_SEP)
        .filter(|s| !s.trim().is_empty())
        .filter_map(parse_commit_record)
        .collect();

    let total_count = commits.len();

    if max_results > 0 && commits.len() > max_results {
        commits.truncate(max_results);
    }

    Ok((commits, total_count))
}

/// Get the diff/patch for a specific commit and file.
fn get_commit_diff(repo_path: &str, hash: &str, file: &str) -> Result<String, String> {
    // PERF-03: single `git show` spawn instead of the previous
    //   1) `git rev-parse --verify <hash>^` (parent probe)
    //   2) `git diff <hash>^..<hash>` OR `git diff <empty-tree> <hash>` (initial)
    // sequence. `git show <hash> --format= --patch -- <file>` handles the
    // initial-commit case natively (diff against /dev/null, no parent
    // required) and avoids hard-coding the magic empty-tree SHA
    // `4b825dc6…` — which is not actually present in every clone (`git
    // diff <empty-tree>` fails with `bad object` when the tree object
    // isn't reachable from any ref). The patch-section output is byte-
    // identical to `git diff <hash>^..<hash>` for non-initial commits,
    // verified pre-change by walking real history on the xray repo.
    //
    // Net effect: 200-commit `xray_git_history file=… includeDiff=true`
    // drops from 400 → 200 spawns (≈1–4s saved on Windows).
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_path)
        .arg("show")
        .arg(hash)
        .arg("--format=")
        .arg("--patch")
        .arg("--")
        .arg(file);

    let output = run_git(&mut cmd)?;

    // Truncate to MAX_PATCH_LINES
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() > MAX_PATCH_LINES {
        let truncated: String = lines[..MAX_PATCH_LINES].join("\n");
        Ok(format!("{}\n... (truncated at {} lines)", truncated, MAX_PATCH_LINES))
    } else {
        Ok(output)
    }
}

/// Get top authors for a file or directory, ranked by commit count.
///
/// `path` can be a file, directory, or empty string (entire repo).
/// When empty, queries all commits in the repo.
///
/// Returns `(authors, total_commits, total_authors)`.
pub fn top_authors(
    repo_path: &str,
    path: &str,
    filter: &DateFilter,
    top: usize,
    message_filter: Option<&str>,
) -> Result<(Vec<AuthorStats>, usize, usize), String> {
    // Use git shortlog for author aggregation (much faster than manual counting)
    // But git shortlog doesn't give us first/last dates, so we use git log
    let format = format!("{}%H{}%ai{}%an{}%ae{}%s{}", RECORD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP);

    let mut cmd = Command::new("git");
    cmd.current_dir(repo_path)
        .arg("log")
        .arg(format!("--format={}", format));

    // --follow only works for single files, not directories or empty path
    // Heuristic: use --follow when path has a file extension (contains '.')
    if !path.is_empty() && path.contains('.') {
        cmd.arg("--follow");
    }

    add_date_args(&mut cmd, filter);

    // Safety cap: prevent OOM on huge repos without date filters.
    // 50K commits covers ~10 years of daily commits for most projects.
    cmd.arg("--max-count=50000");

    if let Some(message) = message_filter {
        cmd.arg(format!("--grep={}", message));
    }

    if !path.is_empty() {
        cmd.arg("--").arg(path);
    }

    let output = run_git(&mut cmd)?;

    let commits: Vec<CommitInfo> = output
        .split(RECORD_SEP)
        .filter(|s| !s.trim().is_empty())
        .filter_map(parse_commit_record)
        .collect();

    // Aggregate by author
    #[derive(Default)]
    struct InternalStats {
        count: usize,
        name: String,
        email: String,
        first_date: Option<String>,
        last_date: Option<String>,
    }

    let mut author_map: HashMap<(String, String), InternalStats> = HashMap::new();

    for commit in &commits {
        // PERF-04: tuple key avoids `format!("{} <{}>", …)` per commit. The
        // formatted display string was only used as a HashMap key, never
        // returned to the caller — so the formatting work was 100% waste on
        // every iteration after the first commit per author. Concrete cost
        // on a 50k-commit / 50-author repo: ~49,950 redundant String
        // allocations + format calls per `top_authors` invocation. Tuple
        // key keeps `(name, email)` separately and avoids the format
        // entirely; `InternalStats.name` / `.email` already stored the
        // unformatted parts so no information loss.
        let key = (commit.author_name.clone(), commit.author_email.clone());
        let stats = author_map.entry(key).or_insert_with(|| InternalStats {
            name: commit.author_name.clone(),
            email: commit.author_email.clone(),
            ..Default::default()
        });
        stats.count += 1;
        // Commits come in reverse chronological order
        if stats.last_date.is_none() {
            stats.last_date = Some(commit.date.clone());
        }
        stats.first_date = Some(commit.date.clone()); // keeps getting overwritten to oldest
    }

    let total_commits: usize = author_map.values().map(|s| s.count).sum();
    let total_authors = author_map.len();

    let mut ranked: Vec<_> = author_map.into_values().collect();
    ranked.sort_by_key(|b| std::cmp::Reverse(b.count));
    ranked.truncate(top);

    let authors: Vec<AuthorStats> = ranked
        .into_iter()
        .map(|s| AuthorStats {
            name: s.name,
            email: s.email,
            commit_count: s.count,
            first_change: s.first_date.unwrap_or_default(),
            last_change: s.last_date.unwrap_or_default(),
        })
        .collect();

    Ok((authors, total_commits, total_authors))
}

/// Get activity across ALL files in a repo for a date range.
///
/// Returns `(file_map, commits_processed)` where file_map maps
/// file paths to their commits.
pub fn repo_activity(
    repo_path: &str,
    filter: &DateFilter,
    author_filter: Option<&str>,
    message_filter: Option<&str>,
    path_filter: Option<&str>,
) -> Result<(HashMap<String, Vec<CommitInfo>>, u64), String> {
    // Use git log with --name-only to get changed files per commit
    let format = format!("{}%H{}%ai{}%an{}%ae{}%s{}", RECORD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP);

    let mut cmd = Command::new("git");
    cmd.current_dir(repo_path)
        .arg("log")
        .arg(format!("--format={}", format))
        .arg("--name-only");

    add_date_args(&mut cmd, filter);

    // Safety cap: repo_activity with --name-only produces more output per commit.
    // 10K commits is a reasonable limit for activity overview.
    cmd.arg("--max-count=10000");

    if let Some(author) = author_filter {
        cmd.arg(format!("--author={}", author));
    }
    if let Some(message) = message_filter {
        cmd.arg(format!("--grep={}", message));
    }

    // Add path filter via git log's -- <pathspec> syntax
    if let Some(path) = path_filter
        && !path.is_empty() {
            cmd.arg("--").arg(path);
        }

    let output = run_git(&mut cmd)?;

    let mut file_history: HashMap<String, Vec<CommitInfo>> = HashMap::new();
    let mut commits_processed = 0u64;

    // Parse output: each record starts with RECORD_SEP, followed by commit info,
    // then blank line, then file names (one per line)
    for record in output.split(RECORD_SEP) {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }

        // Split at blank line: first part is commit info, rest is file list
        let parts: Vec<&str> = record.splitn(2, "\n\n").collect();

        let commit_info_str = parts[0];
        let file_list_str = if parts.len() > 1 { parts[1] } else { "" };

        if let Some(info) = parse_commit_record(commit_info_str) {
            commits_processed += 1;

            for file_line in file_list_str.lines() {
                let file_path = file_line.trim();
                if !file_path.is_empty() {
                    file_history
                        .entry(file_path.to_string())
                        .or_default()
                        .push(info.clone());
                }
            }
        }
    }

    Ok((file_history, commits_processed))
}

// ─── File existence checks ──────────────────────────────────────────

/// Check whether a file exists in the current HEAD (working tree tracked by git).
///
/// Runs `git ls-files -- <file>` and returns `true` if the output is non-empty
/// (i.e., the file is tracked in the current HEAD). Returns `false` if the file
/// is not in HEAD (never tracked OR was deleted), or if the git command fails.
///
/// NOTE: This function returns `false` for deleted files. Use
/// [`file_ever_existed_in_git`] to check whether a file was ever tracked
/// (including deleted files).
pub fn file_exists_in_current_head(repo: &str, file: &str) -> bool {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo)
        .arg("ls-files")
        .arg("--")
        .arg(file);

    match run_git(&mut cmd) {
        Ok(output) => !output.trim().is_empty(),
        Err(_) => false,
    }
}

/// Backward-compatible alias for [`file_exists_in_current_head`].
///
/// Kept for external callers and older code paths. Prefer the more explicit
/// `file_exists_in_current_head` name, or `file_ever_existed_in_git` when you
/// want to include deleted files.
#[deprecated(note = "Use file_exists_in_current_head (clearer name) or file_ever_existed_in_git (includes deleted files)")]
#[allow(dead_code)]
pub fn file_exists_in_git(repo: &str, file: &str) -> bool {
    file_exists_in_current_head(repo, file)
}

/// Check whether a file was EVER tracked in git history, including deleted files.
///
/// Runs `git log --all --max-count=1 --format=%H -- <file>` and returns `true`
/// if any commit on any branch touched this path (add, modify, or delete).
/// Returns `false` if the file was never tracked or if the git command fails.
///
/// This is the right check for "did this path ever exist in the repo?" — useful
/// for distinguishing "file never existed" (user typo) from "file was deleted"
/// (valid historical query) when producing error/info messages.
///
/// Cost: spawns a single `git log` process (~50-100ms). Call only when the
/// cheaper `file_exists_in_current_head` returns false AND you need to decide
/// between "never existed" and "deleted".
pub fn file_ever_existed_in_git(repo: &str, file: &str) -> bool {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo)
        .arg("log")
        .arg("--all")
        .arg("--max-count=1")
        .arg("--format=%H")
        .arg("--")
        .arg(file);

    match run_git(&mut cmd) {
        Ok(output) => !output.trim().is_empty(),
        Err(_) => false,
    }
}

/// List tracked files under a directory in current HEAD (single `git ls-files` call).
///
/// Runs `git ls-files -- <dir>` and returns the output as a HashSet of
/// repo-relative paths (forward-slash normalized to match cache keys).
///
/// Used by `includeDeleted` logic in `xray_git_activity` to identify which
/// files in a directory are currently tracked (vs. deleted from HEAD).
///
/// MUST use a single `git ls-files` call — see user story 2026-04-17 section
/// on performance invariant. A naive implementation calling
/// `file_exists_in_current_head` per file in a cache result set would be
/// 75-225 seconds on large repos (200K files). This single call reads only
/// `.git/index` and runs in ~200-700ms even on huge repos.
pub fn list_tracked_files_under(repo: &str, dir: &str) -> std::collections::HashSet<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo)
        .arg("ls-files")
        .arg("-z"); // NUL-separated — safe for unusual filenames

    if !dir.is_empty() {
        cmd.arg("--").arg(dir);
    }

    let output = match cmd.output() {
        Ok(o) if o.status.success() => o,
        _ => return std::collections::HashSet::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    text.split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.replace('\\', "/"))
        .collect()
}

// ─── Blame ──────────────────────────────────────────────────────────

/// Run `git blame` for a line range and parse the porcelain output.
///
/// `start_line` and `end_line` are 1-based inclusive.
/// If `end_line` is None, only `start_line` is blamed.
pub fn blame_lines(
    repo_path: &str,
    file: &str,
    start_line: usize,
    end_line: Option<usize>,
) -> Result<Vec<BlameLine>, String> {
    let end = end_line.unwrap_or(start_line);

    let mut cmd = Command::new("git");
    cmd.current_dir(repo_path)
        .arg("blame")
        .arg(format!("-L{},{}", start_line, end))
        .arg("--porcelain")
        .arg("--")
        .arg(file);

    let output = run_git(&mut cmd)?;
    parse_blame_porcelain(&output)
}

/// Metadata cached for a commit hash seen earlier in porcelain output.
/// Git only emits full headers the first time a commit appears; subsequent
/// lines from the same commit only have the hash line + content.
#[derive(Clone)]
struct BlameCommitMeta {
    author_name: String,
    author_email: String,
    author_time: i64,
    author_tz: String,
}

/// Parse git blame --porcelain output into BlameLine entries.
///
/// Porcelain format (first occurrence of a commit):
/// ```text
/// <hash> <orig_line> <final_line> [<num_lines>]
/// author <name>
/// author-mail <<email>>
/// author-time <timestamp>
/// author-tz <timezone>
/// committer ...
/// committer-mail ...
/// committer-time ...
/// committer-tz ...
/// summary <subject>
/// [previous <hash> <file>]
/// [boundary]
/// filename <current_file>
/// \t<content line>
/// ```
///
/// Subsequent occurrences of the same commit only have:
/// ```text
/// <hash> <orig_line> <final_line>
/// \t<content line>
/// ```
pub(crate) fn parse_blame_porcelain(output: &str) -> Result<Vec<BlameLine>, String> {
    let mut results: Vec<BlameLine> = Vec::new();
    let mut seen: HashMap<String, BlameCommitMeta> = HashMap::new();
    let mut lines_iter = output.lines().peekable();

    while let Some(line) = lines_iter.next() {
        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        // Parse hash line: "<hash> <orig_line> <final_line> [<num_lines>]"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let hash = parts[0];
        // Validate it looks like a hash (40 hex chars)
        if hash.len() != 40 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }

        let final_line: usize = match parts[2].parse() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let mut author_name = String::new();
        let mut author_email = String::new();
        let mut author_time: i64 = 0;
        let mut author_tz = String::new();
        let mut content = String::new();
        let mut has_headers = false;

        // Read header fields until we hit the content line (starts with \t)
        for header_line in lines_iter.by_ref() {
            if let Some(stripped) = header_line.strip_prefix('\t') {
                // Content line — strip the leading tab
                content = stripped.to_string();
                break;
            }

            if let Some(val) = header_line.strip_prefix("author ") {
                author_name = val.to_string();
                has_headers = true;
            } else if let Some(val) = header_line.strip_prefix("author-mail ") {
                // Remove angle brackets: <email> -> email
                author_email = val.trim_start_matches('<').trim_end_matches('>').to_string();
            } else if let Some(val) = header_line.strip_prefix("author-time ") {
                author_time = val.parse().unwrap_or(0);
            } else if let Some(val) = header_line.strip_prefix("author-tz ") {
                author_tz = val.to_string();
            }
            // Skip other headers (committer, summary, filename, previous, boundary)
        }

        // If we got headers, cache them for later reuse
        if has_headers {
            seen.insert(hash.to_string(), BlameCommitMeta {
                author_name: author_name.clone(),
                author_email: author_email.clone(),
                author_time,
                author_tz: author_tz.clone(),
            });
        } else if let Some(cached) = seen.get(hash) {
            // Reuse cached metadata from first occurrence
            author_name = cached.author_name.clone();
            author_email = cached.author_email.clone();
            author_time = cached.author_time;
            author_tz = cached.author_tz.clone();
        }

        // Format date from timestamp + timezone
        let date = format_blame_date(author_time, &author_tz);

        results.push(BlameLine {
            line: final_line,
            hash: hash[..8.min(hash.len())].to_string(), // short hash for readability
            author_name,
            author_email,
            date,
            content,
        });
    }

    Ok(results)
}

/// Parse a timezone offset string like "+0300", "-0500", "+0545" into seconds.
/// Returns 0 for invalid/empty input.
fn parse_tz_offset(tz: &str) -> i64 {
    if tz.len() < 5 {
        return 0;
    }
    let sign: i64 = if tz.starts_with('-') { -1 } else { 1 };
    let hours: i64 = tz[1..3].parse().unwrap_or(0);
    let minutes: i64 = tz[3..5].parse().unwrap_or(0);
    sign * (hours * 3600 + minutes * 60)
}

/// Format a Unix timestamp + timezone offset into "YYYY-MM-DD HH:MM:SS <tz>" string.
/// Applies the timezone offset to get local civil time before formatting.
pub(crate) fn format_blame_date(timestamp: i64, tz: &str) -> String {
    // Apply timezone offset to get local time
    let offset = parse_tz_offset(tz);
    let local_timestamp = timestamp + offset;

    let secs_per_day: i64 = 86400;
    let days = if local_timestamp >= 0 { local_timestamp / secs_per_day } else { (local_timestamp - secs_per_day + 1) / secs_per_day };
    let time_of_day = local_timestamp - days * secs_per_day;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let days_civil = days + 719468;
    let era = if days_civil >= 0 { days_civil } else { days_civil - 146096 } / 146097;
    let doe = (days_civil - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}", y, m, d, hours, minutes, seconds, tz)
}

pub mod cache;

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cache_tests.rs"]
mod cache_tests;