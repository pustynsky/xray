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
    rollback_workspace_state, cross_load_definition_index, handle_xray_reindex_inner,
};
use serde_json::json;

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
    let mode_before;
    let generation_before;
    {
        let ws = ctx.workspace.read().unwrap();
        assert_eq!(ws.mode, WorkspaceBindingMode::PinnedCli,
            "Precondition: default ctx must be in PinnedCli mode");
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

    // Critical: workspace state must be UNCHANGED (no partial mutation).
    let ws = ctx.workspace.read().unwrap();
    assert_eq!(ws.dir, pinned_dir_before,
        "Rejected switch must NOT mutate workspace.dir");
    assert_eq!(ws.mode, mode_before,
        "Rejected switch must NOT mutate workspace.mode");
    assert_eq!(ws.generation, generation_before,
        "Rejected switch must NOT bump workspace.generation");
}