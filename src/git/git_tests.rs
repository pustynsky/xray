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
    assert!(file_exists_in_git(".", "Cargo.toml"), "Cargo.toml should be tracked in git");
}

#[test]
fn test_file_exists_in_git_nonexistent_file() {
    assert!(
        !file_exists_in_git(".", "nonexistent_file_xyz_abc_123.rs"),
        "Nonexistent file should not be tracked in git"
    );
}

#[test]
fn test_file_exists_in_git_bad_repo() {
    assert!(
        !file_exists_in_git("C:\\nonexistent\\repo\\path\\xyz", "file.rs"),
        "Bad repo should return false"
    );
}
