#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use super::*;
use std::time::{Duration, Instant};

/// Helper: create GrepSearchParams with given filters.
fn make_params<'a>(
    dir_filter: &'a Option<String>,
    ext_filter: &'a [String],
    exclude_dir: &[String],
    exclude: &[String],
) -> GrepSearchParams<'a> {
    GrepSearchParams {
        ext_filter,
        show_lines: false,
        context_lines: 0,
        max_results: 50,
        mode_and: false,
        count_only: false,
        search_start: Instant::now(),
        dir_filter,
        file_filter: &[],
        exclude_patterns: super::utils::ExcludePatterns::from_dirs(exclude_dir),
        exclude_lower: exclude.iter().map(|s| s.to_lowercase()).collect(),
        dir_auto_converted_note: None,
        exact_file_path: &None,
        exact_file_path_canonical: &None,
        auto_balance: true,
        max_occurrences_per_term: None,
        lock_wait_ms: 0.0,
        trigram_stale: false,
    }
}

// ─── passes_file_filters tests ──────────────────────────────────

#[test]
fn test_passes_file_filters_no_filters() {
    let params = make_params(&None, &[], &[], &[]);
    assert!(passes_file_filters("C:/project/src/file.cs", &params));
}

#[test]
fn test_passes_file_filters_dir_match() {
    let dir = Some("C:/project/src".to_string());
    let params = make_params(&dir, &[], &[], &[]);
    assert!(passes_file_filters("C:/project/src/file.cs", &params));
}

#[test]
fn test_passes_file_filters_dir_no_match() {
    let dir = Some("C:/project/src".to_string());
    let params = make_params(&dir, &[], &[], &[]);
    assert!(!passes_file_filters("C:/project/lib/file.cs", &params));
}

#[test]
fn test_passes_file_filters_ext_match() {
    let ext = vec!["cs".to_string()];
    let params = make_params(&None, &ext, &[], &[]);
    assert!(passes_file_filters("C:/project/file.cs", &params));
}

#[test]
fn test_passes_file_filters_ext_no_match() {
    let ext = vec!["cs".to_string()];
    let params = make_params(&None, &ext, &[], &[]);
    assert!(!passes_file_filters("C:/project/file.xml", &params));
}

#[test]
fn test_passes_file_filters_ext_comma_separated() {
    let ext = vec!["cs".to_string(), "sql".to_string()];
    let params = make_params(&None, &ext, &[], &[]);
    assert!(passes_file_filters("C:/project/file.cs", &params));
    assert!(passes_file_filters("C:/project/file.sql", &params));
    assert!(!passes_file_filters("C:/project/file.xml", &params));
}

#[test]
fn test_passes_file_filters_exclude_dir() {
    let excl_dir = vec!["test".to_string()];
    let params = make_params(&None, &[], &excl_dir, &[]);
    assert!(!passes_file_filters("C:/project/test/file.cs", &params));
    assert!(passes_file_filters("C:/project/src/file.cs", &params));
}

#[test]
fn test_passes_file_filters_exclude_pattern() {
    let excl = vec!["Mock".to_string()];
    let params = make_params(&None, &[], &[], &excl);
    assert!(!passes_file_filters("C:/project/ServiceMock.cs", &params));
    assert!(passes_file_filters("C:/project/Service.cs", &params));
}

#[test]
fn test_passes_file_filters_combined() {
    let dir = Some("C:/project/src".to_string());
    let ext = vec!["cs".to_string()];
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
    scores.insert(0, FileScoreEntry { file_path: "a.cs".into(), lines: vec![3, 1, 2], tf_idf: 1.0, occurrences: 3, terms_matched: 1, per_term_occurrences: vec![3] });
    scores.insert(1, FileScoreEntry { file_path: "b.cs".into(), lines: vec![5], tf_idf: 2.0, occurrences: 1, terms_matched: 1, per_term_occurrences: vec![1] });

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
    scores.insert(0, FileScoreEntry { file_path: "a.cs".into(), lines: vec![1], tf_idf: 1.0, occurrences: 1, terms_matched: 2, per_term_occurrences: vec![1, 1] });
    scores.insert(1, FileScoreEntry { file_path: "b.cs".into(), lines: vec![1], tf_idf: 2.0, occurrences: 1, terms_matched: 1, per_term_occurrences: vec![1] });

    let (results, total_files, _total_occ) = finalize_grep_results(scores, true, 2);
    assert_eq!(total_files, 1);
    assert_eq!(results[0].file_path, "a.cs");
}

#[test]
fn test_finalize_dedup_lines() {
    let mut scores = HashMap::new();
    scores.insert(0, FileScoreEntry { file_path: "a.cs".into(), lines: vec![5, 3, 5, 1, 3], tf_idf: 1.0, occurrences: 5, terms_matched: 1, per_term_occurrences: vec![5] });

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
        10, 42, &json!(["term1"]), "or", &index, elapsed, &ctx, false, 0.0,
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
        10, 42, &json!(["term1"]), "or", &index, elapsed, &ctx, true, 0.0,
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
        0, 0, &json!(["x"]), "or", &index, elapsed, &ctx, false, 0.0,
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
        0, 0, &json!(["x"]), "or", &index, elapsed, &ctx, false, 0.0,
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
        0, 0, &json!(["x"]), "or", &index, elapsed, &ctx, false, 0.0,
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
        0, 0, &json!(["x"]), "or", &index, elapsed, &ctx, false, 0.0,
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
    let args = json!({"terms": ["hello"]});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert_eq!(result.terms, vec!["hello".to_string()]);
    assert!(result.use_substring); // default
    assert!(!result.use_regex);
    assert!(!result.use_phrase);
    assert!(!result.mode_and);
    assert_eq!(result.max_results, 50); // default
}

#[test]
fn test_parse_grep_args_substring_mutually_exclusive_with_regex() {
    let args = json!({"terms": ["hello"], "regex": true, "substring": true});
    let result = parse_grep_args(&args, "C:/project");
    assert!(result.is_err());
}

#[test]
fn test_parse_grep_args_substring_mutually_exclusive_with_phrase() {
    let args = json!({"terms": ["hello"], "phrase": true, "substring": true});
    let result = parse_grep_args(&args, "C:/project");
    assert!(result.is_err());
}

#[test]
fn test_parse_grep_args_regex_auto_disables_substring() {
    let args = json!({"terms": ["hello"], "regex": true});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(!result.use_substring);
    assert!(result.use_regex);
}

#[test]
fn test_parse_grep_args_context_lines_enables_show_lines() {
    let args = json!({"terms": ["hello"], "contextLines": 3});
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert!(result.show_lines);
    assert_eq!(result.context_lines, 3);
}

#[test]
fn test_parse_grep_args_exclude_lists() {
    let args = json!({
        "terms": ["hello"],
        "excludeDir": ["test", "bin"],
        "exclude": ["mock"]
    });
    let result = parse_grep_args(&args, "C:/project").unwrap();
    assert_eq!(result.exclude_dir, vec!["test", "bin"]);
    assert_eq!(result.exclude, vec!["mock"]);
}

#[test]
fn test_parse_grep_args_dir_as_file_path_auto_converts_by_heuristic() {
    // Non-existent file path but inside server_dir — detected by looks_like_file_path heuristic.
    // Should auto-convert to parent + exact-file scope (NOT a file= substring filter).
    let args = json!({"terms": ["hello"], "dir": "C:/nonexistent/project/src/parser_sql.rs"});
    let parsed = parse_grep_args(&args, "C:/nonexistent/project")
        .expect("heuristic file path should auto-convert, not error");
    // file_filter MUST stay None on auto-convert — it's reserved for explicit user
    // `file=` (substring/comma-OR semantics). Auto-convert pins the FULL path via
    // `exact_file_path` so nested duplicates of the same basename can't leak.
    assert!(parsed.file_filter.is_empty(),
        "auto-convert must not set file_filter (substring path); got: {:?}", parsed.file_filter);
    let exact = parsed.exact_file_path.as_deref()
        .expect("auto-convert must populate exact_file_path with the resolved file path");
    let exact_norm = exact.to_lowercase().replace('\\', "/");
    assert!(exact_norm.ends_with("src/parser_sql.rs"),
        "exact_file_path must be the full resolved path, got: {}", exact);
    let note = parsed.dir_auto_converted_note.expect("note should be set");
    assert!(note.contains("parser_sql.rs"), "note: {}", note);
}

#[test]
fn test_parse_grep_args_explicit_file_filter() {
    // User-provided `file` parameter should be captured verbatim.
    let args = json!({"terms": ["hello"], "file": ["CHANGELOG.md"]});
    let parsed = parse_grep_args(&args, "C:/project").unwrap();
    assert_eq!(parsed.file_filter, vec!["CHANGELOG.md".to_string()]);
    assert!(parsed.dir_auto_converted_note.is_none(),
        "explicit file= should NOT set dir_auto_converted_note");
}

#[test]
fn test_parse_grep_args_explicit_file_wins_over_autoconvert() {
    // If user passes BOTH dir=<file> and file=<something>, explicit file= wins
    // for substring scoping; the dir= path is still pinned via exact_file_path,
    // so the intersection (exact-file AND substring-basename) is the scope.
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("Foo.cs");
    std::fs::write(&file, "x").unwrap();
    let server_dir = tmp.path().to_string_lossy().to_string();
    let args = json!({
        "terms": ["hello"],
        "dir": file.to_string_lossy().to_string(),
        "file": ["ExplicitName"]
    });
    let parsed = parse_grep_args(&args, &server_dir).unwrap();
    assert_eq!(parsed.file_filter, vec!["ExplicitName".to_string()],
        "explicit file= must NOT be overwritten by auto-convert");
    assert!(parsed.exact_file_path.is_some(),
        "auto-convert must still pin exact_file_path even when explicit file= is provided");
    assert!(parsed.dir_auto_converted_note.is_some(),
        "auto-conversion note should still be attached so the LLM sees the hint");
}

#[test]
fn test_parse_grep_args_dir_as_real_file_auto_converts() {
    // dir= pointing to a real file should auto-convert into
    // dir=<parent> + exact_file_path=<full path>, with a hint note attached.
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("test_file.txt");
    std::fs::write(&file, "content").unwrap();
    let server_dir = tmp.path().to_string_lossy().to_string();
    let file_str = file.to_string_lossy().to_string();
    let args = json!({"terms": ["hello"], "dir": file_str});
    let parsed = parse_grep_args(&args, &server_dir)
        .expect("file path in dir= should auto-convert, not error");
    assert!(parsed.file_filter.is_empty(),
        "auto-convert must not set file_filter (that's substring); got: {:?}", parsed.file_filter);
    let exact = parsed.exact_file_path.as_deref()
        .expect("exact_file_path should be populated from the resolved file");
    let exact_norm = exact.to_lowercase().replace('\\', "/");
    assert!(exact_norm.ends_with("test_file.txt"),
        "exact_file_path must be the resolved full path, got: {}", exact);
    assert!(parsed.dir_auto_converted_note.is_some(),
        "dir_auto_converted_note should be set");
    let note = parsed.dir_auto_converted_note.unwrap();
    assert!(note.contains("test_file.txt"), "note: {}", note);
    assert!(note.contains("auto-converted"), "note: {}", note);
}

#[test]
fn test_parse_grep_args_dir_as_directory_accepted() {
    // Directory path should still work
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("subdir");
    std::fs::create_dir_all(&sub).unwrap();
    let server_dir = tmp.path().to_string_lossy().to_string();
    let sub_str = sub.to_string_lossy().to_string();
    let args = json!({"terms": ["hello"], "dir": sub_str});
    let result = parse_grep_args(&args, &server_dir);
    assert!(result.is_ok(), "Directory as dir= should be accepted, got: {:?}", result);
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

    let params = make_params(&None, &[], &[], &[]);
    let terms = vec!["hello".to_string()];
    let scores = score_normal_token_search(&terms, &index, &params);
    assert_eq!(scores.len(), 2);
    assert!(scores.contains_key(&0));
    assert!(scores.contains_key(&1));
    assert_eq!(scores[&0].occurrences, 2);
    assert_eq!(scores[&1].occurrences, 1);
}

#[test]
fn test_score_normal_token_search_no_match() {
    let index = ContentIndex::default();
    let params = make_params(&None, &[], &[], &[]);
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
        count_only: true,
        ..make_params(&None, &[], &[], &[])
    };

    let results = vec![FileScoreEntry {
        file_path: "test.cs".to_string(),
        lines: vec![1, 2],
        tf_idf: 1.0,
        occurrences: 2,
        terms_matched: 1,
        per_term_occurrences: vec![2],
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
    let params = make_params(&None, &[], &[], &[]);

    let results = vec![FileScoreEntry {
        file_path: "test.cs".to_string(),
        lines: vec![1],
        tf_idf: 0.5,
        occurrences: 1,
        terms_matched: 1,
        per_term_occurrences: vec![1],
    }];
    let terms = vec!["hello".to_string()];

    let result = build_grep_response(&results, &terms, 1, 1, "or", &index, &ctx, &params);
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(v.get("files").is_some());
    assert!(v.get("summary").is_some());
}

// ═══════════════════════════════════════════════════════════════════
// GREP-007 / GREP-014 / GREP-015 — input hardening regression tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_parse_grep_args_rejects_empty_terms_grep015() {
    let args = json!({"terms": []});
    let err = parse_grep_args(&args, "C:/project").unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(err_str.contains("at least one entry") || err_str.contains("must not be empty"),
        "empty array should be rejected: {}", err_str);

    // Array of only-whitespace entries normalises to empty after trim/skip.
    let args = json!({"terms": ["", "   "]});
    let err = parse_grep_args(&args, "C:/project").unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(err_str.contains("at least one entry") || err_str.contains("must not be empty"),
        "all-whitespace entries should be rejected: {}", err_str);
}

#[test]
fn test_parse_grep_args_rejects_oversized_max_results_grep007() {
    let args = json!({"terms": ["hello"], "maxResults": 10_000_001u64});
    let err = parse_grep_args(&args, "C:/project").unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(err_str.contains("maxResults must be 0..=10000"), "{}", err_str);
}

#[test]
fn test_parse_grep_args_rejects_oversized_context_lines_grep007() {
    let args = json!({"terms": ["hello"], "contextLines": 1_000_000u64});
    let err = parse_grep_args(&args, "C:/project").unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(err_str.contains("contextLines must be 0..=50"), "{}", err_str);
}

#[test]
fn test_parse_grep_args_accepts_max_results_at_cap_grep007() {
    let args = json!({"terms": ["hello"], "maxResults": 10_000u64});
    let parsed = parse_grep_args(&args, "C:/project").unwrap();
    assert_eq!(parsed.max_results, 10_000);
}

#[test]
fn test_expand_regex_terms_dedups_overlapping_matches_grep014() {
    use crate::ContentIndex;
    let mut index = ContentIndex::default();
    index.index.insert("UserService".to_string(), vec![]);
    index.index.insert("UserController".to_string(), vec![]);
    index.index.insert("OrderService".to_string(), vec![]);

    // Two patterns that both match `UserService`. Without dedup it would
    // appear twice and double-count its scoring contribution downstream.
    let raw = vec!["User.*".to_string(), ".*Service".to_string()];
    let expanded = expand_regex_terms(&raw, &index).unwrap();
    let user_service_count = expanded.iter().filter(|s| s.as_str() == "UserService").count();
    assert_eq!(user_service_count, 1, "expanded={:?}", expanded);
    // Verify the result is sorted (precondition for dedup).
    let mut sorted = expanded.clone();
    sorted.sort();
    assert_eq!(expanded, sorted);
}

// ─── line_regex_perf_hint tests (AC-1) ──────────────────────────

fn perf_hint(
    search_mode: &str,
    search_elapsed_ms: u64,
    index_files: usize,
    prefilter_used: bool,
) -> Option<String> {
    line_regex_perf_hint(
        search_mode,
        search_elapsed_ms,
        index_files,
        prefilter_used,
        None,
        None,
    )
}

fn sample_line_regex_scan(
    read_ms: u64,
    whole_file_precheck_ms: u64,
    line_eval_ms: u64,
    residual_ms: u64,
) -> LineRegexScanTelemetry {
    let measured_scan_ms = read_ms
        .saturating_add(whole_file_precheck_ms)
        .saturating_add(line_eval_ms)
        .saturating_add(residual_ms);
    LineRegexScanTelemetry {
        scan_duration: std::time::Duration::from_millis(measured_scan_ms),
        read_duration: std::time::Duration::from_millis(read_ms),
        whole_file_precheck_duration: std::time::Duration::from_millis(whole_file_precheck_ms),
        line_eval_duration: std::time::Duration::from_millis(line_eval_ms),
        files_visited: 60_000,
        files_skipped_by_prefilter: 59_900,
        files_read: 100,
        ..LineRegexScanTelemetry::default()
    }
}

#[test]
fn test_line_regex_perf_hint_fires_on_slow_large_scan() {
    let hint = perf_hint("lineRegex", 5_000, 60_000, false);
    assert!(hint.is_some(), "slow lineRegex on large index should produce a hint");
    let h = hint.unwrap();
    assert!(h.contains("5000ms"), "hint should report elapsed ms; got: {}", h);
    assert!(h.contains("index of 60000 files"), "hint should report the indexed-files upper bound, not imply all were scanned; got: {}", h);
    assert!(h.contains("trigram"), "hint should explain WHY (literal-trigram prefilter could not help); got: {}", h);
    assert!(h.contains("terms="), "hint should suggest the substring alternative; got: {}", h);
    // Reviewer fix: must use the real xray_help parameter name (`tool=...`),
    // not the invented `topic=...`. Pinning the literal so the contract
    // cannot drift again.
    assert!(h.contains("xray_help tool=\"xray_grep\""),
        "hint must point at the real xray_help argument syntax; got: {}", h);
}

#[test]
fn test_line_regex_perf_hint_fires_on_lineregex_and_mode() {
    // `mode_and` lineRegex emits searchMode="lineRegex-and" — must trigger via prefix match.
    let hint = perf_hint("lineRegex-and", 3_000, 5_000, false);
    assert!(hint.is_some(), "lineRegex-and should also trigger the hint");
}

#[test]
fn test_line_regex_perf_hint_silent_for_substring_modes() {
    // Negative cases: every non-lineRegex searchMode that build_grep_base_summary
    // can emit must NOT carry a lineRegex perf hint.
    assert!(perf_hint("substring-or", 60_000, 100_000, false).is_none());
    assert!(perf_hint("substring-and", 60_000, 100_000, false).is_none());
    assert!(perf_hint("phrase", 60_000, 100_000, false).is_none());
    assert!(perf_hint("phrase-or", 60_000, 100_000, false).is_none());
    assert!(perf_hint("phrase-and", 60_000, 100_000, false).is_none());
    assert!(perf_hint("regex", 60_000, 100_000, false).is_none());
    assert!(perf_hint("or", 60_000, 100_000, false).is_none());
    assert!(perf_hint("and", 60_000, 100_000, false).is_none());
}

#[test]
fn test_line_regex_perf_hint_silent_for_fast_scan() {
    // Below LINE_REGEX_SLOW_MS — no hint, even on a huge index.
    assert!(perf_hint("lineRegex", 1_999, 100_000, false).is_none());
    assert!(perf_hint("lineRegex", 0, 100_000, false).is_none());
}

#[test]
fn test_line_regex_perf_hint_silent_for_small_index() {
    // Below LINE_REGEX_LARGE_INDEX_FILES — slow scans on tiny repos are unactionable.
    assert!(perf_hint("lineRegex", 10_000, 999, false).is_none());
    assert!(perf_hint("lineRegex", 10_000, 0, false).is_none());
}

#[test]
fn test_line_regex_perf_hint_threshold_boundaries() {
    // Exactly at thresholds should fire (>= comparisons).
    assert!(perf_hint("lineRegex", LINE_REGEX_SLOW_MS, LINE_REGEX_LARGE_INDEX_FILES, false).is_some());
}

#[test]
fn test_line_regex_perf_hint_prefilter_used_branch_message_differs() {
    // AC-4: when the literal-trigram prefilter actually ran, a still-slow
    // search emits a different hint copy (talks about per-line regex cost,
    // NOT about adding a substring prefilter — the prefilter is already on).
    let no_prefilter = perf_hint("lineRegex", 5_000, 60_000, false).unwrap();
    let with_prefilter = perf_hint("lineRegex", 5_000, 60_000, true).unwrap();
    assert_ne!(no_prefilter, with_prefilter,
        "prefilter_used should change the hint copy");
    assert!(with_prefilter.contains("even with the literal-trigram prefilter"),
        "prefilter-on hint should acknowledge the prefilter ran; got: {}", with_prefilter);
    assert!(no_prefilter.contains("could not narrow the search"),
        "prefilter-off hint should explain why the prefilter did not help; got: {}", no_prefilter);
}


#[test]
fn test_line_regex_perf_hint_prefilter_used_uses_measured_file_read_phase() {
    let telemetry = sample_line_regex_scan(500, 10, 5, 0);
    let hint = line_regex_perf_hint(
        "lineRegex",
        5_000,
        60_000,
        true,
        Some(&telemetry),
        None,
    ).unwrap();
    assert!(hint.contains("literal-trigram prefilter narrowed"),
        "prefilter-on hint should acknowledge the prefilter ran; got: {}", hint);
    assert!(hint.contains("file reads dominate"),
        "hint should report the measured file-read bottleneck; got: {}", hint);
    assert!(!hint.contains("per-line regex evaluation is the dominant phase"),
        "hint must not claim line evaluation dominates when readMs dominates; got: {}", hint);
}

#[test]
fn test_line_regex_perf_hint_reports_line_eval_only_when_measured() {
    let telemetry = sample_line_regex_scan(5, 10, 500, 0);
    let hint = line_regex_perf_hint(
        "lineRegex",
        5_000,
        60_000,
        true,
        Some(&telemetry),
        None,
    ).unwrap();
    assert!(hint.contains("per-line regex evaluation is the dominant phase"),
        "hint should report line evaluation only when telemetry supports it; got: {}", hint);
}

#[test]
fn test_line_regex_perf_hint_reports_scan_residual_phase() {
    let telemetry = sample_line_regex_scan(5, 10, 5, 500);
    let hint = line_regex_perf_hint(
        "lineRegex",
        5_000,
        60_000,
        false,
        Some(&telemetry),
        Some("candidate set covers 50000/60000 files (>50% threshold)"),
    ).unwrap();
    assert!(hint.contains("attempted but did not narrow"),
        "hint should preserve attempted-prefilter context; got: {}", hint);
    assert!(hint.contains("residual scan-loop overhead"),
        "hint should report residual scan overhead when named phases do not explain scanMs; got: {}", hint);
}

#[test]
fn test_line_regex_perf_hint_reports_response_build_phase() {
    let mut telemetry = sample_line_regex_scan(5, 10, 5, 0);
    telemetry.response_build_duration = std::time::Duration::from_millis(500);
    telemetry.response_finalize_duration = std::time::Duration::from_millis(100);
    let hint = line_regex_perf_hint(
        "lineRegex",
        5_000,
        60_000,
        true,
        Some(&telemetry),
        None,
    ).unwrap();
    assert!(hint.contains("merge/sort/truncation/response building dominates"),
        "hint should report response construction when that phase dominates; got: {}", hint);
}

#[test]
fn test_line_regex_perf_hint_reports_response_finalize_phase() {
    let mut telemetry = sample_line_regex_scan(5, 10, 5, 0);
    telemetry.response_finalize_duration = std::time::Duration::from_millis(500);
    let hint = line_regex_perf_hint(
        "lineRegex",
        5_000,
        60_000,
        true,
        Some(&telemetry),
        None,
    ).unwrap();
    assert!(hint.contains("merge/sort/truncation/response building dominates"),
        "hint should report response finalization in the response bucket; got: {}", hint);
}

#[test]
fn test_apply_literal_prefilter_summary_attempted_but_discarded_overrides_default_hint() {
    // AC-4 round-3 (commit-reviewer R1 MINOR-1): when the prefilter was
    // ATTEMPTED but discarded (short-circuit, OR-bail, fragments-too-short),
    // the default "no extractable required-substring prefix" hint installed
    // by `build_grep_base_summary` is misleading -- the regex DID have an
    // extractable literal, the prefilter just chose not to use it.
    // `apply_literal_prefilter_summary` must replace that copy with one that
    // points the user at `summary.literalPrefilter.reason`.
    use serde_json::json;
    let mut summary = json!({
        "perfHint": "lineRegex took 5000ms ... could not narrow the search ...",
    });
    let info = LiteralPrefilterInfo {
        used: false,
        candidate_files: 0,
        total_files: 60_000,
        extracted_fragments: vec!["app".into()],
        short_circuited: true,
        reason: Some("candidate set covers 50000/60000 files (>50% threshold)".into()),
        total_files_after_scope: None,
        candidate_files_after_scope: None,
    };
    let telemetry = sample_line_regex_scan(500, 10, 5, 0);
    apply_literal_prefilter_summary(&mut summary, &info, &telemetry, 5_000, "lineRegex");
    let hint = summary["perfHint"].as_str().expect("perfHint should be a string");
    assert!(hint.contains("attempted but did not narrow"),
        "discarded hint must acknowledge the attempt; got: {}", hint);
    assert!(hint.contains("candidate set covers 50000/60000"),
        "discarded hint must surface the actual reason; got: {}", hint);
    assert!(!hint.contains("could not narrow the search"),
        "default 'no extractable prefix' copy must NOT survive when prefilter was attempted; got: {}", hint);
}

#[test]
fn test_apply_literal_prefilter_summary_no_reason_leaves_default_hint() {
    // AC-4 round-3: third state -- prefilter not even attempted (no reason
    // recorded). The helper must NOT clobber the default hint installed by
    // `build_grep_base_summary`.
    use serde_json::json;
    let original_hint = "original hint from build_grep_base_summary";
    let mut summary = json!({ "perfHint": original_hint });
    let info = LiteralPrefilterInfo {
        used: false,
        candidate_files: 0,
        total_files: 60_000,
        extracted_fragments: vec![],
        short_circuited: false,
        reason: None,
        total_files_after_scope: None,
        candidate_files_after_scope: None,
    };
    let telemetry = sample_line_regex_scan(500, 10, 5, 0);
    apply_literal_prefilter_summary(&mut summary, &info, &telemetry, 5_000, "lineRegex");
    assert_eq!(summary["perfHint"].as_str(), Some(original_hint),
        "perfHint must be preserved when prefilter was not attempted (reason=None)");
}

