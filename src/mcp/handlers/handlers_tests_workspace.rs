//! P2 Group D: Regression tests for the `cmd_serve` / `handle_xray_reindex_inner`
//! complexity refactoring. Validates the state-machine helpers extracted during
//! that refactor: `rollback_workspace_state`, `cross_load_definition_index`,
//! and the PinnedCli-mode workspace-switch guard inside `handle_xray_reindex_inner`.
//!
//! These tests live in a #[path]-mounted submodule of `mcp::handlers::mod` so
//! they can call the private (`fn`, not `pub`) helpers directly without exposing
//! them in the public API surface.

use super::{
    HandlerContext, WorkspaceBindingMode, WorkspaceStatus,
    cross_load_content_index, cross_load_definition_index,
    handle_xray_reindex_definitions_inner, handle_xray_reindex_inner,
    rollback_workspace_state,
};
use super::handlers_test_utils::make_ctx_with_defs;
use serde_json::json;


fn wait_until(predicate: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    predicate()
}

/// D1 — `rollback_workspace_state` must restore ALL four mutable fields of
/// `WorkspaceBinding` (dir, canonical_dir-via-set_dir, mode, generation) AND
/// force `status = Resolved`. Regression of any one of these would leave the
/// server stuck in `Reindexing` or pointing at a corrupted dir after an error.
#[test]
fn test_rollback_workspace_state_restores_all_fields() {
    let ctx = HandlerContext::default();

    // Snapshot the "good" pre-switch state.
    let original_dir = "/good/workspace".to_string();
    let original_mode = WorkspaceBindingMode::ManualOverride;
    let original_generation: u64 = 42;
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(original_dir.clone());
        ws.mode = original_mode;
        ws.generation = original_generation;
        ws.status = WorkspaceStatus::Resolved;
    }

    // Simulate a workspace switch in progress: every field is changed and
    // status is flipped to `Reindexing` (mid-flight).
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir("/bad/half_built".to_string());
        ws.mode = WorkspaceBindingMode::DotBootstrap;
        ws.generation = 999;
        ws.status = WorkspaceStatus::Reindexing;
    }

    // Roll back.
    rollback_workspace_state(&ctx, &original_dir, original_mode, original_generation);

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, original_dir,
        "rollback must restore dir to the pre-switch value");
    assert!(ws.canonical_dir.contains("good") || ws.canonical_dir == original_dir,
        "rollback must recompute canonical_dir from the restored dir; got: {}", ws.canonical_dir);
    assert_eq!(ws.mode, original_mode,
        "rollback must restore mode (regression: leaving DotBootstrap leaks bootstrap behavior)");
    assert_eq!(ws.generation, original_generation,
        "rollback must restore generation (regression: stale generation breaks generation-safe commits)");
    assert_eq!(ws.status, WorkspaceStatus::Resolved,
        "rollback MUST force status=Resolved (regression: server stuck in Reindexing after error → all tools blocked)");
}

/// D2 — `cross_load_definition_index` must early-return `None` when the
/// HandlerContext was constructed without a definition index (`def_index = None`).
/// Regression here would either panic on `.unwrap()` of the missing Arc or spawn
/// a useless background thread that then fails to write to the absent index.
#[test]
fn test_cross_load_definition_index_returns_none_when_def_index_disabled() {
    let ctx = HandlerContext::default();
    assert!(ctx.def_index.is_none(),
        "Precondition: HandlerContext::default() must have def_index = None");

    let result = cross_load_definition_index(&ctx, "/any/dir");

    assert!(result.is_none(),
        "cross_load_definition_index must return None when def_index is disabled \
         (regression: would otherwise spawn background thread that panics or no-ops)");
}

/// Regression guard: content-index cache cross-load must preserve watch mutation mode.
/// The cached read-only index has no watch lookups; handler code must re-enable them
/// (set `file_tokens_authoritative=true`, populate `path_to_id`) and schedule a
/// background `rebuild_file_tokens` so the first user edit doesn't pay the rebuild
/// cost under the exclusive watcher write lock (C2-A, 2026-05-02).
#[test]
fn test_cross_load_content_index_preserves_watch_lazy_reverse_map_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("watched.rs"),
        "fn cross_load_watch_mode_token() {}\n",
    ).unwrap();

    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let index_base = tmp.path().join("indexes");
    let content_index = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: workspace_str.clone(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    assert!(!content_index.file_tokens_authoritative,
        "Precondition: cached build output should be read-only/plain");
    crate::index::save_content_index(&content_index, &index_base).unwrap();

    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = index_base;
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::mcp::watcher::build_watch_index_from(crate::ContentIndex {
            root: workspace_str.clone(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        });
        assert!(current.file_tokens_authoritative,
            "Precondition: existing runtime index must represent watcher mode");
    }

    let action = cross_load_content_index(&ctx, &workspace_str);

    assert_eq!(action, Some("loaded_cache"));
    let idx = ctx.index.read().unwrap();
    assert!(idx.file_tokens_authoritative,
        "cross-load must preserve watcher-authoritative reverse-map mode");
    assert!(idx.path_to_id.as_ref().map(|lookup| !lookup.is_empty()).unwrap_or(false),
        "watch-mode cross-load must rebuild path lookup for cached content index");
    drop(idx);
    // C2-A: cache-load schedules a background `rebuild_file_tokens` so the
    // first user edit doesn't pay the rebuild cost under the watcher write
    // lock. Wait until that background warmer populates the reverse map.
    assert!(
        wait_until(|| ctx.index.read().map(|i| !i.file_tokens.is_empty()).unwrap_or(false)),
        "watch-mode cross-load must warm per-file token reverse map in background"
    );
}
#[test]
fn test_cross_load_content_index_background_build_preserves_watch_lazy_reverse_map_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_bg");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("watched.rs"),
        "fn background_cross_load_watch_mode_token() {}\n",
    ).unwrap();

    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes_bg");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
    }
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::mcp::watcher::build_watch_index_from(crate::ContentIndex {
            root: "old-workspace".to_string(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        });
        assert!(current.file_tokens_authoritative,
            "Precondition: existing runtime index must represent watcher mode");
    }

    let action = cross_load_content_index(&ctx, &workspace_str);

    assert_eq!(action, Some("background_build"));
    assert!(wait_until(|| ctx.content_ready.load(std::sync::atomic::Ordering::Acquire)),
        "background content build should publish readiness after installing the new index");
    let idx = ctx.index.read().unwrap();
    assert_eq!(idx.root, workspace_str);
    assert!(idx.file_tokens_authoritative,
        "background cross-load must preserve watcher-authoritative mode");
    assert!(idx.path_to_id.as_ref().map(|lookup| !lookup.is_empty()).unwrap_or(false),
        "background cross-load must build path lookup for watcher mutation mode");
    drop(idx);
    // C2-A: see sibling test above. Background build path also schedules a
    // background `rebuild_file_tokens` post-install.
    assert!(
        wait_until(|| ctx.index.read().map(|i| !i.file_tokens.is_empty()).unwrap_or(false)),
        "background cross-load must warm per-file token reverse map in background"
    );
}

#[test]
fn test_cross_load_content_index_background_build_failure_does_not_mark_ready() {
    let tmp = tempfile::tempdir().unwrap();
    let missing_workspace = tmp.path().join("missing_workspace");
    let missing_workspace_str = crate::clean_path(&missing_workspace.to_string_lossy());
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes_fail");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(missing_workspace_str.clone());
    }
    ctx.content_ready.store(true, std::sync::atomic::Ordering::Release);
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::ContentIndex {
            root: "old-workspace".to_string(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        };
    }

    let action = cross_load_content_index(&ctx, &missing_workspace_str);

    assert_eq!(action, Some("background_build"));
    assert!(wait_until(|| !ctx.content_building.load(std::sync::atomic::Ordering::Acquire)),
        "background content build should exit after build failure");
    assert!(!ctx.content_ready.load(std::sync::atomic::Ordering::Acquire),
        "failed background cross-load must not advertise content readiness");
    let idx = ctx.index.read().unwrap();
    assert_eq!(idx.root, "old-workspace",
        "failed background cross-load must not replace the previous index");
}


#[test]
fn test_cross_load_content_index_existing_background_build_does_not_mark_ready() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_waiting");
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes_waiting");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
    }
    ctx.content_ready.store(true, std::sync::atomic::Ordering::Release);
    ctx.content_building.store(true, std::sync::atomic::Ordering::Release);
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::ContentIndex {
            root: "old-workspace".to_string(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        };
    }

    let action = cross_load_content_index(&ctx, &workspace_str);

    assert_eq!(action, Some("build_in_progress"));
    assert!(!ctx.content_ready.load(std::sync::atomic::Ordering::Acquire),
        "new workspace must stay not-ready while another content build is in flight");
    let idx = ctx.index.read().unwrap();
    assert_eq!(idx.root, "old-workspace",
        "pre-existing build gate must not replace the index for this workspace");
}

#[test]
fn test_cross_load_content_index_stale_background_build_does_not_publish() {
    let tmp = tempfile::tempdir().unwrap();
    let stale_workspace = tmp.path().join("stale_workspace");
    let current_workspace = tmp.path().join("current_workspace");
    std::fs::create_dir_all(&stale_workspace).unwrap();
    std::fs::create_dir_all(&current_workspace).unwrap();
    std::fs::write(
        stale_workspace.join("stale.rs"),
        "fn stale_background_build_token() {}\n",
    ).unwrap();
    let stale_workspace_str = crate::clean_path(
        &std::fs::canonicalize(&stale_workspace).unwrap().to_string_lossy(),
    );
    let current_workspace_str = crate::clean_path(
        &std::fs::canonicalize(&current_workspace).unwrap().to_string_lossy(),
    );
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes_stale");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(current_workspace_str);
        ws.generation += 1;
    }
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::ContentIndex {
            root: "old-workspace".to_string(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        };
    }

    let action = cross_load_content_index(&ctx, &stale_workspace_str);

    assert_eq!(action, Some("background_build"));
    assert!(wait_until(|| !ctx.content_building.load(std::sync::atomic::Ordering::Acquire)),
        "stale background content build should exit");
    assert!(!ctx.content_ready.load(std::sync::atomic::Ordering::Acquire),
        "stale background build must not mark the current workspace ready");
    let idx = ctx.index.read().unwrap();
    assert_eq!(idx.root, "old-workspace",
        "stale background build must not publish an index for another workspace");
}


/// Regression guard: explicit xray_reindex must also preserve watch mutation mode.
/// This protects the handler-level sentinel from regressing back to path_to_id presence.
#[test]
fn test_handle_xray_reindex_inner_preserves_watch_lazy_reverse_map_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("watched.rs"),
        "fn handler_reindex_watch_mode_token() {}\n",
    ).unwrap();

    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
    }
    {
        let mut current = ctx.index.write().unwrap();
        *current = crate::mcp::watcher::build_watch_index_from(crate::ContentIndex {
            root: workspace_str.clone(),
            extensions: vec!["rs".to_string()],
            ..Default::default()
        });
        assert!(current.file_tokens_authoritative,
            "Precondition: existing runtime index must represent watcher mode");
    }

    let result = handle_xray_reindex_inner(&ctx, &json!({}));

    assert!(!result.is_error,
        "same-workspace xray_reindex should succeed in PinnedCli mode");
    let idx = ctx.index.read().unwrap();
    assert!(idx.file_tokens_authoritative,
        "xray_reindex must preserve watcher-authoritative reverse-map mode");
    assert!(idx.path_to_id.as_ref().map(|lookup| !lookup.is_empty()).unwrap_or(false),
        "watch-mode xray_reindex must rebuild path lookup for the rebuilt index");
    drop(idx);
    // C2-A: xray_reindex install path also schedules a background warmer.
    assert!(
        wait_until(|| ctx.index.read().map(|i| !i.file_tokens.is_empty()).unwrap_or(false)),
        "watch-mode xray_reindex must warm per-file token reverse map in background"
    );
}


/// D3 — `handle_xray_reindex_inner` must reject a workspace switch when the
/// server is in `PinnedCli` mode (started with `--dir <path>`). The request
/// must return an error result AND must NOT mutate the workspace binding
/// (dir, mode, generation must all stay at their pinned values).
///
/// This test exercises the early-return guard at the top of
/// `handle_xray_reindex_inner` — without it, a malicious or buggy reindex call
/// could escape the CLI-pinned workspace.
#[test]
fn test_handle_xray_reindex_inner_rejects_switch_in_pinned_cli_mode() {
    let ctx = HandlerContext::default();
    // HandlerContext::default() pins ws to ".", PinnedCli mode, generation=1.
    let pinned_dir_before = ctx.server_dir();
    let canonical_dir_before;
    let mode_before;
    let generation_before;
    {
        let ws = ctx.workspace.read().unwrap();
        assert_eq!(ws.mode, WorkspaceBindingMode::PinnedCli,
            "Precondition: default ctx must be in PinnedCli mode");
        canonical_dir_before = ws.canonical_dir.clone();
        mode_before = ws.mode;
        generation_before = ws.generation;
    }

    // Request a switch to a totally different directory.
    // We pick a non-existent path so canonicalize() falls back to the literal
    // string — that's still distinct from the pinned ".", so workspace_changed=true,
    // which triggers the PinnedCli guard.
    let switch_target = if cfg!(windows) {
        "Z:\\nonexistent_xray_test_target_d3"
    } else {
        "/nonexistent_xray_test_target_d3"
    };
    let args = json!({"dir": switch_target});

    let result = handle_xray_reindex_inner(&ctx, &args);
    assert!(result.is_error,
        "PinnedCli mode must reject workspace switches; got success result");
    let msg = result.content.iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>().join(" ");
    let msg_lower = msg.to_lowercase();
    assert!(msg_lower.contains("pinned") || msg_lower.contains("--dir"),
        "Error message must explain the pinning constraint; got: {}", msg);
    assert!(msg.contains("omit the `dir` argument"),
        "Error message must tell callers how to rebuild the pinned workspace; got: {}", msg);

    // Critical: workspace state must be UNCHANGED (no partial mutation).
    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, pinned_dir_before,
        "Rejected switch must NOT mutate workspace.dir");
    assert_eq!(ws.canonical_dir, canonical_dir_before,
        "Rejected switch must NOT mutate workspace.canonical_dir");
    assert_eq!(ws.mode, mode_before,
        "Rejected switch must NOT mutate workspace.mode");
    assert_eq!(ws.generation, generation_before,
        "Rejected switch must NOT bump workspace.generation");
}

#[cfg(windows)]
#[test]
fn test_handle_xray_reindex_definitions_allows_same_pinned_dir_with_separator_variation() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("sample.rs"),
        "fn separator_regression_sample() {}\n",
    ).unwrap();

    let canonical = std::fs::canonicalize(&workspace).unwrap();
    let forward_dir = code_xray::clean_path(&canonical.to_string_lossy());
    let backslash_dir = forward_dir.replace('/', "\\");

    let mut ctx = make_ctx_with_defs();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = tmp.path().join("indexes");
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(backslash_dir.clone());
        ws.mode = WorkspaceBindingMode::PinnedCli;
    }
    let (canonical_dir_before, generation_before) = {
        let ws = ctx.workspace.read().unwrap();
        (ws.canonical_dir.clone(), ws.generation)
    };

    let result = handle_xray_reindex_definitions_inner(
        &ctx,
        &json!({ "dir": forward_dir, "ext": ["rs"] }),
    );

    assert!(
        !result.is_error,
        "same pinned workspace with separator variation should reindex; got: {}",
        result.content[0].text
    );
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["status"], "ok");
    assert!(
        output.get("workspaceChanged").is_none(),
        "same workspace must not be reported as a switch: {}",
        output
    );

    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, backslash_dir);
    assert_eq!(ws.canonical_dir, canonical_dir_before);
    assert_eq!(ws.mode, WorkspaceBindingMode::PinnedCli);
    assert_eq!(ws.generation, generation_before);
}

#[test]
fn test_reindex_pinned_switch_errors_tell_callers_to_omit_dir() {
    let switch_target = if cfg!(windows) {
        "Z:\\nonexistent_xray_test_target_guidance"
    } else {
        "/nonexistent_xray_test_target_guidance"
    };
    let args = json!({"dir": switch_target});

    let content_ctx = HandlerContext::default();
    let definitions_ctx = make_ctx_with_defs();
    for ctx in [&content_ctx, &definitions_ctx] {
        ctx.workspace.write().unwrap().mode = WorkspaceBindingMode::PinnedCli;
    }
    let cases = [
        ("xray_reindex", handle_xray_reindex_inner(&content_ctx, &args)),
        (
            "xray_reindex_definitions",
            handle_xray_reindex_definitions_inner(&definitions_ctx, &args),
        ),
    ];

    for (tool_name, result) in cases {
        assert!(result.is_error, "{} must reject pinned workspace switches", tool_name);
        let msg = result.content.iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>().join(" ");
        assert!(
            msg.contains("omit the `dir` argument"),
            "{} pinned switch error must tell callers to omit dir; got: {}",
            tool_name,
            msg
        );
        assert!(
            msg.contains("start another server instance or use CLI"),
            "{} pinned switch error must explain how to index a different workspace; got: {}",
            tool_name,
            msg
        );
    }
}

// ─── Bug-report 2026-04-28: force-rebuild + live branch warning ─────────
//
// Validates that `xray_reindex` against the *current* workspace forces a
// fresh rebuild instead of silently returning the on-disk cache, and that
// the optional `useCache: true` opt-out preserves the legacy fast path for
// callers who prefer cold-rebuild latency over freshness. Also covers the
// branch_warning live-probe TTL cache and its workspace-switch invalidation.

/// Helper: build a content-index cache on disk for `workspace_str`, then
/// register `ctx` against it without loading anything into memory yet — so
/// each test starts from a known "cache-on-disk, empty-in-memory" state.
fn seed_disk_cache(
    ctx: &mut HandlerContext,
    tmp: &tempfile::TempDir,
    workspace_str: &str,
) {
    let index_base = tmp.path().join("indexes");
    let cache = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: workspace_str.to_string(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    crate::index::save_content_index(&cache, &index_base).unwrap();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = index_base;
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.to_string());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }
}

/// Bug-report 2026-04-28, Acceptance #3: `xray_reindex` against the current
/// workspace must rebuild from the live filesystem (`indexAction="rebuilt"`,
/// `rebuiltFromDisk=true`, `forceRebuild=true`) rather than silently reusing
/// the existing on-disk cache.
#[test]
fn test_handle_xray_reindex_inner_forces_rebuild_for_current_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn force_rebuild_seed() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = HandlerContext::default();
    seed_disk_cache(&mut ctx, &tmp, &workspace_str);

    let result = handle_xray_reindex_inner(&ctx, &json!({}));
    assert!(!result.is_error, "current-workspace reindex must succeed");
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["indexAction"], "rebuilt",
        "current-workspace reindex must rebuild from disk, got: {}", output);
    assert_eq!(output["rebuiltFromDisk"], true,
        "rebuiltFromDisk must mirror indexAction=='rebuilt'");
    assert_eq!(output["forceRebuild"], true,
        "forceRebuild must be true for current-workspace reindex without useCache");
}

/// Bug-report 2026-04-28, Acceptance #3 (opt-out): explicit `useCache: true`
/// preserves the legacy cache-load fast path for callers who prefer
/// cold-rebuild latency over freshness.
#[test]
fn test_handle_xray_reindex_inner_use_cache_opt_in_keeps_loaded_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_use_cache");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn use_cache_seed() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = HandlerContext::default();
    seed_disk_cache(&mut ctx, &tmp, &workspace_str);

    let result = handle_xray_reindex_inner(&ctx, &json!({"useCache": true}));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["indexAction"], "loaded_cache",
        "useCache=true must keep the legacy fast path");
    assert_eq!(output["rebuiltFromDisk"], false);
    assert_eq!(output["forceRebuild"], false);
}

/// Bug-report 2026-04-28, Acceptance #4: a token added to disk after the
/// cache was built becomes searchable in the in-memory index immediately
/// after `xray_reindex` (because force-rebuild walks the live filesystem).
#[test]
fn test_handle_xray_reindex_inner_picks_up_new_tokens_from_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_new_token");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn before_reindex_token() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = HandlerContext::default();
    seed_disk_cache(&mut ctx, &tmp, &workspace_str);

    // Mutate the workspace AFTER the cache was built. Use a uniquely-shaped
    // identifier so substring-search is unambiguous.
    std::fs::write(
        workspace.join("seed.rs"),
        "fn after_reindex_uniq_zzqq_token() {}\n",
    ).unwrap();

    let result = handle_xray_reindex_inner(&ctx, &json!({}));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["indexAction"], "rebuilt");

    let idx = ctx.index.read().unwrap();
    let tokens: Vec<&String> = idx.index.keys().collect();
    assert!(
        tokens.iter().any(|t| t.contains("after_reindex_uniq_zzqq_token")),
        "force-rebuild must surface tokens added after the cache was built; tokens={:?}",
        tokens.iter().filter(|t| t.contains("reindex")).collect::<Vec<_>>()
    );
}

/// `build_or_load_content_index(force_rebuild=false)` must still serve the
/// existing on-disk cache. Pins the workspace-switch fast path semantics so a
/// future refactor can't accidentally make every cross-load do a full rebuild.
#[test]
fn test_build_or_load_content_index_force_false_uses_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_keep_cache");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn keep_cache_seed() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let index_base = tmp.path().join("indexes");
    let cache = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: workspace_str.clone(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    crate::index::save_content_index(&cache, &index_base).unwrap();

    let (_idx, action) = super::build_or_load_content_index(
        &workspace_str, "rs", &index_base, false, /* force_rebuild = */ false,
    ).expect("cached index should load");
    assert_eq!(action, "loaded_cache",
        "force_rebuild=false must keep the cache fast path");
}

/// `build_or_load_content_index(force_rebuild=true)` must rebuild even when a
/// matching on-disk cache exists.
#[test]
fn test_build_or_load_content_index_force_true_ignores_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_force_true");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("seed.rs"), "fn force_true_seed() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let index_base = tmp.path().join("indexes");
    let cache = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: workspace_str.clone(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    crate::index::save_content_index(&cache, &index_base).unwrap();

    let (_idx, action) = super::build_or_load_content_index(
        &workspace_str, "rs", &index_base, false, /* force_rebuild = */ true,
    ).expect("force-rebuild should still produce an index");
    assert_eq!(action, "rebuilt",
        "force_rebuild=true must bypass the cache fast path");
}

/// Bug-report 2026-04-28: `branch_warning` with `live_branch_probe_enabled=false`
/// (test default) keeps reading the static `current_branch` snapshot — this
/// guarantees that turning the long-running probe off via the flag keeps the
/// pre-fix behaviour for tests and CLI subcommands.
#[test]
fn test_branch_warning_live_probe_disabled_uses_startup_value() {
    let ctx = HandlerContext {
        current_branch: Some("feature/legacy".to_string()),
        ..HandlerContext::default()
    };
    assert!(!ctx.live_branch_probe_enabled,
        "Default ctx must keep the live probe disabled");
    let warning = super::utils::branch_warning(&ctx);
    assert!(warning.is_some(),
        "feature-branch startup value must still surface a warning");
    assert!(warning.unwrap().contains("feature/legacy"));
}

/// Bug-report 2026-04-28: a workspace switch via `handle_xray_reindex_inner`
/// must drop the cached live-branch probe entry for the outgoing workspace,
/// so a subsequent call probing the same dir cannot serve a pre-switch value.
#[test]
fn test_workspace_switch_invalidates_current_branch_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace_a = tmp.path().join("ws_a");
    let workspace_b = tmp.path().join("ws_b");
    for w in [&workspace_a, &workspace_b] {
        std::fs::create_dir_all(w).unwrap();
        std::fs::write(w.join("seed.rs"), "fn ws_switch_seed() {}\n").unwrap();
    }
    let dir_a = crate::clean_path(
        &std::fs::canonicalize(&workspace_a).unwrap().to_string_lossy(),
    );
    let dir_b = crate::clean_path(
        &std::fs::canonicalize(&workspace_b).unwrap().to_string_lossy(),
    );

    let mut ctx = HandlerContext::default();
    seed_disk_cache(&mut ctx, &tmp, &dir_a);
    // Workspace must be switchable for this test (default is PinnedCli).
    ctx.workspace.write().unwrap().mode = WorkspaceBindingMode::ManualOverride;

    // Pre-populate cache for the outgoing workspace so we can observe the
    // explicit removal that `handle_xray_reindex_inner` performs on switch.
    {
        let mut cache = ctx.current_branch_cache.write().unwrap();
        cache.insert(
            dir_a.clone(),
            (std::time::Instant::now(), Some("feature/should_be_evicted".to_string())),
        );
    }
    assert!(
        ctx.current_branch_cache.read().unwrap().contains_key(&dir_a),
        "Precondition: cache must contain the outgoing-workspace entry"
    );

    // Build a cache on disk for ws_b too so the switch's cross-load succeeds.
    let cache_b = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: dir_b.clone(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    crate::index::save_content_index(&cache_b, &ctx.index_base).unwrap();

    let result = handle_xray_reindex_inner(&ctx, &json!({"dir": dir_b}));
    assert!(!result.is_error, "workspace switch must succeed: {}", result.content[0].text);

    let cache = ctx.current_branch_cache.read().unwrap();
    assert!(
        !cache.contains_key(&dir_a),
        "Workspace switch must drop the outgoing-workspace branch cache entry"
    );
}

/// Bug-report 2026-04-28 (reviewer follow-up): when `save_content_index` fails
/// for a force-rebuilt index, the function MUST return the freshly-built
/// in-memory index instead of re-loading the surviving (stale) on-disk cache.
/// We force the failure by pointing `index_base` at a regular file — the
/// `create_dir_all(index_base)` at the top of `save_content_index` then errors
/// because the path component already exists as a non-directory.
#[test]
fn test_build_or_load_content_index_save_failure_keeps_fresh_in_memory_index() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace_save_fail");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("seed.rs"),
        "fn save_fail_uniq_zzqq_token() {}\n",
    ).unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    // Point index_base at a regular FILE — save_content_index's create_dir_all
    // will fail with ErrorKind::NotADirectory (Linux) / similar on Windows.
    let index_base_file = tmp.path().join("not_a_dir.dat");
    std::fs::write(&index_base_file, b"placeholder").unwrap();

    let (idx, action) = super::build_or_load_content_index(
        &workspace_str,
        "rs",
        &index_base_file,
        false,
        /* force_rebuild = */ true,
    ).expect("force-rebuild must yield an in-memory index even when save fails");

    assert_eq!(
        action, "rebuilt",
        "save failure must NOT downgrade indexAction to loaded_cache"
    );
    let tokens: Vec<&String> = idx.index.keys().collect();
    assert!(
        tokens.iter().any(|t| t.contains("save_fail_uniq_zzqq_token")),
        "in-memory idx must contain freshly-built tokens despite save failure; matching={:?}",
        tokens.iter().filter(|t| t.contains("save_fail")).collect::<Vec<_>>()
    );
}

/// Bug-report 2026-04-28 (reviewer Round-2 finding): the same save-failure
/// stale-cache bug existed in `handle_xray_reindex_definitions_inner` (the
/// `xray_reindex_definitions` handler). Mirror the content-side fix: when
/// `save_definition_index` fails, keep the freshly-built `new_index` in
/// memory and skip the drop+reload — otherwise the handler would silently
/// re-publish stale definitions while reporting fresh counts to the caller.
#[test]
fn test_handle_xray_reindex_definitions_inner_save_failure_keeps_fresh_in_memory_index() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws_def_save_fail");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("seed.rs"),
        "fn def_save_fail_uniq_zzqq_marker() {}\n",
    ).unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = make_ctx_with_defs();
    ctx.server_ext = "rs".to_string();
    // Force save_definition_index to fail by pointing index_base at a regular
    // file: the `create_dir_all(index_base)` inside the save layer will error
    // because the path component already exists as a non-directory.
    let index_base_file = tmp.path().join("not_a_dir.dat");
    std::fs::write(&index_base_file, b"placeholder").unwrap();
    ctx.index_base = index_base_file;
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }

    let result = handle_xray_reindex_definitions_inner(&ctx, &json!({}));
    assert!(
        !result.is_error,
        "definition reindex must succeed even when save fails: {}",
        result.content[0].text
    );

    // Verify the in-memory definition index has the fresh symbol — not the
    // stale entries baked into make_ctx_with_defs() (ResilientClient etc.).
    let def_arc = ctx.def_index.as_ref().expect("ctx must have def_index_arc").clone();
    let def_idx = def_arc.read().unwrap();
    assert!(
        def_idx.definitions.iter().any(|d| d.name.contains("def_save_fail_uniq_zzqq_marker")),
        "in-memory def index must contain freshly-built symbol despite save failure; got {} definitions",
        def_idx.definitions.len()
    );
}


/// Save-failure smoke test (2026-05-05) for `cross_load_definition_index`'s
/// background-build path. Forces `save_definition_index` to fail by pointing
/// `index_base` at a regular file (so `create_dir_all(index_base)` inside the
/// save layer errors), then waits for the BG build to publish via `def_ready`
/// and asserts the in-memory index has the freshly-built marker.
///
/// **Documentary, NOT mutation-killing.** This test cannot distinguish the
/// FIXED path (`if save_failed { new_idx } else { drop+reload }`) from the
/// inverted-conditional mutation: when `index_base` is a regular file the
/// drop+reload's `load_definition_index` ALSO fails and the rebuild-fallback
/// rescues correctness with fresh data either way. A stale-shard
/// mutation-killer (pre-seed shard → read-only → force save fail → expect
/// drop+reload to read stale) cannot work here either, because the cache
/// check at the top of `cross_load_definition_index` and the drop+reload at
/// the bottom both call `load_definition_index` — if a pre-seeded shard is
/// readable the cache check short-circuits to `"loaded_cache"` before the
/// BG build runs, so we never reach the save-failed branch under test. The
/// mutation-killing variant lives in
/// `test_handle_xray_reindex_definitions_inner_save_failure_does_not_publish_stale_shard`,
/// which exercises a sibling path (`xray_reindex_definitions`) that always
/// rebuilds and therefore can be tested with a stale-shard fixture. This
/// test catches grosser regressions only: removed save call, missing
/// fallback, or BG build never publishing on save failure.
#[test]
#[cfg(feature = "lang-rust")]
fn test_cross_load_definition_index_save_failure_keeps_fresh_in_memory_index() {
    use std::sync::atomic::Ordering;

    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws_cross_load_save_fail");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("seed.rs"),
        "fn cross_load_save_fail_uniq_zzqq_marker() {}\n",
    ).unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );

    let mut ctx = make_ctx_with_defs();
    ctx.def_extensions = vec!["rs".to_string()];
    // Force save_definition_index to fail by pointing index_base at a regular
    // file: the `create_dir_all(index_base)` inside the save layer will error
    // because the path component already exists as a non-directory.
    let index_base_file = tmp.path().join("not_a_dir.dat");
    std::fs::write(&index_base_file, b"placeholder").unwrap();
    ctx.index_base = index_base_file;
    // Reset def_ready so we can poll it for background completion below.
    ctx.def_ready.store(false, Ordering::Release);

    let action = cross_load_definition_index(&ctx, &workspace_str);
    assert_eq!(
        action,
        Some("background_build"),
        "no cache exists — must spawn background build, not load_cache"
    );

    // Wait for background build to publish to the def_index Arc.
    assert!(
        wait_until(|| ctx.def_ready.load(Ordering::Acquire)),
        "background build did not signal def_ready within timeout"
    );

    // Verify the in-memory def index has the fresh symbol — not the stale
    // entries baked into make_ctx_with_defs() (ResilientClient etc.).
    let def_arc = ctx.def_index.as_ref().expect("ctx must have def_index_arc").clone();
    let def_idx = def_arc.read().unwrap();
    assert!(
        def_idx.definitions.iter().any(|d| d.name.contains("cross_load_save_fail_uniq_zzqq_marker")),
        "cross-loaded def index must contain freshly-built symbol despite save failure; got {} definitions",
        def_idx.definitions.len()
    );
}

/// Bug-report 2026-04-28 (reviewer Round-3 stricter coverage): the
/// previous save-failure tests only prove fresh data ends up in memory —
/// they do not distinguish the BUGGY drop+reload+rebuild fallback path
/// (which would also publish fresh data when load also fails) from the
/// FIXED path that returns the in-memory new_index directly. This test
/// explicitly distinguishes them by:
///   1. seeding a readable stale on-disk shard with a unique stale-only marker
///   2. mutating the workspace so the stale marker no longer exists
///   3. making every shard file read-only so save_sharded's
///      rename(temp, target) fails with ACCESS_DENIED while load_definition_index
///      can still open the (read-only but readable) stale shard
///   4. asserting the published index contains the FRESH marker AND does NOT
///      contain the stale-only marker.
///
/// Pre-fix code would publish the stale-only marker via the load fallback;
/// post-fix code keeps the in-memory fresh new_index.
///
/// Windows-only: POSIX rename over a read-only target succeeds when the
/// parent directory is writable, so this trick does not apply on Linux/macOS.
#[cfg(windows)]
#[test]
fn test_handle_xray_reindex_definitions_inner_save_failure_does_not_publish_stale_shard() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws_def_stale_pin");
    std::fs::create_dir_all(&workspace).unwrap();
    let seed_path = workspace.join("seed.rs");
    std::fs::write(&seed_path, "fn stale_only_zzqq_marker() {}\n").unwrap();
    let workspace_str = crate::clean_path(
        &std::fs::canonicalize(&workspace).unwrap().to_string_lossy(),
    );
    let index_base = tmp.path().join("indexes_stale_pin");

    // 1. Build & save STALE def index containing the unique stale-only marker.
    let stale_idx = crate::definitions::build_definition_index(
        &crate::definitions::DefIndexArgs {
            dir: workspace_str.clone(),
            ext: "rs".to_string(),
            threads: 1,
            respect_git_exclude: false,
        },
    );
    crate::definitions::save_definition_index(&stale_idx, &index_base)
        .expect("seeding stale shard must succeed");
    drop(stale_idx);

    // 2. Rewrite the workspace so the stale marker is GONE and the fresh marker
    //    is the only symbol. A correct rebuild MUST surface fresh_zzqq_marker
    //    and MUST NOT surface stale_only_zzqq_marker.
    std::fs::write(&seed_path, "fn fresh_zzqq_marker() {}\n").unwrap();

    // 3. Make every shard file read-only so save_sharded's atomic-rename fails
    //    on Windows. Load can still read the (read-only) stale shard.
    fn set_readonly_recursive(path: &std::path::Path) {
        if path.is_file() {
            let mut perm = std::fs::metadata(path).unwrap().permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(path, perm).unwrap();
        } else if path.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                set_readonly_recursive(&entry.unwrap().path());
            }
        }
    }
    set_readonly_recursive(&index_base);

    // 4. Reindex via the handler.
    let mut ctx = make_ctx_with_defs();
    ctx.server_ext = "rs".to_string();
    ctx.index_base = index_base.clone();
    {
        let mut ws = ctx.workspace.write().unwrap();
        ws.set_dir(workspace_str.clone());
        ws.mode = WorkspaceBindingMode::ManualOverride;
    }

    let result = handle_xray_reindex_definitions_inner(&ctx, &json!({}));

    // Restore writable so tempdir cleanup succeeds regardless of test outcome.
    // The `permissions_set_readonly_false` lint warns that `set_readonly(false)`
    // doesn't fully restore Unix permissions; here we only need write-back
    // for tempdir teardown on Windows, so the lint is intentional.
    #[allow(clippy::permissions_set_readonly_false)]
    fn unset_readonly_recursive(path: &std::path::Path) {
        if path.is_file() {
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perm = meta.permissions();
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(path, perm);
            }
        } else if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                unset_readonly_recursive(&entry.path());
            }
        }
    }
    unset_readonly_recursive(&index_base);

    assert!(
        !result.is_error,
        "definition reindex must succeed even when save fails: {}",
        result.content[0].text
    );

    // 5. Assert fresh marker present AND stale-only marker absent.
    let def_arc = ctx.def_index.as_ref().expect("ctx must have def_index_arc").clone();
    let def_idx = def_arc.read().unwrap();
    let names: Vec<&str> = def_idx.definitions.iter().map(|d| d.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.contains("fresh_zzqq_marker")),
        "in-memory def index must contain freshly-built symbol; got {:?}",
        names
    );
    assert!(
        !names.iter().any(|n| n.contains("stale_only_zzqq_marker")),
        "in-memory def index must NOT contain the stale-only symbol that exists only on the read-only shard; got {:?}",
        names
    );
}



