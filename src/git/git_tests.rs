//! Unit tests for the git history module (CLI-based implementation).
//!
//! Tests run against the search repository itself (which has a `.git/` directory).
//! Requires `git` to be in PATH.

use super::*;

// ─── Date validation tests ──────────────────────────────────────────

#[test]
fn test_validate_date_valid() {
    assert!(validate_date("2025-01-15").is_ok());
}

#[test]
fn test_validate_date_epoch() {
    assert!(validate_date("1970-01-01").is_ok());
}

#[test]
fn test_validate_date_invalid_format() {
    assert!(validate_date("not-a-date").is_err());
}

#[test]
fn test_validate_date_invalid_month() {
    assert!(validate_date("2025-13-01").is_err());
}

#[test]
fn test_validate_date_invalid_day() {
    assert!(validate_date("2025-01-32").is_err());
}

#[test]
fn test_validate_date_too_short() {
    assert!(validate_date("2025-1-1").is_err());
}

// ─── next_day tests ─────────────────────────────────────────────────

#[test]
fn test_next_day_normal() {
    assert_eq!(next_day("2025-01-15"), "2025-01-16");
}

#[test]
fn test_next_day_end_of_month() {
    assert_eq!(next_day("2025-01-31"), "2025-02-01");
}

#[test]
fn test_next_day_end_of_year() {
    assert_eq!(next_day("2025-12-31"), "2026-01-01");
}

#[test]
fn test_next_day_leap_year_feb() {
    assert_eq!(next_day("2024-02-28"), "2024-02-29");
    assert_eq!(next_day("2024-02-29"), "2024-03-01");
}

#[test]
fn test_next_day_non_leap_feb() {
    assert_eq!(next_day("2025-02-28"), "2025-03-01");
}

// ─── Date filter tests ──────────────────────────────────────────────

#[test]
fn test_parse_date_filter_none() {
    let filter = parse_date_filter(None, None, None).unwrap();
    assert!(filter.from_date.is_none());
    assert!(filter.to_date.is_none());
}

#[test]
fn test_parse_date_filter_from_only() {
    let filter = parse_date_filter(Some("2025-01-01"), None, None).unwrap();
    assert_eq!(filter.from_date.as_deref(), Some("2025-01-01"));
    assert!(filter.to_date.is_none());
}

#[test]
fn test_parse_date_filter_to_only() {
    let filter = parse_date_filter(None, Some("2025-12-31"), None).unwrap();
    assert!(filter.from_date.is_none());
    assert_eq!(filter.to_date.as_deref(), Some("2025-12-31"));
}

#[test]
fn test_parse_date_filter_exact_date_overrides() {
    let filter = parse_date_filter(Some("2024-01-01"), Some("2024-12-31"), Some("2025-06-15")).unwrap();
    // date should override from/to
    assert_eq!(filter.from_date.as_deref(), Some("2025-06-15"));
    assert_eq!(filter.to_date.as_deref(), Some("2025-06-15"));
}

#[test]
fn test_parse_date_filter_invalid_date() {
    let result = parse_date_filter(Some("bad"), None, None);
    assert!(result.is_err());
}

// ─── File history tests (using search repo itself) ──────────────────

#[test]
fn test_file_history_returns_commits() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = file_history(".", "Cargo.toml", &filter, false, 10, None, None);
    assert!(result.is_ok(), "Should succeed on own repo: {:?}", result.err());
    let (commits, total) = result.unwrap();
    assert!(!commits.is_empty(), "Cargo.toml should have commit history");
    assert!(total >= commits.len(), "Total should be >= returned commits");
}

#[test]
fn test_file_history_nonexistent_file() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = file_history(".", "nonexistent_file_xyz_abc_123.rs", &filter, false, 50, None, None);
    assert!(result.is_ok(), "Should succeed even for nonexistent file");
    let (commits, total) = result.unwrap();
    assert!(commits.is_empty(), "Nonexistent file should have no commits");
    assert_eq!(total, 0);
}

#[test]
fn test_file_history_bad_repo() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = file_history("C:\\nonexistent\\repo\\path\\xyz", "file.rs", &filter, false, 50, None, None);
    assert!(result.is_err(), "Should fail for nonexistent repo");
}

#[test]
fn test_file_history_max_results_limits_output() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = file_history(".", "Cargo.toml", &filter, false, 2, None, None);
    assert!(result.is_ok());
    let (commits, total) = result.unwrap();
    assert!(commits.len() <= 2, "Should return at most 2 commits");
    assert!(total >= commits.len(), "Total should count all matching commits");
}

#[test]
fn test_file_history_with_diff() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = file_history(".", "Cargo.toml", &filter, true, 3, None, None);
    assert!(result.is_ok());
    let (commits, _) = result.unwrap();
    assert!(!commits.is_empty());
    // At least one commit should have a non-empty patch
    let has_patch = commits.iter().any(|c| c.patch.as_ref().is_some_and(|p| !p.is_empty()));
    assert!(has_patch, "Diff mode should include patch text");
}

#[test]
fn test_file_history_date_filter_narrows_results() {
    let no_filter = DateFilter { from_date: None, to_date: None };
    let (_, all_total) = file_history(".", "Cargo.toml", &no_filter, false, 0, None, None).unwrap();

    // Filter to a very old date range that likely has no commits
    let narrow_filter = parse_date_filter(Some("1970-01-01"), Some("1970-01-02"), None).unwrap();
    let (narrow_commits, narrow_total) = file_history(".", "Cargo.toml", &narrow_filter, false, 0, None, None).unwrap();

    assert!(narrow_total <= all_total, "Narrow filter should return <= total commits");
    assert!(narrow_commits.is_empty(), "1970 date range should have no commits");
}

// ─── Top authors tests ──────────────────────────────────────────────

#[test]
fn test_top_authors_returns_ranked() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = top_authors(".", "Cargo.toml", &filter, 10, None);
    assert!(result.is_ok(), "Should succeed: {:?}", result.err());
    let (authors, total_commits, total_authors) = result.unwrap();
    assert!(!authors.is_empty(), "Should have at least one author");
    assert!(total_commits > 0);
    assert!(total_authors > 0);

    // Verify ranking: each author should have >= commits than the next
    for i in 1..authors.len() {
        assert!(
            authors[i - 1].commit_count >= authors[i].commit_count,
            "Authors should be ranked by commit count (descending)"
        );
    }
}

#[test]
fn test_top_authors_nonexistent_file() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = top_authors(".", "nonexistent_xyz_abc_123.rs", &filter, 10, None);
    assert!(result.is_ok());
    let (authors, total_commits, _) = result.unwrap();
    assert!(authors.is_empty());
    assert_eq!(total_commits, 0);
}

#[test]
fn test_top_authors_limits_results() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = top_authors(".", "Cargo.toml", &filter, 1, None);
    assert!(result.is_ok());
    let (authors, _, _) = result.unwrap();
    assert!(authors.len() <= 1, "Should return at most 1 author");
}

// ─── Repo activity tests ────────────────────────────────────────────

#[test]
fn test_repo_activity_returns_files() {
    // Use a broad date range
    let filter = parse_date_filter(Some("2020-01-01"), Some("2030-12-31"), None).unwrap();
    let result = repo_activity(".", &filter, None, None, None);
    assert!(result.is_ok(), "Should succeed: {:?}", result.err());
    let (files, commits_processed) = result.unwrap();
    assert!(!files.is_empty(), "Should have at least one file with changes");
    assert!(commits_processed > 0, "Should have processed some commits");
}

#[test]
fn test_repo_activity_empty_date_range() {
    let filter = parse_date_filter(Some("1970-01-01"), Some("1970-01-02"), None).unwrap();
    let result = repo_activity(".", &filter, None, None, None);
    assert!(result.is_ok());
    let (files, _) = result.unwrap();
    assert!(files.is_empty(), "Very old date range should have no activity");
}

#[test]
fn test_repo_activity_with_path_filter() {
    let filter = parse_date_filter(Some("2020-01-01"), Some("2030-12-31"), None).unwrap();
    let result = repo_activity(".", &filter, None, None, Some("src/git"));
    assert!(result.is_ok(), "Should succeed: {:?}", result.err());
    let (files, commits_processed) = result.unwrap();
    assert!(!files.is_empty(), "Should have files in src/git");
    assert!(commits_processed > 0, "Should have processed commits");
    // All returned files should be under src/git
    for path in files.keys() {
        assert!(
            path.starts_with("src/git"),
            "File '{}' should be under src/git",
            path
        );
    }
}

#[test]
fn test_repo_activity_with_path_filter_no_results() {
    let filter = parse_date_filter(Some("2020-01-01"), Some("2030-12-31"), None).unwrap();
    let result = repo_activity(".", &filter, None, None, Some("nonexistent_directory_xyz"));
    assert!(result.is_ok(), "Should succeed even with nonexistent path");
    let (files, _) = result.unwrap();
    assert!(files.is_empty(), "Nonexistent path should return no files");
}

#[test]
fn test_repo_activity_bad_repo() {
    let filter = DateFilter { from_date: None, to_date: None };
    let result = repo_activity("C:\\nonexistent\\repo\\xyz", &filter, None, None, None);
    assert!(result.is_err(), "Should fail for nonexistent repo");
}

// ─── Commit info field tests ────────────────────────────────────────

#[test]
fn test_commit_info_has_all_fields() {
    let filter = DateFilter { from_date: None, to_date: None };
    let (commits, _) = file_history(".", "Cargo.toml", &filter, false, 1, None, None).unwrap();
    assert!(!commits.is_empty());

    let commit = &commits[0];
    assert!(!commit.hash.is_empty(), "Hash should not be empty");
    assert!(commit.hash.len() >= 40, "Hash should be full SHA, got len={}", commit.hash.len());
    assert!(!commit.date.is_empty(), "Date should not be empty");
    assert!(!commit.author_name.is_empty(), "Author name should not be empty");
    assert!(!commit.author_email.is_empty(), "Author email should not be empty");
    assert!(!commit.message.is_empty(), "Message should not be empty");
    assert!(commit.patch.is_none(), "Patch should be None when include_diff=false");
}

// ─── Git blame tests ────────────────────────────────────────────────

#[test]
fn test_blame_lines_returns_results() {
    // Blame a known file in the search repo itself
    let result = blame_lines(".", "Cargo.toml", 1, Some(3));
    assert!(result.is_ok(), "Should succeed: {:?}", result.err());
    let lines = result.unwrap();
    assert_eq!(lines.len(), 3, "Should return 3 blame lines");

    for line in &lines {
        assert!(!line.hash.is_empty(), "Hash should not be empty");
        assert!(!line.author_name.is_empty(), "Author should not be empty");
        assert!(!line.date.is_empty(), "Date should not be empty");
    }
}

#[test]
fn test_blame_lines_single_line() {
    let result = blame_lines(".", "Cargo.toml", 1, None);
    assert!(result.is_ok());
    let lines = result.unwrap();
    assert_eq!(lines.len(), 1, "Should return exactly 1 line");
    assert_eq!(lines[0].line, 1);
}

#[test]
fn test_blame_lines_nonexistent_file() {
    let result = blame_lines(".", "nonexistent_xyz_abc_123.rs", 1, Some(5));
    assert!(result.is_err(), "Should fail for nonexistent file");
}

#[test]
fn test_blame_lines_bad_repo() {
    let result = blame_lines("C:\\nonexistent\\repo\\xyz", "file.rs", 1, Some(5));
    assert!(result.is_err(), "Should fail for nonexistent repo");
}

#[test]
fn test_blame_lines_has_content() {
    let result = blame_lines(".", "Cargo.toml", 1, Some(1));
    assert!(result.is_ok());
    let lines = result.unwrap();
    assert!(!lines.is_empty());
    // First line of Cargo.toml should contain "[package]"
    assert!(
        lines[0].content.contains("[package]"),
        "First line of Cargo.toml should be [package], got: '{}'",
        lines[0].content
    );
}

// ─── Blame porcelain parser tests ───────────────────────────────────

#[test]
fn test_parse_blame_porcelain_basic() {
    let porcelain = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 1\n\
author Alice\n\
author-mail <alice@example.com>\n\
author-time 1700000000\n\
author-tz +0000\n\
committer Alice\n\
committer-mail <alice@example.com>\n\
committer-time 1700000000\n\
committer-tz +0000\n\
summary Initial commit\n\
filename src/main.rs\n\
\tlet x = 42;\n";

    let result = parse_blame_porcelain(porcelain);
    assert!(result.is_ok());
    let lines = result.unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].line, 1);
    assert_eq!(lines[0].hash, "aaaaaaaa"); // short hash
    assert_eq!(lines[0].author_name, "Alice");
    assert_eq!(lines[0].author_email, "alice@example.com");
    assert_eq!(lines[0].content, "let x = 42;");
}

#[test]
fn test_parse_blame_porcelain_repeated_hash() {
    // When the same commit appears on multiple lines, git only emits full headers
    // on the first occurrence. Subsequent lines have just the hash line and content.
    let porcelain = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 2\n\
author Alice\n\
author-mail <alice@example.com>\n\
author-time 1700000000\n\
author-tz +0000\n\
committer Alice\n\
committer-mail <alice@example.com>\n\
committer-time 1700000000\n\
committer-tz +0000\n\
summary Initial commit\n\
filename src/main.rs\n\
\tline one\n\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 2 2\n\
\tline two\n";

    let result = parse_blame_porcelain(porcelain);
    assert!(result.is_ok());
    let lines = result.unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].line, 1);
    assert_eq!(lines[0].content, "line one");
    assert_eq!(lines[1].line, 2);
    assert_eq!(lines[1].content, "line two");
    // Both should have Alice's info (reused from cache of first occurrence)
    assert_eq!(lines[1].author_name, "Alice");
    assert_eq!(lines[1].author_email, "alice@example.com");
}

#[test]
fn test_parse_blame_porcelain_empty_input() {
    let result = parse_blame_porcelain("");
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[test]
fn test_next_day_malformed_fallback() {
    // Fallback should return original date unchanged (not append garbage)
    assert_eq!(next_day("baddate"), "baddate");
    assert_eq!(next_day(""), "");
}

// ─── format_blame_date / parse_tz_offset tests ─────────────────────

#[test]
fn test_format_blame_date() {
    // UTC — baseline (no offset)
    let date = format_blame_date(1700000000, "+0000");
    assert_eq!(date, "2023-11-14 22:13:20 +0000");
}

#[test]
fn test_format_blame_date_positive_offset() {
    // +0300 — crosses midnight (UTC 22:13 + 3h = next day 01:13)
    let date = format_blame_date(1700000000, "+0300");
    assert_eq!(date, "2023-11-15 01:13:20 +0300");
}

#[test]
fn test_format_blame_date_negative_offset() {
    // -0500 — goes back 5 hours
    let date = format_blame_date(1700000000, "-0500");
    assert_eq!(date, "2023-11-14 17:13:20 -0500");
}

#[test]
fn test_format_blame_date_nepal_offset() {
    // +0545 — quarter-hour offset (Nepal)
    let date = format_blame_date(1700000000, "+0545");
    assert_eq!(date, "2023-11-15 03:58:20 +0545");
}

#[test]
fn test_parse_tz_offset() {
    assert_eq!(parse_tz_offset("+0000"), 0);
    assert_eq!(parse_tz_offset("+0300"), 10800);
    assert_eq!(parse_tz_offset("-0500"), -18000);
    assert_eq!(parse_tz_offset("+0545"), 20700);
    assert_eq!(parse_tz_offset("+1200"), 43200);
    assert_eq!(parse_tz_offset("-1200"), -43200);
    assert_eq!(parse_tz_offset(""), 0);       // empty
    assert_eq!(parse_tz_offset("UTC"), 0);    // text zone
    assert_eq!(parse_tz_offset("+00"), 0);    // truncated
}
// ─── BUG-4: Reversed date range validation ──────────────────────────

#[test]
fn test_parse_date_filter_reversed_range_returns_error() {
    // from > to should return an error (BUG-4 fix)
    let result = parse_date_filter(Some("2026-12-31"), Some("2026-01-01"), None);
    assert!(result.is_err(), "Reversed date range (from > to) should return error");
    let err = result.unwrap_err();
    assert!(err.contains("after"), "Error should mention 'after', got: {}", err);
}

#[test]
fn test_parse_date_filter_same_dates_is_ok() {
    // from == to is valid (single-day filter)
    let result = parse_date_filter(Some("2026-06-15"), Some("2026-06-15"), None);
    assert!(result.is_ok(), "Same from/to dates should be valid");
}

#[test]
fn test_parse_date_filter_correct_order_is_ok() {
    let result = parse_date_filter(Some("2026-01-01"), Some("2026-12-31"), None);
    assert!(result.is_ok(), "Correct date order should be valid");
}

// ─── file_exists_in_git tests ───────────────────────────────────────

#[test]
fn test_file_exists_in_git_tracked_file() {
    // Cargo.toml is tracked in the search repo
    assert!(file_exists_in_current_head(".", "Cargo.toml"), "Cargo.toml should be tracked in git");
}

#[test]
fn test_file_exists_in_git_nonexistent_file() {
    assert!(
        !file_exists_in_current_head(".", "nonexistent_file_xyz_abc_123.rs"),
        "Nonexistent file should not be tracked in git"
    );
}

#[test]
fn test_file_exists_in_git_bad_repo() {
    assert!(
        !file_exists_in_current_head("C:\\nonexistent\\repo\\path\\xyz", "file.rs"),
        "Bad repo should return false"
    );
}


// ─── Deleted file support tests ──────────────────────────────────────
// These tests create a temporary git repo with a file that is added,
// modified, then deleted. They verify:
//   * file_exists_in_current_head returns false for the deleted file
//   * file_ever_existed_in_git returns true for the deleted file
//   * file_history (with internal --follow fallback) returns all commits
//   * list_tracked_files_under returns only files that currently exist in HEAD

#[cfg(test)]
fn run_git(repo: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("failed to run git");
    assert!(status.success(), "git {:?} failed", args);
}

#[cfg(test)]
fn setup_repo_with_deleted_file() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let repo = dir.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    run_git(repo, &["config", "user.email", "test@example.com"]);
    run_git(repo, &["config", "user.name", "Test Author"]);
    // Create and commit a file that will survive.
    std::fs::write(repo.join("survivor.txt"), "survivor v1\n").expect("write survivor.txt");
    run_git(repo, &["add", "survivor.txt"]);
    run_git(repo, &["commit", "-m", "add survivor", "--quiet"]);
    // Create a file that will be deleted.
    std::fs::write(repo.join("legacy.txt"), "legacy v1\n").expect("write legacy.txt");
    run_git(repo, &["add", "legacy.txt"]);
    run_git(repo, &["commit", "-m", "add legacy", "--quiet"]);
    std::fs::write(repo.join("legacy.txt"), "legacy v2\n").expect("update legacy.txt");
    run_git(repo, &["commit", "-am", "modify legacy", "--quiet"]);
    // Delete the file.
    std::fs::remove_file(repo.join("legacy.txt")).expect("remove legacy.txt");
    run_git(repo, &["commit", "-am", "delete legacy", "--quiet"]);
    dir
}

#[test]
fn test_file_exists_in_current_head_rejects_deleted() {
    let dir = setup_repo_with_deleted_file();
    let repo = dir.path().to_str().expect("repo path utf-8");
    assert!(
        !file_exists_in_current_head(repo, "legacy.txt"),
        "legacy.txt was deleted -- should not be in current HEAD"
    );
    assert!(
        file_exists_in_current_head(repo, "survivor.txt"),
        "survivor.txt is still in HEAD"
    );
}

#[test]
fn test_file_ever_existed_in_git_accepts_deleted() {
    let dir = setup_repo_with_deleted_file();
    let repo = dir.path().to_str().expect("repo path utf-8");
    assert!(
        file_ever_existed_in_git(repo, "legacy.txt"),
        "legacy.txt was deleted but must still be recognised by file_ever_existed_in_git"
    );
    assert!(
        file_ever_existed_in_git(repo, "survivor.txt"),
        "survivor.txt exists in HEAD -- must also return true"
    );
    assert!(
        !file_ever_existed_in_git(repo, "never_added.txt"),
        "never_added.txt was never committed -- must return false"
    );
}

#[test]
fn test_file_history_returns_commits_for_deleted_file() {
    // Critical scenario: deleted file must still yield history via --follow fallback.
    let dir = setup_repo_with_deleted_file();
    let repo = dir.path().to_str().expect("repo path utf-8");
    let filter = DateFilter { from_date: None, to_date: None };
    let (commits, _) = file_history(repo, "legacy.txt", &filter, false, 50, None, None)
        .expect("file_history must succeed for deleted file");
    // We expect 3 commits: add, modify, delete.
    assert!(
        commits.len() >= 3,
        "deleted file must return full history (expected >= 3, got {})",
        commits.len()
    );
    // The delete commit must be among them.
    assert!(
        commits.iter().any(|c| c.message.contains("delete legacy")),
        "history for deleted file must include the delete commit. Got: {:?}",
        commits.iter().map(|c| &c.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_file_history_returns_empty_for_never_existed_file() {
    let dir = setup_repo_with_deleted_file();
    let repo = dir.path().to_str().expect("repo path utf-8");
    let filter = DateFilter { from_date: None, to_date: None };
    let (commits, _) = file_history(repo, "never_here.txt", &filter, false, 50, None, None)
        .expect("file_history succeeds but returns empty for never-existed file");
    assert!(
        commits.is_empty(),
        "never-existed file must return empty history"
    );
}

#[test]
fn test_list_tracked_files_under_excludes_deleted() {
    let dir = setup_repo_with_deleted_file();
    let repo = dir.path().to_str().expect("repo path utf-8");
    let tracked = list_tracked_files_under(repo, ".");
    assert!(
        tracked.contains("survivor.txt"),
        "survivor.txt must be in tracked set, got {:?}",
        tracked
    );
    assert!(
        !tracked.contains("legacy.txt"),
        "legacy.txt is deleted -- must NOT appear in tracked set, got {:?}",
        tracked
    );
}

#[test]
fn test_list_tracked_files_under_bad_repo_returns_empty() {
    let tracked = list_tracked_files_under("C:\\nonexistent\\repo\\path\\xyz", ".");
    assert!(
        tracked.is_empty(),
        "bad repo must return empty set, got {:?}",
        tracked
    );
}

// ── PERF-03: get_commit_diff via `git show` ─────────────────────

/// Initial-commit regression guard. Pre-PERF-03 the path used a
/// hard-coded empty-tree SHA (`4b825dc6…`) which does not exist in
/// every clone (it only resolves when the empty tree object is
/// reachable from a ref) — so `git diff <empty-tree> <hash>` could
/// fail with `bad object` on perfectly valid initial commits in
/// freshly-init'd repos. PERF-03 switches to `git show` which handles
/// the no-parent case natively (diff against /dev/null). This test
/// builds a tempdir repo with exactly one commit and asserts the
/// returned patch contains the canonical `new file mode` /
/// `--- /dev/null` header lines so a future "optimisation" that
/// reintroduces the empty-tree SHA fails loudly here.
#[test]
fn test_file_history_with_diff_includes_initial_commit_patch() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let repo = dir.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    run_git(repo, &["config", "user.email", "perf03@example.com"]);
    run_git(repo, &["config", "user.name", "PERF-03 Test"]);
    std::fs::write(repo.join("seed.txt"), "alpha\nbeta\n").expect("write seed.txt");
    run_git(repo, &["add", "seed.txt"]);
    run_git(repo, &["commit", "-m", "initial commit (no parent)", "--quiet"]);

    let repo_str = repo.to_str().expect("repo utf-8");
    let filter = DateFilter { from_date: None, to_date: None };
    let result = file_history(repo_str, "seed.txt", &filter, true, 5, None, None);
    let (commits, _) = result.expect("file_history must succeed on initial commit");
    assert_eq!(commits.len(), 1, "exactly one commit expected");
    let patch = commits[0]
        .patch
        .as_ref()
        .expect("initial commit must include patch — pre-PERF-03 this could be empty if the empty-tree SHA was unreachable");
    assert!(
        patch.contains("new file mode"),
        "initial-commit patch must declare a new-file header, got:\n{}",
        patch
    );
    assert!(
        patch.contains("--- /dev/null"),
        "initial-commit patch must diff against /dev/null (proves git show handled no-parent case natively without the magic empty-tree SHA), got:\n{}",
        patch
    );
    assert!(
        patch.contains("+alpha") && patch.contains("+beta"),
        "initial-commit patch must include the seeded content, got:\n{}",
        patch
    );
}

/// Non-initial commit regression guard. Asserts the patch section
/// produced by PERF-03's `git show` path contains the same
/// `+`/`-` content lines you'd get from `git diff <hash>^..<hash>`.
/// Byte-identity of the patch section (after the `+++`/`---` lines)
/// was verified pre-merge by walking real history on the xray repo;
/// this test pins the structural shape that future refactors must
/// preserve.
#[test]
fn test_file_history_with_diff_normal_commit_patch_shape() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let repo = dir.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    run_git(repo, &["config", "user.email", "perf03@example.com"]);
    run_git(repo, &["config", "user.name", "PERF-03 Test"]);
    std::fs::write(repo.join("seed.txt"), "v1\n").expect("write seed.txt v1");
    run_git(repo, &["add", "seed.txt"]);
    run_git(repo, &["commit", "-m", "v1", "--quiet"]);
    std::fs::write(repo.join("seed.txt"), "v2\n").expect("write seed.txt v2");
    run_git(repo, &["commit", "-am", "v2", "--quiet"]);

    let repo_str = repo.to_str().expect("repo utf-8");
    let filter = DateFilter { from_date: None, to_date: None };
    let (commits, _) = file_history(repo_str, "seed.txt", &filter, true, 5, None, None)
        .expect("file_history must succeed");
    // commits come newest-first; index 0 is the v2 modification commit.
    let patch = commits[0]
        .patch
        .as_ref()
        .expect("v2 commit must include patch");
    assert!(
        patch.contains("-v1") && patch.contains("+v2"),
        "v2 commit patch must show -v1 / +v2, got:\n{}",
        patch
    );
    assert!(
        !patch.contains("new file mode"),
        "non-initial commit patch must NOT declare a new-file header (would mean git show treated v2 as a creation), got:\n{}",
        patch
    );
}
/// PERF-03 follow-up regression: `get_commit_diff` against a merge
/// commit MUST yield a non-empty FIRST-PARENT patch (matching legacy
/// `git diff <hash>^..<hash>` semantics), not the default `git show
/// <merge>` combined diff which prunes paths that are uninteresting
/// against AT LEAST ONE parent.
///
/// Setup: branch `feat` writes `v2-feat`, then main writes `v2-main`,
/// then merge with manual resolution to `v2-merged` (distinct from
/// both parents — otherwise `git log <file>` history simplification
/// would skip the merge anyway). Pre-fix `get_commit_diff(repo,
/// <merge>, "f.txt")` returned an EMPTY string because the default
/// `git show <merge>` produces combined-diff output for merges, and
/// combined-diff prunes paths where the merge result equals at least
/// one parent's tree (here the feature side equals `v2-feat` which
/// the merge took). Post-fix `--first-parent` restores legacy
/// behaviour: diff against parent #1 (the main-side commit).
///
/// We bypass `file_history` (which uses `--follow` by default and
/// activates aggressive history simplification that hides merges)
/// and call `get_commit_diff` directly so the fix is tested in
/// isolation — this is the function that PERF-03 changed and that
/// the regression actually lives in.
#[test]
fn test_get_commit_diff_merge_commit_uses_first_parent() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let repo = dir.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    run_git(repo, &["config", "user.email", "perf03-merge@example.com"]);
    run_git(repo, &["config", "user.name", "PERF-03 Merge Test"]);
    // Disable autocrlf for predictable byte-level patch shape on Windows.
    run_git(repo, &["config", "core.autocrlf", "false"]);

    std::fs::write(repo.join("f.txt"), "v1\n").expect("write v1");
    run_git(repo, &["add", "f.txt"]);
    run_git(repo, &["commit", "-m", "a", "--quiet"]);

    run_git(repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("f.txt"), "v2-feat\n").expect("write v2-feat");
    run_git(repo, &["commit", "-am", "b", "--quiet"]);

    run_git(repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("f.txt"), "v2-main\n").expect("write v2-main");
    run_git(repo, &["commit", "-am", "c", "--quiet"]);

    // --no-ff guarantees an actual merge commit; --no-commit + manual
    // resolution to a value distinct from both parents avoids history
    // simplification (and is more representative of real-world merges
    // with conflicts).
    run_git(
        repo,
        &[
            "merge",
            "--no-ff",
            "--no-commit",
            "--quiet",
            "--strategy-option=theirs",
            "feat",
        ],
    );
    std::fs::write(repo.join("f.txt"), "v2-merged\n").expect("write merge resolution");
    run_git(repo, &["commit", "-am", "merge", "--quiet"]);

    let repo_str = repo.to_str().expect("repo utf-8");
    let merge_hash = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse")
        .stdout;
    let merge_hash = String::from_utf8(merge_hash).expect("hash utf-8").trim().to_string();

    let patch =
        get_commit_diff(repo_str, &merge_hash, "f.txt").expect("get_commit_diff must succeed");
    assert!(
        patch.contains("-v2-main") && patch.contains("+v2-merged"),
        "merge commit patch must show first-parent diff (-v2-main / +v2-merged) — \
         pre-fix `git show <merge>` produced an EMPTY combined diff because the merge \
         result was uninteresting against the feature-side parent. Got:\n{}",
        patch
    );
}

