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
/// without eagerly rebuilding the per-file reverse-token map.
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
    assert!(idx.file_tokens.is_empty(),
        "watch-mode cross-load must leave per-file token reverse map lazy on cache load");
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
    assert!(idx.file_tokens.is_empty(),
        "background cross-load must leave per-file token reverse map lazy");
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
    assert!(idx.file_tokens.is_empty(),
        "watch-mode xray_reindex must leave per-file token reverse map lazy after rebuild");
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
