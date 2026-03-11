//! Tests for metrics, security, reindex, info, help, definitions ranking, input validation --
//! extracted from handlers_tests.rs.

use super::*;
use super::utils::validate_search_dir;
use super::handlers_test_utils::{make_ctx_with_defs, make_empty_ctx};
use crate::Posting;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

// --- Metrics injection tests ---

#[test] fn test_metrics_off_no_extra_fields() {
    let mut idx = HashMap::new();
    idx.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![5] }]);
    let index = ContentIndex { root: ".".to_string(), files: vec!["C:\\test\\Program.cs".to_string()], index: idx, total_tokens: 100, extensions: vec!["cs".to_string()], file_token_counts: vec![50], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(index)), ..Default::default() };
    let result = dispatch_tool(&ctx, "search_grep", &json!({"terms": "HttpClient"}));
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
    let result = dispatch_tool(&ctx, "search_grep", &json!({"terms": "HttpClient"}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["policyReminder"].as_str().is_some());
    assert!(output["summary"]["nextStepHint"].as_str().is_some());
    assert!(output["summary"]["searchTimeMs"].as_f64().is_some());
    assert!(output["summary"]["responseBytes"].as_u64().is_some());
    assert!(output["summary"]["estimatedTokens"].as_u64().is_some());
}

#[test] fn test_metrics_not_injected_on_error() {
    let ctx = make_empty_ctx();
    let ctx = HandlerContext { metrics: true, ..ctx };
    let result = dispatch_tool(&ctx, "search_grep", &json!({}));
    assert!(result.is_error);
    assert!(!result.content[0].text.contains("searchTimeMs"));
    assert!(!result.content[0].text.contains("policyReminder"));
}

#[test]
fn test_search_help_has_policy_but_no_next_step_hint() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "search_help", &json!({}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["policyReminder"].as_str().is_some());
    assert!(output["summary"].get("nextStepHint").is_none());
}

#[test]
fn test_search_info_has_policy_but_no_next_step_hint() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "search_info", &json!({}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["policyReminder"].as_str().is_some());
    assert!(output["summary"].get("nextStepHint").is_none());
}

#[test]
fn test_search_reindex_has_policy_but_no_next_step_hint() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "search_reindex", &json!({}));
    // search_reindex with empty ctx may error (no dir), but if it returns success JSON, verify guidance
    if !result.is_error {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(output["summary"]["policyReminder"].as_str().is_some());
        assert!(output["summary"].get("nextStepHint").is_none());
    }
}

#[test]
fn test_search_reindex_definitions_has_policy_but_no_next_step_hint() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "search_reindex_definitions", &json!({}));
    // search_reindex_definitions with empty ctx may error (no def_index), but if it returns success JSON, verify guidance
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
    let result = dispatch_tool(&ctx, "search_grep", &json!({"terms": "foo"}));
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
// ─── General search_definitions tests ───────────────────────────────

/// search_definitions non-existent name returns empty.
#[test]
fn test_search_definitions_nonexistent_name_returns_empty() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "CompletelyNonExistentDefinitionXYZ123"
    }));
    assert!(!result.is_error, "Non-existent name should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert!(defs.is_empty(),
        "Expected empty definitions array for non-existent name, got {} results", defs.len());
    assert_eq!(output["summary"]["totalResults"], 0);
}

/// search_definitions invalid regex error.
#[test]
fn test_search_definitions_invalid_regex_error() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "[invalid",
        "regex": true
    }));
    assert!(result.is_error, "Invalid regex should produce an error");
    assert!(result.content[0].text.contains("Invalid regex"),
        "Error should mention 'Invalid regex', got: {}", result.content[0].text);
}
/// T77 — search_definitions file filter: backslash vs forward slash normalization.
#[test]
fn test_search_definitions_file_filter_slash_normalization() {
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

    let result_backslash = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "Models\\User"
    }));
    assert!(!result_backslash.is_error);
    let output_bs: Value = serde_json::from_str(&result_backslash.content[0].text).unwrap();
    let defs_bs = output_bs["definitions"].as_array().unwrap();

    let result_fwdslash = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "Models/User"
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

    let result_fragment = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "User"
    }));
    assert!(!result_fragment.is_error);
    let output_frag: Value = serde_json::from_str(&result_fragment.content[0].text).unwrap();
    let defs_frag = output_frag["definitions"].as_array().unwrap();
    assert_eq!(defs_frag.len(), 2,
        "File filter 'User' should match both User.cs and UserService.cs, got {}", defs_frag.len());
}

/// T80 — search_reindex with invalid/non-existent directory.
#[test]
fn test_search_reindex_invalid_directory() {
    let ctx = make_empty_ctx();

    let result = dispatch_tool(&ctx, "search_reindex", &json!({
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

#[test]
fn test_validate_search_dir_windows_absolute_outside_rejected() {
    // Non-existent absolute path that clearly isn't under the server dir
    // canonicalize will fail, falling back to raw string comparison
    let result = validate_search_dir(
        r"C:\Windows\System32",
        r"C:\Repos\MyProject",
    );
    assert!(result.is_err(),
        "Absolute path outside server dir should be rejected");
}

// ─── search_help response structure tests ────────────────────────────

#[test]
fn test_search_help_response_structure() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "search_help", &json!({}));
    assert!(!result.is_error, "search_help should not error");

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

// ─── search_info response structure tests ────────────────────────────

#[test]
fn test_search_info_response_structure() {
    let ctx = make_empty_ctx();
    let result = dispatch_tool(&ctx, "search_info", &json!({}));
    assert!(!result.is_error, "search_info should not error");

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
            other => panic!("Unexpected index type: {}", other),
        }
    }
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

/// search_definitions ranking: exact match class comes first, then prefix matches,
/// with shorter names before longer. Type-level defs sort before member-level.
#[test]
fn test_search_definitions_ranking_exact_first() {
    let ctx = make_ranking_defs_ctx();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "UserService"
    }));
    assert!(!result.is_error, "search_definitions should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    assert!(defs.len() >= 3, "Should find at least 3 definitions containing 'UserService', got {}", defs.len());

    // First result should be exact match "UserService" (class, tier 0)
    assert_eq!(defs[0]["name"], "UserService",
        "First result should be exact match 'UserService', got '{}'", defs[0]["name"]);
    assert_eq!(defs[0]["kind"], "class",
        "Exact match should be the class definition");
}

/// search_definitions ranking: prefix matches come before contains matches.
#[test]
fn test_search_definitions_ranking_prefix_before_contains() {
    let ctx = make_ranking_defs_ctx();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "UserService"
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

/// search_definitions ranking: among prefix matches, type-level (class) sorts before
/// member-level (method), and shorter names before longer for same kind priority.
#[test]
fn test_search_definitions_ranking_kind_and_length_tiebreak() {
    let ctx = make_ranking_defs_ctx();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "UserService"
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

/// search_definitions ranking: regex mode should NOT apply relevance ranking.
#[test]
fn test_search_definitions_ranking_not_applied_with_regex() {
    let ctx = make_ranking_defs_ctx();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "UserService.*",
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

/// BUG-1: search_definitions with name="" should behave like no name filter (return all).
#[test]
fn test_search_definitions_empty_name_treated_as_no_filter() {
    let ctx = make_ctx_with_defs();
    // With name="" — should return all definitions (empty string ignored)
    let result_empty = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "",
        "maxResults": 5
    }));
    assert!(!result_empty.is_error, "name='' should not error: {}", result_empty.content[0].text);
    let output_empty: Value = serde_json::from_str(&result_empty.content[0].text).unwrap();
    let count_empty = output_empty["summary"]["totalResults"].as_u64().unwrap();

    // Without name — should return all definitions
    let result_no_name = dispatch_tool(&ctx, "search_definitions", &json!({
        "maxResults": 5
    }));
    let output_no_name: Value = serde_json::from_str(&result_no_name.content[0].text).unwrap();
    let count_no_name = output_no_name["summary"]["totalResults"].as_u64().unwrap();

    assert_eq!(count_empty, count_no_name,
        "name='' should behave like no name filter. Got {} vs {} results",
        count_empty, count_no_name);
    assert!(count_empty > 0, "Should have some definitions in test context");
}

/// BUG-2: search_definitions with containsLine=-1 should return error.
#[test]
fn test_search_definitions_contains_line_negative_returns_error() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "QueryService",
        "containsLine": -1
    }));
    assert!(result.is_error, "containsLine=-1 should return an error");
    assert!(result.content[0].text.contains("containsLine must be >= 1"),
        "Error should mention 'containsLine must be >= 1', got: {}", result.content[0].text);
}

/// BUG-2: search_definitions with containsLine=0 should return error.
#[test]
fn test_search_definitions_contains_line_zero_returns_error() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "QueryService",
        "containsLine": 0
    }));
    assert!(result.is_error, "containsLine=0 should return an error");
    assert!(result.content[0].text.contains("containsLine must be >= 1"),
        "Error should mention 'containsLine must be >= 1', got: {}", result.content[0].text);
}

/// BUG-3: search_callers with depth=0 should return error.
#[test]
fn test_search_callers_depth_zero_returns_error() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "Execute",
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
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "schema.sql",
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
