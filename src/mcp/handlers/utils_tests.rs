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
    // Use a real temp directory so canonicalize works on all platforms
    let tmp = tempfile::tempdir().unwrap();
    let dir_str = tmp.path().to_string_lossy().to_string();
    let result = validate_search_dir(&dir_str, &dir_str);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn test_validate_search_dir_outside_rejects() {
    // Use two real sibling temp directories — neither is a subdirectory of the other
    let parent = std::env::temp_dir();
    let dir_a = parent.join(format!("xray_vsd_a_{}_{}", std::process::id(),
        std::sync::atomic::AtomicU64::new(0).fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let dir_b = parent.join(format!("xray_vsd_b_{}_{}", std::process::id(),
        std::sync::atomic::AtomicU64::new(0).fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let result = validate_search_dir(
        &dir_a.to_string_lossy(),
        &dir_b.to_string_lossy(),
    );
    assert!(result.is_err(), "Sibling directories should be rejected");
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
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
    let result = build_line_content_from_matches(content, &match_lines, 5);
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
fn test_truncate_preserves_capped_line_content_preview_when_budget_allows() {
    let output = json!({
        "files": [{
            "path": "/path/file.cs",
            "lines": (1..=120).collect::<Vec<_>>(),
            "lineContent": [{
                "startLine": 1,
                "lines": (0..120)
                    .map(|j| format!("    some long preview source line {:03} with enough text to exceed the original budget", j))
                    .collect::<Vec<_>>(),
                "matchIndices": (0..120).collect::<Vec<_>>(),
            }],
        }],
        "summary": {"totalFiles": 1}
    });

    let budget = DEFAULT_MAX_RESPONSE_BYTES / 2;
    let initial_size = serde_json::to_string(&output).unwrap().len();
    assert!(initial_size > budget, "test response must start over budget");

    let result = truncate_large_response(output, budget);
    let files = result.get("files").and_then(|f| f.as_array()).unwrap();
    let file = &files[0];

    assert!(file.get("lineContent").is_some(), "lineContent preview should be preserved");
    assert!(file.get("lineContentOmitted").is_none(), "lineContent should not be dropped when capped preview fits");
    assert!(file["lineContentLinesOmitted"].as_u64().unwrap() > 0,
        "lineContentLinesOmitted should record the preview cap");

    let line_content = file["lineContent"].as_array().unwrap();
    let preview_lines = line_content[0]["lines"].as_array().unwrap();
    assert!(preview_lines.len() <= MAX_LINE_CONTENT_LINES_PER_FILE,
        "lineContent preview should be capped to {} source lines", MAX_LINE_CONTENT_LINES_PER_FILE);
    let match_indices = line_content[0]["matchIndices"].as_array().unwrap();
    assert!(match_indices.len() <= MAX_LINES_PER_FILE,
        "lineContent match indices should stay aligned with capped match lines");
}

#[test]
fn test_truncate_removes_line_content_only_when_capped_preview_exceeds_budget() {
    let output = json!({
        "files": [{
            "path": "/path/file.cs",
            "lines": (1..=200).collect::<Vec<_>>(),
            "lineContent": [{
                "startLine": 1,
                "lines": (0..200).map(|_| "x".repeat(1024)).collect::<Vec<_>>(),
                "matchIndices": (0..200).collect::<Vec<_>>(),
            }],
        }],
        "summary": {"totalFiles": 1}
    });

    let result = truncate_large_response(output, 1024);
    let files = result.get("files").and_then(|f| f.as_array()).unwrap();
    let file = &files[0];

    assert!(file.get("lineContent").is_none(),
        "lineContent should be removed only as a final fallback");
    assert_eq!(file["lineContentOmitted"], true);
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
fn test_inject_response_guidance_creates_summary() {
    let result = ToolCallResult::success(r#"{"files":[]}"#.to_string());
    let ctx = HandlerContext::default();
    let result = inject_response_guidance(result, "xray_grep", "rs, md", &ctx);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output.get("summary").is_some());
    let reminder = output["summary"]["policyReminder"].as_str().unwrap();
    assert!(reminder.contains("XRAY_POLICY"), "should contain policy header");
    assert!(reminder.contains("[rs, md]"), "should list indexed extensions in VIOLATION clause");
    assert!(reminder.contains("REQUIRED:"), "should use imperative REQUIRED directive");
    assert!(output["summary"]["nextStepHint"].as_str().is_some());
}

#[test]
fn test_inject_response_guidance_preserves_existing_next_step_hint() {
    let result = ToolCallResult::success(r#"{"files":[],"summary":{"nextStepHint":"custom"}}"#.to_string());
    let ctx = HandlerContext::default();
    let result = inject_response_guidance(result, "xray_grep", "rs, md", &ctx);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(output["summary"]["nextStepHint"].as_str(), Some("custom"));
    assert!(output["summary"]["policyReminder"].as_str().is_some());
}


#[test]
fn test_inject_response_guidance_skips_non_json_success() {
    let result = ToolCallResult::success("plain text".to_string());
    let ctx = HandlerContext::default();
    let result = inject_response_guidance(result, "xray_grep", "rs, md", &ctx);
    assert_eq!(result.content[0].text, "plain text");
}

#[test]
fn test_inject_response_guidance_empty_ext() {
    let result = ToolCallResult::success(r#"{"files":[]}"#.to_string());
    let ctx = HandlerContext::default();
    let result = inject_response_guidance(result, "xray_grep", "", &ctx);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let reminder = output["summary"]["policyReminder"].as_str().unwrap();
    assert!(reminder.contains("XRAY_POLICY"), "should contain policy header");
    assert!(!reminder.contains("Indexed extensions"), "should NOT contain extensions when empty");
}

#[test]
fn test_build_policy_reminder_with_extensions() {
    let reminder = build_policy_reminder("rs, md");
    // New imperative framing: VIOLATION = calling built-in on indexed extensions
    assert!(reminder.contains("VIOLATION = calling built-in"),
        "policyReminder should frame built-in calls as VIOLATION");
    assert!(reminder.contains("[rs, md]"),
        "policyReminder should list the indexed extensions in VIOLATION clause");
    assert!(reminder.contains("REQUIRED:"),
        "policyReminder should use REQUIRED (imperative), not 'prefer'");
    assert!(reminder.contains("xray_definitions (read)"),
        "policyReminder should name the required read tool");
    assert!(reminder.contains("xray_grep (search)"),
        "policyReminder should name the required search tool");
    assert!(reminder.contains("xray_edit (edit)"),
        "policyReminder should name the required edit tool");
    assert!(reminder.contains("NO EXCEPTIONS"),
        "policyReminder should close rationalization lanes with NO EXCEPTIONS");
    assert!(reminder.contains("protocol error"),
        "policyReminder should frame violations as protocol errors");
}

#[test]
fn test_build_policy_reminder_empty_ext() {
    let reminder = build_policy_reminder("");
    // Empty ext -> no VIOLATION line (no indexed extensions to protect)
    assert!(!reminder.contains("VIOLATION"), "empty ext should not emit VIOLATION line");
    assert!(reminder.contains("XRAY_POLICY"));
    assert!(reminder.contains("ENFORCEMENT"), "policyReminder should use ENFORCEMENT framing");
}

#[test]
fn test_build_policy_reminder_whitespace_only_ext() {
    let reminder = build_policy_reminder("  ");
    // Whitespace-only ext is treated as empty -> no VIOLATION line
    assert!(!reminder.contains("VIOLATION"),
        "whitespace-only ext should not emit VIOLATION line");
    assert!(reminder.contains("XRAY_POLICY"));
}

/// The policyReminder embedded in every successful MCP response must contain
/// a compact INTENT -> TOOL mapping. This provides re-entrancy of the tool
/// selection rules between tool calls (vs only the system-prompt instructions,
/// which the model may "forget" as context grows).
#[test]
fn test_build_policy_reminder_has_intent_oneliner() {
    let reminder = build_policy_reminder("rs,md,ps1");
    assert!(reminder.contains("INTENT->TOOL"),
        "policyReminder should contain INTENT->TOOL oneliner. Got: {}", reminder);
    assert!(reminder.contains("context-around-match->xray_grep showLines"),
        "INTENT->TOOL oneliner should map context-around-match to xray_grep showLines");
    assert!(reminder.contains("read-method-body->xray_definitions includeBody"),
        "INTENT->TOOL oneliner should map read-method-body to xray_definitions includeBody");
    assert!(reminder.contains("replace-in-files->xray_edit"),
        "INTENT->TOOL oneliner should map replace-in-files to xray_edit");
    assert!(reminder.contains("list-dir->xray_fast dirsOnly"),
        "INTENT->TOOL oneliner should map list-dir to xray_fast dirsOnly");
    assert!(reminder.contains("stack-trace (file:line)->xray_definitions containsLine"),
        "INTENT->TOOL oneliner should map stack-trace to xray_definitions containsLine");
}

/// The INTENT->TOOL oneliner should be present regardless of whether
/// indexed extensions are configured (it references tool names, not extensions).
#[test]
fn test_build_policy_reminder_intent_oneliner_without_extensions() {
    let reminder = build_policy_reminder("");
    assert!(reminder.contains("INTENT->TOOL"),
        "policyReminder should contain INTENT->TOOL even when no indexed extensions are configured");
}

#[test]
fn test_build_policy_reminder_is_imperative() {
    // Part 4 (2026-04-17): policyReminder must use imperative enforcement framing,
    // not passive wording. This test guards against regression to 'Prefer xray...'
    // style that was shown (by meta-analysis during Part 4 authoring) to tolerate
    // built-in tool fallback.
    let reminder = build_policy_reminder("rs,md,ps1");

    // Must contain imperative directives
    assert!(reminder.contains("REQUIRED:"),
        "policyReminder must use REQUIRED (imperative), not 'prefer' (passive)");
    assert!(reminder.contains("NO EXCEPTIONS"),
        "policyReminder must close rationalization lanes with NO EXCEPTIONS");
    assert!(reminder.contains("STOP"),
        "policyReminder must contain STOP action verb for pre-call self-audit");
    assert!(reminder.contains("protocol error"),
        "policyReminder must frame violations as protocol errors");
    assert!(reminder.contains("ENFORCEMENT"),
        "policyReminder header must use ENFORCEMENT framing");

    // Must NOT contain the old passive wording that was replaced
    assert!(!reminder.contains("Prefer xray"),
        "policyReminder must not revert to passive 'Prefer xray' wording");
    assert!(!reminder.contains("Check xray applicability"),
        "policyReminder must not revert to passive 'Check applicability' wording");
    assert!(!reminder.contains("with explicit justification"),
        "policyReminder must not revert to the rationalization-friendly 'with explicit justification' clause");
}


#[test]
fn test_truncate_definitions_array() {
    // Build a xray_definitions-style response with many definitions — way over budget
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

#[test]
fn test_truncate_preserves_guidance_fields() {
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
        "summary": {
            "totalFiles": 1000,
            "totalOccurrences": 1000,
            "policyReminder": "policy",
            "nextStepHint": "hint"
        }
    });

    let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
    let summary = result.get("summary").unwrap();
    assert_eq!(summary.get("policyReminder").and_then(|v| v.as_str()), Some("policy"));
    assert_eq!(summary.get("nextStepHint").and_then(|v| v.as_str()), Some("hint"));
}

/// Regression: a slow lineRegex scan emits `summary.perfHint` in
/// `build_grep_base_summary`, and a large response also triggers
/// `truncate_large_response` which writes its own `summary.hint`. The two
/// fields MUST coexist — overloading the same key would silently swallow the
/// perf guidance exactly when it matters most (broad/slow lineRegex queries
/// are the responses most likely to trip the byte cap). See user-story
/// `xray-grep-lineRegex-perf-hints_2026-04-26.md` AC-1 + commit-reviewer
/// finding #1 (2026-04-26).
#[test]
fn test_truncate_preserves_perf_hint_distinct_from_truncation_hint() {
    let mut files = Vec::new();
    for i in 0..1000 {
        files.push(json!({
            "path": format!("/some/long/path/to/file_{}.cs", i),
            "score": 0.001,
            "occurrences": 1,
            "lines": [1],
        }));
    }
    let perf_hint_text = "lineRegex took 5000ms over 60000 candidate files (no trigram prefilter ...)";
    let output = json!({
        "files": files,
        "summary": {
            "totalFiles": 1000,
            "totalOccurrences": 1000,
            "perfHint": perf_hint_text,
        }
    });

    let result = truncate_large_response(output, DEFAULT_MAX_RESPONSE_BYTES);
    let summary = result.get("summary").unwrap();
    assert_eq!(
        summary.get("perfHint").and_then(|v| v.as_str()),
        Some(perf_hint_text),
        "perfHint must survive truncation; the truncation pass writes summary.hint and \
         must not collide with the lineRegex perf field"
    );
    // Sanity: truncation's own hint is also present (proves both fields coexist).
    assert!(summary.get("hint").is_some(),
        "truncation should still set its own summary.hint");
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

// ─── looks_like_file_path tests ────────────────────────────────────────

#[test]
fn test_looks_like_file_path_rust_file() {
    assert!(looks_like_file_path("src/main.rs"));
    assert!(looks_like_file_path("C:/Repos/project/src/lib.rs"));
}

#[test]
fn test_looks_like_file_path_various_extensions() {
    assert!(looks_like_file_path("file.cs"));
    assert!(looks_like_file_path("file.ts"));
    assert!(looks_like_file_path("file.py"));
    assert!(looks_like_file_path("file.json"));
    assert!(looks_like_file_path("file.xml"));
    assert!(looks_like_file_path("file.sql"));
    assert!(looks_like_file_path("file.md"));
    assert!(looks_like_file_path("file.toml"));
    assert!(looks_like_file_path("file.yaml"));
    assert!(looks_like_file_path("file.config"));
    assert!(looks_like_file_path("file.csproj"));
}

#[test]
fn test_looks_like_file_path_case_insensitive() {
    assert!(looks_like_file_path("file.RS"));
    assert!(looks_like_file_path("file.Json"));
    assert!(looks_like_file_path("file.CS"));
}

#[test]
fn test_looks_like_file_path_directories_return_false() {
    assert!(!looks_like_file_path("src/definitions"));
    assert!(!looks_like_file_path("C:/Repos/project"));
    assert!(!looks_like_file_path("some/path/without/ext"));
}

#[test]
fn test_looks_like_file_path_unknown_extension_returns_false() {
    assert!(!looks_like_file_path("file.xyz"));
}

// ─── ExcludePatterns tests ─────────────────────────────────────────────

/// Helper: normalize path (lowercase + forward slashes) as production code does
fn normalize_path(p: &str) -> String {
    p.to_lowercase().replace('\\', "/")
}

#[test]
fn test_exclude_patterns_segment_match() {
    let excl = vec!["test".to_string()];
    let patterns = ExcludePatterns::from_dirs(&excl);
    assert!(patterns.matches(&normalize_path("src/test/Service.cs")));
    assert!(patterns.matches(&normalize_path("test/Service.cs")));
}

#[test]
fn test_exclude_patterns_not_substring() {
    // "test" should NOT match "contest" (substring but not segment)
    let excl = vec!["test".to_string()];
    let patterns = ExcludePatterns::from_dirs(&excl);
    assert!(!patterns.matches(&normalize_path("src/contest/Service.cs")));
    assert!(!patterns.matches(&normalize_path("src/latest/file.rs")));
}

#[test]
fn test_exclude_patterns_backslash_paths() {
    // Windows paths with backslashes — normalize_path converts to forward slashes
    let excl = vec!["test".to_string()];
    let patterns = ExcludePatterns::from_dirs(&excl);
    assert!(patterns.matches(&normalize_path("src\\test\\Service.cs")));
    assert!(!patterns.matches(&normalize_path("src\\contest\\Service.cs")));
}

#[test]
fn test_exclude_patterns_case_insensitive() {
    let excl = vec!["Test".to_string()];
    let patterns = ExcludePatterns::from_dirs(&excl);
    assert!(patterns.matches(&normalize_path("src/test/Service.cs")));
    assert!(patterns.matches(&normalize_path("src/TEST/Service.cs")));
}

#[test]
fn test_exclude_patterns_empty() {
    let excl: Vec<String> = vec![];
    let patterns = ExcludePatterns::from_dirs(&excl);
    assert!(patterns.is_empty());
    assert!(!patterns.matches(&normalize_path("src/test/Service.cs")));
}

#[test]
fn test_exclude_patterns_multiple() {
    let excl = vec!["test".to_string(), "mock".to_string()];
    let patterns = ExcludePatterns::from_dirs(&excl);
    assert!(patterns.matches(&normalize_path("src/test/Service.cs")));
    assert!(patterns.matches(&normalize_path("src/mock/Helper.cs")));
    assert!(!patterns.matches(&normalize_path("src/main/Service.cs")));
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

// ─── Unit tests for extracted phase functions ──────────────────

#[test]
fn test_phase_measure_json_size() {
    let v = json!({"key": "value"});
    let size = measure_json_size(&v);
    assert!(size > 0, "measure_json_size should return positive size");
    assert_eq!(size, serde_json::to_string(&v).unwrap().len());
}

#[test]
fn test_phase_cap_lines_per_file_truncates() {
    let many_lines: Vec<u32> = (1..=200).collect();
    let mut output = json!({
        "files": [
            {
                "path": "a.cs",
                "lines": many_lines,
                "lineContent": [{
                    "startLine": 1,
                    "lines": (0..200).map(|j| format!("code line {}", j)).collect::<Vec<_>>(),
                    "matchIndices": (0..200).collect::<Vec<_>>(),
                }]
            }
        ],
        "summary": {"totalFiles": 1}
    });
    let mut reasons = Vec::new();

    phase_cap_lines_per_file(&mut output, &mut reasons);

    let files = output["files"].as_array().unwrap();
    let lines = files[0]["lines"].as_array().unwrap();
    assert!(lines.len() <= MAX_LINES_PER_FILE,
        "Lines should be capped to {}, got {}", MAX_LINES_PER_FILE, lines.len());
    assert!(files[0]["linesOmitted"].as_u64().unwrap() > 0,
        "linesOmitted should be set");
    assert!(files[0].get("lineContent").is_some(),
        "lineContent should be preserved as a capped preview");
    assert!(files[0].get("lineContentOmitted").is_none(),
        "lineContentOmitted should only be set when preview is fully removed");
    assert!(files[0]["lineContentLinesOmitted"].as_u64().unwrap() > 0,
        "lineContentLinesOmitted should be set");
    let preview_lines = files[0]["lineContent"][0]["lines"].as_array().unwrap();
    assert!(preview_lines.len() <= MAX_LINE_CONTENT_LINES_PER_FILE,
        "lineContent preview should be capped");
    assert!(!reasons.is_empty(), "Should add a reason");
}

#[test]
fn test_phase_cap_lines_per_file_truncates_line_content_across_groups() {
    let many_lines: Vec<u32> = (1..=200).collect();
    let mut output = json!({
        "files": [{
            "path": "a.cs",
            "lines": many_lines,
            "lineContent": [
                {
                    "startLine": 1,
                    "lines": (0..30).map(|j| format!("group 1 line {}", j)).collect::<Vec<_>>(),
                    "matchIndices": [0, 10, 20],
                },
                {
                    "startLine": 100,
                    "lines": (0..30).map(|j| format!("group 2 line {}", j)).collect::<Vec<_>>(),
                    "matchIndices": [0, 10, 20],
                },
                {
                    "startLine": 200,
                    "lines": (0..30).map(|j| format!("group 3 line {}", j)).collect::<Vec<_>>(),
                    "matchIndices": [0, 10, 20],
                }
            ]
        }],
        "summary": {"totalFiles": 1}
    });
    let mut reasons = Vec::new();

    phase_cap_lines_per_file(&mut output, &mut reasons);

    let file = &output["files"][0];
    let line_content = file["lineContent"].as_array().unwrap();
    assert_eq!(line_content.len(), 2, "third preview group should be dropped");
    assert_eq!(line_content[0]["lines"].as_array().unwrap().len(), 30);
    assert_eq!(line_content[1]["lines"].as_array().unwrap().len(), 20,
        "second preview group should be partially kept");
    assert_eq!(line_content[1]["matchIndices"], json!([0, 10]),
        "match indices beyond the kept source lines should be pruned");
    assert_eq!(file["lineContentLinesOmitted"], json!(40));
    assert!(file.get("lineContentOmitted").is_none());
    assert!(!reasons.is_empty(), "Should add a reason");
}

#[test]
fn test_phase_cap_lines_per_file_noop_when_small() {
    let mut output = json!({
        "files": [{"path": "a.cs", "lines": [1, 2, 3]}],
        "summary": {"totalFiles": 1}
    });
    let mut reasons = Vec::new();

    phase_cap_lines_per_file(&mut output, &mut reasons);

    let files = output["files"].as_array().unwrap();
    assert_eq!(files[0]["lines"].as_array().unwrap().len(), 3,
        "Small lines array should be unchanged");
    assert!(files[0].get("linesOmitted").is_none(),
        "linesOmitted should not be set for small arrays");
}

#[test]
fn test_phase_cap_matched_tokens_truncates() {
    let many_tokens: Vec<String> = (0..500).map(|i| format!("token_{}", i)).collect();
    let mut output = json!({
        "files": [],
        "summary": {"totalFiles": 0, "matchedTokens": many_tokens}
    });
    let mut reasons = Vec::new();

    phase_cap_matched_tokens(&mut output, &mut reasons);

    let tokens = output["summary"]["matchedTokens"].as_array().unwrap();
    assert!(tokens.len() <= MAX_MATCHED_TOKENS,
        "matchedTokens should be capped to {}, got {}", MAX_MATCHED_TOKENS, tokens.len());
    assert!(output["summary"]["matchedTokensOmitted"].as_u64().unwrap() > 0);
    assert!(!reasons.is_empty());
}

#[test]
fn test_phase_cap_matched_tokens_noop_when_small() {
    let mut output = json!({
        "summary": {"matchedTokens": ["a", "b"]}
    });
    let mut reasons = Vec::new();

    phase_cap_matched_tokens(&mut output, &mut reasons);

    let tokens = output["summary"]["matchedTokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 2);
    assert!(reasons.is_empty(), "Should not add a reason when no truncation");
}

#[test]
fn test_phase_remove_lines_arrays() {
    let mut output = json!({
        "files": [
            {"path": "a.cs", "lines": [1, 2, 3]},
            {"path": "b.cs", "lines": [4, 5]}
        ],
        "summary": {"totalFiles": 2}
    });
    let mut reasons = Vec::new();

    phase_remove_lines_arrays(&mut output, &mut reasons);

    let files = output["files"].as_array().unwrap();
    for file in files {
        assert!(file.get("lines").is_none(), "lines array should be removed");
    }
    assert!(!reasons.is_empty());
}

#[test]
fn test_phase_reduce_file_count() {
    let mut files = Vec::new();
    for i in 0..100 {
        files.push(json!({"path": format!("file_{}.cs", i), "score": 0.1}));
    }
    let mut output = json!({
        "files": files,
        "summary": {"totalFiles": 100}
    });
    let mut reasons = Vec::new();

    // Use a small budget so files must be reduced
    phase_reduce_file_count(&mut output, 500, &mut reasons);

    let result_files = output["files"].as_array().unwrap();
    assert!(result_files.len() < 100,
        "File count should be reduced from 100, got {}", result_files.len());
    assert!(!result_files.is_empty(), "Should keep at least 1 file");
    assert!(!reasons.is_empty());
}

#[test]
fn test_phase_strip_body_fields_strips_definitions() {
    let mut output = json!({
        "definitions": [
            {
                "name": "Method1",
                "kind": "method",
                "body": ["line1", "line2"],
                "bodyStartLine": 10,
                "bodyTruncated": false,
                "totalBodyLines": 2,
                "docCommentLines": 0
            }
        ],
        "summary": {"returned": 1}
    });
    let mut reasons = Vec::new();

    phase_strip_body_fields(&mut output, &mut reasons);

    let defs = output["definitions"].as_array().unwrap();
    assert!(defs[0].get("body").is_none(), "body should be stripped");
    assert!(defs[0].get("bodyStartLine").is_none(), "bodyStartLine should be stripped");
    assert!(defs[0].get("bodyTruncated").is_none(), "bodyTruncated should be stripped");
    assert!(defs[0].get("totalBodyLines").is_none(), "totalBodyLines should be stripped");
    assert!(defs[0].get("docCommentLines").is_none(), "docCommentLines should be stripped");
    assert!(defs[0].get("name").is_some(), "name should be preserved");
    assert!(defs[0].get("kind").is_some(), "kind should be preserved");
    assert!(!reasons.is_empty());
    assert_eq!(output["summary"]["bodiesStrippedForSize"], true);
}

#[test]
fn test_phase_strip_body_fields_noop_without_bodies() {
    let mut output = json!({
        "definitions": [
            {"name": "Method1", "kind": "method"}
        ],
        "summary": {"returned": 1}
    });
    let mut reasons = Vec::new();

    phase_strip_body_fields(&mut output, &mut reasons);

    assert!(reasons.is_empty(), "Should not add a reason when no bodies stripped");
    assert!(output["summary"].get("bodiesStrippedForSize").is_none());
}

#[test]
fn test_phase_truncate_largest_array_definitions() {
    let mut defs = Vec::new();
    for i in 0..100 {
        defs.push(json!({"name": format!("Def_{}", i), "kind": "method"}));
    }
    let mut output = json!({
        "definitions": defs,
        "summary": {"returned": 100}
    });
    let mut reasons = Vec::new();

    // Budget smaller than total size
    phase_truncate_largest_array(&mut output, 500, &mut reasons);

    let result_defs = output["definitions"].as_array().unwrap();
    assert!(result_defs.len() < 100,
        "Definitions should be truncated, got {}", result_defs.len());
    assert!(!reasons.is_empty());
    assert!(output["summary"]["returned"].as_u64().is_some(),
        "summary.returned should be updated");
}

#[test]
fn test_phase_truncate_largest_array_noop_when_under_budget() {
    let mut output = json!({
        "definitions": [{"name": "A"}],
        "summary": {"returned": 1}
    });
    let mut reasons = Vec::new();

    // Very large budget — no truncation needed
    phase_truncate_largest_array(&mut output, 100_000, &mut reasons);

    assert_eq!(output["definitions"].as_array().unwrap().len(), 1);
    assert!(reasons.is_empty(), "Should not add reason when under budget");
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

// ─── PERF-08: ensure_file_index single-flight gate tests ────────────

mod ensure_file_index_tests {
    use super::super::ensure_file_index;
    use crate::FileIndex;
    use crate::mcp::handlers::HandlerContext;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    /// Build a fresh `FileIndex` with no entries — cheap, suitable for
    /// the single-flight tests where the *count* of build invocations
    /// is the only thing under test, not the index contents themselves.
    fn empty_index(root: &str) -> FileIndex {
        FileIndex {
            root: root.to_string(),
            format_version: crate::FILE_INDEX_VERSION,
            created_at: 0,
            max_age_secs: 0,
            entries: Vec::new(),
            respect_git_exclude: false,
        }
    }

    /// PERF-08 AC #1 — single-flight on cold start under contention.
    /// Spawn 16 threads against a brand-new (`file_index = None`,
    /// `dirty = true`) `HandlerContext`. They all race into
    /// `ensure_file_index` simultaneously. Exactly one of them must
    /// run the build closure; the other 15 must observe the freshly
    /// built index without invoking `build_fn`. This pins the
    /// single-flight contract — without the gate, every thread would
    /// see `needs_rebuild=true` and run a parallel walk + 8 MB
    /// allocation + on-disk save.
    ///
    /// A `Barrier` is used to maximise the race window, and a small
    /// `sleep` inside the build closure widens it further so that
    /// even on a fast machine the late waiters reliably arrive while
    /// the slot is taken.
    #[test]
    fn test_ensure_file_index_single_flight_cold_start_16_threads() {
        let ctx = Arc::new(HandlerContext::default());
        let build_count = Arc::new(AtomicU64::new(0));
        let barrier = Arc::new(Barrier::new(16));

        let mut handles = Vec::with_capacity(16);
        for _ in 0..16 {
            let ctx = ctx.clone();
            let build_count = build_count.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                ensure_file_index(&ctx, || {
                    build_count.fetch_add(1, Ordering::SeqCst);
                    // Hold the slot long enough for late waiters to arrive
                    // and find `building=true` — without this, fast machines
                    // could let each thread complete in turn and the
                    // single-flight semantics would never be exercised.
                    thread::sleep(Duration::from_millis(50));
                    Ok(empty_index("/tmp/perf08-cold"))
                })
            }));
        }
        for h in handles {
            h.join().expect("worker thread panicked").expect("ensure_file_index failed");
        }

        let actual = build_count.load(Ordering::SeqCst);
        assert_eq!(
            actual, 1,
            "PERF-08 single-flight: expected exactly 1 build invocation under 16-thread contention, got {}",
            actual
        );
        assert!(
            ctx.file_index.read().unwrap().is_some(),
            "file_index must be populated after the cold-start race"
        );
        assert!(
            !ctx.file_index_dirty.load(Ordering::Relaxed),
            "dirty flag must be cleared after a successful build"
        );
    }

    /// PERF-08 AC #2 — exactly one extra build per dirty signal.
    ///
    /// Sequence:
    /// 1. Cold-start race with 16 threads → 1 build (the cold one).
    /// 2. Watcher signals invalidation by setting `file_index_dirty=true`.
    /// 3. 16 fresh threads race again → exactly 1 *additional* build.
    ///
    /// Total build count must be 2, proving the gate correctly resets
    /// after each invalidation cycle (rejecting the `OnceCell` design
    /// which lacks reset semantics).
    #[test]
    fn test_ensure_file_index_single_flight_after_dirty_signal() {
        let ctx = Arc::new(HandlerContext::default());
        let build_count = Arc::new(AtomicU64::new(0));

        // Phase 1: cold-start race
        let barrier1 = Arc::new(Barrier::new(16));
        let mut handles = Vec::with_capacity(16);
        for _ in 0..16 {
            let ctx = ctx.clone();
            let build_count = build_count.clone();
            let barrier = barrier1.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                ensure_file_index(&ctx, || {
                    build_count.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(30));
                    Ok(empty_index("/tmp/perf08-dirty-1"))
                })
            }));
        }
        for h in handles {
            h.join().unwrap().unwrap();
        }
        assert_eq!(build_count.load(Ordering::SeqCst), 1, "phase 1: cold build must run exactly once");

        // Phase 2: simulate watcher invalidation
        ctx.file_index_dirty.store(true, Ordering::Relaxed);

        // Phase 3: rebuild race
        let barrier2 = Arc::new(Barrier::new(16));
        let mut handles = Vec::with_capacity(16);
        for _ in 0..16 {
            let ctx = ctx.clone();
            let build_count = build_count.clone();
            let barrier = barrier2.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                ensure_file_index(&ctx, || {
                    build_count.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(30));
                    Ok(empty_index("/tmp/perf08-dirty-2"))
                })
            }));
        }
        for h in handles {
            h.join().unwrap().unwrap();
        }
        assert_eq!(
            build_count.load(Ordering::SeqCst),
            2,
            "PERF-08 reset semantics: cold build (1) + one rebuild after dirty signal (1) = 2 total; \
             extra builds would mean the gate failed to suppress the second-wave race"
        );
    }

    /// PERF-08 AC #3 — panic recovery.
    ///
    /// If the first build closure panics, the RAII guard inside the gate
    /// must clear `building=false` and `notify_all` waiters so the next
    /// caller can take over the slot — otherwise the gate would deadlock
    /// permanently after any build failure that unwinds.
    ///
    /// We trigger a panic on the first call, catch it via
    /// `catch_unwind`, then verify a fresh call succeeds and the index
    /// is populated.
    #[test]
    fn test_ensure_file_index_recovers_after_builder_panic() {
        let ctx = Arc::new(HandlerContext::default());
        let attempt = Arc::new(Mutex::new(0u32));

        let attempt_clone = attempt.clone();
        let ctx_clone = ctx.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ensure_file_index(&ctx_clone, || {
                *attempt_clone.lock().unwrap() += 1;
                panic!("PERF-08 simulated builder panic");
            })
        }));
        assert!(result.is_err(), "first call must propagate the panic");
        assert_eq!(*attempt.lock().unwrap(), 1, "panicking closure ran once");
        // Gate must be released even though build_fn unwound.
        assert!(
            !*ctx.file_index_build_gate.building.lock().unwrap(),
            "RAII guard must reset building=false after a panic — otherwise next caller deadlocks"
        );
        // Index is still missing, dirty is still effectively true.
        assert!(
            ctx.file_index.read().unwrap().is_none(),
            "panicking build must not have published an index"
        );

        // Second call must succeed (gate accepted a new builder).
        ensure_file_index(&ctx, || {
            *attempt.lock().unwrap() += 1;
            Ok(empty_index("/tmp/perf08-recovery"))
        })
        .expect("second call must succeed after panic recovery");
        assert_eq!(*attempt.lock().unwrap(), 2, "recovery build ran exactly once");
        assert!(
            ctx.file_index.read().unwrap().is_some(),
            "recovery call must populate the index"
        );
    }

    /// Defensive guard: when the index is already fresh
    /// (`file_index = Some`, `dirty = false`), `ensure_file_index`
    /// must return immediately without invoking `build_fn`. Without
    /// this fast-path the gate would defeat the whole purpose of the
    /// in-memory cache.
    #[test]
    fn test_ensure_file_index_skips_build_when_fresh() {
        let ctx = Arc::new(HandlerContext::default());
        // Pre-populate as if a previous build already ran.
        *ctx.file_index.write().unwrap() = Some(empty_index("/tmp/perf08-fresh"));
        ctx.file_index_dirty.store(false, Ordering::Relaxed);

        let build_count = Arc::new(AtomicU64::new(0));
        let bc = build_count.clone();
        ensure_file_index(&ctx, || {
            bc.fetch_add(1, Ordering::SeqCst);
            Ok(empty_index("/tmp/perf08-should-not-run"))
        })
        .expect("ensure_file_index must succeed");
        assert_eq!(
            build_count.load(Ordering::SeqCst),
            0,
            "build closure must NOT run when the index is fresh and not dirty"
        );
    }

    /// PERF-08 follow-up regression — mid-build invalidation must NOT be lost.
    ///
    /// Pre-fix the builder did `file_index_dirty.store(false)` *after*
    /// publishing the index. If a watcher set `dirty=true` *during* the
    /// in-flight build, that signal was unconditionally erased: a single
    /// AtomicBool slot, builder writes `false` last → the next caller
    /// observed `dirty=false, file_index=Some` and **skipped** the
    /// rebuild despite a stale snapshot. Post-fix the builder swaps
    /// `dirty=false` *before* running `build_fn`, so any signal arriving
    /// during the build is preserved as a fresh `true` and triggers one
    /// more rebuild on the next call.
    ///
    /// We exercise this by injecting `dirty=true` from inside the build
    /// closure (the simplest way to model "watcher signal arriving while
    /// the build is still running"). After the first call returns, dirty
    /// MUST be observable as `true` so the next caller rebuilds.
    #[test]
    fn test_ensure_file_index_preserves_mid_build_dirty_signal() {
        let ctx = Arc::new(HandlerContext::default());
        let build_count = Arc::new(AtomicU64::new(0));

        // First build: simulate a watcher signal landing mid-build.
        let bc = build_count.clone();
        let ctx2 = ctx.clone();
        ensure_file_index(&ctx, || {
            bc.fetch_add(1, Ordering::SeqCst);
            // Watcher signals invalidation *during* the build.
            ctx2.file_index_dirty.store(true, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(10));
            Ok(empty_index("/tmp/perf08-mid-build-dirty"))
        })
        .unwrap();

        assert!(
            ctx.file_index_dirty.load(Ordering::Relaxed),
            "mid-build watcher signal MUST survive the builder's pre-build dirty clear — \
             pre-fix the unconditional `store(false)` *after* publish erased this signal, \
             leaving the cache stale until the next watcher event"
        );

        // Second call: dirty was preserved, so build_fn must run again.
        let bc = build_count.clone();
        ensure_file_index(&ctx, || {
            bc.fetch_add(1, Ordering::SeqCst);
            Ok(empty_index("/tmp/perf08-mid-build-dirty-2"))
        })
        .unwrap();
        assert_eq!(
            build_count.load(Ordering::SeqCst),
            2,
            "second call MUST rebuild because mid-build dirty signal was preserved"
        );
        assert!(
            !ctx.file_index_dirty.load(Ordering::Relaxed),
            "after the second build (no further mid-build signal) dirty must be back to false"
        );
    }

    /// PERF-08 follow-up regression — failed build restores dirty flag.
    ///
    /// The pre-build dirty swap means a failed build that returns
    /// `Err(...)` must put `dirty` back to its pre-build value,
    /// otherwise the next caller could see `dirty=false,
    /// file_index=None` and the distinction between 'never built' and
    /// 'built and stale' would collapse. The `DirtyRestoreGuard` RAII
    /// type guarantees restoration on `?` / panic / any non-success exit.
    #[test]
    fn test_ensure_file_index_restores_dirty_on_build_error() {
        let ctx = Arc::new(HandlerContext::default());
        // Cold start: dirty starts true via Default. Confirm this assumption
        // explicitly so the test fails loudly if the default ever changes.
        assert!(
            ctx.file_index_dirty.load(Ordering::Relaxed),
            "HandlerContext::default() must start with dirty=true"
        );

        let result = ensure_file_index(&ctx, || {
            Err::<crate::FileIndex, _>("PERF-08 simulated build failure".to_string())
        });
        assert!(result.is_err(), "build error must propagate");

        assert!(
            ctx.file_index_dirty.load(Ordering::Relaxed),
            "DirtyRestoreGuard must put dirty=true back after a failed build, otherwise \
             a subsequent caller could observe `dirty=false, file_index=None` and the \
             distinction between 'never built' and 'built and stale' would collapse"
        );
        assert!(
            ctx.file_index.read().unwrap().is_none(),
            "failed build must not have published any index"
        );
    }

}


// ─── name_similarity tests ─────────────────────────────────────────

#[test]
fn test_name_similarity_identical() {
    let score = name_similarity("handle_search", "handle_search");
    assert!((score - 1.0).abs() < 0.001, "Identical strings should have score 1.0, got {}", score);
}

#[test]
fn test_name_similarity_completely_different() {
    let score = name_similarity("abc", "xyz");
    assert!(score < 0.3, "Completely different strings should score low, got {}", score);
}

#[test]
fn test_name_similarity_partial_match() {
    let score = name_similarity("handle_xray_callers", "handle_xray_definitions");
    assert!(score > 0.5, "Partial match should score above 0.5, got {}", score);
    assert!(score < 1.0, "Partial match should not be 1.0, got {}", score);
}

#[test]
fn test_name_similarity_typo() {
    let score = name_similarity("userservice", "userservise");
    assert!(score > 0.8, "Typo should have high similarity, got {}", score);
}


// ─── resolve_dir_to_absolute tests ────────────────────────────────────────

#[test]
fn test_resolve_dir_to_absolute_absolute_path_passthrough() {
    // Create a real absolute temp dir so canonicalize works on Windows
    let tmp = tempfile::tempdir().unwrap();
    let abs_path = tmp.path().to_string_lossy().to_string();
    let result = resolve_dir_to_absolute(&abs_path, "/server/dir");
    // Absolute path should not be prefixed with server_dir
    assert!(!result.contains("server"), "Result '{}' should not contain server_dir", result);
    // Should contain the original path components
    let result_norm = result.replace('\\', "/").to_lowercase();
    let abs_norm = abs_path.replace('\\', "/").to_lowercase();
    assert!(result_norm.contains(&abs_norm.trim_start_matches("\\\\?\\").to_string())
        || result_norm == abs_norm,
        "Result '{}' should match input '{}'", result, abs_path);
}

#[test]
fn test_resolve_dir_to_absolute_relative_path_resolved() {
    // Create a real temp directory structure so canonicalize works
    let base = tempfile::tempdir().unwrap();
    let sub = base.path().join("subdir").join("nested");
    std::fs::create_dir_all(&sub).unwrap();

    let base_str = base.path().to_string_lossy().to_string();
    let result = resolve_dir_to_absolute("subdir/nested", &base_str);

    // Should be resolved to absolute path containing the base + subdir
    let result_norm = result.replace('\\', "/").to_lowercase();
    let base_norm = base_str.replace('\\', "/").to_lowercase();
    assert!(result_norm.contains(&base_norm.trim_end_matches('/').to_string()),
        "Result '{}' should contain server_dir '{}'", result, base_str);
    assert!(result_norm.contains("subdir/nested") || result_norm.contains("subdir\\nested"),
        "Result '{}' should contain 'subdir/nested'", result);
}

#[test]
fn test_resolve_dir_to_absolute_dot_path() {
    let result = resolve_dir_to_absolute(".", "/server/dir");
    assert_eq!(result, "/server/dir");
}

#[test]
fn test_resolve_dir_to_absolute_nonexistent_relative_fallback() {
    // When canonicalize fails (path doesn't exist), should still return resolved string
    let result = resolve_dir_to_absolute("nonexistent/path", "/server/dir");
    assert!(result.contains("server/dir"), "Should contain server_dir");
    assert!(result.contains("nonexistent/path"), "Should contain relative path");
}

#[test]
fn test_resolve_dir_to_absolute_windows_backslashes() {
    let result = resolve_dir_to_absolute("sub\\nested", "/server/dir");
    // Should normalize backslashes
    assert!(result.contains("sub") && result.contains("nested"));
}

// ─── validate_search_dir with relative paths ────────────────────────────────

#[test]
fn test_validate_search_dir_relative_subdir_resolved() {
    // Create a real directory structure
    let base = tempfile::tempdir().unwrap();
    let sub = base.path().join("src").join("models");
    std::fs::create_dir_all(&sub).unwrap();

    let base_str = base.path().to_string_lossy().to_string();
    let result = validate_search_dir("src/models", &base_str);

    assert!(result.is_ok(), "Relative subdir should be accepted, got: {:?}", result);
    // Should return Some(canonical_path) since it's a proper subdirectory
    assert!(result.unwrap().is_some(), "Should return Some(subdir_filter)");
}

#[test]
fn test_validate_search_dir_relative_nonexistent_accepted_as_subdir() {
    let base = tempfile::tempdir().unwrap();
    let base_str = base.path().to_string_lossy().to_string();

    // Non-existent relative path — resolves to base_str/nonexistent/dir
    // This is WITHIN server_dir, so it should be accepted (returns Ok(Some(...)))
    // The directory doesn't exist, but validate_search_dir only checks path prefix
    let result = validate_search_dir("nonexistent/dir", &base_str);
    assert!(result.is_ok(), "Relative path within server_dir should be accepted: {:?}", result);
    assert!(result.unwrap().is_some(), "Should return Some(subdir_filter)");
}

// ─────────────────────────────────────────────────────────────────────
// Symlink-aware regression test for `validate_search_dir`.
//
// Bug: validate_search_dir used to canonicalize the requested dir before
// comparing against server_dir. For a symlinked subdirectory like
// `<root>/personal -> D:\Personal`, canonicalize resolved `personal` to
// `D:\Personal`, which does NOT start with `<root>`, so the validator
// returned an error refusing the search request. After the fix, the helper
// uses logical-path comparison (matching what the indexer sees via
// `WalkBuilder::follow_links`) and accepts the request.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn test_validate_search_dir_through_symlinked_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let external = tmp.path().join("external");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("note.md"), "x").unwrap();

    // root/personal -> external (the docs/personal pattern)
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&external, root.join("personal")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, root.join("personal")).unwrap();

    let symlinked_subdir = root.join("personal").to_string_lossy().to_string();
    let root_str = root.to_string_lossy().to_string();

    let result = validate_search_dir(&symlinked_subdir, &root_str);
    assert!(
        result.is_ok(),
        "Symlinked subdir must be accepted (regression for `docs/personal` use case). \
         Got error: {:?}",
        result
    );
    let filter = result.unwrap();
    assert!(
        filter.is_some(),
        "Symlinked subdir should produce a Some(subdir_filter), not None."
    );
    // The returned filter must reflect the LOGICAL path (under root), not the
    // canonicalized symlink target — otherwise downstream filters would not
    // match indexed entries.
    let filter_str = filter.unwrap().to_lowercase().replace('\\', "/");
    let expected = symlinked_subdir.to_lowercase().replace('\\', "/");
    assert_eq!(
        filter_str.trim_end_matches('/'),
        expected.trim_end_matches('/'),
        "Returned subdir filter must be the logical path, not the symlink target."
    );
}



// ─────────────────────────────────────────────────────────────────────
// Phase 2 regression: `resolve_dir_to_absolute` must NEVER call canonicalize.
// Symlinked subdirectories (e.g. docs/personal -> D:\Personal\…) must
// resolve to the LOGICAL path under server_dir, not to the symlink target.
// Pre-fix the function called `std::fs::canonicalize`, returning the
// external target — which then mismatched indexed entries (recorded by
// `WalkBuilder::follow_links` as logical paths under server_dir) and
// downstream filters/validations silently produced wrong results.
// ─────────────────────────────────────────────────────────────────────
#[test]
fn test_resolve_dir_to_absolute_through_symlinked_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let external = tmp.path().join("external");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("note.md"), "x").unwrap();

    // root/personal -> external (the docs/personal pattern)
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&external, root.join("personal")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, root.join("personal")).unwrap();

    let symlinked_subdir = root.join("personal").to_string_lossy().to_string();
    let root_str = root.to_string_lossy().to_string();
    let external_str = external.to_string_lossy().to_string();

    // Call as ABSOLUTE input — exercises the absolute-path branch which
    // previously canonicalized to the symlink target.
    let resolved_abs = resolve_dir_to_absolute(&symlinked_subdir, &root_str);

    let resolved_norm = resolved_abs.to_lowercase().replace('\\', "/");
    let expected_norm = symlinked_subdir.to_lowercase().replace('\\', "/");
    let external_norm = external_str.to_lowercase().replace('\\', "/");

    assert_eq!(
        resolved_norm.trim_end_matches('/'),
        expected_norm.trim_end_matches('/'),
        "resolve_dir_to_absolute MUST return the LOGICAL path under server_dir, \
         not the symlink target. Got '{}', expected '{}'.",
        resolved_abs, symlinked_subdir
    );
    assert!(
        !resolved_norm.contains(external_norm.trim_end_matches('/').trim_start_matches("/")),
        "Returned path must NOT contain the symlink target ('{}'). Got: '{}'",
        external_str, resolved_abs
    );

    // Call as RELATIVE input — exercises the relative-path branch which
    // previously canonicalized the joined path to the symlink target.
    let resolved_rel = resolve_dir_to_absolute("personal", &root_str);
    let rel_norm = resolved_rel.to_lowercase().replace('\\', "/");
    let expected_rel_norm = format!(
        "{}/personal",
        root_str.to_lowercase().replace('\\', "/").trim_end_matches('/')
    );
    assert_eq!(
        rel_norm.trim_end_matches('/'),
        expected_rel_norm.trim_end_matches('/'),
        "Relative input must be joined to server_dir as logical text — got '{}', expected '{}'.",
        resolved_rel, expected_rel_norm
    );
}

// ─── §4.2 read_string_array / read_kind_array (2026-04-25 migration) ────

#[test]
fn read_string_array_absent_returns_empty() {
    let args = json!({});
    assert_eq!(read_string_array(&args, "terms").unwrap(), Vec::<String>::new());
}

#[test]
fn read_string_array_null_returns_empty() {
    let args = json!({ "terms": null });
    assert_eq!(read_string_array(&args, "terms").unwrap(), Vec::<String>::new());
}

#[test]
fn read_string_array_happy_path() {
    let args = json!({ "terms": ["foo", "bar"] });
    assert_eq!(
        read_string_array(&args, "terms").unwrap(),
        vec!["foo".to_string(), "bar".to_string()]
    );
}

#[test]
fn read_string_array_skips_empty_preserves_inner_whitespace() {
    // Whitespace-only entries are skipped (treated as absent), but inner
    // leading/trailing whitespace inside non-empty entries is PRESERVED —
    // it is significant for regex patterns (e.g. `"^## "` differs
    // semantically from `"^##"`).
    let args = json!({ "terms": ["  foo  ", "", "   ", "bar"] });
    assert_eq!(
        read_string_array(&args, "terms").unwrap(),
        vec!["  foo  ".to_string(), "bar".to_string()]
    );
}

#[test]
fn read_string_array_preserves_literal_comma_in_entry() {
    // Core motivation for the migration: literal `,` inside an entry must
    // survive untouched (regex CSV patterns, log prefixes, etc.).
    let args = json!({ "terms": ["^[^,]+,[^,]+$"] });
    assert_eq!(
        read_string_array(&args, "terms").unwrap(),
        vec!["^[^,]+,[^,]+$".to_string()]
    );
}

#[test]
fn read_string_array_rejects_string_form_with_actionable_message() {
    let args = json!({ "terms": "a,b" });
    let err = read_string_array(&args, "terms").unwrap_err();
    assert!(err.contains("array of strings"), "err = {err}");
    assert!(err.contains("[\"a\",\"b\"]"), "err = {err}");
    assert!(err.contains("'terms'"), "err = {err}");
}

#[test]
fn read_string_array_rejects_number() {
    let args = json!({ "terms": 42 });
    let err = read_string_array(&args, "terms").unwrap_err();
    assert!(err.contains("array of strings"), "err = {err}");
    assert!(err.contains("number"), "err = {err}");
}

#[test]
fn read_string_array_rejects_non_string_element() {
    let args = json!({ "terms": ["foo", 7, "bar"] });
    let err = read_string_array(&args, "terms").unwrap_err();
    assert!(err.contains("element [1]"), "err = {err}");
    assert!(err.contains("number"), "err = {err}");
}

#[test]
fn read_kind_array_accepts_known_kinds() {
    let args = json!({ "kind": ["class", "interface", "enum"] });
    assert_eq!(
        read_kind_array(&args).unwrap(),
        vec!["class".to_string(), "interface".to_string(), "enum".to_string()]
    );
}

#[test]
fn read_kind_array_rejects_unknown_kind() {
    let args = json!({ "kind": ["class", "banana"] });
    let err = read_kind_array(&args).unwrap_err();
    assert!(err.contains("banana"), "err = {err}");
    assert!(err.contains("Valid values"), "err = {err}");
}

#[test]
fn read_kind_array_empty_when_absent() {
    let args = json!({});
    assert_eq!(read_kind_array(&args).unwrap(), Vec::<String>::new());
}

#[test]
fn read_kind_array_inherits_string_form_rejection() {
    let args = json!({ "kind": "class,interface" });
    let err = read_kind_array(&args).unwrap_err();
    assert!(err.contains("array of strings"), "err = {err}");
}

// ─── read_string / read_required_string / read_enum_string_* ───────────
//
// Coverage for the 2026-04-25 strict-typing migration of OPTIONAL and REQUIRED
// single-string MCP parameters. Symmetric to `read_string_array` above.
//
// What we pin:
//  * happy path  (string -> Some / String / default)
//  * absence    (None | Null -> Ok(None) for read_string,
//                              Err("Missing") for read_required_string,
//                              default for read_enum_string_with_default)
//  * empty / whitespace -> hard Err (NOT silently absent — see
//                                    `read_string` doc comment)
//  * wrong type (Array, Number, Bool, Object) -> Err naming the type
//  * enum non-member -> Err with `must be one of` + `'<key>'`
//
// The empty-string-is-Err contract is non-negotiable: it is the whole point of
// the migration (close the "silent filter dropped" failure mode). If a future
// caller adds back "empty == absent" coercion, these tests fail loudly.

#[test]
fn read_string_returns_none_when_missing() {
    let args = json!({});
    assert_eq!(read_string(&args, "class").unwrap(), None);
}

#[test]
fn read_string_returns_none_when_null() {
    let args = json!({ "class": null });
    assert_eq!(read_string(&args, "class").unwrap(), None);
}

#[test]
fn read_string_returns_value_when_string() {
    let args = json!({ "class": "UserService" });
    assert_eq!(read_string(&args, "class").unwrap(), Some("UserService".to_string()));
}

#[test]
fn read_string_preserves_internal_whitespace() {
    // Symmetric with `read_string_array` element-skip: only outer trim()
    // governs absent-vs-present; internal whitespace is significant.
    let args = json!({ "class": "  My Service  " });
    assert_eq!(read_string(&args, "class").unwrap(),
        Some("  My Service  ".to_string()));
}

#[test]
fn read_string_rejects_empty_string() {
    let args = json!({ "class": "" });
    let err = read_string(&args, "class").unwrap_err();
    assert!(err.contains("'class'"), "err = {err}");
    assert!(err.contains("non-empty"), "err = {err}");
    assert!(err.contains("2026-04-25"), "err = {err}");
}

#[test]
fn read_string_rejects_whitespace_only() {
    let args = json!({ "class": "   \t  " });
    let err = read_string(&args, "class").unwrap_err();
    assert!(err.contains("non-empty"), "err = {err}");
}

#[test]
fn read_string_rejects_array() {
    let args = json!({ "class": ["UserService"] });
    let err = read_string(&args, "class").unwrap_err();
    assert!(err.contains("'class'"), "err = {err}");
    assert!(err.contains("must be a string"), "err = {err}");
    assert!(err.contains("array"), "err = {err}");
    assert!(err.contains("2026-04-25"), "err = {err}");
}

#[test]
fn read_string_rejects_number() {
    let args = json!({ "class": 42 });
    let err = read_string(&args, "class").unwrap_err();
    assert!(err.contains("must be a string"), "err = {err}");
    assert!(err.contains("number"), "err = {err}");
}

#[test]
fn read_string_rejects_bool() {
    let args = json!({ "class": true });
    let err = read_string(&args, "class").unwrap_err();
    assert!(err.contains("boolean"), "err = {err}");
}

#[test]
fn read_string_rejects_object() {
    let args = json!({ "class": {"name": "X"} });
    let err = read_string(&args, "class").unwrap_err();
    assert!(err.contains("object"), "err = {err}");
}

#[test]
fn read_required_string_returns_value() {
    let args = json!({ "repo": "." });
    assert_eq!(read_required_string(&args, "repo").unwrap(), ".".to_string());
}

#[test]
fn read_required_string_missing_yields_clear_error() {
    let args = json!({});
    let err = read_required_string(&args, "repo").unwrap_err();
    assert!(err.contains("Missing required parameter"), "err = {err}");
    assert!(err.contains("'repo'"), "err = {err}");
}

#[test]
fn read_required_string_null_yields_clear_error() {
    let args = json!({ "repo": null });
    let err = read_required_string(&args, "repo").unwrap_err();
    assert!(err.contains("Missing required parameter"), "err = {err}");
}

#[test]
fn read_required_string_wrong_type_distinguishes_from_missing() {
    // The whole point of `read_required_string` over the legacy
    // `args.get(K).and_then(|v| v.as_str()).ok_or("Missing required")`:
    // wrong-type input must NOT show up as "missing".
    let args = json!({ "repo": ["."] });
    let err = read_required_string(&args, "repo").unwrap_err();
    assert!(!err.contains("Missing required"),
        "wrong-type input must NOT collapse to Missing: err = {err}");
    assert!(err.contains("must be a string"), "err = {err}");
    assert!(err.contains("array"), "err = {err}");
}

#[test]
fn read_required_string_rejects_empty() {
    let args = json!({ "repo": "" });
    let err = read_required_string(&args, "repo").unwrap_err();
    // Empty string is rejected by `read_string` first; either error shape
    // ("non-empty" or "Missing required") is acceptable as long as the key
    // is named and the call returns Err.
    assert!(err.contains("'repo'"), "err = {err}");
}

#[test]
fn read_enum_string_with_default_returns_default_when_absent() {
    let args = json!({});
    assert_eq!(
        read_enum_string_with_default(&args, "direction", &["up", "down"], "up").unwrap(),
        "up".to_string()
    );
}

#[test]
fn read_enum_string_with_default_accepts_member() {
    let args = json!({ "direction": "down" });
    assert_eq!(
        read_enum_string_with_default(&args, "direction", &["up", "down"], "up").unwrap(),
        "down".to_string()
    );
}

#[test]
fn read_enum_string_with_default_is_case_insensitive() {
    let args = json!({ "direction": "DOWN" });
    let v = read_enum_string_with_default(&args, "direction", &["up", "down"], "up").unwrap();
    // Returns the user-supplied casing; downstream callers .to_lowercase() if needed.
    assert_eq!(v, "DOWN".to_string());
}

#[test]
fn read_enum_string_with_default_rejects_non_member() {
    let args = json!({ "direction": "sideways" });
    let err = read_enum_string_with_default(&args, "direction", &["up", "down"], "up").unwrap_err();
    assert!(err.contains("'direction'"), "err = {err}");
    assert!(err.contains("must be one of"), "err = {err}");
}

#[test]
fn read_enum_string_with_default_rejects_wrong_type() {
    let args = json!({ "direction": ["up"] });
    let err = read_enum_string_with_default(&args, "direction", &["up", "down"], "up").unwrap_err();
    assert!(err.contains("must be a string"), "err = {err}");
}

#[test]
fn read_enum_string_opt_returns_none_when_absent() {
    let args = json!({});
    assert_eq!(
        read_enum_string_opt(&args, "sortBy", &["lines", "callCount"]).unwrap(),
        None
    );
}

#[test]
fn read_enum_string_opt_accepts_member() {
    let args = json!({ "sortBy": "lines" });
    assert_eq!(
        read_enum_string_opt(&args, "sortBy", &["lines", "callCount"]).unwrap(),
        Some("lines".to_string())
    );
}

#[test]
fn read_enum_string_opt_rejects_non_member() {
    let args = json!({ "sortBy": "chaos" });
    let err = read_enum_string_opt(&args, "sortBy", &["lines", "callCount"]).unwrap_err();
    assert!(err.contains("'sortBy'"), "err = {err}");
    assert!(err.contains("must be one of"), "err = {err}");
}

// ─── Drift-guards for the const-slice enum domains ────────────────────
//
// `direction`, `mode`, `sortBy` are not backed by Rust enums (they are bare
// strings inside the JSON request). The compile-time exhaustive-match guard
// used for `kind` (DefinitionKind <-> ALL_KINDS) is therefore not available.
// These runtime drift-guards pin the slice contents AND verify each entry
// flows through the matching downstream branch — if a developer adds a
// variant to ALL_X without wiring up the consumer, these fire.

mod drift_guards {
    use crate::mcp::handlers::callers::ALL_DIRECTIONS;
    use crate::mcp::handlers::definitions::ALL_SORT_FIELDS;
    use crate::mcp::handlers::grep::ALL_GREP_MODES;

    #[test]
    fn all_directions_pinned_to_up_down() {
        assert_eq!(ALL_DIRECTIONS, &["up", "down"],
            "ALL_DIRECTIONS drift: handler `direction` branches assume exactly up/down. \
             If you add a value, audit handle_xray_callers + handle_multi_method_callers.");
    }

    #[test]
    fn all_grep_modes_pinned_to_or_and() {
        assert_eq!(ALL_GREP_MODES, &["or", "and"],
            "ALL_GREP_MODES drift: handler `mode_and` consumer assumes binary or/and. \
             If you add a value, audit handle_xray_grep term-combining logic.");
    }

    #[test]
    fn all_sort_fields_validated_via_read_enum_string_opt() {
        // Drift-guard: every entry in ALL_SORT_FIELDS must round-trip cleanly
        // through `read_enum_string_opt` against the live `ALL_SORT_FIELDS`
        // constant, and a non-member string must produce the new
        // "must be one of [...]" error containing the full slice.
        //
        // NOTE: this guard validates the parser-level contract only — it does
        // NOT prove that every entry is wired into the downstream sort
        // function (`get_sort_value` in definitions.rs). Adding a new field
        // to `ALL_SORT_FIELDS` without a matching `get_sort_value` arm would
        // PASS this test but silently degrade the new field to a constant
        // sort key at runtime. Tracking item: see
        // `docs/user-stories/todo_2026-04-25_xray-edit-and-response-ux.md`
        // (out-of-band) and the broader sort-field coverage gap.
        use serde_json::json;
        // Drift-guard: exercise read_enum_string_opt with each field of
        // ALL_SORT_FIELDS so adding a value (or renaming one) without
        // updating the validator is caught at compile-time-of-tests.
        for &field in ALL_SORT_FIELDS {
            let args = json!({ "sortBy": field });
            let r = super::super::read_enum_string_opt(&args, "sortBy", ALL_SORT_FIELDS);
            assert!(r.is_ok(), "ALL_SORT_FIELDS entry {field:?} must parse OK");
            assert_eq!(r.unwrap(), Some(field.to_string()));
        }
        let args = json!({ "sortBy": "definitelyNotASortField" });
        let err = super::super::read_enum_string_opt(&args, "sortBy", ALL_SORT_FIELDS).unwrap_err();
        for &field in ALL_SORT_FIELDS {
            assert!(err.contains(field), "error must enumerate slice; missing {field:?}: {err}");
        }
    }
}

