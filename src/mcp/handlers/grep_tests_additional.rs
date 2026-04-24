#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use super::*;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

fn make_params_default<'a>() -> GrepSearchParams<'a> {
    GrepSearchParams {
        ext_filter: &None,
        show_lines: false,
        context_lines: 0,
        max_results: 50,
        mode_and: false,
        count_only: false,
        search_start: Instant::now(),
        dir_filter: &None,
        file_filter: &None,
        exclude_patterns: super::utils::ExcludePatterns::from_dirs(&[]),
        exclude_lower: vec![],
        dir_auto_converted_note: None,
        exact_file_path: &None,
        auto_balance: true,
        max_occurrences_per_term: None,
    }
}

// ─── auto_switch_to_phrase_if_needed tests ──────────────────────

#[test]
fn test_auto_switch_no_special_chars_returns_none() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["hello".to_string(), "world".to_string()];
    let result = auto_switch_to_phrase_if_needed(&ctx, &index, "hello,world", &raw_terms, &params);
    assert!(result.is_none(), "Should return None when no terms contain spaces or punctuation");
}

#[test]
fn test_auto_switch_with_spaces_returns_some() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["create procedure".to_string()];
    let result = auto_switch_to_phrase_if_needed(&ctx, &index, "CREATE PROCEDURE", &raw_terms, &params);
    assert!(result.is_some(), "Should return Some when terms contain spaces");
}

#[test]
fn test_auto_switch_with_punctuation_returns_some() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["#[cfg(test)]".to_string()];
    let result = auto_switch_to_phrase_if_needed(&ctx, &index, "#[cfg(test)]", &raw_terms, &params);
    assert!(result.is_some(), "Should return Some when terms contain punctuation like #[cfg(test)]");
}

#[test]
fn test_auto_switch_with_angle_brackets_returns_some() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["<summary>".to_string()];
    let result = auto_switch_to_phrase_if_needed(&ctx, &index, "<summary>", &raw_terms, &params);
    assert!(result.is_some(), "Should return Some when terms contain angle brackets");
}

#[test]
fn test_auto_switch_underscore_only_returns_none() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["my_variable".to_string()];
    let result = auto_switch_to_phrase_if_needed(&ctx, &index, "my_variable", &raw_terms, &params);
    assert!(result.is_none(), "Should NOT auto-switch for underscores (they are valid in tokens)");
}

// ─── has_non_token_chars tests ──────────────────────────────────

#[test]
fn test_has_non_token_chars_alphanumeric() {
    assert!(!has_non_token_chars("hello123"));
}

#[test]
fn test_has_non_token_chars_underscore() {
    assert!(!has_non_token_chars("my_var_123"));
}

#[test]
fn test_has_non_token_chars_brackets() {
    assert!(has_non_token_chars("#[cfg(test)]"));
}

#[test]
fn test_has_non_token_chars_dot() {
    assert!(has_non_token_chars("System.IO"));
}

#[test]
fn test_has_non_token_chars_at_sign() {
    assert!(has_non_token_chars("@Attribute"));
}

#[test]
fn test_has_non_token_chars_angle_brackets() {
    assert!(has_non_token_chars("<summary>"));
}

// ─── score_token_postings tests ─────────────────────────────────

#[test]
fn test_score_token_postings_basic() {
    use crate::Posting;
    let mut index = ContentIndex::default();
    index.files = vec!["file1.cs".to_string()];
    index.file_token_counts = vec![100];
    index.index.insert("userservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![10, 20] },
    ]);

    let params = make_params_default();
    let mut tokens_with_hits = HashSet::new();
    let mut file_scores = HashMap::new();
    let mut file_matched_terms = HashMap::new();

    score_token_postings(
        &["userservice".to_string()], 0, &index, &params, 1.0,
        &mut tokens_with_hits, &mut file_scores, &mut file_matched_terms,
    );

    assert!(tokens_with_hits.contains("userservice"));
    assert_eq!(file_scores.len(), 1);
    assert_eq!(file_scores[&0].occurrences, 2);
    assert_eq!(file_matched_terms[&0].len(), 1);
}

#[test]
fn test_score_token_postings_filters_applied() {
    use crate::Posting;
    let mut index = ContentIndex::default();
    index.files = vec!["file1.cs".to_string(), "file2.xml".to_string()];
    index.file_token_counts = vec![100, 50];
    index.index.insert("token".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![1] },
    ]);

    let ext = Some("cs".to_string());
    let params = GrepSearchParams {
        ext_filter: &ext,
        ..make_params_default()
    };

    let mut tokens_with_hits = HashSet::new();
    let mut file_scores = HashMap::new();
    let mut file_matched_terms = HashMap::new();

    score_token_postings(
        &["token".to_string()], 0, &index, &params, 2.0,
        &mut tokens_with_hits, &mut file_scores, &mut file_matched_terms,
    );

    // Only file1.cs should pass (ext filter = cs)
    assert_eq!(file_scores.len(), 1);
    assert!(file_scores.contains_key(&0));
    assert!(!file_scores.contains_key(&1));
}

#[test]
fn test_score_token_postings_multi_term_tracking() {
    use crate::Posting;
    let mut index = ContentIndex::default();
    index.files = vec!["file1.cs".to_string()];
    index.file_token_counts = vec![100];
    index.index.insert("term_a".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
    ]);
    index.index.insert("term_b".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
    ]);

    let params = make_params_default();
    let mut tokens_with_hits = HashSet::new();
    let mut file_scores = HashMap::new();
    let mut file_matched_terms = HashMap::new();

    score_token_postings(
        &["term_a".to_string()], 0, &index, &params, 1.0,
        &mut tokens_with_hits, &mut file_scores, &mut file_matched_terms,
    );
    score_token_postings(
        &["term_b".to_string()], 1, &index, &params, 1.0,
        &mut tokens_with_hits, &mut file_scores, &mut file_matched_terms,
    );

    assert_eq!(file_matched_terms[&0].len(), 2);
    assert!(file_matched_terms[&0].contains(&0));
    assert!(file_matched_terms[&0].contains(&1));
}

// ─── build_substring_response tests ─────────────────────────────

#[test]
fn test_build_substring_response_count_only() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = GrepSearchParams {
        count_only: true,
        ..make_params_default()
    };

    let results = vec![FileScoreEntry {
        file_path: "test.cs".to_string(),
        lines: vec![1, 2],
        tf_idf: 1.0,
        occurrences: 2,
        terms_matched: 1,
        per_term_occurrences: vec![2],
    }];
    let raw_terms = vec!["svc".to_string()];
    let matched_tokens = vec!["userservice".to_string()];
    let warnings = vec!["Short substring query (<4 chars) may return broad results".to_string()];

    let result = build_substring_response(
        &results, &raw_terms, &matched_tokens, &warnings,
        1, 2, "or", &index, &ctx, &params,
        None,
    );
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let summary = &v["summary"];
    // Block A fix: matchedTokens should NOT be present in countOnly mode
    assert!(summary.get("matchedTokens").is_none(),
        "matchedTokens should be absent in countOnly mode (Block A fix)");
    assert!(summary.get("warnings").is_some());
    assert!(v.get("files").is_none());
}

#[test]
fn test_build_substring_response_normal() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();

    let results = vec![FileScoreEntry {
        file_path: "test.cs".to_string(),
        lines: vec![1],
        tf_idf: 0.5,
        occurrences: 1,
        terms_matched: 1,
        per_term_occurrences: vec![1],
    }];
    let raw_terms = vec!["hello".to_string()];
    let matched_tokens = vec!["hello".to_string()];

    let result = build_substring_response(
        &results, &raw_terms, &matched_tokens, &[],
        1, 1, "or", &index, &ctx, &params,
        None,
    );
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("files").is_some());
    assert!(v.get("summary").is_some());
    let summary = &v["summary"];
    assert!(summary.get("matchedTokens").is_some());
    assert!(summary.get("warnings").is_none());
}

// ─── parse_grep_args additional tests ────────────────────────────

#[test]
fn test_parse_grep_args_mode_and() {
    let args = json!({"terms": "hello", "mode": "and", "substring": false});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(result.mode_and);
}

#[test]
fn test_parse_grep_args_count_only() {
    let args = json!({"terms": "hello", "countOnly": true});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(result.count_only);
}

#[test]
fn test_parse_grep_args_show_lines_explicit() {
    let args = json!({"terms": "hello", "showLines": true});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(result.show_lines);
}

// ─── score_normal_token_search with filter test ─────────────────

#[test]
fn test_score_normal_token_search_with_ext_filter() {
    use crate::Posting;
    let mut index = ContentIndex::default();
    index.files = vec!["file1.cs".to_string(), "file2.xml".to_string()];
    index.file_token_counts = vec![100, 50];
    index.index.insert("hello".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![1] },
    ]);

    let ext = Some("cs".to_string());
    let params = GrepSearchParams {
        ext_filter: &ext,
        ..make_params_default()
    };
    let terms = vec!["hello".to_string()];
    let scores = score_normal_token_search(&terms, &index, &params);
    assert_eq!(scores.len(), 1, "Only .cs file should pass filter");
    assert!(scores.contains_key(&0));
}

// ─── merge_phrase_results_or tests ──────────────────────────────────

#[test]
fn test_merge_phrase_or_empty() {
    let result = merge_phrase_results_or(vec![]);
    assert!(result.is_empty(), "Empty input should produce empty output");
}

#[test]
fn test_merge_phrase_or_single_phrase() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1, 5], content: None },
        PhraseFileMatch { file_path: "b.cs".into(), lines: vec![3], content: None },
    ];
    let result = merge_phrase_results_or(vec![phrase1]);
    assert_eq!(result.len(), 2, "Single phrase with 2 files should produce 2 results");
}

#[test]
fn test_merge_phrase_or_disjoint_files() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1], content: None },
        PhraseFileMatch { file_path: "b.cs".into(), lines: vec![2], content: None },
    ];
    let phrase2 = vec![
        PhraseFileMatch { file_path: "c.cs".into(), lines: vec![3], content: None },
        PhraseFileMatch { file_path: "d.cs".into(), lines: vec![4], content: None },
    ];
    let result = merge_phrase_results_or(vec![phrase1, phrase2]);
    assert_eq!(result.len(), 4, "Disjoint files should all appear in union");
}

#[test]
fn test_merge_phrase_or_overlapping_files() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1, 3], content: None },
    ];
    let phrase2 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![2, 5], content: None },
    ];
    let result = merge_phrase_results_or(vec![phrase1, phrase2]);
    assert_eq!(result.len(), 1, "Same file from two phrases should merge to 1");
    let entry = &result[0];
    assert_eq!(entry.file_path, "a.cs");
    assert_eq!(entry.lines, vec![1, 2, 3, 5], "Lines should be merged, sorted, deduped");
}

#[test]
fn test_merge_phrase_or_preserves_content() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1], content: None },
    ];
    let phrase2 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![2], content: Some("file content".into()) },
    ];
    let result = merge_phrase_results_or(vec![phrase1, phrase2]);
    assert_eq!(result.len(), 1);
    // Content from phrase2 should be kept since phrase1 had None
    assert!(result[0].content.is_some(), "Content should be preserved from second phrase");
    assert_eq!(result[0].content.as_deref().unwrap(), "file content");
}

#[test]
fn test_merge_phrase_or_deduplicates_lines() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1, 3, 5], content: None },
    ];
    let phrase2 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![3, 5, 7], content: None },
    ];
    let result = merge_phrase_results_or(vec![phrase1, phrase2]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].lines, vec![1, 3, 5, 7], "Duplicate lines should be removed");
}

// ─── merge_phrase_results_and tests ─────────────────────────────────

#[test]
fn test_merge_phrase_and_empty() {
    let result = merge_phrase_results_and(vec![]);
    assert!(result.is_empty(), "Empty input should produce empty output");
}

#[test]
fn test_merge_phrase_and_single_phrase() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1], content: None },
        PhraseFileMatch { file_path: "b.cs".into(), lines: vec![2], content: None },
    ];
    let result = merge_phrase_results_and(vec![phrase1]);
    assert_eq!(result.len(), 2, "Single phrase should keep all files");
}

#[test]
fn test_merge_phrase_and_no_overlap() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1], content: None },
    ];
    let phrase2 = vec![
        PhraseFileMatch { file_path: "b.cs".into(), lines: vec![2], content: None },
    ];
    let result = merge_phrase_results_and(vec![phrase1, phrase2]);
    assert!(result.is_empty(), "No overlapping files should produce empty result");
}

#[test]
fn test_merge_phrase_and_full_overlap() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1], content: None },
        PhraseFileMatch { file_path: "b.cs".into(), lines: vec![2], content: None },
    ];
    let phrase2 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![5], content: None },
        PhraseFileMatch { file_path: "b.cs".into(), lines: vec![6], content: None },
    ];
    let result = merge_phrase_results_and(vec![phrase1, phrase2]);
    assert_eq!(result.len(), 2, "Both files appear in both phrases → both kept");
}

#[test]
fn test_merge_phrase_and_partial_overlap() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1], content: None },
        PhraseFileMatch { file_path: "b.cs".into(), lines: vec![2], content: None },
        PhraseFileMatch { file_path: "c.cs".into(), lines: vec![3], content: None },
    ];
    let phrase2 = vec![
        PhraseFileMatch { file_path: "b.cs".into(), lines: vec![10], content: None },
        PhraseFileMatch { file_path: "c.cs".into(), lines: vec![20], content: None },
        PhraseFileMatch { file_path: "d.cs".into(), lines: vec![30], content: None },
    ];
    let result = merge_phrase_results_and(vec![phrase1, phrase2]);
    assert_eq!(result.len(), 2, "Only b.cs and c.cs are in both");
    let paths: HashSet<String> = result.iter().map(|r| r.file_path.clone()).collect();
    assert!(paths.contains("b.cs"));
    assert!(paths.contains("c.cs"));
    assert!(!paths.contains("a.cs"));
    assert!(!paths.contains("d.cs"));
}

#[test]
fn test_merge_phrase_and_merges_lines() {
    let phrase1 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![1, 3], content: None },
    ];
    let phrase2 = vec![
        PhraseFileMatch { file_path: "a.cs".into(), lines: vec![2, 3, 5], content: None },
    ];
    let result = merge_phrase_results_and(vec![phrase1, phrase2]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].lines, vec![1, 2, 3, 5], "Lines should be merged, sorted, deduped");
}

// ─── regex + spaces searchModeNote tests ────────────────────────────

#[test]
fn test_regex_with_spaces_produces_search_mode_note() {
    let _index = ContentIndex::default();
    let ctx = HandlerContext::default();

    let result = handle_xray_grep(&ctx, &json!({
        "terms": "private.*double Percentile",
        "regex": true
    }));

    // Should succeed (0 results is fine) and contain searchModeNote
    assert!(!result.is_error, "regex search should succeed even with 0 results");
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    let summary = &parsed["summary"];
    assert!(summary.get("searchModeNote").is_some(),
        "regex=true with spaces in terms should produce searchModeNote");
    let note = summary["searchModeNote"].as_str().unwrap();
    // The hint was updated when `lineRegex` was added — it now mentions alphanumeric+underscore
    // tokens (more accurate than the old "tokens which never contain spaces" wording) AND points
    // to the lineRegex=true escape hatch as the actionable fix.
    assert!(note.contains("alphanumeric+underscore tokens"),
        "searchModeNote should explain the token-vs-line mismatch. Got: {}", note);
    assert!(note.contains("lineRegex=true"),
        "searchModeNote should suggest lineRegex=true as the actionable fix. Got: {}", note);
}

#[test]
fn test_regex_without_spaces_no_search_mode_note() {
    let _index = ContentIndex::default();
    let ctx = HandlerContext::default();

    let result = handle_xray_grep(&ctx, &json!({
        "terms": "I[A-Z]\\w+Cache",
        "regex": true
    }));

    assert!(!result.is_error);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    let summary = &parsed["summary"];
    assert!(summary.get("searchModeNote").is_none(),
        "regex=true without spaces should NOT produce searchModeNote");
}

// ─── inject_grep_ext_hint tests ─────────────────────────────────────

#[test]
fn test_inject_grep_ext_hint_non_indexed_ext_adds_hint() {
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs md".to_string();

    let json_text = r#"{"files":[],"summary":{"totalFiles":0,"totalOccurrences":0}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter = Some("ps1".to_string());

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = output["summary"]["hint"].as_str().unwrap();
    assert!(hint.contains("ps1"), "hint should mention the non-indexed extension");
    assert!(hint.contains("not in content index"), "hint should explain why");
    assert!(hint.contains("read_file"), "hint should suggest alternative");
}

#[test]
fn test_inject_grep_ext_hint_indexed_ext_no_hint() {
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs md".to_string();

    let json_text = r#"{"files":[],"summary":{"totalFiles":0,"totalOccurrences":0}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter = Some("rs".to_string());

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"].get("hint").is_none(),
        "Should NOT add hint when ext IS indexed but just 0 results");
}

#[test]
fn test_inject_grep_ext_hint_no_ext_filter_no_hint() {
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs md".to_string();

    let json_text = r#"{"files":[],"summary":{"totalFiles":0,"totalOccurrences":0}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter: Option<String> = None;

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"].get("hint").is_none(),
        "Should NOT add hint when no ext filter is set");
}

#[test]
fn test_inject_grep_ext_hint_positive_results_no_hint() {
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs md".to_string();

    let json_text = r#"{"files":[{"path":"test.ps1"}],"summary":{"totalFiles":1,"totalOccurrences":2}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter = Some("ps1".to_string());

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"].get("hint").is_none(),
        "Should NOT add hint when there are positive results");
}

#[test]
fn test_inject_grep_ext_hint_mixed_ext_filter() {
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs md".to_string();

    let json_text = r#"{"files":[],"summary":{"totalFiles":0,"totalOccurrences":0}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter = Some("rs,ps1".to_string()); // rs is indexed, ps1 is not

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = output["summary"]["hint"].as_str().unwrap();
    assert!(hint.contains("ps1"), "hint should mention non-indexed ps1");
    assert!(!hint.contains("'rs"), "hint should NOT mention indexed rs");
}


// ─── L25: excludeDir matches by directory segment, not full path substring ──

#[test]
fn test_exclude_dir_matches_segment_not_substring() {
    // L25: excludeDir=["test"] should NOT exclude a file at "contest/file.rs"
    // because "test" is a substring of "contest" but not a directory segment
    let excl = vec!["test".to_string()];
    let params = GrepSearchParams {
        exclude_patterns: super::utils::ExcludePatterns::from_dirs(&excl),
        ..make_params_default()
    };
    // Should NOT be excluded — "contest" is not the same directory segment as "test"
    assert!(passes_file_filters("src/contest/file.rs", &params),
        "excludeDir='test' should NOT exclude 'contest/file.rs' (substring match was wrong)");
    // Should be excluded — "test" is a directory segment
    assert!(!passes_file_filters("src/test/file.rs", &params),
        "excludeDir='test' should exclude 'src/test/file.rs'");
    // Should be excluded — case-insensitive
    assert!(!passes_file_filters("src/Test/file.rs", &params),
        "excludeDir='test' should exclude 'src/Test/file.rs' (case-insensitive)");
    // Should NOT be excluded — "testing" != "test"
    assert!(passes_file_filters("src/testing/file.rs", &params),
        "excludeDir='test' should NOT exclude 'testing/file.rs'");
    // Should be excluded — "test" at the start of path
    assert!(!passes_file_filters("test/file.rs", &params),
        "excludeDir='test' should exclude 'test/file.rs' (at path start)");
}

#[test]
fn test_exclude_dir_matches_backslash_segments() {
    // Windows paths use backslashes
    let excl = vec!["test".to_string()];
    let params = GrepSearchParams {
        exclude_patterns: super::utils::ExcludePatterns::from_dirs(&excl),
        ..make_params_default()
    };
    assert!(!passes_file_filters("src\\test\\file.rs", &params),
        "excludeDir should match backslash-separated segments");
    assert!(passes_file_filters("src\\contest\\file.rs", &params),
        "excludeDir should NOT match substring in backslash paths");
}

// ─── L22/L23: TF-IDF zero guards ───────────────────────────────────

#[test]
fn test_tfidf_zero_file_token_count_no_division_by_zero() {
    // L23: file_token_counts[file_id] = 0 should not cause NaN/Inf in TF-IDF
    let mut index = ContentIndex {
        root: "/test".to_string(),
        extensions: vec!["rs".to_string()],
        files: vec!["src/zero.rs".to_string()],
        file_token_counts: vec![0], // zero token count
        ..Default::default()
    };
    index.index.insert("hello".to_string(), vec![crate::Posting {
        file_id: 0,
        lines: vec![1],
    }]);

    let params = GrepSearchParams {
        ..make_params_default()
    };
    // Should not panic or produce NaN — the guard converts 0 to 1.0
    let results = score_normal_token_search(&["hello".to_string()], &index, &params);
    assert_eq!(results.len(), 1, "Should find one file");
    let entry = results.values().next().unwrap();
    assert!(entry.tf_idf.is_finite(), "TF-IDF should be finite, not NaN/Inf");
}

// ─── intersect_sorted_unique tests (tier-B helper) ──────────────

#[test]
fn test_intersect_sorted_unique_basic_overlap() {
    let a = vec![1u32, 3, 5, 7, 9];
    let b = vec![2u32, 3, 4, 7, 10];
    assert_eq!(intersect_sorted_unique(&a, &b), vec![3, 7]);
}

#[test]
fn test_intersect_sorted_unique_no_overlap() {
    let a = vec![1u32, 2, 3];
    let b = vec![4u32, 5, 6];
    assert!(intersect_sorted_unique(&a, &b).is_empty());
}

#[test]
fn test_intersect_sorted_unique_full_overlap() {
    let a = vec![1u32, 2, 3];
    let b = vec![1u32, 2, 3];
    assert_eq!(intersect_sorted_unique(&a, &b), vec![1, 2, 3]);
}

#[test]
fn test_intersect_sorted_unique_empty_inputs() {
    let empty: Vec<u32> = Vec::new();
    let a = vec![1u32, 2, 3];
    assert!(intersect_sorted_unique(&empty, &a).is_empty());
    assert!(intersect_sorted_unique(&a, &empty).is_empty());
    let empty2: Vec<u32> = Vec::new();
    assert!(intersect_sorted_unique(&empty, &empty2).is_empty());
}

#[test]
fn test_intersect_sorted_unique_subset() {
    // a is a strict subset of b
    let a = vec![5u32, 10, 15];
    let b = vec![1u32, 5, 7, 10, 12, 15, 20];
    assert_eq!(intersect_sorted_unique(&a, &b), vec![5, 10, 15]);
    // and reverse
    assert_eq!(intersect_sorted_unique(&b, &a), vec![5, 10, 15]);
}

#[test]
fn test_intersect_sorted_unique_single_element() {
    assert_eq!(intersect_sorted_unique(&[42u32], &[42u32]), vec![42]);
    assert!(intersect_sorted_unique(&[42u32], &[43u32]).is_empty());
}

#[test]
fn test_intersect_sorted_unique_preserves_order_and_uniqueness() {
    let a = vec![1u32, 2, 4, 8, 16, 32, 64];
    let b = vec![2u32, 4, 16, 64, 128];
    let result = intersect_sorted_unique(&a, &b);
    assert_eq!(result, vec![2, 4, 16, 64]);
    // Verify sortedness + uniqueness invariants
    for w in result.windows(2) {
        assert!(w[0] < w[1], "result must be strictly ascending");
    }
}


// ─── apply_auto_balance tests ───

fn entry(path: &str, tf: f64, occ: usize, per_term: Vec<usize>) -> FileScoreEntry {
    let total: usize = per_term.iter().sum();
    let terms_matched = per_term.iter().filter(|&&v| v > 0).count();
    FileScoreEntry {
        file_path: path.into(),
        lines: vec![1; total.max(occ)],
        tf_idf: tf,
        occurrences: total.max(occ),
        terms_matched,
        per_term_occurrences: per_term,
    }
}

#[test]
fn test_auto_balance_triggers_when_one_term_dominates() {
    // term0 (rare): 5 occurrences in 1 file. term1 (dominant): 100 occurrences
    // across 100 dominant-only files. Ratio = 100/5 = 20 > 10 → trigger.
    let raw = vec!["todo".to_string(), "localstorage".to_string()];
    let mut results = vec![entry("rare.rs", 5.0, 5, vec![5, 0])];
    for i in 0..100 {
        results.push(entry(&format!("dom{i}.rs"), 1.0, 1, vec![0, 1]));
    }
    let original_len = results.len();
    let info = apply_auto_balance(&mut results, 2, &raw, None).expect("should trigger");
    assert_eq!(info.dominant_term, "localstorage");
    assert_eq!(info.dominant_occurrences, 100);
    assert_eq!(info.min_nonzero_occurrences, 5);
    // cap = 2 * second_max (5) clamped [20,100] = 20
    assert_eq!(info.cap, 20);
    assert_eq!(info.dropped_files, 100 - 20);
    assert_eq!(results.len(), original_len - info.dropped_files);
    // The rare file (matched only by rare term) is always kept.
    assert!(results.iter().any(|r| r.file_path == "rare.rs"));
}

#[test]
fn test_auto_balance_skipped_for_single_term() {
    let raw = vec!["foo".to_string()];
    let mut results = vec![
        entry("a.rs", 1.0, 100, vec![100]),
        entry("b.rs", 0.5, 1, vec![1]),
    ];
    let info = apply_auto_balance(&mut results, 1, &raw, None);
    assert!(info.is_none(), "single-term query must not auto-balance");
    assert_eq!(results.len(), 2);
}

#[test]
fn test_auto_balance_skipped_when_ratio_below_threshold() {
    // 9x ratio — below 10x trigger. No balancing.
    let raw = vec!["a".to_string(), "b".to_string()];
    let mut results = vec![
        entry("a.rs", 1.0, 10, vec![10, 0]),
        entry("b.rs", 1.0, 90, vec![0, 90]),
    ];
    let info = apply_auto_balance(&mut results, 2, &raw, None);
    assert!(info.is_none(), "ratio 9x must not trigger (threshold is 10x)");
    assert_eq!(results.len(), 2);
}

#[test]
fn test_auto_balance_explicit_max_occurrences_overrides_derived_cap() {
    let raw = vec!["rare".to_string(), "common".to_string()];
    let mut results = vec![entry("rare.rs", 5.0, 1, vec![1, 0])];
    for i in 0..50 {
        results.push(entry(&format!("dom{i}.rs"), 1.0, 1, vec![0, 1]));
    }
    // Derived cap would be max(20, 2*1)=20; user override = 5
    let info = apply_auto_balance(&mut results, 2, &raw, Some(5)).expect("should trigger");
    assert_eq!(info.cap, 5);
    assert_eq!(info.dropped_files, 45);
    // 1 rare + 5 dominant-only kept
    assert_eq!(results.len(), 6);
}

#[test]
fn test_auto_balance_keeps_multi_term_files_above_cap() {
    // 1 file matches BOTH terms. 50 dominant-only files. Cap=10 →
    // 50-10=40 dropped, the multi-term file is always kept regardless of cap.
    let raw = vec!["a".to_string(), "b".to_string()];
    let mut results = vec![entry("both.rs", 10.0, 2, vec![1, 50])];
    for i in 0..50 {
        results.push(entry(&format!("dom{i}.rs"), 0.1, 1, vec![0, 1]));
    }
    let info = apply_auto_balance(&mut results, 2, &raw, Some(10)).expect("should trigger");
    // dominant total occurrences = 50 (in dom files) + 50 (in both.rs) = 100; rare = 1 → ratio 100
    assert_eq!(info.dominant_term, "b");
    assert!(results.iter().any(|r| r.file_path == "both.rs"),
        "file matched by both terms must always be kept");
    // 1 multi-term + 10 dominant-only kept
    assert_eq!(results.len(), 11);
    assert_eq!(info.dropped_files, 40);
}

#[test]
fn test_auto_balance_returns_none_when_nothing_to_drop() {
    // Even with extreme imbalance, if there are zero dominant-only files
    // (every dominant match co-occurs with the rare term), nothing to trim.
    let raw = vec!["a".to_string(), "b".to_string()];
    let mut results = vec![entry("shared.rs", 1.0, 200, vec![1, 200])];
    let info = apply_auto_balance(&mut results, 2, &raw, None);
    assert!(info.is_none(), "no dominant-only files → nothing to drop");
    assert_eq!(results.len(), 1);
}
