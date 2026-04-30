#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use std::collections::HashMap;
use code_xray::Posting;
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
    for list in ti.trigram_map.values() {
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

#[test]
fn test_build_trigram_index_repeated_trigram_within_single_token() {
    // Token "01010" generates trigram "010" at two positions (offset 0 and 2).
    // The posting list for "010" must contain the token's index exactly once.
    let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
    inverted.insert("01010".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("zzzzz".to_string(), vec![Posting { file_id: 1, lines: vec![2] }]);

    let ti = build_trigram_index(&inverted);
    let list = ti.trigram_map.get("010").unwrap();
    assert_eq!(list.len(), 1, "trigram '010' should have exactly 1 entry, got {:?}", list);
}

#[test]
fn test_build_trigram_parallel_matches_serial() {
    // Generate >2000 tokens to exceed the parallel threshold.
    // Build with max_threads=1 (forced serial) and max_threads=4 (forced parallel).
    // Results must be identical.
    use crate::index::build_trigram_index_from_tokens;

    let tokens: Vec<String> = (0..3000)
        .map(|i| format!("token_{:05}_suffix", i))
        .collect();

    let serial = build_trigram_index_from_tokens(tokens.clone(), 1);
    let parallel = build_trigram_index_from_tokens(tokens, 4);

    assert_eq!(serial.tokens, parallel.tokens, "tokens must be identical");
    assert_eq!(serial.trigram_map.len(), parallel.trigram_map.len(),
        "trigram_map size must match");

    for (trigram, serial_list) in &serial.trigram_map {
        let parallel_list = parallel.trigram_map.get(trigram)
            .unwrap_or_else(|| panic!("trigram '{}' missing from parallel result", trigram));
        assert_eq!(serial_list, parallel_list,
            "posting list for trigram '{}' differs", trigram);
    }
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
fn test_create_debug_log_file_creates_file_with_header() {
    let tmp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(tmp.path());
    let server_dir = root.to_string_lossy().to_string();

    let log_path = crate::index::create_debug_log_file(&root, &server_dir).unwrap();

    assert!(log_path.exists());
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("elapsed"));
    assert!(content.contains("WS_MB"));
    assert!(content.contains("label"));
}

#[test]
fn test_create_debug_log_file_returns_error_when_index_base_is_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(tmp.path());
    let index_base = root.join("not-a-directory");
    std::fs::write(&index_base, "not a directory").unwrap();
    let server_dir = root.to_string_lossy().to_string();

    let err = crate::index::create_debug_log_file(&index_base, &server_dir).unwrap_err();

    assert!(err.path().starts_with(&index_base));
    assert!(!err.path().exists());
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
    assert!((2020..=2100).contains(&year), "Year out of range: {}", year);
    assert!((1..=12).contains(&month), "Month out of range: {}", month);
    assert!((1..=31).contains(&day), "Day out of range: {}", day);
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
    let idx = code_xray::ContentIndex {
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
    let idx = code_xray::ContentIndex {
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
    let idx = code_xray::ContentIndex {
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

    let idx = code_xray::ContentIndex {
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
    // New fields from memory optimization
    assert!(estimate["allocatorOverheadMB"].as_f64().is_some(),
        "Should have allocatorOverheadMB field");
    assert!(estimate["allocatorOverheadMB"].as_f64().unwrap() >= 0.0,
        "allocatorOverheadMB should be >= 0");
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
    let idx = code_xray::ContentIndex {
        root: root_str.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
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
    let canonical_tmp = crate::canonicalize_test_root(tmp.path());
    let index_base = canonical_tmp.as_path();

    let root_dir = canonical_tmp.join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = crate::clean_path(&root_dir.to_string_lossy());

    // Save a content index with "cs,sql,md" extensions
    let idx = code_xray::ContentIndex {
        root: root_str.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
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
    let canonical_tmp = crate::canonicalize_test_root(tmp.path());
    let index_base = canonical_tmp.as_path();

    let root_dir = canonical_tmp.join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = crate::clean_path(&root_dir.to_string_lossy());

    let idx = code_xray::ContentIndex {
        root: root_str.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
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
fn test_save_compressed_atomic_no_tmp_left_behind() {
    // Atomic save should not leave a .tmp file after successful save
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.word-search");
    let data: Vec<String> = vec!["hello".to_string(), "world".to_string()];
    crate::index::save_compressed(&path, &data, "test").unwrap();

    assert!(path.exists(), "Target file should exist after save");
    // Verify .tmp file is cleaned up (appended, not with_extension)
    let tmp_path = {
        let mut p = path.as_os_str().to_owned();
        p.push(".tmp");
        std::path::PathBuf::from(p)
    };
    assert!(!tmp_path.exists(), ".tmp file should NOT exist after successful save");
    // Also check wrong .tmp path (with_extension) doesn't exist
    assert!(!path.with_extension("tmp").exists(), "No with_extension tmp file either");

    // Verify the saved file can be loaded back
    let loaded: Vec<String> = crate::index::load_compressed(&path, "test").unwrap();
    assert_eq!(loaded, data);
}

#[test]
fn test_save_compressed_atomic_preserves_old_on_new_save() {
    // Verify that a second save over an existing file works correctly
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.word-search");

    // First save
    let data1: Vec<String> = vec!["first".to_string()];
    crate::index::save_compressed(&path, &data1, "test").unwrap();

    // Second save (overwrite)
    let data2: Vec<String> = vec!["second".to_string(), "updated".to_string()];
    crate::index::save_compressed(&path, &data2, "test").unwrap();

    // Should load the second version
    let loaded: Vec<String> = crate::index::load_compressed(&path, "test").unwrap();
    assert_eq!(loaded, data2);
}

#[test]
fn test_build_index_nonexistent_dir_returns_error() {
    let result = crate::index::build_index(&crate::IndexArgs {
        dir: "/nonexistent/path/that/does/not/exist".to_string(),
        ..Default::default()
    });
    assert!(result.is_err(), "build_index should return Err for nonexistent directory");
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("does not exist"), "Error should mention 'does not exist', got: {}", err_msg);
}

#[test]
fn test_build_content_index_nonexistent_dir_returns_error() {
    let result = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: "/nonexistent/path/that/does/not/exist".to_string(),
        ..Default::default()
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
        threads: 1,
        ..Default::default()
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
        threads: 1,
        ..Default::default()
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

// ─── Content index format_version tests ──────────────────────────

#[test]
fn test_content_index_format_version_correct_loads_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let mut idx = code_xray::ContentIndex::default();
    idx.format_version = code_xray::CONTENT_INDEX_VERSION;
    idx.root = tmp.path().to_string_lossy().to_string();
    idx.extensions = vec!["rs".to_string()];

    // Save to the expected path
    let path = crate::index::content_index_path_for(
        &idx.root, "rs", tmp.path(),
    );
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = crate::index::load_content_index(&idx.root, "rs", tmp.path());
    assert!(result.is_ok(), "Loading content index with correct version should succeed");
    assert_eq!(result.unwrap().format_version, code_xray::CONTENT_INDEX_VERSION);
}

#[test]
fn test_content_index_format_version_mismatch_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let mut idx = code_xray::ContentIndex::default();
    idx.format_version = 999; // wrong version
    idx.root = tmp.path().to_string_lossy().to_string();
    idx.extensions = vec!["rs".to_string()];

    let path = crate::index::content_index_path_for(
        &idx.root, "rs", tmp.path(),
    );
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = crate::index::load_content_index(&idx.root, "rs", tmp.path());
    assert!(result.is_err(), "Loading content index with wrong version should fail");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("format version mismatch"), "Error should mention version mismatch, got: {}", err);
}

#[test]
fn test_content_index_format_version_legacy_zero_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let mut idx = code_xray::ContentIndex::default();
    idx.format_version = 0; // legacy index without version
    idx.root = tmp.path().to_string_lossy().to_string();
    idx.extensions = vec!["rs".to_string()];

    let path = crate::index::content_index_path_for(
        &idx.root, "rs", tmp.path(),
    );
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = crate::index::load_content_index(&idx.root, "rs", tmp.path());
    assert!(result.is_err(), "Loading legacy content index (version 0) should fail");
}

#[test]
fn test_build_content_index_sets_format_version() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("file.rs"), "fn main() {}").unwrap();
    let result = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.path().to_string_lossy().to_string(),
        ext: "rs".to_string(),
        threads: 1,
        ..Default::default()
    });
    assert!(result.is_ok());
    assert_eq!(result.unwrap().format_version, code_xray::CONTENT_INDEX_VERSION,
        "build_content_index should set format_version to CONTENT_INDEX_VERSION");
}

#[test]
fn test_content_index_old_format_without_version_field_does_not_crash() {
    // Simulate an old-format index file by saving a struct WITHOUT format_version
    // using raw bincode. When loaded by new code, the 4-byte shift in binary layout
    // causes garbled Vec lengths. The 2 GB deserialization limit in load_compressed
    // must prevent OOM/abort — returning Err instead.
    #[derive(serde::Serialize)]
    struct OldContentIndex {
        root: String,
        // no format_version field!
        created_at: u64,
        max_age_secs: u64,
        files: Vec<String>,
        index: std::collections::HashMap<String, Vec<code_xray::Posting>>,
        total_tokens: u64,
        extensions: Vec<String>,
        file_token_counts: Vec<u32>,
    }

    let tmp = tempfile::tempdir().unwrap();
    let old_idx = OldContentIndex {
        root: tmp.path().to_string_lossy().to_string(),
        created_at: 1741600000,
        max_age_secs: 86400,
        files: vec!["file.cs".to_string()],
        index: std::collections::HashMap::new(),
        total_tokens: 10,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![10],
    };

    // Save using the same compressed format
    let path = tmp.path().join("test.word-search");
    crate::index::save_compressed(&path, &old_idx, "test").unwrap();

    // Try loading as new ContentIndex — should return Err, NOT crash/abort
    let result = crate::index::load_compressed::<code_xray::ContentIndex>(&path, "test");
    // We don't care whether it returns Ok (with garbled data) or Err —
    // the key assertion is that we REACH this line without crashing.
    // If the deserialization attempted a multi-TB allocation, the process would abort
    // before reaching this assert.
    if let Ok(idx) = &result {
        // If it somehow deserialized, the version should be garbage (not 1)
        assert_ne!(idx.format_version, code_xray::CONTENT_INDEX_VERSION,
            "Old format should not accidentally produce correct version");
    }
    // If Err — that's the expected, correct outcome
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
        path_to_id: {
            let mut m = std::collections::HashMap::new();
            m.insert(std::path::PathBuf::from("src/UserService.cs"), 0u32);
            m
        },
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
    // New fields from memory optimization
    assert!(estimate["allocatorOverheadMB"].as_f64().is_some(),
        "Should have allocatorOverheadMB field");
    assert!(estimate["methodCallsOverheadMB"].as_f64().is_some(),
        "Should have methodCallsOverheadMB field");
    // Verify counts are non-zero (the actual source of truth)
    assert!(estimate["definitionCount"].as_u64().unwrap() > 0);
    assert!(estimate["callSiteCount"].as_u64().unwrap() > 0);
}

// ─── shrink_maps tests ────────────────────────────────────────────

#[test]
fn test_content_index_shrink_maps_preserves_data() {
    let mut index = HashMap::new();
    index.insert("httpclient".to_string(), vec![
        Posting { file_id: 0, lines: vec![1, 5, 10] },
        Posting { file_id: 1, lines: vec![3] },
    ]);
    index.insert("ilogger".to_string(), vec![
        Posting { file_id: 0, lines: vec![2] },
    ]);

    let mut idx = code_xray::ContentIndex {
        root: ".".to_string(),
        files: vec!["file0.cs".to_string(), "file1.cs".to_string()],
        index,
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 30],
        ..Default::default()
    };

    // Record state before shrink
    let tokens_before = idx.index.len();
    let postings_before: usize = idx.index.values().map(|v| v.len()).sum();

    // Shrink should not change data
    idx.shrink_maps();

    assert_eq!(idx.index.len(), tokens_before, "Token count should be preserved after shrink");
    let postings_after: usize = idx.index.values().map(|v| v.len()).sum();
    assert_eq!(postings_after, postings_before, "Posting count should be preserved after shrink");

    // Data should still be accessible
    let httpclient = idx.index.get("httpclient").unwrap();
    assert_eq!(httpclient.len(), 2);
    assert_eq!(httpclient[0].lines, vec![1, 5, 10]);
}

#[test]
fn test_definition_index_shrink_maps_preserves_data() {
    use crate::definitions::{DefinitionEntry, DefinitionKind, CallSite};

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 50, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index = std::collections::HashMap::new();
    name_index.insert("userservice".to_string(), vec![0u32]);

    let mut kind_index = std::collections::HashMap::new();
    kind_index.insert(DefinitionKind::Class, vec![0u32]);

    let mut file_index = std::collections::HashMap::new();
    file_index.insert(0u32, vec![0u32]);

    let mut method_calls = std::collections::HashMap::new();
    method_calls.insert(0u32, vec![
        CallSite { method_name: "Save".to_string(), receiver_type: Some("DbContext".to_string()), line: 10, receiver_is_generic: false },
    ]);

    let mut idx = crate::definitions::DefinitionIndex {
        root: ".".to_string(),
        definitions,
        name_index,
        kind_index,
        file_index,
        method_calls,
        ..Default::default()
    };

    // Record state before shrink
    let name_count = idx.name_index.len();
    let call_count: usize = idx.method_calls.values().map(|v| v.len()).sum();

    // Shrink should not change data
    idx.shrink_maps();

    assert_eq!(idx.name_index.len(), name_count, "name_index count should be preserved");
    let call_count_after: usize = idx.method_calls.values().map(|v| v.len()).sum();
    assert_eq!(call_count_after, call_count, "method_calls count should be preserved");

    // Data should still be accessible
    assert!(idx.name_index.contains_key("userservice"), "name_index should still contain key");
    assert_eq!(idx.method_calls.get(&0).unwrap()[0].method_name, "Save", "CallSite data should be preserved");
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


// ─── Chunked content-build tests ────────────────────────────────────

/// Verify that chunked content-build (CONTENT_CHUNK_SIZE=4096) produces
/// correct results with multiple files — file_ids sequential, all tokens found.
#[test]
fn test_chunked_content_build_multiple_files_correct_file_ids() {
    let tmp = tempfile::tempdir().unwrap();

    // Create 15 files with distinct content
    for i in 0..15 {
        let content = format!(
            "class Service{i} {{\n    void Process{i}() {{ }}\n}}\n",
            i = i
        );
        std::fs::write(tmp.path().join(format!("file{}.cs", i)), content).unwrap();
    }

    let result = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.path().to_string_lossy().to_string(),
        threads: 4, // Multi-threaded to exercise sub-chunking
        ..Default::default()
    });
    assert!(result.is_ok(), "build_content_index should succeed");
    let index = result.unwrap();

    // Should have exactly 15 files
    assert_eq!(index.files.len(), 15, "Should index all 15 files");

    // file_token_counts should have same length as files
    assert_eq!(index.file_token_counts.len(), 15,
        "file_token_counts should match files count");

    // All Service{i} tokens should be found in the index
    for i in 0..15 {
        let class_token = format!("service{}", i);
        assert!(index.index.contains_key(&class_token),
            "Should find token '{}' in index", class_token);
    }

    // Verify file_ids in postings are valid (within files range)
    for (token, postings) in &index.index {
        for posting in postings {
            assert!((posting.file_id as usize) < index.files.len(),
                "Posting for token '{}' has file_id {} but files.len() = {}",
                token, posting.file_id, index.files.len());
        }
    }

    // total_tokens should be positive
    assert!(index.total_tokens > 0, "Should have positive total_tokens");
}

/// Verify file_id → file path mapping is consistent across chunked build.
/// Each file_id should point to the correct file in the files Vec.
#[test]
fn test_chunked_content_build_file_id_to_path_consistency() {
    let tmp = tempfile::tempdir().unwrap();

    // Create files with unique identifiable tokens
    for i in 0..8 {
        let content = format!("uniquetoken{}", i);
        std::fs::write(tmp.path().join(format!("unique{}.cs", i)), content).unwrap();
    }

    let result = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.path().to_string_lossy().to_string(),
        threads: 2,
        ..Default::default()
    });
    let index = result.unwrap();

    // For each unique token, the posting's file_id should point to a file
    // whose path contains the corresponding number
    for i in 0..8 {
        let token = format!("uniquetoken{}", i);
        if let Some(postings) = index.index.get(&token) {
            for posting in postings {
                let file_path = &index.files[posting.file_id as usize];
                assert!(file_path.contains(&format!("unique{}", i)),
                    "Token '{}' posting points to file '{}' which doesn't match expected 'unique{}'",
                    token, file_path, i);
            }
        } else {
            panic!("Token '{}' not found in index", token);
        }
    }
}

/// Verify single-thread and multi-thread content builds produce same token counts.
#[test]
fn test_chunked_content_build_single_vs_multi_thread() {
    let tmp = tempfile::tempdir().unwrap();

    for i in 0..12 {
        let content = format!(
            "namespace App{i} {{ class Controller{i} {{ void Handle{i}() {{ }} }} }}",
            i = i
        );
        std::fs::write(tmp.path().join(format!("ctrl{}.cs", i)), content).unwrap();
    }

    let idx_single = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.path().to_string_lossy().to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();

    let idx_multi = crate::index::build_content_index(&crate::ContentIndexArgs {
        dir: tmp.path().to_string_lossy().to_string(),
        threads: 4,
        ..Default::default()
    }).unwrap();

    assert_eq!(idx_single.files.len(), idx_multi.files.len(),
        "Single and multi-thread should produce same file count");
    assert_eq!(idx_single.index.len(), idx_multi.index.len(),
        "Single and multi-thread should produce same unique token count");
    assert_eq!(idx_single.total_tokens, idx_multi.total_tokens,
        "Single and multi-thread should produce same total token count");
}

// ─── find_content_index_for_dir meta-based optimization tests ─────

/// Verify that find_content_index_for_dir skips non-matching indexes
/// without loading the full index when .meta sidecar files are present.
#[test]
fn test_find_content_index_uses_meta_to_skip_non_matching_root() {
    let tmp = tempfile::tempdir().unwrap();
    let canonical_tmp = crate::canonicalize_test_root(tmp.path());
    let index_base = canonical_tmp.as_path();

    // Create two directories
    let dir_a = canonical_tmp.join("project_a");
    let dir_b = canonical_tmp.join("project_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let root_a = crate::clean_path(&dir_a.to_string_lossy());
    let root_b = crate::clean_path(&dir_b.to_string_lossy());

    // Save content index for project_a
    let idx_a = code_xray::ContentIndex {
        root: root_a.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx_a, index_base).unwrap();

    // Searching for project_b should NOT find project_a's index
    // (meta sidecar has root=project_a which doesn't match project_b)
    let result = crate::index::find_content_index_for_dir(&root_b, index_base, &[]);
    assert!(result.is_none(),
        "Should not find project_a's index when searching for project_b");

    // Searching for project_a SHOULD find it
    let result = crate::index::find_content_index_for_dir(&root_a, index_base, &[]);
    assert!(result.is_some(),
        "Should find project_a's index when searching for project_a");
}

/// Verify that find_content_index_for_dir works when .meta file is missing
/// (fallback to read_root_from_index_file or full load).
#[test]
fn test_find_content_index_works_without_meta_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let canonical_tmp = crate::canonicalize_test_root(tmp.path());
    let index_base = canonical_tmp.as_path();

    let root_dir = canonical_tmp.join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = crate::clean_path(&root_dir.to_string_lossy());

    // Save content index (creates both .word-search and .word-search.meta)
    let idx = code_xray::ContentIndex {
        root: root_str.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string(), "md".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx, index_base).unwrap();

    // Delete the .meta sidecar file to test the fallback path
    for entry in std::fs::read_dir(index_base).unwrap().flatten() {
        let path = entry.path();
        if path.to_string_lossy().ends_with(".meta") {
            std::fs::remove_file(&path).unwrap();
        }
    }

    // Should still find the index via fallback (read_root_from_index_file)
    let result = crate::index::find_content_index_for_dir(&root_str, index_base, &["rs".to_string(), "md".to_string()]);
    assert!(result.is_some(),
        "Should find index even without .meta sidecar (fallback path)");
}

/// Verify that meta-based filtering correctly rejects extension mismatches.
#[test]
fn test_find_content_index_meta_rejects_extension_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = crate::clean_path(&root_dir.to_string_lossy());

    // Save content index with only "rs" extension
    let idx = code_xray::ContentIndex {
        root: root_str.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx, index_base).unwrap();

    // Request "rs,md" — meta should reject because "md" is not in cached extensions
    let expected = vec!["rs".to_string(), "md".to_string()];
    let result = crate::index::find_content_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_none(),
        "Meta-based filtering should reject when cached extensions don't include all expected");
}

// ─── cleanup_stale_same_root_indexes tests ─────

/// Verify that cleanup_stale_same_root_indexes removes old indexes for the same root.
#[test]
fn test_cleanup_stale_same_root_removes_old_index() {
    let tmp = tempfile::tempdir().unwrap();
    let canonical_tmp = crate::canonicalize_test_root(tmp.path());
    let index_base = canonical_tmp.as_path();

    let root_dir = canonical_tmp.join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = crate::clean_path(&root_dir.to_string_lossy());

    // Save content index with "rs" extension
    let idx1 = code_xray::ContentIndex {
        root: root_str.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx1, index_base).unwrap();

    // Count .word-search files
    let count_ws = || -> usize {
        std::fs::read_dir(index_base).unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "word-search"))
            .count()
    };
    assert_eq!(count_ws(), 1, "Should have 1 content index after first save");

    // Save content index with "rs,md" extensions (different hash)
    let idx2 = code_xray::ContentIndex {
        root: root_str.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string(), "md".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx2, index_base).unwrap();
    assert_eq!(count_ws(), 2, "Should have 2 content indexes before cleanup");

    // Now run cleanup (simulating what serve.rs does after background build)
    let new_path = crate::content_index_path_for(&root_str, "rs,md", index_base);
    crate::index::cleanup_stale_same_root_indexes(index_base, &new_path, &root_str, "word-search");

    // Old "rs" index should be cleaned up
    assert_eq!(count_ws(), 1, "Should have 1 content index after cleanup");

    // Verify the remaining index is the new one
    let result = crate::index::find_content_index_for_dir(&root_str, index_base, &["rs".to_string(), "md".to_string()]);
    assert!(result.is_some(), "Should find the new rs,md index");
}

/// Verify that cleanup does NOT remove indexes for different root directories.
#[test]
fn test_cleanup_stale_same_root_does_not_clean_other_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let dir_a = tmp.path().join("project_a");
    let dir_b = tmp.path().join("project_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let root_a = crate::clean_path(&dir_a.to_string_lossy());
    let root_b = crate::clean_path(&dir_b.to_string_lossy());

    // Save content index for project_a with "rs"
    let idx_a = code_xray::ContentIndex {
        root: root_a.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx_a, index_base).unwrap();

    // Save content index for project_b with "rs"
    let idx_b = code_xray::ContentIndex {
        root: root_b.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx_b, index_base).unwrap();

    let count_ws = || -> usize {
        std::fs::read_dir(index_base).unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "word-search"))
            .count()
    };
    assert_eq!(count_ws(), 2, "Should have 2 content indexes (one per project)");

    // Save new content index for project_a with "rs,md"
    let idx_a2 = code_xray::ContentIndex {
        root: root_a.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string(), "md".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx_a2, index_base).unwrap();

    // Run cleanup for project_a
    let new_path = crate::content_index_path_for(&root_a, "rs,md", index_base);
    crate::index::cleanup_stale_same_root_indexes(index_base, &new_path, &root_a, "word-search");

    // Should still have 2 indexes: new project_a + untouched project_b
    assert_eq!(count_ws(), 2, "Should still have 2 content indexes (cleanup only affects same root)");
}

#[test]
fn test_worker_panics_preserved_in_serialization_roundtrip() {
    // Regression test for P0-1: worker_panics must survive bincode round-trip
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_string_lossy().to_string();
    let mut index = code_xray::ContentIndex::default();
    index.root = dir.clone();
    index.format_version = code_xray::CONTENT_INDEX_VERSION;
    index.worker_panics = 3;

    let idx_base = tmp.path();
    crate::save_content_index(&index, idx_base).expect("save failed");
    let loaded = crate::load_content_index(&dir, "", idx_base).expect("load failed");
    assert_eq!(loaded.worker_panics, 3, "worker_panics must survive save/load round-trip");
}

// ─── Case-insensitive path comparison on Windows (regression: path_eq) ─────

/// Save a content index with one casing, look up with a different casing.
/// On Windows must succeed (NTFS is case-insensitive); on Unix must fail
/// (filesystem is case-sensitive). Regression for MAJOR-3 from the
/// 2026-04-20 full-snapshot review: orphan caches accumulated under
/// `%LOCALAPPDATA%\xray` because `meta.root != clean` was case-sensitive
/// while `find_definition_index_for_dir` already used `eq_ignore_ascii_case`.
#[test]
fn test_find_content_index_case_insensitive_lookup_on_windows() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    // Use a non-existent path so canonicalize falls back to the literal —
    // lets us drive both casings deterministically.
    let saved_root = "C:/Repos/UPPER/Project".to_string();
    let lookup_root = "c:/repos/upper/project".to_string();

    let idx = code_xray::ContentIndex {
        root: saved_root.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx, index_base).unwrap();

    let result = crate::index::find_content_index_for_dir(&lookup_root, index_base, &[]);
    if cfg!(windows) {
        assert!(result.is_some(),
            "On Windows, lookup with different casing must find the saved index (path_eq)");
        assert_eq!(result.unwrap().root, saved_root,
            "Returned index must be the one saved under uppercase root");
    } else {
        assert!(result.is_none(),
            "On Unix, lookup with different casing must NOT find the saved index");
    }
}

/// Same as above, but exercises the meta-less fallback path — verifies
/// the `read_root_from_index_file` branch also uses `path_eq`.
#[test]
fn test_find_content_index_case_insensitive_lookup_without_meta() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let saved_root = "C:/Repos/UPPER/Project".to_string();
    let lookup_root = "c:/repos/upper/project".to_string();

    let idx = code_xray::ContentIndex {
        root: saved_root.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx, index_base).unwrap();

    // Drop the .meta sidecar to force the fallback branch.
    for entry in std::fs::read_dir(index_base).unwrap().flatten() {
        let path = entry.path();
        if path.to_string_lossy().ends_with(".meta") {
            std::fs::remove_file(&path).unwrap();
        }
    }

    let result = crate::index::find_content_index_for_dir(&lookup_root, index_base, &[]);
    if cfg!(windows) {
        assert!(result.is_some(),
            "Fallback (no .meta) lookup with different casing must find the saved index on Windows");
    } else {
        assert!(result.is_none(),
            "Fallback (no .meta) lookup with different casing must NOT match on Unix");
    }
}

/// Verify cleanup_stale_same_root_indexes also uses path_eq — without it,
/// stale indexes with different-case root strings would never be removed,
/// silently accumulating in `%LOCALAPPDATA%\xray`.
#[test]
fn test_cleanup_stale_same_root_case_insensitive_on_windows() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let saved_root = "C:/Repos/UPPER/Project".to_string();
    let cleanup_root = "c:/repos/upper/project".to_string();

    let idx_old = code_xray::ContentIndex {
        root: saved_root.clone(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        max_age_secs: 86400,
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    crate::save_content_index(&idx_old, index_base).unwrap();

    // Pretend we just saved a NEW index for the same logical dir but with
    // different casing. The newly_saved_path points at a hash that does not
    // match the old one (different exts), so the old one should be cleaned.
    let new_path = crate::content_index_path_for(&cleanup_root, "rs,md", index_base);
    crate::index::cleanup_stale_same_root_indexes(index_base, &new_path, &cleanup_root, "word-search");

    let count_ws = std::fs::read_dir(index_base).unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "word-search"))
        .count();

    if cfg!(windows) {
        assert_eq!(count_ws, 0,
            "On Windows, stale index with different-case root must be removed");
    } else {
        assert_eq!(count_ws, 1,
            "On Unix, different-case root means different logical dir — must NOT be removed");
    }
}

// ─── Bincode field-order contract: roundtrip tests for header readers ─────
//
// Resolves MAJOR-5 from the 2026-04-20 full-snapshot review: the readers
// `read_root_from_index_file` and `read_format_version_from_index_file`
// rely on a non-obvious contract — `root: String` MUST be the first bincode
// field, and `format_version: u32` MUST be the second. Reordering fields in
// `FileIndex` / `ContentIndex` / `DefinitionIndex` would silently break the
// fast version-check path (returns garbled values) without any compile-time
// or test-time signal. These tests fail loudly if anyone reorders fields,
// encoding the contract executably.

#[test]
fn test_read_root_from_file_index_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sample.file-list");

    let idx = code_xray::FileIndex {
        root: "C:/Repos/SomeProject".to_string(),
        format_version: code_xray::FILE_INDEX_VERSION,
        created_at: 1_700_000_000,
        max_age_secs: 86_400,
        entries: Vec::new(),
        respect_git_exclude: false,
    };

    crate::index::save_compressed(&path, &idx, "test-file-index")
        .expect("save_compressed must succeed");

    let read_root = crate::index::read_root_from_index_file_pub(&path);
    assert_eq!(read_root.as_deref(), Some("C:/Repos/SomeProject"),
        "FileIndex.root must be the first bincode field — reorder = broken header reader");

    let read_version = crate::index::read_format_version_from_index_file(&path);
    assert_eq!(read_version, Some(code_xray::FILE_INDEX_VERSION),
        "FileIndex.format_version must be the second bincode field (immediately after root) — reorder = silently broken stale-index detection");
}

#[test]
fn test_read_root_and_version_from_content_index_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sample.word-search");

    let mut idx = code_xray::ContentIndex::default();
    idx.root = "D:/some/workspace".to_string();
    idx.format_version = code_xray::CONTENT_INDEX_VERSION;
    idx.max_age_secs = 86_400;

    crate::index::save_compressed(&path, &idx, "test-content-index")
        .expect("save_compressed must succeed");

    let read_root = crate::index::read_root_from_index_file_pub(&path);
    assert_eq!(read_root.as_deref(), Some("D:/some/workspace"),
        "ContentIndex.root must be the first bincode field");

    let read_version = crate::index::read_format_version_from_index_file(&path);
    assert_eq!(read_version, Some(code_xray::CONTENT_INDEX_VERSION),
        "ContentIndex.format_version must be the second bincode field (immediately after root) — reorder = silently broken stale-index detection");
}

#[test]
fn test_read_root_and_version_from_definition_index_roundtrip() {
    use crate::definitions::DefinitionIndex;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sample.code-structure");

    let mut idx = DefinitionIndex::default();
    idx.root = "E:/code/proj".to_string();
    idx.format_version = crate::definitions::DEFINITION_INDEX_VERSION;

    crate::index::save_compressed(&path, &idx, "test-definition-index")
        .expect("save_compressed must succeed");

    let read_root = crate::index::read_root_from_index_file_pub(&path);
    assert_eq!(read_root.as_deref(), Some("E:/code/proj"),
        "DefinitionIndex.root must be the first bincode field");

    let read_version = crate::index::read_format_version_from_index_file(&path);
    assert_eq!(read_version, Some(crate::definitions::DEFINITION_INDEX_VERSION),
        "DefinitionIndex.format_version must be the second bincode field");
}

/// Sanity check: round-tripping a non-default version value confirms the
/// reader picks up the actual stored bytes, not a constant. If the version
/// field were repositioned, this test would either return None or read
/// garbage from neighbouring fields.
#[test]
fn test_read_format_version_picks_up_stored_value_not_constant() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sample.word-search");

    let mut idx = code_xray::ContentIndex::default();
    idx.root = "X".to_string();
    idx.format_version = 0xDEAD_BEEF; // distinct sentinel, NOT the live constant

    crate::index::save_compressed(&path, &idx, "sentinel")
        .expect("save_compressed must succeed");

    let read_version = crate::index::read_format_version_from_index_file(&path);
    assert_eq!(read_version, Some(0xDEAD_BEEFu32),
        "version reader must return the actually-stored u32, not a constant or stale read");
}

// ─── Stale `files` counter regression — memory estimate path ────────
//
// Counterpart of `live_file_count_*` tests in watcher/definitions: the
// in-memory size estimate must use live count too, otherwise tombstoned
// slots silently inflate `memoryEstimate.contentIndex.fileCount` in
// `xray_info`. See `user-stories/stale-content-index-files-counter.md`.

#[test]
fn estimate_content_index_memory_uses_live_count_with_tombstones() {
    use std::path::PathBuf;
    let mut p2id = HashMap::new();
    p2id.insert(PathBuf::from("alive_a.cs"), 0u32);
    p2id.insert(PathBuf::from("alive_b.cs"), 2u32);
    let idx = code_xray::ContentIndex {
        root: ".".to_string(),
        // Slot 1 is tombstoned (file removed): empty string + missing from path_to_id.
        files: vec!["alive_a.cs".to_string(), String::new(), "alive_b.cs".to_string()],
        index: HashMap::new(),
        total_tokens: 0,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![5, 0, 5],
        path_to_id: Some(p2id),
        ..Default::default()
    };
    let estimate = crate::index::estimate_content_index_memory(&idx);
    assert_eq!(estimate["fileCount"], 2,
        "fileCount in memory estimate MUST be live count, not Vec capacity");
}

#[test]
fn estimate_definition_index_memory_uses_live_count_with_tombstones() {
    use std::path::PathBuf;
    let mut idx = crate::definitions::DefinitionIndex::default();
    idx.root = ".".to_string();
    idx.files = vec!["alive_a.rs".to_string(), String::new(), "alive_b.rs".to_string()];
    idx.path_to_id.insert(PathBuf::from("alive_a.rs"), 0);
    idx.path_to_id.insert(PathBuf::from("alive_b.rs"), 2);

    let estimate = crate::index::estimate_definition_index_memory(&idx);
    assert_eq!(estimate["fileCount"], 2,
        "fileCount in memory estimate MUST be live count, not Vec capacity");
}


#[test]
fn phase_field_formatting_escapes_line_breaks() {
    let fields = super::format_phase_fields(&[
        ("startupMode", "coldBuild".to_string()),
        ("note", "first\r\nsecond".to_string()),
    ]);

    assert_eq!(fields, "startupMode=coldBuild note=first\\r\\nsecond");
}

#[test]
fn format_debug_path_falls_back_to_filename_only() {
    let rendered = super::format_debug_path(std::path::Path::new("/secret/acme/repos_xray_123.word-search"));

    assert_eq!(rendered, "repos_xray_123.word-search");
}

#[test]
fn duration_ms_format_uses_one_decimal_place() {
    let rendered = crate::index::format_duration_ms(std::time::Duration::from_micros(1234));

    assert_eq!(rendered, "1.2");
}

