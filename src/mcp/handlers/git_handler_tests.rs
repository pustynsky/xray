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
        warning.contains("File never tracked in git"),
        "Warning should mention 'File never tracked in git' (post-2026-04-17 wording), got: {}",
        warning
    );
}

#[test]
fn test_annotate_empty_git_result_writes_warning_at_top_level() {
    // Regression: pin JSON contract -- `warning` and `info` are TOP-LEVEL fields,
    // NOT nested under `summary`. Documented in docs/mcp-guide.md, docs/e2e/git-tests.md,
    // and src/tips.rs. Moving them under `summary` would silently break LLM consumers
    // that rely on documented field placement.
    let ctx = make_git_test_ctx();
    let args = json!({
        "repo": ".",
        "file": "nonexistent_file_xyz_abc_123.rs"
    });
    let result = handle_git_history(&ctx, &args, false);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(
        output.get("warning").is_some(),
        "`warning` MUST be at top level of the response, got: {}",
        serde_json::to_string_pretty(&output).unwrap()
    );
    if let Some(summary) = output.get("summary") {
        assert!(
            summary.get("warning").is_none(),
            "`warning` MUST NOT be nested under `summary` (top-level placement is the documented contract)"
        );
        assert!(
            summary.get("info").is_none(),
            "`info` MUST NOT be nested under `summary` (top-level placement is the documented contract)"
        );
    }
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
    assert_eq!(output["summary"]["tool"].as_str(), Some("xray_branch_status"));
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
        warning.contains("File never tracked in git"),
        "Warning should mention 'File never tracked in git' (post-2026-04-17 wording), got: {}",
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
        warning.contains("File never tracked in git"),
        "Warning should mention 'File never tracked in git' (post-2026-04-17 wording), got: {}",
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
