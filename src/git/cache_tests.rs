//! Unit tests for the git history cache module.
//!
//! All tests use mock data — no real git repository required.
//! Tests cover: parser, path normalization, queries, cache validity, struct size.

use super::cache::*;
use std::io::Cursor;

// ─── Test helpers ───────────────────────────────────────────────────

/// Build a mock cache from a git-log-style string.
fn parse_mock_log(input: &str) -> GitHistoryCache {
    let reader = Cursor::new(input.as_bytes());
    let mut builder = GitHistoryCache::builder();
    parse_git_log_stream(reader, &mut builder).expect("parse should succeed");
    GitHistoryCache::from_builder(
        builder,
        "abc123def456abc123def456abc123def456abc1".to_string(),
        "main".to_string(),
    )
}

/// A reusable multi-commit mock log.
/// 3 commits, 2 authors, touching multiple files.
fn multi_commit_log() -> &'static str {
    // Commit 1: hash aaa..., timestamp 1700000000, author Alice
    // Commit 2: hash bbb..., timestamp 1700001000, author Bob
    // Commit 3: hash ccc..., timestamp 1700002000, author Alice
    concat!(
        "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞alice@example.com␞Alice␞Initial commit\n",
        "src/main.rs\n",
        "Cargo.toml\n",
        "\n",
        "COMMIT:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb␞1700001000␞bob@example.com␞Bob␞Add feature X\n",
        "src/main.rs\n",
        "src/lib.rs\n",
        "\n",
        "COMMIT:cccccccccccccccccccccccccccccccccccccccc␞1700002000␞alice@example.com␞Alice␞Fix bug in main\n",
        "src/main.rs\n",
        "\n",
    )
}

// ─── Parser tests ───────────────────────────────────────────────────

#[test]
fn test_parser_multi_commit() {
    let cache = parse_mock_log(multi_commit_log());

    // Should have 3 commits
    assert_eq!(cache.commits.len(), 3, "Expected 3 commits");

    // Should have 2 unique authors
    assert_eq!(cache.authors.len(), 2, "Expected 2 unique authors");

    // src/main.rs should appear in all 3 commits
    let main_rs = cache.file_commits.get("src/main.rs");
    assert!(main_rs.is_some(), "src/main.rs should be in file_commits");
    assert_eq!(main_rs.unwrap().len(), 3, "src/main.rs should have 3 commit refs");

    // Cargo.toml should appear in 1 commit
    let cargo = cache.file_commits.get("Cargo.toml");
    assert!(cargo.is_some(), "Cargo.toml should be in file_commits");
    assert_eq!(cargo.unwrap().len(), 1, "Cargo.toml should have 1 commit ref");

    // src/lib.rs should appear in 1 commit
    let lib_rs = cache.file_commits.get("src/lib.rs");
    assert!(lib_rs.is_some(), "src/lib.rs should be in file_commits");
    assert_eq!(lib_rs.unwrap().len(), 1, "src/lib.rs should have 1 commit ref");

    // Total unique files: 3
    assert_eq!(cache.file_commits.len(), 3, "Expected 3 unique files");
}

#[test]
fn test_parser_commit_fields() {
    let cache = parse_mock_log(multi_commit_log());

    // Verify first commit fields
    let meta = &cache.commits[0];
    assert_eq!(meta.timestamp, 1700000000);
    assert_eq!(
        format_hex_hash(&meta.hash),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );

    // Verify author resolution
    let author = &cache.authors[meta.author_idx as usize];
    assert_eq!(author.name, "Alice");
    assert_eq!(author.email, "alice@example.com");

    // Verify subject from pool
    let subject_start = meta.subject_offset as usize;
    let subject_end = subject_start + meta.subject_len as usize;
    let subject = &cache.subjects[subject_start..subject_end];
    assert_eq!(subject, "Initial commit");
}

#[test]
fn test_parser_empty_input() {
    let cache = parse_mock_log("");
    assert_eq!(cache.commits.len(), 0);
    assert_eq!(cache.authors.len(), 0);
    assert_eq!(cache.file_commits.len(), 0);
    assert!(cache.subjects.is_empty());
}

#[test]
fn test_parser_empty_subject() {
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞alice@example.com␞Alice␞\n\
               src/main.rs\n\n";
    let cache = parse_mock_log(log);

    assert_eq!(cache.commits.len(), 1);
    let meta = &cache.commits[0];
    let subject_start = meta.subject_offset as usize;
    let subject_end = subject_start + meta.subject_len as usize;
    let subject = &cache.subjects[subject_start..subject_end];
    assert_eq!(subject, "", "Empty subject should be preserved");
}

#[test]
fn test_parser_subject_with_field_sep() {
    // Subject contains the ␞ separator — parser should rejoin via fields[4..].join()
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞a@b.com␞Name␞Part A␞Part B\n\
               file.rs\n\n";
    let cache = parse_mock_log(log);

    assert_eq!(cache.commits.len(), 1);
    let (info, _total) = cache.query_file_history("file.rs", None, None, None, None, None);
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].subject, "Part A␞Part B", "Subject with ␞ should be rejoined");
}

#[test]
fn test_parser_empty_file_list() {
    // A commit with no file paths (e.g., merge commit with no changes)
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞a@b.com␞Name␞Merge commit\n\n";
    let cache = parse_mock_log(log);

    assert_eq!(cache.commits.len(), 1);
    assert_eq!(cache.file_commits.len(), 0, "No files should be recorded");
}

#[test]
fn test_parser_merge_commit_many_files() {
    let mut log = String::from(
        "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞a@b.com␞Name␞Big merge\n",
    );
    for i in 0..100 {
        log.push_str(&format!("path/to/file_{}.rs\n", i));
    }
    log.push('\n');

    let cache = parse_mock_log(&log);

    assert_eq!(cache.commits.len(), 1);
    assert_eq!(cache.file_commits.len(), 100, "All 100 files should be recorded");
}

#[test]
fn test_parser_malformed_line_skipped() {
    // Malformed commit line (too few fields) should be skipped
    let log = "COMMIT:bad_line_no_separators\n\
               COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞a@b.com␞Name␞Good commit\n\
               file.rs\n\n";
    let cache = parse_mock_log(log);

    assert_eq!(cache.commits.len(), 1, "Only the good commit should be parsed");
}

#[test]
fn test_parser_bad_hash_skipped() {
    // Invalid hex hash should be skipped
    let log = "COMMIT:ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ␞1700000000␞a@b.com␞Name␞Bad hash\n\
               file.rs\n\n\
               COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞a@b.com␞Name␞Good\n\
               file2.rs\n\n";
    let cache = parse_mock_log(log);

    assert_eq!(cache.commits.len(), 1, "Only the good commit should be parsed");
    assert!(cache.file_commits.contains_key("file2.rs"));
    // file.rs was after the bad commit line, but current_commit_idx was None, so it should NOT be added
    assert!(!cache.file_commits.contains_key("file.rs"),
        "Files after bad commit should not be recorded");
}

// ─── Normalize path tests ───────────────────────────────────────────

#[test]
fn test_normalize_path_backslash() {
    assert_eq!(GitHistoryCache::normalize_path("src\\main.rs"), "src/main.rs");
}

#[test]
fn test_normalize_path_dot_slash() {
    assert_eq!(GitHistoryCache::normalize_path("./src/main.rs"), "src/main.rs");
}

#[test]
fn test_normalize_path_empty() {
    assert_eq!(GitHistoryCache::normalize_path(""), "");
}

#[test]
fn test_normalize_path_dot() {
    assert_eq!(GitHistoryCache::normalize_path("."), "");
}

#[test]
fn test_normalize_path_trailing_slash() {
    assert_eq!(GitHistoryCache::normalize_path("src/"), "src");
}

#[test]
fn test_normalize_path_double_slash() {
    assert_eq!(GitHistoryCache::normalize_path("src//main.rs"), "src/main.rs");
}

#[test]
fn test_normalize_path_whitespace() {
    assert_eq!(GitHistoryCache::normalize_path("  src/main.rs  "), "src/main.rs");
}

#[test]
fn test_normalize_path_mixed() {
    assert_eq!(
        GitHistoryCache::normalize_path(".\\src\\\\main.rs"),
        "src/main.rs"
    );
}

#[test]
fn test_normalize_path_multiple_dot_slash() {
    assert_eq!(GitHistoryCache::normalize_path("././src/main.rs"), "src/main.rs");
}

// ─── Query: file history ────────────────────────────────────────────

#[test]
fn test_query_file_history_basic() {
    let cache = parse_mock_log(multi_commit_log());

    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, None, None);
    assert_eq!(history.len(), 3, "src/main.rs should have 3 commits");

    // Should be sorted by timestamp descending (newest first)
    assert!(history[0].timestamp >= history[1].timestamp);
    assert!(history[1].timestamp >= history[2].timestamp);
}

#[test]
fn test_query_file_history_max_results() {
    let cache = parse_mock_log(multi_commit_log());

    let (history, _total) = cache.query_file_history("src/main.rs", Some(2), None, None, None, None);
    assert_eq!(history.len(), 2, "Should return at most 2 results");

    // Should be the 2 newest commits
    assert_eq!(history[0].timestamp, 1700002000);
    assert_eq!(history[1].timestamp, 1700001000);
}

#[test]
fn test_query_file_history_from_date_filter() {
    let cache = parse_mock_log(multi_commit_log());

    // Only commits >= timestamp 1700001000
    let (history, _total) = cache.query_file_history("src/main.rs", None, Some(1700001000), None, None, None);
    assert_eq!(history.len(), 2, "Should return 2 commits after from filter");
    assert!(history.iter().all(|c| c.timestamp >= 1700001000));
}

#[test]
fn test_query_file_history_to_date_filter() {
    let cache = parse_mock_log(multi_commit_log());

    // Only commits <= timestamp 1700001000
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, Some(1700001000), None, None);
    assert_eq!(history.len(), 2, "Should return 2 commits before to filter");
    assert!(history.iter().all(|c| c.timestamp <= 1700001000));
}

#[test]
fn test_query_file_history_from_to_filter() {
    let cache = parse_mock_log(multi_commit_log());

    // Only commits between 1700000500 and 1700001500
    let (history, _total) = cache.query_file_history("src/main.rs", None, Some(1700000500), Some(1700001500), None, None);
    assert_eq!(history.len(), 1, "Should return 1 commit in range");
    assert_eq!(history[0].timestamp, 1700001000);
}

#[test]
fn test_query_file_history_nonexistent_file() {
    let cache = parse_mock_log(multi_commit_log());

    let (history, _total) = cache.query_file_history("nonexistent.rs", None, None, None, None, None);
    assert!(history.is_empty(), "Nonexistent file should return empty vec");
}

#[test]
fn test_query_file_history_commit_info_fields() {
    let cache = parse_mock_log(multi_commit_log());

    let (history, _total) = cache.query_file_history("Cargo.toml", None, None, None, None, None);
    assert_eq!(history.len(), 1);

    let commit = &history[0];
    assert_eq!(commit.hash, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(commit.timestamp, 1700000000);
    assert_eq!(commit.author_name, "Alice");
    assert_eq!(commit.author_email, "alice@example.com");
    assert_eq!(commit.subject, "Initial commit");
}

// ─── Query: authors ─────────────────────────────────────────────────

#[test]
fn test_query_authors_single_file() {
    let cache = parse_mock_log(multi_commit_log());

    let authors = cache.query_authors("src/main.rs", None, None, None, None);
    assert_eq!(authors.len(), 2, "src/main.rs should have 2 authors");

    // Alice has 2 commits, Bob has 1
    let alice = authors.iter().find(|a| a.name == "Alice").expect("Alice should be present");
    let bob = authors.iter().find(|a| a.name == "Bob").expect("Bob should be present");

    assert_eq!(alice.commit_count, 2);
    assert_eq!(bob.commit_count, 1);

    // Authors should be sorted by commit count descending
    assert!(authors[0].commit_count >= authors[1].commit_count);
}

#[test]
fn test_query_authors_timestamps() {
    let cache = parse_mock_log(multi_commit_log());

    let authors = cache.query_authors("src/main.rs", None, None, None, None);

    // Alice has commits at 1700000000 and 1700002000
    let alice = authors.iter().find(|a| a.name == "Alice").expect("Alice should be present");
    assert_eq!(alice.first_commit_timestamp, 1700000000, "Alice's first commit should be earliest");
    assert_eq!(alice.last_commit_timestamp, 1700002000, "Alice's last commit should be latest");

    // Bob has only one commit at 1700001000
    let bob = authors.iter().find(|a| a.name == "Bob").expect("Bob should be present");
    assert_eq!(bob.first_commit_timestamp, 1700001000, "Bob's first and last should be the same");
    assert_eq!(bob.last_commit_timestamp, 1700001000, "Bob's first and last should be the same");
}

#[test]
fn test_query_authors_directory() {
    let cache = parse_mock_log(multi_commit_log());

    // "src" should match src/main.rs and src/lib.rs
    let authors = cache.query_authors("src", None, None, None, None);
    assert_eq!(authors.len(), 2, "src/ should have 2 authors");

    // Alice: commits 0 (src/main.rs) and 2 (src/main.rs) — deduplicated = 2 unique commits
    // Bob: commit 1 (src/main.rs, src/lib.rs) — 1 unique commit
    let alice = authors.iter().find(|a| a.name == "Alice").unwrap();
    let bob = authors.iter().find(|a| a.name == "Bob").unwrap();
    assert_eq!(alice.commit_count, 2);
    assert_eq!(bob.commit_count, 1);
}

#[test]
fn test_query_authors_empty_path_matches_all() {
    let cache = parse_mock_log(multi_commit_log());

    let authors = cache.query_authors("", None, None, None, None);
    assert_eq!(authors.len(), 2, "Empty path should match all files");
}

// ─── Query: activity ────────────────────────────────────────────────

#[test]
fn test_query_activity_directory_prefix() {
    let cache = parse_mock_log(multi_commit_log());

    let activity = cache.query_activity("src", None, None, None, None);
    assert_eq!(activity.len(), 2, "src/ should have 2 files (main.rs, lib.rs)");

    let main_activity = activity.iter().find(|a| a.file_path == "src/main.rs").unwrap();
    assert_eq!(main_activity.commit_count, 3);

    let lib_activity = activity.iter().find(|a| a.file_path == "src/lib.rs").unwrap();
    assert_eq!(lib_activity.commit_count, 1);
}

#[test]
fn test_query_activity_prefix_no_false_positive() {
    // Ensure "src" doesn't match "src2/file.rs"
    let log = concat!(
        "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞a@b.com␞Name␞Commit 1\n",
        "src/main.rs\n",
        "src2/other.rs\n",
        "\n",
    );
    let cache = parse_mock_log(log);

    let activity = cache.query_activity("src", None, None, None, None);
    assert_eq!(activity.len(), 1, "src should not match src2");
    assert_eq!(activity[0].file_path, "src/main.rs");
}

#[test]
fn test_query_activity_date_filter() {
    let cache = parse_mock_log(multi_commit_log());

    // Only activity after timestamp 1700001500
    let activity = cache.query_activity("src", Some(1700001500), None, None, None);
    assert_eq!(activity.len(), 1, "Only src/main.rs has commits after 1700001500");
    assert_eq!(activity[0].file_path, "src/main.rs");
    assert_eq!(activity[0].commit_count, 1);
}

#[test]
fn test_query_activity_empty_path_matches_all() {
    let cache = parse_mock_log(multi_commit_log());

    let activity = cache.query_activity("", None, None, None, None);
    assert_eq!(activity.len(), 3, "Empty path should match all 3 files");
}

#[test]
fn test_query_activity_authors_list() {
    let cache = parse_mock_log(multi_commit_log());

    let activity = cache.query_activity("src/main.rs", None, None, None, None);
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].authors.len(), 2, "src/main.rs should have 2 unique authors");
}

// ─── is_valid_for tests ─────────────────────────────────────────────

#[test]
fn test_is_valid_for_matching_head() {
    let cache = parse_mock_log(multi_commit_log());
    assert!(
        cache.is_valid_for("abc123def456abc123def456abc123def456abc1"),
        "Should be valid for matching HEAD hash"
    );
}

#[test]
fn test_is_valid_for_non_matching_head() {
    let cache = parse_mock_log(multi_commit_log());
    assert!(
        !cache.is_valid_for("different_hash_value"),
        "Should be invalid for different HEAD hash"
    );
}

#[test]
fn test_is_valid_for_checks_format_version() {
    let mut cache = parse_mock_log(multi_commit_log());
    cache.format_version = 999;
    assert!(
        !cache.is_valid_for("abc123def456abc123def456abc123def456abc1"),
        "Should be invalid for mismatched format version"
    );
}

// ─── detect_default_branch — requires real git, marked #[ignore] ────

#[test]
fn test_detect_default_branch() {
    // This test requires a real git repository
    let repo = std::path::Path::new(".");
    let branch = GitHistoryCache::detect_default_branch(repo);
    assert!(branch.is_ok(), "Should detect a branch: {:?}", branch.err());
    let branch = branch.unwrap();
    assert!(
        ["main", "master", "develop", "trunk", "HEAD"].contains(&branch.as_str()),
        "Branch should be one of the expected names, got: {}",
        branch
    );
}

// ─── CommitMeta size verification ───────────────────────────────────

#[test]
fn test_commit_meta_size() {
    let size = std::mem::size_of::<CommitMeta>();
    // Design target: 38 bytes. With Rust alignment (i64 forces 8-byte struct alignment),
    // actual size is 40 bytes (38 rounded up to next multiple of 8).
    // This is acceptable: 50K commits × 40 bytes = 2 MB (vs 1.9 MB at 38 bytes).
    assert!(
        size <= 48,
        "CommitMeta should be compact, got {} bytes (target: 38, max acceptable: 48)",
        size
    );
    eprintln!("[test] CommitMeta size: {} bytes (design target: 38)", size);
}

// ─── Hex conversion tests ───────────────────────────────────────────

#[test]
fn test_hex_to_bytes_roundtrip() {
    let hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let bytes = parse_hex_hash(hex).unwrap();
    let back = format_hex_hash(&bytes);
    assert_eq!(back, hex);
}

#[test]
fn test_hex_to_bytes_mixed_case() {
    let hex = "AbCdEf0123456789AbCdEf0123456789AbCdEf01";
    let bytes = parse_hex_hash(hex).unwrap();
    let back = format_hex_hash(&bytes);
    assert_eq!(back, hex.to_lowercase());
}

#[test]
fn test_hex_to_bytes_invalid_length() {
    assert!(parse_hex_hash("abc").is_err());
    assert!(parse_hex_hash("").is_err());
}

#[test]
fn test_hex_to_bytes_invalid_chars() {
    let bad = "gggggggggggggggggggggggggggggggggggggggg";
    assert!(parse_hex_hash(bad).is_err());
}

// ─── Serialization roundtrip test ───────────────────────────────────

#[test]
fn test_cache_serialization_roundtrip() {
    let cache = parse_mock_log(multi_commit_log());

    // Serialize with bincode
    let encoded = bincode::serialize(&cache).expect("serialization should succeed");
    let decoded: GitHistoryCache =
        bincode::deserialize(&encoded).expect("deserialization should succeed");

    assert_eq!(decoded.format_version, cache.format_version);
    assert_eq!(decoded.head_hash, cache.head_hash);
    assert_eq!(decoded.branch, cache.branch);
    assert_eq!(decoded.commits.len(), cache.commits.len());
    assert_eq!(decoded.authors.len(), cache.authors.len());
    assert_eq!(decoded.subjects, cache.subjects);
    assert_eq!(decoded.file_commits.len(), cache.file_commits.len());

    // Verify query still works after deserialization
    let (history, _total) = decoded.query_file_history("src/main.rs", None, None, None, None, None);
    assert_eq!(history.len(), 3);
}

#[test]
fn test_cache_lz4_compressed_roundtrip() {
    let cache = parse_mock_log(multi_commit_log());

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.git-history");

    // Use the same save_compressed/load_compressed from index.rs
    crate::index::save_compressed(&path, &cache, "git-history-test").unwrap();
    let loaded: GitHistoryCache =
        crate::index::load_compressed(&path, "git-history-test").unwrap();

    assert_eq!(loaded.commits.len(), cache.commits.len());
    assert_eq!(loaded.authors.len(), cache.authors.len());
    assert_eq!(loaded.subjects, cache.subjects);
    assert_eq!(loaded.file_commits.len(), cache.file_commits.len());

    // Verify queries work on loaded cache
    let (history, _total) = loaded.query_file_history("src/main.rs", None, None, None, None, None);
    assert_eq!(history.len(), 3);
}

// ─── Edge case: author deduplication ────────────────────────────────

#[test]
fn test_author_deduplication() {
    let log = concat!(
        "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞same@example.com␞Same Author␞Commit 1\n",
        "file1.rs\n\n",
        "COMMIT:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb␞1700001000␞same@example.com␞Same Author␞Commit 2\n",
        "file2.rs\n\n",
    );
    let cache = parse_mock_log(log);

    assert_eq!(cache.authors.len(), 1, "Same author should be deduplicated");
    assert_eq!(cache.commits[0].author_idx, cache.commits[1].author_idx);
}

// ─── Edge case: query with normalization ────────────────────────────

#[test]
fn test_query_with_backslash_path() {
    let cache = parse_mock_log(multi_commit_log());

    // Query with Windows-style path should still find the file
    let (history, _total) = cache.query_file_history("src\\main.rs", None, None, None, None, None);
    assert_eq!(history.len(), 3, "Backslash path should be normalized for lookup");
}

#[test]
fn test_query_with_dot_slash_path() {
    let cache = parse_mock_log(multi_commit_log());

    let (history, _total) = cache.query_file_history("./src/main.rs", None, None, None, None, None);
    assert_eq!(history.len(), 3, "./prefix should be normalized for lookup");
}

// ─── Edge case: format_version field ────────────────────────────────

#[test]
fn test_format_version_is_set() {
    let cache = parse_mock_log(multi_commit_log());
    assert_eq!(cache.format_version, FORMAT_VERSION);
}

// ─── Path prefix matching unit tests ────────────────────────────────

#[test]
fn test_query_activity_exact_file_match() {
    let cache = parse_mock_log(multi_commit_log());

    let activity = cache.query_activity("src/main.rs", None, None, None, None);
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].file_path, "src/main.rs");
}

#[test]
fn test_query_activity_sorted_by_last_modified() {
    let cache = parse_mock_log(multi_commit_log());

    let activity = cache.query_activity("", None, None, None, None);
    for i in 1..activity.len() {
        assert!(
            activity[i - 1].last_modified >= activity[i].last_modified,
            "Activity should be sorted by last_modified descending"
        );
    }
}

// ─── Disk persistence tests (save_to_disk / load_from_disk) ─────────

#[test]
fn test_save_load_disk_roundtrip() {
    let cache = parse_mock_log(multi_commit_log());

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test-cache.git-history");

    // Save to disk
    cache.save_to_disk(&path).expect("save_to_disk should succeed");
    assert!(path.exists(), "Cache file should exist after save");

    // Load from disk
    let loaded = GitHistoryCache::load_from_disk(&path).expect("load_from_disk should succeed");

    // Verify all fields match
    assert_eq!(loaded.format_version, cache.format_version);
    assert_eq!(loaded.head_hash, cache.head_hash);
    assert_eq!(loaded.branch, cache.branch);
    assert_eq!(loaded.commits.len(), cache.commits.len());
    assert_eq!(loaded.authors.len(), cache.authors.len());
    assert_eq!(loaded.subjects, cache.subjects);
    assert_eq!(loaded.file_commits.len(), cache.file_commits.len());

    // Verify queries work on loaded cache
    let (history, _total) = loaded.query_file_history("src/main.rs", None, None, None, None, None);
    assert_eq!(history.len(), 3);

    let authors = loaded.query_authors("src", None, None, None, None);
    assert_eq!(authors.len(), 2);
}

#[test]
fn test_save_to_disk_atomic_write() {
    // Verify that save_to_disk uses atomic write (temp file + rename)
    // by checking that the temp file is cleaned up
    let cache = parse_mock_log(multi_commit_log());

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("atomic-test.git-history");
    let tmp_path = tmp.path().join("atomic-test.git-history.tmp");

    cache.save_to_disk(&path).unwrap();

    assert!(path.exists(), "Final cache file should exist");
    assert!(!tmp_path.exists(), "Temp file should be cleaned up after rename");
}

#[test]
fn test_load_from_disk_missing_file() {
    let path = std::path::Path::new("/nonexistent/path/cache.git-history");
    let result = GitHistoryCache::load_from_disk(path);
    assert!(result.is_err(), "Loading nonexistent file should fail");
}

#[test]
fn test_load_from_disk_corrupt_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("corrupt.git-history");

    // Write garbage data
    std::fs::write(&path, b"this is not valid cache data at all!!!").unwrap();

    let result = GitHistoryCache::load_from_disk(&path);
    assert!(result.is_err(), "Loading corrupt file should fail");
}

#[test]
fn test_load_from_disk_wrong_format_version() {
    let mut cache = parse_mock_log(multi_commit_log());
    cache.format_version = 999; // wrong version

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("wrong-version.git-history");

    // Save with wrong version — use raw save_compressed to bypass any validation
    crate::index::save_compressed(&path, &cache, "test").unwrap();

    let result = GitHistoryCache::load_from_disk(&path);
    assert!(result.is_err(), "Loading cache with wrong format_version should fail");
    let err_msg = result.unwrap_err();
    assert!(
        err_msg.contains("format version mismatch"),
        "Error should mention format version, got: {}",
        err_msg
    );
}

#[test]
fn test_save_to_disk_creates_parent_dirs() {
    let cache = parse_mock_log(multi_commit_log());

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nested").join("dir").join("cache.git-history");

    cache.save_to_disk(&path).expect("save_to_disk should create parent dirs");
    assert!(path.exists());
}

// ─── cache_path_for tests ───────────────────────────────────────────

#[test]
fn test_cache_path_for_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let path = GitHistoryCache::cache_path_for(".", tmp.path());

    let ext = path.extension().and_then(|e| e.to_str());
    assert_eq!(ext, Some("git-history"), "Cache file should have .git-history extension");
}

#[test]
fn test_cache_path_for_deterministic() {
    let tmp = tempfile::tempdir().unwrap();

    let path1 = GitHistoryCache::cache_path_for(".", tmp.path());
    let path2 = GitHistoryCache::cache_path_for(".", tmp.path());

    assert_eq!(path1, path2, "Same input should produce same path");
}

// ─── is_ancestor / object_exists — require git, marked #[ignore] ───

#[test]
fn test_is_ancestor_in_real_repo() {
    let repo = std::path::Path::new(".");
    // HEAD~1 should be an ancestor of HEAD in any repo with history
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let parent = std::process::Command::new("git")
        .args(["rev-parse", "HEAD~1"])
        .output()
        .unwrap();
    let parent = String::from_utf8_lossy(&parent.stdout).trim().to_string();

    assert!(
        GitHistoryCache::is_ancestor(repo, &parent, &head),
        "HEAD~1 should be ancestor of HEAD"
    );
}

#[test]
fn test_object_exists_in_real_repo() {
    let repo = std::path::Path::new(".");
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();

    assert!(
        GitHistoryCache::object_exists(repo, &head),
        "HEAD should exist as a git object"
    );
    assert!(
        !GitHistoryCache::object_exists(repo, "0000000000000000000000000000000000000000"),
        "Zeroed hash should not exist"
    );
}
// ─── Gap 1: Integration test for build() with temp git repo ─────────

#[test]
fn test_build_with_real_git_repo() {
    use std::process::Command;

    // Create a unique temp directory
    let tmp_base = std::env::temp_dir();
    let tmp_dir = tmp_base.join(format!("search_cache_test_{}", std::process::id()));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir).unwrap();
    }
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Helper to run git commands in the temp dir
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&tmp_dir)
            .output()
            .expect("Failed to run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    // 1. git init
    git(&["init"]);
    git(&["config", "user.email", "test@test.com"]);
    git(&["config", "user.name", "Test"]);

    // 2. First commit: create file_a.txt
    let file_a = tmp_dir.join("file_a.txt");
    std::fs::write(&file_a, "initial content").unwrap();
    git(&["add", "file_a.txt"]);
    git(&["commit", "-m", "Initial commit"]);

    // 3. Second commit: create file_b.txt and modify file_a.txt
    let file_b = tmp_dir.join("file_b.txt");
    std::fs::write(&file_b, "file b content").unwrap();
    std::fs::write(&file_a, "modified content").unwrap();
    git(&["add", "file_a.txt", "file_b.txt"]);
    git(&["commit", "-m", "Add file_b and modify file_a"]);

    // 4. Detect the branch name (could be main or master depending on git version)
    let branch = GitHistoryCache::detect_default_branch(&tmp_dir)
        .expect("Should detect branch");

    // 5. Build the cache
    let cache = GitHistoryCache::build(&tmp_dir, &branch)
        .expect("build() should succeed");

    // 6. Verify: 2 commits
    assert_eq!(cache.commits.len(), 2, "Expected 2 commits");

    // 7. Verify: 1 author
    assert_eq!(cache.authors.len(), 1, "Expected 1 author");
    assert_eq!(cache.authors[0].name, "Test");
    assert_eq!(cache.authors[0].email, "test@test.com");

    // 8. Verify: file_a.txt appears in 2 commits, file_b.txt in 1
    let file_a_commits = cache.file_commits.get("file_a.txt");
    assert!(file_a_commits.is_some(), "file_a.txt should be in file_commits");
    assert_eq!(file_a_commits.unwrap().len(), 2, "file_a.txt should have 2 commit refs");

    let file_b_commits = cache.file_commits.get("file_b.txt");
    assert!(file_b_commits.is_some(), "file_b.txt should be in file_commits");
    assert_eq!(file_b_commits.unwrap().len(), 1, "file_b.txt should have 1 commit ref");

    // 9. Verify queries work
    let (history, _total) = cache.query_file_history("file_a.txt", None, None, None, None, None);
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].author_name, "Test");

    let authors = cache.query_authors("", None, None, None, None);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].name, "Test");
    assert_eq!(authors[0].commit_count, 2);

    // Cleanup
    std::fs::remove_dir_all(&tmp_dir).ok();
}

// ─── Gap 2: Bad timestamp parsing test ──────────────────────────────

#[test]
fn test_parser_bad_timestamp_skipped() {
    // Commit with a non-numeric timestamp should be skipped entirely
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞not_a_number␞test@test.com␞Test User␞Some subject\n\
               file1.rs\n\n\
               COMMIT:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb␞1700000000␞good@test.com␞Good User␞Good commit\n\
               file2.rs\n\n";
    let cache = parse_mock_log(log);

    // Only the good commit should be parsed
    assert_eq!(cache.commits.len(), 1, "Only the good commit should be parsed");

    // file1.rs should NOT be in file_commits (its commit was skipped)
    assert!(
        !cache.file_commits.contains_key("file1.rs"),
        "file1.rs should NOT be recorded — its commit had a bad timestamp"
    );

    // file2.rs should be recorded (good commit)
    assert!(
        cache.file_commits.contains_key("file2.rs"),
        "file2.rs should be recorded"
    );

    // Verify the good commit fields
    let (info, _total) = cache.query_file_history("file2.rs", None, None, None, None, None);
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].author_name, "Good User");
    assert_eq!(info[0].timestamp, 1700000000);
}

// ─── Gap 3: Author pool overflow test ───────────────────────────────

#[test]
fn test_author_pool_overflow_via_parser() {
    // Test that parse_git_log_stream returns an error when >65535 unique authors appear.
    // We test the boundary: 65534th, 65535th, and 65536th authors.
    // The check is `self.authors.len() >= 65535`, so:
    // - 65534th author (index 65534) should succeed (len=65534 < 65535)
    // - 65535th author (index 65535) should fail (len=65535 >= 65535)
    //
    // We construct a mock log with exactly 65536 unique authors.

    let mut log = String::with_capacity(65536 * 120);

    for i in 0..65536u32 {
        // Create a unique valid 40-char hex hash (35 a's + 5 hex digits)
        let hash = format!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa{:05x}", i);
        debug_assert_eq!(hash.len(), 40);

        let email = format!("a{}@b.com", i);
        let name = format!("A{}", i);

        log.push_str(&format!(
            "COMMIT:{}␞1700000000␞{}␞{}␞C{}\nf.rs\n\n",
            hash, email, name, i
        ));
    }

    let reader = Cursor::new(log.as_bytes());
    let mut builder = GitHistoryCache::builder();
    let result = parse_git_log_stream(reader, &mut builder);

    // The parser should propagate the error from intern_author on the 65536th author
    assert!(
        result.is_err(),
        "Parsing 65536 unique authors should return an error"
    );
    let err_msg = result.unwrap_err();
    assert!(
        err_msg.contains("Too many unique authors"),
        "Error should mention author limit, got: {}",
        err_msg
    );
}

#[test]
fn test_author_pool_boundary_65535_succeeds() {
    // Verify that exactly 65535 unique authors work (the maximum allowed).
    // Use 65535 unique authors — all should succeed.

    let mut log = String::with_capacity(65535 * 100);

    for i in 0..65535u32 {
        let hash = format!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb{:04x}", i);
        let hash = &hash[..40];
        let email = format!("x{}@y.com", i);
        let name = format!("X{}", i);

        log.push_str(&format!(
            "COMMIT:{}␞1700000000␞{}␞{}␞S{}\nf.rs\n\n",
            hash, email, name, i
        ));
    }

    let reader = Cursor::new(log.as_bytes());
    let mut builder = GitHistoryCache::builder();
    let result = parse_git_log_stream(reader, &mut builder);

    assert!(
        result.is_ok(),
        "Parsing exactly 65535 unique authors should succeed, got: {:?}",
        result.err()
    );

    let cache = GitHistoryCache::from_builder(
        builder,
        "0000000000000000000000000000000000000000".to_string(),
        "main".to_string(),
    );
    assert_eq!(cache.authors.len(), 65535);
    assert_eq!(cache.commits.len(), 65535);
}

// ─── Gap 4: cache_path_for() different directories → different paths ─

#[test]
fn test_cache_path_for_different_dirs_produce_different_paths() {
    let tmp = tempfile::tempdir().unwrap();

    // Create two real directories so canonicalize() works
    let dir_a = tmp.path().join("ProjectA");
    let dir_b = tmp.path().join("ProjectB");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let path_a = GitHistoryCache::cache_path_for(
        dir_a.to_str().unwrap(),
        tmp.path(),
    );
    let path_b = GitHistoryCache::cache_path_for(
        dir_b.to_str().unwrap(),
        tmp.path(),
    );

    assert_ne!(
        path_a, path_b,
        "Different directories should produce different cache paths"
    );

    // Both should have .git-history extension
    assert_eq!(
        path_a.extension().and_then(|e| e.to_str()),
        Some("git-history")
    );
    assert_eq!(
        path_b.extension().and_then(|e| e.to_str()),
        Some("git-history")
    );
}


// ─── Integration tests: real temp git repos ─────────────────────────

#[test]
fn test_build_save_load_roundtrip() {
    use std::process::Command;

    // Create a unique temp directory
    let tmp_base = std::env::temp_dir();
    let tmp_dir = tmp_base.join(format!("search_roundtrip_test_{}", std::process::id()));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir).unwrap();
    }
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&tmp_dir)
            .output()
            .expect("Failed to run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    // Set up repo with 2 commits
    git(&["init"]);
    git(&["config", "user.email", "roundtrip@test.com"]);
    git(&["config", "user.name", "RoundTrip"]);

    std::fs::write(tmp_dir.join("file_a.txt"), "aaa").unwrap();
    git(&["add", "file_a.txt"]);
    git(&["commit", "-m", "First commit"]);

    std::fs::write(tmp_dir.join("file_b.txt"), "bbb").unwrap();
    std::fs::write(tmp_dir.join("file_a.txt"), "aaa modified").unwrap();
    git(&["add", "file_a.txt", "file_b.txt"]);
    git(&["commit", "-m", "Second commit"]);

    let branch = GitHistoryCache::detect_default_branch(&tmp_dir).unwrap();
    let cache = GitHistoryCache::build(&tmp_dir, &branch).unwrap();

    // Save to disk
    let cache_path = tmp_dir.join("test-cache.git-history");
    cache.save_to_disk(&cache_path).unwrap();

    // Load from disk
    let loaded = GitHistoryCache::load_from_disk(&cache_path).unwrap();

    // Verify structural equality
    assert_eq!(loaded.commits.len(), cache.commits.len());
    assert_eq!(loaded.authors.len(), cache.authors.len());
    assert_eq!(loaded.file_commits.len(), cache.file_commits.len());
    assert_eq!(loaded.head_hash, cache.head_hash);
    assert_eq!(loaded.branch, cache.branch);
    assert_eq!(loaded.subjects, cache.subjects);

    // Verify queries produce same results
    let (orig_history, _total) = cache.query_file_history("file_a.txt", None, None, None, None, None);
    let (loaded_history, _total) = loaded.query_file_history("file_a.txt", None, None, None, None, None);
    assert_eq!(orig_history.len(), loaded_history.len());
    for (o, l) in orig_history.iter().zip(loaded_history.iter()) {
        assert_eq!(o.hash, l.hash);
        assert_eq!(o.timestamp, l.timestamp);
        assert_eq!(o.author_name, l.author_name);
        assert_eq!(o.subject, l.subject);
    }

    let orig_authors = cache.query_authors("", None, None, None, None);
    let loaded_authors = loaded.query_authors("", None, None, None, None);
    assert_eq!(orig_authors.len(), loaded_authors.len());
    assert_eq!(orig_authors[0].name, loaded_authors[0].name);
    assert_eq!(orig_authors[0].commit_count, loaded_authors[0].commit_count);

    // Cleanup
    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_build_then_add_commit_rebuild() {
    use std::process::Command;

    let tmp_base = std::env::temp_dir();
    let tmp_dir = tmp_base.join(format!("search_rebuild_test_{}", std::process::id()));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir).unwrap();
    }
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&tmp_dir)
            .output()
            .expect("Failed to run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    // Set up repo with 2 commits
    git(&["init"]);
    git(&["config", "user.email", "rebuild@test.com"]);
    git(&["config", "user.name", "Rebuild"]);

    std::fs::write(tmp_dir.join("file_a.txt"), "aaa").unwrap();
    git(&["add", "file_a.txt"]);
    git(&["commit", "-m", "Commit 1"]);

    std::fs::write(tmp_dir.join("file_b.txt"), "bbb").unwrap();
    git(&["add", "file_b.txt"]);
    git(&["commit", "-m", "Commit 2"]);

    let branch = GitHistoryCache::detect_default_branch(&tmp_dir).unwrap();
    let cache1 = GitHistoryCache::build(&tmp_dir, &branch).unwrap();
    assert_eq!(cache1.commits.len(), 2, "Initial build should have 2 commits");

    // Add a 3rd commit with a new file
    std::fs::write(tmp_dir.join("file_c.txt"), "ccc").unwrap();
    git(&["add", "file_c.txt"]);
    git(&["commit", "-m", "Commit 3"]);

    // Rebuild
    let cache2 = GitHistoryCache::build(&tmp_dir, &branch).unwrap();
    assert_eq!(cache2.commits.len(), 3, "Rebuilt cache should have 3 commits");

    // Verify file_c.txt is now in file_commits
    assert!(
        cache2.file_commits.contains_key("file_c.txt"),
        "file_c.txt should be in file_commits after rebuild"
    );
    assert_eq!(
        cache2.file_commits.get("file_c.txt").unwrap().len(),
        1,
        "file_c.txt should have 1 commit ref"
    );

    // Verify file_a.txt and file_b.txt still have correct commit counts
    assert_eq!(
        cache2.file_commits.get("file_a.txt").unwrap().len(),
        1,
        "file_a.txt should still have 1 commit ref"
    );
    assert_eq!(
        cache2.file_commits.get("file_b.txt").unwrap().len(),
        1,
        "file_b.txt should still have 1 commit ref"
    );

    // Cleanup
    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_build_with_unicode_filenames() {
    use std::process::Command;

    let tmp_base = std::env::temp_dir();
    let tmp_dir = tmp_base.join(format!("search_unicode_test_{}", std::process::id()));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir).unwrap();
    }
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&tmp_dir)
            .output()
            .expect("Failed to run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    git(&["init"]);
    git(&["config", "user.email", "unicode@test.com"]);
    git(&["config", "user.name", "Unicode"]);

    // Create files with unicode names
    let unicode_files = ["файл.txt", "日本語.rs", "café.md"];
    for name in &unicode_files {
        std::fs::write(tmp_dir.join(name), format!("content of {}", name)).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-m", "Add unicode files"]);

    let branch = GitHistoryCache::detect_default_branch(&tmp_dir).unwrap();
    let cache = GitHistoryCache::build(&tmp_dir, &branch).unwrap();

    // Verify all 3 unicode files are in file_commits
    assert_eq!(cache.commits.len(), 1, "Should have 1 commit");
    assert_eq!(
        cache.file_commits.len(),
        3,
        "Should have 3 files in file_commits, got keys: {:?}",
        cache.file_commits.keys().collect::<Vec<_>>()
    );

    for name in &unicode_files {
        assert!(
            cache.file_commits.contains_key(*name),
            "file_commits should contain '{}', got keys: {:?}",
            name,
            cache.file_commits.keys().collect::<Vec<_>>()
        );
    }

    // Verify query works with unicode filename
    let (history, _total) = cache.query_file_history("файл.txt", None, None, None, None, None);
    assert_eq!(history.len(), 1, "файл.txt should have 1 commit");
    assert_eq!(history[0].author_name, "Unicode");

    // Cleanup
    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_build_on_empty_repo() {
    use std::process::Command;

    let tmp_base = std::env::temp_dir();
    let tmp_dir = tmp_base.join(format!("search_empty_repo_test_{}", std::process::id()));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir).unwrap();
    }
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // git init only — no commits
    let output = Command::new("git")
        .args(["init"])
        .current_dir(&tmp_dir)
        .output()
        .expect("Failed to run git init");
    assert!(output.status.success());

    // detect_default_branch should return something (likely "HEAD" as fallback)
    let branch = GitHistoryCache::detect_default_branch(&tmp_dir);
    // In an empty repo, no branch exists yet, so detect_default_branch falls back to "HEAD"
    let branch = branch.unwrap_or_else(|_| "HEAD".to_string());

    // build() on an empty repo: git log on HEAD with no commits will fail
    let result = GitHistoryCache::build(&tmp_dir, &branch);

    // Either it returns an error (git log fails on empty repo) or an empty cache
    // — either is acceptable, as long as there's no panic
    match result {
        Ok(cache) => {
            assert_eq!(cache.commits.len(), 0, "Empty repo cache should have 0 commits");
            assert_eq!(cache.file_commits.len(), 0, "Empty repo cache should have 0 files");
        }
        Err(e) => {
            // Expected: git log fails on empty repo (no commits on HEAD)
            eprintln!("[test] build() on empty repo returned expected error: {}", e);
        }
    }

    // Cleanup
    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_is_valid_for_with_real_head() {
    use std::process::Command;

    let tmp_base = std::env::temp_dir();
    let tmp_dir = tmp_base.join(format!("search_valid_head_test_{}", std::process::id()));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir).unwrap();
    }
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let git = |args: &[&str]| -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(&tmp_dir)
            .output()
            .expect("Failed to run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    git(&["init"]);
    git(&["config", "user.email", "valid@test.com"]);
    git(&["config", "user.name", "ValidTest"]);

    std::fs::write(tmp_dir.join("file.txt"), "content").unwrap();
    git(&["add", "file.txt"]);
    git(&["commit", "-m", "Single commit"]);

    // Get HEAD hash
    let head_hash = git(&["rev-parse", "HEAD"]);

    let branch = GitHistoryCache::detect_default_branch(&tmp_dir).unwrap();
    let cache = GitHistoryCache::build(&tmp_dir, &branch).unwrap();

    // Cache should be valid for the real HEAD hash
    assert!(
        cache.is_valid_for(&head_hash),
        "Cache should be valid for the real HEAD hash '{}'",
        head_hash
    );

    // Cache should NOT be valid for a zeroed hash
    assert!(
        !cache.is_valid_for("0000000000000000000000000000000000000000"),
        "Cache should not be valid for zeroed hash"
    );

    // Cache should NOT be valid for a random string
    assert!(
        !cache.is_valid_for("not_a_real_hash"),
        "Cache should not be valid for a random string"
    );

    // Cleanup
    std::fs::remove_dir_all(&tmp_dir).ok();
}


// ─── Bug investigation: date boundary tests for query_file_history ──

#[test]
fn test_query_file_history_exact_date_boundary() {
    // Simulate: commit at 2024-12-16 17:28:32 UTC (timestamp 1734370112)
    // Query with from=1734307200 (2024-12-16 00:00:00) to=1734393599 (2024-12-16 23:59:59)
    // Should find the commit.
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1734370112␞dev@example.com␞Developer␞Fix something\n\
               src/helpers/PartialRetryHelper.cs\n\n";
    let cache = parse_mock_log(log);

    // Exact date boundary: start and end of 2024-12-16
    let from_ts = 1734307200i64;  // 2024-12-16 00:00:00 UTC
    let to_ts = 1734393599i64;    // 2024-12-16 23:59:59 UTC

    let (history, _total) = cache.query_file_history(
        "src/helpers/PartialRetryHelper.cs",
        None,
        Some(from_ts),
        Some(to_ts),
        None,
        None,
    );
    assert_eq!(
        history.len(), 1,
        "Commit at 1734370112 should be within [1734307200, 1734393599] (2024-12-16)"
    );
    assert_eq!(history[0].timestamp, 1734370112);
}

#[test]
fn test_query_file_history_wrong_year_returns_empty() {
    // Same commit at 2024-12-16, but query with 2025-12-16 range
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1734370112␞dev@example.com␞Developer␞Fix something\n\
               src/helpers/PartialRetryHelper.cs\n\n";
    let cache = parse_mock_log(log);

    // 2025-12-16 range (one year later!)
    let from_ts = 1765843200i64;  // 2025-12-16 00:00:00 UTC
    let to_ts = 1765929599i64;    // 2025-12-16 23:59:59 UTC

    let (history, _total) = cache.query_file_history(
        "src/helpers/PartialRetryHelper.cs",
        None,
        Some(from_ts),
        Some(to_ts),
        None,
        None,
    );
    assert_eq!(
        history.len(), 0,
        "Commit from 2024-12-16 should NOT be found when querying 2025-12-16"
    );
}

#[test]
fn test_query_activity_vs_file_history_consistency() {
    // Both should return the same commit when using the same date range.
    // This test verifies there's no discrepancy between the two query methods.
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1734370112␞dev@example.com␞Developer␞Fix something\n\
               src/helpers/PartialRetryHelper.cs\n\n";
    let cache = parse_mock_log(log);

    let from_ts = 1734307200i64;  // 2024-12-16 00:00:00 UTC
    let to_ts = 1734393599i64;    // 2024-12-16 23:59:59 UTC

    // query_file_history for the specific file
    let (history, _total) = cache.query_file_history(
        "src/helpers/PartialRetryHelper.cs",
        None,
        Some(from_ts),
        Some(to_ts),
        None,
        None,
    );

    // query_activity for all files
    let activity = cache.query_activity("", Some(from_ts), Some(to_ts), None, None);

    // Both should find the commit
    assert_eq!(history.len(), 1, "query_file_history should find 1 commit");
    assert_eq!(activity.len(), 1, "query_activity should find 1 file");
    assert_eq!(activity[0].commit_count, 1, "query_activity should show 1 commit for the file");
    assert_eq!(activity[0].file_path, "src/helpers/PartialRetryHelper.cs");
}

#[test]
fn test_query_file_history_path_case_sensitivity() {
    // Git stores paths case-sensitively. Verify that a case mismatch
    // causes query_file_history to return empty (HashMap exact match).
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1734370112␞dev@example.com␞Developer␞Fix\n\
               src/Helpers/PartialRetryHelper.cs\n\n";
    let cache = parse_mock_log(log);

    // Exact path should work
    let (history, _total) = cache.query_file_history("src/Helpers/PartialRetryHelper.cs", None, None, None, None, None);
    assert_eq!(history.len(), 1, "Exact path should find the commit");

    // Different case should NOT work (HashMap is case-sensitive)
    let (history_wrong_case, _total) = cache.query_file_history("src/helpers/PartialRetryHelper.cs", None, None, None, None, None);
    assert_eq!(
        history_wrong_case.len(), 0,
        "Case-mismatched path should NOT find the commit (HashMap is case-sensitive)"
    );

    // But query_activity with prefix match also requires exact case match
    let activity = cache.query_activity("src/helpers", None, None, None, None);
    assert_eq!(
        activity.len(), 0,
        "query_activity with wrong-case prefix should NOT find the file"
    );
}

// ─── Bug 2 investigation: query_authors first/last timestamp ────────

#[test]
fn test_query_authors_first_last_timestamps_nonzero() {
    // Verify that query_authors always returns non-zero timestamps
    // for files that have commits.
    let log = concat!(
        "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1734370112␞dev@example.com␞Developer␞First commit\n",
        "owners.txt\n\n",
        "COMMIT:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb␞1734456512␞dev@example.com␞Developer␞Second commit\n",
        "owners.txt\n\n",
    );
    let cache = parse_mock_log(log);

    let authors = cache.query_authors("owners.txt", None, None, None, None);
    assert_eq!(authors.len(), 1, "Should have 1 author");

    let author = &authors[0];
    assert_eq!(author.name, "Developer");
    assert_eq!(author.commit_count, 2);
    assert!(
        author.first_commit_timestamp > 0,
        "first_commit_timestamp should be non-zero, got {}",
        author.first_commit_timestamp
    );
    assert!(
        author.last_commit_timestamp > 0,
        "last_commit_timestamp should be non-zero, got {}",
        author.last_commit_timestamp
    );
    assert_eq!(author.first_commit_timestamp, 1734370112, "First commit timestamp");
    assert_eq!(author.last_commit_timestamp, 1734456512, "Last commit timestamp");
}

#[test]
fn test_query_authors_single_commit_timestamps_equal() {
    // When an author has only 1 commit, first and last should be equal
    let log = "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1734370112␞dev@example.com␞Developer␞Only commit\n\
               owners.txt\n\n";
    let cache = parse_mock_log(log);

    let authors = cache.query_authors("owners.txt", None, None, None, None);
    assert_eq!(authors.len(), 1);

    let author = &authors[0];
    assert_eq!(
        author.first_commit_timestamp, author.last_commit_timestamp,
        "Single-commit author should have equal first/last timestamps"
    );
    assert_eq!(author.first_commit_timestamp, 1734370112);
}


// ─── Query: author filter ───────────────────────────────────────────

#[test]
fn test_query_file_history_author_filter() {
    let cache = parse_mock_log(multi_commit_log());
    // Alice has 2 commits on src/main.rs, Bob has 1
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, Some("Alice"), None);
    assert_eq!(history.len(), 2, "Alice should have 2 commits on src/main.rs");
    assert!(history.iter().all(|c| c.author_name == "Alice"));
}

#[test]
fn test_query_file_history_author_filter_by_email() {
    let cache = parse_mock_log(multi_commit_log());
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, Some("bob@"), None);
    assert_eq!(history.len(), 1, "bob@ should match Bob's email");
    assert_eq!(history[0].author_name, "Bob");
}

#[test]
fn test_query_file_history_author_filter_case_insensitive() {
    let cache = parse_mock_log(multi_commit_log());
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, Some("alice"), None);
    assert_eq!(history.len(), 2, "Case-insensitive 'alice' should match 'Alice'");
}

#[test]
fn test_query_file_history_author_filter_no_match() {
    let cache = parse_mock_log(multi_commit_log());
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, Some("nonexistent"), None);
    assert!(history.is_empty(), "No author should match 'nonexistent'");
}

#[test]
fn test_query_file_history_message_filter() {
    let cache = parse_mock_log(multi_commit_log());
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, None, Some("bug"));
    assert_eq!(history.len(), 1, "Only 'Fix bug in main' should match 'bug'");
    assert!(history[0].subject.contains("bug"));
}

#[test]
fn test_query_file_history_message_filter_case_insensitive() {
    let cache = parse_mock_log(multi_commit_log());
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, None, Some("FIX BUG"));
    assert_eq!(history.len(), 1, "Case-insensitive 'FIX BUG' should match");
}

#[test]
fn test_query_file_history_message_filter_no_match() {
    let cache = parse_mock_log(multi_commit_log());
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, None, Some("nonexistent"));
    assert!(history.is_empty());
}

#[test]
fn test_query_file_history_author_and_message_combined() {
    let cache = parse_mock_log(multi_commit_log());
    // Alice + "bug" = only the "Fix bug in main" commit
    let (history, _total) = cache.query_file_history("src/main.rs", None, None, None, Some("Alice"), Some("bug"));
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].author_name, "Alice");
    assert!(history[0].subject.contains("bug"));
}

#[test]
fn test_query_file_history_author_and_date_combined() {
    let cache = parse_mock_log(multi_commit_log());
    // Alice + timestamp range that only includes the first commit
    let (history, _total) = cache.query_file_history("src/main.rs", None, Some(1699999000), Some(1700000500), Some("Alice"), None);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].subject, "Initial commit");
}

// ─── Query: activity with author/message filter ─────────────────────

#[test]
fn test_query_activity_author_filter() {
    let cache = parse_mock_log(multi_commit_log());
    let activity = cache.query_activity("", None, None, Some("Bob"), None);
    // Bob only touched src/main.rs and src/lib.rs
    assert_eq!(activity.len(), 2, "Bob touched 2 files");
    let paths: Vec<&str> = activity.iter().map(|a| a.file_path.as_str()).collect();
    assert!(paths.contains(&"src/main.rs"));
    assert!(paths.contains(&"src/lib.rs"));
}

#[test]
fn test_query_activity_message_filter() {
    let cache = parse_mock_log(multi_commit_log());
    let activity = cache.query_activity("", None, None, None, Some("Initial"));
    // "Initial commit" only touched src/main.rs and Cargo.toml
    assert_eq!(activity.len(), 2);
}

#[test]
fn test_query_activity_author_and_message_combined() {
    let cache = parse_mock_log(multi_commit_log());
    // Alice + "Initial" = only the first commit
    let activity = cache.query_activity("", None, None, Some("Alice"), Some("Initial"));
    assert_eq!(activity.len(), 2, "Initial commit touched src/main.rs and Cargo.toml");
}

// ─── Query: authors with filters ────────────────────────────────────

#[test]
fn test_query_authors_with_message_filter() {
    let cache = parse_mock_log(multi_commit_log());
    // Only commits with "feature" in message
    let authors = cache.query_authors("src", None, Some("feature"), None, None);
    // "Add feature X" is by Bob
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].name, "Bob");
}

#[test]
fn test_query_authors_with_date_filter() {
    let cache = parse_mock_log(multi_commit_log());
    // Only commits after 1700001500 - should be only the "Fix bug" by Alice
    let authors = cache.query_authors("src/main.rs", None, None, Some(1700001500), None);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].name, "Alice");
}

#[test]
fn test_query_authors_with_author_filter() {
    let cache = parse_mock_log(multi_commit_log());
    let authors = cache.query_authors("src/main.rs", Some("Alice"), None, None, None);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].name, "Alice");
    assert_eq!(authors[0].commit_count, 2);
}

// ─── Query: directory ownership (whole repo) ────────────────────────

#[test]
fn test_query_authors_whole_repo() {
    let cache = parse_mock_log(multi_commit_log());
    // Empty path = entire repo
    let authors = cache.query_authors("", None, None, None, None);
    assert_eq!(authors.len(), 2, "Should have 2 authors across entire repo");
    let alice = authors.iter().find(|a| a.name == "Alice").unwrap();
    let bob = authors.iter().find(|a| a.name == "Bob").unwrap();
    // Alice: 2 unique commits (aaaa..., cccc...)
    // Bob: 1 unique commit (bbbb...)
    assert_eq!(alice.commit_count, 2);
    assert_eq!(bob.commit_count, 1);
}

// ─── Bug fix: query_file_history returns total count before truncation ──

#[test]
fn test_query_file_history_total_count_before_truncation() {
    // BUG-2 fix: query_file_history should return total count BEFORE maxResults truncation.
    // Previously, totalCommits equaled returned count (e.g. 2 instead of 3).
    let cache = parse_mock_log(multi_commit_log());

    // src/main.rs has 3 commits. Request maxResults=2.
    let (history, total_count) = cache.query_file_history("src/main.rs", Some(2), None, None, None, None);

    assert_eq!(history.len(), 2, "Should return 2 commits (maxResults=2)");
    assert_eq!(total_count, 3, "Total count should be 3 (all commits), not 2 (truncated)");
}

#[test]
fn test_query_file_history_total_count_no_truncation() {
    // When maxResults is not limiting, total_count == returned count.
    let cache = parse_mock_log(multi_commit_log());

    let (history, total_count) = cache.query_file_history("src/main.rs", None, None, None, None, None);

    assert_eq!(history.len(), 3);
    assert_eq!(total_count, 3, "Total count should equal returned when no truncation");
}

#[test]
fn test_query_file_history_total_count_with_filters() {
    // Total count should reflect filtered results, not raw file commit count.
    let cache = parse_mock_log(multi_commit_log());

    // Alice has 2 commits on src/main.rs. Request maxResults=1.
    let (history, total_count) = cache.query_file_history("src/main.rs", Some(1), None, None, Some("Alice"), None);

    assert_eq!(history.len(), 1, "Should return 1 commit (maxResults=1)");
    assert_eq!(total_count, 2, "Total should be 2 (Alice's commits), not 1 (truncated)");
}

#[test]
fn test_query_file_history_total_count_nonexistent_file() {
    let cache = parse_mock_log(multi_commit_log());

    let (history, total_count) = cache.query_file_history("nonexistent.rs", Some(10), None, None, None, None);

    assert!(history.is_empty());
    assert_eq!(total_count, 0, "Nonexistent file should have 0 total");
}

// ─── Tests for subjects in FileActivity (Change A) ──────────────────

#[test]
fn test_query_activity_returns_subjects() {
    let cache = parse_mock_log(multi_commit_log());
    let activity = cache.query_activity("src/main.rs", None, None, None, None);
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].subjects.len(), 3, "src/main.rs has 3 commits with unique subjects");
    assert!(activity[0].subjects.contains(&"Add feature X".to_string()));
    assert!(activity[0].subjects.contains(&"Fix bug in main".to_string()));
    assert!(activity[0].subjects.contains(&"Initial commit".to_string()));
}

#[test]
fn test_query_activity_subjects_deduped() {
    // Two commits with same subject should be deduped
    let log = concat!(
        "COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞alice@example.com␞Alice␞Fix tests\n",
        "src/main.rs\n",
        "\n",
        "COMMIT:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb␞1700001000␞bob@example.com␞Bob␞Fix tests\n",
        "src/main.rs\n",
        "\n",
    );
    let cache = parse_mock_log(log);
    let activity = cache.query_activity("", None, None, None, None);
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].subjects.len(), 1, "Duplicate subjects should be deduped");
    assert_eq!(activity[0].subjects[0], "Fix tests");
}

#[test]
fn test_query_activity_subjects_with_message_filter() {
    let cache = parse_mock_log(multi_commit_log());
    // Filter to "Initial" only
    let activity = cache.query_activity("src/main.rs", None, None, None, Some("Initial"));
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].subjects.len(), 1);
    assert_eq!(activity[0].subjects[0], "Initial commit");
}

// ─── Tests for query_activity_by_commit (Change B) ──────────────────

#[test]
fn test_query_activity_by_commit_basic() {
    let cache = parse_mock_log(multi_commit_log());
    let commits = cache.query_activity_by_commit("", None, None, None, None, None, None);
    assert_eq!(commits.len(), 3, "Should have 3 commits");
    // Sorted by timestamp desc (newest first)
    assert_eq!(commits[0].subject, "Fix bug in main");
    assert_eq!(commits[1].subject, "Add feature X");
    assert_eq!(commits[2].subject, "Initial commit");
}

#[test]
fn test_query_activity_by_commit_files() {
    let cache = parse_mock_log(multi_commit_log());
    let commits = cache.query_activity_by_commit("", None, None, None, None, None, None);
    // "Fix bug in main" (commit ccc) touched src/main.rs
    assert_eq!(commits[0].files, vec!["src/main.rs"]);
    assert_eq!(commits[0].total_files, 1);
    // "Add feature X" (commit bbb) touched src/lib.rs and src/main.rs
    assert_eq!(commits[1].files, vec!["src/lib.rs", "src/main.rs"]);
    assert_eq!(commits[1].total_files, 2);
    // "Initial commit" (commit aaa) touched Cargo.toml and src/main.rs
    assert_eq!(commits[2].files, vec!["Cargo.toml", "src/main.rs"]);
    assert_eq!(commits[2].total_files, 2);
}

#[test]
fn test_query_activity_by_commit_date_filter() {
    let cache = parse_mock_log(multi_commit_log());
    // Only commits after 1700001500
    let commits = cache.query_activity_by_commit("", Some(1700001500), None, None, None, None, None);
    assert_eq!(commits.len(), 1, "Only 1 commit after 1700001500");
    assert_eq!(commits[0].subject, "Fix bug in main");
}

#[test]
fn test_query_activity_by_commit_author_filter() {
    let cache = parse_mock_log(multi_commit_log());
    let commits = cache.query_activity_by_commit("", None, None, Some("Bob"), None, None, None);
    assert_eq!(commits.len(), 1, "Bob has 1 commit");
    assert_eq!(commits[0].subject, "Add feature X");
}

#[test]
fn test_query_activity_by_commit_message_filter() {
    let cache = parse_mock_log(multi_commit_log());
    let commits = cache.query_activity_by_commit("", None, None, None, Some("bug"), None, None);
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "Fix bug in main");
}

#[test]
fn test_query_activity_by_commit_path_filter() {
    let cache = parse_mock_log(multi_commit_log());
    // Only commits that touched src/lib.rs
    let commits = cache.query_activity_by_commit("src/lib.rs", None, None, None, None, None, None);
    assert_eq!(commits.len(), 1, "Only 1 commit touched src/lib.rs");
    assert_eq!(commits[0].subject, "Add feature X");
    // Files should only include files matching the path filter
    assert_eq!(commits[0].files, vec!["src/lib.rs"]);
}

#[test]
fn test_query_activity_by_commit_max_results() {
    let cache = parse_mock_log(multi_commit_log());
    let commits = cache.query_activity_by_commit("", None, None, None, None, Some(2), None);
    assert_eq!(commits.len(), 2, "Should return only 2 commits");
    // Most recent first
    assert_eq!(commits[0].subject, "Fix bug in main");
    assert_eq!(commits[1].subject, "Add feature X");
}

#[test]
fn test_query_activity_by_commit_max_files_per_commit() {
    // Create a commit that touches many files
    let mut log = String::from("COMMIT:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa␞1700000000␞alice@example.com␞Alice␞Big refactor\n");
    for i in 0..30 {
        log.push_str(&format!("src/file_{:02}.rs\n", i));
    }
    log.push('\n');
    let cache = parse_mock_log(&log);
    let commits = cache.query_activity_by_commit("", None, None, None, None, None, Some(5));
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].files.len(), 5, "Should cap files at 5");
    assert_eq!(commits[0].total_files, 30, "total_files should reflect original count");
}

#[test]
fn test_query_activity_by_commit_sorted_by_date_desc() {
    let cache = parse_mock_log(multi_commit_log());
    let commits = cache.query_activity_by_commit("", None, None, None, None, None, None);
    for i in 1..commits.len() {
        assert!(
            commits[i - 1].timestamp >= commits[i].timestamp,
            "Commits should be sorted by timestamp descending"
        );
    }
}

#[test]
fn test_query_activity_by_commit_empty_result() {
    let cache = parse_mock_log(multi_commit_log());
    // No commits in this date range
    let commits = cache.query_activity_by_commit("", Some(1800000000), None, None, None, None, None);
    assert!(commits.is_empty(), "Should return empty for future dates");
}

#[test]
fn test_query_activity_by_commit_hash_format() {
    let cache = parse_mock_log(multi_commit_log());
    let commits = cache.query_activity_by_commit("", None, None, None, None, None, None);
    for c in &commits {
        assert_eq!(c.hash.len(), 40, "Hash should be 40-char hex string");
        assert!(c.hash.chars().all(|ch| ch.is_ascii_hexdigit()), "Hash should be hex");
    }
}