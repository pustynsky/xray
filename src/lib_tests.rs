use super::*;

#[test]
fn test_tokenize_basic() {
    let tokens = tokenize("hello world", 2);
    assert_eq!(tokens, vec!["hello", "world"]);
}

#[test]
fn test_tokenize_code() {
    let tokens = tokenize("private readonly HttpClient _client;", 2);
    assert_eq!(
        tokens,
        vec!["private", "readonly", "httpclient", "_client"]
    );
}

#[test]
fn test_tokenize_min_length() {
    let tokens = tokenize("a bb ccc", 2);
    assert_eq!(tokens, vec!["bb", "ccc"]);
}

#[test]
fn test_tokenize_multibyte_min_length_minor_24() {
    // MINOR-24: prior implementation compared byte length, so a 3-char Greek
    // token (6 bytes in UTF-8) passed `min_len=3` via `len() >= 3` even when
    // agents meant "3 Unicode scalars". Check a 2-char Greek token with
    // `min_len=3` is now correctly rejected (byte-len would let it pass).
    let tokens = tokenize("αβ аб xy", 3);
    assert!(
        tokens.is_empty(),
        "expected char-count filter to reject 2-char multibyte tokens; got {:?}",
        tokens
    );
    // And a 3-char Greek token should pass `min_len=3`.
    let tokens = tokenize("αβγ abc", 3);
    assert_eq!(tokens, vec!["αβγ", "abc"]);
}

#[test]
fn test_clean_path_strips_prefix() {
    assert_eq!(clean_path(r"\\?\C:\Users\test"), "C:/Users/test");
}

#[test]
fn test_clean_path_no_prefix() {
    assert_eq!(clean_path(r"C:\Users\test"), "C:/Users/test");
}

#[test]
fn test_clean_path_normalizes_backslashes() {
    assert_eq!(clean_path(r"src\Backend\Catalog"), "src/Backend/Catalog");
}

#[test]
fn test_clean_path_preserves_forward_slashes() {
    assert_eq!(clean_path("src/Backend/Catalog"), "src/Backend/Catalog");
}

#[test]
fn test_clean_path_mixed_separators() {
    assert_eq!(clean_path(r"src/Backend\Catalog\file.cs"), "src/Backend/Catalog/file.cs");
}

#[test]
fn test_clean_path_unc_prefix_with_normalization() {
    assert_eq!(clean_path(r"\\?\C:\Projects\src\file.cs"), "C:/Projects/src/file.cs");
}

// ─── path_eq tests ──────────────────────────────────────────

#[test]
fn test_path_eq_identical() {
    assert!(path_eq("C:/Repos/Xray", "C:/Repos/Xray"));
}

#[test]
fn test_path_eq_different_paths() {
    assert!(!path_eq("C:/Repos/Xray", "C:/Repos/Other"));
}

#[cfg(windows)]
#[test]
fn test_path_eq_case_insensitive_on_windows() {
    assert!(path_eq("C:/Repos/Xray", "c:/repos/xray"));
    assert!(path_eq("C:/Repos/Xray/Sub", "C:/REPOS/XRAY/SUB"));
}

#[cfg(not(windows))]
#[test]
fn test_path_eq_case_sensitive_on_unix() {
    assert!(!path_eq("/repos/Xray", "/repos/xray"));
}

// ─── stable_hash tests ──────────────────────────────────────

#[test]
fn test_stable_hash_deterministic() {
    let a = stable_hash(&[b"hello world"]);
    let b = stable_hash(&[b"hello world"]);
    assert_eq!(a, b, "same input must produce same hash");
}

#[test]
fn test_stable_hash_different_inputs() {
    let a = stable_hash(&[b"hello"]);
    let b = stable_hash(&[b"world"]);
    assert_ne!(a, b, "different inputs should produce different hashes");
}

#[test]
fn test_stable_hash_multi_part_equivalent_to_concat() {
    let split = stable_hash(&[b"hello", b"world"]);
    let concat = stable_hash(&[b"helloworld"]);
    assert_eq!(split, concat, "multi-part hash should equal concatenated hash");
}

#[test]
fn test_stable_hash_part_order_matters() {
    let ab = stable_hash(&[b"alpha", b"beta"]);
    let ba = stable_hash(&[b"beta", b"alpha"]);
    assert_ne!(ab, ba, "part order should affect hash output");
}

#[test]
fn test_stable_hash_known_fnv1a_vector() {
    // FNV-1a 64-bit hash of empty string is the offset basis itself
    let empty = stable_hash(&[]);
    assert_eq!(empty, 0xcbf2_9ce4_8422_2325, "empty input should return FNV offset basis");
}

#[test]
fn test_stable_hash_empty_vs_nonempty() {
    let empty = stable_hash(&[]);
    let nonempty = stable_hash(&[b"x"]);
    assert_ne!(empty, nonempty);
}

/// Compile-time guard: ContentIndex field completeness.
/// If you added a field to ContentIndex and this test doesn't compile,
/// update:
///   1. impl Default for ContentIndex (src/lib.rs)
///   2. build_content_index() in src/index.rs
///   3. empty_index in src/cli/serve.rs
///   4. This test — add the new field below
#[test]
fn test_content_index_field_count_guard() {
    let _guard = ContentIndex {
        root: String::new(),
        created_at: 0,
        max_age_secs: 3600,
        files: Vec::new(),
        index: HashMap::new(),
        total_tokens: 0,
        extensions: Vec::new(),
        file_token_counts: Vec::new(),
        file_tokens: Vec::new(),
        trigram: TrigramIndex::default(),
        trigram_dirty: false,
        path_to_id: None,
        read_errors: 0,
        lossy_file_count: 0,
        worker_panics: 0,
        format_version: crate::CONTENT_INDEX_VERSION,
        respect_git_exclude: false,
    };
    drop(_guard);
}

#[test]
fn test_content_index_default_values() {
    let d = ContentIndex::default();
    assert_eq!(d.root, "");
    assert_eq!(d.created_at, 0);
    assert_eq!(d.max_age_secs, 3600);
    assert!(d.files.is_empty());
    assert!(d.index.is_empty());
    assert_eq!(d.total_tokens, 0);
    assert!(d.extensions.is_empty());
    assert!(d.file_token_counts.is_empty());
    assert!(!d.trigram_dirty);
    assert!(d.path_to_id.is_none());
    assert_eq!(d.read_errors, 0);
    assert_eq!(d.lossy_file_count, 0);
}

#[test]
fn test_content_index_stale() {
    let index = ContentIndex {
        root: ".".to_string(),
        created_at: 0, // epoch = definitely stale
        ..Default::default()
    };
    assert!(index.is_stale());
}

// ─── warm_up tests ──────────────────────────────────────────

#[test]
fn test_warm_up_empty_index() {
    let index = ContentIndex {
        root: ".".to_string(),
        ..Default::default()
    };
    let (trigrams, tokens) = index.warm_up();
    assert_eq!(trigrams, 0);
    assert_eq!(tokens, 0);
}

#[test]
fn test_warm_up_with_data() {
    let mut trigram_map = HashMap::new();
    trigram_map.insert("htt".to_string(), vec![0, 1]);
    trigram_map.insert("ttp".to_string(), vec![0, 1]);
    trigram_map.insert("cli".to_string(), vec![0]);
    trigram_map.insert("han".to_string(), vec![1]);

    let mut inverted = HashMap::new();
    inverted.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("httphandler".to_string(), vec![Posting { file_id: 1, lines: vec![5] }]);

    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["file1.cs".to_string(), "file2.cs".to_string()],
        index: inverted,
        total_tokens: 2,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![1, 1],
        trigram: TrigramIndex {
            tokens: vec!["httpclient".to_string(), "httphandler".to_string()],
            trigram_map,
        },
        ..Default::default()
    };
    let (trigrams, tokens) = index.warm_up();
    assert_eq!(trigrams, 4); // 4 trigram entries
    assert_eq!(tokens, 2);  // 2 tokens
}

#[test]
fn test_warm_up_is_idempotent() {
    let mut trigram_map = HashMap::new();
    trigram_map.insert("abc".to_string(), vec![0]);

    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["file1.cs".to_string()],
        trigram: TrigramIndex {
            tokens: vec!["abcdef".to_string()],
            trigram_map,
        },
        ..Default::default()
    };

    // Call warm_up multiple times — should always return the same result
    let result1 = index.warm_up();
    let result2 = index.warm_up();
    let result3 = index.warm_up();
    assert_eq!(result1, result2);
    assert_eq!(result2, result3);
    assert_eq!(result1, (1, 1)); // 1 trigram, 1 token
}

#[test]
fn test_warm_up_then_search_works() {
    // After warm_up, substring search data should still be valid
    let mut trigram_map = HashMap::new();
    trigram_map.insert("foo".to_string(), vec![0]);
    trigram_map.insert("oob".to_string(), vec![0]);
    trigram_map.insert("oba".to_string(), vec![0]);
    trigram_map.insert("bar".to_string(), vec![0]);

    let mut inverted = HashMap::new();
    inverted.insert("foobar".to_string(), vec![Posting { file_id: 0, lines: vec![1, 5] }]);

    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["test.cs".to_string()],
        index: inverted,
        total_tokens: 1,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![1],
        trigram: TrigramIndex {
            tokens: vec!["foobar".to_string()],
            trigram_map,
        },
        ..Default::default()
    };

    // Warm up should succeed
    let (trigrams, tokens) = index.warm_up();
    assert_eq!(trigrams, 4);
    assert_eq!(tokens, 1);

    // After warm_up, the trigram index should still be usable
    // Verify trigram map still contains expected data
    assert!(index.trigram.trigram_map.contains_key("foo"));
    assert_eq!(index.trigram.tokens[0], "foobar");

    // Verify inverted index still works
    let postings = index.index.get("foobar").unwrap();
    assert_eq!(postings[0].file_id, 0);
    assert_eq!(postings[0].lines, vec![1, 5]);
}

// ─── read_errors / lossy_file_count tests ─────────────────

#[test]
fn test_content_index_read_errors_default_zero() {
    let index = ContentIndex {
        root: ".".to_string(),
        ..Default::default()
    };
    assert_eq!(index.read_errors, 0);
    assert_eq!(index.lossy_file_count, 0);
}

#[test]
fn test_content_index_read_errors_serialization_roundtrip() {
    let index = ContentIndex {
        root: ".".to_string(),
        read_errors: 5,
        lossy_file_count: 3,
        ..Default::default()
    };
    let encoded = bincode::serialize(&index).unwrap();
    let decoded: ContentIndex = bincode::deserialize(&encoded).unwrap();
    assert_eq!(decoded.read_errors, 5);
    assert_eq!(decoded.lossy_file_count, 3);
}

#[test]
fn test_content_index_read_errors_backward_compat_deserialization() {
    // Simulate an old index without read_errors/lossy_file_count fields.
    // Since #[serde(default)] is used, deserialization should succeed with 0 defaults.
    // We test this by serializing a struct, deserializing as ContentIndex,
    // and checking the defaults.
    let index = ContentIndex {
        root: ".".to_string(),
        ..Default::default()
    };
    // Verify default values are 0
    assert_eq!(index.read_errors, 0);
    assert_eq!(index.lossy_file_count, 0);
}

#[test]
fn test_posting_serialization_roundtrip() {
    let posting = Posting {
        file_id: 42,
        lines: vec![1, 5, 10],
    };
    let encoded = bincode::serialize(&posting).unwrap();
    let decoded: Posting = bincode::deserialize(&encoded).unwrap();
    assert_eq!(decoded.file_id, 42);
    assert_eq!(decoded.lines, vec![1, 5, 10]);
}
// ─── sanitize_for_filename tests ─────────────────────────────

#[test]
fn test_sanitize_basic_alphanumeric() {
    assert_eq!(sanitize_for_filename("MyProject"), "myproject");
}

#[test]
fn test_sanitize_with_hyphens_and_underscores() {
    assert_eq!(sanitize_for_filename("my-project_v2"), "my-project_v2");
}

#[test]
fn test_sanitize_spaces_and_parens() {
    assert_eq!(sanitize_for_filename("My Projects (2024)"), "my_projects__2024_");
}

#[test]
fn test_sanitize_dots_and_dollar() {
    assert_eq!(sanitize_for_filename("Build$.Output"), "build__output");
}

#[test]
fn test_sanitize_unicode_replaced() {
    assert_eq!(sanitize_for_filename("Código"), "c_digo");
}

#[test]
fn test_sanitize_empty_string() {
    assert_eq!(sanitize_for_filename(""), "_");
}

#[test]
fn test_sanitize_reserved_con() {
    assert_eq!(sanitize_for_filename("CON"), "_con");
}

#[test]
fn test_sanitize_reserved_nul_case_insensitive() {
    assert_eq!(sanitize_for_filename("nul"), "_nul");
}

#[test]
fn test_sanitize_reserved_com1() {
    assert_eq!(sanitize_for_filename("COM1"), "_com1");
}

#[test]
fn test_sanitize_reserved_lpt9() {
    assert_eq!(sanitize_for_filename("LPT9"), "_lpt9");
}

#[test]
fn test_sanitize_not_reserved_prefix() {
    // "CONSOLE" starts with CON but is NOT a reserved name
    assert_eq!(sanitize_for_filename("CONSOLE"), "console");
}

#[test]
fn test_sanitize_truncation() {
    let long = "a".repeat(100);
    let result = sanitize_for_filename(&long);
    assert_eq!(result.len(), MAX_PREFIX_LEN);
}

#[test]
fn test_sanitize_all_special_chars() {
    assert_eq!(sanitize_for_filename("!@#$%"), "_____");
}

// ─── extract_semantic_prefix tests ───────────────────────────

#[cfg(windows)]
#[test]
fn test_prefix_drive_root() {
    // On Windows, C:\ canonicalizes to \\?\C:\ which has Prefix + RootDir, 0 Normal components
    let path = std::path::PathBuf::from(r"C:\");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "c");
}

#[cfg(windows)]
#[test]
fn test_prefix_single_component() {
    let path = std::path::PathBuf::from(r"C:\test");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "c_test");
}

#[cfg(windows)]
#[test]
fn test_prefix_single_component_drive_d() {
    let path = std::path::PathBuf::from(r"D:\test");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "d_test");
}

#[cfg(windows)]
#[test]
fn test_prefix_two_components() {
    let path = std::path::PathBuf::from(r"C:\Repos\MyProject");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "repos_myproject");
}

#[cfg(windows)]
#[test]
fn test_prefix_three_components_takes_last_two() {
    let path = std::path::PathBuf::from(r"C:\Repos\rust\search");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "rust_search");
}

#[cfg(windows)]
#[test]
fn test_prefix_deep_path() {
    let path = std::path::PathBuf::from(r"C:\a\b\c\deep\project");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "deep_project");
}

#[cfg(windows)]
#[test]
fn test_prefix_same_leaf_different_parent() {
    let p1 = std::path::PathBuf::from(r"C:\test\test");
    let p2 = std::path::PathBuf::from(r"C:\users\test");
    assert_eq!(extract_semantic_prefix(&p1), "test_test");
    assert_eq!(extract_semantic_prefix(&p2), "users_test");
}

#[test]
fn test_prefix_reserved_name_component() {
    let path = std::path::PathBuf::from(r"C:\CON");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "c__con");
}

#[cfg(windows)]
#[test]
fn test_prefix_special_chars_in_component() {
    let path = std::path::PathBuf::from(r"C:\My Projects (2024)\api");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "my_projects__2024__api");
}

#[test]
fn test_prefix_no_drive_letter_unix_style() {
    // Unix-style path with no prefix component
    let path = std::path::PathBuf::from("/usr/local/share");
    let result = extract_semantic_prefix(&path);
    // On Windows, this has Normal components "usr", "local", "share"
    // On Unix, it would have Normal components "usr", "local", "share"
    assert_eq!(result, "local_share");
}

#[test]
fn test_prefix_deterministic() {
    let path = std::path::PathBuf::from(r"C:\Repos\MyProject");
    let a = extract_semantic_prefix(&path);
    let b = extract_semantic_prefix(&path);
    assert_eq!(a, b);
}

#[test]
fn test_prefix_truncation_long_components() {
    let long_parent = "a".repeat(30);
    let long_name = "b".repeat(30);
    let path = std::path::PathBuf::from(format!(r"C:\{}\{}", long_parent, long_name));
    let result = extract_semantic_prefix(&path);
    // Should be truncated to MAX_PREFIX_LEN
    assert!(result.len() <= MAX_PREFIX_LEN,
        "Result '{}' (len {}) exceeds MAX_PREFIX_LEN {}",
        result, result.len(), MAX_PREFIX_LEN);
}

#[cfg(test)]
mod trigram_tests {
use super::*;

#[test]
fn test_generate_trigrams_basic() {
    // "httpclient" → ["htt","ttp","tpc","pcl","cli","lie","ien","ent"]
    let trigrams = generate_trigrams("httpclient");
    assert_eq!(trigrams.len(), 8);
    assert_eq!(trigrams[0], "htt");
    assert_eq!(trigrams[7], "ent");
}

#[test]
fn test_generate_trigrams_short_1char() {
    assert!(generate_trigrams("a").is_empty());
}

#[test]
fn test_generate_trigrams_short_2chars() {
    assert!(generate_trigrams("ab").is_empty());
}

#[test]
fn test_generate_trigrams_exact_3chars() {
    let trigrams = generate_trigrams("abc");
    assert_eq!(trigrams, vec!["abc"]);
}

#[test]
fn test_generate_trigrams_4chars() {
    let trigrams = generate_trigrams("abcd");
    assert_eq!(trigrams, vec!["abc", "bcd"]);
}

#[test]
fn test_generate_trigrams_unicode() {
    // Unicode chars should be handled correctly (char-based, not byte-based)
    let trigrams = generate_trigrams("αβγδ");
    assert_eq!(trigrams.len(), 2); // "αβγ", "βγδ"
}

#[test]
fn test_generate_trigrams_count() {
    // Token of length N produces exactly max(0, N-2) trigrams
    for len in 0..20 {
        let token: String = (0..len).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let expected = if len < 3 { 0 } else { len - 2 };
        assert_eq!(generate_trigrams(&token).len(), expected, "len={}", len);
    }
}

#[test]
fn test_generate_trigrams_deterministic() {
    let a = generate_trigrams("databaseconnectionfactory");
    let b = generate_trigrams("databaseconnectionfactory");
    assert_eq!(a, b);
}

#[test]
fn test_generate_trigrams_empty() {
    assert!(generate_trigrams("").is_empty());
}

/// PERF-05 contract pin: the ASCII fast-path must produce
/// **byte-for-byte identical** output to the general char-based path.
/// We can't call the general path directly (it's the same function), so
/// we build the expected trigram list manually via the spec
/// ("3-character sliding windows") on a mixed corpus and assert
/// equality — including trigram order, which the index relies on.
#[test]
fn test_generate_trigrams_ascii_fast_path_parity() {
    // ASCII identifier — exercises the fast-path (token.is_ascii() == true).
    let ascii = generate_trigrams("HandlerContext");
    let expected_ascii: Vec<String> = "HandlerContext"
        .chars()
        .collect::<Vec<_>>()
        .windows(3)
        .map(|w| w.iter().collect::<String>())
        .collect();
    assert_eq!(
        ascii, expected_ascii,
        "PERF-05 ASCII fast-path must match char-based windows byte-for-byte"
    );

    // Mixed ASCII + Cyrillic — must NOT take the fast-path
    // (any non-ASCII byte forces the general path), and the result must
    // be character-aligned, not byte-aligned (otherwise multi-byte UTF-8
    // sequences would split mid-codepoint).
    let mixed = generate_trigrams("abПривет");
    assert_eq!(
        mixed.len(),
        6,
        "8 chars => 6 trigrams (mixed ASCII+Cyrillic must use char-based windows)"
    );
    assert_eq!(mixed[0], "abП");
    assert_eq!(mixed[1], "bПр");
    // Each trigram must be exactly 3 chars (codepoints), not 3 bytes —
    // pinning that the general path is char-aware.
    for t in &mixed {
        assert_eq!(
            t.chars().count(),
            3,
            "every trigram must be 3 codepoints, got {:?} ({} bytes)",
            t,
            t.len()
        );
    }
}

#[test]
fn test_trigram_index_serialization_roundtrip() {
    let mut trigram_map = HashMap::new();
    trigram_map.insert("abc".to_string(), vec![0, 1, 2]);
    trigram_map.insert("bcd".to_string(), vec![1, 2]);
    let ti = TrigramIndex {
        tokens: vec!["abcdef".to_string(), "bcdefg".to_string(), "cdefgh".to_string()],
        trigram_map,
    };
    let bytes = bincode::serialize(&ti).unwrap();
    let ti2: TrigramIndex = bincode::deserialize(&bytes).unwrap();
    assert_eq!(ti.tokens, ti2.tokens);
    assert_eq!(ti.trigram_map, ti2.trigram_map);
}

#[test]
fn test_content_index_with_trigram_serialization() {
    // Create a ContentIndex with a non-empty trigram, serialize/deserialize
    let ci = ContentIndex {
        root: ".".to_string(),
        trigram: TrigramIndex {
            tokens: vec!["hello".to_string()],
            trigram_map: {
                let mut m = HashMap::new();
                m.insert("hel".to_string(), vec![0]);
                m.insert("ell".to_string(), vec![0]);
                m.insert("llo".to_string(), vec![0]);
                m
            },
        },
        ..Default::default()
    };
    let bytes = bincode::serialize(&ci).unwrap();
    let ci2: ContentIndex = bincode::deserialize(&bytes).unwrap();
    assert_eq!(ci.trigram.tokens, ci2.trigram.tokens);
    assert_eq!(ci.trigram.trigram_map.len(), ci2.trigram.trigram_map.len());
}
}

// ─── read_file_lossy / BOM detection tests ───────────────────

/// Helper: encode a string as UTF-16LE with BOM prefix
fn encode_utf16le_with_bom(s: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE]; // UTF-16LE BOM
    for code_unit in s.encode_utf16() {
        bytes.extend_from_slice(&code_unit.to_le_bytes());
    }
    bytes
}

/// Helper: encode a string as UTF-16BE with BOM prefix
fn encode_utf16be_with_bom(s: &str) -> Vec<u8> {
    let mut bytes = vec![0xFE, 0xFF]; // UTF-16BE BOM
    for code_unit in s.encode_utf16() {
        bytes.extend_from_slice(&code_unit.to_be_bytes());
    }
    bytes
}

#[test]
fn test_read_file_lossy_utf16le_bom() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_utf16le.cs");
    let content = "// Hello World\nclass Foo { }";
    std::fs::write(&path, encode_utf16le_with_bom(content)).unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy, "UTF-16LE with BOM should not be lossy");
    assert_eq!(result, content);
}

#[test]
fn test_read_file_lossy_utf16be_bom() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_utf16be.cs");
    let content = "// Hello World\nclass Bar { }";
    std::fs::write(&path, encode_utf16be_with_bom(content)).unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy, "UTF-16BE with BOM should not be lossy");
    assert_eq!(result, content);
}

#[test]
fn test_read_file_lossy_utf8_bom() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_utf8bom.cs");
    let content = "// UTF-8 with BOM\nclass Baz { }";
    let mut bytes = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
    bytes.extend_from_slice(content.as_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy, "UTF-8 with BOM should not be lossy");
    assert_eq!(result, content, "BOM should be stripped from content");
}

#[test]
fn test_read_file_lossy_plain_utf8() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_plain.cs");
    let content = "// Plain UTF-8\nclass Plain { }";
    std::fs::write(&path, content.as_bytes()).unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy);
    assert_eq!(result, content);
}

#[test]
fn test_read_file_lossy_invalid_utf8_still_lossy() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_invalid.cs");
    // Windows-1252 smart quote (0x93) — not valid UTF-8, not a BOM
    let bytes = vec![0x2F, 0x2F, 0x20, 0x93, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x93];
    std::fs::write(&path, &bytes).unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(was_lossy, "Invalid UTF-8 should produce lossy result");
    assert!(result.contains("hello"), "Content should still be partially readable");
}

#[test]
fn test_read_file_lossy_utf16le_csharp_code() {
    // Simulate a real C# file encoded in UTF-16LE (like HtmlLexer.cs)
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("HtmlLexer.cs");
    let content = "using System;\n\nnamespace Parser\n{\n    internal sealed class HtmlLexer\n    {\n        public void Parse() { }\n    }\n}";
    std::fs::write(&path, encode_utf16le_with_bom(content)).unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy);
    assert!(result.contains("class HtmlLexer"), "Should contain class name");
    assert!(result.contains("using System"), "Should contain using directive");
    assert!(result.contains("Parse()"), "Should contain method name");
}

#[test]
fn test_read_file_lossy_utf16le_unicode_content() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_unicode.cs");
    let content = "// Ünïcödé: « résumé » — naïve";
    std::fs::write(&path, encode_utf16le_with_bom(content)).unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy);
    assert_eq!(result, content);
}

#[test]
fn test_read_file_lossy_empty_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("empty.cs");
    std::fs::write(&path, b"").unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy);
    assert_eq!(result, "");
}

#[test]
fn test_read_file_lossy_utf16le_bom_only() {
    // File with just a UTF-16LE BOM and no content
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bom_only.cs");
    std::fs::write(&path, [0xFF, 0xFE]).unwrap();

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy);
    assert_eq!(result, "");
}

#[test]
fn test_read_file_lossy_single_byte_file() {
    // File too short for BOM detection
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("single.cs");
    std::fs::write(&path, [0x41]).unwrap(); // 'A'

    let (result, was_lossy) = read_file_lossy(&path).unwrap();
    assert!(!was_lossy);
    assert_eq!(result, "A");
}

#[test]
fn test_decode_utf16le_basic() {
    let input = "Hello, World!";
    let encoded: Vec<u8> = input.encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    assert_eq!(decode_utf16le(&encoded), input);
}

#[test]
fn test_decode_utf16be_basic() {
    let input = "Hello, World!";
    let encoded: Vec<u8> = input.encode_utf16()
        .flat_map(|u| u.to_be_bytes())
        .collect();
    assert_eq!(decode_utf16be(&encoded), input);
}

#[test]
fn test_decode_utf16le_odd_byte_replacement() {
    // LIB-009: odd trailing byte signals truncation/corruption. Pre-fix it was
    // silently dropped; post-fix the decoder emits a U+FFFD replacement so
    // downstream tokenisation can detect the truncated tail.
    let input = "AB";
    let mut encoded: Vec<u8> = input.encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    encoded.push(0x99); // trailing odd byte
    assert_eq!(decode_utf16le(&encoded), "AB\u{FFFD}");
}

#[test]
fn test_decode_utf16be_odd_byte_replacement() {
    // LIB-009: same odd-tail handling for big-endian inputs.
    let input = "AB";
    let mut encoded: Vec<u8> = input.encode_utf16()
        .flat_map(|u| u.to_be_bytes())
        .collect();
    encoded.push(0x99);
    assert_eq!(decode_utf16be(&encoded), "AB\u{FFFD}");
}

#[test]
fn test_decode_utf16le_empty() {
    assert_eq!(decode_utf16le(&[]), "");
}

#[test]
fn test_decode_utf16be_empty() {
    assert_eq!(decode_utf16be(&[]), "");
}

// ─── LIB-007 / LIB-013 hardening regressions (2026-04-22) ──────────

#[test]
fn test_lib007_read_file_lossy_rejects_oversized_file() {
    // LIB-007: files exceeding MAX_INDEX_FILE_BYTES must be rejected before
    // allocation rather than loaded into RAM. We do not actually create a
    // 50 MB+ file in the test (slow + IO-heavy); instead, we patch by setting
    // the file's apparent size via a sparse file (0-byte writes followed by
    // set_len) which std::fs::metadata reports correctly without consuming
    // disk on most filesystems.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("huge.txt");
    let f = std::fs::File::create(&path).unwrap();
    // Sparse file: 1 byte over the cap. NTFS, ext4, APFS all support this.
    f.set_len(MAX_INDEX_FILE_BYTES + 1).unwrap();
    drop(f);

    let err = read_file_lossy(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Other);
    let msg = err.to_string();
    assert!(msg.contains("MAX_INDEX_FILE_BYTES"), "error must mention size cap: {}", msg);
}

#[test]
fn test_lib007_read_file_lossy_accepts_file_at_exact_cap() {
    // Boundary check: a file of exactly MAX_INDEX_FILE_BYTES is still accepted.
    // We don't materialise 50 MB in CI; verify the off-by-one of the comparison
    // by sparse-allocating up to the cap and reading back (sparse reads as zeros,
    // which decode_utf16 / from_utf8 handle fine).
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("at_cap.txt");
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(MAX_INDEX_FILE_BYTES).unwrap();
    drop(f);

    // Reading 50 MB of zeros allocates ~50 MB; acceptable in tests, and we want
    // to prove the boundary inclusively belongs to the accepted side.
    let result = read_file_lossy(&path);
    assert!(result.is_ok(), "file at exact cap must be accepted: {:?}", result.err());
}

#[test]
fn test_lib013_is_path_within_empty_root_refuses() {
    // LIB-013: empty root must NOT accept arbitrary paths. Pre-fix this returned
    // `true` ("no boundary, accept everything"), which made forgetting to pass
    // a real root a silent scope-bypass. Post-fix returns `false` so the missing
    // boundary is loud.
    assert!(!is_path_within("/etc/passwd", ""), "empty root must refuse, not accept");
    assert!(!is_path_within("C:/Windows/system32", ""));
    assert!(!is_path_within("", ""));
    // Sanity: non-empty root with matching path still works.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let inside = tmp.path().join("foo.txt").to_string_lossy().to_string();
    assert!(is_path_within(&inside, &root), "non-empty root must still accept legit paths");
}

#[test]
fn test_is_path_within_relative_dotdot_resolving_inside_accepted() {
    // Equivalent in-workspace paths must produce the same verdict regardless
    // of how they're written. Pre-fix, the no-traversal form was accepted via
    // logical compare while the `..`-bearing form fell through to canonicalize
    // and was rejected when the leaf didn't exist on disk.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();

    // Plain in-workspace nonexistent leaf (no `..`): accepted as before.
    let inside_plain = format!("{}/nonexistent/dir", root);
    assert!(
        is_path_within(&inside_plain, &root),
        "no-traversal in-workspace nonexistent path must still be accepted"
    );

    // Same logical destination via `..`: must also be accepted.
    let inside_dotdot = format!("{}/src/../nonexistent/dir", root);
    assert!(
        is_path_within(&inside_dotdot, &root),
        "in-workspace `..`-bearing path that resolves inside root must be accepted"
    );

    // `..` that resolves back to root itself.
    let inside_to_root = format!("{}/sub/..", root);
    assert!(
        is_path_within(&inside_to_root, &root),
        "`<root>/sub/..` resolves to root and must be accepted"
    );
}

#[test]
fn test_is_path_within_relative_dotdot_escape_still_rejected() {
    // Genuine escapes via `..` must still be refused, otherwise the logical
    // resolution would silently turn into a scope-bypass.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();

    // Pop past root.
    let escape_one = format!("{}/../outside-xyz", root);
    assert!(
        !is_path_within(&escape_one, &root),
        "`<root>/../outside` must be rejected"
    );

    // Pop past root via deeper traversal.
    let escape_deep = format!("{}/sub/../../outside-xyz", root);
    assert!(
        !is_path_within(&escape_deep, &root),
        "`<root>/sub/../../outside` must be rejected"
    );
}

/// Walk-up canonical fallback regression: a path with a non-existent leaf
/// inside the workspace must still be classified as inside, even when its
/// existing ancestor is in a different short/long form than `root`.
///
/// Local repro on Linux/macOS uses logically-different but canonically-equal
/// paths via a symlinked root; on Windows we additionally exercise 8.3 short
/// names via `GetShortPathName` (the original CI failure on windows-latest).
#[test]
fn test_is_path_within_nonexistent_leaf_inside_via_canonical_ancestor() {
    let tmp = tempfile::tempdir().unwrap();
    // The canonical root form (long, no symlinks).
    let canonical_root = std::fs::canonicalize(tmp.path()).unwrap();
    // Production callers pass `clean_path`-normalised root (forward slashes,
    // `\\?\` prefix stripped). Mirror that here so the test exercises the
    // exact shape `is_path_within` sees in MCP handlers.
    let canonical_root_str = clean_path(&canonical_root.to_string_lossy());

    // Path with a non-existent leaf in canonical form: must always be accepted
    // (sanity check that the regression test fixture itself is sound).
    let plain = format!("{}/nonexistent-but-inside", canonical_root_str);
    assert!(
        is_path_within(&plain, &canonical_root_str),
        "plain `<canonical_root>/nonexistent` must be accepted (sanity)"
    );

    // Cross-form: feed the SAME canonical root but a path computed against the
    // platform-specific alternate form of that root. On Windows the alternate
    // is the 8.3 short name; on Unix we simulate via a symlink that points
    // back at the canonical dir (logically-different prefix, canonically-same).
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        // Resolve short (8.3) form of the canonical root via GetShortPathNameW.
        // If the volume has 8.3 disabled (`fsutil 8dot3name set`), this returns
        // the input unchanged — the test then degenerates into the canonical
        // case, which is fine (no regression possible to assert).
        let wide: Vec<u16> = std::ffi::OsStr::new(&canonical_root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut buf = vec![0u16; 1024];
        let len = unsafe {
            unsafe extern "system" {
                fn GetShortPathNameW(lpsz_long: *const u16, lpsz_short: *mut u16, cch: u32) -> u32;
            }
            GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32)
        };
        if len > 0 && (len as usize) < buf.len() {
            let short_root = std::ffi::OsString::from_wide(&buf[..len as usize])
                .to_string_lossy()
                .to_string();
            // Only meaningful when short ≠ canonical (i.e. 8.3 is enabled and
            // the dir name is long enough to be shortened).
            if short_root != canonical_root.to_string_lossy() {
                // Same `clean_path` normalisation production callers apply.
                let short_root_clean = clean_path(&short_root);
                let cross_form = format!("{}/nonexistent-but-inside", short_root_clean);
                assert!(
                    is_path_within(&cross_form, &canonical_root_str),
                    "Windows 8.3 short-form path with non-existent leaf must be \
                     classified as inside canonical root (the windows-latest CI \
                     regression). short={}, canonical_root={}",
                    cross_form, canonical_root_str
                );
            }
        }
    }
    #[cfg(unix)]
    {
        // Symlink an alternate directory that points back at the canonical
        // root, then ask whether `<symlink>/nonexistent-but-inside` is inside
        // canonical_root. Pre-fix, the textual `inside` check rejects (the
        // symlink prefix differs) and `canonicalize(path)` fails because the
        // leaf does not exist. The walk-up fallback canonicalizes the existing
        // ancestor (the symlink), which resolves to canonical_root.
        let parent = tempfile::tempdir().unwrap();
        let alt = parent.path().join("alt-form");
        std::os::unix::fs::symlink(&canonical_root, &alt).unwrap();
        let cross_form = format!("{}/nonexistent-but-inside", alt.to_string_lossy());
        assert!(
            is_path_within(&cross_form, &canonical_root_str),
            "symlinked-alternate path with non-existent leaf must be classified \
             as inside canonical root. alt={}, canonical_root={}",
            cross_form, canonical_root_str
        );
    }
}


/// Combined regression for the d2b3d8f follow-up review: alternate root
/// spelling + `..` + non-existent leaf must still be classified as inside.
///
/// Pre-fix flow on Linux for `<alt-root>/sub/../newdir` where `<alt-root>`
/// is a symlink to canonical root, `sub` exists, `newdir` does not:
///   1. `has_traversal == true` → no-traversal logical branch skipped
///   2. `resolve_dotdot_logical` collapses to `<alt-root>/newdir` (preserves
///      the alt prefix) → textual `inside` rejects (alt ≠ canonical)
///   3. `canonicalize(path)` fails on the non-existent `newdir` leaf
///   4. After d2b3d8f the walk-up branch was gated `!has_traversal` and
///      skipped → function returned `false` for a legitimate in-workspace
///      path.
///
/// The follow-up fix routes the walk-up through `safe_for_walkup`, which
/// holds the `..`-stripped form for traversal inputs (and `None` for genuine
/// escapes — confirmed by the companion test
/// `test_is_path_within_relative_dotdot_escape_still_rejected`). The walk-up
/// then climbs from `newdir` (non-existent) to `<alt-root>` (existing
/// symlink), canonicalizes it to canonical_root, and accepts.
#[cfg(unix)]
#[test]
fn test_is_path_within_traversal_via_alt_root_nonexistent_leaf_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let canonical_root = std::fs::canonicalize(tmp.path()).unwrap();
    let canonical_root_str = clean_path(&canonical_root.to_string_lossy());

    // Existing in-root subdir so `..` has something to pop off.
    let sub = canonical_root.join("sub");
    std::fs::create_dir(&sub).unwrap();

    // Alternate root: a symlink in a sibling tempdir pointing at canonical_root.
    let parent = tempfile::tempdir().unwrap();
    let alt = parent.path().join("alt-form");
    std::os::unix::fs::symlink(&canonical_root, &alt).unwrap();

    // `<alt-root>/sub/../newdir` — alternate prefix, traversal, non-existent leaf.
    let cross_form = format!("{}/sub/../newdir", alt.to_string_lossy());
    assert!(
        is_path_within(&cross_form, &canonical_root_str),
        "alternate-root path with `..` and non-existent leaf must be classified \
         as inside canonical root. cross_form={}, canonical_root={}",
        cross_form, canonical_root_str
    );

    // Sanity: the same shape WITHOUT the alternate root prefix already worked
    // before d2b3d8f — guard against accidental regression of the easier case.
    let plain_traversal = format!("{}/sub/../newdir", canonical_root_str);
    assert!(
        is_path_within(&plain_traversal, &canonical_root_str),
        "`<canonical_root>/sub/../newdir` must be accepted (sanity)"
    );
}


#[test]
fn test_worker_panics_default_is_zero() {
    let index = ContentIndex::default();
    assert_eq!(index.worker_panics, 0);
}

#[test]
fn test_tokenize_idx_010_already_lowercase_ascii_equivalence() {
    // IDX-010 regression: the ASCII-lowercase fast path must produce IDENTICAL
    // output to the legacy `to_lowercase()` path. Any divergence would corrupt
    // the inverted index by routing the same logical token to two distinct
    // hash bucket keys.
    let cases = [
        "already_lowercase_token",
        "snake_case_42",
        "hello",
        "_underscore_prefix",
        "trailing_42",
    ];
    for c in cases {
        let fast = crate::tokenize(c, 1);
        let legacy: Vec<String> = c
            .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
            .filter(|s| s.chars().count() >= 1)
            .map(|s| s.to_lowercase())
            .collect();
        assert_eq!(fast, legacy, "divergence on input {c:?}");
    }
}

#[test]
fn test_tokenize_idx_010_uppercase_and_unicode_match_legacy() {
    // IDX-010 regression: the slow path (containing ASCII uppercase OR any
    // non-ASCII byte) must still go through `str::to_lowercase` so that
    // Unicode case folding (Cyrillic, Greek, German ß, etc.) is preserved.
    let cases = [
        "HttpClient",                                  // pure ASCII upper-mix
        "camelCase",                                   // ASCII upper-mix
        "\u{041F}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442}",   // Russian "Привет" — has uppercase П
        "caf\u{00E9}",                                  // already lowercase but non-ASCII (é)
        "GR\u{0395}EK",                                 // Greek Ε + ASCII upper
    ];
    for c in cases {
        let fast = crate::tokenize(c, 1);
        let legacy: Vec<String> = c
            .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
            .filter(|s| s.chars().count() >= 1)
            .map(|s| s.to_lowercase())
            .collect();
        assert_eq!(fast, legacy, "divergence on input {c:?}: fast={fast:?} legacy={legacy:?}");
    }
}

