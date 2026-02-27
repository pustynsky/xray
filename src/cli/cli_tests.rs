use super::*;
use std::collections::HashMap;
use crate::{ContentIndex, Posting, TrigramIndex};

// ─── Helper: build a minimal ContentIndex from known data ────────

/// Build a test ContentIndex with given files, tokens→postings, and token counts.
/// Also builds a valid TrigramIndex from the token set.
fn make_test_index(
    files: Vec<&str>,
    postings: Vec<(&str, Vec<Posting>)>,
    file_token_counts: Vec<u32>,
) -> ContentIndex {
    let files: Vec<String> = files.into_iter().map(|s| s.to_string()).collect();
    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    for (token, posts) in postings {
        index.insert(token.to_string(), posts);
    }

    // Build trigram index from the token keys
    let mut tokens: Vec<String> = index.keys().cloned().collect();
    tokens.sort();
    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        for tri in search_index::generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }

    ContentIndex {
        root: ".".to_string(),
        files,
        index,
        file_token_counts,
        trigram: TrigramIndex { tokens, trigram_map },
        ..Default::default()
    }
}

// ═══════════════════════════════════════════════════════════════════
//  file_matches_filters() tests — pure predicate, no index needed
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_file_matches_filters_no_filters() {
    // No filters → everything passes
    assert!(file_matches_filters("src/main.cs", &None, &[], &[]));
}

#[test]
fn test_file_matches_filters_ext_match() {
    let ext = Some("cs".to_string());
    assert!(file_matches_filters("src/Service.cs", &ext, &[], &[]));
}

#[test]
fn test_file_matches_filters_ext_no_match() {
    let ext = Some("cs".to_string());
    assert!(!file_matches_filters("src/main.rs", &ext, &[], &[]));
}

#[test]
fn test_file_matches_filters_ext_case_insensitive() {
    let ext = Some("cs".to_string());
    assert!(file_matches_filters("src/Service.CS", &ext, &[], &[]));
    assert!(file_matches_filters("src/Service.Cs", &ext, &[], &[]));
}

#[test]
fn test_file_matches_filters_exclude_dir() {
    let exclude_dir = vec!["tests".to_string()];
    assert!(!file_matches_filters("src/tests/unit.cs", &None, &exclude_dir, &[]));
    assert!(file_matches_filters("src/main.cs", &None, &exclude_dir, &[]));
}

#[test]
fn test_file_matches_filters_exclude_dir_case_insensitive() {
    let exclude_dir = vec!["Tests".to_string()];
    assert!(!file_matches_filters("src/tests/unit.cs", &None, &exclude_dir, &[]));
    assert!(!file_matches_filters("src/TESTS/unit.cs", &None, &exclude_dir, &[]));
}

#[test]
fn test_file_matches_filters_exclude_pattern() {
    let exclude = vec!["mock".to_string()];
    assert!(!file_matches_filters("src/ServiceMock.cs", &None, &[], &exclude));
    assert!(file_matches_filters("src/Service.cs", &None, &[], &exclude));
}

#[test]
fn test_file_matches_filters_exclude_pattern_case_insensitive() {
    let exclude = vec!["Mock".to_string()];
    assert!(!file_matches_filters("src/servicemock.cs", &None, &[], &exclude));
}

#[test]
fn test_file_matches_filters_multiple_excludes() {
    let exclude_dir = vec!["tests".to_string()];
    let exclude = vec!["mock".to_string()];
    // Excluded by dir
    assert!(!file_matches_filters("tests/unit.cs", &None, &exclude_dir, &exclude));
    // Excluded by pattern
    assert!(!file_matches_filters("src/ServiceMock.cs", &None, &exclude_dir, &exclude));
    // Passes both
    assert!(file_matches_filters("src/Service.cs", &None, &exclude_dir, &exclude));
}

#[test]
fn test_file_matches_filters_combined_ext_and_exclude() {
    let ext = Some("cs".to_string());
    let exclude_dir = vec!["tests".to_string()];
    // Wrong extension
    assert!(!file_matches_filters("src/main.rs", &ext, &exclude_dir, &[]));
    // Right extension but excluded dir
    assert!(!file_matches_filters("tests/unit.cs", &ext, &exclude_dir, &[]));
    // Passes both
    assert!(file_matches_filters("src/Service.cs", &ext, &exclude_dir, &[]));
}

#[test]
fn test_file_matches_filters_no_extension_file() {
    let ext = Some("cs".to_string());
    // File with no extension should not match ext filter
    assert!(!file_matches_filters("Makefile", &ext, &[], &[]));
}

// ═══════════════════════════════════════════════════════════════════
//  expand_regex_terms() tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_expand_regex_terms_matches_tokens() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("httpclient".to_string(), vec![]);
    index_keys.insert("httphandler".to_string(), vec![]);
    index_keys.insert("ilogger".to_string(), vec![]);
    index_keys.insert("icache".to_string(), vec![]);

    let terms = vec!["http.*".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();
    assert_eq!(result.len(), 2);
    let mut sorted = result.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["httpclient", "httphandler"]);
}

#[test]
fn test_expand_regex_terms_no_match() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("httpclient".to_string(), vec![]);

    let terms = vec!["zzzznonexistent".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_expand_regex_terms_invalid_regex() {
    let index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    let terms = vec!["[invalid".to_string()];
    let result = expand_regex_terms(&terms, &index_keys);
    assert!(result.is_err());
}

#[test]
fn test_expand_regex_terms_multiple_patterns() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("httpclient".to_string(), vec![]);
    index_keys.insert("ilogger".to_string(), vec![]);
    index_keys.insert("icache".to_string(), vec![]);

    let terms = vec!["http.*".to_string(), "i.*".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();
    // http.* matches httpclient; i.* matches ilogger, icache
    assert_eq!(result.len(), 3);
}

#[test]
fn test_expand_regex_terms_case_insensitive() {
    let mut index_keys: HashMap<String, Vec<Posting>> = HashMap::new();
    index_keys.insert("httpclient".to_string(), vec![]);

    // Pattern is uppercase but matching is case-insensitive ((?i) is added)
    let terms = vec!["HTTPCLIENT".to_string()];
    let result = expand_regex_terms(&terms, &index_keys).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "httpclient");
}

// ═══════════════════════════════════════════════════════════════════
//  expand_substring_terms() tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_expand_substring_terms_matches_via_trigram() {
    // Build a TrigramIndex with known tokens
    let tokens = vec![
        "httpclient".to_string(),
        "httphandler".to_string(),
        "ilogger".to_string(),
    ];
    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        for tri in search_index::generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }
    let trigram_idx = TrigramIndex { tokens, trigram_map };

    // "client" is a substring of "httpclient" only
    let terms = vec!["client".to_string()];
    let result = expand_substring_terms(&terms, &trigram_idx);
    assert_eq!(result, vec!["httpclient"]);
}

#[test]
fn test_expand_substring_terms_matches_multiple() {
    let tokens = vec![
        "httpclient".to_string(),
        "httphandler".to_string(),
        "ilogger".to_string(),
    ];
    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        for tri in search_index::generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }
    let trigram_idx = TrigramIndex { tokens, trigram_map };

    // "http" is a substring of both httpclient and httphandler
    let terms = vec!["http".to_string()];
    let result = expand_substring_terms(&terms, &trigram_idx);
    let mut sorted = result.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["httpclient", "httphandler"]);
}

#[test]
fn test_expand_substring_terms_no_match() {
    let tokens = vec!["httpclient".to_string()];
    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (idx, token) in tokens.iter().enumerate() {
        for tri in search_index::generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }
    let trigram_idx = TrigramIndex { tokens, trigram_map };

    let terms = vec!["zzzzz".to_string()];
    let result = expand_substring_terms(&terms, &trigram_idx);
    assert!(result.is_empty());
}

#[test]
fn test_expand_substring_terms_short_term_linear_scan() {
    // Terms shorter than 3 chars use linear scan (no trigrams)
    let tokens = vec![
        "ax".to_string(),
        "bx".to_string(),
        "abc".to_string(),
    ];
    let trigram_map: HashMap<String, Vec<u32>> = HashMap::new(); // empty — short terms don't use it
    let trigram_idx = TrigramIndex { tokens, trigram_map };

    // "ax" should match "ax" via linear scan
    let terms = vec!["ax".to_string()];
    let result = expand_substring_terms(&terms, &trigram_idx);
    assert_eq!(result, vec!["ax"]);
}

// ═══════════════════════════════════════════════════════════════════
//  expand_grep_terms() tests — routing logic
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_expand_grep_terms_exact_mode() {
    let index = make_test_index(
        vec!["file.cs"],
        vec![("httpclient", vec![Posting { file_id: 0, lines: vec![1] }])],
        vec![10],
    );
    // Exact mode: terms returned as-is, no expansion
    let raw = vec!["myterm".to_string()];
    let result = expand_grep_terms(&raw, &index, false, false).unwrap();
    assert_eq!(result, vec!["myterm"]);
}

#[test]
fn test_expand_grep_terms_substring_mode() {
    let index = make_test_index(
        vec!["file.cs"],
        vec![
            ("httpclient", vec![Posting { file_id: 0, lines: vec![1] }]),
            ("httphandler", vec![Posting { file_id: 0, lines: vec![2] }]),
        ],
        vec![10],
    );
    // Substring mode: "http" should expand to matching tokens
    let raw = vec!["http".to_string()];
    let result = expand_grep_terms(&raw, &index, true, false).unwrap();
    let mut sorted = result.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["httpclient", "httphandler"]);
}

#[test]
fn test_expand_grep_terms_regex_mode() {
    let index = make_test_index(
        vec!["file.cs"],
        vec![
            ("httpclient", vec![Posting { file_id: 0, lines: vec![1] }]),
            ("ilogger", vec![Posting { file_id: 0, lines: vec![2] }]),
        ],
        vec![10],
    );
    // Regex mode: "http.*" should match httpclient
    let raw = vec!["http.*".to_string()];
    let result = expand_grep_terms(&raw, &index, false, true).unwrap();
    assert_eq!(result, vec!["httpclient"]);
}

#[test]
fn test_expand_grep_terms_regex_invalid() {
    let index = make_test_index(vec!["file.cs"], vec![], vec![10]);
    let raw = vec!["[invalid".to_string()];
    let result = expand_grep_terms(&raw, &index, false, true);
    assert!(result.is_err());
}

// ═══════════════════════════════════════════════════════════════════
//  find_phrase_candidates() tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_find_phrase_candidates_all_tokens_found() {
    let index = make_test_index(
        vec!["src/a.cs", "src/b.cs"],
        vec![
            ("new", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    // "new" is in files 0,1. "httpclient" is in file 0 only.
    // Intersection = {0}
    let tokens = vec!["new".to_string(), "httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &None, &[], &[]);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&0));
}

#[test]
fn test_find_phrase_candidates_token_missing() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![
            ("new", vec![Posting { file_id: 0, lines: vec![1] }]),
        ],
        vec![10],
    );
    // "httpclient" is not in the index → empty result
    let tokens = vec!["new".to_string(), "httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &None, &[], &[]);
    assert!(result.is_empty());
}

#[test]
fn test_find_phrase_candidates_with_ext_filter() {
    let index = make_test_index(
        vec!["src/a.cs", "src/b.rs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    // Filter to .cs only
    let ext = Some("cs".to_string());
    let tokens = vec!["httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &ext, &[], &[]);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&0));
}

#[test]
fn test_find_phrase_candidates_with_exclude_dir() {
    let index = make_test_index(
        vec!["src/Service.cs", "tests/ServiceTest.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    let exclude_dir = vec!["tests".to_string()];
    let tokens = vec!["httpclient".to_string()];
    let result = find_phrase_candidates(&index, &tokens, &None, &exclude_dir, &[]);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&0)); // only src/Service.cs
}

#[test]
fn test_find_phrase_candidates_empty_tokens() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![("httpclient", vec![Posting { file_id: 0, lines: vec![1] }])],
        vec![10],
    );
    let tokens: Vec<String> = vec![];
    let result = find_phrase_candidates(&index, &tokens, &None, &[], &[]);
    // No tokens → no candidates (unwrap_or_default returns empty set)
    assert!(result.is_empty());
}

// ═══════════════════════════════════════════════════════════════════
//  score_grep_results() tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_score_grep_results_single_term() {
    // Need 3+ files so IDF > 0 (term appears in 2 of 3 files → IDF = ln(3/2) > 0)
    let index = make_test_index(
        vec!["src/a.cs", "src/b.cs", "src/c.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1, 5] },
                Posting { file_id: 1, lines: vec![3] },
            ]),
            ("othertoken", vec![
                Posting { file_id: 2, lines: vec![1] },
            ]),
        ],
        vec![10, 20, 10],
    );
    let terms = vec!["httpclient".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &[], false, 1);
    assert_eq!(results.len(), 2);
    // File 0 has 2 occurrences in 10 tokens (higher TF) → should rank higher
    assert_eq!(results[0].file_path, "src/a.cs");
    assert_eq!(results[0].occurrences, 2);
    assert_eq!(results[1].file_path, "src/b.cs");
    assert_eq!(results[1].occurrences, 1);
}

#[test]
fn test_score_grep_results_require_all_filters() {
    let index = make_test_index(
        vec!["src/both.cs", "src/only_one.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
            ("ilogger", vec![
                Posting { file_id: 0, lines: vec![2] },
            ]),
        ],
        vec![10, 10],
    );
    let terms = vec!["httpclient".to_string(), "ilogger".to_string()];

    // require_all = true, raw_term_count = 2 → only files with both terms
    let results = score_grep_results(&index, &terms, &None, &[], &[], true, 2);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].file_path, "src/both.cs");
    assert_eq!(results[0].terms_matched, 2);
}

#[test]
fn test_score_grep_results_require_all_false() {
    let index = make_test_index(
        vec!["src/both.cs", "src/only_one.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
            ("ilogger", vec![
                Posting { file_id: 0, lines: vec![2] },
            ]),
        ],
        vec![10, 10],
    );
    let terms = vec!["httpclient".to_string(), "ilogger".to_string()];

    // require_all = false → both files returned (OR)
    let results = score_grep_results(&index, &terms, &None, &[], &[], false, 2);
    assert_eq!(results.len(), 2);
}

#[test]
fn test_score_grep_results_with_ext_filter() {
    let index = make_test_index(
        vec!["src/a.cs", "src/b.rs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    let ext = Some("cs".to_string());
    let terms = vec!["httpclient".to_string()];
    let results = score_grep_results(&index, &terms, &ext, &[], &[], false, 1);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].file_path, "src/a.cs");
}

#[test]
fn test_score_grep_results_with_exclude() {
    let index = make_test_index(
        vec!["src/Service.cs", "src/ServiceMock.cs"],
        vec![
            ("httpclient", vec![
                Posting { file_id: 0, lines: vec![1] },
                Posting { file_id: 1, lines: vec![1] },
            ]),
        ],
        vec![10, 10],
    );
    let exclude = vec!["mock".to_string()];
    let terms = vec!["httpclient".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &exclude, false, 1);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].file_path, "src/Service.cs");
}

#[test]
fn test_score_grep_results_empty_terms() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![("httpclient", vec![Posting { file_id: 0, lines: vec![1] }])],
        vec![10],
    );
    let terms: Vec<String> = vec![];
    let results = score_grep_results(&index, &terms, &None, &[], &[], false, 0);
    assert!(results.is_empty());
}

#[test]
fn test_score_grep_results_lines_deduped_and_sorted() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![
            // Two terms both appear on line 5 → should be deduped
            ("termx", vec![Posting { file_id: 0, lines: vec![5, 3, 5] }]),
            ("termy", vec![Posting { file_id: 0, lines: vec![3, 7] }]),
        ],
        vec![10],
    );
    let terms = vec!["termx".to_string(), "termy".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &[], false, 2);
    assert_eq!(results.len(), 1);
    // Lines should be sorted and deduped: [3, 5, 7]
    assert_eq!(results[0].lines, vec![3, 5, 7]);
}

#[test]
fn test_score_grep_results_tf_idf_ranking() {
    // File A: small file with 2 occurrences of term (high TF)
    // File B: large file with 1 occurrence of term (low TF)
    // File C: doesn't have the term (so IDF = ln(3/2) > 0, not ln(1) = 0)
    let index = make_test_index(
        vec!["small.cs", "large.cs", "unrelated.cs"],
        vec![
            ("targetterm", vec![
                Posting { file_id: 0, lines: vec![1, 2] },
                Posting { file_id: 1, lines: vec![50] },
            ]),
            ("othertoken", vec![
                Posting { file_id: 2, lines: vec![1] },
            ]),
        ],
        vec![5, 500, 10], // small file has 5 tokens, large has 500, unrelated has 10
    );
    let terms = vec!["targetterm".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &[], false, 1);
    assert_eq!(results.len(), 2);
    // Small file should rank first (higher TF-IDF)
    assert_eq!(results[0].file_path, "small.cs");
    assert!(results[0].tf_idf > results[1].tf_idf);
}

#[test]
fn test_score_grep_results_nonexistent_term() {
    let index = make_test_index(
        vec!["src/a.cs"],
        vec![("httpclient", vec![Posting { file_id: 0, lines: vec![1] }])],
        vec![10],
    );
    let terms = vec!["nonexistent".to_string()];
    let results = score_grep_results(&index, &terms, &None, &[], &[], false, 1);
    assert!(results.is_empty());
}
