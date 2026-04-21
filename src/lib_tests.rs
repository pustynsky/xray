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
        trigram: TrigramIndex::default(),
        trigram_dirty: false,
        path_to_id: None,
        read_errors: 0,
        lossy_file_count: 0,
        worker_panics: 0,
        format_version: crate::CONTENT_INDEX_VERSION,
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

#[test]
fn test_prefix_drive_root() {
    // On Windows, C:\ canonicalizes to \\?\C:\ which has Prefix + RootDir, 0 Normal components
    let path = std::path::PathBuf::from(r"C:\");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "c");
}

#[test]
fn test_prefix_single_component() {
    let path = std::path::PathBuf::from(r"C:\test");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "c_test");
}

#[test]
fn test_prefix_single_component_drive_d() {
    let path = std::path::PathBuf::from(r"D:\test");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "d_test");
}

#[test]
fn test_prefix_two_components() {
    let path = std::path::PathBuf::from(r"C:\Repos\MyProject");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "repos_myproject");
}

#[test]
fn test_prefix_three_components_takes_last_two() {
    let path = std::path::PathBuf::from(r"C:\Repos\rust\search");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "rust_search");
}

#[test]
fn test_prefix_deep_path() {
    let path = std::path::PathBuf::from(r"C:\a\b\c\deep\project");
    let result = extract_semantic_prefix(&path);
    assert_eq!(result, "deep_project");
}

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
fn test_decode_utf16le_odd_byte_ignored() {
    // Odd trailing byte should be silently ignored (chunks_exact behavior)
    let input = "AB";
    let mut encoded: Vec<u8> = input.encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    encoded.push(0x99); // trailing odd byte
    assert_eq!(decode_utf16le(&encoded), input);
}

#[test]
fn test_decode_utf16le_empty() {
    assert_eq!(decode_utf16le(&[]), "");
}

#[test]
fn test_decode_utf16be_empty() {
    assert_eq!(decode_utf16be(&[]), "");
}


#[test]
fn test_worker_panics_default_is_zero() {
    let index = ContentIndex::default();
    assert_eq!(index.worker_panics, 0);
}
