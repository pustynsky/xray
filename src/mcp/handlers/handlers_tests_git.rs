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

    // Build a mock cache from git-log-style input
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
    let reader = Cursor::new(log.as_bytes());
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
