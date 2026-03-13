use super::*;

/// Helper: build a minimal GitHistoryCache with test data and save it to a temp directory.
fn create_test_git_history_cache(dir: &std::path::Path) -> std::path::PathBuf {
    use crate::git::cache::{GitHistoryCacheBuilder, parse_git_log_stream};

    // Use the public streaming parser to build a cache
    let git_log = "\
COMMIT:aabbccddee00112233445566778899aabbccddee␞1700000000␞alice@example.com␞Alice␞Initial commit
src/main.rs

COMMIT:112233445566778899aabbccddeeff0011223344␞1700001000␞bob@example.com␞Bob␞Add feature
src/main.rs
src/lib.rs
";
    let mut builder = GitHistoryCacheBuilder::new();
    let reader = std::io::BufReader::new(git_log.as_bytes());
    parse_git_log_stream(reader, &mut builder).unwrap();

    let cache = builder.build(
        "aabbccddee00112233445566778899aabbccddee".to_string(),
        "main".to_string(),
    );

    let cache_path = dir.join("test_12345678.git-history");
    cache.save_to_disk(&cache_path).unwrap();
    cache_path
}

#[test]
fn test_info_json_includes_git_history() {
    let tmp = std::env::temp_dir().join(format!("xray_info_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let _cache_path = create_test_git_history_cache(&tmp);

    let result = cmd_info_json_for_dir(&tmp);

    let indexes = result["indexes"].as_array().expect("indexes should be an array");
    let git_entries: Vec<_> = indexes.iter()
        .filter(|idx| idx["type"] == "git-history")
        .collect();

    assert_eq!(git_entries.len(), 1, "Expected exactly 1 git-history entry");

    let entry = &git_entries[0];
    assert_eq!(entry["type"], "git-history");
    assert_eq!(entry["commits"], 2);
    assert_eq!(entry["files"], 2); // src/main.rs and src/lib.rs
    assert_eq!(entry["authors"], 2); // Alice and Bob
    assert_eq!(entry["branch"], "main");
    assert_eq!(entry["headHash"], "aabbccddee00112233445566778899aabbccddee");
    assert!(entry["sizeMb"].as_f64().unwrap() >= 0.0);
    assert!(entry["ageHours"].as_f64().unwrap() >= 0.0);
    assert!(entry["filename"].as_str().unwrap().ends_with(".git-history"));

    // Cleanup
    std::fs::remove_dir_all(&tmp).unwrap_or_default();
}

#[test]
fn test_info_json_empty_dir_no_git_history() {
    let tmp = std::env::temp_dir().join(format!("xray_info_empty_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let result = cmd_info_json_for_dir(&tmp);
    let indexes = result["indexes"].as_array().expect("indexes should be an array");
    let git_entries: Vec<_> = indexes.iter()
        .filter(|idx| idx["type"] == "git-history")
        .collect();
    assert_eq!(git_entries.len(), 0, "Empty dir should have no git-history entries");

    // Cleanup
    std::fs::remove_dir_all(&tmp).unwrap_or_default();
}

#[test]
fn test_info_json_nonexistent_dir() {
    let nonexistent = std::path::Path::new("/nonexistent_xray_info_test_dir_12345");
    let result = cmd_info_json_for_dir(nonexistent);
    let indexes = result["indexes"].as_array().expect("indexes should be an array");
    assert!(indexes.is_empty());
}

#[test]
fn test_info_json_git_history_corrupt_file_skipped() {
    let tmp = std::env::temp_dir().join(format!("xray_info_corrupt_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    // Write a corrupt .git-history file
    let corrupt_path = tmp.join("corrupt_12345678.git-history");
    std::fs::write(&corrupt_path, b"THIS_IS_NOT_A_VALID_GIT_CACHE").unwrap();

    let result = cmd_info_json_for_dir(&tmp);
    let indexes = result["indexes"].as_array().expect("indexes should be an array");
    let git_entries: Vec<_> = indexes.iter()
        .filter(|idx| idx["type"] == "git-history")
        .collect();
    assert_eq!(git_entries.len(), 0, "Corrupt git-history file should be skipped");

    // Cleanup
    std::fs::remove_dir_all(&tmp).unwrap_or_default();
}

#[test]
fn test_meta_sidecar_roundtrip() {
    let tmp = std::env::temp_dir().join(format!("search_meta_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    // Create a fake index file and write a .meta sidecar
    let index_path = tmp.join("test_12345678.word-search");
    std::fs::write(&index_path, b"fake index data").unwrap();

    let meta = crate::index::IndexMeta {
        root: "C:/Repos/TestProject".to_string(),
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        max_age_secs: 86400,
        files: 42,
        extensions: vec!["cs".to_string(), "xml".to_string()],
        details: crate::index::IndexDetails::Content {
            unique_tokens: 1000,
            total_tokens: 50000,
            parse_errors: None,
            lossy_file_count: None,
        },
    };

    crate::index::save_index_meta(&index_path, &meta);

    // Verify .meta file exists
    let meta_path = index_path.with_extension("word-search.meta");
    assert!(meta_path.exists(), "Meta file should exist at {}", meta_path.display());

    // Verify it loads back
    let loaded = crate::index::load_index_meta(&index_path);
    assert!(loaded.is_some(), "Should be able to load meta from sidecar");
    let loaded = loaded.unwrap();
    assert_eq!(loaded.root, "C:/Repos/TestProject");
    assert_eq!(loaded.files, 42);
    assert!(matches!(loaded.details, crate::index::IndexDetails::Content { unique_tokens: 1000, total_tokens: 50000, .. }));
    assert_eq!(loaded.extensions, vec!["cs", "xml"]);

    // Verify cmd_info_json_for_dir reads from .meta
    let result = cmd_info_json_for_dir(&tmp);
    let indexes = result["indexes"].as_array().expect("indexes should be an array");
    // Should find the content index via .meta (even though the .word-search file is fake)
    let content_entries: Vec<_> = indexes.iter()
        .filter(|idx| idx["type"] == "content")
        .collect();
    assert_eq!(content_entries.len(), 1, "Should find 1 content index via .meta");
    assert_eq!(content_entries[0]["root"], "C:/Repos/TestProject");
    assert_eq!(content_entries[0]["files"], 42);
    assert_eq!(content_entries[0]["totalTokens"], 50000);

    // Cleanup
    std::fs::remove_dir_all(&tmp).unwrap_or_default();
}

#[test]
fn test_meta_to_json_all_types() {
    use crate::index::IndexDetails;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Content
    let meta = IndexMeta {
        root: "C:/test".to_string(),
        created_at: now,
        max_age_secs: 3600,
        files: 100,
        extensions: vec!["cs".to_string()],
        details: IndexDetails::Content {
            unique_tokens: 5000,
            total_tokens: 100000,
            parse_errors: None,
            lossy_file_count: None,
        },
    };
    let json = meta_to_json(&meta, 1_048_576, "test.word-search");
    assert_eq!(json["type"], "content");
    assert_eq!(json["files"], 100);
    assert_eq!(json["totalTokens"], 100000);
    assert_eq!(json["sizeMb"], 1.0);

    // Definition
    let meta = IndexMeta {
        root: "C:/test".to_string(),
        created_at: now,
        max_age_secs: 0,
        files: 50,
        extensions: vec!["cs".to_string()],
        details: IndexDetails::Definition {
            definitions: 1000,
            call_sites: 5000,
            parse_errors: Some(3),
            lossy_file_count: None,
        },
    };
    let json = meta_to_json(&meta, 2_097_152, "test.code-structure");
    assert_eq!(json["type"], "definition");
    assert_eq!(json["definitions"], 1000);
    assert_eq!(json["callSites"], 5000);
    assert_eq!(json["readErrors"], 3);

    // File list
    let meta = IndexMeta {
        root: "C:/test".to_string(),
        created_at: now,
        max_age_secs: 3600,
        files: 0,
        extensions: Vec::new(),
        details: IndexDetails::FileList {
            entries: 500,
        },
    };
    let json = meta_to_json(&meta, 262144, "test.file-list");
    assert_eq!(json["type"], "file");
    assert_eq!(json["entries"], 500);

    // Git history
    let meta = IndexMeta {
        root: String::new(),
        created_at: now,
        max_age_secs: 0,
        files: 1000,
        extensions: Vec::new(),
        details: IndexDetails::GitHistory {
            commits: 5000,
            authors: 20,
            branch: "main".to_string(),
            head_hash: "abc123def456".to_string(),
        },
    };
    let json = meta_to_json(&meta, 524288, "test.git-history");
    assert_eq!(json["type"], "git-history");
    assert_eq!(json["commits"], 5000);
    assert_eq!(json["authors"], 20);
    assert_eq!(json["branch"], "main");
}

#[test]
fn test_meta_serde_roundtrip_all_variants() {
    use crate::index::IndexDetails;

    // Content round-trip
    let meta = IndexMeta {
        root: "C:/test".to_string(),
        created_at: 1000,
        max_age_secs: 3600,
        files: 42,
        extensions: vec!["cs".to_string()],
        details: IndexDetails::Content {
            unique_tokens: 500,
            total_tokens: 10000,
            parse_errors: Some(2),
            lossy_file_count: None,
        },
    };
    let json = serde_json::to_string(&meta).unwrap();
    let loaded: IndexMeta = serde_json::from_str(&json).unwrap();
    assert!(matches!(loaded.details, IndexDetails::Content { unique_tokens: 500, total_tokens: 10000, .. }));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["type"], "content");

    // Definition round-trip
    let meta = IndexMeta {
        root: "C:/test".to_string(),
        created_at: 2000,
        max_age_secs: 0,
        files: 50,
        extensions: vec!["cs".to_string(), "ts".to_string()],
        details: IndexDetails::Definition {
            definitions: 100,
            call_sites: 500,
            parse_errors: Some(3),
            lossy_file_count: Some(1),
        },
    };
    let json = serde_json::to_string(&meta).unwrap();
    let loaded: IndexMeta = serde_json::from_str(&json).unwrap();
    assert!(matches!(loaded.details, IndexDetails::Definition { definitions: 100, call_sites: 500, .. }));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["type"], "definition");

    // FileList round-trip
    let meta = IndexMeta {
        root: "C:/test".to_string(),
        created_at: 3000,
        max_age_secs: 7200,
        files: 0,
        extensions: Vec::new(),
        details: IndexDetails::FileList { entries: 1000 },
    };
    let json = serde_json::to_string(&meta).unwrap();
    let loaded: IndexMeta = serde_json::from_str(&json).unwrap();
    assert!(matches!(loaded.details, IndexDetails::FileList { entries: 1000 }));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["type"], "file-list");

    // GitHistory round-trip
    let meta = IndexMeta {
        root: String::new(),
        created_at: 4000,
        max_age_secs: 0,
        files: 200,
        extensions: Vec::new(),
        details: IndexDetails::GitHistory {
            commits: 500,
            authors: 10,
            branch: "main".to_string(),
            head_hash: "abc123".to_string(),
        },
    };
    let json = serde_json::to_string(&meta).unwrap();
    let loaded: IndexMeta = serde_json::from_str(&json).unwrap();
    assert!(matches!(loaded.details, IndexDetails::GitHistory { commits: 500, authors: 10, .. }));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["type"], "git-history");
}
