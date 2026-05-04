//! Tests for git handler cache, noCache, date validation -- extracted from handlers_tests.rs.

use super::*;
use super::handlers_test_utils::make_empty_ctx;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
// ═══════════════════════════════════════════════════════════════════════
// Git handler cache tests
// ═══════════════════════════════════════════════════════════════════════

/// Helper: create a HandlerContext with a populated GitHistoryCache.
fn make_ctx_with_git_cache() -> HandlerContext {
    use crate::git::cache::*;
    use std::io::Cursor;

    // Build a mock cache from git-log-style input.
    // CACHE-002: production parser expects NUL-separated records (`git log -z`).
    // Fixture is written with `\n` for readability, then translated to NUL below.
    let log = concat!(
        "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞alice@example.com␞Alice␞Initial commit\n",
        "src/main.rs\n",
        "Cargo.toml\n",
        "\n",
        "COMMIT:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb␞1700001000␞bob@example.com␞Bob␞Add feature\n",
        "src/main.rs\n",
        "src/lib.rs\n",
        "\n",
        "COMMIT:cccccccccccccccccccccccccccccccccccccccc␞1700002000␞alice@example.com␞Alice␞Fix bug\n",
        "src/main.rs\n",
        "\n",
    );
    let nul_input: Vec<u8> = log.bytes().map(|b| if b == b'\n' { 0 } else { b }).collect();
    let reader = Cursor::new(nul_input);
    let mut builder = GitHistoryCache::builder();
    parse_git_log_stream(reader, &mut builder).expect("parse should succeed");
    let cache = GitHistoryCache::from_builder(
        builder,
        "abc123def456abc123def456abc123def456abc1".to_string(),
        "main".to_string(),
    );

    let mut ctx = make_empty_ctx();
    *ctx.git_cache.write().unwrap() = Some(cache);
    ctx.git_cache_ready = Arc::new(AtomicBool::new(true));
    ctx
}

/// xray_git_authors with populated cache returns non-empty firstChange and lastChange.
#[test]
fn test_git_authors_cached_has_first_and_last_change() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_authors", &json!({
        "repo": ".",
        "file": "src/main.rs"
    }));
    assert!(!result.is_error, "xray_git_authors should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Should use cache path (hint contains "from cache")
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("cache"), "Should use cache path, hint: {}", hint);

    let authors = output["authors"].as_array().unwrap();
    assert!(authors.len() >= 2, "Should have at least 2 authors, got {}", authors.len());

    for author in authors {
        let first = author["firstChange"].as_str().unwrap();
        let last = author["lastChange"].as_str().unwrap();
        assert!(!first.is_empty(), "firstChange should not be empty for author {}", author["name"]);
        assert!(!last.is_empty(), "lastChange should not be empty for author {}", author["name"]);
        // Verify date format (YYYY-MM-DD HH:MM:SS +0000)
        assert!(first.len() > 10, "firstChange should be a full date, got: {}", first);
        assert!(last.len() > 10, "lastChange should be a full date, got: {}", last);
    }

    // Alice: first commit at 1700000000, last at 1700002000
    let alice = authors.iter().find(|a| a["name"] == "Alice").unwrap();
    assert_ne!(alice["firstChange"], alice["lastChange"],
        "Alice has commits at different times, firstChange should differ from lastChange");
}

/// xray_git_history with populated cache returns commits from cache.
#[test]
fn test_git_history_cached_returns_commits() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "maxResults": 5
    }));
    assert!(!result.is_error, "xray_git_history should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("cache"), "Should use cache path, hint: {}", hint);

    let commits = output["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 3, "src/main.rs should have 3 commits");

    // Verify commits are sorted newest first
    let ts0 = commits[0]["date"].as_str().unwrap();
    let ts2 = commits[2]["date"].as_str().unwrap();
    assert!(ts0 > ts2, "Commits should be sorted newest first");
}

/// xray_git_activity with populated cache returns activity from cache.
#[test]
fn test_git_activity_cached_returns_files() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": "."
    }));
    assert!(!result.is_error, "xray_git_activity should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("cache"), "Should use cache path, hint: {}", hint);

    let activity = output["activity"].as_array().unwrap();
    assert_eq!(activity.len(), 3, "Should have 3 files in activity");
}

/// xray_git_diff always uses CLI, never cache (cache has no patch data).
#[test]
fn test_git_diff_does_not_use_cache() {
    let ctx = make_ctx_with_git_cache();
    // xray_git_diff with a fake repo will fail (no real git repo at "."),
    // but the key test is that it does NOT use the cache path
    let result = dispatch_tool(&ctx, "xray_git_diff", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "maxResults": 1
    }));
    // It may succeed (if we're in a real git repo) or fail (if not),
    // but if it succeeds, it should NOT have "(from cache)" in hint
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let hint = output["summary"]["hint"].as_str().unwrap_or("");
        assert!(!hint.contains("cache"),
            "xray_git_diff should never use cache, hint: {}", hint);
    }
}
/// noCache: xray_git_history with noCache=true bypasses cache and uses CLI.
#[test]
fn test_git_history_no_cache_bypasses_cache() {
    let ctx = make_ctx_with_git_cache();
    // With noCache=true, the cache should be bypassed even though it's populated.
    // Since we're in a real git repo (the workspace), this should either succeed
    // via CLI or fail with a git error — but NOT use the cache (no "(from cache)" hint).
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "Cargo.toml",
        "maxResults": 2,
        "noCache": true
    }));
    // If it succeeds (real git repo), verify no cache hint
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let hint = output["summary"]["hint"].as_str().unwrap_or("");
        assert!(!hint.contains("cache"),
            "noCache=true should bypass cache, but hint says: {}", hint);
    }
    // If it errors (e.g., Cargo.toml not in mock repo), that's fine —
    // the key test is that it didn't use the cache path
}

/// noCache: xray_git_history without noCache uses cache when available.
#[test]
fn test_git_history_default_uses_cache() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "maxResults": 2
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("cache"),
        "Without noCache, should use cache. Hint: {}", hint);
}

/// noCache: xray_git_authors with noCache=true bypasses cache.
#[test]
fn test_git_authors_no_cache_bypasses_cache() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_authors", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "noCache": true
    }));
    // If succeeded via CLI, verify no cache hint
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let hint = output["summary"]["hint"].as_str().unwrap_or("");
        assert!(!hint.contains("cache"),
            "noCache=true should bypass cache for authors, but hint says: {}", hint);
    }
}

/// noCache: xray_git_activity with noCache=true bypasses cache.
#[test]
fn test_git_activity_no_cache_bypasses_cache() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": ".",
        "noCache": true
    }));
    // If succeeded via CLI, verify no cache hint
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let hint = output["summary"]["hint"].as_str().unwrap_or("");
        assert!(!hint.contains("cache"),
            "noCache=true should bypass cache for activity, but hint says: {}", hint);
    }
}

/// noCache: noCache=false should behave same as omitting — use cache.
#[test]
fn test_git_history_no_cache_false_uses_cache() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "maxResults": 2,
        "noCache": false
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("cache"),
        "noCache=false should use cache. Hint: {}", hint);
}

/// BUG-4: xray_git_history with reversed date range should return error (cache path).
#[test]
fn test_git_history_cached_reversed_dates_returns_error() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "from": "2026-12-31",
        "to": "2026-01-01"
    }));
    assert!(result.is_error, "Reversed date range should return error");
    assert!(result.content[0].text.contains("after"),
        "Error should mention 'after', got: {}", result.content[0].text);
}


// ─── includeDeleted parameter (cache path) ──────────────────────────
// Verifies that includeDeleted=true is wired through the cache code path,
// surfaces the includeDeleted field in summary, and updates the hint text.
// The actual file-set filtering uses git::list_tracked_files_under under
// the hood (single `git ls-files` spawn — see git_tests::test_list_tracked_files_under_*).

#[test]
fn test_git_activity_include_deleted_default_false() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": "."
    }));
    assert!(!result.is_error, "xray_git_activity should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let include_deleted = output["summary"]["includeDeleted"].as_bool();
    assert_eq!(include_deleted, Some(false), "includeDeleted must default to false, got {:?}", include_deleted);
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(!hint.contains("NOT in current HEAD"),
        "default hint should NOT mention 'NOT in current HEAD', got: {}", hint);
}

#[test]
fn test_git_activity_include_deleted_true_sets_field_and_hint() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": ".",
        "includeDeleted": true
    }));
    assert!(!result.is_error, "xray_git_activity should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let include_deleted = output["summary"]["includeDeleted"].as_bool();
    assert_eq!(include_deleted, Some(true), "includeDeleted=true must round-trip into summary, got {:?}", include_deleted);
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("NOT in current HEAD"),
        "hint should mention 'NOT in current HEAD' when includeDeleted=true, got: {}", hint);
}


// ─── includeDeleted parameter (CLI path) ──────────────────────────
// Verifies summary.totalEntries is derived from the *filtered* files_array,
// not the unfiltered file_map. Pre-fix the CLI path used
//   total_entries = file_map.values().map(|v| v.len()).sum()
// while files_array was already filtered down to deleted-only files —
// producing a contract bug where summary.totalEntries reported commits for
// files no longer present in activity[]. The cache path was already correct.

/// Test helper: build a tiny git repo with one survivor file and one deleted
/// file (mirrors `git_tests::setup_repo_with_deleted_file`, kept private here
/// to avoid cross-module test-helper visibility churn).
fn setup_mixed_repo_for_include_deleted() -> tempfile::TempDir {
    fn run_git(repo: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C").arg(repo)
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
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let repo = dir.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    run_git(repo, &["config", "user.email", "test@example.com"]);
    run_git(repo, &["config", "user.name", "Test Author"]);
    // Survivor: 2 commits, still in HEAD.
    std::fs::write(repo.join("survivor.txt"), "survivor v1\n").unwrap();
    run_git(repo, &["add", "survivor.txt"]);
    run_git(repo, &["commit", "-m", "add survivor", "--quiet"]);
    std::fs::write(repo.join("survivor.txt"), "survivor v2\n").unwrap();
    run_git(repo, &["commit", "-am", "modify survivor", "--quiet"]);
    // Legacy: 3 commits (add, modify, delete), removed from HEAD.
    std::fs::write(repo.join("legacy.txt"), "legacy v1\n").unwrap();
    run_git(repo, &["add", "legacy.txt"]);
    run_git(repo, &["commit", "-m", "add legacy", "--quiet"]);
    std::fs::write(repo.join("legacy.txt"), "legacy v2\n").unwrap();
    run_git(repo, &["commit", "-am", "modify legacy", "--quiet"]);
    std::fs::remove_file(repo.join("legacy.txt")).unwrap();
    run_git(repo, &["commit", "-am", "delete legacy", "--quiet"]);
    dir
}

/// Regression: CLI path with includeDeleted=true must derive summary.totalEntries
/// from the filtered files_array, not the unfiltered file_map. Pre-fix,
/// summary.filesChanged correctly reflected only deleted files (1) but
/// summary.totalEntries still summed commits over both deleted AND surviving
/// files (2 + 3 = 5 instead of 3).
#[test]
fn test_git_activity_cli_include_deleted_total_entries_matches_filtered_activity() {
    let dir = setup_mixed_repo_for_include_deleted();
    let repo_path = dir.path().to_str().unwrap().to_string();

    // Empty ctx -> no git cache -> CLI path is taken.
    let ctx = make_empty_ctx();

    // Baseline (includeDeleted=false): both files present.
    let baseline = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": repo_path,
        "noCache": true
    }));
    assert!(!baseline.is_error, "baseline must succeed: {}", baseline.content[0].text);
    let baseline_out: Value = serde_json::from_str(&baseline.content[0].text).unwrap();
    let baseline_files = baseline_out["activity"].as_array().unwrap();
    assert_eq!(baseline_files.len(), 2,
        "baseline must list both survivor.txt and legacy.txt, got: {}",
        serde_json::to_string_pretty(&baseline_out).unwrap());
    let baseline_total: u64 = baseline_files.iter()
        .map(|f| f["commitCount"].as_u64().unwrap_or(0)).sum();
    let baseline_summary_total = baseline_out["summary"]["totalEntries"].as_u64().unwrap();
    assert_eq!(baseline_total, baseline_summary_total,
        "baseline: summary.totalEntries must equal sum of activity[].commitCount");

    // Filtered (includeDeleted=true): only legacy.txt survives the filter.
    let filtered = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": repo_path,
        "noCache": true,
        "includeDeleted": true
    }));
    assert!(!filtered.is_error, "filtered must succeed: {}", filtered.content[0].text);
    let filtered_out: Value = serde_json::from_str(&filtered.content[0].text).unwrap();
    let filtered_files = filtered_out["activity"].as_array().unwrap();
    assert_eq!(filtered_files.len(), 1,
        "includeDeleted=true must keep only legacy.txt, got: {}",
        serde_json::to_string_pretty(&filtered_out).unwrap());
    assert_eq!(filtered_files[0]["path"].as_str().unwrap(), "legacy.txt");

    let filtered_legacy_commits = filtered_files[0]["commitCount"].as_u64().unwrap();
    let filtered_summary_total = filtered_out["summary"]["totalEntries"].as_u64().unwrap();
    let filtered_summary_files = filtered_out["summary"]["filesChanged"].as_u64().unwrap();

    assert_eq!(filtered_summary_files, 1,
        "summary.filesChanged must equal activity.len() after filtering");
    assert_eq!(filtered_summary_total, filtered_legacy_commits,
        "BUG REGRESSION: summary.totalEntries ({}) must equal sum of returned commitCounts ({}) — pre-fix this also counted survivor.txt commits",
        filtered_summary_total, filtered_legacy_commits);
    assert!(filtered_summary_total < baseline_summary_total,
        "includeDeleted filter must shrink totalEntries (baseline={}, filtered={})",
        baseline_summary_total, filtered_summary_total);
}


#[test]
fn test_git_activity_include_deleted_filters_existing_files_in_real_repo() {
    // Mock cache references src/main.rs, Cargo.toml, src/lib.rs — all of which
    // EXIST in the current xray repo. With includeDeleted=true the activity
    // list MUST be filtered down (these files are not deleted).
    let ctx = make_ctx_with_git_cache();
    let baseline = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": "."
    }));
    assert!(!baseline.is_error);
    let baseline_out: Value = serde_json::from_str(&baseline.content[0].text).unwrap();
    let baseline_count = baseline_out["activity"].as_array().unwrap().len();
    assert!(baseline_count > 0, "baseline must have files");

    let filtered = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": ".",
        "includeDeleted": true
    }));
    assert!(!filtered.is_error);
    let filtered_out: Value = serde_json::from_str(&filtered.content[0].text).unwrap();
    let filtered_count = filtered_out["activity"].as_array().unwrap().len();

    // src/main.rs and Cargo.toml exist in HEAD -> filtered out by includeDeleted=true.
    // src/lib.rs also exists. So filtered_count should be strictly less than baseline_count.
    assert!(filtered_count < baseline_count,
        "includeDeleted=true must filter out files that exist in current HEAD (baseline={}, filtered={})",
        baseline_count, filtered_count);
}

// ── firstCommit handler tests ─────────────────────────────────────────

/// firstCommit=true on xray_git_history: response shape is {firstCommit: {...}, summary: {...}},
/// NOT the usual {commits: [...], summary: {...}}. Hint mentions firstCommit mode.
/// Bypasses the populated cache (no "(from cache)" hint).
#[test]
fn test_git_history_first_commit_returns_creation_envelope() {
    let ctx = make_ctx_with_git_cache();
    // Use Cargo.toml — exists in real workspace git repo where these tests run.
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "Cargo.toml",
        "firstCommit": true
    }));
    if result.is_error {
        // Workspace may be a non-git checkout in some CI configs; skip rather
        // than fail — the unit-level coverage in git_tests.rs is authoritative.
        return;
    }
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Envelope: firstCommit field present (not commits array), no "from cache" hint.
    assert!(output.get("firstCommit").is_some(),
        "firstCommit mode must return a firstCommit field, got: {}", output);
    assert!(output.get("commits").is_none(),
        "firstCommit mode must NOT return the commits array (different envelope), got: {}", output);
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(!hint.contains("(from cache)"),
        "firstCommit must bypass cache, hint says: {}", hint);
    assert_eq!(output["summary"]["mode"].as_str(), Some("firstCommit"),
        "summary.mode must be 'firstCommit', got: {}", output["summary"]);

    let first = &output["firstCommit"];
    assert!(first.is_object(),
        "firstCommit must be a populated object for Cargo.toml (it has a creation commit), got: {}", first);
    assert!(first["hash"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
        "hash must be populated, got: {}", first);
    assert!(first["message"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
        "message must be populated, got: {}", first);
}

/// firstCommit on xray_git_diff is rejected (no patch is returned in firstCommit mode).
#[test]
fn test_git_diff_first_commit_rejected() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_diff", &json!({
        "repo": ".",
        "file": "Cargo.toml",
        "firstCommit": true
    }));
    assert!(result.is_error,
        "firstCommit on xray_git_diff must return an error, got success: {}",
        result.content[0].text);
    let msg = &result.content[0].text;
    assert!(msg.contains("firstCommit") && msg.contains("xray_git_diff"),
        "error message must mention firstCommit and xray_git_diff, got: {}", msg);
}

/// firstCommit on a never-existed file: firstCommit=null and the empty-result
/// annotation kicks in (warning/info distinguishing never-existed vs deleted).
#[test]
fn test_git_history_first_commit_nonexistent_returns_null() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "definitely/does/not/exist/anywhere.txt",
        "firstCommit": true
    }));
    if result.is_error { return; } // see note in envelope test above
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["firstCommit"].is_null(),
        "firstCommit on a nonexistent path must be null, got: {}", output["firstCommit"]);
    assert_eq!(output["summary"]["mode"].as_str(), Some("firstCommit"));
}

