#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use super::*;
use super::super::handlers_test_utils::make_params_default;
use std::collections::{HashMap, HashSet};


// ─── auto_switch_to_phrase_if_needed tests ──────────────────────

#[test]
fn test_auto_switch_no_special_chars_returns_none() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["hello".to_string(), "world".to_string()];
    let terms = vec!["hello".to_string(), "world".to_string()];
    let result = auto_switch_to_phrase_if_needed(
        &ctx,
        &index,
        &terms,
        &raw_terms,
        &params,
        &resolve_grep_file_scope(&index, &params),
    );
    assert!(result.is_none(), "Should return None when no terms contain spaces or punctuation");
}

#[test]
fn test_auto_switch_with_spaces_returns_some() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["create procedure".to_string()];
    let terms = vec!["CREATE PROCEDURE".to_string()];
    let result = auto_switch_to_phrase_if_needed(
        &ctx,
        &index,
        &terms,
        &raw_terms,
        &params,
        &resolve_grep_file_scope(&index, &params),
    );
    assert!(result.is_some(), "Should return Some when terms contain spaces");
}

#[test]
fn test_auto_switch_with_punctuation_returns_some() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["#[cfg(test)]".to_string()];
    let terms = vec!["#[cfg(test)]".to_string()];
    let result = auto_switch_to_phrase_if_needed(
        &ctx,
        &index,
        &terms,
        &raw_terms,
        &params,
        &resolve_grep_file_scope(&index, &params),
    );
    assert!(result.is_some(), "Should return Some when terms contain punctuation like #[cfg(test)]");
}

#[test]
fn test_auto_switch_with_angle_brackets_returns_some() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["<summary>".to_string()];
    let terms = vec!["<summary>".to_string()];
    let result = auto_switch_to_phrase_if_needed(
        &ctx,
        &index,
        &terms,
        &raw_terms,
        &params,
        &resolve_grep_file_scope(&index, &params),
    );
    assert!(result.is_some(), "Should return Some when terms contain angle brackets");
}

#[test]
fn test_auto_switch_underscore_only_returns_none() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = make_params_default();
    let raw_terms = vec!["my_variable".to_string()];
    let terms = vec!["my_variable".to_string()];
    let result = auto_switch_to_phrase_if_needed(
        &ctx,
        &index,
        &terms,
        &raw_terms,
        &params,
        &resolve_grep_file_scope(&index, &params),
    );
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
        &["userservice".to_string()], 0, &index,
        &resolve_grep_file_scope(&index, &params), 1.0,
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

    let ext = vec!["cs".to_string()];
    let params = GrepSearchParams {
        ext_filter: &ext,
        ..make_params_default()
    };

    let mut tokens_with_hits = HashSet::new();
    let mut file_scores = HashMap::new();
    let mut file_matched_terms = HashMap::new();

    score_token_postings(
        &["token".to_string()], 0, &index,
        &resolve_grep_file_scope(&index, &params), 2.0,
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
        &["term_a".to_string()], 0, &index,
        &resolve_grep_file_scope(&index, &params), 1.0,
        &mut tokens_with_hits, &mut file_scores, &mut file_matched_terms,
    );
    score_token_postings(
        &["term_b".to_string()], 1, &index,
        &resolve_grep_file_scope(&index, &params), 1.0,
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
    let args = json!({"terms": ["hello"], "mode": "and", "substring": false});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(result.mode_and);
}

#[test]
fn test_parse_grep_args_count_only() {
    let args = json!({"terms": ["hello"], "countOnly": true});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(result.count_only);
}

#[test]
fn test_parse_grep_args_show_lines_explicit() {
    let args = json!({"terms": ["hello"], "showLines": true});
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

    let ext = vec!["cs".to_string()];
    let params = GrepSearchParams {
        ext_filter: &ext,
        ..make_params_default()
    };
    let terms = vec!["hello".to_string()];
    let (scores, _) = score_normal_token_search(
        &terms,
        &index,
        &params,
        &resolve_grep_file_scope(&index, &params),
    );
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
        "terms": ["private.*double Percentile"],
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
        "terms": ["I[A-Z]\\w+Cache"],
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
    let ext_filter = vec!["ps1".to_string()];

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
    let ext_filter = vec!["rs".to_string()];
    let original_text = result.content[0].text.clone();

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"].get("hint").is_none(),
        "Should NOT add hint when ext IS indexed but just 0 results");
    // Byte-for-byte preservation: zero-result indexed-ext is a no-touch path,
    // `summary_changed` must stay false, no JSON re-serialization. Mutation
    // guard for an unconditional `summary_changed = true` regression in the
    // post-XML-branch tail.
    assert_eq!(result.content[0].text, original_text,
        "result.content text must be byte-for-byte preserved on indexed-ext no-touch path");
}

#[test]
fn test_inject_grep_ext_hint_no_ext_filter_no_hint() {
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "rs md".to_string();

    let json_text = r#"{"files":[],"summary":{"totalFiles":0,"totalOccurrences":0}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter: Vec<String> = vec![];

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
    let ext_filter = vec!["ps1".to_string()];

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
    let ext_filter = vec!["rs".to_string(), "ps1".to_string()]; // rs is indexed, ps1 is not

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = output["summary"]["hint"].as_str().unwrap();
    assert!(hint.contains("ps1"), "hint should mention non-indexed ps1");
    assert!(!hint.contains("'rs"), "hint should NOT mention indexed rs");
}

#[test]
#[cfg(feature = "lang-xml")]
fn test_inject_grep_ext_hint_xml_shaped_emits_xml_on_demand_hint() {
    // User-story scenario: agent narrowed by ext=["xml"] (or any XML-shaped
    // ext like "manifestxml"/"props"/"resx") and got 0 results from the
    // content index. The XML on-demand parser path is independent of --ext
    // and supports the full whitelist, so a redirect hint must fire.
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "cs".to_string(); // xml NOT in content index

    let json_text = r#"{"files":[],"summary":{"totalFiles":0,"totalOccurrences":0}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter = vec!["xml".to_string()];

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let xml_hint = output["summary"]["xmlOnDemandHint"].as_str()
        .expect("xmlOnDemandHint must be present for XML-shaped ext with 0 results");
    assert!(xml_hint.contains("xray_definitions"),
        "xmlOnDemandHint must point at xray_definitions; got: {xml_hint}");
    assert!(xml_hint.contains("xray_fast"),
        "xmlOnDemandHint must mention the xray_fast discovery recipe; got: {xml_hint}");
    assert!(xml_hint.contains("manifestxml"),
        "xmlOnDemandHint must list manifestxml in the whitelist; got: {xml_hint}");
    // The legacy generic hint must ALSO fire (xml is non-indexed in this
    // ctx). The two hints are independent fields and must coexist.
    assert!(output["summary"]["hint"].as_str().is_some(),
        "generic non-indexed-ext hint must coexist with xmlOnDemandHint");
}

#[test]
#[cfg(feature = "lang-xml")]
fn test_inject_grep_ext_hint_non_xml_ext_no_xml_hint() {
    // Negative-case mutation guard: ps1 is NON-XML even though it's
    // non-indexed; the new XML-on-demand hint must NOT fire — only the
    // legacy generic hint. Catches accidental "any non-indexed ext gets
    // xmlOnDemandHint" regression.
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "cs".to_string();

    let json_text = r#"{"files":[],"summary":{"totalFiles":0,"totalOccurrences":0}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter = vec!["ps1".to_string()];

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["hint"].as_str().is_some(),
        "legacy hint must still fire for non-indexed non-XML ext");
    assert!(output["summary"].get("xmlOnDemandHint").is_none(),
        "xmlOnDemandHint must NOT fire for non-XML ext like ps1");
}

#[test]
#[cfg(feature = "lang-xml")]
fn test_inject_grep_ext_hint_manifestxml_emits_xml_on_demand_hint() {
    // The motivating bug from the user story: agent ran
    // `xray_grep ext=["xml"]` over a workspace that uses `.manifestxml`,
    // got 0 results, fell back to PowerShell. Symmetrically, a query with
    // ext=["manifestxml"] when the server only indexes `xml` (or nothing)
    // must redirect to xray_definitions on-demand — the on-demand path
    // accepts manifestxml even when --ext does not.
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "xml".to_string(); // xml indexed, manifestxml NOT

    let json_text = r#"{"files":[],"summary":{"totalFiles":0,"totalOccurrences":0}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let ext_filter = vec!["manifestxml".to_string()];

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let xml_hint = output["summary"]["xmlOnDemandHint"].as_str()
        .expect("xmlOnDemandHint must be present for manifestxml ext");
    assert!(xml_hint.contains("manifestxml"),
        "xmlOnDemandHint must echo the manifestxml ext; got: {xml_hint}");
    assert!(xml_hint.contains("does NOT"),
        "xmlOnDemandHint must explain that ext=[\"xml\"] does NOT match .manifestxml; got: {xml_hint}");
}

#[test]
#[cfg(feature = "lang-xml")]
fn test_inject_grep_ext_hint_xml_positive_results_no_xml_hint() {
    // Mutation guard for early-return ordering: if a future edit moves the
    // `totalFiles > 0` early-return BELOW the XML branch, successful
    // `xray_grep ext=["xml"]` responses would falsely emit
    // `summary.xmlOnDemandHint` and divert agents away from the real results.
    // This positive-results test pins the invariant that BOTH hint fields
    // are absent when there are matches, regardless of XML-shaped ext.
    //
    // Also asserts byte-for-byte preservation of `result.content[0].text`:
    // the new code path uses a `summary_changed` boolean to skip JSON
    // re-serialization for no-touch cases. A regression that always re-serializes
    // would break downstream consumers that hash the response.
    let mut ctx = HandlerContext::default();
    ctx.server_ext = "xml".to_string();

    let json_text = r#"{"files":[{"path":"app.xml"}],"summary":{"totalFiles":1,"totalOccurrences":3}}"#;
    let mut result = ToolCallResult::success(json_text.to_string());
    let original_text = result.content[0].text.clone();
    let ext_filter = vec!["xml".to_string()];

    inject_grep_ext_hint(&mut result, &ext_filter, &ctx);

    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"].get("hint").is_none(),
        "legacy hint must NOT fire when totalFiles>0; got: {}", result.content[0].text);
    assert!(output["summary"].get("xmlOnDemandHint").is_none(),
        "xmlOnDemandHint must NOT fire when totalFiles>0 (would divert from real results); got: {}",
        result.content[0].text);
    assert_eq!(result.content[0].text, original_text,
        "result.content text must be byte-for-byte preserved when no hint fires");
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
    let (results, _) = score_normal_token_search(
        &["hello".to_string()],
        &index,
        &params,
        &resolve_grep_file_scope(&index, &params),
    );
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

// ─── filesOnly / invert / file-glob-warning tests ─────────────────────
//
// These cover the contract added by `filesOnly=true`, `invert=true`, and the
// `file=` literal-glob warning. They use the same `make_grep_ctx` shape as
// `make_substring_ctx` from `handlers_tests_grep.rs` (kept local here to
// avoid cross-module re-export).

use crate::index::build_trigram_index;
use crate::Posting;
use super::super::handlers_test_utils::HandlerContextBuilder;

fn make_grep_ctx(tokens_to_files: Vec<(&str, u32, Vec<u32>)>, files: Vec<&str>, exts: Vec<&str>) -> HandlerContext {
    let mut index_map: HashMap<String, Vec<Posting>> = HashMap::new();
    for (token, file_id, lines) in &tokens_to_files {
        index_map
            .entry(token.to_string())
            .or_default()
            .push(Posting { file_id: *file_id, lines: lines.clone() });
    }
    let file_token_counts: Vec<u32> = {
        let mut counts = vec![0u32; files.len()];
        for (_, file_id, lines) in &tokens_to_files {
            if (*file_id as usize) < counts.len() {
                counts[*file_id as usize] += lines.len() as u32;
            }
        }
        counts
    };
    let total_tokens: u64 = file_token_counts.iter().map(|&c| c as u64).sum();
    let trigram = build_trigram_index(&index_map);
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files.iter().map(|s| s.to_string()).collect(),
        index: index_map,
        total_tokens,
        extensions: exts.iter().map(|s| s.to_string()).collect(),
        file_token_counts,
        trigram,
        ..Default::default()
    };
    HandlerContextBuilder::new()
        .with_content_index(content_index)
        .build()
}

#[test]
fn test_token_regex_count_only_broad_pattern_has_bounded_query_metadata() {
    const TOKEN_COUNT: usize = 20_000;
    let tokens: Vec<String> = (0..TOKEN_COUNT)
        .map(|token_id| format!("token_{token_id:05}"))
        .collect();
    let postings = tokens.iter()
        .map(|token| (token.as_str(), 0, vec![1]))
        .collect();
    let ctx = make_grep_ctx(postings, vec!["C:/test/target.rs"], vec!["rs"]);

    let result = handle_xray_grep(&ctx, &json!({
        "terms": [".*"],
        "regex": true,
        "countOnly": true,
        "maxResults": 3,
    }));

    assert!(!result.is_error, "broad token regex failed: {}", result.content[0].text);
    let response_text = &result.content[0].text;
    assert!(response_text.len() < super::utils::DEFAULT_MAX_RESPONSE_BYTES,
        "countOnly token-regex response must stay bounded, got {} bytes",
        response_text.len());
    let output: Value = serde_json::from_str(response_text).unwrap();
    let summary = &output["summary"];
    let expansion = &summary["regexExpansion"];
    assert_eq!(summary["termsSearched"], json!([".*"]));
    assert_eq!(expansion["schemaVersion"], json!(2));
    assert_eq!(expansion["strategy"], "globalVocabulary");
    assert_eq!(expansion["strategyReason"], "globalVocabularyBaseline");
    assert_eq!(expansion["accountingScope"], "globalVocabulary");
    assert_eq!(expansion["patterns"], json!(1));
    assert_eq!(expansion["tokensExamined"], json!(TOKEN_COUNT));
    assert_eq!(expansion["matchedTokenCount"], json!(TOKEN_COUNT));
    assert_eq!(expansion["postingListsVisited"], json!(TOKEN_COUNT));
    assert_eq!(expansion["postingsChecked"], json!(TOKEN_COUNT));
    assert_eq!(expansion["postingsInScope"], json!(TOKEN_COUNT));
    let timings = &expansion["timings"];
    for field in [
        "planMs",
        "compileMs",
        "universeBuildMs",
        "scanCollectMs",
        "sortDedupMs",
        "expansionTotalMs",
        "postingScoreMs",
    ] {
        assert!(timings[field].as_f64().is_some(), "missing timing {field}: {timings}");
    }
    let expansion_phases = timings["planMs"].as_f64().unwrap()
        + timings["universeBuildMs"].as_f64().unwrap()
        + timings["scanCollectMs"].as_f64().unwrap()
        + timings["sortDedupMs"].as_f64().unwrap();
    assert!(timings["expansionTotalMs"].as_f64().unwrap() + 0.001 >= expansion_phases);
    assert!(timings["postingScoreMs"].as_f64().unwrap() > 0.0);
    assert!(expansion.get("matchedTokenPreview").is_none());
    assert!(expansion.get("previewTruncated").is_none());
}

#[test]
fn test_token_regex_preview_is_capped_and_independent_of_max_results() {
    const TOKEN_COUNT: usize = 25;
    let tokens: Vec<String> = (0..TOKEN_COUNT)
        .map(|token_id| format!("token_{token_id:02}"))
        .collect();
    let postings = tokens.iter().enumerate()
        .map(|(token_id, token)| (token.as_str(), (token_id % 2) as u32, vec![1]))
        .collect();
    let ctx = make_grep_ctx(
        postings,
        vec!["C:/test/A.rs", "C:/test/B.rs"],
        vec!["rs"],
    );
    let run = |max_results: usize| -> Value {
        let result = handle_xray_grep(&ctx, &json!({
            "terms": ["ToKeN_.*"],
            "regex": true,
            "maxResults": max_results,
        }));
        assert!(!result.is_error, "token regex failed: {}", result.content[0].text);
        serde_json::from_str(&result.content[0].text).unwrap()
    };

    let unlimited = run(0);
    let capped = run(1);
    assert_eq!(unlimited["files"].as_array().unwrap().len(), 2);
    assert_eq!(capped["files"].as_array().unwrap().len(), 1);
    assert_eq!(unlimited["summary"]["termsSearched"], json!(["ToKeN_.*"]));
    let mut unlimited_semantics = unlimited["summary"]["regexExpansion"].clone();
    let mut capped_semantics = capped["summary"]["regexExpansion"].clone();
    for expansion in [&mut unlimited_semantics, &mut capped_semantics] {
        let object = expansion.as_object_mut().unwrap();
        for field in [
            "schemaVersion",
            "strategy",
            "strategyReason",
            "accountingScope",
            "timings",
        ] {
            object.remove(field);
        }
    }
    assert_eq!(unlimited_semantics, capped_semantics);
    let expansion = &unlimited["summary"]["regexExpansion"];
    assert_eq!(expansion["schemaVersion"], json!(2));
    assert_eq!(expansion["strategy"], "globalVocabulary");
    assert_eq!(expansion["accountingScope"], "globalVocabulary");
    let preview = expansion["matchedTokenPreview"].as_array().unwrap();
    assert_eq!(expansion["matchedTokenCount"], json!(TOKEN_COUNT));
    assert_eq!(preview.len(), TOKEN_REGEX_PREVIEW_MAX);
    assert_eq!(expansion["previewTruncated"], json!(5));
    assert_eq!(preview.first(), Some(&json!("token_00")));
    assert_eq!(preview.last(), Some(&json!("token_19")));
}

#[test]
fn test_token_regex_posting_counters_distinguish_global_and_scoped_work() {
    let ctx = make_grep_ctx(
        vec![
            ("tokenone", 0, vec![1]),
            ("tokenone", 1, vec![1]),
            ("tokentwo", 1, vec![2]),
        ],
        vec!["C:/test/A.rs", "C:/test/B.rs"],
        vec!["rs"],
    );

    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["token.*"],
        "regex": true,
        "countOnly": true,
        "file": ["A.rs"],
    }));

    assert!(!result.is_error, "scoped token regex failed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let expansion = &output["summary"]["regexExpansion"];
    assert_eq!(expansion["strategy"], "globalVocabulary");
    assert_eq!(expansion["accountingScope"], "globalVocabulary");
    assert_eq!(expansion["postingListsVisited"], json!(2));
    assert_eq!(expansion["postingsChecked"], json!(3));
    assert_eq!(expansion["postingsInScope"], json!(1));
    assert_eq!(output["summary"]["totalFiles"], json!(1));
}

#[test]
fn test_non_empty_global_token_regex_matches_v1_semantics_without_v2_telemetry() {
    let ctx = make_grep_ctx(
        vec![
            ("tokenone", 0, vec![1]),
            ("tokenone", 1, vec![2]),
            ("tokentwo", 1, vec![3]),
        ],
        vec!["C:/test/A.rs", "C:/test/B.rs"],
        vec!["rs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["token.*"],
        "regex": true,
    }));
    assert!(!result.is_error, "token regex failed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let mut expansion = output["summary"]["regexExpansion"].clone();
    let object = expansion.as_object_mut().unwrap();
    for field in [
        "schemaVersion",
        "strategy",
        "strategyReason",
        "accountingScope",
        "timings",
    ] {
        object.remove(field);
    }

    assert_eq!(expansion, json!({
        "patterns": 1,
        "tokensExamined": 2,
        "matchedTokenCount": 2,
        "postingListsVisited": 2,
        "postingsChecked": 3,
        "postingsInScope": 3,
        "matchedTokenPreview": ["tokenone", "tokentwo"],
        "previewTruncated": 0,
    }));
}

#[test]
fn test_token_regex_and_preserves_expanded_token_count_semantics() {
    let ctx = make_grep_ctx(
        vec![
            ("alphaone", 0, vec![1]),
            ("alphatwo", 0, vec![2]),
            ("alphabeta", 1, vec![1]),
            ("alphaone", 2, vec![1]),
            ("onlybeta", 2, vec![2]),
        ],
        vec!["C:/test/A.rs", "C:/test/B.rs", "C:/test/C.rs"],
        vec!["rs"],
    );

    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["alpha.*", ".*beta"],
        "regex": true,
        "mode": "and",
        "filesOnly": true,
    }));

    assert!(!result.is_error, "token regex AND failed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let paths: HashSet<&str> = output["files"].as_array().unwrap().iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert_eq!(paths, HashSet::from(["C:/test/A.rs", "C:/test/C.rs"]));
    assert!(!paths.contains("C:/test/B.rs"),
        "one token matching both patterns is deduplicated and counts once");
}

#[test]
fn test_files_only_strips_lines_and_score() {
    let ctx = make_grep_ctx(
        vec![("httpclient", 0, vec![5, 12]), ("httpclient", 1, vec![3])],
        vec!["C:/test/A.cs", "C:/test/B.cs"],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "filesOnly": true,
        "showLines": true,
    }));
    assert!(!result.is_error, "grep should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["filesOnly"], json!(true));
    let files = output["files"].as_array().expect("files array");
    assert_eq!(files.len(), 2);
    for f in files {
        let obj = f.as_object().expect("file entry is object");
        assert!(obj.contains_key("path"), "path must remain: {:?}", obj);
        // Only `path` and `occurrences` allowed; lines / lineContent / score must be stripped.
        for key in obj.keys() {
            assert!(
                matches!(key.as_str(), "path" | "occurrences"),
                "filesOnly should strip key `{}` from file entry: {:?}",
                key, obj
            );
        }
    }
}

#[test]
fn test_files_only_with_count_only_count_only_wins() {
    // countOnly returns no files array — filesOnly is then a no-op (summary tagged).
    let ctx = make_grep_ctx(
        vec![("httpclient", 0, vec![5])],
        vec!["C:/test/A.cs"],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "filesOnly": true,
        "countOnly": true,
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    // countOnly path doesn't build a files array.
    assert!(output.get("files").is_none() || output["files"].as_array().is_some_and(|a| a.is_empty()));
}

#[test]
fn test_invert_without_scope_errors() {
    let ctx = make_grep_ctx(
        vec![("httpclient", 0, vec![5])],
        vec!["C:/test/A.cs"],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "invert": true,
    }));
    assert!(result.is_error, "invert without scope must error");
    let msg = &result.content[0].text;
    assert!(
        msg.contains("invert=true requires an explicit scope"),
        "error should explain scope requirement, got: {}",
        msg
    );
}

#[test]
fn test_invert_with_ext_lists_files_without_match() {
    // 3 .cs files; only A contains the term — invert should list B and C.
    let ctx = make_grep_ctx(
        vec![("httpclient", 0, vec![5])],
        vec!["C:/test/A.cs", "C:/test/B.cs", "C:/test/C.cs"],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "invert": true,
        "ext": ["cs"],
    }));
    assert!(!result.is_error, "invert with scope should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["invert"], json!(true));
    assert_eq!(output["summary"]["filesOnly"], json!(true));
    assert_eq!(output["summary"]["totalFiles"], json!(2), "complement has 2 files");
    assert_eq!(output["summary"]["totalFilesInScope"], json!(3));
    assert_eq!(output["summary"]["totalFilesWithMatches"], json!(1));
    let paths: HashSet<String> = output["files"]
        .as_array().unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    assert!(paths.contains("C:/test/B.cs"));
    assert!(paths.contains("C:/test/C.cs"));
    assert!(!paths.contains("C:/test/A.cs"));
    // Non-truncated invert is exhaustive over the scoped universe.
    assert_eq!(output["resultStatus"]["status"], json!("complete"));
    assert_eq!(output["resultStatus"]["complete"], json!(true));
    assert_eq!(output["resultStatus"]["safeForExhaustiveClaims"], json!(true));
    assert!(output["resultStatus"]["reasons"].as_array().unwrap().is_empty());
}

#[test]
fn test_file_filter_literal_glob_emits_warning_when_zero_matches() {
    // file=["**/*.cs"] is a glob, treated as a substring — matches nothing.
    let ctx = make_grep_ctx(
        vec![("httpclient", 0, vec![5])],
        vec!["C:/test/A.cs"],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "file": ["**/*.cs"],
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], json!(0));
    let warning = &output["summary"]["warnings"]["fileFilterLiteralGlob"];
    assert!(warning.is_object(), "glob warning should be emitted, got summary: {}", output["summary"]);
    let entries = warning["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], json!("**/*.cs"));
}

#[test]
fn test_file_filter_literal_substring_no_warning_when_matches_exist() {
    // file=["A"] is a plain substring — matches A.cs, no warning expected.
    let ctx = make_grep_ctx(
        vec![("httpclient", 0, vec![5])],
        vec!["C:/test/A.cs"],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "file": ["A"],
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], json!(1));
    // No glob metachar in `file=`, so no warning regardless of match count.
    assert!(
        output["summary"].get("warnings").is_none()
            || output["summary"]["warnings"].get("fileFilterLiteralGlob").is_none(),
        "non-glob file filter must not trigger the literal-glob warning: {}",
        output["summary"]
    );
}

#[test]
fn test_invert_with_count_only_errors() {
    // countOnly omits the matched-files list that apply_invert needs to
    // compute the complement; the combo would silently return the whole
    // scoped universe as "did not match". Reject up-front.
    let ctx = make_grep_ctx(
        vec![("httpclient", 0, vec![5])],
        vec!["C:/test/A.cs"],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "invert": true,
        "ext": ["cs"],
        "countOnly": true,
    }));
    assert!(result.is_error, "invert + countOnly must error");
    let msg = &result.content[0].text;
    assert!(
        msg.contains("mutually exclusive"),
        "error should mention mutual exclusivity, got: {}",
        msg
    );
}

#[test]
fn test_invert_complement_correct_when_matches_exceed_max_results() {
    // 4 .cs files match the term, 1 does not. With user-requested maxResults=2,
    // the inner search would (without the fix) truncate `files` to 2,
    // making apply_invert mis-classify the 2 hidden matches as non-matches
    // and return a 3-file complement. The fix runs the inner search uncapped
    // when invert=true so the complement is the true single non-matching file.
    let ctx = make_grep_ctx(
        vec![
            ("httpclient", 0, vec![5]),
            ("httpclient", 1, vec![5]),
            ("httpclient", 2, vec![5]),
            ("httpclient", 3, vec![5]),
            // file_id=4 has no `httpclient` token — the only true miss.
        ],
        vec!["C:/test/A.cs", "C:/test/B.cs", "C:/test/C.cs", "C:/test/D.cs", "C:/test/E.cs"],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "invert": true,
        "ext": ["cs"],
        "maxResults": 2,
    }));
    assert!(!result.is_error, "invert should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["invert"], json!(true));
    assert_eq!(output["summary"]["totalFilesInScope"], json!(5));
    assert_eq!(output["summary"]["totalFilesWithMatches"], json!(4),
        "inner search must run uncapped under invert");
    assert_eq!(output["summary"]["totalFiles"], json!(1),
        "complement is exactly the single non-matching file");
    let paths: HashSet<String> = output["files"]
        .as_array().unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    assert!(paths.contains("C:/test/E.cs"));
    assert!(!paths.contains("C:/test/A.cs"));
}

#[test]
fn test_invert_caps_complement_at_user_max_results() {
    // 5 files match, 5 do not. User asks for maxResults=2. Inner search runs
    // uncapped (so all 5 matches are visible), then the FINAL complement is
    // truncated to 2 with truncated=true reported in the summary.
    let ctx = make_grep_ctx(
        vec![
            ("httpclient", 0, vec![5]),
            ("httpclient", 1, vec![5]),
            ("httpclient", 2, vec![5]),
            ("httpclient", 3, vec![5]),
            ("httpclient", 4, vec![5]),
            // file_ids 5..=9 have no `httpclient` token.
        ],
        vec![
            "C:/test/A.cs", "C:/test/B.cs", "C:/test/C.cs", "C:/test/D.cs", "C:/test/E.cs",
            "C:/test/F.cs", "C:/test/G.cs", "C:/test/H.cs", "C:/test/I.cs", "C:/test/J.cs",
        ],
        vec!["cs"],
    );
    let result = handle_xray_grep(&ctx, &json!({
        "terms": ["httpclient"],
        "substring": true,
        "invert": true,
        "ext": ["cs"],
        "maxResults": 2,
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFilesWithMatches"], json!(5));
    // totalFiles surfaces the TRUE complement size (5), regardless of the cap.
    assert_eq!(output["summary"]["totalFiles"], json!(5));
    // resultStatus reflects the post-cap delivered slice.
    assert_eq!(output["resultStatus"]["shown"]["files"], json!(2),
        "complement delivered capped at maxResults=2");
    assert_eq!(output["resultStatus"]["total"]["files"], json!(5));
    assert_eq!(output["resultStatus"]["omitted"]["files"], json!(3));
    // Capped invert MUST flip exhaustive-claim guards so downstream consumers
    // do not treat the partial listing as a definitive "did any files miss".
    assert_eq!(output["resultStatus"]["status"], json!("partial"));
    assert_eq!(output["resultStatus"]["complete"], json!(false));
    assert_eq!(output["resultStatus"]["safeForExhaustiveClaims"], json!(false));
    let reasons = output["resultStatus"]["reasons"].as_array().unwrap();
    assert!(reasons.iter().any(|r| r == "max_results"),
        "capped invert should cite max_results in reasons: {:?}", reasons);
    // And the actual files array honors the cap.
    assert_eq!(output["files"].as_array().unwrap().len(), 2);
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

#[test]
fn test_invert_partition_contract_forward_and_invert_partition_scope() {
    // Forward filesOnly + invert must partition the scoped universe byte-exactly.
    let scope = vec![
        "C:/test/A.cs", "C:/test/B.cs", "C:/test/C.cs",
        "C:/test/D.cs", "C:/test/E.cs",
    ];
    let ctx = make_grep_ctx(
        vec![
            ("httpclient", 0, vec![5]),
            ("httpclient", 1, vec![5]),
            ("httpclient", 2, vec![5]),
            // file_ids 3, 4 (D.cs, E.cs) have no `httpclient` token.
        ],
        scope.clone(),
        vec!["cs"],
    );

    let run = |invert: bool| -> Value {
        let result = handle_xray_grep(&ctx, &json!({
            "terms": ["httpclient"],
            "substring": true,
            "filesOnly": true,
            "invert": invert,
            "ext": ["cs"],
        }));
        assert!(!result.is_error,
            "grep (invert={}) should not error: {}", invert, result.content[0].text);
        serde_json::from_str(&result.content[0].text).unwrap()
    };

    let forward = run(false);
    let inverted = run(true);

    let collect_paths = |out: &Value| -> HashSet<String> {
        out["files"].as_array().unwrap().iter()
            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect()
    };
    let fwd_paths = collect_paths(&forward);
    let inv_paths = collect_paths(&inverted);
    // Exact sets (subsumes disjointness + union = scope).
    let expected_fwd: HashSet<String> = ["C:/test/A.cs", "C:/test/B.cs", "C:/test/C.cs"]
        .into_iter().map(String::from).collect();
    let expected_inv: HashSet<String> = ["C:/test/D.cs", "C:/test/E.cs"]
        .into_iter().map(String::from).collect();
    assert_eq!(fwd_paths, expected_fwd, "forward set mismatch");
    assert_eq!(inv_paths, expected_inv, "invert set mismatch");

    // filesOnly on both, exhaustive (no truncation/caps) on both.
    for (label, out) in [("forward", &forward), ("invert", &inverted)] {
        assert_eq!(out["summary"]["filesOnly"], json!(true),
            "{} summary.filesOnly", label);
        let status = &out["resultStatus"];
        assert_eq!(status["status"], json!("complete"),
            "{} resultStatus.status", label);
        assert_eq!(status["complete"], json!(true),
            "{} resultStatus.complete", label);
        assert_eq!(status["safeForExhaustiveClaims"], json!(true),
            "{} resultStatus.safeForExhaustiveClaims", label);
        assert!(status["reasons"].as_array().unwrap().is_empty(),
            "{} resultStatus.reasons must be empty: {:?}", label, status["reasons"]);
        assert_eq!(out["summary"]["scope"]["requested"], json!(true));
        assert_eq!(out["summary"]["scope"]["strategy"], json!("linearScan"));
        assert_eq!(out["summary"]["scope"]["indexFiles"], json!(scope.len()));
        assert_eq!(out["summary"]["scope"]["matchedFiles"], json!(scope.len()));
    }
    // totalFilesInScope is invert-only; reflects the post-filter universe.
    assert_eq!(inverted["summary"]["totalFilesInScope"], json!(scope.len()));
}
