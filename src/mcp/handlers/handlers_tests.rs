//! Tests for MCP handlers -- core tests (tools, dispatch, context, readiness).
//! Grep/fast/find/git/misc tests are in separate handlers_tests_*.rs files.
//! C#-specific tests are in handlers_tests_csharp.rs.

#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability

use super::*;
use super::handlers_test_utils::make_ctx_with_defs;
use crate::Posting;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
#[test]
fn test_tool_definitions_count() {
    let tools = tool_definitions(&["cs".to_string()]);
    assert_eq!(tools.len(), 15);
}

#[test]
fn test_tool_definitions_names() {
    let tools = tool_definitions(&["cs".to_string()]);
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"xray_grep"));
    assert!(names.contains(&"xray_fast"));
    assert!(names.contains(&"xray_info"));
    assert!(names.contains(&"xray_reindex"));
    assert!(names.contains(&"xray_reindex_definitions"));
    assert!(names.contains(&"xray_definitions"));
    assert!(names.contains(&"xray_callers"));
    assert!(names.contains(&"xray_edit"));
    assert!(names.contains(&"xray_help"));
}

#[test]
fn test_tool_definitions_have_schemas() {
    let tools = tool_definitions(&["cs".to_string()]);
    for tool in &tools {
        assert!(tool.input_schema.is_object(), "Tool {} should have an object schema", tool.name);
        assert_eq!(tool.input_schema["type"], "object");
    }
}

#[test]
fn test_all_tools_have_required_field() {
    let tools = tool_definitions(&["cs".to_string()]);
    for tool in &tools {
        assert!(
            tool.input_schema.get("required").is_some(),
            "Tool '{}' inputSchema is missing 'required' field. \
             MCP clients (e.g. MS-Roo-Code) expect 'required' to always be present, \
             even as an empty array. Without it, JSON.parse() fails with \
             'Unexpected end of JSON input' during auto-approve toggle.",
            tool.name
        );
        assert!(
            tool.input_schema["required"].is_array(),
            "Tool '{}' 'required' field must be an array, got: {}",
            tool.name,
            tool.input_schema["required"]
        );
    }
}

#[test]
fn test_xray_grep_required_fields() {
    let tools = tool_definitions(&["cs".to_string()]);
    let grep = tools.iter().find(|t| t.name == "xray_grep").unwrap();
    let required = grep.input_schema["required"].as_array().unwrap();
    assert_eq!(required.len(), 1);
    assert_eq!(required[0], "terms");
}


/// Compile-time guard: lists ALL HandlerContext fields explicitly.
/// If a new field is added to HandlerContext, this test will fail to compile,
/// reminding the developer to update `impl Default` and this guard.
#[test]
fn test_handler_context_field_count_guard() {
    let _guard = HandlerContext {
        index: Arc::new(RwLock::new(ContentIndex::default())),
        def_index: None,
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(".".to_string()))),
        server_ext: "cs".to_string(),
        metrics: false,
        index_base: PathBuf::from("."),
        max_response_bytes: 0,
        content_ready: Arc::new(AtomicBool::new(true)),
        def_ready: Arc::new(AtomicBool::new(true)),
        git_cache: Arc::new(RwLock::new(None)),
        git_cache_ready: Arc::new(AtomicBool::new(false)),
        current_branch: None,
        def_extensions: Vec::new(),
        file_index: Arc::new(RwLock::new(None)),
        file_index_dirty: Arc::new(AtomicBool::new(true)),
        content_building: Arc::new(AtomicBool::new(false)),
        def_building: Arc::new(AtomicBool::new(false)),
        watcher_generation: Arc::new(AtomicU64::new(0)),
        watch_enabled: false,
        watch_debounce_ms: 500,
        respect_git_exclude: false,
        watcher_stats: Arc::new(crate::mcp::watcher::WatcherStats::new()),
        periodic_rescan_enabled: false,
        rescan_interval_sec: 300,
        branch_name_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
        file_index_build_gate: Arc::new(crate::mcp::handlers::utils::FileIndexBuildGate::new()),
        autosave_dirty: Arc::new(AtomicBool::new(false)),
    };
    drop(_guard);
}

#[test]
fn test_handler_context_default_respect_git_exclude_false() {
    let ctx = HandlerContext::default();
    assert!(!ctx.respect_git_exclude,
        "HandlerContext::default() must set respect_git_exclude to false to match CLI/MCP defaults");
}

#[test]
fn test_handler_context_respect_git_exclude_settable() {
    // Ensures the field is publicly settable so serve.rs can initialize it
    // from ServeArgs.respect_git_exclude (guards against accidental private visibility).
    let mut ctx = HandlerContext::default();
    ctx.respect_git_exclude = true;
    assert!(ctx.respect_git_exclude);
}

/// Verify that Default creates correct values for test-critical fields.
#[test]
fn test_handler_context_default_values() {
    let ctx = HandlerContext::default();
    assert_eq!(ctx.server_dir(), ".");
    assert_eq!(ctx.server_ext, "cs");
    assert!(!ctx.metrics);
    assert_eq!(ctx.index_base, PathBuf::from("."));
    assert_eq!(ctx.max_response_bytes, crate::mcp::handlers::utils::DEFAULT_MAX_RESPONSE_BYTES);
    assert!(ctx.content_ready.load(std::sync::atomic::Ordering::Relaxed), "content_ready should default to true");
    assert!(ctx.def_ready.load(std::sync::atomic::Ordering::Relaxed), "def_ready should default to true");
    assert!(!ctx.git_cache_ready.load(std::sync::atomic::Ordering::Relaxed), "git_cache_ready should default to false");
    assert!(ctx.def_index.is_none());
    assert!(ctx.current_branch.is_none());
    assert!(ctx.def_extensions.is_empty(), "def_extensions should default to empty Vec");
    assert!(ctx.file_index.read().unwrap().is_none(), "file_index should default to None");
    assert!(ctx.file_index_dirty.load(std::sync::atomic::Ordering::Relaxed), "file_index_dirty should default to true");
}

fn make_empty_ctx() -> HandlerContext {
    HandlerContext::default()
}

#[test]
fn test_dispatch_unknown_tool() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "nonexistent_tool", &json!({}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("Unknown tool"));
}

#[test]
fn test_dispatch_grep_missing_terms() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("must contain at least one entry"));
}

#[test]
fn test_dispatch_grep_empty_index() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["HttpClient"]}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 0);
}

#[test]
fn test_dispatch_grep_unknown_arg_warning_in_summary() {
    // Default (no env): unknown args are surfaced as a warning in summary,
    // but the tool still runs successfully so existing scripts don't break.
    let _guard = STRICT_ARGS_ENV_LOCK.lock().unwrap();
    let ctx = make_empty_ctx();
    // SAFETY: serial single-threaded test; restored at end.
    unsafe { std::env::remove_var("XRAY_STRICT_ARGS") };
    let result = dispatch_tool(
        &ctx,
        "xray_grep",
        &json!({"terms": ["HttpClient"], "isRegexp": true}),
    );
    assert!(!result.is_error, "default mode should not hard-fail on unknown arg");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let summary = output["summary"].as_object().expect("summary present");
    let warning = summary
        .get("unknownArgsWarning")
        .and_then(|v| v.as_str())
        .expect("unknownArgsWarning should be set");
    assert!(warning.contains("'isRegexp'"), "warning should mention the bad key, got: {warning}");
    assert!(warning.contains("Use 'regex' instead"), "warning should suggest fix, got: {warning}");
}

#[test]
fn test_dispatch_grep_unknown_arg_strict_mode_hard_errors() {
    let _guard = STRICT_ARGS_ENV_LOCK.lock().unwrap();
    let ctx = make_empty_ctx();
    // SAFETY: serial single-threaded test; restored at end.
    unsafe { std::env::set_var("XRAY_STRICT_ARGS", "1") };
    let result = dispatch_tool(
        &ctx,
        "xray_grep",
        &json!({"terms": ["HttpClient"], "includePattern": "src/**"}),
    );
    unsafe { std::env::remove_var("XRAY_STRICT_ARGS") };
    assert!(result.is_error, "strict mode should hard-error on unknown arg");
    let body: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(body["error"], "UNKNOWN_ARGS");
    let unknown = body["unknownArgs"].as_array().unwrap();
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0]["key"], "includePattern");
}

use super::arg_validation::STRICT_ARGS_ENV_LOCK;

#[test]
fn test_dispatch_grep_with_results() {
    let mut idx = HashMap::new();
    idx.insert("httpclient".to_string(), vec![Posting {
        file_id: 0,
        lines: vec![5, 12],
    }]);
    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\test\\Program.cs".to_string()],
        index: idx,
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50],
        ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["HttpClient"], "substring": false}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1);
    assert_eq!(output["files"][0]["path"], "C:\\test\\Program.cs");
    assert_eq!(output["files"][0]["occurrences"], 2);
}

// --- xray_callers error tests (general) ---

#[test]
fn test_xray_callers_no_def_index() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({"method": ["Foo"]}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("Definition index not available"));
}

// --- xray_reindex_definitions tests ---

#[test]
fn test_reindex_definitions_no_def_index() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_reindex_definitions", &json!({}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("Definition index not available"));
}

#[test]
fn test_reindex_definitions_has_schema() {
    let tools = tool_definitions(&["cs".to_string()]);
    let tool = tools.iter().find(|t| t.name == "xray_reindex_definitions").unwrap();
    let props = tool.input_schema["properties"].as_object().unwrap();
    assert!(props.contains_key("dir"), "Should have dir parameter");
    assert!(props.contains_key("ext"), "Should have ext parameter");
}

// --- containsLine error test (general) ---

#[test]
fn test_contains_line_requires_file() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "containsLine": 391
    }));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("containsLine requires 'file' parameter"));
}

// --- xray_callers schema tests ---

#[test]
fn test_xray_callers_has_required_params() {
    let tools = tool_definitions(&["cs".to_string()]);
    let callers = tools.iter().find(|t| t.name == "xray_callers").unwrap();
    let required = callers.input_schema["required"].as_array().unwrap();
    assert_eq!(required.len(), 1);
    assert_eq!(required[0], "method");
}

#[test]
fn test_xray_callers_has_limit_params() {
    let tools = tool_definitions(&["cs".to_string()]);
    let callers = tools.iter().find(|t| t.name == "xray_callers").unwrap();
    let props = callers.input_schema["properties"].as_object().unwrap();
    assert!(props.contains_key("maxCallersPerLevel"), "Should have maxCallersPerLevel");
    assert!(props.contains_key("maxTotalNodes"), "Should have maxTotalNodes");
}

#[test]
fn test_xray_definitions_has_contains_line() {
    let tools = tool_definitions(&["cs".to_string()]);
    let defs = tools.iter().find(|t| t.name == "xray_definitions").unwrap();
    let props = defs.input_schema["properties"].as_object().unwrap();
    assert!(props.contains_key("containsLine"), "Should have containsLine parameter");
}

// --- maxResults=0 means unlimited tests ---

#[test]
fn test_xray_definitions_max_results_zero_means_unlimited() {
    let ctx = make_ctx_with_defs();
    // maxResults=0 should return ALL definitions, not cap at 100
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "maxResults": 0
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalResults"].as_u64().unwrap();
    let returned = output["definitions"].as_array().unwrap().len() as u64;
    assert!(total > 0, "Should have definitions in test context");
    assert_eq!(returned, total, "maxResults=0 should return ALL definitions (unlimited), got {}/{}", returned, total);
}

#[test]
fn test_xray_definitions_max_results_one_caps_output() {
    let ctx = make_ctx_with_defs();
    // Use name filter to bypass autoSummary (which triggers when results > maxResults without name filter)
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "maxResults": 1,
        "name": ["Service"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let returned = output["definitions"].as_array().unwrap().len();
    assert_eq!(returned, 1, "maxResults=1 with name filter should return exactly 1 definition");
}

#[test]
fn test_xray_definitions_max_results_default_is_100() {
    let ctx = make_ctx_with_defs();
    // When maxResults is omitted, default should be 100
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalResults"].as_u64().unwrap();
    let returned = output["definitions"].as_array().unwrap().len() as u64;
    // Our test context has fewer than 100 definitions, so returned == total
    assert_eq!(returned, total, "With default maxResults (100), should return all definitions when total < 100");
}
// ─── Async startup: index-building readiness tests ──────────────────

#[test]
fn test_dispatch_grep_while_content_index_building() {
    let ctx = HandlerContext {
        content_ready: Arc::new(AtomicBool::new(false)),
        ..make_empty_ctx()
    };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["foo"]}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("being built"),
        "Expected 'being built' message, got: {}", result.content[0].text);
}

#[test]
fn test_dispatch_definitions_while_def_index_building() {
    let ctx = HandlerContext {
        def_ready: Arc::new(AtomicBool::new(false)),
        def_index: Some(Arc::new(RwLock::new(crate::definitions::DefinitionIndex {
            root: ".".to_string(),
            created_at: 0,
            extensions: vec!["cs".to_string()],
            files: Vec::new(),
            definitions: Vec::new(),
            name_index: HashMap::new(),
            kind_index: HashMap::new(),
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index: HashMap::new(),
            path_to_id: HashMap::new(),
            method_calls: HashMap::new(),
            ..Default::default()
        }))),
        ..make_empty_ctx()
    };
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({"name": "Foo"}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("being built"),
        "Expected 'being built' message, got: {}", result.content[0].text);
}

#[test]
fn test_dispatch_callers_while_def_index_building() {
    let ctx = HandlerContext {
        def_ready: Arc::new(AtomicBool::new(false)),
        def_index: Some(Arc::new(RwLock::new(crate::definitions::DefinitionIndex {
            root: ".".to_string(),
            created_at: 0,
            extensions: vec!["cs".to_string()],
            files: Vec::new(),
            definitions: Vec::new(),
            name_index: HashMap::new(),
            kind_index: HashMap::new(),
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index: HashMap::new(),
            path_to_id: HashMap::new(),
            method_calls: HashMap::new(),
            ..Default::default()
        }))),
        ..make_empty_ctx()
    };
    let result = dispatch_tool(&ctx, "xray_callers", &json!({"method": ["Foo"]}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("being built"),
        "Expected 'being built' message, got: {}", result.content[0].text);
}

#[test]
fn test_dispatch_reindex_not_blocked_when_content_not_ready() {
    // After the pre-fix (Stage 0), xray_reindex is NOT blocked by content_ready=false.
    // It should proceed to execute (not return "already building").
    // This is critical for the workspace switch flow where content_ready=false
    // but no background build is running.
    let ctx = HandlerContext {
        content_ready: Arc::new(AtomicBool::new(false)),
        ..make_empty_ctx()
    };
    let result = dispatch_tool(&ctx, "xray_reindex", &json!({}));
    // Should succeed (not error), even though content_ready is false
    assert!(!result.is_error,
        "xray_reindex should NOT be blocked by content_ready=false, got: {}", result.content[0].text);
}

#[test]
fn test_dispatch_reindex_blocked_when_content_building() {
    // content_building=true means another build is in progress — should block.
    let ctx = HandlerContext {
        content_building: Arc::new(AtomicBool::new(true)),
        ..make_empty_ctx()
    };
    let result = dispatch_tool(&ctx, "xray_reindex", &json!({}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("already being built"),
        "Expected 'already being built' message, got: {}", result.content[0].text);
}

#[test]
fn test_dispatch_reindex_definitions_blocked_when_def_building() {
    // def_building=true means another build is in progress — should block.
    let ctx = HandlerContext {
        def_index: Some(Arc::new(RwLock::new(crate::definitions::DefinitionIndex::default()))),
        def_building: Arc::new(AtomicBool::new(true)),
        ..make_empty_ctx()
    };
    let result = dispatch_tool(&ctx, "xray_reindex_definitions", &json!({}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("already being built"),
        "Expected 'already being built' message, got: {}", result.content[0].text);
}

#[test]
fn test_reindex_sets_content_ready_true_after_completion() {
    // Regression test: when content_ready=false (e.g., after roots/list reset),
    // xray_reindex must set it back to true after loading the index.
    // Without this, xray_grep stays permanently blocked with "building" error.
    let ctx = HandlerContext {
        content_ready: Arc::new(AtomicBool::new(false)),
        ..make_empty_ctx()
    };
    assert!(!ctx.content_ready.load(std::sync::atomic::Ordering::Acquire));
    let result = dispatch_tool(&ctx, "xray_reindex", &json!({}));
    assert!(!result.is_error, "xray_reindex should succeed, got: {}", result.content[0].text);
    assert!(ctx.content_ready.load(std::sync::atomic::Ordering::Acquire),
        "content_ready must be true after xray_reindex completes");
}

#[test]
fn test_dispatch_fast_while_content_index_building() {
    // xray_fast uses its own file-list index, NOT the content index.
    // It should work even when content index is still building.
    let ctx = HandlerContext {
        content_ready: Arc::new(AtomicBool::new(false)),
        ..make_empty_ctx()
    };
    let result = dispatch_tool(&ctx, "xray_fast", &json!({"pattern": ["foo"]}));
    assert!(!result.is_error,
        "xray_fast should not be blocked by content_ready=false, got: {}",
        result.content[0].text);
}

#[test]
fn test_dispatch_help_works_while_index_building() {
    let ctx = HandlerContext {
        content_ready: Arc::new(AtomicBool::new(false)),
        def_ready: Arc::new(AtomicBool::new(false)),
        ..make_empty_ctx()
    };
    let result = dispatch_tool(&ctx, "xray_help", &json!({}));
    assert!(!result.is_error, "xray_help should work during index build");

    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error, "xray_info should work during index build");
}

