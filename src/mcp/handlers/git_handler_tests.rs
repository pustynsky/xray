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

#[test]
fn test_date_invalid_calendar_ranges_git003() {
    // GIT-003: month/day must be calendar-valid, not just numeric.
    assert!(date_str_to_timestamp_start("2026-99-99").is_err());
    assert!(date_str_to_timestamp_start("2026-13-01").is_err());
    assert!(date_str_to_timestamp_start("2026-00-01").is_err());
    assert!(date_str_to_timestamp_start("2026-01-32").is_err());
    assert!(date_str_to_timestamp_start("2026-01-00").is_err());
    // Feb 29 invalid in non-leap year, valid in leap year.
    assert!(date_str_to_timestamp_start("2025-02-29").is_err());
    assert!(date_str_to_timestamp_start("2024-02-29").is_ok());
    // April has 30 days.
    assert!(date_str_to_timestamp_start("2026-04-31").is_err());
    assert!(date_str_to_timestamp_start("2026-04-30").is_ok());
}

#[test]
fn test_parse_bounded_usize_git008() {
    // GIT-008: defaults, valid values, and out-of-range rejected.
    let args = serde_json::json!({});
    assert_eq!(parse_bounded_usize(&args, "k", 50, 1000).unwrap(), 50);

    let args = serde_json::json!({ "k": 100 });
    assert_eq!(parse_bounded_usize(&args, "k", 50, 1000).unwrap(), 100);

    let args = serde_json::json!({ "k": 5000 });
    let err = parse_bounded_usize(&args, "k", 50, 1000).unwrap_err();
    assert!(err.contains("k must be 0..=1000"), "{}", err);

    // u64::MAX must not silently wrap.
    let args = serde_json::json!({ "k": u64::MAX });
    assert!(parse_bounded_usize(&args, "k", 50, 1000).is_err());
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
    let w = build_warning("main", true, &Some("main".to_string()), Some(0), false);
    assert!(w.is_none(), "No warning when on main and up-to-date");
}

#[test]
fn test_build_warning_on_main_behind() {
    let w = build_warning("main", true, &Some("main".to_string()), Some(5), false);
    assert!(w.is_some());
    assert!(w.as_ref().unwrap().contains("5 commits behind"));
}

#[test]
fn test_build_warning_on_feature_branch() {
    let w = build_warning("dev/my-feature", false, &Some("master".to_string()), Some(47), false);
    assert!(w.is_some());
    let warning = w.unwrap();
    assert!(warning.contains("dev/my-feature"), "Warning should mention branch name");
    assert!(warning.contains("master"), "Warning should mention main branch");
    assert!(warning.contains("47 commits behind"), "Warning should mention behind count");
}

#[test]
fn test_build_warning_on_feature_branch_no_behind() {
    let w = build_warning("dev/my-feature", false, &Some("main".to_string()), Some(0), false);
    assert!(w.is_some());
    let warning = w.unwrap();
    assert!(warning.contains("dev/my-feature"));
    assert!(!warning.contains("commits behind"), "Should not mention behind when 0");
}

#[test]
fn test_build_warning_on_feature_branch_no_remote() {
    let w = build_warning("dev/my-feature", false, &Some("main".to_string()), None, false);
    assert!(w.is_some());
    let warning = w.unwrap();
    assert!(warning.contains("dev/my-feature"));
}

#[test]
fn test_build_warning_unrelated_histories() {
    // When histories are unrelated the count is suppressed; the warning must
    // explain why instead of implying "0 commits behind".
    let w = build_warning("master", true, &Some("main".to_string()), None, true);
    assert!(w.is_some());
    let warning = w.unwrap();
    assert!(warning.contains("unrelated histories"), "got: {warning}");
    assert!(warning.contains("origin/main"), "should name the trunk remote");
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

// === PERF-02 main-branch detection cache tests ===

/// First `detect_main_branch_name` call against `.` (the xray repo itself,
/// which has a `main` branch) populates `branch_name_cache`. Pinned because
/// PERF-02's whole point is single-spawn cold + zero-spawn warm — a future
/// refactor that drops the cache would silently restore the 4× spawn cost
/// the story exists to remove.
#[test]
fn test_detect_main_branch_name_populates_cache_on_first_call() {
    let ctx = make_git_test_ctx();
    assert!(
        ctx.branch_name_cache.read().unwrap().is_empty(),
        "cache must start empty so the assertion below is meaningful"
    );

    let resolved = detect_main_branch_name(&ctx, ".");
    assert_eq!(resolved.as_deref(), Some("main"), "xray repo has a main branch");

    let cache = ctx.branch_name_cache.read().unwrap();
    assert_eq!(cache.len(), 1, "exactly one entry expected after one call");
    assert_eq!(
        cache.get("."),
        Some(&Some("main".to_string())),
        "cache key is the raw repo string the caller passed"
    );
}

/// Second call with the same repo string MUST return the cached value
/// (verified by mutating the cache to an obviously-fake entry first and
/// checking the function returns the fake instead of re-probing). This
/// pins the contract that the cache is consulted before any git spawn.
#[test]
fn test_detect_main_branch_name_returns_cached_value_without_reprobe() {
    let ctx = make_git_test_ctx();
    ctx.branch_name_cache
        .write()
        .unwrap()
        .insert(".".to_string(), Some("trunk".to_string()));

    let resolved = detect_main_branch_name(&ctx, ".");
    assert_eq!(
        resolved.as_deref(),
        Some("trunk"),
        "cache hit must short-circuit before probing — if this asserts \
         'main' instead, the cache lookup was bypassed and PERF-02 is \
         silently undone"
    );
}
/// PERF-02 follow-up: negative results (`None`) are intentionally NOT
/// cached. Caching `Some(None)` was a permanent-poisoning bug — a path
/// probed before its repo existed (e.g. detect runs against an empty
/// workspace, then user runs `git init` + creates `main`) would forever
/// return None until server restart. We trade ~1 extra `git for-each-ref`
/// per repeated bad-path call (≈5-20 ms) for self-healing recovery.
#[test]
fn test_detect_main_branch_name_does_not_cache_negative_result() {
    let ctx = make_git_test_ctx();
    let bad = "/nonexistent/repo/path/perf-02-test";
    let resolved = detect_main_branch_name(&ctx, bad);
    assert_eq!(resolved, None, "bad repo path resolves to None");

    let cache = ctx.branch_name_cache.read().unwrap();
    assert!(
        !cache.contains_key(bad),
        "negative result must NOT be cached so a later `git init` self-heals; \
         cache keys present: {:?}",
        cache.keys().collect::<Vec<_>>()
    );
}

/// PERF-02 follow-up: a repo that initially probed as None recovers
/// once it actually has a branch — pinning the no-negative-cache contract
/// end-to-end. Builds a tempdir, probes (no `.git` yet → None, no cache
/// entry), runs `git init -b main` + first commit, probes again → must
/// return `Some("main")`. Pre-fix this would have stayed None forever.
#[test]
fn test_detect_main_branch_name_self_heals_after_git_init() {
    use std::process::Command;
    let dir = tempfile::TempDir::new().expect("tempdir");
    let repo = dir.path().to_str().expect("repo utf-8");
    let ctx = make_git_test_ctx();

    // Cold probe before `git init` — must resolve to None and NOT cache.
    let pre = detect_main_branch_name(&ctx, repo);
    assert_eq!(pre, None, "empty dir has no branches");
    assert!(
        !ctx.branch_name_cache.read().unwrap().contains_key(repo),
        "negative pre-init probe must not poison the cache"
    );

    // Now initialise the repo with a `main` branch and a commit.
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("git");
        assert!(out.status.success(), "git {:?} failed: {:?}", args, out);
    };
    run(&["init", "--quiet", "-b", "main"]);
    std::fs::write(dir.path().join("f.txt"), "hi\n").expect("write");
    run(&["add", "f.txt"]);
    run(&["commit", "-m", "init", "--quiet"]);

    // Re-probe must now succeed. Pre-fix: cached `Some(None)` would
    // shortcut and return None forever.
    let post = detect_main_branch_name(&ctx, repo);
    assert_eq!(
        post.as_deref(),
        Some("main"),
        "after `git init -b main` + commit the probe must self-heal"
    );
}

/// Different repo strings get separate cache entries. This is the
/// natural workspace-switch invalidation path documented on the
/// `branch_name_cache` field.
#[test]
fn test_detect_main_branch_name_keys_by_repo_path() {
    use std::process::Command;
    // Two distinct positive repos must occupy two distinct cache slots.
    // (Bad paths are intentionally not cached — see
    // `test_detect_main_branch_name_does_not_cache_negative_result`.)
    let dir_a = tempfile::TempDir::new().expect("tempdir-a");
    let dir_b = tempfile::TempDir::new().expect("tempdir-b");
    let init_repo = |dir: &std::path::Path| {
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {:?} failed: {:?}", args, out);
        };
        run(&["init", "--quiet", "-b", "main"]);
        std::fs::write(dir.join("f.txt"), "hi\n").expect("write");
        run(&["add", "f.txt"]);
        run(&["commit", "-m", "init", "--quiet"]);
    };
    init_repo(dir_a.path());
    init_repo(dir_b.path());

    let ctx = make_git_test_ctx();
    let path_a = dir_a.path().to_str().expect("utf-8");
    let path_b = dir_b.path().to_str().expect("utf-8");
    let _ = detect_main_branch_name(&ctx, path_a);
    let _ = detect_main_branch_name(&ctx, path_b);

    let cache = ctx.branch_name_cache.read().unwrap();
    assert_eq!(
        cache.len(),
        2,
        "each distinct positive repo string gets its own cache entry"
    );
    assert!(cache.contains_key(path_a));
    assert!(cache.contains_key(path_b));
}

/// PERF-02 regression: when a repo has BOTH `refs/heads/master` AND
/// `refs/remotes/origin/main` (and no local `main`), `detect_main_branch_name`
/// MUST resolve to `"main"` to match legacy 4-probe behaviour. Pre-fix code
/// trusted `git for-each-ref` to emit refs in argument order, but git sorts
/// the output by refname → `master` arrived before `origin/main` and the
/// function returned `"master"`, silently flipping `behindMain`/`aheadOfMain`
/// onto the wrong upstream for users with this layout. Builds a real temp
/// repo because mocking git is not in scope here.
#[test]
fn test_detect_main_branch_name_prefers_main_over_local_master() {
    use std::process::Command;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("git must be on PATH for this test");
        assert!(status.success(), "git {:?} failed in {:?}", args, repo);
    };
    // master is the only local branch; origin/main is a remote-tracking ref.
    run(&["init", "--quiet", "-b", "master"]);
    run(&["config", "user.email", "perf02@test.local"]);
    run(&["config", "user.name", "PERF-02 Test"]);
    std::fs::write(repo.join("seed.txt"), "x").unwrap();
    run(&["add", "seed.txt"]);
    run(&["commit", "--quiet", "-m", "init"]);
    run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

    let ctx = make_git_test_ctx();
    let resolved = detect_main_branch_name(&ctx, repo.to_str().unwrap());
    assert_eq!(
        resolved.as_deref(),
        Some("main"),
        "main (even remote-only) MUST win over local master — \
         legacy 4-probe semantics, lost when a refname-sorted \
         for-each-ref output was treated as if it were arg-ordered"
    );
}

/// Run `git` in `repo` with a fixed test identity; assert success; return
/// trimmed stdout. Shared by the trunk/orphan/shallow branch-status fixtures.
fn git_in(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git must be on PATH for this test");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Build a temp repo on `master` whose `origin/main` is an unrelated orphan
/// root commit (no common ancestor with HEAD). When `master_remote_related`
/// is true, `origin/master` points at HEAD (shares history); when false it
/// points at a *second* unrelated orphan root, so NO well-known trunk remote
/// shares history with HEAD. Returns the TempDir — keep it alive for the test.
fn make_dual_trunk_repo(master_remote_related: bool) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git_in(repo, &["init", "--quiet", "-b", "master"]);
    std::fs::write(repo.join("seed.txt"), "x").unwrap();
    git_in(repo, &["add", "seed.txt"]);
    git_in(repo, &["commit", "--quiet", "-m", "init"]);

    // `git commit-tree <tree>` with no `-p` makes a parentless root commit — a
    // history wholly unrelated to HEAD (also a root commit), exactly the
    // orphan-trunk shape that produced the bogus "65028 behind" reports.
    let tree = git_in(repo, &["rev-parse", "HEAD^{tree}"]);
    let orphan_main = git_in(repo, &["commit-tree", &tree, "-m", "orphan-main"]);
    git_in(repo, &["update-ref", "refs/remotes/origin/main", &orphan_main]);

    if master_remote_related {
        git_in(repo, &["update-ref", "refs/remotes/origin/master", "HEAD"]);
    } else {
        let orphan_master = git_in(repo, &["commit-tree", &tree, "-m", "orphan-master"]);
        git_in(repo, &["update-ref", "refs/remotes/origin/master", &orphan_master]);
    }

    tmp
}

/// Bug A regression: a repo on `master` whose orphan ref is `origin/main` must
/// NOT report a bogus distance against the unrelated `main`. The handler must
/// fall back to the related `origin/master`, report it as `mainBranch`, and
/// produce a real (here: 0) `behindMain` instead of tens of thousands.
#[test]
fn test_branch_status_falls_back_to_related_trunk_when_main_unrelated() {
    let tmp = make_dual_trunk_repo(true);
    let ctx = make_git_test_ctx();
    let args = json!({ "repo": tmp.path().to_str().unwrap() });
    let result = handle_branch_status(&ctx, &args);
    assert!(!result.is_error, "Should succeed");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(
        output["mainBranch"].as_str(),
        Some("master"),
        "must compare against the related trunk, not the orphan main"
    );
    assert_eq!(
        output["behindMain"].as_u64(),
        Some(0),
        "HEAD == origin/master, so behind must be 0 — not a bogus orphan count"
    );
    assert_eq!(output["unrelatedHistories"].as_bool(), Some(false));
}

/// Bug B regression: when NO well-known trunk remote shares history with HEAD,
/// the symmetric-difference count is meaningless. The handler must suppress
/// `behindMain`/`aheadOfMain` (null) and raise `unrelatedHistories=true` plus
/// an explanatory warning, instead of emitting the bogus number.
#[test]
fn test_branch_status_suppresses_count_for_unrelated_histories() {
    let tmp = make_dual_trunk_repo(false);
    let ctx = make_git_test_ctx();
    let args = json!({ "repo": tmp.path().to_str().unwrap() });
    let result = handle_branch_status(&ctx, &args);
    assert!(!result.is_error, "Should succeed");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(
        output["behindMain"].is_null(),
        "behindMain must be suppressed for unrelated histories, got {:?}",
        output["behindMain"]
    );
    assert!(output["aheadOfMain"].is_null(), "aheadOfMain must be suppressed too");
    assert_eq!(output["unrelatedHistories"].as_bool(), Some(true));
    let warning = output["warning"].as_str().unwrap_or("");
    assert!(
        warning.contains("unrelated histories"),
        "warning should explain the suppression, got: {warning}"
    );
}

/// Reviewer guard (Bug B refinement): on a SHALLOW clone a missing merge-base
/// can be an artifact of the truncated graft, NOT genuinely unrelated history.
/// `origin/main` here really is an orphan, but because the repo is flagged
/// shallow the handler must NOT claim `unrelatedHistories` — it suppresses the
/// count without an orphan-trunk claim it cannot substantiate (the separate
/// shallow warning covers the truncation).
#[test]
fn test_branch_status_shallow_missing_merge_base_not_flagged_unrelated() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git_in(repo, &["init", "--quiet", "-b", "master"]);
    std::fs::write(repo.join("seed.txt"), "x").unwrap();
    git_in(repo, &["add", "seed.txt"]);
    git_in(repo, &["commit", "--quiet", "-m", "init"]);
    let tree = git_in(repo, &["rev-parse", "HEAD^{tree}"]);
    let orphan = git_in(repo, &["commit-tree", &tree, "-m", "orphan-main"]);
    git_in(repo, &["update-ref", "refs/remotes/origin/main", &orphan]);
    // Mark the repo shallow using HEAD's real hash as the graft boundary, so
    // every git command still resolves while detect_shallow() reports shallow.
    let head = git_in(repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join(".git").join("shallow"), format!("{head}\n")).unwrap();

    let ctx = make_git_test_ctx();
    let args = json!({ "repo": repo.to_str().unwrap() });
    let result = handle_branch_status(&ctx, &args);
    assert!(!result.is_error, "Should succeed");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["isShallow"].as_bool(), Some(true), "repo must be detected shallow");
    assert_eq!(
        output["unrelatedHistories"].as_bool(),
        Some(false),
        "a shallow graft can hide the real ancestor — must NOT claim unrelated"
    );
    assert!(
        output["behindMain"].is_null(),
        "count still suppressed on a shallow no-merge-base"
    );
}

/// Reviewer guard (Bug B refinement): an UNBORN HEAD (no commits) produces no
/// merge-base against an existing `origin/main`, but that is "nothing to
/// compare", NOT unrelated histories. Exercised directly because the handler
/// errors earlier on an unborn HEAD (`rev-parse --abbrev-ref HEAD` exits 128),
/// so only the helper itself can be observed in this state.
#[test]
fn test_resolve_behind_ahead_unborn_head_not_unrelated() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git_in(repo, &["init", "--quiet", "-b", "master"]);
    // origin/main is a real commit (built from the empty tree); HEAD is unborn.
    let empty_tree = git_in(repo, &["write-tree"]);
    let commit = git_in(repo, &["commit-tree", &empty_tree, "-m", "remote-main"]);
    git_in(repo, &["update-ref", "refs/remotes/origin/main", &commit]);

    let (behind, ahead, effective, unrelated) =
        resolve_behind_ahead(repo.to_str().unwrap(), "main", false);
    assert!(behind.is_none() && ahead.is_none(), "no count for an unborn HEAD");
    assert_eq!(effective.as_deref(), Some("main"));
    assert!(!unrelated, "unborn HEAD is not 'unrelated histories'");
}


// ─── Shallow-clone annotation tests ─────────────────────────────

/// Helper: create a `git init`-ed tempdir with a `shallow` file present.
/// Uses a real repo so `git rev-parse --git-path shallow` resolves correctly.
fn make_shallow_tempdir(boundaries: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(dir.path())
        .status()
        .expect("git init must run");
    assert!(status.success(), "git init failed");
    let body = boundaries
        .iter()
        .map(|s| format!("{}\n", s))
        .collect::<String>();
    std::fs::write(dir.path().join(".git").join("shallow"), body).unwrap();
    dir
}

#[test]
fn test_annotate_shallow_clone_noop_when_repo_not_shallow() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let mut result = ToolCallResult::success(r#"{"foo":1}"#.to_string());
    let args = json!({ "repo": dir.path().to_str().unwrap() });
    annotate_shallow_clone(&mut result, &args);
    let body: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(body.get("shallowClone").is_none(), "non-shallow repo => no annotation");
}

#[test]
fn test_annotate_shallow_clone_noop_for_error_results() {
    let dir = make_shallow_tempdir(&["deadbeef"]);
    let mut result = ToolCallResult::error("boom".to_string());
    let args = json!({ "repo": dir.path().to_str().unwrap() });
    annotate_shallow_clone(&mut result, &args);
    // Error result content stays unchanged.
    assert_eq!(result.content[0].text, "boom");
    assert!(result.is_error);
}

#[test]
fn test_annotate_shallow_clone_noop_when_no_repo_arg() {
    let mut result = ToolCallResult::success(r#"{"foo":1}"#.to_string());
    let args = json!({});
    annotate_shallow_clone(&mut result, &args);
    assert_eq!(result.content[0].text, r#"{"foo":1}"#);
}

#[test]
fn test_annotate_shallow_clone_injects_warning_for_shallow_repo() {
    let dir = make_shallow_tempdir(&["abc1230000000000000000000000000000000001"]);
    let mut result = ToolCallResult::success(
        json!({"commits": [], "summary": {}}).to_string(),
    );
    let args = json!({ "repo": dir.path().to_str().unwrap() });
    annotate_shallow_clone(&mut result, &args);

    let body: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let shallow = body.get("shallowClone").expect("shallowClone field added");
    assert_eq!(shallow["isShallow"], json!(true));
    assert_eq!(shallow["boundaries"].as_array().unwrap().len(), 1);
    assert!(
        shallow["warning"].as_str().unwrap().contains("git fetch --unshallow"),
        "warning text mentions the fix command"
    );
    assert!(shallow.get("firstCommitAtBoundary").is_none());
}

#[test]
fn test_annotate_shallow_clone_escalates_when_first_commit_at_boundary() {
    let boundary = "abc1230000000000000000000000000000000001";
    let dir = make_shallow_tempdir(&[boundary]);
    let mut result = ToolCallResult::success(
        json!({
            "firstCommit": { "hash": boundary, "date": "2026-04-05", "author": "X" },
            "summary": {}
        })
        .to_string(),
    );
    let args = json!({ "repo": dir.path().to_str().unwrap() });
    annotate_shallow_clone(&mut result, &args);

    let body: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let shallow = body.get("shallowClone").expect("shallowClone field");
    assert_eq!(shallow["firstCommitAtBoundary"], json!(true));
    assert!(
        shallow["warning"].as_str().unwrap().contains("IS the graft boundary"),
        "escalated warning explicitly calls out the issue"
    );
}

#[test]
fn test_annotate_shallow_clone_does_not_escalate_when_first_commit_below_boundary() {
    let dir = make_shallow_tempdir(&["abc1230000000000000000000000000000000001"]);
    let mut result = ToolCallResult::success(
        json!({
            "firstCommit": { "hash": "deadbeef00000000000000000000000000000000", "date": "2024", "author": "X" },
            "summary": {}
        })
        .to_string(),
    );
    let args = json!({ "repo": dir.path().to_str().unwrap() });
    annotate_shallow_clone(&mut result, &args);

    let body: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let shallow = body.get("shallowClone").expect("shallowClone field");
    assert!(shallow.get("firstCommitAtBoundary").is_none());
}


// ─── cache_is_fresh_for_shallow tests ───────────────────────
//
// These cover the per-request gate that forces handlers to bypass the
// in-memory git cache after shallow-state drift (most importantly
// `git fetch --unshallow` while the server is running).

fn cache_with_fp(fp: Option<&str>) -> crate::git::cache::GitHistoryCache {
    let mut cache = crate::git::cache::GitHistoryCache {
        format_version: crate::git::cache::FORMAT_VERSION,
        head_hash: "abc".to_string(),
        branch: "main".to_string(),
        built_at: 0,
        commits: vec![],
        authors: vec![],
        subjects: String::new(),
        file_commits: std::collections::HashMap::new(),
        shallow_fingerprint: None,
    };
    cache.shallow_fingerprint = fp.map(str::to_string);
    cache
}


#[test]
fn test_repo_matches_workspace_uses_canonical_identity() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let repo = crate::canonicalize_test_root(temp.path());
    let ctx = HandlerContext::default();
    ctx.workspace
        .write()
        .unwrap()
        .set_dir(repo.to_string_lossy().into_owned());

    assert!(repo_matches_workspace(&ctx, repo.to_str().unwrap()));
}

#[test]
fn test_repo_matches_workspace_rejects_other_or_missing_repo() {
    let workspace_temp = tempfile::TempDir::new().expect("tempdir");
    let workspace = crate::canonicalize_test_root(workspace_temp.path());
    let other_temp = tempfile::TempDir::new().expect("tempdir");
    let other = crate::canonicalize_test_root(other_temp.path());
    let ctx = HandlerContext::default();
    ctx.workspace
        .write()
        .unwrap()
        .set_dir(workspace.to_string_lossy().into_owned());

    assert!(!repo_matches_workspace(&ctx, other.to_str().unwrap()));
    assert!(!repo_matches_workspace(
        &ctx,
        workspace.join("missing").to_str().unwrap()
    ));
}

#[test]
fn test_cache_is_fresh_when_both_none_for_full_repo() {
    crate::git::shallow_cache_clear();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let cache = cache_with_fp(None);
    assert!(
        cache_is_fresh_for_shallow(&cache, dir.path().to_str().unwrap()),
        "non-git dir + cache built as non-shallow => fresh"
    );
}

#[test]
fn test_cache_is_stale_after_unshallow_drift() {
    crate::git::shallow_cache_clear();
    let dir = tempfile::TempDir::new().expect("tempdir");
    // Cache was built when the repo was shallow with fingerprint "abc".
    let cache = cache_with_fp(Some("abc"));
    // Repo is currently NOT shallow (no .git, no shallow file).
    assert!(
        !cache_is_fresh_for_shallow(&cache, dir.path().to_str().unwrap()),
        "cache stamped shallow vs. live non-shallow repo => STALE => fall through to CLI"
    );
}

#[test]
fn test_cache_is_stale_when_repo_became_shallow() {
    crate::git::shallow_cache_clear();
    // Build a real shallow-looking repo: `git init` + drop a shallow file.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(dir.path())
        .status()
        .expect("git init");
    assert!(status.success());
    std::fs::write(
        dir.path().join(".git").join("shallow"),
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
    )
    .unwrap();

    // Cache predates the shallow fetch — has fingerprint None.
    let cache = cache_with_fp(None);
    assert!(
        !cache_is_fresh_for_shallow(&cache, dir.path().to_str().unwrap()),
        "cache stamped non-shallow vs. live shallow repo => STALE"
    );
}

#[test]
fn test_cache_is_fresh_when_fingerprints_match() {
    crate::git::shallow_cache_clear();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(dir.path())
        .status()
        .expect("git init");
    assert!(status.success());
    std::fs::write(
        dir.path().join(".git").join("shallow"),
        "aaa\n",
    )
    .unwrap();

    let cache = cache_with_fp(Some("aaa"));
    assert!(
        cache_is_fresh_for_shallow(&cache, dir.path().to_str().unwrap()),
        "matching shallow_fingerprint => cache is fresh"
    );
}


#[test]
fn test_ancestry_base_accepts_only_single_revision_ancestry_grammar() {
    for (revision, expected_base) in [
        ("HEAD~1^2", "HEAD"),
        ("HEAD^^^", "HEAD"),
        ("refs/heads/main~", "refs/heads/main"),
    ] {
        assert_eq!(ancestry_base(revision), Some(expected_base), "{revision}");
    }

    for revision in [
        "HEAD~1a",
        "HEAD^{commit}",
        "main..HEAD~1",
        "^main",
        "HEAD@{1}",
    ] {
        assert_eq!(ancestry_base(revision), None, "{revision}");
    }

    assert!(is_full_object_id("0123456789abcdef0123456789abcdef01234567"));
    assert!(is_full_object_id(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    ));
    assert!(!is_full_object_id("deadbeef"));
}


fn commit_revision(repo: &std::path::Path, content: &str, message: &str) -> String {
    std::fs::write(repo.join("revision.txt"), content).unwrap();
    git_in(repo, &["add", "revision.txt"]);
    git_in(repo, &["commit", "--quiet", "-m", message]);
    git_in(repo, &["rev-parse", "HEAD"])
}

fn make_revision_status_repo() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let repo = crate::canonicalize_test_root(temp.path());
    git_in(&repo, &["init", "--quiet", "-b", "main"]);
    let first_head = commit_revision(&repo, "first\n", "first");
    (temp, repo, first_head)
}

fn branch_status_output(repo: &std::path::Path, expected_ref: Option<&str>) -> Value {
    let ctx = make_git_test_ctx();
    let mut args = json!({ "repo": repo.to_string_lossy() });
    if let Some(expected_ref) = expected_ref {
        args["expectedRef"] = json!(expected_ref);
    }
    let result = handle_branch_status(&ctx, &args);
    assert!(!result.is_error, "branch status failed: {}", result.content[0].text);
    serde_json::from_str(&result.content[0].text).unwrap()
}

#[test]
fn test_branch_status_expected_ref_mismatch() {
    let (_temp, repo, expected_head) = make_revision_status_repo();
    let actual_head = commit_revision(&repo, "second\n", "second");
    let output = branch_status_output(&repo, Some(&expected_head));
    assert_eq!(output["actualHead"], actual_head);
    assert_eq!(output["expectedRef"], expected_head);
    assert_eq!(output["expectedHead"], expected_head);
    assert_eq!(output["revisionMatches"], false);
    assert_eq!(output["revisionStatus"], "mismatch");
    assert!(output["revisionWarning"].as_str().unwrap().contains("local checkout"));
    assert_eq!(output["worktreeDirty"], false);
    assert!(output["summary"]["subTimings"]["revisionMs"].as_f64().is_some());
}

#[test]
fn test_branch_status_expected_ref_match_keeps_dirty_separate() {
    let (_temp, repo, head) = make_revision_status_repo();
    std::fs::write(repo.join("untracked.txt"), "dirty\n").unwrap();
    let output = branch_status_output(&repo, Some("HEAD"));
    assert_eq!(output["actualHead"], head);
    assert_eq!(output["expectedHead"], head);
    assert_eq!(output["revisionMatches"], true);
    assert_eq!(output["revisionStatus"], "match");
    assert!(output["revisionWarning"].is_null());
    assert_eq!(output["worktreeDirty"], true);
}

#[test]
fn test_branch_status_expected_ref_unresolved() {
    let (_temp, repo, head) = make_revision_status_repo();
    let output = branch_status_output(&repo, Some("refs/heads/missing"));
    assert_eq!(output["actualHead"], head);
    assert!(output["expectedHead"].is_null());
    assert!(output["revisionMatches"].is_null());
    assert_eq!(output["revisionStatus"], "unresolved_ref");
    assert!(output["revisionWarning"].as_str().unwrap().contains("local Git object database"));
}

#[test]
fn test_branch_status_expected_ref_remote_only_resolves() {
    let (_temp, repo, first_head) = make_revision_status_repo();
    git_in(&repo, &["update-ref", "refs/remotes/origin/review", &first_head]);
    let actual_head = commit_revision(&repo, "second\n", "second");
    let output = branch_status_output(&repo, Some("origin/review"));
    assert_eq!(output["actualHead"], actual_head);
    assert_eq!(output["expectedHead"], first_head);
    assert_eq!(output["revisionStatus"], "mismatch");
}

#[test]
fn test_branch_status_expected_ref_detached_head_matches() {
    let (_temp, repo, head) = make_revision_status_repo();
    git_in(&repo, &["checkout", "--quiet", "--detach", &head]);
    let output = branch_status_output(&repo, Some("HEAD"));
    assert_eq!(output["currentBranch"], "HEAD");
    assert_eq!(output["actualHead"], head);
    assert_eq!(output["expectedHead"], head);
    assert_eq!(output["revisionStatus"], "match");
}

#[test]
fn test_branch_status_expected_ref_missing_named_ref_stays_unresolved_when_shallow() {
    let (_temp, repo, head) = make_revision_status_repo();
    std::fs::write(repo.join(".git").join("shallow"), format!("{head}\n")).unwrap();
    let output = branch_status_output(&repo, Some("refs/remotes/origin/missing"));
    assert_eq!(output["isShallow"], true);
    assert_eq!(output["revisionStatus"], "unresolved_ref");
    assert!(output["revisionWarning"].as_str().unwrap().contains("not available"));
    assert!(output["expectedHead"].is_null());
    assert!(output["revisionMatches"].is_null());
}

#[test]
fn test_branch_status_expected_ref_shallow_history_for_missing_ancestor() {
    let (_temp, repo, first_head) = make_revision_status_repo();
    let actual_head = commit_revision(&repo, "second\n", "second");
    std::fs::write(repo.join(".git").join("shallow"), format!("{actual_head}\n")).unwrap();
    let output = branch_status_output(&repo, Some("HEAD~1"));
    assert_eq!(output["isShallow"], true);
    assert_eq!(output["actualHead"], actual_head);
    assert_ne!(first_head, actual_head);
    assert_eq!(output["revisionStatus"], "shallow_history");
    assert!(output["revisionWarning"].as_str().unwrap().contains("shallow history"));
    assert!(output["expectedHead"].is_null());
    assert!(output["revisionMatches"].is_null());
}

#[test]
fn test_branch_status_expected_ref_hex_like_name_stays_unresolved_when_shallow() {
    let (_temp, repo, head) = make_revision_status_repo();
    std::fs::write(repo.join(".git").join("shallow"), format!("{head}\n")).unwrap();
    let output = branch_status_output(&repo, Some("deadbeef"));
    assert_eq!(output["revisionStatus"], "unresolved_ref");
}

#[test]
fn test_branch_status_expected_ref_reflog_failure_stays_unresolved_when_shallow() {
    let (_temp, repo, head) = make_revision_status_repo();
    std::fs::write(repo.join(".git").join("shallow"), format!("{head}\n")).unwrap();
    let output = branch_status_output(&repo, Some("HEAD@{999999}"));
    assert_eq!(output["revisionStatus"], "unresolved_ref");
}


#[test]
fn test_branch_status_expected_ref_shallow_history_for_missing_object_id() {
    let (_temp, repo, head) = make_revision_status_repo();
    std::fs::write(repo.join(".git").join("shallow"), format!("{head}\n")).unwrap();
    let missing_object = "0123456789abcdef0123456789abcdef01234567";
    let output = branch_status_output(&repo, Some(missing_object));
    assert_eq!(output["revisionStatus"], "shallow_history");
    assert!(output["revisionWarning"].as_str().unwrap().contains("may hide"));
}


#[test]
fn test_branch_status_expected_ref_peels_annotated_and_lightweight_tags() {
    let (_temp, repo, head) = make_revision_status_repo();
    git_in(&repo, &["tag", "lightweight-review", &head]);
    git_in(&repo, &["tag", "-a", "annotated-review", "-m", "review", &head]);

    for tag in ["lightweight-review", "annotated-review"] {
        let output = branch_status_output(&repo, Some(tag));
        assert_eq!(output["expectedRef"], tag);
        assert_eq!(output["expectedHead"], head);
        assert_eq!(output["revisionMatches"], true);
        assert_eq!(output["revisionStatus"], "match");
    }
}

#[test]
fn test_branch_status_expected_ref_rejects_option_injection() {
    let (_temp, repo, _head) = make_revision_status_repo();
    let ctx = make_git_test_ctx();
    let result = handle_branch_status(&ctx, &json!({
        "repo": repo.to_string_lossy(),
        "expectedRef": "--upload-pack=evil.sh",
    }));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("must not start with '-'"));
}

#[test]
fn test_branch_status_expected_ref_omitted_is_backward_compatible() {
    let (_temp, repo, head) = make_revision_status_repo();
    let output = branch_status_output(&repo, None);
    assert_eq!(output["actualHead"], head);
    assert!(output["expectedRef"].is_null());
    assert!(output["expectedHead"].is_null());
    assert!(output["revisionMatches"].is_null());
    assert_eq!(output["revisionStatus"], "not_requested");
}

#[test]
fn test_branch_status_expected_ref_is_advertised_in_schema() {
    let tool = git_tool_definitions().into_iter()
        .find(|tool| tool.name == "xray_branch_status")
        .unwrap();
    assert!(tool.input_schema["properties"]["expectedRef"].is_object());
}
