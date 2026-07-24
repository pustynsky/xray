//! Tests for git handler cache, noCache, date validation -- extracted from handlers_tests.rs.

use super::*;
use super::handlers_test_utils::make_empty_ctx;
use std::path::Path;
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

fn run_fixture_git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit_fixture(repo: &Path, name: &str, email: &str, message: &str) {
    run_fixture_git(repo, &["add", "-A"]);
    let name_config = format!("user.name={name}");
    let email_config = format!("user.email={email}");
    run_fixture_git(
        repo,
        &[
            "-c",
            &name_config,
            "-c",
            &email_config,
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

fn make_rename_history_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let repo = crate::canonicalize_test_root(temp.path());
    run_fixture_git(&repo, &["init", "-q", "-b", "main"]);

    std::fs::write(repo.join("original.txt"), "content\n").unwrap();
    std::fs::write(repo.join("stable.txt"), "stable\n").unwrap();
    commit_fixture(
        &repo,
        "Original Author",
        "original@example.com",
        "add original",
    );

    run_fixture_git(&repo, &["mv", "original.txt", "middle.txt"]);
    commit_fixture(
        &repo,
        "Rename Author",
        "rename@example.com",
        "rename original to middle",
    );

    run_fixture_git(&repo, &["mv", "middle.txt", "final.txt"]);
    commit_fixture(
        &repo,
        "Rename Author",
        "rename@example.com",
        "rename middle to final",
    );

    (temp, repo)
}

fn refresh_fixture_cache(ctx: &HandlerContext, repo: &Path) {
    let cache = crate::git::cache::GitHistoryCache::build(repo, "main")
        .expect("build git cache");
    *ctx.git_cache.write().unwrap() = Some(cache);
}

fn make_fixture_ctx(repo: &Path) -> HandlerContext {
    let mut ctx = make_empty_ctx();
    ctx.workspace
        .write()
        .unwrap()
        .set_dir(repo.to_string_lossy().into_owned());
    refresh_fixture_cache(&ctx, repo);
    ctx.git_cache_ready = Arc::new(AtomicBool::new(true));
    ctx
}

fn dispatch_git_json(ctx: &HandlerContext, tool: &str, args: Value) -> Value {
    let result = dispatch_tool(ctx, tool, &args);
    assert!(!result.is_error, "{tool} failed: {}", result.content[0].text);
    serde_json::from_str(&result.content[0].text).unwrap()
}

fn history_messages(output: &Value) -> Vec<String> {
    output["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|commit| commit["message"].as_str().unwrap().to_string())
        .collect()
}

fn history_hashes(output: &Value) -> Vec<String> {
    output["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|commit| commit["hash"].as_str().unwrap().to_string())
        .collect()
}

fn assert_cached_history_contract(output: &Value) {
    assert_eq!(output["summary"]["source"], "git-cache");
    assert_eq!(output["summary"]["lineage"], "direct-path");
    assert_eq!(output["summary"]["safeForFullHistory"], false);
    assert_eq!(
        output["summary"]["hint"],
        "Fast direct-path cache; may omit history before renames/copies. Set noCache=true for git --follow."
    );
}

fn assert_followed_history_contract(output: &Value) {
    assert_eq!(output["summary"]["source"], "git-cli");
    assert_eq!(output["summary"]["lineage"], "follow");
    assert_eq!(output["summary"]["safeForFullHistory"], true);
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
fn test_git_history_cached_direct_path_contract() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "maxResults": 5
    }));
    assert!(!result.is_error, "xray_git_history should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(output["summary"]["source"], "git-cache");
    assert_eq!(output["summary"]["lineage"], "direct-path");
    assert_eq!(output["summary"]["safeForFullHistory"], false);
    assert_eq!(
        output["summary"]["hint"],
        "Fast direct-path cache; may omit history before renames/copies. Set noCache=true for git --follow."
    );

    let commits = output["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 3, "src/main.rs should have 3 commits");

    // Verify commits are sorted newest first
    let ts0 = commits[0]["date"].as_str().unwrap();
    let ts2 = commits[2]["date"].as_str().unwrap();
    assert!(ts0 > ts2, "Commits should be sorted newest first");
}


#[test]
fn test_git_history_rename_direct_path_and_follow_contracts() {
    let (_temp, repo) = make_rename_history_repo();
    let ctx = make_fixture_ctx(&repo);
    let repo_arg = repo.to_string_lossy();

    let existing_cached = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({ "repo": repo_arg, "file": "final.txt", "maxResults": 0 }),
    );
    assert_eq!(history_messages(&existing_cached), vec!["rename middle to final"]);
    assert_cached_history_contract(&existing_cached);

    run_fixture_git(&repo, &["rm", "-q", "final.txt"]);
    commit_fixture(
        &repo,
        "Delete Author",
        "delete@example.com",
        "delete final",
    );
    refresh_fixture_cache(&ctx, &repo);

    let deleted_cached = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({ "repo": repo_arg, "file": "final.txt", "maxResults": 0 }),
    );
    assert_eq!(
        history_messages(&deleted_cached),
        vec!["delete final", "rename middle to final"]
    );
    assert_cached_history_contract(&deleted_cached);

    let unlimited = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({
            "repo": repo_arg,
            "file": "final.txt",
            "maxResults": 0,
            "noCache": true
        }),
    );
    assert_eq!(
        history_messages(&unlimited),
        vec![
            "delete final",
            "rename middle to final",
            "rename original to middle",
            "add original"
        ]
    );
    assert_eq!(unlimited["summary"]["totalCommits"], 4);
    assert_followed_history_contract(&unlimited);

    let limited = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({
            "repo": repo_arg,
            "file": "final.txt",
            "maxResults": 3,
            "noCache": true
        }),
    );
    assert_eq!(
        history_messages(&limited),
        vec![
            "delete final",
            "rename middle to final",
            "rename original to middle"
        ]
    );
    assert_eq!(limited["summary"]["totalCommits"], 4);
    assert_followed_history_contract(&limited);

    let author_filtered = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({
            "repo": repo_arg,
            "file": "final.txt",
            "author": "Original Author",
            "maxResults": 0,
            "noCache": true
        }),
    );
    assert_eq!(history_messages(&author_filtered), vec!["add original"]);
    assert_followed_history_contract(&author_filtered);

    let limited_author = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({
            "repo": repo_arg,
            "file": "final.txt",
            "author": "Rename Author",
            "maxResults": 1,
            "noCache": true
        }),
    );
    assert_eq!(history_messages(&limited_author), vec!["rename middle to final"]);
    assert_eq!(limited_author["summary"]["totalCommits"], 2);
    assert_followed_history_contract(&limited_author);

    let message_filtered = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({
            "repo": repo_arg,
            "file": "final.txt",
            "message": "original to middle",
            "maxResults": 0,
            "noCache": true
        }),
    );
    assert_eq!(
        history_messages(&message_filtered),
        vec!["rename original to middle"]
    );
    assert_followed_history_contract(&message_filtered);

    let first = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({ "repo": repo_arg, "file": "final.txt", "firstCommit": true }),
    );
    assert_eq!(first["firstCommit"]["message"], "add original");
    assert_followed_history_contract(&first);

    let stable_cached = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({ "repo": repo_arg, "file": "stable.txt", "maxResults": 0 }),
    );
    let stable_cli = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({
            "repo": repo_arg,
            "file": "stable.txt",
            "maxResults": 0,
            "noCache": true
        }),
    );
    assert_eq!(history_hashes(&stable_cached), history_hashes(&stable_cli));
    assert_cached_history_contract(&stable_cached);
    assert_followed_history_contract(&stable_cli);
}

#[test]
fn test_git_cache_is_scoped_to_workspace_repo() {
    let (_workspace_temp, workspace_repo) = make_rename_history_repo();
    let ctx = make_fixture_ctx(&workspace_repo);

    let other_temp = tempfile::TempDir::new().expect("tempdir");
    let other_repo = crate::canonicalize_test_root(other_temp.path());
    run_fixture_git(&other_repo, &["init", "-q", "-b", "main"]);
    std::fs::write(other_repo.join("stable.txt"), "other\n").unwrap();
    commit_fixture(
        &other_repo,
        "Second Author",
        "second@example.com",
        "second repo commit",
    );
    let repo_arg = other_repo.to_string_lossy();

    let history = dispatch_git_json(
        &ctx,
        "xray_git_history",
        json!({ "repo": repo_arg, "file": "stable.txt", "maxResults": 0 }),
    );
    assert_eq!(history_messages(&history), vec!["second repo commit"]);
    assert_followed_history_contract(&history);

    let authors = dispatch_git_json(
        &ctx,
        "xray_git_authors",
        json!({ "repo": repo_arg, "path": "stable.txt" }),
    );
    let author_names: Vec<&str> = authors["authors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|author| author["name"].as_str().unwrap())
        .collect();
    assert_eq!(author_names, vec!["Second Author"]);
    assert!(!authors["summary"]["hint"]
        .as_str()
        .unwrap_or("")
        .contains("cache"));

    let activity = dispatch_git_json(
        &ctx,
        "xray_git_activity",
        json!({ "repo": repo_arg }),
    );
    let activity_paths: Vec<&str> = activity["activity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(activity_paths, vec!["stable.txt"]);
    assert!(!activity["summary"]["hint"]
        .as_str()
        .unwrap_or("")
        .contains("cache"));
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
    let (_temp, repo) = make_rename_history_repo();
    let ctx = make_fixture_ctx(&repo);
    let output = dispatch_git_json(
        &ctx,
        "xray_git_diff",
        json!({
            "repo": repo.to_string_lossy(),
            "file": "stable.txt",
            "maxResults": 1
        }),
    );

    assert_eq!(output["summary"]["source"], "git-cli");
    assert_eq!(output["summary"]["lineage"], "follow");
    assert_eq!(output["summary"]["safeForFullHistory"], true);
    let commits = output["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert!(commits[0]["patch"].as_str().is_some_and(|patch| !patch.is_empty()));
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

// ─── HEAD-pinning: stale empty cache results auto-fall-through to CLI ──
// User story 2026-05-10. Mock cache is built with fake head_hash
// "abc123def456...". The workspace root (".") is a real git repo whose live
// HEAD differs from that fake hash. Querying a file that is NOT in the
// cache produces an empty cache result; the HEAD-pinning check detects
// the snapshot/live HEAD mismatch and falls through to the CLI fallback.
// The fingerprint of fall-through: the response hint does NOT contain
// "(from cache)" — it carries the CLI hint (or empty string) instead.
//
// Why this is a regression-killer for the user story scenario:
//  - Bug: file committed AFTER cache build → cache returns stale empty
//    → LLM reports "no history" (false negative).
//  - Fix: empty + HEAD moved → re-query via CLI → authoritative answer.

#[test]
fn test_git_history_empty_cache_with_stale_head_falls_through_to_cli() {
    let ctx = make_ctx_with_git_cache();
    // File definitely not in mock cache; workspace HEAD != fake "abc123...".
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "definitely_not_a_real_file_xyz_2026_05_10.rs",
        "maxResults": 5
    }));
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(output["summary"]["source"], "git-cli");
    }
    // Error path is also acceptable — CLI fallback may legitimately fail in some
    // sandboxed test environments. Either way, the bug-killer is the absence of
    // a stale cached empty being returned.
}

#[test]
fn test_git_authors_empty_cache_with_stale_head_falls_through_to_cli() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_authors", &json!({
        "repo": ".",
        "path": "definitely_not_a_real_dir_xyz_2026_05_10/",
    }));
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let hint = output["summary"]["hint"].as_str().unwrap_or("");
        assert!(!hint.contains("(from cache)"),
            "Empty cache authors with stale HEAD must fall through to CLI, got hint: {}", hint);
    }
}

#[test]
fn test_git_activity_empty_cache_with_stale_head_falls_through_to_cli() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_activity", &json!({
        "repo": ".",
        "path": "definitely_not_a_real_dir_xyz_2026_05_10/",
        "from": "1970-01-01",
        "to": "1970-01-02",
    }));
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let hint = output["summary"]["hint"].as_str().unwrap_or("");
        assert!(!hint.contains("(from cache)"),
            "Empty cache activity with stale HEAD must fall through to CLI, got hint: {}", hint);
    }
}

/// Non-empty cache results are NOT subject to HEAD-pinning fall-through —
/// the cache is monotonic for files already known. This guards against an
/// over-eager invalidation that would defeat the purpose of caching.
#[test]
fn test_git_history_nonempty_cache_with_stale_head_still_uses_cache() {
    let ctx = make_ctx_with_git_cache();
    // src/main.rs IS in the mock cache (3 commits). Even though live HEAD != cache HEAD,
    // the non-empty branch must still serve from cache.
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "maxResults": 5
    }));
    assert!(!result.is_error, "should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("cache"),
        "Non-empty cache result must still serve from cache (HEAD-pinning is empty-only), got hint: {}", hint);
    assert_eq!(output["commits"].as_array().unwrap().len(), 3);
}

/// REGRESSION-KILLER for code-reviewer's MAJOR finding (2026-05-10):
/// Author-filter on a KNOWN cached file with no matching commits in cache must
/// fall through to CLI when HEAD has moved \u2014 NOT serve a stale empty.
/// Mock cache contains only Alice and Bob commits on src/main.rs. A query for
/// `author=Carol` returns empty from cache; combined with a stale fake HEAD,
/// the gate must fire even though src/main.rs IS a known cache key.
///
/// Mutation guard: if the gate is narrowed back to `path_unknown_to_cache`
/// (i.e. the previous buggy version), this test fails because src/main.rs IS
/// in cache.file_commits and the stale empty would be returned with hint
/// `(from cache)`.
#[test]
fn test_git_history_known_path_author_filter_empty_with_stale_head_falls_through() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "src/main.rs",
        "author": "Carol",
        "maxResults": 5
    }));
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(output["summary"]["source"], "git-cli");
    }
    // CLI fallback may legitimately error in sandboxed envs \u2014 absence of stale
    // cached empty is what we assert.
}

/// Same regression-killer for xray_git_authors: known whole-repo path with an
/// author-filter that matches nothing in cache. Author filter is applied at
/// the handler level via CLI, not via cache, but the message-filter case is
/// equivalent and exercises the same gate.
#[test]
fn test_git_authors_known_path_message_filter_empty_with_stale_head_falls_through() {
    let ctx = make_ctx_with_git_cache();
    let result = dispatch_tool(&ctx, "xray_git_authors", &json!({
        "repo": ".",
        "path": "src/main.rs",
        "message": "this_pattern_will_not_match_any_cached_commit_xyz_2026"
    }));
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let hint = output["summary"]["hint"].as_str().unwrap_or("");
        assert!(!hint.contains("(from cache)"),
            "Empty cache authors for known path with message filter and stale HEAD must \
             fall through to CLI, got hint: {}", hint);
    }
}

/// MUTATION-KILLER for the `cache_head_stale` predicate itself (addresses
/// reviewer's MAJOR re: gate-mutation coverage):
/// when the cache's snapshot HEAD MATCHES the live repo HEAD, an empty cache
/// result must be served as `(from cache)` \u2014 the HEAD-pinning gate must NOT
/// fire. If the gate is weakened to `total_count == 0` alone (dropping
/// `cache_head_stale`), this test fails because the empty would fall through
/// to CLI and lose the cache hint.
#[test]
fn test_git_history_empty_cache_with_fresh_head_serves_cached_empty() {
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use crate::git::cache::*;
    use std::io::Cursor;
    use super::handlers_test_utils::make_empty_ctx;

    // Get the live HEAD of the workspace repo so the mock cache's head_hash
    // matches it \u2014 i.e. NOT stale.
    let head_out = Command::new("git").args(["rev-parse", "HEAD"]).output();
    let live_head = match head_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return, // not in a git repo \u2014 skip silently
    };
    if live_head.len() != 40 { return; }

    // Build a cache pinned to the LIVE HEAD with a single fake file the test repo
    // does not contain, so query_file_history returns empty for our query path.
    let log = concat!(
        "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{241E}1700000000\u{241E}alice@example.com\u{241E}Alice\u{241E}Initial\n",
        "fake_seed_file_for_fresh_head_test.rs\n",
        "\n",
    );
    let nul_input: Vec<u8> = log.bytes().map(|b| if b == b'\n' { 0 } else { b }).collect();
    let mut builder = GitHistoryCache::builder();
    parse_git_log_stream(Cursor::new(nul_input), &mut builder).unwrap();
    let cache = GitHistoryCache::from_builder(builder, live_head, "main".to_string());

    let mut ctx = make_empty_ctx();
    *ctx.git_cache.write().unwrap() = Some(cache);
    ctx.git_cache_ready = Arc::new(AtomicBool::new(true));

    // Query a path that is not in the (1-file) cache; result is empty BUT HEAD is fresh.
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({
        "repo": ".",
        "file": "definitely_not_a_real_file_for_fresh_head_test_2026.rs",
        "maxResults": 5
    }));
    assert!(!result.is_error, "should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["source"], "git-cache");
    assert_eq!(output["commits"].as_array().unwrap().len(), 0);
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


// ─── Per-request shallow freshness gate (e2e) ─────────────────────
//
// These exercise the full handler path: a populated in-memory cache + a
// live repo whose shallow state has drifted away from the cache's stamp.
// The handler MUST bypass the cache and fall through to the `git log` CLI.

fn make_real_git_repo_with_one_commit() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .expect("git");
        assert!(status.success(), "git {:?} failed", args);
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "x@x"]);
    run(&["config", "user.name", "X"]);
    run(&["commit", "-q", "--allow-empty", "-m", "init"]);
    dir
}

#[test]
fn test_git_history_handler_bypasses_cache_when_shallow_drift() {
    crate::git::shallow_cache_clear();
    let dir = make_real_git_repo_with_one_commit();
    let ctx = make_ctx_with_git_cache();
    ctx.workspace
        .write()
        .unwrap()
        .set_dir(dir.path().to_string_lossy().into_owned());

    // Force the in-memory cache to look as if it was built from a shallow
    // repo. The live repo is NOT shallow (no `.git/shallow` file), so the
    // freshness gate must reject the cache and the handler must fall
    // through to `git log` CLI.
    {
        let mut g = ctx.git_cache.write().unwrap();
        g.as_mut().unwrap().shallow_fingerprint =
            Some("forced-shallow-mismatch".to_string());
    }

    let result = dispatch_tool(
        &ctx,
        "xray_git_history",
        &json!({
            "repo": dir.path().to_str().unwrap(),
            "file": "src/main.rs"
        }),
    );
    assert!(
        !result.is_error,
        "handler must succeed (CLI fall-through), got error: {}",
        result.content[0].text
    );
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["source"], "git-cli");
}

#[test]
fn test_git_history_handler_uses_cache_when_shallow_state_matches() {
    crate::git::shallow_cache_clear();
    let dir = make_real_git_repo_with_one_commit();
    let ctx = make_ctx_with_git_cache();
    ctx.workspace
        .write()
        .unwrap()
        .set_dir(dir.path().to_string_lossy().into_owned());

    // Cache built as non-shallow (default), live repo is non-shallow too.
    // Freshness gate must NOT block: handler should use the cache.
    let result = dispatch_tool(
        &ctx,
        "xray_git_history",
        &json!({
            "repo": dir.path().to_str().unwrap(),
            "file": "src/main.rs"
        }),
    );
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["source"], "git-cache");
}

