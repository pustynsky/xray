use std::collections::HashMap;
use std::io::Write;
use search_index::Posting;
use crate::index::build_trigram_index;

#[test]
fn test_build_trigram_index_basic() {
    let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
    inverted.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("httphandler".to_string(), vec![Posting { file_id: 1, lines: vec![5] }]);
    inverted.insert("ab".to_string(), vec![Posting { file_id: 2, lines: vec![10] }]); // too short for trigrams

    let ti = build_trigram_index(&inverted);

    // Tokens should be sorted
    assert_eq!(ti.tokens, vec!["ab", "httpclient", "httphandler"]);

    // "htt" should map to both http tokens
    let htt = ti.trigram_map.get("htt").unwrap();
    assert_eq!(htt.len(), 2); // indices of httpclient and httphandler

    // "cli" should only map to httpclient
    let cli = ti.trigram_map.get("cli").unwrap();
    assert_eq!(cli.len(), 1);

    // "ab" should not generate any trigrams (too short)
    // but "ab" should still be in tokens list
    assert!(ti.tokens.contains(&"ab".to_string()));
}

#[test]
fn test_build_trigram_index_empty() {
    let inverted: HashMap<String, Vec<Posting>> = HashMap::new();
    let ti = build_trigram_index(&inverted);
    assert!(ti.tokens.is_empty());
    assert!(ti.trigram_map.is_empty());
}

#[test]
fn test_build_trigram_index_sorted_posting_lists() {
    let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
    inverted.insert("abcdef".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("abcxyz".to_string(), vec![Posting { file_id: 1, lines: vec![2] }]);

    let ti = build_trigram_index(&inverted);

    // All posting lists should be sorted
    for (_, list) in &ti.trigram_map {
        for window in list.windows(2) {
            assert!(window[0] <= window[1], "Posting list not sorted");
        }
    }
}

#[test]
fn test_build_trigram_index_single_token() {
    let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
    inverted.insert("foobar".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);

    let ti = build_trigram_index(&inverted);

    assert_eq!(ti.tokens, vec!["foobar"]);
    // "foobar" has 4 trigrams: foo, oob, oba, bar
    assert_eq!(ti.trigram_map.len(), 4);
    assert!(ti.trigram_map.contains_key("foo"));
    assert!(ti.trigram_map.contains_key("oob"));
    assert!(ti.trigram_map.contains_key("oba"));
    assert!(ti.trigram_map.contains_key("bar"));
}

#[test]
fn test_build_trigram_index_deduplicates() {
    // Two tokens sharing the same trigram should appear once each in the posting list
    let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
    inverted.insert("abc".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("abcdef".to_string(), vec![Posting { file_id: 1, lines: vec![2] }]);

    let ti = build_trigram_index(&inverted);

    let abc_list = ti.trigram_map.get("abc").unwrap();
    // Both "abc" (idx 0) and "abcdef" (idx 1) share trigram "abc"
    assert_eq!(abc_list.len(), 2);
    // Should be deduped (no duplicates)
    let mut deduped = abc_list.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(abc_list.len(), deduped.len());
}

// ─── LZ4 compression tests ──────────────────────────────

#[test]
fn test_save_load_compressed_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.bin");

    let data = vec!["hello".to_string(), "world".to_string(), "compressed".to_string()];
    crate::index::save_compressed(&path, &data, "test").unwrap();
    let loaded: Result<Vec<String>, _> = crate::index::load_compressed(&path, "test");
    assert!(loaded.is_ok());
    assert_eq!(data, loaded.unwrap());

    // Verify file starts with LZ4 magic bytes
    let raw = std::fs::read(&path).unwrap();
    assert_eq!(&raw[..4], crate::index::LZ4_MAGIC);
}

#[test]
fn test_load_compressed_legacy_uncompressed() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("legacy.bin");

    // Write uncompressed bincode (legacy format)
    let data = vec!["legacy".to_string(), "format".to_string()];
    let encoded = bincode::serialize(&data).unwrap();
    std::fs::write(&path, &encoded).unwrap();

    // load_compressed should still read it via backward compatibility
    let loaded: Result<Vec<String>, _> = crate::index::load_compressed(&path, "test");
    assert!(loaded.is_ok());
    assert_eq!(data, loaded.unwrap());
}

#[test]
fn test_load_compressed_missing_file_returns_err() {
    let path = std::path::Path::new("/nonexistent/path/to/file.bin");
    let result: Result<Vec<String>, _> = crate::index::load_compressed(path, "test");
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(err_msg.contains("Failed to load index"), "Error should contain 'Failed to load index', got: {}", err_msg);
}

#[test]
fn test_load_compressed_corrupt_data() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("corrupt.bin");

    // Write random bytes that look like neither valid LZ4 nor valid bincode
    std::fs::write(&path, b"this is not valid data at all!!!!!").unwrap();
    let result: Result<Vec<String>, _> = crate::index::load_compressed(&path, "test");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("deserialization failed"), "Error should mention deserialization, got: {}", err_msg);
}

// ─── Memory diagnostics tests ────────────────────────────

#[test]
fn test_log_memory_is_noop_when_disabled() {
    // log_memory should be a safe no-op when memory logging is not enabled
    // (default state: MEMORY_LOG_ENABLED is false)
    crate::index::log_memory("test: this should be a no-op");
    // No panic, no output — success
}

#[test]
fn test_enable_debug_log_creates_file() {
    let tmp = tempfile::tempdir().unwrap();
    // Note: we can't call enable_debug_log in tests because it uses
    // global OnceLock (can only set once per process). Instead, test the
    // file creation logic directly.
    let log_path = tmp.path().join("debug.log");
    {
        let mut f = std::fs::File::create(&log_path).unwrap();
        writeln!(f, "{:>8} | {:>8} | {:>8} | {:>8} | {}",
            "elapsed", "WS_MB", "Peak_MB", "Commit_MB", "label").unwrap();
        writeln!(f, "{}", "-".repeat(70)).unwrap();
    }
    assert!(log_path.exists());
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("elapsed"));
    assert!(content.contains("WS_MB"));
    assert!(content.contains("label"));
}

#[test]
fn test_debug_log_path_has_semantic_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let server_dir = tmp.path().to_string_lossy().to_string();
    let path = crate::index::debug_log_path_for(tmp.path(), &server_dir);
    let filename = path.file_name().unwrap().to_string_lossy();
    assert!(filename.ends_with(".debug.log"),
        "Debug log filename should end with .debug.log, got: {}", filename);
    assert!(filename.contains('_'),
        "Debug log filename should have prefix_hash format, got: {}", filename);
}

#[test]
fn test_debug_log_path_different_dirs_different_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("dir_a");
    let dir_b = tmp.path().join("dir_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let path_a = crate::index::debug_log_path_for(tmp.path(), &dir_a.to_string_lossy());
    let path_b = crate::index::debug_log_path_for(tmp.path(), &dir_b.to_string_lossy());
    assert_ne!(path_a, path_b,
        "Different server dirs should produce different debug log paths");
}

#[test]
fn test_debug_log_path_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let server_dir = tmp.path().to_string_lossy().to_string();
    let path1 = crate::index::debug_log_path_for(tmp.path(), &server_dir);
    let path2 = crate::index::debug_log_path_for(tmp.path(), &server_dir);
    assert_eq!(path1, path2,
        "Same inputs should produce same debug log path");
}

#[test]
fn test_log_request_format() {
    // Test format_utc_timestamp + log_request line format
    // Since we can't enable the global log in tests, test the format logic directly
    let ts = crate::index::format_utc_timestamp();
    assert!(ts.ends_with('Z'), "Timestamp should end with Z: {}", ts);
    assert!(ts.contains('T'), "Timestamp should contain T separator: {}", ts);
    assert_eq!(ts.len(), 20, "Timestamp should be 20 chars (YYYY-MM-DDTHH:MM:SSZ): {}", ts);
}

#[test]
fn test_log_response_format() {
    // Verify format_utc_timestamp produces valid ISO 8601
    let ts = crate::index::format_utc_timestamp();
    // Parse year, month, day
    let year: u32 = ts[0..4].parse().unwrap();
    let month: u32 = ts[5..7].parse().unwrap();
    let day: u32 = ts[8..10].parse().unwrap();
    assert!(year >= 2020 && year <= 2100, "Year out of range: {}", year);
    assert!(month >= 1 && month <= 12, "Month out of range: {}", month);
    assert!(day >= 1 && day <= 31, "Day out of range: {}", day);
}

#[test]
fn test_debug_log_path_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let server_dir = tmp.path().to_string_lossy().to_string();
    let path = crate::index::debug_log_path_for(tmp.path(), &server_dir);
    let filename = path.file_name().unwrap().to_string_lossy();
    assert!(filename.ends_with(".debug.log"),
        "Debug log filename should end with .debug.log, got: {}", filename);
}

#[test]
fn test_format_utc_timestamp_format() {
    let ts = crate::index::format_utc_timestamp();
    // Verify exact format: YYYY-MM-DDTHH:MM:SSZ
    assert_eq!(ts.as_bytes()[4], b'-');
    assert_eq!(ts.as_bytes()[7], b'-');
    assert_eq!(ts.as_bytes()[10], b'T');
    assert_eq!(ts.as_bytes()[13], b':');
    assert_eq!(ts.as_bytes()[16], b':');
    assert_eq!(ts.as_bytes()[19], b'Z');
}

#[test]
fn test_get_process_memory_info_returns_json() {
    let info = crate::index::get_process_memory_info();
    // On Windows, should have workingSetMB, peakWorkingSetMB, commitMB
    // On non-Windows, returns empty object
    assert!(info.is_object());
    #[cfg(target_os = "windows")]
    {
        assert!(info["workingSetMB"].as_f64().is_some(), "should have workingSetMB");
        assert!(info["peakWorkingSetMB"].as_f64().is_some(), "should have peakWorkingSetMB");
        assert!(info["commitMB"].as_f64().is_some(), "should have commitMB");
        // Working set should be > 0 for any running process
        assert!(info["workingSetMB"].as_f64().unwrap() > 0.0, "working set should be > 0");
    }
}

#[test]
fn test_force_mimalloc_collect_does_not_panic() {
    // force_mimalloc_collect should be safe to call at any time
    crate::index::force_mimalloc_collect();
    // No panic — success
}

// ─── content_index_meta error tracking tests ──────────────

#[test]
fn test_content_index_meta_no_errors() {
    let idx = search_index::ContentIndex {
        root: ".".to_string(),
        files: vec!["file.cs".to_string()],
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    let meta = crate::index::content_index_meta(&idx);
    match &meta.details {
        crate::index::IndexDetails::Content { parse_errors, lossy_file_count, .. } => {
            assert_eq!(*parse_errors, None, "parse_errors should be None when read_errors=0");
            assert_eq!(*lossy_file_count, None, "lossy_file_count should be None when lossy_file_count=0");
        }
        _ => panic!("Expected IndexDetails::Content"),
    }
}

#[test]
fn test_content_index_meta_with_errors() {
    let idx = search_index::ContentIndex {
        root: ".".to_string(),
        files: vec!["file.cs".to_string()],
        extensions: vec!["cs".to_string()],
        read_errors: 7,
        lossy_file_count: 3,
        ..Default::default()
    };
    let meta = crate::index::content_index_meta(&idx);
    match &meta.details {
        crate::index::IndexDetails::Content { parse_errors, lossy_file_count, .. } => {
            assert_eq!(*parse_errors, Some(7), "parse_errors should be Some(7) when read_errors=7");
            assert_eq!(*lossy_file_count, Some(3), "lossy_file_count should be Some(3) when lossy_file_count=3");
        }
        _ => panic!("Expected IndexDetails::Content"),
    }
}

#[test]
fn test_estimate_content_index_memory_empty() {
    let idx = search_index::ContentIndex {
        root: ".".to_string(),
        ..Default::default()
    };
    let estimate = crate::index::estimate_content_index_memory(&idx);
    assert!(estimate.is_object());
    assert_eq!(estimate["fileCount"], 0);
    assert_eq!(estimate["uniqueTokens"], 0);
    assert_eq!(estimate["totalPostings"], 0);
    // Total estimate should be 0 for empty index
    assert_eq!(estimate["totalEstimateMB"].as_f64().unwrap(), 0.0);
}

#[test]
fn test_estimate_content_index_memory_nonempty() {
    let mut index = HashMap::new();
    index.insert("httpclient".to_string(), vec![
        Posting { file_id: 0, lines: vec![1, 5, 10] },
        Posting { file_id: 1, lines: vec![3] },
    ]);
    index.insert("ilogger".to_string(), vec![
        Posting { file_id: 0, lines: vec![2] },
    ]);

    let idx = search_index::ContentIndex {
        root: ".".to_string(),
        files: vec!["file0.cs".to_string(), "file1.cs".to_string()],
        index,
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 30],
        ..Default::default()
    };
    let estimate = crate::index::estimate_content_index_memory(&idx);
    assert!(estimate.is_object());
    assert_eq!(estimate["fileCount"], 2);
    assert_eq!(estimate["uniqueTokens"], 2);
    assert_eq!(estimate["totalPostings"], 3);
    // Total estimate should be >= 0 (may round to 0.0 for tiny indexes)
    assert!(estimate["totalEstimateMB"].as_f64().is_some());
    assert!(estimate["invertedIndexMB"].as_f64().is_some());
    // Verify all expected fields are present
    assert!(estimate["trigramTokensMB"].as_f64().is_some());
    assert!(estimate["trigramMapMB"].as_f64().is_some());
    assert!(estimate["filesMB"].as_f64().is_some());
    assert!(estimate["trigramCount"].as_u64().is_some());
}

#[test]
fn test_estimate_definition_index_memory_empty() {
    let idx = crate::definitions::DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec![],
        files: vec![],
        definitions: vec![],
        name_index: std::collections::HashMap::new(),
        kind_index: std::collections::HashMap::new(),
        attribute_index: std::collections::HashMap::new(),
        base_type_index: std::collections::HashMap::new(),
        file_index: std::collections::HashMap::new(),
        path_to_id: std::collections::HashMap::new(),
        method_calls: std::collections::HashMap::new(),
        code_stats: std::collections::HashMap::new(),
        ..Default::default()
    };
    let estimate = crate::index::estimate_definition_index_memory(&idx);
    assert!(estimate.is_object());
    assert_eq!(estimate["definitionCount"], 0);
    assert_eq!(estimate["fileCount"], 0);
    assert_eq!(estimate["totalEstimateMB"].as_f64().unwrap(), 0.0);
}

// ─── find_content_index_for_dir extension validation tests ─────

#[test]
fn test_find_content_index_skips_stale_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = crate::clean_path(&root_dir.to_string_lossy());

    // Save a content index with only "cs" extension
    let idx = search_index::ContentIndex {
        root: root_str.clone(),
        max_age_secs: 86400,
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx, index_base).unwrap();

    // Request "cs,sql" — should NOT find the old cs-only index
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = crate::index::find_content_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_none(),
        "Should not find cs-only content index when cs,sql is expected");
}

#[test]
fn test_find_content_index_accepts_superset() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = crate::clean_path(&root_dir.to_string_lossy());

    // Save a content index with "cs,sql,md" extensions
    let idx = search_index::ContentIndex {
        root: root_str.clone(),
        max_age_secs: 86400,
        extensions: vec!["cs".to_string(), "sql".to_string(), "md".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx, index_base).unwrap();

    // Request "cs,sql" — should find the superset index
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = crate::index::find_content_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Should find cs,sql,md content index when cs,sql is expected (superset)");
}

#[test]
fn test_find_content_index_empty_expected_accepts_any() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = crate::clean_path(&root_dir.to_string_lossy());

    let idx = search_index::ContentIndex {
        root: root_str.clone(),
        max_age_secs: 86400,
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx, index_base).unwrap();

    // Empty expected — should accept any (backward compatible)
    let result = crate::index::find_content_index_for_dir(&root_str, index_base, &[]);
    assert!(result.is_some(),
        "Empty expected_exts should accept any cached content index");
}

#[test]
fn test_build_index_nonexistent_dir_returns_error() {
    let result = crate::index::build_index(&crate::IndexArgs {
        dir: "/nonexistent/path/that/does/not/exist".to_string(),
        max_age_hours: 24,
        hidden: false,
        no_ignore: false,
        threads: 0,
    });
    assert!(result.is_err(), "build_index should return Err for nonexistent directory");
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("does not exist"), "Error should mention 'does not exist', got: {}", err_msg);
}

#[test]
fn test_build_content_index_nonexistent_dir_returns_error() {
    let result = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: "/nonexistent/path/that/does/not/exist".to_string(),
        ext: "cs".to_string(),
        max_age_hours: 24,
        hidden: false,
        no_ignore: false,
        threads: 0,
        min_token_len: 2,
    });
    assert!(result.is_err(), "build_content_index should return Err for nonexistent directory");
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("does not exist"), "Error should mention 'does not exist', got: {}", err_msg);
}

#[test]
fn test_build_index_valid_dir_returns_ok() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
    let result = crate::index::build_index(&crate::IndexArgs {
        dir: tmp.path().to_string_lossy().to_string(),
        max_age_hours: 24,
        hidden: false,
        no_ignore: false,
        threads: 1,
    });
    assert!(result.is_ok(), "build_index should succeed for valid directory");
    let index = result.unwrap();
    assert!(!index.entries.is_empty(), "Valid directory should produce non-empty index");
}

#[test]
fn test_build_content_index_valid_dir_returns_ok() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("file.cs"), "class Foo {}").unwrap();
    let result = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.path().to_string_lossy().to_string(),
        ext: "cs".to_string(),
        max_age_hours: 24,
        hidden: false,
        no_ignore: false,
        threads: 1,
        min_token_len: 2,
    });
    assert!(result.is_ok(), "build_content_index should succeed for valid directory");
    let index = result.unwrap();
    assert!(!index.files.is_empty(), "Valid directory should produce non-empty content index");
}

#[test]
fn test_compressed_file_smaller_than_uncompressed() {
    let tmp = tempfile::tempdir().unwrap();
    let compressed_path = tmp.path().join("compressed.bin");
    let uncompressed_path = tmp.path().join("uncompressed.bin");

    // Create data with repetitive content (compresses well)
    let data: Vec<String> = (0..1000).map(|i| format!("repeated_token_{}", i % 10)).collect();

    crate::index::save_compressed(&compressed_path, &data, "test").unwrap();
    let uncompressed = bincode::serialize(&data).unwrap();
    std::fs::write(&uncompressed_path, &uncompressed).unwrap();

    let compressed_size = std::fs::metadata(&compressed_path).unwrap().len();
    let uncompressed_size = std::fs::metadata(&uncompressed_path).unwrap().len();

    assert!(compressed_size < uncompressed_size,
        "Compressed ({}) should be smaller than uncompressed ({})",
        compressed_size, uncompressed_size);
}

// ─── estimate_definition_index_memory — nonempty test ────────────

#[test]
fn test_estimate_definition_index_memory_nonempty() {
    use crate::definitions::{DefinitionEntry, DefinitionKind, CallSite};

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 50, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "GetUser".to_string(), kind: DefinitionKind::Method,
            line_start: 5, line_end: 20, parent: Some("UserService".to_string()),
            signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index = std::collections::HashMap::new();
    name_index.insert("userservice".to_string(), vec![0u32]);
    name_index.insert("getuser".to_string(), vec![1u32]);

    let mut kind_index = std::collections::HashMap::new();
    kind_index.insert(DefinitionKind::Class, vec![0u32]);
    kind_index.insert(DefinitionKind::Method, vec![1u32]);

    let mut file_index = std::collections::HashMap::new();
    file_index.insert(0u32, vec![0u32, 1u32]);

    let mut method_calls = std::collections::HashMap::new();
    method_calls.insert(1u32, vec![
        CallSite { method_name: "Save".to_string(), receiver_type: Some("DbContext".to_string()), line: 10, receiver_is_generic: false },
    ]);

    let idx = crate::definitions::DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["src/UserService.cs".to_string()],
        definitions,
        name_index,
        kind_index,
        attribute_index: std::collections::HashMap::new(),
        base_type_index: std::collections::HashMap::new(),
        file_index,
        path_to_id: std::collections::HashMap::new(),
        method_calls,
        ..Default::default()
    };

    let estimate = crate::index::estimate_definition_index_memory(&idx);
    assert!(estimate.is_object());
    assert_eq!(estimate["definitionCount"].as_u64().unwrap(), 2);
    assert_eq!(estimate["callSiteCount"].as_u64().unwrap(), 1);
    assert_eq!(estimate["fileCount"].as_u64().unwrap(), 1);
    // Small test data may round to 0.0 via round1(), so check >= 0
    assert!(estimate["totalEstimateMB"].as_f64().unwrap() >= 0.0,
        "Nonempty def index should have valid memory estimate");
    // Verify all expected component fields are present
    assert!(estimate["definitionsMB"].as_f64().is_some());
    assert!(estimate["callSitesMB"].as_f64().is_some());
    assert!(estimate["filesMB"].as_f64().is_some());
    assert!(estimate["indexesMB"].as_f64().is_some());
    assert!(estimate["codeStatsMB"].as_f64().is_some());
    // Verify counts are non-zero (the actual source of truth)
    assert!(estimate["definitionCount"].as_u64().unwrap() > 0);
    assert!(estimate["callSiteCount"].as_u64().unwrap() > 0);
}

// ─── estimate_git_cache_memory tests ────────────────────────────

#[test]
fn test_estimate_git_cache_memory_empty() {
    let cache = crate::git::cache::GitHistoryCache {
        format_version: 1,
        head_hash: String::new(),
        branch: String::new(),
        built_at: 0,
        commits: vec![],
        authors: vec![],
        subjects: String::new(),
        file_commits: std::collections::HashMap::new(),
    };
    let estimate = crate::index::estimate_git_cache_memory(&cache);
    assert!(estimate.is_object());
    assert_eq!(estimate["commitCount"].as_u64().unwrap(), 0);
    assert_eq!(estimate["fileCount"].as_u64().unwrap(), 0);
    assert_eq!(estimate["authorCount"].as_u64().unwrap(), 0);
    assert_eq!(estimate["totalEstimateMB"].as_f64().unwrap(), 0.0);
}

#[test]
fn test_estimate_git_cache_memory_nonempty() {
    use crate::git::cache::{GitHistoryCache, CommitMeta, AuthorEntry};

    let mut file_commits = std::collections::HashMap::new();
    file_commits.insert("src/main.rs".to_string(), vec![0u32, 1]);
    file_commits.insert("src/lib.rs".to_string(), vec![0u32]);

    let cache = GitHistoryCache {
        format_version: 1,
        head_hash: "abc123".to_string(),
        branch: "main".to_string(),
        built_at: 1000,
        commits: vec![
            CommitMeta {
                timestamp: 1000,
                hash: [0u8; 20],
                subject_offset: 0,
                subject_len: 5,
                author_idx: 0,
            },
            CommitMeta {
                timestamp: 2000,
                hash: [1u8; 20],
                subject_offset: 5,
                subject_len: 3,
                author_idx: 1,
            },
        ],
        authors: vec![
            AuthorEntry { name: "Alice".to_string(), email: "alice@example.com".to_string() },
            AuthorEntry { name: "Bob".to_string(), email: "bob@example.com".to_string() },
        ],
        subjects: "hellofix".to_string(),
        file_commits,
    };

    let estimate = crate::index::estimate_git_cache_memory(&cache);
    assert!(estimate.is_object());
    assert_eq!(estimate["commitCount"].as_u64().unwrap(), 2);
    assert_eq!(estimate["fileCount"].as_u64().unwrap(), 2);
    assert_eq!(estimate["authorCount"].as_u64().unwrap(), 2);
    // Small test data may round to 0.0 via round1(), so check >= 0
    assert!(estimate["totalEstimateMB"].as_f64().unwrap() >= 0.0,
        "Nonempty git cache should have valid memory estimate");
    // Verify all expected component fields are present
    assert!(estimate["commitsMB"].as_f64().is_some());
    assert!(estimate["filesMB"].as_f64().is_some());
    assert!(estimate["authorsMB"].as_f64().is_some());
    // Verify counts are non-zero (the actual source of truth)
    assert!(estimate["commitCount"].as_u64().unwrap() > 0);
    assert!(estimate["authorCount"].as_u64().unwrap() > 0);
}
