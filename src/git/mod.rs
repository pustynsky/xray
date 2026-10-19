//! Git history query module — calls `git` CLI for optimal performance.
//!
//! Uses `git log` CLI with commit-graph and bloom filter optimizations for
//! path-limited queries. On-demand fallback when in-memory cache is not available.
//! See `cache.rs` for the pre-built in-memory cache path (sub-millisecond queries).

use std::collections::HashMap;
use std::path::Path;
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
    pub(crate) full_message: String,
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

/// Missing directory and "no .git here" need the identical fix, so they share
/// one sentence.
fn not_a_git_repo_error(repo: &str) -> String {
    format!(
        "Not a git repository: '{}'. xray_git_* tools need the 'repo' argument to point at a git working tree.",
        repo
    )
}

pub(crate) fn git_spawn_error(repo: &str, err: &std::io::Error) -> String {
    let repo = if repo.is_empty() { "." } else { repo };
    // A missing binary and an unusable working directory produce the same spawn
    // failure, so only a conclusive verdict about the path may pick a message.
    match Path::new(repo).metadata() {
        Ok(meta) if !meta.is_dir() => not_a_git_repo_error(repo),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => not_a_git_repo_error(repo),
        Err(_) => format!("Failed to start git in '{}': {}", repo, err),
        Ok(_) if err.kind() == std::io::ErrorKind::NotFound => format!(
            "git is not available: {}. Install git and make sure it is in PATH.",
            err
        ),
        Ok(_) => format!("Failed to start git in '{}': {}", repo, err),
    }
}

/// Filesystem answer to "is there a repository here", used when git's stderr is
/// localized and the English matcher cannot see it.
fn no_repository_at(repo: &str) -> bool {
    no_repository_below(Path::new(repo), std::env::var_os("GIT_DIR").is_some())
}

/// `Ok(false)` is the only conclusive "nothing here"; an I/O or ACL failure means
/// the filesystem cannot answer.
fn conclusively_absent(path: &Path) -> bool {
    matches!(path.try_exists(), Ok(false))
}

/// `git_dir_override` is a parameter rather than an inner `var_os` read so tests
/// can cover the GIT_DIR case without mutating process environment.
fn no_repository_below(repo: &Path, git_dir_override: bool) -> bool {
    // GIT_DIR redirects discovery, so a filesystem walk cannot answer.
    if git_dir_override {
        return false;
    }
    let Ok(start) = repo.canonicalize() else {
        return false;
    };
    // A bare repo IS its own git dir, so it has no `.git` to find below.
    if !conclusively_absent(&start.join("HEAD")) && !conclusively_absent(&start.join("objects")) {
        return false;
    }
    let mut dir = start.as_path();
    loop {
        // Anything but a proven absence — a gitfile worktree, a submodule, an
        // unreadable ancestor — means we must not claim there is no repository.
        if !conclusively_absent(&dir.join(".git")) {
            return false;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return true,
        }
    }
}

pub(crate) fn git_command_error(
    repo: &str,
    command: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> String {
    let stderr = stderr.trim();
    // Anchored: an unanchored match fires on any diagnostic that echoes a
    // user-supplied path. 128 is git's fatal exit, so a routine non-zero status
    // never pays for the probe.
    let not_a_repo = stderr
        .lines()
        .any(|line| line.trim_start().starts_with("fatal: not a git repository"))
        || (exit_code == Some(128) && no_repository_at(repo));
    if not_a_repo {
        return not_a_git_repo_error(repo);
    }
    let command = if command.is_empty() { "command" } else { command };
    format!("git {} failed: {}", command, stderr)
}

/// Working directory of a builder, defaulting to the inherited process cwd.
fn command_repo(cmd: &Command) -> String {
    cmd.get_current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".to_string())
}

/// Run a git command and return stdout as String.
fn run_git(cmd: &mut Command) -> Result<String, String> {
    let output = match cmd.output() {
        Ok(output) => output,
        Err(e) => return Err(git_spawn_error(&command_repo(cmd), &e)),
    };

    if !output.status.success() {
        // Only the subcommand: the full arg list carries the RECORD_SEP/FIELD_SEP
        // control chars from --format and is unreadable in an error string.
        let subcommand = cmd
            .get_args()
            .next()
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(git_command_error(
            &command_repo(cmd),
            &subcommand,
            &stderr,
            output.status.code(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn add_log_filters(
    cmd: &mut Command,
    author_filter: Option<&str>,
    message_filter: Option<&str>,
) {
    if author_filter.is_some() || message_filter.is_some() {
        cmd.arg("--fixed-strings").arg("--regexp-ignore-case");
    }
    if let Some(author) = author_filter {
        cmd.arg(format!("--author={author}"));
    }
    if let Some(message) = message_filter {
        cmd.arg(format!("--grep={message}"));
    }
}

/// Parse a git log record (using FIELD_SEP-separated fields) into CommitInfo.
fn parse_commit_record(record: &str) -> Option<CommitInfo> {
    let fields: Vec<&str> = record.split(FIELD_SEP).collect();
    if fields.len() < 5 {
        return None;
    }

    // The trailing FIELD_SEP leaves an empty final chunk.
    let message_end = if fields.last().map(|s| s.trim().is_empty()).unwrap_or(false) {
        fields.len() - 1
    } else {
        fields.len()
    };

    let full_message = fields[4..message_end].join(FIELD_SEP).trim().to_string();
    let message = full_message.lines().next().unwrap_or_default().trim().to_string();

    Some(CommitInfo {
        hash: fields[0].trim().to_string(),
        date: fields[1].trim().to_string(),
        author_name: fields[2].trim().to_string(),
        author_email: fields[3].trim().to_string(),
        message,
        full_message,
        patch: None,
    })
}

fn parse_full_commit_output(output: &str) -> Vec<CommitInfo> {
    let mut fields = output.split('\0');
    let mut commits = Vec::new();
    while let Some(hash) = fields.next() {
        let hash = hash.trim_matches(['\r', '\n']);
        if hash.is_empty() {
            break;
        }
        let Some(date) = fields.next() else { break };
        let Some(author_name) = fields.next() else { break };
        let Some(author_email) = fields.next() else { break };
        let Some(message) = fields.next() else { break };
        let Some(full_message) = fields.next() else { break };
        commits.push(CommitInfo {
            hash: hash.to_string(),
            date: date.trim().to_string(),
            author_name: author_name.trim().to_string(),
            author_email: author_email.trim().to_string(),
            message: message.trim().to_string(),
            full_message: full_message.trim().to_string(),
            patch: None,
        });
    }
    commits
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
        repo_path, file, filter, max_results, author_filter, message_filter,
        FileHistoryQueryMode::FOLLOW_SUBJECT,
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
            repo_path, file, filter, max_results, author_filter, message_filter,
            FileHistoryQueryMode::DIRECT_SUBJECT,
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

/// Return followed file history with full commit messages retained for post-filtering.
pub fn file_history_with_full_messages(
    repo_path: &str,
    file: &str,
    filter: &DateFilter,
) -> Result<(Vec<CommitInfo>, usize), String> {
    let (mut commits, mut total_count) = run_file_history_query(
        repo_path, file, filter, 0, None, None, FileHistoryQueryMode::FOLLOW_FULL,
    )?;
    if total_count == 0 {
        let (no_follow_commits, no_follow_total) = run_file_history_query(
            repo_path, file, filter, 0, None, None, FileHistoryQueryMode::DIRECT_FULL,
        )?;
        if no_follow_total > 0 {
            commits = no_follow_commits;
            total_count = no_follow_total;
        }
    }
    Ok((commits, total_count))
}

#[derive(Clone, Copy)]
struct FileHistoryQueryMode {
    follow: bool,
    include_full_message: bool,
}

impl FileHistoryQueryMode {
    const FOLLOW_SUBJECT: Self = Self { follow: true, include_full_message: false };
    const DIRECT_SUBJECT: Self = Self { follow: false, include_full_message: false };
    const FOLLOW_FULL: Self = Self { follow: true, include_full_message: true };
    const DIRECT_FULL: Self = Self { follow: false, include_full_message: true };
}

/// Run one `git log` query with or without `--follow`.
fn run_file_history_query(
    repo_path: &str,
    file: &str,
    filter: &DateFilter,
    max_results: usize,
    author_filter: Option<&str>,
    message_filter: Option<&str>,
    mode: FileHistoryQueryMode,
) -> Result<(Vec<CommitInfo>, usize), String> {
    let format = if mode.include_full_message {
        "%H%x00%ai%x00%an%x00%ae%x00%s%x00%B%x00".to_string()
    } else {
        format!("{}%H{}%ai{}%an{}%ae{}%s{}", RECORD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP)
    };

    let mut cmd = crate::background_command("git");
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

    if mode.follow {
        cmd.arg("--follow");
    }

    add_date_args(&mut cmd, filter);

    add_log_filters(&mut cmd, author_filter, message_filter);

    cmd.arg("--").arg(file);

    let output = run_git(&mut cmd)?;

    let mut commits: Vec<CommitInfo> = if mode.include_full_message {
        parse_full_commit_output(&output)
    } else {
        output
            .split(RECORD_SEP)
            .filter(|record| !record.trim().is_empty())
            .filter_map(parse_commit_record)
            .collect()
    };

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
    // PERF-03 follow-up: `--first-parent` is REQUIRED for merge commits.
    // Default `git show <merge>` produces a *combined* diff that prunes
    // "uninteresting" paths (paths where the merge result equals at least
    // one parent) — for a typical merge of feature into main, that means
    // an EMPTY patch on every file the merge actually touched, because
    // the merge result equals the feature side. The legacy `git diff
    // <hash>^..<hash>` was implicitly first-parent (`^` resolves to
    // parent #1), so without `--first-parent` here a merge commit's
    // patch silently went from "normal diff vs trunk" to empty string.
    // Verified empirically with a temp repo (theirs-strategy merge) where
    // the default `git show` returned 0 lines and `--first-parent`
    // matched `git diff HEAD^..HEAD` exactly. For non-merge commits
    // `--first-parent` is a no-op (only one parent exists).
    //
    // Net effect: 200-commit `xray_git_history file=… includeDiff=true`
    // drops from 400 → 200 spawns (≈1–4s saved on Windows).
    let mut cmd = crate::background_command("git");
    cmd.current_dir(repo_path)
        .arg("show")
        .arg("--first-parent")
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

/// Get the commit that introduced a file (the "creation" commit).
///
/// Uses `git log --follow --diff-filter=A --max-count=1` so the result is
/// correct across renames (--follow walks back through rename detection) and
/// `--diff-filter=A` ensures we pick the commit that classifies as Added for
/// this path, not just the oldest reachable modification. For files deleted
/// from current HEAD, `--follow` may still find the original add via tree
/// traversal in many cases; when it returns nothing, we retry without
/// `--follow` (which can succeed on truly deleted-without-rename files where
/// `--follow` bails). Default git rename detection (`diff.renames=true`)
/// also helps the no-follow path classify a rename commit's Add side.
///
/// Returns `Ok(None)` when both queries produced no commit at all (the path
/// never existed in this repository).
///
/// # Why dedicated rather than "oldest entry of file_history"
///
///   * `--diff-filter=A` is the only flag that classifies a commit as the
///     creation event for this path. Hand-rolling "oldest of full history"
///     requires materialising every commit touching the file just to take
///     the last one.
///   * Bypasses the in-memory git cache: cache stores commits in branch-tip
///     order without rename graph reconstruction or per-commit add/modify
///     classification, so cache-derived "oldest" would be wrong on renamed
///     files and indistinguishable from a modify on first-time-seen paths.
///
/// # Why no `--reverse`
///
/// Combining `--reverse` with `--follow` empirically drops the creation
/// commit during git's history simplification (verified on git 2.40+).
/// Without `--reverse`, `--max-count=1` plus `--diff-filter=A` returns the
/// single Add event directly.
pub fn file_first_commit(repo_path: &str, file: &str) -> Result<Option<CommitInfo>, String> {
    if let Some(c) = run_first_commit_query(repo_path, file, true)? {
        return Ok(Some(c));
    }
    // Fallback: if the --follow query returns no Add event (rare; e.g. some
    // deleted-without-rename histories where --follow's tree traversal yields
    // nothing), retry without --follow. Plain --diff-filter=A still finds the
    // original add for paths that were never renamed.
    run_first_commit_query(repo_path, file, false)
}

fn run_first_commit_query(
    repo_path: &str,
    file: &str,
    follow: bool,
) -> Result<Option<CommitInfo>, String> {
    let format = format!(
        "{}%H{}%ai{}%an{}%ae{}%s{}",
        RECORD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP
    );

    let mut cmd = crate::background_command("git");
    cmd.current_dir(repo_path)
        .arg("log")
        .arg(format!("--format={}", format))
        .arg("--diff-filter=A")
        .arg("--max-count=1");

    if follow {
        cmd.arg("--follow");
    }

    cmd.arg("--").arg(file);

    let output = run_git(&mut cmd)?;

    Ok(output
        .split(RECORD_SEP)
        .filter(|s| !s.trim().is_empty())
        .filter_map(parse_commit_record)
        .next())
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

    let mut cmd = crate::background_command("git");
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

    add_log_filters(&mut cmd, None, message_filter);

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
    // PERF-04 follow-up: stable secondary sort key (name asc, then email asc)
    // makes the ranking fully deterministic on tie. Pre-fix the only sort key
    // was `Reverse(count)`, so authors with equal commit counts came out in
    // HashMap iteration order — which depends on the hash of the key type.
    // Switching the key from `String` (`format!("{} <{}>", …)`) to
    // `(String, String)` in PERF-04 changed that hash, silently flipping tie
    // ordering between callers. Fully-specified comparator pins the order
    // for snapshot tests / golden output / paginated UIs.
    ranked.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.email.cmp(&b.email))
    });
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

    let mut cmd = crate::background_command("git");
    cmd.current_dir(repo_path)
        .arg("log")
        .arg(format!("--format={}", format))
        .arg("--name-only");

    add_date_args(&mut cmd, filter);

    // Safety cap: repo_activity with --name-only produces more output per commit.
    // 10K commits is a reasonable limit for activity overview.
    cmd.arg("--max-count=10000");

    add_log_filters(&mut cmd, author_filter, message_filter);

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
    let mut cmd = crate::background_command("git");
    cmd.current_dir(repo)
        .arg("ls-files")
        .arg("--")
        .arg(file);

    match run_git(&mut cmd) {
        Ok(output) => !output.trim().is_empty(),
        Err(_) => false,
    }
}

/// Return the current HEAD commit hash of the repo, or `None` if git fails
/// (bare repo with no commits, missing git, etc.).
///
/// Used by the cache-empty-result HEAD-pinning check (user story
/// 2026-05-10): when a cached query returns empty AND the cache's snapshot
/// HEAD differs from the live HEAD, the empty result is potentially stale
/// (e.g. file committed AFTER cache build) and the caller falls through
/// to the CLI fallback for an authoritative answer.
///
/// Cost: one `git rev-parse HEAD` (~1-3 ms). Called only on empty cache
/// results, so non-empty queries pay zero overhead.
pub fn current_head_hash(repo: &str) -> Option<String> {
    let mut cmd = crate::background_command("git");
    cmd.current_dir(repo).args(["rev-parse", "HEAD"]);
    match run_git(&mut cmd) {
        Ok(output) => {
            let trimmed = output.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
        }
        Err(_) => None,
    }
}


/// Information about a shallow-cloned repository.
///
/// Set when the repository was created via `git clone --depth=N` (or had a
/// later `--depth` fetch added). The boundaries are commit hashes that mark
/// where git's local view of history ends — anything older is invisible to
/// `git log`, `git blame`, `git diff`, etc., even though it exists on the
/// remote.
///
/// Source: contents of `<repo>/.git/shallow` (one hex hash per line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShallowInfo {
    /// Commit hashes at which history is grafted (truncated). Each entry is
    /// the OLDEST commit visible on its history branch — its parents are not
    /// in the local object store.
    pub boundaries: Vec<String>,
}

impl ShallowInfo {
    /// Format a single-line warning suitable for embedding in tool responses.
    pub fn warning_text(&self) -> String {
        if self.boundaries.is_empty() {
            return String::new();
        }
        let preview: Vec<&str> = self
            .boundaries
            .iter()
            .take(3)
            .map(|s| s.get(..s.len().min(12)).unwrap_or(s.as_str()))
            .collect();
        let more = if self.boundaries.len() > 3 {
            format!(" (+{} more)", self.boundaries.len() - 3)
        } else {
            String::new()
        };
        format!(
            "Repository is shallow-cloned (graft boundary: {}{}). \
             History before these commits is INVISIBLE to git locally — counts \
             and authorship may be incomplete. Run `git fetch --unshallow` for \
             the full history.",
            preview.join(", "),
            more
        )
    }
}

// ─── Shallow-clone state cache ───────────────────────────────────
//
// Per-repo memo of where this repo's `shallow` file lives. The path itself
// is stable for the lifetime of the process (gitdir does not move) and
// resolving it costs one `git rev-parse --git-path shallow` subprocess
// (~5 ms cold), so caching the path saves the subprocess on every
// subsequent call. The shallow file CONTENTS are NOT memoised — every
// call to `shallow_fingerprint` re-reads the (typically <1 KB) file. This
// trades ~10–50 µs per request for the strongest possible coherency
// guarantee: no undocumented dependency on filesystem mtime resolution,
// no risk of preserving a stale fingerprint after `git fetch --depth=N`
// rewrites the file with an unchanged mtime.

struct ShallowState {
    /// Resolved absolute path to the `shallow` file. Computed once per repo
    /// via `git rev-parse --git-path shallow` so worktrees and submodules
    /// (where `.git` is a gitfile pointing at a separate gitdir) work.
    /// `None` if the resolution failed (not a git repo, missing git binary).
    shallow_path: Option<std::path::PathBuf>,
}

static SHALLOW_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, ShallowState>>,
> = std::sync::OnceLock::new();

fn shallow_cache() -> &'static std::sync::Mutex<HashMap<String, ShallowState>> {
    SHALLOW_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Resolve where this repo's `shallow` file would live. Works for normal
/// repos AND worktrees/submodules (`<repo>/.git` may be a gitfile pointing
/// elsewhere). Returns the path even when the file does not currently
/// exist; callers must `stat`/`read` it to detect presence.
fn resolve_shallow_path(repo: &str) -> Option<std::path::PathBuf> {
    let out = crate::background_command("git")
        .current_dir(repo)
        .args(["rev-parse", "--git-path", "shallow"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let p = std::path::Path::new(trimmed);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::path::Path::new(repo).join(p)
    })
}

/// Resolve (and memoise) the path to this repo's shallow file.
fn cached_shallow_path(repo: &str) -> Option<std::path::PathBuf> {
    let mut map = shallow_cache().lock().ok()?;
    let state = map.entry(repo.to_string()).or_insert_with(|| ShallowState {
        shallow_path: resolve_shallow_path(repo),
    });
    state.shallow_path.clone()
}

fn read_boundaries(path: &std::path::Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let bs: Vec<String> = content
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if bs.is_empty() { None } else { Some(bs) }
}

fn fingerprint_from_boundaries(mut bs: Vec<String>) -> String {
    bs.sort();
    bs.dedup();
    bs.join(",")
}

/// Read shallow-clone info from this repo's `shallow` file. Returns `None`
/// if the repo is not shallow (file missing or empty).
///
/// Resolves the actual shallow-file path via `git rev-parse --git-path
/// shallow` (memoised per repo) so worktrees and submodules whose `.git`
/// is a gitfile work correctly.
pub fn detect_shallow(repo: &str) -> Option<ShallowInfo> {
    let shallow_path = cached_shallow_path(repo)?;
    let boundaries = read_boundaries(&shallow_path)?;
    Some(ShallowInfo { boundaries })
}

/// Stable canonical representation of shallow boundaries for cache keying.
///
/// Returns `Some("hash1,hash2,...")` (sorted, deduped) when the repo is
/// shallow, `None` otherwise. A change in this value between cache build
/// time and load time MUST invalidate the cache — see
/// [`crate::git::cache::GitHistoryCache::is_valid_for_with_shallow`].
///
/// Cost per call: one stat + one read of `.git/shallow` (typically <1 KB).
/// On warm Windows NTFS this is ~10–50 µs; on Linux ~2–10 µs. The shallow
/// file is intentionally NOT memoised by content/mtime — reading on every
/// call removes any reliance on `modified()` resolution and guarantees we
/// observe `git fetch --unshallow` and `--depth=N` boundary changes
/// immediately, even when the filesystem reports an unchanged mtime.
pub fn shallow_fingerprint(repo: &str) -> Option<String> {
    let path = cached_shallow_path(repo)?;
    let bs = read_boundaries(&path)?;
    Some(fingerprint_from_boundaries(bs))
}

#[cfg(test)]
pub fn shallow_cache_clear() {
    if let Some(m) = SHALLOW_CACHE.get()
        && let Ok(mut g) = m.lock()
    {
        g.clear();
    }
}


// ─── Working-tree line-ending policy ─────────────────────────────────
// Answers "which line ending would Git put in the working tree for this path",
// so a file created by xray_edit can be `git add`ed without tripping
// core.safecrlf. Read-only: never runs `git add` and never writes config.

/// Line ending Git materializes in the working tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorktreeEol {
    Lf,
    Crlf,
}

/// Which Git rule produced a [`WorktreeEol`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EolSource {
    /// `eol=lf` / `eol=crlf` from an effective `.gitattributes`.
    GitattributesEol,
    /// `text` / `text=auto` combined with `core.autocrlf` / `core.eol`.
    WorktreePolicy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AutoCrlf {
    True,
    Input,
    False,
}

#[derive(Clone, Copy, Default)]
struct EolConfig {
    autocrlf: Option<AutoCrlf>,
    eol: Option<WorktreeEol>,
}

fn native_eol() -> WorktreeEol {
    if cfg!(windows) { WorktreeEol::Crlf } else { WorktreeEol::Lf }
}

/// Line ending Git would write into the working tree for `path`.
///
/// `None` means "Git has no opinion": no worktree above the path, Git missing
/// or failing, or an attribute/config combination under which Git performs no
/// conversion at all. The caller then applies its own default.
///
/// The path does NOT have to exist — `git check-attr` is pattern-based, which
/// is exactly what a not-yet-created file needs. No Git process is spawned
/// when there is no `.git` above the path.
pub fn worktree_line_ending(path: &Path) -> Option<(WorktreeEol, EolSource)> {
    // The write follows symlinks, so the policy must come from where the bytes
    // actually land, not from the lexical path the caller typed.
    let physical = physical_target_path(path)?;
    let root = discover_worktree_root(&physical)?;
    let relative = worktree_relative_path(&root, &physical)?;
    let (text, eol) = check_attr_line_ending_inputs(&root, &relative)?;

    // An explicit `eol` attribute outranks every config knob.
    match eol.as_str() {
        "lf" => return Some((WorktreeEol::Lf, EolSource::GitattributesEol)),
        "crlf" => return Some((WorktreeEol::Crlf, EolSource::GitattributesEol)),
        _ => {}
    }

    let config = read_eol_config(&root);
    let resolved = match text.as_str() {
        // Declared text: always converted. core.eol is ignored while
        // core.autocrlf is true or input, per git-config(1).
        "set" | "auto" => match config.autocrlf {
            Some(AutoCrlf::True) => WorktreeEol::Crlf,
            Some(AutoCrlf::Input) => WorktreeEol::Lf,
            _ => config.eol.unwrap_or_else(native_eol),
        },
        // Undeclared text: core.autocrlf alone decides, and `false` means
        // Git writes the bytes through untouched.
        "unspecified" => match config.autocrlf {
            Some(AutoCrlf::True) => WorktreeEol::Crlf,
            Some(AutoCrlf::Input) => WorktreeEol::Lf,
            _ => return None,
        },
        // `-text` (unset) or an unrecognized value: no conversion.
        _ => return None,
    };
    Some((resolved, EolSource::WorktreePolicy))
}

/// Where the write will physically land: the nearest existing ancestor is
/// canonicalized — so a directory symlink attributes the file to the repository
/// it really points into — and the not-yet-created suffix is re-appended.
fn physical_target_path(path: &Path) -> Option<std::path::PathBuf> {
    let mut missing_suffix: Vec<&std::ffi::OsStr> = Vec::new();
    let mut current = path;
    loop {
        if let Ok(resolved) = std::fs::canonicalize(current) {
            let mut physical = strip_verbatim_prefix(resolved);
            for component in missing_suffix.iter().rev() {
                physical.push(component);
            }
            return Some(physical);
        }
        missing_suffix.push(current.file_name()?);
        current = current.parent()?;
    }
}

/// Windows canonicalization yields verbatim paths (`\\?\C:\…`), which
/// `CreateProcess` rejects as a working directory. Convert the two forms that
/// have an ordinary equivalent; anything else (device namespace, Volume GUID)
/// is left alone rather than corrupted into a relative path.
fn strip_verbatim_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        enum VerbatimKind {
            Disk,
            Unc,
            Other,
        }
        use std::path::{Component, Prefix};
        let kind = match path.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(_) => VerbatimKind::Disk,
                Prefix::VerbatimUNC(_, _) => VerbatimKind::Unc,
                _ => VerbatimKind::Other,
            },
            _ => VerbatimKind::Other,
        };
        let Some(text) = path.to_str() else {
            return path;
        };
        match kind {
            // \\?\C:\rest -> C:\rest
            VerbatimKind::Disk => {
                if let Some(rest) = text.strip_prefix(r"\\?\") {
                    return std::path::PathBuf::from(rest);
                }
            }
            // \\?\UNC\server\share\rest -> \\server\share\rest
            VerbatimKind::Unc => {
                if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
                    return std::path::PathBuf::from(format!(r"\\{}", rest));
                }
            }
            VerbatimKind::Other => {}
        }
    }
    path
}

/// Nearest ancestor directory holding a `.git` entry. `.git` may be a file
/// (linked worktree or submodule), so existence is enough.
fn discover_worktree_root(path: &Path) -> Option<std::path::PathBuf> {
    let mut current = path.parent();
    while let Some(directory) = current {
        if directory.join(".git").exists() {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    None
}

/// Root-relative, forward-slash path — the form `git check-attr` expects.
fn worktree_relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(relative.to_str()?.replace('\\', "/"))
}

/// Effective `text` and `eol` for a path, including the legacy `crlf`
/// attribute. `git check-attr text eol` does NOT surface `crlf`, yet Git still
/// honors it — `git ls-files --eol` reports `attr/text` for a `* crlf` rule — so
/// the compatibility table from gitattributes(5) is applied here.
fn check_attr_line_ending_inputs(root: &Path, relative: &str) -> Option<(String, String)> {
    let mut cmd = crate::background_command("git");
    cmd.current_dir(root)
        .args(["check-attr", "-z", "text", "eol", "crlf", "--"])
        .arg(relative);
    let output = run_git(&mut cmd).ok()?;

    // -z output is a flat NUL-separated stream of (path, attribute, value).
    let fields: Vec<&str> = output.split('\0').collect();
    let mut text = None;
    let mut eol = None;
    let mut crlf = None;
    for chunk in fields.chunks(3) {
        let [_, attribute, value] = chunk else { continue };
        match *attribute {
            "text" => text = Some((*value).to_string()),
            "eol" => eol = Some((*value).to_string()),
            "crlf" => crlf = Some((*value).to_string()),
            _ => {}
        }
    }
    Some(apply_legacy_crlf_attribute(text?, eol?, crlf?))
}

/// gitattributes(5) compatibility: `crlf` -> `text`, `-crlf` -> `-text`,
/// `crlf=input` -> `eol=lf`. Git drops the legacy attribute entirely as soon as
/// a modern one applies — verified against `git ls-files --eol` for
/// `text -crlf`, `-text crlf`, `eol=crlf crlf=input` and `text=auto crlf=input`,
/// where the effective attribute was `text`, `-text`, `text eol=crlf` and
/// `text=auto` respectively.
fn apply_legacy_crlf_attribute(text: String, eol: String, crlf: String) -> (String, String) {
    if text != "unspecified" || eol != "unspecified" {
        return (text, eol);
    }
    match crlf.as_str() {
        "input" => (text, "lf".to_string()),
        "set" => ("set".to_string(), eol),
        "unset" => ("unset".to_string(), eol),
        _ => (text, eol),
    }
}

/// `core.autocrlf` / `core.eol` as Git resolves them for this worktree — system,
/// global, local, and `include`/`includeIf` are all folded in by Git itself.
/// Not cached: config is read once per created file, and a stale answer would
/// be worse than the spawn it saves.
fn read_eol_config(root: &Path) -> EolConfig {
    let mut cmd = crate::background_command("git");
    cmd.current_dir(root)
        .args(["config", "-z", "--get-regexp", r"^core\.(autocrlf|eol)$"]);
    // Exit 1 here just means neither key is set; check-attr already proved Git works.
    let Ok(output) = run_git(&mut cmd) else {
        return EolConfig::default();
    };

    let mut config = EolConfig::default();
    // Entries arrive lowest-precedence first, so a later one legitimately wins.
    for entry in output.split('\0').filter(|entry| !entry.is_empty()) {
        let Some((key, value)) = entry.split_once('\n') else { continue };
        let value = value.trim().to_ascii_lowercase();
        match key.trim().to_ascii_lowercase().as_str() {
            "core.autocrlf" => {
                let parsed = match value.as_str() {
                    "true" | "yes" | "on" | "1" => Some(AutoCrlf::True),
                    "input" => Some(AutoCrlf::Input),
                    "false" | "no" | "off" | "0" | "" => Some(AutoCrlf::False),
                    _ => None,
                };
                if parsed.is_some() {
                    config.autocrlf = parsed;
                }
            }
            // Anything else, including "native", falls back to the platform default.
            "core.eol" => {
                config.eol = match value.as_str() {
                    "lf" => Some(WorktreeEol::Lf),
                    "crlf" => Some(WorktreeEol::Crlf),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    config
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
    let mut cmd = crate::background_command("git");
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
    let mut cmd = crate::background_command("git");
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

    let mut cmd = crate::background_command("git");
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
mod line_ending_policy_tests {
    use super::*;

    #[test]
    fn legacy_crlf_applies_only_when_no_modern_attribute_is_set() {
        let call = |text: &str, eol: &str, crlf: &str| {
            apply_legacy_crlf_attribute(text.to_string(), eol.to_string(), crlf.to_string())
        };

        // Alone, the legacy attribute maps per gitattributes(5).
        assert_eq!(call("unspecified", "unspecified", "set"), ("set".into(), "unspecified".into()));
        assert_eq!(call("unspecified", "unspecified", "unset"), ("unset".into(), "unspecified".into()));
        assert_eq!(call("unspecified", "unspecified", "input"), ("unspecified".into(), "lf".into()));

        // A modern attribute suppresses it entirely. `git ls-files --eol` reports
        // the effective attribute as `text`, `-text`, `text eol=crlf` and
        // `text=auto` for these four combinations.
        assert_eq!(call("set", "unspecified", "unset"), ("set".into(), "unspecified".into()));
        assert_eq!(call("unset", "unspecified", "set"), ("unset".into(), "unspecified".into()));
        assert_eq!(call("unspecified", "crlf", "input"), ("unspecified".into(), "crlf".into()));
        assert_eq!(call("auto", "unspecified", "input"), ("auto".into(), "unspecified".into()));

        // No legacy rule at all.
        assert_eq!(call("auto", "unspecified", "unspecified"), ("auto".into(), "unspecified".into()));
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_disk_and_unc_prefixes_become_usable_working_directories() {
        use std::path::PathBuf;

        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\repo\src")),
            PathBuf::from(r"C:\repo\src")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\repo")),
            PathBuf::from(r"\\server\share\repo")
        );
    }

    /// A Volume GUID path has no ordinary equivalent: stripping the prefix would
    /// turn it into a relative path, so it must be returned untouched.
    #[cfg(windows)]
    #[test]
    fn other_verbatim_prefixes_are_left_untouched() {
        use std::path::PathBuf;

        let volume = PathBuf::from(r"\\?\Volume{11111111-2222-3333-4444-555555555555}\repo");
        assert_eq!(strip_verbatim_prefix(volume.clone()), volume);

        let device = PathBuf::from(r"\\.\PIPE\name");
        assert_eq!(strip_verbatim_prefix(device.clone()), device);

        let ordinary = PathBuf::from(r"C:\repo\src");
        assert_eq!(strip_verbatim_prefix(ordinary.clone()), ordinary);
    }
}


#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cache_tests.rs"]
mod cache_tests;