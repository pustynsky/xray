use super::*;
use std::time::{Duration, Instant};

/// Helper: create GrepSearchParams with given filters.
fn make_params<'a>(
    dir_filter: &'a Option<String>,
    ext_filter: &'a Option<String>,
    exclude_dir: &'a [String],
    exclude: &'a [String],
) -> GrepSearchParams<'a> {
    GrepSearchParams {
        ext_filter,
        exclude_dir,
        exclude,
        show_lines: false,
        context_lines: 0,
        max_results: 50,
        mode_and: false,
        count_only: false,
        search_start: Instant::now(),
        dir_filter,
    }
}

// ─── passes_file_filters tests ──────────────────────────────────

#[test]
fn test_passes_file_filters_no_filters() {
    let params = make_params(&None, &None, &[], &[]);
    assert!(passes_file_filters("C:/project/src/file.cs", &params));
}

#[test]
fn test_passes_file_filters_dir_match() {
    let dir = Some("C:/project/src".to_string());
    let params = make_params(&dir, &None, &[], &[]);
    assert!(passes_file_filters("C:/project/src/file.cs", &params));
}

#[test]
fn test_passes_file_filters_dir_no_match() {
    let dir = Some("C:/project/src".to_string());
    let params = make_params(&dir, &None, &[], &[]);
    assert!(!passes_file_filters("C:/project/lib/file.cs", &params));
}

#[test]
fn test_passes_file_filters_ext_match() {
    let ext = Some("cs".to_string());
    let params = make_params(&None, &ext, &[], &[]);
    assert!(passes_file_filters("C:/project/file.cs", &params));
}

#[test]
fn test_passes_file_filters_ext_no_match() {
    let ext = Some("cs".to_string());
    let params = make_params(&None, &ext, &[], &[]);
    assert!(!passes_file_filters("C:/project/file.xml", &params));
}

#[test]
fn test_passes_file_filters_ext_comma_separated() {
    let ext = Some("cs,sql".to_string());
    let params = make_params(&None, &ext, &[], &[]);
    assert!(passes_file_filters("C:/project/file.cs", &params));
    assert!(passes_file_filters("C:/project/file.sql", &params));
    assert!(!passes_file_filters("C:/project/file.xml", &params));
}

#[test]
fn test_passes_file_filters_exclude_dir() {
    let excl_dir = vec!["test".to_string()];
    let params = make_params(&None, &None, &excl_dir, &[]);
    assert!(!passes_file_filters("C:/project/test/file.cs", &params));
    assert!(passes_file_filters("C:/project/src/file.cs", &params));
}

#[test]
fn test_passes_file_filters_exclude_pattern() {
    let excl = vec!["Mock".to_string()];
    let params = make_params(&None, &None, &[], &excl);
    assert!(!passes_file_filters("C:/project/ServiceMock.cs", &params));
    assert!(passes_file_filters("C:/project/Service.cs", &params));
}

#[test]
fn test_passes_file_filters_combined() {
    let dir = Some("C:/project/src".to_string());
    let ext = Some("cs".to_string());
    let excl_dir = vec!["test".to_string()];
    let excl = vec!["Mock".to_string()];
    let params = make_params(&dir, &ext, &excl_dir, &excl);
    // All filters pass
    assert!(passes_file_filters("C:/project/src/Service.cs", &params));
    // Wrong dir
    assert!(!passes_file_filters("C:/project/lib/Service.cs", &params));
    // Wrong ext
    assert!(!passes_file_filters("C:/project/src/Service.xml", &params));
    // Excluded dir
    assert!(!passes_file_filters("C:/project/src/test/Service.cs", &params));
    // Excluded pattern
    assert!(!passes_file_filters("C:/project/src/ServiceMock.cs", &params));
}

// ─── finalize_grep_results tests ────────────────────────────────

#[test]
fn test_finalize_or_mode_passes_all() {
    let mut scores = HashMap::new();
    scores.insert(0, FileScoreEntry { file_path: "a.cs".into(), lines: vec![3, 1, 2], tf_idf: 1.0, occurrences: 3, terms_matched: 1 });
    scores.insert(1, FileScoreEntry { file_path: "b.cs".into(), lines: vec![5], tf_idf: 2.0, occurrences: 1, terms_matched: 1 });

    let (results, total_files, total_occ) = finalize_grep_results(scores, false, 2);
    assert_eq!(total_files, 2);
    assert_eq!(total_occ, 4);
    // Sorted by tf_idf descending
    assert_eq!(results[0].file_path, "b.cs");
    assert_eq!(results[1].file_path, "a.cs");
    // Lines deduped and sorted
    assert_eq!(results[1].lines, vec![1, 2, 3]);
}

#[test]
fn test_finalize_and_mode_filters() {
    let mut scores = HashMap::new();
    scores.insert(0, FileScoreEntry { file_path: "a.cs".into(), lines: vec![1], tf_idf: 1.0, occurrences: 1, terms_matched: 2 });
    scores.insert(1, FileScoreEntry { file_path: "b.cs".into(), lines: vec![1], tf_idf: 2.0, occurrences: 1, terms_matched: 1 });

    let (results, total_files, _total_occ) = finalize_grep_results(scores, true, 2);
    assert_eq!(total_files, 1);
    assert_eq!(results[0].file_path, "a.cs");
}

#[test]
fn test_finalize_dedup_lines() {
    let mut scores = HashMap::new();
    scores.insert(0, FileScoreEntry { file_path: "a.cs".into(), lines: vec![5, 3, 5, 1, 3], tf_idf: 1.0, occurrences: 5, terms_matched: 1 });

    let (results, _, _) = finalize_grep_results(scores, false, 1);
    assert_eq!(results[0].lines, vec![1, 3, 5]);
}

#[test]
fn test_finalize_empty_input() {
    let scores = HashMap::new();
    let (results, total_files, total_occ) = finalize_grep_results(scores, false, 1);
    assert_eq!(total_files, 0);
    assert_eq!(total_occ, 0);
    assert!(results.is_empty());
}

// ─── build_grep_base_summary tests ──────────────────────────────

#[test]
fn test_summary_basic_fields() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let elapsed = Duration::from_millis(5);

    let summary = build_grep_base_summary(
        10, 42, &json!(["term1"]), "or", &index, elapsed, &ctx, false,
    );
    assert_eq!(summary["totalFiles"], 10);
    assert_eq!(summary["totalOccurrences"], 42);
    assert_eq!(summary["searchMode"], "or");
    // Without include_index_stats, no indexFiles
    assert!(summary.get("indexFiles").is_none());
}

#[test]
fn test_summary_with_index_stats() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let elapsed = Duration::from_millis(5);

    let summary = build_grep_base_summary(
        10, 42, &json!(["term1"]), "or", &index, elapsed, &ctx, true,
    );
    assert!(summary.get("indexFiles").is_some());
    assert!(summary.get("indexTokens").is_some());
    assert!(summary.get("searchTimeMs").is_some());
}

#[test]
fn test_summary_with_read_errors() {
    let index = ContentIndex { read_errors: 3, lossy_file_count: 2, ..Default::default() };
    let ctx = HandlerContext::default();
    let elapsed = Duration::from_millis(1);

    let summary = build_grep_base_summary(
        0, 0, &json!(["x"]), "or", &index, elapsed, &ctx, false,
    );
    assert_eq!(summary["readErrors"], 3);
    assert_eq!(summary["lossyUtf8Files"], 2);
}

#[test]
fn test_summary_no_read_errors_when_zero() {
    let index = ContentIndex { read_errors: 0, lossy_file_count: 0, ..Default::default() };
    let ctx = HandlerContext::default();
    let elapsed = Duration::from_millis(1);

    let summary = build_grep_base_summary(
        0, 0, &json!(["x"]), "or", &index, elapsed, &ctx, false,
    );
    assert!(summary.get("readErrors").is_none());
    assert!(summary.get("lossyUtf8Files").is_none());
}

#[test]
fn test_summary_branch_warning_on_feature_branch() {
    let index = ContentIndex::default();
    let ctx = HandlerContext {
        current_branch: Some("feature/test".to_string()),
        ..Default::default()
    };
    let elapsed = Duration::from_millis(1);

    let summary = build_grep_base_summary(
        0, 0, &json!(["x"]), "or", &index, elapsed, &ctx, false,
    );
    assert!(summary.get("branchWarning").is_some());
}

#[test]
fn test_summary_no_branch_warning_on_main() {
    let index = ContentIndex::default();
    let ctx = HandlerContext {
        current_branch: Some("main".to_string()),
        ..Default::default()
    };
    let elapsed = Duration::from_millis(1);

    let summary = build_grep_base_summary(
        0, 0, &json!(["x"]), "or", &index, elapsed, &ctx, false,
    );
    assert!(summary.get("branchWarning").is_none());
}

// ═══════════════════════════════════════════════════════════════════
// Tests for extracted helper functions (complexity reduction)
// ═══════════════════════════════════════════════════════════════════

// ─── parse_grep_args tests ──────────────────────────────────────

#[test]
fn test_parse_grep_args_missing_terms() {
    let args = json!({});
    let result = parse_grep_args(&args, "C:/project");
    assert!(result.is_err());
}

#[test]
fn test_parse_grep_args_basic() {
    let args = json!({"terms": "hello"});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert_eq!(result.terms_str, "hello");
    assert!(result.use_substring); // default
    assert!(!result.use_regex);
    assert!(!result.use_phrase);
    assert!(!result.mode_and);
    assert_eq!(result.max_results, 50); // default
}

#[test]
fn test_parse_grep_args_substring_mutually_exclusive_with_regex() {
    let args = json!({"terms": "hello", "regex": true, "substring": true});
    let result = parse_grep_args(&args, "C:/project");
    assert!(result.is_err());
}

#[test]
fn test_parse_grep_args_substring_mutually_exclusive_with_phrase() {
    let args = json!({"terms": "hello", "phrase": true, "substring": true});
    let result = parse_grep_args(&args, "C:/project");
    assert!(result.is_err());
}

#[test]
fn test_parse_grep_args_regex_auto_disables_substring() {
    let args = json!({"terms": "hello", "regex": true});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(!result.use_substring);
    assert!(result.use_regex);
}

#[test]
fn test_parse_grep_args_context_lines_enables_show_lines() {
    let args = json!({"terms": "hello", "contextLines": 3});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(result.show_lines);
    assert_eq!(result.context_lines, 3);
}

#[test]
fn test_parse_grep_args_exclude_lists() {
    let args = json!({
        "terms": "hello",
        "excludeDir": ["test", "bin"],
        "exclude": ["mock"]
    });
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert_eq!(result.exclude_dir, vec!["test", "bin"]);
    assert_eq!(result.exclude, vec!["mock"]);
}

// ─── expand_regex_terms tests ───────────────────────────────────

#[test]
fn test_expand_regex_terms_basic() {
    use crate::Posting;
    let mut index = ContentIndex::default();
    index.index.insert("userservice".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    index.index.insert("orderservice".to_string(), vec![Posting { file_id: 0, lines: vec![2] }]);
    index.index.insert("unrelated".to_string(), vec![Posting { file_id: 0, lines: vec![3] }]);

    let terms = vec![".*service".to_string()];
    let expanded = expand_regex_terms(&terms, &index).unwrap();
    assert!(expanded.contains(&"userservice".to_string()));
    assert!(expanded.contains(&"orderservice".to_string()));
    assert!(!expanded.contains(&"unrelated".to_string()));
}

#[test]
fn test_expand_regex_terms_invalid_pattern() {
    let index = ContentIndex::default();
    let terms = vec!["[invalid".to_string()];
    let result = expand_regex_terms(&terms, &index);
    assert!(result.is_err());
}

// ─── score_normal_token_search tests ────────────────────────────

#[test]
fn test_score_normal_token_search_basic() {
    use crate::Posting;
    let mut index = ContentIndex::default();
    index.files = vec!["file1.cs".to_string(), "file2.cs".to_string()];
    index.file_token_counts = vec![100, 50];
    index.index.insert("hello".to_string(), vec![
        Posting { file_id: 0, lines: vec![1, 5] },
        Posting { file_id: 1, lines: vec![3] },
    ]);

    let params = make_params(&None, &None, &[], &[]);
    let terms = vec!["hello".to_string()];
    let scores = score_normal_token_search(&terms, &index, &params);
    assert_eq!(scores.len(), 2);
    assert!(scores.get(&0).is_some());
    assert!(scores.get(&1).is_some());
    assert_eq!(scores[&0].occurrences, 2);
    assert_eq!(scores[&1].occurrences, 1);
}

#[test]
fn test_score_normal_token_search_no_match() {
    let index = ContentIndex::default();
    let params = make_params(&None, &None, &[], &[]);
    let terms = vec!["nonexistent".to_string()];
    let scores = score_normal_token_search(&terms, &index, &params);
    assert!(scores.is_empty());
}

// ─── find_matching_tokens_for_term tests ────────────────────────

#[test]
fn test_find_matching_tokens_short_term() {
    use crate::TrigramIndex;
    let mut trigram_idx = TrigramIndex::default();
    trigram_idx.tokens = vec!["ab".to_string(), "abc".to_string(), "xyz".to_string()];
    // Short term (<3 chars) → linear scan
    let results = find_matching_tokens_for_term("ab", &trigram_idx);
    assert_eq!(results.len(), 2); // "ab" and "abc" both contain "ab"
}

#[test]
fn test_find_matching_tokens_empty_trigrams() {
    use crate::TrigramIndex;
    let trigram_idx = TrigramIndex::default();
    let results = find_matching_tokens_for_term("hello", &trigram_idx);
    assert!(results.is_empty());
}

// ─── build_grep_response tests ──────────────────────────────────

#[test]
fn test_build_grep_response_count_only() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = GrepSearchParams {
        ext_filter: &None,
        exclude_dir: &[],
        exclude: &[],
        show_lines: false,
        context_lines: 0,
        max_results: 50,
        mode_and: false,
        count_only: true,
        search_start: Instant::now(),
        dir_filter: &None,
    };

    let results = vec![FileScoreEntry {
        file_path: "test.cs".to_string(),
        lines: vec![1, 2],
        tf_idf: 1.0,
        occurrences: 2,
        terms_matched: 1,
    }];
    let terms = vec!["hello".to_string()];

    let result = build_grep_response(&results, &terms, 1, 2, "or", &index, &ctx, &params);
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("summary").is_some());
    // count_only → no files array
    assert!(v.get("files").is_none());
}

#[test]
fn test_build_grep_response_with_files() {
    let index = ContentIndex::default();
    let ctx = HandlerContext::default();
    let params = GrepSearchParams {
        ext_filter: &None,
        exclude_dir: &[],
        exclude: &[],
        show_lines: false,
        context_lines: 0,
        max_results: 50,
        mode_and: false,
        count_only: false,
        search_start: Instant::now(),
        dir_filter: &None,
    };

    let results = vec![FileScoreEntry {
        file_path: "test.cs".to_string(),
        lines: vec![1],
        tf_idf: 0.5,
        occurrences: 1,
        terms_matched: 1,
    }];
    let terms = vec!["hello".to_string()];

    let result = build_grep_response(&results, &terms, 1, 1, "or", &index, &ctx, &params);
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("files").is_some());
    assert!(v.get("summary").is_some());
}
