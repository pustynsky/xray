//! Tests for metrics, security, reindex, info, help, definitions ranking, input validation --
//! extracted from handlers_tests.rs.

use super::*;
use super::arg_validation::STRICT_ARGS_ENV_LOCK;

use super::utils::validate_search_dir;
use super::handlers_test_utils::{make_ctx_with_defs, make_empty_ctx};
use crate::Posting;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}


struct GuidancePrefixOverrideGuard {
    previous: Option<bool>,
}

impl GuidancePrefixOverrideGuard {
    fn set(value: Option<bool>) -> Self {
        Self {
            previous: utils::set_guidance_prefix_override_for_test(value),
        }
    }
}

impl Drop for GuidancePrefixOverrideGuard {
    fn drop(&mut self) {
        utils::set_guidance_prefix_override_for_test(self.previous);
    }
}


fn split_guidance_prefixed_response(text: &str) -> (&str, Value) {
    let (prefix, suffix) = text
        .split_once("\n\n")
        .expect("prefix mode should separate guidance and JSON suffix");
    let output = serde_json::from_str(suffix).expect("JSON suffix should parse");
    (prefix, output)
}

fn assert_json_without_guidance_prefix(text: &str) -> Value {
    assert!(!text.starts_with("=== XRAY AGENT GUIDANCE ==="));
    serde_json::from_str(text).expect("response should parse directly as JSON")
}

// --- Metrics injection tests ---

#[test] fn test_metrics_off_no_extra_fields() {
    let mut idx = HashMap::new();
    idx.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![5] }]);
    let index = ContentIndex { root: ".".to_string(), files: vec!["C:\\test\\Program.cs".to_string()], index: idx, total_tokens: 100, extensions: vec!["cs".to_string()], file_token_counts: vec![50], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), ..Default::default() };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["HttpClient"]}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["policyReminder"].as_str().is_some());
    assert!(output["summary"]["nextStepHint"].as_str().is_some());
    assert!(output["summary"].get("responseBytes").is_none());
    assert!(output["summary"].get("estimatedTokens").is_none());
}

#[test] fn test_metrics_on_injects_fields() {
    let mut idx = HashMap::new();
    idx.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![5] }]);
    let index = ContentIndex { root: ".".to_string(), files: vec!["C:\\test\\Program.cs".to_string()], index: idx, total_tokens: 100, extensions: vec!["cs".to_string()], file_token_counts: vec![50], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), metrics: true, ..Default::default() };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["HttpClient"]}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["policyReminder"].as_str().is_some());
    assert!(output["summary"]["nextStepHint"].as_str().is_some());
    assert!(output["summary"]["searchTimeMs"].as_f64().is_some());
    assert!(output["summary"]["responseBytes"].as_u64().is_some());
    assert!(output["summary"]["estimatedTokens"].as_u64().is_some());
}

#[test]
fn test_guidance_prefix_dispatch_runs_after_warnings_and_metrics() {
    let _guard = STRICT_ARGS_ENV_LOCK.lock().unwrap();
    let _prefix = GuidancePrefixOverrideGuard::set(Some(true));
    let _strict_env = EnvVarGuard::remove("XRAY_STRICT_ARGS");

    let mut idx = HashMap::new();
    idx.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![5] }]);
    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\test\\Program.cs".to_string()],
        index: idx,
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50],
        ..Default::default()
    };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), metrics: true, ..Default::default() };

    let result = dispatch_tool(
        &ctx,
        "xray_grep",
        &json!({"terms": ["HttpClient"], "includePattern": "*.cs"}),
    );
    assert!(!result.is_error);

    let text = &result.content[0].text;
    let (prefix, suffix) = text
        .split_once("\n\n")
        .expect("prefix mode should separate guidance and JSON suffix");
    assert!(prefix.starts_with("=== XRAY AGENT GUIDANCE ==="));
    assert!(prefix.contains("→ Next: use xray_definitions for AST structure or xray_callers for call trees"));
    assert!(prefix.contains("Use xray_* MCP tools for indexed files; built-in read/search/edit equivalents are policy violations."));

    let output: Value = serde_json::from_str(suffix).unwrap();
    let summary = output["summary"].as_object().unwrap();
    assert!(!summary.contains_key("nextStepHint"));
    assert!(!summary.contains_key("policyReminder"));
    assert!(summary["unknownArgsWarning"].as_str().unwrap().contains("includePattern"));
    assert_eq!(summary["unknownArgs"][0]["key"], "includePattern");
    assert!(summary["searchTimeMs"].as_f64().is_some());
    assert_eq!(summary["responseBytes"].as_u64().unwrap(), text.len() as u64);
    assert_eq!(summary["estimatedTokens"].as_u64().unwrap(), text.len() as u64 / 4);
}


#[test]
fn test_guidance_prefix_plain_text_error_dispatch_preserves_json_contract_and_warning() {
    let _guard = STRICT_ARGS_ENV_LOCK.lock().unwrap();
    let _strict_env = EnvVarGuard::remove("XRAY_STRICT_ARGS");
    let ctx = HandlerContext { metrics: true, ..make_empty_ctx() };

    {
        let _prefix = GuidancePrefixOverrideGuard::set(Some(false));
        let result = dispatch_tool(&ctx, "xray_grep", &json!({"includePattern": "*.cs"}));
        assert!(result.is_error);
        let output = assert_json_without_guidance_prefix(&result.content[0].text);
        assert!(output["error"].as_str().is_some());
        let summary = output["summary"].as_object().unwrap();
        assert!(summary["policyReminder"].as_str().is_some());
        assert!(summary["nextStepHint"].as_str().is_some());
        assert!(summary["unknownArgsWarning"].as_str().unwrap().contains("includePattern"));
        assert_eq!(summary["unknownArgs"][0]["key"], "includePattern");
    }

    {
        let _prefix = GuidancePrefixOverrideGuard::set(Some(true));
        let result = dispatch_tool(&ctx, "xray_grep", &json!({"includePattern": "*.cs"}));
        assert!(result.is_error);
        let (prefix, output) = split_guidance_prefixed_response(&result.content[0].text);
        assert!(prefix.starts_with("=== XRAY AGENT GUIDANCE ==="));
        assert!(prefix.contains("Use xray_* MCP tools for indexed files"));
        assert!(output["error"].as_str().is_some());
        let summary = output["summary"].as_object().unwrap();
        assert!(!summary.contains_key("policyReminder"));
        assert!(!summary.contains_key("nextStepHint"));
        assert!(summary["unknownArgsWarning"].as_str().unwrap().contains("includePattern"));
        assert_eq!(summary["unknownArgs"][0]["key"], "includePattern");
        assert!(summary["totalTimeMs"].as_f64().is_some());
    }
}

#[test]
fn test_guidance_prefix_applies_to_previously_excluded_tools_through_dispatch() {
    // 2026-04-30: previously these four tools short-circuited the guidance-prefix
    // pipeline and leaked summary.policyReminder into JSON. Contract is now
    // uniform: with XRAY_GUIDANCE_PREFIX=1 every JSON-producing tool moves
    // guidance fields into the raw prefix.
    let _guard = STRICT_ARGS_ENV_LOCK.lock().unwrap();
    let _prefix = GuidancePrefixOverrideGuard::set(Some(true));

    let cases = vec![
        ("xray_info", make_empty_ctx(), json!({})),
        ("xray_help", make_empty_ctx(), json!({})),
        ("xray_reindex", make_empty_ctx(), json!({ "ext": "rs" })),
        ("xray_reindex_definitions", make_ctx_with_defs(), json!({ "ext": "rs" })),
    ];

    for (tool_name, ctx, arguments) in cases {
        let result = dispatch_tool(&ctx, tool_name, &arguments);
        let text = &result.content[0].text;
        let (prefix, output) = split_guidance_prefixed_response(text);
        assert!(
            prefix.starts_with("=== XRAY AGENT GUIDANCE ==="),
            "{tool_name} must emit guidance prefix"
        );
        assert!(
            prefix.contains("Use xray_* MCP tools for indexed files"),
            "{tool_name} prefix must include compact policy guidance"
        );
        let summary = output["summary"].as_object().unwrap_or_else(|| {
            panic!("{tool_name} JSON suffix must keep a summary object")
        });
        assert!(
            !summary.contains_key("policyReminder"),
            "{tool_name} JSON suffix must not contain policyReminder"
        );
        assert!(
            !summary.contains_key("nextStepHint"),
            "{tool_name} JSON suffix must not contain nextStepHint"
        );
    }
}

#[test]
fn test_guidance_prefix_dispatch_after_truncation_recomputes_wire_metrics() {
    let _guard = STRICT_ARGS_ENV_LOCK.lock().unwrap();
    let _prefix = GuidancePrefixOverrideGuard::set(Some(true));

    let mut idx = HashMap::new();
    let mut files = Vec::new();
    let mut file_token_counts = Vec::new();
    for i in 0..100 {
        files.push(format!("C:\\Projects\\Module_{:03}\\Component_{:03}Service.cs", i / 10, i));
        file_token_counts.push(100u32);
        let lines: Vec<u32> = (1..=20).collect();
        idx.entry("targettoken".to_string())
            .or_insert_with(Vec::new)
            .push(Posting { file_id: i as u32, lines });
    }

    let index = ContentIndex {
        root: ".".to_string(),
        files,
        index: idx,
        total_tokens: 10_000,
        extensions: vec!["cs".to_string()],
        file_token_counts,
        ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        metrics: true,
        max_response_bytes: 2_000,
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_grep", &json!({
        "terms": ["targettoken"],
        "maxResults": 0,
        "substring": false
    }));
    assert!(!result.is_error);

    let text = &result.content[0].text;
    let (_prefix, output) = split_guidance_prefixed_response(text);
    let summary = output["summary"].as_object().unwrap();
    assert!(!summary.contains_key("policyReminder"));
    assert!(!summary.contains_key("nextStepHint"));
    assert_eq!(summary["responseTruncated"], true);
    assert!(summary["truncationReason"].as_str().is_some());
    assert_eq!(summary["responseBytes"].as_u64().unwrap(), text.len() as u64);
    assert_eq!(summary["estimatedTokens"].as_u64().unwrap(), text.len() as u64 / 4);
    assert!(output["files"].as_array().unwrap().len() < 100);
}


#[test]
fn test_metrics_preserves_handler_search_time() {
    // B4 fix: if handler already set searchTimeMs, inject_metrics should NOT overwrite it,
    // and should add totalTimeMs for overall dispatch time.
    let mut idx = HashMap::new();
    idx.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![5] }]);
    let index = ContentIndex { root: ".".to_string(), files: vec!["C:\\test\\Program.cs".to_string()], index: idx, total_tokens: 100, extensions: vec!["cs".to_string()], file_token_counts: vec![50], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), metrics: true, ..Default::default() };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["HttpClient"]}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    // searchTimeMs should be present (set by handler)
    let search_time = output["summary"]["searchTimeMs"].as_f64().unwrap();
    assert!(search_time >= 0.0, "searchTimeMs should be non-negative");
    // totalTimeMs should also be present (set by inject_metrics)
    let total_time = output["summary"]["totalTimeMs"].as_f64().unwrap();
    assert!(total_time >= search_time, "totalTimeMs should be >= searchTimeMs");
}

#[test]
fn test_error_response_has_guidance() {
    // A5 fix: error responses should include policyReminder and workspace metadata
    let index = ContentIndex { root: ".".to_string(), ..Default::default() };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(index)),
        server_ext: "rs".to_string(),
        ..Default::default()
    };
    // Call with missing required parameter to trigger an error
    let result = dispatch_tool(&ctx, "xray_grep", &json!({}));
    assert!(result.is_error, "Should be an error response");
    // Error response should still have guidance injected
    let text = &result.content[0].text;
    // inject_response_guidance works on JSON — if the error is JSON, it should have summary
    if let Ok(output) = serde_json::from_str::<Value>(text)
        && let Some(summary) = output.get("summary") {
            assert!(summary.get("policyReminder").is_some(),
                "Error JSON response should have policyReminder in summary");
        }
    // The key assertion: is_error must be preserved
    assert!(result.is_error, "is_error flag must be preserved after guidance injection");
}

#[test] fn test_metrics_injected_on_error() {
    // 2026-04-27: invariant change. Previously plain-text errors short-circuited
    // the metrics pipeline (callers couldn't tell "server spent N ms on CPU"
    // from "transport hang"). inject_response_guidance now wraps plain-text
    // errors into a JSON envelope `{"error": "...", "summary": {}}`, and the
    // dispatcher no longer short-circuits on `is_error`, so metrics + policy
    // reminder land on errors too.
    let ctx = make_empty_ctx();
    let ctx = HandlerContext { metrics: true, ..ctx };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({}));
    assert!(result.is_error, "is_error flag must be preserved through metrics injection");
    let text = &result.content[0].text;
    let output: Value = serde_json::from_str(text)
        .expect("plain-text error should be wrapped into JSON envelope");
    assert!(output["error"].as_str().is_some(), "envelope must carry the original error message under .error");
    let summary = &output["summary"];
    assert!(summary["totalTimeMs"].as_f64().is_some(), "totalTimeMs must be injected on error path");
    assert!(summary["policyReminder"].as_str().is_some(), "policyReminder must be injected on error path");
    assert!(summary["serverDir"].as_str().is_some(), "workspace metadata must be injected on error path");
}

#[test] fn test_metrics_injected_on_xray_edit_error() {
    // Per-tool variant: confirms the error-pipeline contract holds for xray_edit
    // specifically (not just xray_grep). xray_edit is the tool whose 5-10 min
    // hang on a 140KB file motivated the fix.
    let ctx = make_empty_ctx();
    let ctx = HandlerContext { metrics: true, ..ctx };
    let result = dispatch_tool(&ctx, "xray_edit", &json!({}));
    assert!(result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text)
        .expect("xray_edit plain-text error must be wrapped into JSON envelope");
    assert!(output["error"].as_str().is_some());
    assert!(output["summary"]["totalTimeMs"].as_f64().is_some());
    assert!(output["summary"]["policyReminder"].as_str().is_some());
}

#[test] fn test_metrics_injected_on_xray_definitions_error() {
    // Per-tool variant: xray_definitions exercises a different handler path.
    let ctx = make_empty_ctx();
    let ctx = HandlerContext { metrics: true, ..ctx };
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({}));
    assert!(result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text)
        .expect("xray_definitions plain-text error must be wrapped into JSON envelope");
    assert!(output["error"].as_str().is_some());
    assert!(output["summary"]["totalTimeMs"].as_f64().is_some());
    assert!(output["summary"]["policyReminder"].as_str().is_some());
}

#[test]
fn test_xray_help_has_policy_but_no_next_step_hint() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_help", &json!({}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["policyReminder"].as_str().is_some());
    assert!(output["summary"].get("nextStepHint").is_none());
}

#[test]
fn test_xray_info_has_policy_but_no_next_step_hint() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["policyReminder"].as_str().is_some());
    assert!(output["summary"].get("nextStepHint").is_none());
}

#[test]
fn test_xray_reindex_has_policy_but_no_next_step_hint() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_reindex", &json!({}));
    // xray_reindex with empty ctx may error (no dir), but if it returns success JSON, verify guidance
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(output["summary"]["policyReminder"].as_str().is_some());
        assert!(output["summary"].get("nextStepHint").is_none());
    }
}

#[test]
fn test_xray_reindex_definitions_has_policy_but_no_next_step_hint() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_reindex_definitions", &json!({}));
    // xray_reindex_definitions with empty ctx may error (no def_index), but if it returns success JSON, verify guidance
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(output["summary"]["policyReminder"].as_str().is_some());
        assert!(output["summary"].get("nextStepHint").is_none());
    }
}

#[test] fn test_metrics_search_time_is_positive() {
    let mut idx = HashMap::new();
    idx.insert("foo".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    let index = ContentIndex { root: ".".to_string(), files: vec!["test.cs".to_string()], index: idx, total_tokens: 10, extensions: vec!["cs".to_string()], file_token_counts: vec![10], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), metrics: true, ..Default::default() };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["foo"]}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["searchTimeMs"].as_f64().unwrap() >= 0.0);
}

// --- Subdir tests ---

#[test] fn test_validate_search_dir_subdirectory() {
    let parent_tmp = tempfile::tempdir().unwrap();
    let tmp = parent_tmp.path().join("subdir_val");
    std::fs::create_dir_all(&tmp).unwrap();
    let result = validate_search_dir(&tmp.to_string_lossy(), &parent_tmp.path().to_string_lossy());
    assert!(result.is_ok());
    assert!(result.unwrap().is_some());
}
// ─── General xray_definitions tests ───────────────────────────────

/// xray_definitions non-existent name returns empty.
#[test]
fn test_xray_definitions_nonexistent_name_returns_empty() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["CompletelyNonExistentDefinitionXYZ123"]
    }));
    assert!(!result.is_error, "Non-existent name should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert!(defs.is_empty(),
        "Expected empty definitions array for non-existent name, got {} results", defs.len());
    assert_eq!(output["summary"]["totalResults"], 0);
}

/// xray_definitions invalid regex error.
#[test]
fn test_xray_definitions_invalid_regex_error() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["[invalid"],
        "regex": true
    }));
    assert!(result.is_error, "Invalid regex should produce an error");
    assert!(result.content[0].text.contains("Invalid regex"),
        "Error should mention 'Invalid regex', got: {}", result.content[0].text);
}
/// T77 — xray_definitions file filter: backslash vs forward slash normalization.
#[test]
fn test_xray_definitions_file_filter_slash_normalization() {
    use crate::definitions::*;

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\Models\\User.cs".to_string(),
            "C:\\src\\Services\\UserService.cs".to_string(),
        ],
        total_tokens: 50,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![25, 25],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserModel".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\Models\\User.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\Services\\UserService.cs"), 1);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\Models\\User.cs".to_string(),
            "C:\\src\\Services\\UserService.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    let result_backslash = dispatch_tool(&ctx, "xray_definitions", &json!({
        "file": ["Models\\User"]
    }));
    assert!(!result_backslash.is_error);
    let output_bs: Value = serde_json::from_str(&result_backslash.content[0].text).unwrap();
    let defs_bs = output_bs["definitions"].as_array().unwrap();

    let result_fwdslash = dispatch_tool(&ctx, "xray_definitions", &json!({
        "file": ["Models/User"]
    }));
    assert!(!result_fwdslash.is_error);
    let output_fs: Value = serde_json::from_str(&result_fwdslash.content[0].text).unwrap();
    let defs_fs = output_fs["definitions"].as_array().unwrap();

    assert_eq!(defs_bs.len(), 1,
        "Backslash file filter should find UserModel, got {} results", defs_bs.len());
    assert_eq!(defs_bs[0]["name"], "UserModel");

    if defs_fs.is_empty() {
        assert_eq!(defs_fs.len(), 0,
            "Forward slash filter currently does not match backslash paths (no normalization)");
    } else {
        assert_eq!(defs_fs.len(), defs_bs.len(),
            "If slash normalization exists, both filters should return same count");
    }

    let result_fragment = dispatch_tool(&ctx, "xray_definitions", &json!({
        "file": ["User"]
    }));
    assert!(!result_fragment.is_error);
    let output_frag: Value = serde_json::from_str(&result_fragment.content[0].text).unwrap();
    let defs_frag = output_frag["definitions"].as_array().unwrap();
    assert_eq!(defs_frag.len(), 2,
        "File filter 'User' should match both User.cs and UserService.cs, got {}", defs_frag.len());
}

/// T80 — xray_reindex with invalid/non-existent directory.
#[test]
fn test_xray_reindex_invalid_directory() {
    let ctx = make_empty_ctx();

    let result = dispatch_tool(&ctx, "xray_reindex", &json!({
        "dir": "Z:\\nonexistent\\path\\that\\does\\not\\exist"
    }));

    assert!(result.is_error, "Reindex with non-existent dir should error");
    let error_text = &result.content[0].text;
    assert!(
        error_text.contains("Server started with") || error_text.contains("not exist") || error_text.contains("error"),
        "Error should explain the issue. Got: {}", error_text
    );
}
// ─── validate_search_dir security boundary tests ────────────────────

#[test]
fn test_validate_search_dir_subdir_accepted() {
    // Create a real temp directory structure so canonicalize works
    let base = std::env::temp_dir().join(format!("search_sec_base_{}_{}", std::process::id(),
        std::sync::atomic::AtomicU64::new(0).fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let sub = base.join("subdir");
    std::fs::create_dir_all(&sub).unwrap();

    let result = validate_search_dir(
        &sub.to_string_lossy(),
        &base.to_string_lossy(),
    );
    assert!(result.is_ok(), "Subdirectory should be accepted, got: {:?}", result);
    assert!(result.unwrap().is_some(), "Subdirectory should return Some(canonical_subdir)");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_validate_search_dir_outside_rejected() {
    // Two sibling directories — neither is a subdirectory of the other
    let parent = std::env::temp_dir().join(format!("search_sec_outside_{}", std::process::id()));
    let dir_a = parent.join("dir_a");
    let dir_b = parent.join("dir_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let result = validate_search_dir(
        &dir_b.to_string_lossy(),
        &dir_a.to_string_lossy(),
    );
    assert!(result.is_err(), "Path outside server dir should be rejected");
    assert!(result.unwrap_err().contains("--dir"),
        "Error message should mention --dir");

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn test_validate_search_dir_path_traversal_rejected() {
    // Create base/subdir, then try to access base/subdir/../../.. which escapes base
    let base = std::env::temp_dir().join(format!("search_sec_traversal_{}", std::process::id()));
    let sub = base.join("subdir");
    std::fs::create_dir_all(&sub).unwrap();

    // Path traversal: subdir/../../.. resolves above base
    let traversal = sub.join("..").join("..").join("..");
    let result = validate_search_dir(
        &traversal.to_string_lossy(),
        &base.to_string_lossy(),
    );
    assert!(result.is_err(),
        "Path traversal escaping base dir should be rejected, got: {:?}", result);

    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(windows)]
#[test]
fn test_validate_search_dir_windows_absolute_outside_rejected() {
    // Non-existent absolute path that clearly isn't under the server dir
    // canonicalize will fail, falling back to raw string comparison.
    // Windows-only: drive-letter paths are only absolute on Windows.
    let result = validate_search_dir(
        r"C:\Windows\System32",
        r"C:\Repos\MyProject",
    );
    assert!(result.is_err(),
        "Absolute path outside server dir should be rejected");
}

// ─── xray_help response structure tests ────────────────────────────

#[test]
fn test_xray_help_response_structure() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_help", &json!({}));
    assert!(!result.is_error, "xray_help should not error");

    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Validate top-level keys exist (from tips::render_json)
    assert!(output["bestPractices"].is_array(), "Response should have 'bestPractices' array");
    assert!(output["strategyRecipes"].is_array(), "Response should have 'strategyRecipes' array");
    assert!(output["performanceTiers"].is_object(), "Response should have 'performanceTiers' object");
    assert!(output["toolPriority"].is_array(), "Response should have 'toolPriority' array");

    // bestPractices should be non-empty and each entry should have rule/why/example
    let practices = output["bestPractices"].as_array().unwrap();
    assert!(!practices.is_empty(), "bestPractices should not be empty");
    for practice in practices {
        assert!(practice["rule"].is_string(), "Each practice should have 'rule'");
        assert!(practice["why"].is_string(), "Each practice should have 'why'");
        assert!(practice["example"].is_string(), "Each practice should have 'example'");
    }

    // strategyRecipes should be non-empty and each entry should have name/when/steps/antiPatterns
    let recipes = output["strategyRecipes"].as_array().unwrap();
    assert!(!recipes.is_empty(), "strategyRecipes should not be empty");
    for recipe in recipes {
        assert!(recipe["name"].is_string(), "Each recipe should have 'name'");
        assert!(recipe["when"].is_string(), "Each recipe should have 'when'");
        assert!(recipe["steps"].is_array(), "Each recipe should have 'steps'");
        assert!(recipe["antiPatterns"].is_array(), "Each recipe should have 'antiPatterns'");
    }

    // performanceTiers should have entries
    let tiers = output["performanceTiers"].as_object().unwrap();
    assert!(!tiers.is_empty(), "performanceTiers should not be empty");

    // toolPriority should be non-empty
    let priority = output["toolPriority"].as_array().unwrap();
    assert!(!priority.is_empty(), "toolPriority should not be empty");

    // Verify counts match the source of truth
    assert_eq!(practices.len(), crate::tips::tips(&[]).len(),
        "bestPractices count should match tips::tips()");
    assert_eq!(recipes.len(), crate::tips::strategies().len(),
        "strategyRecipes count should match tips::strategies()");
    assert_eq!(priority.len(), crate::tips::tool_priority(&[]).len(),
        "toolPriority count should match tips::tool_priority()");
}

// ─── xray_info response structure tests ────────────────────────────

#[test]
fn test_xray_info_exposes_watcher_stats_when_watch_enabled() {
    // Phase 0 of periodic-rescan rollout: when --watch is on, xray_info
    // must surface lock-free counters that operators use to diagnose
    // missed fs events (notify backend drops).
    let mut ctx = make_empty_ctx();
    ctx.watch_enabled = true;
    ctx.watcher_stats
        .events_total
        .store(7, std::sync::atomic::Ordering::Relaxed);
    ctx.watcher_stats
        .events_empty_paths
        .store(2, std::sync::atomic::Ordering::Relaxed);
    ctx.watcher_stats
        .events_errors
        .store(1, std::sync::atomic::Ordering::Relaxed);

    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error, "xray_info should not error");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let watcher = output.get("watcher").expect("watcher block must be present when watch_enabled=true");
    assert_eq!(watcher["eventsTotal"], json!(7));
    assert_eq!(watcher["eventsEmptyPaths"], json!(2));
    assert_eq!(watcher["eventsErrors"], json!(1));
}

#[test]
fn test_xray_info_omits_watcher_stats_when_watch_disabled() {
    // Symmetric guard: when --watch is off, the watcher block is omitted
    // entirely so that consumers don't mistake "0 events" for "watcher
    // running but never received anything" (a real bug signal).
    let ctx = make_empty_ctx();
    assert!(!ctx.watch_enabled, "precondition: default ctx has watch disabled");
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output.get("watcher").is_none(),
        "watcher block must be absent when watch_enabled=false");
}

#[test]
fn test_xray_info_response_structure() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error, "xray_info should not error");

    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Validate top-level keys exist (from cli::info::cmd_info_json)
    assert!(output["directory"].is_string(), "Response should have 'directory' string");
    assert!(output["indexes"].is_array(), "Response should have 'indexes' array");

    // indexes is an array (may be empty if no indexes exist, which is fine for test)
    let indexes = output["indexes"].as_array().unwrap();

    // If indexes exist, validate their structure
    for idx in indexes {
        assert!(idx["type"].is_string(), "Each index should have a 'type' field");
        let idx_type = idx["type"].as_str().unwrap();
        match idx_type {
            "file" => {
                assert!(idx["root"].is_string(), "File index should have 'root'");
                assert!(idx["entries"].is_number(), "File index should have 'entries'");
                assert!(idx["sizeMb"].is_number(), "File index should have 'sizeMb'");
                assert!(idx["ageHours"].is_number(), "File index should have 'ageHours'");
            }
            "content" => {
                assert!(idx["root"].is_string(), "Content index should have 'root'");
                assert!(idx["files"].is_number(), "Content index should have 'files'");
                assert!(idx["totalTokens"].is_number(), "Content index should have 'totalTokens'");
                assert!(idx["extensions"].is_array(), "Content index should have 'extensions'");
                assert!(idx["sizeMb"].is_number(), "Content index should have 'sizeMb'");
                assert!(idx["ageHours"].is_number(), "Content index should have 'ageHours'");
            }
            "definition" => {
                assert!(idx["root"].is_string(), "Definition index should have 'root'");
                assert!(idx["files"].is_number(), "Definition index should have 'files'");
                assert!(idx["definitions"].is_number(), "Definition index should have 'definitions'");
                assert!(idx["callSites"].is_number(), "Definition index should have 'callSites'");
                assert!(idx["extensions"].is_array(), "Definition index should have 'extensions'");
                assert!(idx["sizeMb"].is_number(), "Definition index should have 'sizeMb'");
                assert!(idx["ageHours"].is_number(), "Definition index should have 'ageHours'");
            }
            "git-history" => {
                assert!(idx["commits"].is_number(), "Git history should have 'commits'");
                assert!(idx["files"].is_number(), "Git history should have 'files'");
                assert!(idx["authors"].is_number(), "Git history should have 'authors'");
                assert!(idx["headHash"].is_string(), "Git history should have 'headHash'");
                assert!(idx["branch"].is_string(), "Git history should have 'branch'");
                assert!(idx["sizeMb"].is_number(), "Git history should have 'sizeMb'");
                assert!(idx["ageHours"].is_number(), "Git history should have 'ageHours'");
            }
            "file-list" => {
                // File-list indexes may be left over from other tests (e.g. xray_fast).
                // MCP handler only emits root + sizeMb (no full deserialization).
                assert!(idx["root"].is_string(), "File-list index should have 'root'");
                assert!(idx["sizeMb"].is_number(), "File-list index should have 'sizeMb'");
            }
            other => panic!("Unexpected index type: {}", other),
        }
    }
}
// ─── xray_info file=[X] per-file metadata mode (§2.3 of
// todo_2026-04-25_xray-edit-append-and-line-staleness.md) ────────────
//
// `xray_info file=["path", ...]` MUST return per-file metadata
// (lineCount/byteSize/extension/indexed/lineEnding) without putting file
// content into the response. lineCount MUST agree with `xray_edit`'s
// `originalLineCount`/`newLineCount` so the round-trip
// `xray_info` -> `xray_edit` works without off-by-one.
//
// Notes for future maintainers:
// - `make_info_ctx` pins the workspace to a tmp dir AND seeds `server_ext`
//   so the `indexed` flag is computed correctly without bringing up a real
//   ContentIndex.
// - Tests intentionally exercise both LF and CRLF inputs, plus a
//   relative-path call, plus the security boundary check (out-of-workspace
//   path returns a per-file `error`, not a top-level error).

fn make_info_ctx(dir: &std::path::Path, server_ext: &str) -> HandlerContext {
    HandlerContext {
        workspace: Arc::new(RwLock::new(
            WorkspaceBinding::pinned(dir.to_string_lossy().to_string()),
        )),
        server_ext: server_ext.to_string(),
        ..HandlerContext::default()
    }
}

#[test]
fn test_xray_info_file_mode_returns_metadata_lf() {
    let tmp = tempfile::tempdir().unwrap();
    // 3 logical lines (a/B/c), each terminated by LF — the trailing newline
    // is a terminator, not a 4th line. count_lines_for_info MUST return 3,
    // matching xray_edit's count_lines.
    std::fs::write(tmp.path().join("sample.rs"), b"a\nB\nc\n").unwrap();

    let ctx = make_info_ctx(tmp.path(), "rs,md");
    let result = dispatch_tool(
        &ctx,
        "xray_info",
        &json!({ "file": ["sample.rs"] }),
    );
    assert!(!result.is_error, "xray_info file=[..] must succeed: {}", result.content[0].text);

    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = out["files"].as_array().expect("files array");
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!(f["path"].as_str().unwrap(), "sample.rs");
    assert_eq!(f["lineCount"].as_u64().unwrap(), 3,
        "3 LF-terminated lines must yield lineCount=3 (matches xray_edit count_lines)");
    assert_eq!(f["byteSize"].as_u64().unwrap(), 6);
    assert_eq!(f["extension"].as_str().unwrap(), "rs");
    assert!(f["indexed"].as_bool().unwrap(), "`rs` is in server_ext, must be indexed=true");
    assert_eq!(f["lineEnding"].as_str().unwrap(), "LF");
    assert!(f.get("error").is_none(), "successful entry must NOT carry `error`");
    // The whole point of this mode: NO file content in the response.
    assert!(f.get("content").is_none(), "file content must not leak into xray_info response");
    assert_eq!(out["summary"]["requested"].as_u64().unwrap(), 1);
    assert_eq!(out["summary"]["returned"].as_u64().unwrap(), 1);
}

#[test]
fn test_xray_info_file_mode_detects_crlf_line_ending() {
    let tmp = tempfile::tempdir().unwrap();
    // CRLF input mirrors a Windows-edited source file. lineEnding must be
    // reported as "CRLF" so callers can pre-empt the line-ending mismatch
    // class of bugs without reading the file themselves.
    std::fs::write(tmp.path().join("win.rs"), b"line1\r\nline2\r\n").unwrap();

    let ctx = make_info_ctx(tmp.path(), "rs");
    let result = dispatch_tool(&ctx, "xray_info", &json!({ "file": ["win.rs"] }));
    assert!(!result.is_error, "{}", result.content[0].text);

    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let f = &out["files"][0];
    assert_eq!(f["lineCount"].as_u64().unwrap(), 2,
        "CRLF count_lines: 'line1\\r\\nline2\\r\\n' is 2 lines");
    assert_eq!(f["lineEnding"].as_str().unwrap(), "CRLF");
    assert_eq!(f["byteSize"].as_u64().unwrap(), 14);
}

#[test]
fn test_xray_info_file_mode_detects_crlf_in_utf16le_with_bom() {
    // UTF-16LE on disk: BOM `FF FE`, then code units in little-endian.
    // CRLF as code units is `000D 000A` => bytes `0D 00 0A 00`. The 0x0A
    // byte is preceded by 0x00, NOT 0x0D — a byte-only scan misclassifies
    // this as LF. lineEnding must be reported as CRLF so the contract
    // ("reflects the file as stored on disk") holds for UTF-16 inputs the
    // metadata mode supports via read_file_lossy.
    let tmp = tempfile::tempdir().unwrap();
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE];
    for ch in "a\r\nb\r\n".encode_utf16() {
        bytes.extend_from_slice(&ch.to_le_bytes());
    }
    std::fs::write(tmp.path().join("u16le.txt"), &bytes).unwrap();

    let ctx = make_info_ctx(tmp.path(), "txt");
    let result = dispatch_tool(&ctx, "xray_info", &json!({ "file": ["u16le.txt"] }));
    assert!(!result.is_error, "{}", result.content[0].text);

    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let f = &out["files"][0];
    assert_eq!(f["lineEnding"].as_str().unwrap(), "CRLF",
        "UTF-16LE CRLF must be detected at code-unit granularity, not byte granularity");
}

#[test]
fn test_xray_info_file_mode_detects_crlf_in_utf16be_with_bom() {
    // UTF-16BE on disk: BOM `FE FF`, then code units in big-endian.
    // CRLF as code units is `000D 000A` => bytes `00 0D 00 0A`. The 0x0A
    // byte is preceded by 0x00, NOT 0x0D — same misclassification trap as
    // UTF-16LE but with the opposite byte order.
    let tmp = tempfile::tempdir().unwrap();
    let mut bytes: Vec<u8> = vec![0xFE, 0xFF];
    for ch in "a\r\nb\r\n".encode_utf16() {
        bytes.extend_from_slice(&ch.to_be_bytes());
    }
    std::fs::write(tmp.path().join("u16be.txt"), &bytes).unwrap();

    let ctx = make_info_ctx(tmp.path(), "txt");
    let result = dispatch_tool(&ctx, "xray_info", &json!({ "file": ["u16be.txt"] }));
    assert!(!result.is_error, "{}", result.content[0].text);

    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let f = &out["files"][0];
    assert_eq!(f["lineEnding"].as_str().unwrap(), "CRLF",
        "UTF-16BE CRLF must be detected at code-unit granularity, not byte granularity");
}

#[test]
fn test_xray_info_file_mode_indexed_false_for_unknown_extension() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("data.bin"), b"abc").unwrap();

    let ctx = make_info_ctx(tmp.path(), "rs,md");
    let result = dispatch_tool(&ctx, "xray_info", &json!({ "file": ["data.bin"] }));
    assert!(!result.is_error, "{}", result.content[0].text);

    let f: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let entry = &f["files"][0];
    assert_eq!(entry["extension"].as_str().unwrap(), "bin");
    assert!(!entry["indexed"].as_bool().unwrap(),
        "`bin` is NOT in server_ext, must be indexed=false");
    assert_eq!(entry["lineEnding"].as_str().unwrap(), "NONE",
        "file with no newline byte must report lineEnding=NONE");
    assert_eq!(entry["lineCount"].as_u64().unwrap(), 1,
        "non-empty file with no trailing LF still has 1 logical line");
}

#[test]
fn test_xray_info_file_mode_handles_empty_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("empty.rs"), b"").unwrap();

    let ctx = make_info_ctx(tmp.path(), "rs");
    let result = dispatch_tool(&ctx, "xray_info", &json!({ "file": ["empty.rs"] }));
    assert!(!result.is_error, "{}", result.content[0].text);

    let f: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let entry = &f["files"][0];
    // Matches xray_edit's contract: empty file -> count_lines = 0,
    // which is the value `expectedLineCount` should carry on the next
    // INSERT-into-empty-file edit.
    assert_eq!(entry["lineCount"].as_u64().unwrap(), 0);
    assert_eq!(entry["byteSize"].as_u64().unwrap(), 0);
}

#[test]
fn test_xray_info_file_mode_rejects_path_outside_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("escape.rs");
    std::fs::write(&outside_file, b"x\n").unwrap();

    let ctx = make_info_ctx(workspace.path(), "rs");
    let result = dispatch_tool(
        &ctx,
        "xray_info",
        &json!({ "file": [outside_file.to_string_lossy()] }),
    );
    // Top-level call must NOT error — per-file errors are returned in-band
    // so a batch with one bad path still surfaces metadata for the rest.
    assert!(!result.is_error, "per-file errors must be in-band, not top-level: {}", result.content[0].text);

    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let entry = &out["files"][0];
    assert!(entry.get("error").is_some(),
        "out-of-workspace path must surface an `error` field, got: {:?}", entry);
    let msg = entry["error"].as_str().unwrap();
    assert!(msg.contains("outside the workspace"),
        "error must mention workspace boundary, got: {}", msg);
    // Crucially: NO byteSize/lineCount, the file was never read.
    assert!(entry.get("lineCount").is_none());
}

#[test]
fn test_xray_info_file_mode_reports_missing_file_per_entry() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("present.rs"), b"only\n").unwrap();

    let ctx = make_info_ctx(tmp.path(), "rs");
    let result = dispatch_tool(
        &ctx,
        "xray_info",
        &json!({ "file": ["present.rs", "missing.rs"] }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);

    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = out["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "batch must return one entry per requested path");
    // Order must be preserved so callers can correlate by index.
    assert_eq!(files[0]["path"].as_str().unwrap(), "present.rs");
    assert!(files[0].get("error").is_none());
    assert_eq!(files[0]["lineCount"].as_u64().unwrap(), 1);
    assert_eq!(files[1]["path"].as_str().unwrap(), "missing.rs");
    assert!(files[1].get("error").is_some(), "missing file must carry `error`");
    assert!(files[1].get("lineCount").is_none());
}

#[test]
fn test_xray_info_no_file_arg_preserves_index_summary_mode() {
    // Regression guard: passing no `file` argument must keep the existing
    // index-summary behaviour. Without this, the dispatch wrapper added
    // for §2.3 could accidentally route into the per-file path on an empty
    // array.
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error, "{}", result.content[0].text);
    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(out.get("indexes").is_some(),
        "index-summary mode must still emit `indexes`");
    assert!(out.get("files").is_none(),
        "index-summary mode must NOT emit per-file `files`");

    // Same expectation when `file` is present but empty (read_string_array
    // skips entirely-whitespace entries; this is the "functionally empty" form).
    let result = dispatch_tool(&ctx, "xray_info", &json!({ "file": [] }));
    assert!(!result.is_error);
    let out: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(out.get("indexes").is_some(),
        "empty `file` array must fall through to index-summary mode");
}



// ═══════════════════════════════════════════════════════════════════════
// Relevance Ranking tests
// ═══════════════════════════════════════════════════════════════════════

/// Helper: create a context with definitions for ranking tests.
/// Contains: UserService (class), UserServiceFactory (class), UserServiceHelper (method).
fn make_ranking_defs_ctx() -> HandlerContext {
    use crate::definitions::*;

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\UserService.cs".to_string(),
            "C:\\src\\UserServiceFactory.cs".to_string(),
            "C:\\src\\Helpers.cs".to_string(),
        ],
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 30, 20],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "UserServiceFactory".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "UserServiceHelper".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 30,
            parent: Some("Helpers".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "IUserService".to_string(),
            kind: DefinitionKind::Interface, line_start: 1, line_end: 20,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let path_to_id: HashMap<PathBuf, u32> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\UserService.cs".to_string(),
            "C:\\src\\UserServiceFactory.cs".to_string(),
            "C:\\src\\Helpers.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    }
}

/// xray_definitions ranking: exact match class comes first, then prefix matches,
/// with shorter names before longer. Type-level defs sort before member-level.
#[test]
fn test_xray_definitions_ranking_exact_first() {
    let ctx = make_ranking_defs_ctx();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["UserService"]
    }));
    assert!(!result.is_error, "xray_definitions should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    assert!(defs.len() >= 3, "Should find at least 3 definitions containing 'UserService', got {}", defs.len());

    // First result should be exact match "UserService" (class, tier 0)
    assert_eq!(defs[0]["name"], "UserService",
        "First result should be exact match 'UserService', got '{}'", defs[0]["name"]);
    assert_eq!(defs[0]["kind"], "class",
        "Exact match should be the class definition");
}

/// xray_definitions ranking: prefix matches come before contains matches.
#[test]
fn test_xray_definitions_ranking_prefix_before_contains() {
    let ctx = make_ranking_defs_ctx();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["UserService"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Collect names in order
    let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();

    // "IUserService" is a contains-only match (tier 2) — should be after
    // "UserServiceFactory" and "UserServiceHelper" which are prefix matches (tier 1)
    let iuser_pos = names.iter().position(|n| *n == "IUserService");
    let factory_pos = names.iter().position(|n| *n == "UserServiceFactory");
    let helper_pos = names.iter().position(|n| *n == "UserServiceHelper");

    if let (Some(iuser), Some(factory)) = (iuser_pos, factory_pos) {
        assert!(factory < iuser,
            "Prefix match 'UserServiceFactory' (pos {}) should come before contains match 'IUserService' (pos {})",
            factory, iuser);
    }
    if let (Some(iuser), Some(helper)) = (iuser_pos, helper_pos) {
        assert!(helper < iuser,
            "Prefix match 'UserServiceHelper' (pos {}) should come before contains match 'IUserService' (pos {})",
            helper, iuser);
    }
}

/// xray_definitions ranking: among prefix matches, type-level (class) sorts before
/// member-level (method), and shorter names before longer for same kind priority.
#[test]
fn test_xray_definitions_ranking_kind_and_length_tiebreak() {
    let ctx = make_ranking_defs_ctx();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["UserService"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();

    // Among prefix matches (tier 1): "UserServiceFactory" (class, priority 0)
    // and "UserServiceHelper" (method, priority 1)
    // Class should come before method.
    let factory_pos = names.iter().position(|n| *n == "UserServiceFactory");
    let helper_pos = names.iter().position(|n| *n == "UserServiceHelper");

    if let (Some(factory), Some(helper)) = (factory_pos, helper_pos) {
        assert!(factory < helper,
            "Class 'UserServiceFactory' (pos {}) should sort before method 'UserServiceHelper' (pos {}) due to kind priority",
            factory, helper);
    }
}

/// xray_definitions ranking: regex mode should NOT apply relevance ranking.
#[test]
fn test_xray_definitions_ranking_not_applied_with_regex() {
    let ctx = make_ranking_defs_ctx();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["UserService.*"],
        "regex": true
    }));
    assert!(!result.is_error, "regex search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "Should find definitions matching regex");
    // We don't assert specific order since regex mode uses default order (no ranking)
}
// ═══════════════════════════════════════════════════════════════════════
// Input validation bug fix tests (BUG-1 through BUG-6)
// ═══════════════════════════════════════════════════════════════════════

/// BUG-1: xray_definitions with name=[] should behave like no name filter (return all).
/// Pre-2026-04-25 the equivalent was `name=""`; after the array migration the
/// "absent / no filter" sentinel is the empty array.
#[test]
fn test_xray_definitions_empty_name_treated_as_no_filter() {
    let ctx = make_ctx_with_defs();
    // With name=[] — should return all definitions (empty array ignored)
    let result_empty = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": [],
        "maxResults": 5
    }));
    assert!(!result_empty.is_error, "name=[] should not error: {}", result_empty.content[0].text);
    let output_empty: Value = serde_json::from_str(&result_empty.content[0].text).unwrap();
    let count_empty = output_empty["summary"]["totalResults"].as_u64().unwrap();

    // Without name — should return all definitions
    let result_no_name = dispatch_tool(&ctx, "xray_definitions", &json!({
        "maxResults": 5
    }));
    let output_no_name: Value = serde_json::from_str(&result_no_name.content[0].text).unwrap();
    let count_no_name = output_no_name["summary"]["totalResults"].as_u64().unwrap();

    assert_eq!(count_empty, count_no_name,
        "name=[] should behave like no name filter. Got {} vs {} results",
        count_empty, count_no_name);
    assert!(count_empty > 0, "Should have some definitions in test context");
}

/// BUG-2: xray_definitions with containsLine=-1 should return error.
#[test]
fn test_xray_definitions_contains_line_negative_returns_error() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "file": ["QueryService"],
        "containsLine": -1
    }));
    assert!(result.is_error, "containsLine=-1 should return an error");
    assert!(result.content[0].text.contains("containsLine must be >= 1"),
        "Error should mention 'containsLine must be >= 1', got: {}", result.content[0].text);
}

/// BUG-2: xray_definitions with containsLine=0 should return error.
#[test]
fn test_xray_definitions_contains_line_zero_returns_error() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "file": ["QueryService"],
        "containsLine": 0
    }));
    assert!(result.is_error, "containsLine=0 should return an error");
    assert!(result.content[0].text.contains("containsLine must be >= 1"),
        "Error should mention 'containsLine must be >= 1', got: {}", result.content[0].text);
}

/// BUG-3: xray_callers with depth=0 should return error.
#[test]
fn test_xray_callers_depth_zero_returns_error() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["Execute"],
        "depth": 0
    }));
    assert!(result.is_error, "depth=0 should return an error");
    assert!(result.content[0].text.contains("depth must be >= 1"),
        "Error should mention 'depth must be >= 1', got: {}", result.content[0].text);
}

/// Report gap 4.3: containsLine on SQL file — should find the SP containing the line.
/// Uses manually constructed DefinitionEntry to simulate SQL parser output.
#[test]
fn test_contains_line_sql_stored_procedure() {
    use crate::definitions::*;

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\sql\\schema.sql".to_string()],
        total_tokens: 50,
        extensions: vec!["sql".to_string()],
        file_token_counts: vec![25],
        ..Default::default()
    };

    // Simulate SQL parser output: a stored procedure spanning lines 2-7
    // (matching "CREATE PROCEDURE dbo.usp_GetOrders ... END;")
    let defs = vec![
        DefinitionEntry {
            file_id: 0, name: "usp_GetOrders".to_string(),
            kind: DefinitionKind::StoredProcedure, line_start: 2, line_end: 7,
            parent: None, signature: Some("CREATE PROCEDURE dbo.usp_GetOrders @CustomerId INT".to_string()),
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let path_to_id: HashMap<PathBuf, u32> = HashMap::new();

    for (i, def) in defs.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["sql".to_string()],
        files: vec!["C:\\sql\\schema.sql".to_string()],
        definitions: defs, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    // Line 5 is inside the SP body (between line_start=2 and line_end=7)
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "file": ["schema.sql"],
        "containsLine": 5
    }));
    assert!(!result.is_error, "containsLine on SQL should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    // containsLine uses "containingDefinitions" key (not "definitions")
    let defs_arr = output["containingDefinitions"].as_array()
        .unwrap_or_else(|| panic!("Expected 'containingDefinitions' array in output, got: {}", output));
    assert!(!defs_arr.is_empty(),
        "containsLine=5 should find the SP containing that line, got 0 results");
    assert_eq!(defs_arr[0]["name"], "usp_GetOrders",
        "Should find usp_GetOrders SP, got: {}", defs_arr[0]["name"]);
    assert_eq!(defs_arr[0]["kind"], "storedProcedure",
        "Definition kind should be storedProcedure");
}

#[test]
fn test_xray_info_worker_panics_shows_degraded() {
    // Regression test for P0-1: xray_info must expose workerPanics and degraded=true
    use super::handlers_test_utils::make_empty_ctx;
    use std::sync::atomic::Ordering;
    let ctx = make_empty_ctx();
    // Inject a fake file + worker_panics so handle_xray_info includes the content index
    {
        let mut idx = ctx.index.write().unwrap();
        idx.files.push("fake.rs".to_string());
        idx.worker_panics = 2;
    }
    ctx.content_ready.store(true, Ordering::Release);
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let indexes = output["indexes"].as_array().unwrap();
    let content_idx = indexes.iter().find(|i| i["type"] == "content");
    assert!(content_idx.is_some(), "content index must appear in xray_info");
    let ci = content_idx.unwrap();
    assert_eq!(ci["workerPanics"], 2, "workerPanics must be reported");
    assert_eq!(ci["degraded"], true, "degraded must be true when workerPanics > 0");
}

#[test]
fn test_xray_info_no_degraded_when_no_panics() {
    use super::handlers_test_utils::make_empty_ctx;
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let indexes = output["indexes"].as_array().unwrap();
    let content_idx = indexes.iter().find(|i| i["type"] == "content");
    if let Some(ci) = content_idx {
        // When workerPanics == 0, degraded field should be absent or false
        assert!(
            ci["degraded"].is_null() || ci["degraded"] == false,
            "degraded must be absent or false when workerPanics == 0"
        );
    }
}



// ─── Workspace Discovery tests ─────────────────────────────────

#[test]
fn test_workspace_binding_pinned() {
    let ws = WorkspaceBinding::pinned("C:/Projects/MyApp".to_string());
    assert_eq!(ws.dir, "C:/Projects/MyApp");
    assert_eq!(ws.mode, WorkspaceBindingMode::PinnedCli);
    assert_eq!(ws.status, WorkspaceStatus::Resolved);
    assert_eq!(ws.generation, 1);
}

#[test]
fn test_workspace_binding_dot_bootstrap() {
    let ws = WorkspaceBinding::dot_bootstrap("/home/user/project".to_string());
    assert_eq!(ws.mode, WorkspaceBindingMode::DotBootstrap);
    assert_eq!(ws.status, WorkspaceStatus::Resolved);
    assert_eq!(ws.generation, 1);
}

#[test]
fn test_workspace_binding_unresolved() {
    let ws = WorkspaceBinding::unresolved("C:/Program Files/VS Code".to_string());
    assert_eq!(ws.mode, WorkspaceBindingMode::Unresolved);
    assert_eq!(ws.status, WorkspaceStatus::Unresolved);
    assert_eq!(ws.generation, 0);
}

#[test]
fn test_workspace_binding_mode_display() {
    assert_eq!(WorkspaceBindingMode::PinnedCli.to_string(), "pinned_cli");
    assert_eq!(WorkspaceBindingMode::ClientRoots.to_string(), "client_roots");
    assert_eq!(WorkspaceBindingMode::ManualOverride.to_string(), "manual_override");
    assert_eq!(WorkspaceBindingMode::DotBootstrap.to_string(), "dot_bootstrap");
    assert_eq!(WorkspaceBindingMode::Unresolved.to_string(), "unresolved");
}

#[test]
fn test_workspace_status_display() {
    assert_eq!(WorkspaceStatus::Resolved.to_string(), "resolved");
    assert_eq!(WorkspaceStatus::Reindexing.to_string(), "reindexing");
    assert_eq!(WorkspaceStatus::Unresolved.to_string(), "unresolved");
}

#[test]
fn test_unresolved_blocks_workspace_dependent_tools() {
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::unresolved(".".to_string()))),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["test"]}));
    assert!(result.is_error);
    let text = &result.content[0].text;
    assert!(text.contains("WORKSPACE_UNRESOLVED"), "Should contain WORKSPACE_UNRESOLVED, got: {}", text);
}

#[test]
fn test_unresolved_allows_workspace_independent_tools() {
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::unresolved(".".to_string()))),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error, "xray_info should work even when UNRESOLVED");
    let result = dispatch_tool(&ctx, "xray_help", &json!({}));
    assert!(!result.is_error, "xray_help should work even when UNRESOLVED");
}

#[test]
fn test_workspace_metadata_in_tool_response() {
    let ctx = HandlerContext::default();
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output.get("workspace").is_some(), "xray_info should have workspace section");
    assert_eq!(output["workspace"]["mode"], "pinned_cli");
    assert_eq!(output["workspace"]["status"], "resolved");
}

#[test]
fn test_workspace_metadata_in_summary() {
    let ctx = HandlerContext::default();
    let result = dispatch_tool(&ctx, "xray_help", &json!({}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let summary = &output["summary"];
    assert!(summary.get("serverDir").is_some(), "summary should have serverDir");
    assert_eq!(summary["workspaceStatus"], "resolved");
    assert_eq!(summary["workspaceSource"], "pinned_cli");
    assert!(summary["workspaceGeneration"].as_u64().unwrap() >= 1);
}

#[test]
fn test_has_source_files_with_current_dir() {
    let exts = vec!["rs".to_string()];
    assert!(has_source_files(".", &exts, 3, false), "Current dir should have .rs files");
}

#[test]
fn test_has_source_files_nonexistent_ext() {
    let exts = vec!["zzznotreal".to_string()];
    assert!(!has_source_files(".", &exts, 3, false), "Should not find .zzznotreal files");
}

#[test]
fn test_determine_initial_binding_explicit_path() {
    let ws = determine_initial_binding("C:/Projects/MyApp", &["rs".to_string()], false);
    assert_eq!(ws.mode, WorkspaceBindingMode::PinnedCli);
    assert_eq!(ws.dir, "C:/Projects/MyApp");
}

#[test]
fn test_determine_initial_binding_dot_with_sources() {
    let ws = determine_initial_binding(".", &["rs".to_string()], false);
    assert_eq!(ws.mode, WorkspaceBindingMode::DotBootstrap);
    assert_eq!(ws.status, WorkspaceStatus::Resolved);
}

#[test]
fn test_determine_initial_binding_dot_without_sources() {
    let ws = determine_initial_binding(".", &["zzznotreal".to_string()], false);
    assert_eq!(ws.mode, WorkspaceBindingMode::Unresolved);
    assert_eq!(ws.status, WorkspaceStatus::Unresolved);
}


#[test]
fn test_unresolved_blocks_definitions() {
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::unresolved(".".to_string()))),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({"name": ["test"]}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("WORKSPACE_UNRESOLVED"));
}

#[test]
fn test_unresolved_blocks_callers() {
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::unresolved(".".to_string()))),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_callers", &json!({"method": ["test"]}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("WORKSPACE_UNRESOLVED"));
}

#[test]
fn test_unresolved_blocks_fast() {
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::unresolved(".".to_string()))),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_fast", &json!({"pattern": ["test"]}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("WORKSPACE_UNRESOLVED"));
}


#[test]
fn test_unresolved_blocks_edit() {
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::unresolved(".".to_string()))),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_edit", &json!({"path": "test.rs"}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("WORKSPACE_UNRESOLVED"));
}

#[test]
fn test_unresolved_blocks_git_tools() {
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::unresolved(".".to_string()))),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_git_history", &json!({"repo": ".", "file": ["test.rs"]}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("WORKSPACE_UNRESOLVED"));
    let result = dispatch_tool(&ctx, "xray_branch_status", &json!({"repo": "."}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("WORKSPACE_UNRESOLVED"));
}

#[test]
fn test_reindexing_blocks_workspace_dependent_tools() {
    let ws = WorkspaceBinding {
        canonical_dir: String::new(),
        dir: ".".to_string(),
        mode: WorkspaceBindingMode::ManualOverride,
        status: WorkspaceStatus::Reindexing,
        generation: 2,
    };
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(ws)),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["test"]}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("WORKSPACE_REINDEXING"));
}

#[test]
fn test_reindexing_allows_workspace_independent_tools() {
    let ws = WorkspaceBinding {
        canonical_dir: String::new(),
        dir: ".".to_string(),
        mode: WorkspaceBindingMode::ManualOverride,
        status: WorkspaceStatus::Reindexing,
        generation: 2,
    };
    let ctx = HandlerContext {
        workspace: Arc::new(RwLock::new(ws)),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_info", &json!({}));
    assert!(!result.is_error, "xray_info should work during REINDEXING");
    let result = dispatch_tool(&ctx, "xray_reindex", &json!({}));
    // xray_reindex may error due to empty index, but NOT due to workspace gate
    if result.is_error {
        assert!(!result.content[0].text.contains("WORKSPACE_REINDEXING"),
            "xray_reindex should not be blocked by REINDEXING status");
    }
}

// === Phase 2: dispatch-level gate errors now carry pipeline metadata ===

#[test]
fn test_unresolved_gate_carries_pipeline_metadata() {
    // Phase 2 contract: workspace-unresolved error must carry policyReminder
    // and serverDir from finalize_response (previously bypassed the pipeline).
    let ws = WorkspaceBinding::unresolved(".".to_string());
    let ctx = HandlerContext {
        metrics: true,
        workspace: Arc::new(RwLock::new(ws)),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["test"]}));
    assert!(result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text)
        .expect("gate error must be valid JSON");
    assert_eq!(output["error"], "WORKSPACE_UNRESOLVED");
    let summary = &output["summary"];
    assert!(summary["policyReminder"].as_str().is_some(),
        "policyReminder must be injected on workspace-unresolved gate error");
    assert!(summary["totalTimeMs"].as_f64().is_some(),
        "totalTimeMs must be injected on workspace-unresolved gate error");
    assert!(summary["serverDir"].as_str().is_some(),
        "serverDir must be injected on workspace-unresolved gate error");
}

#[test]
fn test_reindexing_gate_carries_pipeline_metadata() {
    // Phase 2 contract: workspace-reindexing error must carry policyReminder
    // and totalTimeMs from finalize_response.
    let ws = WorkspaceBinding {
        canonical_dir: String::new(),
        dir: ".".to_string(),
        mode: WorkspaceBindingMode::ManualOverride,
        status: WorkspaceStatus::Reindexing,
        generation: 2,
    };
    let ctx = HandlerContext {
        metrics: true,
        workspace: Arc::new(RwLock::new(ws)),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["test"]}));
    assert!(result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text)
        .expect("gate error must be valid JSON");
    assert_eq!(output["error"], "WORKSPACE_REINDEXING");
    let summary = &output["summary"];
    assert!(summary["policyReminder"].as_str().is_some(),
        "policyReminder must be injected on workspace-reindexing gate error");
    assert!(summary["totalTimeMs"].as_f64().is_some(),
        "totalTimeMs must be injected on workspace-reindexing gate error");
}

#[test]
fn test_index_building_gate_carries_pipeline_metadata() {
    // Phase 2 contract: content-index-not-ready error (plain text) must be
    // wrapped into JSON envelope and carry policyReminder + totalTimeMs.
    let ctx = HandlerContext {
        metrics: true,
        content_ready: Arc::new(AtomicBool::new(false)),
        ..Default::default()
    };
    // xray_grep requires content index
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["test"]}));
    assert!(result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text)
        .expect("plain-text gate error must be wrapped into JSON envelope");
    assert!(output["error"].as_str().unwrap().contains("being built"),
        "error must mention index being built");
    let summary = &output["summary"];
    assert!(summary["policyReminder"].as_str().is_some(),
        "policyReminder must be injected on index-building gate error");
    assert!(summary["totalTimeMs"].as_f64().is_some(),
        "totalTimeMs must be injected on index-building gate error");
}

#[test]
fn test_unknown_tool_gate_carries_pipeline_metadata() {
    // Phase 2 contract: "Unknown tool: X" error (plain text) must be wrapped
    // into JSON envelope and carry pipeline metadata.
    let ctx = HandlerContext {
        metrics: true,
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_nonexistent", &json!({}));
    assert!(result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text)
        .expect("unknown-tool error must be wrapped into JSON envelope");
    assert!(output["error"].as_str().unwrap().contains("Unknown tool"),
        "error must mention unknown tool");
    let summary = &output["summary"];
    assert!(summary["policyReminder"].as_str().is_some(),
        "policyReminder must be injected on unknown-tool gate error");
    assert!(summary["totalTimeMs"].as_f64().is_some(),
        "totalTimeMs must be injected on unknown-tool gate error");
}

#[test]
fn test_gate_errors_without_metrics_still_carry_guidance() {
    // Even when --metrics is NOT enabled, gate errors should still carry
    // policyReminder and serverDir (guidance is unconditional, only
    // totalTimeMs requires --metrics).
    let ws = WorkspaceBinding::unresolved(".".to_string());
    let ctx = HandlerContext {
        metrics: false,
        workspace: Arc::new(RwLock::new(ws)),
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_grep", &json!({"terms": ["test"]}));
    assert!(result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text)
        .expect("gate error must be valid JSON");
    let summary = &output["summary"];
    assert!(summary["policyReminder"].as_str().is_some(),
        "policyReminder must be injected even without --metrics");
    assert!(summary["serverDir"].as_str().is_some(),
        "serverDir must be injected even without --metrics");
    // totalTimeMs should NOT be present without --metrics
    assert!(summary.get("totalTimeMs").is_none(),
        "totalTimeMs must NOT be injected without --metrics flag");
}

#[test]
fn test_truncation_preserves_is_error_flag() {
    // Regression: truncate_response_if_needed previously returned
    // ToolCallResult::success, flipping errors into apparent success.
    // This matters for gate errors flowing through finalize_response
    // with metrics=false (truncation path, not inject_metrics path).
    let ctx = HandlerContext {
        metrics: false,
        max_response_bytes: 200, // deliberately tiny — forces truncation
        ..Default::default()
    };
    let result = dispatch_tool(&ctx, "xray_nonexistent", &json!({}));
    assert!(result.is_error,
        "truncated error must retain is_error flag (was flipped to success)");
}


// ─── §4.3 step 6: xray_reindex / xray_reindex_definitions `ext` array migration ───
//
// Regression-pair tests: array-form succeeds (with discriminating files>=1 check
// — default server_ext="cs" yields 0 files in this repo, so without the
// migration the array form would be silently ignored and fall back to "cs" → 0
// files, failing the assertion); bare-string form rejected with migration-aware
// error citing the parameter name and 2026-04-25 / array.

#[test]
fn test_xray_reindex_ext_array_form_accepted() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_reindex", &json!({ "ext": ["rs"] }));
    assert!(!result.is_error,
        "array-form ext must succeed; got: {}", result.content[0].text);
    let output: serde_json::Value =
        serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["status"], "ok");
    assert!(
        output["files"].as_u64().unwrap() >= 1,
        "ext=[\"rs\"] must index >=1 *.rs file (regression: pre-fix the parser \
         ignored array form, fell back to server_ext=\"cs\" → 0 files). Got: {}",
        output
    );
}

#[test]
fn test_xray_reindex_ext_string_form_rejected() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "xray_reindex", &json!({ "ext": "rs" }));
    assert!(result.is_error, "Bare-string ext must be rejected post 2026-04-25 migration");
    let msg = &result.content[0].text;
    assert!(msg.contains("ext"),
        "Error must mention the offending parameter name. Got: {}", msg);
    assert!(msg.contains("array") || msg.contains("2026-04-25"),
        "Error must explain array contract / cite migration. Got: {}", msg);
}

#[test]
fn test_xray_reindex_definitions_ext_array_form_accepted() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_reindex_definitions", &json!({ "ext": ["rs"] }));
    assert!(!result.is_error,
        "array-form ext must succeed; got: {}", result.content[0].text);
    let output: serde_json::Value =
        serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["status"], "ok");
    assert!(
        output["files"].as_u64().unwrap() >= 1,
        "ext=[\"rs\"] must index >=1 *.rs file (regression: pre-fix the parser \
         ignored array form, fell back to server_ext=\"cs\" → 0 files). Got: {}",
        output
    );
}

#[test]
fn test_xray_reindex_definitions_ext_string_form_rejected() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_reindex_definitions", &json!({ "ext": "rs" }));
    assert!(result.is_error, "Bare-string ext must be rejected post 2026-04-25 migration");
    let msg = &result.content[0].text;
    assert!(msg.contains("ext"),
        "Error must mention the offending parameter name. Got: {}", msg);
    assert!(msg.contains("array") || msg.contains("2026-04-25"),
        "Error must explain array contract / cite migration. Got: {}", msg);
}


// ─── §4.6 audit: per-key array-form contract for ALL migrated MCP keys ───
//
// Contract (established as of step 5 round 1, 2026-04-25): every MCP tool key
// migrated from comma-separated `string` to `array<string>` MUST have an
// end-to-end regression test that bare-string is rejected with an error
// citing the parameter name and ("array" OR "2026-04-25").
//
// Steps 5 (xray_fast.{pattern,ext}) and 6 (xray_reindex(_definitions).ext)
// added per-handler tests inline. Earlier steps 2–4 (xray_grep.{terms,file,ext},
// xray_definitions.{name,kind,file,parent}, xray_callers.{method,ext}) did
// not. The helper-level tests (`read_string_array_rejects_string_form_*`)
// prove the helper rejects, but they do NOT catch a regression where a
// handler stops calling `read_string_array` for a specific key (e.g. someone
// re-introduces `args.get("terms").and_then(|v| v.as_str())`).
//
// This single table-driven test closes the audit gap. Each (tool, key) pair
// dispatches with a bare-string value and asserts the canonical rejection
// shape. Adding a new migrated key WITHOUT extending this table is the
// expected forcing function for the next reviewer.
#[test]
fn test_all_migrated_keys_reject_bare_string_form() {
    // (tool, key, extra_args_to_make_call_reach_the_parser)
    // - xray_grep needs nothing else (terms is the only required key; file/ext
    //   are optional but should still be parsed when present).
    // - xray_definitions needs def_index (via make_ctx_with_defs) and at least
    //   `name` to reach the kind/file/parent parsers.
    // - xray_callers needs def_index and `method` to reach the ext parser.
    // - xray_fast / xray_reindex / xray_reindex_definitions are covered by
    //   step-5 / step-6 inline tests; not duplicated here.
    let ctx = make_ctx_with_defs();
    let cases: &[(&str, &str, serde_json::Value)] = &[
        // xray_grep — step 2
        ("xray_grep", "terms", json!({ "terms": "foo" })),
        ("xray_grep", "file",  json!({ "terms": ["foo"], "file": "a.rs" })),
        ("xray_grep", "ext",   json!({ "terms": ["foo"], "ext": "rs" })),
        // xray_definitions — step 3
        ("xray_definitions", "name",   json!({ "name":   "Foo" })),
        ("xray_definitions", "kind",   json!({ "name": ["Foo"], "kind":   "class" })),
        ("xray_definitions", "file",   json!({ "name": ["Foo"], "file":   "a.cs" })),
        ("xray_definitions", "parent", json!({ "name": ["Foo"], "parent": "Bar" })),
        // xray_callers — step 4
        ("xray_callers", "method", json!({ "method": "Foo" })),
        ("xray_callers", "ext",    json!({ "method": ["ExecuteQueryAsync"], "ext": "cs" })),
    ];

    for (tool, key, args) in cases {
        let result = dispatch_tool(&ctx, tool, args);
        assert!(
            result.is_error,
            "({tool}, {key}): bare-string must be rejected post 2026-04-25 migration. Got non-error: {}",
            result.content[0].text
        );
        let msg = &result.content[0].text;
        assert!(
            msg.contains(*key),
            "({tool}, {key}): error must mention the offending parameter name. Got: {msg}"
        );
        assert!(
            msg.contains("array") || msg.contains("2026-04-25"),
            "({tool}, {key}): error must explain array contract or cite migration date. Got: {msg}"
        );
    }
}
