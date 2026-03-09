use super::*;

#[test]
fn test_json_to_string_happy_path() {
    let v = json!({"key": "value", "num": 42});
    let result = json_to_string(&v);
    assert!(result.contains("key"));
    assert!(result.contains("42"));
}

#[test]
fn test_json_to_string_returns_valid_json() {
    let v = json!({"ok": true});
    let result = json_to_string(&v);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["ok"], true);
}

#[test]
fn test_sorted_intersect_empty_left() {
    assert_eq!(sorted_intersect(&[], &[1, 2, 3]), Vec::<u32>::new());
}

#[test]
fn test_sorted_intersect_empty_right() {
    assert_eq!(sorted_intersect(&[1, 2, 3], &[]), Vec::<u32>::new());
}

#[test]
fn test_sorted_intersect_both_empty() {
    assert_eq!(sorted_intersect(&[], &[]), Vec::<u32>::new());
}

#[test]
fn test_sorted_intersect_disjoint() {
    assert_eq!(sorted_intersect(&[1, 3, 5], &[2, 4, 6]), Vec::<u32>::new());
}

#[test]
fn test_normalize_path_sep() {
    assert_eq!(normalize_path_sep(r"C:\foo\bar"), "C:/foo/bar");
}

#[test]
fn test_is_under_dir_basic() {
    assert!(is_under_dir("C:/Repos/MyProject/src/file.cs", "C:/Repos/MyProject"));
}

#[test]
fn test_is_under_dir_case_insensitive() {
    assert!(is_under_dir("C:/repos/myproject/src/file.cs", "C:/Repos/MyProject"));
}

#[test]
fn test_is_under_dir_not_prefix_of_different_dir() {
    assert!(!is_under_dir("C:/Repos/MainProjectExtra/file.cs", "C:/Repos/MainProject"));
}

#[test]
fn test_is_under_dir_exact_match() {
    assert!(!is_under_dir("C:/Repos/MainProject", "C:/Repos/MainProject"));
}

#[test]
fn test_validate_search_dir_exact_match() {
    // We can't easily test this without real directories, but we can test the logic
    // with paths that don't exist (canonicalize will fail, falling back to raw string)
    let result = validate_search_dir("/nonexistent/dir", "/nonexistent/dir");
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn test_validate_search_dir_outside_rejects() {
    let result = validate_search_dir("/other/dir", "/my/dir");
    assert!(result.is_err());
}

#[test]
fn test_grouped_line_content_single_group() {
    let lines = vec!["line0", "line1", "line2", "line3", "line4"];
    let mut to_show = BTreeSet::new();
    to_show.insert(1);
    to_show.insert(2);
    to_show.insert(3);
    let mut match_set = HashSet::new();
    match_set.insert(2);

    let result = build_grouped_line_content(&to_show, &lines, &match_set);
    let groups = result.as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["startLine"], 2);
    assert_eq!(groups[0]["lines"].as_array().unwrap().len(), 3);
}

#[test]
fn test_grouped_line_content_two_groups() {
    let lines = vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
    let mut to_show = BTreeSet::new();
    to_show.insert(1);
    to_show.insert(2);
    to_show.insert(7);
    to_show.insert(8);
    let mut match_set = HashSet::new();
    match_set.insert(1);
    match_set.insert(8);

    let result = build_grouped_line_content(&to_show, &lines, &match_set);
    let groups = result.as_array().unwrap();
    assert_eq!(groups.len(), 2);
}

#[test]
fn test_grouped_line_content_no_matches() {
    let lines = vec!["a", "b", "c"];
    let mut to_show = BTreeSet::new();
    to_show.insert(0);
    let match_set = HashSet::new();

    let result = build_grouped_line_content(&to_show, &lines, &match_set);
    let groups = result.as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert!(groups[0].get("matchIndices").is_none());
}

#[test]
fn test_grouped_line_content_empty() {
    let lines: Vec<&str> = vec![];
    let to_show = BTreeSet::new();
    let match_set = HashSet::new();

    let result = build_grouped_line_content(&to_show, &lines, &match_set);
    let groups = result.as_array().unwrap();
    assert!(groups.is_empty());
}

#[test]
fn test_grouped_line_content_multiple_matches_in_group() {
    let lines = vec!["a", "b", "c", "d", "e"];
    let mut to_show = BTreeSet::new();
    for i in 0..5 { to_show.insert(i); }
    let mut match_set = HashSet::new();
    match_set.insert(1);
    match_set.insert(3);

    let result = build_grouped_line_content(&to_show, &lines, &match_set);
    let groups = result.as_array().unwrap();
    assert_eq!(groups.len(), 1);
    let indices = groups[0]["matchIndices"].as_array().unwrap();
    assert_eq!(indices.len(), 2);
}

#[test]
fn test_context_lines_calculation() {
    let content = (0..20).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    let match_lines = vec![10u32]; // line 10 (1-based)
    let result = build_line_content_from_matches(&content, &match_lines, 2);
    let groups = result.as_array().unwrap();
    assert_eq!(groups.len(), 1);
    // Should show lines 8-12 (5 lines: 2 before + match + 2 after)
    let lines = groups[0]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 5);
}

#[test]
fn test_context_lines_at_file_boundaries() {
    let content = "line1\nline2\nline3";
    let match_lines = vec![1u32];
    let result = build_line_content_from_matches(&content, &match_lines, 5);
    let groups = result.as_array().unwrap();
    assert_eq!(groups.len(), 1);
    let lines = groups[0]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3); // can't go before line 1
}

#[test]
fn test_context_merges_overlapping_ranges() {
    let content = (0..20).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    let match_lines = vec![5u32, 7u32]; // lines 5 and 7 with context 2 overlap
    let result = build_line_content_from_matches(&content, &match_lines, 2);
    let groups = result.as_array().unwrap();
    assert_eq!(groups.len(), 1); // should merge into single group
}

// ─── Response truncation tests ──────────────────────────────────

#[test]
fn test_truncate_small_response_unchanged() {
    let output = json!({
        "files": [{"path": "a.cs", "lines": [1, 2, 3]}],
        "summary": {"totalFiles": 1}
    });
    let result = truncate_large_response(output.clone(), DEFAULT_MAX_RESPONSE_BYTES);
    // Small response should be unchanged
    assert_eq!(result, output);
}

#[test]
fn test_truncate_caps_lines_per_file() {
    // Build a response with files having many lines
    let many_lines: Vec<u32> = (1..=200).collect();
    let mut files = Vec::new();
    for i in 0..100 {
        files.push(json!({
            "path": format!("/some/very/long/path/to/file_{}.cs", i),
            "score": 0.5,
            "occurrences": 200,
            "lines": many_lines,
        }));
    }
    let output = json!({
        "files": files,
        "summary": {"totalFiles": 100, "totalOccurrences": 20000}
    });

    let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
    let result_str = serde_json::to_string(&result).unwrap();

    // Should be truncated
    assert!(result.get("summary").unwrap().get("responseTruncated").is_some(),
        "Expected responseTruncated in summary");

    // Lines per file should be capped
    if let Some(files) = result.get("files").and_then(|f| f.as_array()) {
        for file in files {
            if let Some(lines) = file.get("lines").and_then(|l| l.as_array()) {
                assert!(lines.len() <= MAX_LINES_PER_FILE,
                    "Lines array should be capped to {}", MAX_LINES_PER_FILE);
            }
        }
    }

    // Final size should be under budget
    assert!(result_str.len() <= DEFAULT_MAX_RESPONSE_BYTES + 500, // small tolerance for metadata
        "Response {} bytes should be near budget {}", result_str.len(), DEFAULT_MAX_RESPONSE_BYTES);
}

#[test]
fn test_truncate_caps_matched_tokens() {
    let many_tokens: Vec<String> = (0..500).map(|i| format!("token_{}", i)).collect();
    let mut files = Vec::new();
    for i in 0..50 {
        files.push(json!({
            "path": format!("/path/file_{}.cs", i),
            "lines": [1, 2, 3],
        }));
    }
    let output = json!({
        "files": files,
        "summary": {
            "totalFiles": 50,
            "matchedTokens": many_tokens,
        }
    });

    let initial_size = serde_json::to_string(&output).unwrap().len();
    if initial_size > DEFAULT_MAX_RESPONSE_BYTES {
        let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
        if let Some(tokens) = result.get("summary")
            .and_then(|s| s.get("matchedTokens"))
            .and_then(|t| t.as_array())
        {
            assert!(tokens.len() <= MAX_MATCHED_TOKENS,
                "matchedTokens should be capped to {}", MAX_MATCHED_TOKENS);
        }
    }
}

#[test]
fn test_truncate_removes_line_content() {
    // Build a response with lineContent (large)
    let mut files = Vec::new();
    for i in 0..50 {
        files.push(json!({
            "path": format!("/path/file_{}.cs", i),
            "lines": [1, 2, 3],
            "lineContent": [{
                "startLine": 1,
                "lines": (0..100).map(|j| format!("    some code line {} in file {}", j, i)).collect::<Vec<_>>(),
            }],
        }));
    }
    let output = json!({
        "files": files,
        "summary": {"totalFiles": 50}
    });

    let initial_size = serde_json::to_string(&output).unwrap().len();
    if initial_size > DEFAULT_MAX_RESPONSE_BYTES {
        let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
        // lineContent should be removed
        if let Some(files) = result.get("files").and_then(|f| f.as_array()) {
            for file in files {
                assert!(file.get("lineContent").is_none(),
                    "lineContent should be removed during truncation");
            }
        }
    }
}

#[test]
fn test_truncate_reduces_file_count() {
    // Build a response with 1000 files — way over budget
    let mut files = Vec::new();
    for i in 0..1000 {
        files.push(json!({
            "path": format!("/some/long/path/to/deeply/nested/file_number_{}.cs", i),
            "score": 0.001,
            "occurrences": 1,
            "lines": [1],
        }));
    }
    let output = json!({
        "files": files,
        "summary": {"totalFiles": 1000, "totalOccurrences": 1000}
    });

    let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
    let result_files = result.get("files").and_then(|f| f.as_array()).unwrap();
    assert!(result_files.len() < 1000,
        "File count should be reduced from 1000, got {}", result_files.len());

    // Summary should indicate truncation
    let summary = result.get("summary").unwrap();
    assert_eq!(summary.get("responseTruncated").and_then(|v| v.as_bool()), Some(true));
    assert!(summary.get("truncationReason").is_some());
    assert!(summary.get("hint").is_some());
}

#[test]
fn test_truncate_response_if_needed_small() {
    let small = ToolCallResult::success(r#"{"files":[],"summary":{"totalFiles":0}}"#.to_string());
    let result = truncate_response_if_needed(small, DEFAULT_MAX_RESPONSE_BYTES);
    assert!(!result.is_error);
}

#[test]
fn test_truncate_definitions_array() {
    // Build a search_definitions-style response with many definitions — way over budget
    let mut defs = Vec::new();
    for i in 0..5000 {
        defs.push(json!({
            "name": format!("SomeDefinitionName_{}", i),
            "kind": "property",
            "file": format!("/some/long/path/to/deeply/nested/file_{}.ts", i % 100),
            "lines": format!("{}-{}", i * 10, i * 10 + 5),
            "modifiers": ["public"],
            "parent": format!("SomeParentClass_{}", i % 50),
        }));
    }
    let output = json!({
        "definitions": defs,
        "summary": {
            "totalResults": 5000,
            "returned": 5000,
            "searchTimeMs": 1.23,
            "indexFiles": 500,
            "totalDefinitions": 50000,
        }
    });

    let initial_size = serde_json::to_string(&output).unwrap().len();
    assert!(initial_size > DEFAULT_MAX_RESPONSE_BYTES,
        "Test setup: definitions response should be over budget ({} bytes)", initial_size);

    let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
    let result_str = serde_json::to_string(&result).unwrap();

    // Definitions array should be truncated
    let result_defs = result.get("definitions").and_then(|d| d.as_array()).unwrap();
    assert!(result_defs.len() < 5000,
        "Definitions count should be reduced from 5000, got {}", result_defs.len());

    // Summary should indicate truncation
    let summary = result.get("summary").unwrap();
    assert_eq!(summary.get("responseTruncated").and_then(|v| v.as_bool()), Some(true));
    assert!(summary.get("truncationReason").is_some());
    let reason = summary["truncationReason"].as_str().unwrap();
    assert!(reason.contains("definitions"),
        "Truncation reason should mention 'definitions', got: {}", reason);

    // 'returned' in summary should reflect actual array length after truncation
    let returned = summary.get("returned").and_then(|v| v.as_u64()).unwrap() as usize;
    assert_eq!(returned, result_defs.len(),
        "summary.returned ({}) should match actual definitions array length ({})",
        returned, result_defs.len());

    // Hint should be definitions-specific (not grep-specific)
    let hint = summary.get("hint").and_then(|v| v.as_str()).unwrap();
    assert!(hint.contains("name") && hint.contains("kind") && hint.contains("file"),
        "Hint should mention definitions-specific filters, got: {}", hint);
    assert!(!hint.contains("countOnly"),
        "Hint should NOT mention countOnly (that's for grep), got: {}", hint);

    // Result should be reasonably close to budget
    assert!(result_str.len() <= DEFAULT_MAX_RESPONSE_BYTES * 2,
        "Response {} bytes should be near budget {}", result_str.len(), DEFAULT_MAX_RESPONSE_BYTES);
}

#[test]
fn test_truncate_grep_hint_unchanged() {
    // Verify grep-style responses still get the grep-specific hint
    let mut files = Vec::new();
    for i in 0..1000 {
        files.push(json!({
            "path": format!("/some/long/path/to/file_{}.cs", i),
            "score": 0.001,
            "occurrences": 1,
            "lines": [1],
        }));
    }
    let output = json!({
        "files": files,
        "summary": {"totalFiles": 1000, "totalOccurrences": 1000}
    });

    let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
    let summary = result.get("summary").unwrap();
    let hint = summary.get("hint").and_then(|v| v.as_str()).unwrap();
    assert!(hint.contains("countOnly"),
        "Grep hint should mention countOnly, got: {}", hint);
    assert!(!hint.contains("kind"),
        "Grep hint should NOT mention definitions filters, got: {}", hint);
}

// ─── best_match_tier relevance ranking tests ─────────────────────

#[test]
fn test_best_match_tier_exact_match_returns_0() {
    let terms = vec!["userservice".to_string()];
    assert_eq!(best_match_tier("UserService", &terms), 0);
}

#[test]
fn test_best_match_tier_exact_match_case_insensitive() {
    let terms = vec!["userservice".to_string()];
    assert_eq!(best_match_tier("USERSERVICE", &terms), 0);
    assert_eq!(best_match_tier("userservice", &terms), 0);
    assert_eq!(best_match_tier("UserService", &terms), 0);
}

#[test]
fn test_best_match_tier_prefix_match_returns_1() {
    let terms = vec!["userservice".to_string()];
    assert_eq!(best_match_tier("UserServiceFactory", &terms), 1);
}

#[test]
fn test_best_match_tier_contains_only_returns_2() {
    let terms = vec!["userservice".to_string()];
    assert_eq!(best_match_tier("IUserService", &terms), 2);
}

#[test]
fn test_best_match_tier_no_match_returns_2() {
    // The function is called only on already-filtered results,
    // so a non-matching name still returns 2 (contains/default tier).
    let terms = vec!["userservice".to_string()];
    assert_eq!(best_match_tier("OrderProcessor", &terms), 2);
}

#[test]
fn test_best_match_tier_multiple_terms_best_wins() {
    let terms = vec!["order".to_string(), "userservice".to_string()];
    // "UserService" is exact match for "userservice" → tier 0
    assert_eq!(best_match_tier("UserService", &terms), 0);
    // "OrderProcessor" is prefix match for "order" → tier 1
    assert_eq!(best_match_tier("OrderProcessor", &terms), 1);
    // "IUserService" contains "userservice" → tier 2
    assert_eq!(best_match_tier("IUserService", &terms), 2);
}

#[test]
fn test_best_match_tier_empty_terms_returns_2() {
    let terms: Vec<String> = vec![];
    assert_eq!(best_match_tier("UserService", &terms), 2);
}

#[test]
fn test_best_match_tier_exact_beats_prefix_with_multiple_terms() {
    // When one term is exact and another is prefix, exact wins (tier 0)
    let terms = vec!["iuserservice".to_string(), "userservice".to_string()];
    // "UserService" is exact for "userservice" → 0
    assert_eq!(best_match_tier("UserService", &terms), 0);
    // "IUserService" is exact for "iuserservice" → 0
    assert_eq!(best_match_tier("IUserService", &terms), 0);
}

// ─── matches_ext_filter tests ────────────────────────────────────

#[test]
fn test_matches_ext_filter_single() {
    assert!(matches_ext_filter("src/file.cs", "cs"));
    assert!(!matches_ext_filter("src/file.ts", "cs"));
}

#[test]
fn test_matches_ext_filter_multi() {
    assert!(matches_ext_filter("src/file.cs", "cs,sql"));
    assert!(matches_ext_filter("src/file.sql", "cs,sql"));
    assert!(!matches_ext_filter("src/file.ts", "cs,sql"));
}

#[test]
fn test_matches_ext_filter_case_insensitive() {
    assert!(matches_ext_filter("src/file.CS", "cs"));
    assert!(matches_ext_filter("src/file.cs", "CS"));
}

#[test]
fn test_matches_ext_filter_with_spaces() {
    assert!(matches_ext_filter("src/file.cs", " cs , sql "));
    assert!(matches_ext_filter("src/file.sql", " cs , sql "));
}

#[test]
fn test_matches_ext_filter_no_extension() {
    assert!(!matches_ext_filter("Makefile", "cs"));
}

#[test]
fn test_best_match_tier_prefix_beats_contains() {
    let terms = vec!["user".to_string()];
    // "UserService" starts with "user" → tier 1
    assert_eq!(best_match_tier("UserService", &terms), 1);
    // "IUserService" contains "user" but doesn't start with it → tier 2
    assert_eq!(best_match_tier("IUserService", &terms), 2);
}

// ─── branch_warning tests ─────────────────────────────────────────

/// Helper: create a minimal HandlerContext with a given current_branch.
fn make_ctx_with_branch(branch: Option<&str>) -> HandlerContext {

    use crate::ContentIndex;

    let _index = ContentIndex {
        root: ".".to_string(),
        ..Default::default()
    };
    HandlerContext {
        current_branch: branch.map(|s| s.to_string()),
        ..Default::default()
    }
}

#[test]
fn test_branch_warning_feature_branch() {
    let ctx = make_ctx_with_branch(Some("feature/xyz"));
    let warning = branch_warning(&ctx);
    assert!(warning.is_some());
    let msg = warning.unwrap();
    assert!(msg.contains("feature/xyz"));
    assert!(msg.contains("not on main/master"));
}

#[test]
fn test_branch_warning_main_branch() {
    let ctx = make_ctx_with_branch(Some("main"));
    assert!(branch_warning(&ctx).is_none());
}

#[test]
fn test_branch_warning_master_branch() {
    let ctx = make_ctx_with_branch(Some("master"));
    assert!(branch_warning(&ctx).is_none());
}

#[test]
fn test_branch_warning_none_branch() {
    let ctx = make_ctx_with_branch(None);
    assert!(branch_warning(&ctx).is_none());
}

#[test]
fn test_inject_branch_warning_adds_field() {
    let ctx = make_ctx_with_branch(Some("users/dev/my-feature"));
    let mut summary = json!({"totalFiles": 5});
    inject_branch_warning(&mut summary, &ctx);
    assert!(summary.get("branchWarning").is_some());
    let warning = summary["branchWarning"].as_str().unwrap();
    assert!(warning.contains("users/dev/my-feature"));
}

#[test]
fn test_inject_branch_warning_skips_main() {
    let ctx = make_ctx_with_branch(Some("main"));
    let mut summary = json!({"totalFiles": 5});
    inject_branch_warning(&mut summary, &ctx);
    assert!(summary.get("branchWarning").is_none());
}

#[test]
fn test_inject_branch_warning_skips_none() {
    let ctx = make_ctx_with_branch(None);
    let mut summary = json!({"totalFiles": 5});
    inject_branch_warning(&mut summary, &ctx);
    assert!(summary.get("branchWarning").is_none());
}


// ─── Doc comment scanning tests ─────────────────────────────────────

#[test]
fn test_find_doc_comment_start_csharp_triple_slash() {
    let lines = vec![
        "using System;",           // 0
        "",                         // 1
        "/// <summary>",           // 2
        "/// Validates the order.", // 3
        "/// </summary>",          // 4
        "public bool ValidateOrder(Order order)", // 5
        "{",                        // 6
    ];
    // decl_start_idx = 5 (0-based)
    let result = find_doc_comment_start(&lines, 5);
    assert_eq!(result, 2, "Should find first /// line at index 2");
}

#[test]
fn test_find_doc_comment_start_typescript_jsdoc() {
    let lines = vec![
        "import { Service } from './service';", // 0
        "",                                      // 1
        "/**",                                   // 2
        " * Gets the user by ID.",               // 3
        " * @param id - The user identifier",    // 4
        " * @returns The user model",            // 5
        " */",                                   // 6
        "async function getUser(id: string) {",  // 7
    ];
    let result = find_doc_comment_start(&lines, 7);
    assert_eq!(result, 2, "Should find /** start at index 2");
}

#[test]
fn test_find_doc_comment_start_no_comment() {
    let lines = vec![
        "using System;",       // 0
        "",                     // 1
        "public class Foo {",  // 2
        "    public void Bar()", // 3
    ];
    let result = find_doc_comment_start(&lines, 3);
    assert_eq!(result, 3, "Should return decl_start_idx when no doc comment");
}

#[test]
fn test_find_doc_comment_start_separated_by_code() {
    let lines = vec![
        "/// <summary>Doc for something else</summary>", // 0
        "private int _field;",                            // 1
        "public void Method()",                           // 2
    ];
    let result = find_doc_comment_start(&lines, 2);
    assert_eq!(result, 2, "Should NOT capture comment separated by code line");
}

#[test]
fn test_find_doc_comment_start_with_blank_line_between() {
    let lines = vec![
        "/// <summary>",           // 0
        "/// Doc comment.",        // 1
        "/// </summary>",          // 2
        "",                         // 3 - blank line
        "public void Method()",    // 4
    ];
    let result = find_doc_comment_start(&lines, 4);
    assert_eq!(result, 0, "Should skip blank line and capture doc comment");
}

#[test]
fn test_find_doc_comment_start_rust_doc() {
    let lines = vec![
        "use std::io;",            // 0
        "",                         // 1
        "/// Creates a new instance.", // 2
        "/// With multiple lines.",    // 3
        "pub fn new() -> Self {",      // 4
    ];
    let result = find_doc_comment_start(&lines, 4);
    assert_eq!(result, 2, "Should capture Rust /// doc comments");
}

#[test]
fn test_find_doc_comment_start_at_file_start() {
    let lines = vec![
        "/// Doc at very start.", // 0
        "pub fn first() {}",     // 1
    ];
    let result = find_doc_comment_start(&lines, 1);
    assert_eq!(result, 0, "Should capture doc comment at first line of file");
}

#[test]
fn test_find_doc_comment_start_decl_at_zero() {
    let lines = vec![
        "pub fn first() {}", // 0
    ];
    let result = find_doc_comment_start(&lines, 0);
    assert_eq!(result, 0, "Should return 0 when decl is at line 0");
}

#[test]
fn test_is_doc_comment_line_variants() {
    assert!(is_doc_comment_line("    /// <summary>"));
    assert!(is_doc_comment_line("///"));
    assert!(is_doc_comment_line("/// text"));
    assert!(is_doc_comment_line("    /**"));
    assert!(is_doc_comment_line("     * continuation"));
    assert!(is_doc_comment_line("     */"));
    assert!(is_doc_comment_line("  *"));
    assert!(!is_doc_comment_line("  // regular comment"));
    assert!(!is_doc_comment_line("  public void Method()"));
    assert!(!is_doc_comment_line(""));
    assert!(!is_doc_comment_line("   "));
}

#[test]
fn test_inject_body_with_doc_comments_csharp() {
    // Create a temp file with C# code
    let content = "using System;\n\
                   \n\
                   /// <summary>\n\
                   /// Validates the order.\n\
                   /// </summary>\n\
                   [Authorize]\n\
                   public bool ValidateOrder(Order order)\n\
                   {\n\
                       return true;\n\
                   }\n";
    let dir = std::env::temp_dir().join("search_test_doc_comments_cs");
    let _ = std::fs::create_dir_all(&dir);
    let file_path = dir.join("test.cs");
    std::fs::write(&file_path, content).unwrap();
    let file_str = file_path.to_string_lossy().to_string();

    let mut obj = json!({});
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total = 0usize;

    // line_start=6 (1-based, [Authorize] line), line_end=10
    inject_body_into_obj(
        &mut obj, &file_str, 6, 10, &mut cache, &mut total, 100, 500, true,
        None, None,
    );

    // Body should start at the /// line (line 3, 1-based)
    assert_eq!(obj["bodyStartLine"], 3, "bodyStartLine should be 3 (first /// line)");
    assert_eq!(obj["docCommentLines"], 3, "docCommentLines should be 3");
    let body = obj["body"].as_array().unwrap();
    assert!(body[0].as_str().unwrap().contains("/// <summary>"),
        "First body line should be the doc comment");

    // Cleanup
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_inject_body_without_doc_comments_flag() {
    let content = "/// <summary>\n\
                   /// Doc text.\n\
                   /// </summary>\n\
                   public void Method()\n\
                   {\n\
                   }\n";
    let dir = std::env::temp_dir().join("search_test_no_doc_flag");
    let _ = std::fs::create_dir_all(&dir);
    let file_path = dir.join("test.cs");
    std::fs::write(&file_path, content).unwrap();
    let file_str = file_path.to_string_lossy().to_string();

    let mut obj = json!({});
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total = 0usize;

    // line_start=4, line_end=6, include_doc_comments=false
    inject_body_into_obj(
        &mut obj, &file_str, 4, 6, &mut cache, &mut total, 100, 500, false,
        None, None,
    );

    assert_eq!(obj["bodyStartLine"], 4, "bodyStartLine should be 4 (no expansion)");
    assert!(obj.get("docCommentLines").is_none(), "No docCommentLines when flag is false");
    let body = obj["body"].as_array().unwrap();
    assert!(body[0].as_str().unwrap().contains("public void Method"),
        "First line should be the method declaration");

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── Phase 5a: strip bodies before truncation tests ─────────────

#[test]
fn test_truncate_phase5a_strips_bodies_preserves_signatures() {
    // Build a definitions response with body fields that's over budget
    let mut defs = Vec::new();
    for i in 0..20 {
        let body_lines: Vec<String> = (0..50).map(|j| format!("    code line {} in method {}", j, i)).collect();
        defs.push(json!({
            "name": format!("Method_{}", i),
            "kind": "method",
            "file": format!("file_{}.cs", i),
            "lines": format!("{}-{}", i * 100, i * 100 + 50),
            "parent": "MyClass",
            "body": body_lines,
            "bodyStartLine": i * 100,
            "bodyTruncated": false,
        }));
    }
    let output = json!({
        "definitions": defs,
        "summary": {
            "totalResults": 20,
            "returned": 20,
        }
    });

    let initial_size = serde_json::to_string(&output).unwrap().len();
    // Use a budget small enough to trigger Phase 5a but large enough
    // that stripping bodies alone should suffice (signatures are small)
    let budget = 2000;
    assert!(initial_size > budget, "Test setup: should exceed budget ({} > {})", initial_size, budget);

    let result = truncate_large_response(output, budget);

    // All 20 definitions should be preserved (signatures only)
    let result_defs = result.get("definitions").and_then(|d| d.as_array()).unwrap();
    // If budget is small enough, phase 5b may still truncate some, but should keep MORE than without 5a
    // The key check: remaining entries should NOT have body fields
    for def in result_defs {
        assert!(def.get("body").is_none(),
            "Body should be stripped from definitions entries");
        assert!(def.get("bodyStartLine").is_none(),
            "bodyStartLine should be stripped");
        // Signatures should be preserved
        assert!(def.get("name").is_some(),
            "name (signature) should be preserved");
        assert!(def.get("kind").is_some(),
            "kind (signature) should be preserved");
    }

    // Summary should indicate bodies were stripped
    let summary = result.get("summary").unwrap();
    assert_eq!(summary.get("bodiesStrippedForSize").and_then(|v| v.as_bool()), Some(true),
        "summary.bodiesStrippedForSize should be true");
}

#[test]
fn test_truncate_phase5a_no_body_fields_noop() {
    // Definitions without body fields — Phase 5a should be a no-op
    let mut defs = Vec::new();
    for i in 0..500 {
        defs.push(json!({
            "name": format!("LongDefinitionName_{}", i),
            "kind": "property",
            "file": format!("/some/very/long/path/to/deep/file_{}.cs", i),
            "lines": format!("{}-{}", i * 10, i * 10 + 5),
            "parent": format!("SomeParentClass_{}", i % 50),
        }));
    }
    let output = json!({
        "definitions": defs,
        "summary": {
            "totalResults": 500,
            "returned": 500,
        }
    });

    let initial_size = serde_json::to_string(&output).unwrap().len();
    assert!(initial_size > DEFAULT_MAX_RESPONSE_BYTES, "Test setup: should exceed budget");

    let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);

    // Phase 5a should not have set bodiesStrippedForSize (no bodies to strip)
    let summary = result.get("summary").unwrap();
    assert!(summary.get("bodiesStrippedForSize").is_none(),
        "bodiesStrippedForSize should not be set when no bodies were stripped");

    // Phase 5b should have truncated the array
    let result_defs = result.get("definitions").and_then(|d| d.as_array()).unwrap();
    assert!(result_defs.len() < 500, "Definitions should be truncated by Phase 5b");
}

#[test]
fn test_truncate_phase5a_strips_nested_caller_bodies() {
    // callTree with nested callers that have body fields
    let call_tree = vec![json!({
        "method": "ProcessOrder",
        "class": "OrderService",
        "line": 10,
        "callSite": 25,
        "body": (0..100).map(|i| format!("  line {}", i)).collect::<Vec<_>>(),
        "bodyStartLine": 10,
        "callers": [
            {
                "method": "HandleRequest",
                "class": "Controller",
                "line": 50,
                "callSite": 60,
                "body": (0..100).map(|i| format!("  nested line {}", i)).collect::<Vec<_>>(),
                "bodyStartLine": 50,
            }
        ]
    })];
    let output = json!({
        "callTree": call_tree,
        "summary": { "totalNodes": 2 }
    });

    let budget = 500; // Very small budget to force truncation
    let result = truncate_large_response(output, budget);

    // Body should be stripped from both root and nested callers
    if let Some(tree) = result.get("callTree").and_then(|t| t.as_array()) {
        for node in tree {
            assert!(node.get("body").is_none(), "Root node body should be stripped");
            assert!(node.get("bodyStartLine").is_none(), "Root node bodyStartLine should be stripped");
            if let Some(callers) = node.get("callers").and_then(|c| c.as_array()) {
                for caller in callers {
                    assert!(caller.get("body").is_none(), "Nested caller body should be stripped");
                    assert!(caller.get("bodyStartLine").is_none(), "Nested caller bodyStartLine should be stripped");
                }
            }
            // Method/class metadata should be preserved
            assert!(node.get("method").is_some(), "method should be preserved");
        }
    }
}

#[test]
fn test_inject_body_with_doc_comments_jsdoc() {
    let content = "import { User } from './types';\n\
                   \n\
                   /**\n\
                   * Gets user by ID.\n\
                   * @param id - user ID\n\
                   */\n\
                   export async function getUser(id: string): Promise<User> {\n\
                       return fetch(`/users/${id}`);\n\
                   }\n";
    let dir = std::env::temp_dir().join("search_test_doc_comments_ts");
    let _ = std::fs::create_dir_all(&dir);
    let file_path = dir.join("test.ts");
    std::fs::write(&file_path, content).unwrap();
    let file_str = file_path.to_string_lossy().to_string();

    let mut obj = json!({});
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total = 0usize;

    // line_start=7 (export async function), line_end=9
    inject_body_into_obj(
        &mut obj, &file_str, 7, 9, &mut cache, &mut total, 100, 500, true,
        None, None,
    );

    assert_eq!(obj["bodyStartLine"], 3, "bodyStartLine should be 3 (/** line)");
    assert_eq!(obj["docCommentLines"], 4, "docCommentLines should be 4 (/** through */)");
    let body = obj["body"].as_array().unwrap();
    assert!(body[0].as_str().unwrap().contains("/**"),
        "First body line should be the JSDoc start");

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── bodyLineStart / bodyLineEnd tests ──────────────────────────────

#[test]
fn test_inject_body_body_line_range_filter() {
    // Create a 10-line file
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big.cs");
    let content = (1..=10).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    std::fs::write(&file, &content).unwrap();
    let file_str = file.to_string_lossy().to_string();

    // Method spans lines 3-8 (1-based)
    let mut obj = json!({});
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total = 0usize;

    // Request only lines 5-7 from the body
    inject_body_into_obj(
        &mut obj, &file_str, 3, 8, &mut cache, &mut total, 0, 0, false,
        Some(5), Some(7),
    );

    let body = obj["body"].as_array().unwrap();
    assert_eq!(body.len(), 3, "Should return 3 lines (5,6,7)");
    assert_eq!(body[0].as_str().unwrap(), "line 5");
    assert_eq!(body[1].as_str().unwrap(), "line 6");
    assert_eq!(body[2].as_str().unwrap(), "line 7");
    assert_eq!(obj["bodyStartLine"], 5, "bodyStartLine should be 5 (filtered)");
}

#[test]
fn test_inject_body_body_line_start_only() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big2.cs");
    let content = (1..=10).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    std::fs::write(&file, &content).unwrap();
    let file_str = file.to_string_lossy().to_string();

    // Method spans lines 2-9, request bodyLineStart=6 (no end)
    let mut obj = json!({});
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total = 0usize;

    inject_body_into_obj(
        &mut obj, &file_str, 2, 9, &mut cache, &mut total, 0, 0, false,
        Some(6), None,
    );

    let body = obj["body"].as_array().unwrap();
    assert_eq!(body.len(), 4, "Should return lines 6-9 (4 lines)");
    assert_eq!(body[0].as_str().unwrap(), "line 6");
    assert_eq!(body[3].as_str().unwrap(), "line 9");
    assert_eq!(obj["bodyStartLine"], 6);
}

#[test]
fn test_inject_body_body_line_end_only() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big3.cs");
    let content = (1..=10).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    std::fs::write(&file, &content).unwrap();
    let file_str = file.to_string_lossy().to_string();

    // Method spans lines 2-9, request bodyLineEnd=5 (no start)
    let mut obj = json!({});
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total = 0usize;

    inject_body_into_obj(
        &mut obj, &file_str, 2, 9, &mut cache, &mut total, 0, 0, false,
        None, Some(5),
    );

    let body = obj["body"].as_array().unwrap();
    assert_eq!(body.len(), 4, "Should return lines 2-5 (4 lines)");
    assert_eq!(body[0].as_str().unwrap(), "line 2");
    assert_eq!(body[3].as_str().unwrap(), "line 5");
    assert_eq!(obj["bodyStartLine"], 2);
}

#[test]
fn test_inject_body_body_line_range_outside_method() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big4.cs");
    let content = (1..=10).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    std::fs::write(&file, &content).unwrap();
    let file_str = file.to_string_lossy().to_string();

    // Method spans lines 3-5, request lines 7-9 (completely outside)
    let mut obj = json!({});
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total = 0usize;

    inject_body_into_obj(
        &mut obj, &file_str, 3, 5, &mut cache, &mut total, 0, 0, false,
        Some(7), Some(9),
    );

    let body = obj["body"].as_array().unwrap();
    assert_eq!(body.len(), 0, "Should return empty body when range is outside method");
}

#[test]
fn test_inject_body_body_line_range_with_none_is_full_body() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big5.cs");
    let content = (1..=10).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    std::fs::write(&file, &content).unwrap();
    let file_str = file.to_string_lossy().to_string();

    // Method spans lines 3-8, no body line filter
    let mut obj = json!({});
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut total = 0usize;

    inject_body_into_obj(
        &mut obj, &file_str, 3, 8, &mut cache, &mut total, 0, 0, false,
        None, None,
    );

    let body = obj["body"].as_array().unwrap();
    assert_eq!(body.len(), 6, "Should return full body (lines 3-8)");
    assert_eq!(body[0].as_str().unwrap(), "line 3");
    assert_eq!(body[5].as_str().unwrap(), "line 8");
}
