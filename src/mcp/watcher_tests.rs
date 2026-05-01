#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use super::*;
use std::collections::HashMap;


fn make_test_index() -> ContentIndex {
    let mut idx = HashMap::new();
    idx.insert("httpclient".to_string(), vec![Posting {
        file_id: 0,
        lines: vec![5, 12],
    }]);
    idx.insert("ilogger".to_string(), vec![Posting {
        file_id: 0,
        lines: vec![3],
    }, Posting {
        file_id: 1,
        lines: vec![1],
    }]);

    ContentIndex {
        root: ".".to_string(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        files: vec!["file0.cs".to_string(), "file1.cs".to_string()],
        index: idx,
        total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 30],
        ..Default::default()
    }
}

#[test]
fn test_build_watch_index_has_path_to_id() {
    let index = make_test_index();
    let watch_index = build_watch_index_from(index);

    assert!(watch_index.path_to_id.is_some());
    assert!(watch_index.file_tokens_authoritative);
    assert!(watch_index.file_tokens.is_empty());
}

#[test]
fn test_build_watch_index_populates_path_to_id() {
    let index = make_test_index();
    let watch_index = build_watch_index_from(index);

    let path_to_id = watch_index.path_to_id.as_ref().unwrap();
    assert_eq!(path_to_id.get(&PathBuf::from("file0.cs")), Some(&0));
    assert_eq!(path_to_id.get(&PathBuf::from("file1.cs")), Some(&1));
}

#[test]
fn test_incremental_update_new_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let new_file = dir.join("new_file.cs");
    std::fs::write(&new_file, "class NewClass { HttpClient client; }").unwrap();

    let mut index = make_test_index();
    index.path_to_id = Some(HashMap::new());
    // Populate path_to_id for existing files
    for (i, path) in index.files.iter().enumerate() {
        index.path_to_id.as_mut().unwrap().insert(PathBuf::from(path), i as u32);
    }

    let clean_path = PathBuf::from(crate::clean_path(&new_file.to_string_lossy()));
    update_file_in_index(&mut index, &clean_path);

    // New file should be added
    assert_eq!(index.files.len(), 3);
    assert!(index.index.contains_key("newclass"));
    assert!(index.index.contains_key("httpclient"));
}

#[test]
fn test_incremental_update_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let test_file = dir.join("test.cs");
    std::fs::write(&test_file, "class Original { OldToken stuff; }").unwrap();

    let clean = crate::clean_path(&test_file.to_string_lossy());
    let mut index = ContentIndex {
        root: ".".to_string(),
        files: vec![clean.clone()],
        index: {
            let mut m = HashMap::new();
            m.insert("original".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m.insert("oldtoken".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m
        },
        total_tokens: 10,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![5],
        path_to_id: Some({
            let mut m = HashMap::new();
            m.insert(PathBuf::from(&clean), 0u32);
            m
        }),
        ..Default::default()
    };

    // Now update the file content
    std::fs::write(&test_file, "class Updated { NewToken stuff; }").unwrap();
    update_file_in_index(&mut index, &PathBuf::from(&clean));

    // Old tokens should be gone, new tokens should be present
    assert!(!index.index.contains_key("original"), "old token 'original' should be removed");
    assert!(!index.index.contains_key("oldtoken"), "old token 'oldtoken' should be removed");
    assert!(index.index.contains_key("updated"), "new token 'updated' should be present");
    assert!(index.index.contains_key("newtoken"), "new token 'newtoken' should be present");
}

#[test]
fn test_remove_file() {
    let mut index = make_test_index();
    // Build path_to_id (no forward index needed)
    index = build_watch_index_from(index);

    // Remove file0.cs
    remove_file_from_index(&mut index, &PathBuf::from("file0.cs"));

    // httpclient was only in file0 — should be gone from index
    assert!(!index.index.contains_key("httpclient"), "httpclient should be removed with file0");

    // ilogger was in both files — should still exist for file1
    let ilogger = index.index.get("ilogger").unwrap();
    assert_eq!(ilogger.len(), 1);
    assert_eq!(ilogger[0].file_id, 1);

    // path_to_id should not contain file0 anymore
    let path_to_id = index.path_to_id.as_ref().unwrap();
    assert!(!path_to_id.contains_key(&PathBuf::from("file0.cs")));
    // files vec still has file0 for ID stability
    assert_eq!(index.files.len(), 2);
}

#[test]
fn test_matches_extensions() {
    let exts = vec!["cs".to_string(), "rs".to_string()];
    assert!(matches_extensions(Path::new("foo.cs"), &exts));
    assert!(matches_extensions(Path::new("bar.RS"), &exts));
    assert!(!matches_extensions(Path::new("baz.txt"), &exts));
    assert!(!matches_extensions(Path::new("no_ext"), &exts));
}

#[test]
fn test_is_inside_git_dir() {
    // Should detect .git directory in various positions
    assert!(is_inside_git_dir(Path::new(".git/config")));
    assert!(is_inside_git_dir(Path::new(".git/HEAD")));
    assert!(is_inside_git_dir(Path::new("repo/.git/config")));
    assert!(is_inside_git_dir(Path::new("repo/.git/modules/sub/config")));
    assert!(is_inside_git_dir(Path::new("C:/Projects/repo/.git/objects/pack/pack-abc.idx")));

    // Should NOT flag normal files
    assert!(!is_inside_git_dir(Path::new("src/main.rs")));
    assert!(!is_inside_git_dir(Path::new("my-git-tool/config.xml")));
    assert!(!is_inside_git_dir(Path::new(".gitignore")));
    assert!(!is_inside_git_dir(Path::new(".github/workflows/ci.yml")));
    assert!(!is_inside_git_dir(Path::new("docs/git-workflow.md")));
}

#[test]
fn test_purge_file_from_inverted_index_removes_single_file() {
    let mut inverted = HashMap::new();
    inverted.insert("token_a".to_string(), vec![
        Posting { file_id: 0, lines: vec![1, 5] },
        Posting { file_id: 1, lines: vec![3] },
    ]);
    inverted.insert("token_b".to_string(), vec![
        Posting { file_id: 0, lines: vec![2] },
    ]);
    inverted.insert("token_c".to_string(), vec![
        Posting { file_id: 1, lines: vec![10] },
    ]);

    purge_file_from_inverted_index(&mut inverted, 0);

    // token_a should still exist but only for file_id 1
    let token_a = inverted.get("token_a").unwrap();
    assert_eq!(token_a.len(), 1);
    assert_eq!(token_a[0].file_id, 1);

    // token_b was only in file_id 0 → should be removed entirely
    assert!(!inverted.contains_key("token_b"), "token_b should be removed when its only file is purged");

    // token_c should be untouched
    assert!(inverted.contains_key("token_c"));
    assert_eq!(inverted["token_c"][0].file_id, 1);
}

#[test]
fn test_purge_file_from_inverted_index_nonexistent_file() {
    let mut inverted = HashMap::new();
    inverted.insert("token".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
    ]);

    // Purging a file_id that doesn't exist should be a no-op
    purge_file_from_inverted_index(&mut inverted, 99);

    assert_eq!(inverted.len(), 1);
    assert_eq!(inverted["token"][0].file_id, 0);
}

#[test]
fn test_purge_file_from_inverted_index_empty_index() {
    let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
    purge_file_from_inverted_index(&mut inverted, 0);
    assert!(inverted.is_empty());
}

#[test]
fn test_remove_file_without_forward_index() {
    // Verify that remove works via brute-force scan of inverted index
    let mut index = make_test_index();
    index.path_to_id = Some({
        let mut m = HashMap::new();
        m.insert(PathBuf::from("file0.cs"), 0u32);
        m.insert(PathBuf::from("file1.cs"), 1u32);
        m
    });

    remove_file_from_index(&mut index, &PathBuf::from("file0.cs"));

    // httpclient was only in file0 — should be gone
    assert!(!index.index.contains_key("httpclient"));
    // ilogger was in both files — should still exist for file1
    let ilogger = index.index.get("ilogger").unwrap();
    assert_eq!(ilogger.len(), 1);
    assert_eq!(ilogger[0].file_id, 1);
}

#[test]
fn test_update_existing_file_without_forward_index() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let test_file = dir.join("test.cs");
    std::fs::write(&test_file, "class Original { OldToken stuff; }").unwrap();

    let clean = crate::clean_path(&test_file.to_string_lossy());
    let mut index = ContentIndex {
        root: ".".to_string(),
        files: vec![clean.clone()],
        index: {
            let mut m = HashMap::new();
            m.insert("original".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m.insert("oldtoken".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m
        },
        total_tokens: 10,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![5],
        path_to_id: Some({
            let mut m = HashMap::new();
            m.insert(PathBuf::from(&clean), 0u32);
            m
        }),
        ..Default::default()
    };

    // Update file content
    std::fs::write(&test_file, "class Updated { NewToken stuff; }").unwrap();
    update_file_in_index(&mut index, &PathBuf::from(&clean));

    // Old tokens removed via brute-force scan, new tokens added
    assert!(!index.index.contains_key("original"), "old token should be removed");
    assert!(!index.index.contains_key("oldtoken"), "old token should be removed");
    assert!(index.index.contains_key("updated"), "new token should be present");
    assert!(index.index.contains_key("newtoken"), "new token should be present");
}

#[test]
fn test_batch_purge_files_removes_multiple_files() {
    let mut inverted = HashMap::new();
    inverted.insert("token_a".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![2] },
        Posting { file_id: 2, lines: vec![3] },
    ]);
    inverted.insert("token_b".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
        Posting { file_id: 2, lines: vec![6] },
    ]);
    inverted.insert("token_c".to_string(), vec![
        Posting { file_id: 1, lines: vec![10] },
    ]);

    let mut ids = HashSet::new();
    ids.insert(0);
    ids.insert(2);
    let mut file_tokens = vec![
        vec!["token_a".to_string(), "token_b".to_string()],
        vec!["token_a".to_string(), "token_c".to_string()],
        vec!["token_a".to_string(), "token_b".to_string()],
    ];
    let file_token_counts = vec![1, 1, 1];
    let touched_tokens = batch_purge_files(&mut inverted, &mut file_tokens, &file_token_counts, &ids);
    assert_eq!(touched_tokens, vec!["token_a".to_string(), "token_b".to_string()]);

    // token_a should only have file_id 1
    let token_a = inverted.get("token_a").unwrap();
    assert_eq!(token_a.len(), 1);
    assert_eq!(token_a[0].file_id, 1);

    // token_b was only in files 0 and 2 → should be removed entirely
    assert!(!inverted.contains_key("token_b"), "token_b should be removed");

    // token_c was only in file 1 → should be untouched
    assert!(inverted.contains_key("token_c"));
    assert_eq!(inverted["token_c"][0].file_id, 1);
}

#[test]
fn test_batch_purge_files_empty_set() {
    let mut inverted = HashMap::new();
    inverted.insert("token".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
    ]);

    let mut file_tokens = vec![vec!["token".to_string()]];
    let file_token_counts = vec![1];
    let touched_tokens = batch_purge_files(&mut inverted, &mut file_tokens, &file_token_counts, &HashSet::new());
    assert!(touched_tokens.is_empty());

    // Should be a no-op
    assert_eq!(inverted.len(), 1);
    assert_eq!(inverted["token"][0].file_id, 0);
}

#[test]
fn test_batch_purge_files_single_file_equivalent_to_purge_single() {
    // Verify that batch_purge with 1 file_id gives same result as purge_file_from_inverted_index
    let mut inverted1 = HashMap::new();
    inverted1.insert("token_a".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![2] },
    ]);
    inverted1.insert("token_b".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
    ]);

    let mut inverted2 = inverted1.clone();

    // Single purge
    purge_file_from_inverted_index(&mut inverted1, 0);

    // Batch purge with 1 element
    let mut ids = HashSet::new();
    ids.insert(0);
    let mut file_tokens = vec![
        vec!["token_a".to_string(), "token_b".to_string()],
        vec!["token_a".to_string()],
    ];
    let file_token_counts = vec![1, 1];
    let touched_tokens = batch_purge_files(&mut inverted2, &mut file_tokens, &file_token_counts, &ids);
    assert_eq!(touched_tokens, vec!["token_a".to_string(), "token_b".to_string()]);

    // Results should be identical
    assert_eq!(inverted1.len(), inverted2.len());
    for (key, val1) in &inverted1 {
        let val2 = inverted2.get(key).unwrap();
        assert_eq!(val1.len(), val2.len());
        for (p1, p2) in val1.iter().zip(val2.iter()) {
            assert_eq!(p1.file_id, p2.file_id);
            assert_eq!(p1.lines, p2.lines);
        }
    }
}

fn make_purge_test_index() -> ContentIndex {
    let mut inverted = HashMap::new();
    inverted.insert("shared".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![2] },
        Posting { file_id: 2, lines: vec![3] },
    ]);
    inverted.insert("exclusive_a".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("exclusive_b".to_string(), vec![Posting { file_id: 1, lines: vec![1] }]);
    inverted.insert("exclusive_c".to_string(), vec![Posting { file_id: 2, lines: vec![1] }]);

    let mut index = ContentIndex {
        root: "test".to_string(),
        files: vec!["a.cs".to_string(), "b.cs".to_string(), "c.cs".to_string()],
        index: inverted,
        total_tokens: 9,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![3, 3, 3],
        ..Default::default()
    };
    index.rebuild_file_tokens();
    index
}

fn assert_inverted_eq(
    left: &HashMap<String, Vec<Posting>>,
    right: &HashMap<String, Vec<Posting>>,
) {
    assert_eq!(left.len(), right.len());
    for (token, left_postings) in left {
        let right_postings = right.get(token).unwrap_or_else(|| panic!("missing token {token}"));
        assert_eq!(left_postings.len(), right_postings.len(), "postings len for {token}");
        for (left_posting, right_posting) in left_postings.iter().zip(right_postings.iter()) {
            assert_eq!(left_posting.file_id, right_posting.file_id, "file_id for {token}");
            assert_eq!(left_posting.lines, right_posting.lines, "lines for {token}");
        }
    }
}

#[test]
fn test_targeted_purge_matches_full_scan() {
    let mut targeted = make_purge_test_index();
    let mut full_scan = targeted.clone();
    full_scan.file_tokens.clear();

    let ids = HashSet::from([0u32, 2u32]);
    let targeted_counts = targeted.file_token_counts.clone();
    let full_scan_counts = full_scan.file_token_counts.clone();
    let touched_tokens = batch_purge_files(
        &mut targeted.index,
        &mut targeted.file_tokens,
        &targeted_counts,
        &ids,
    );
    batch_purge_files(&mut full_scan.index, &mut full_scan.file_tokens, &full_scan_counts, &ids);

    assert_eq!(touched_tokens, vec![
        "exclusive_a".to_string(),
        "exclusive_c".to_string(),
        "shared".to_string(),
    ]);
    assert_inverted_eq(&targeted.index, &full_scan.index);
    assert!(targeted.file_tokens[0].is_empty());
    assert!(targeted.file_tokens[2].is_empty());
}

#[test]
fn test_rebuild_file_tokens_roundtrip() {
    let mut index = make_purge_test_index();
    let expected = index.file_tokens.clone();

    index.file_tokens.clear();
    index.rebuild_file_tokens();

    assert_eq!(index.file_tokens, expected);
}

#[test]
fn test_file_tokens_cleared_after_purge() {
    let mut index = make_purge_test_index();
    let ids = HashSet::from([1u32]);

    let file_token_counts = index.file_token_counts.clone();
    batch_purge_files(&mut index.index, &mut index.file_tokens, &file_token_counts, &ids);

    assert!(index.file_tokens[1].is_empty());
    assert!(!index.index.contains_key("exclusive_b"));
    assert!(index.index["shared"].iter().all(|posting| posting.file_id != 1));
}

#[test]
fn test_fallback_when_file_tokens_empty() {
    let mut index = make_purge_test_index();
    index.file_tokens.clear();
    let ids = HashSet::from([1u32]);

    let file_token_counts = index.file_token_counts.clone();
    let touched_tokens = batch_purge_files(&mut index.index, &mut index.file_tokens, &file_token_counts, &ids);

    assert_eq!(touched_tokens, vec![
        "exclusive_a".to_string(),
        "exclusive_b".to_string(),
        "exclusive_c".to_string(),
        "shared".to_string(),
    ]);
    assert!(!index.index.contains_key("exclusive_b"));
    assert!(index.index["shared"].iter().all(|posting| posting.file_id != 1));
}

#[test]
#[should_panic(expected = "file_tokens missing entry for file_id 1")]
fn test_partial_file_tokens_is_bug_not_fallback() {
    let mut index = make_purge_test_index();
    index.file_tokens.truncate(1);
    let ids = HashSet::from([1u32]);

    let file_token_counts = index.file_token_counts.clone();
    batch_purge_files(&mut index.index, &mut index.file_tokens, &file_token_counts, &ids);
}

#[test]
#[should_panic(expected = "file_tokens empty for file_id 1")]
fn test_empty_file_tokens_slot_with_count_is_bug_not_noop() {
    // Regression for the dangerous partial-map shape where the vector length is
    // correct but one live slot is empty. Targeted purge would otherwise skip
    // that file silently and leave stale postings searchable.
    let mut index = make_purge_test_index();
    index.file_tokens[1].clear();
    let ids = HashSet::from([1u32]);

    let file_token_counts = index.file_token_counts.clone();
    batch_purge_files(&mut index.index, &mut index.file_tokens, &file_token_counts, &ids);
}

#[test]
fn test_file_tokens_maintained_after_insert() {
    let mut tokens = HashMap::new();
    tokens.insert("beta".to_string(), vec![1, 2]);
    tokens.insert("alpha".to_string(), vec![1]);
    let result = TokenizedFileResult {
        path: PathBuf::from("new.cs"),
        tokens,
        total_tokens: 3,
    };
    let mut index = ContentIndex {
        root: "test".to_string(),
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    };

    let touched_tokens = apply_tokenized_file(&mut index, result, true);

    assert_eq!(touched_tokens, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(index.file_tokens[0], touched_tokens);
}

#[test]
fn test_watch_update_lazily_rebuilds_file_tokens() {
    let (_tmp, dir, index) = make_batch_test_setup();
    {
        let mut idx = index.write().unwrap();
        idx.file_tokens_authoritative = true;
        idx.file_tokens.clear();
    }

    let file_a = dir.join("a.cs");
    std::fs::write(&file_a, "class Delta { string changed; }\n").unwrap();

    let result = update_content_index(&index, &[], std::slice::from_ref(&file_a));

    assert!(result.ok);
    let idx = index.read().unwrap();
    let file_id = idx.path_to_id.as_ref().unwrap().get(&file_a).copied().unwrap();
    assert!(idx.file_tokens_authoritative);
    assert!(!idx.file_tokens.is_empty());
    assert!(idx.file_tokens[file_id as usize].contains(&"delta".to_string()));
    assert!(idx.index.get("alpha")
        .map(|postings| postings.iter().all(|posting| posting.file_id != file_id))
        .unwrap_or(true));
}


#[test]
fn test_content_index_clone_skips_file_tokens() {
    let index = make_purge_test_index();

    let cloned = index.clone();

    assert!(cloned.file_tokens.is_empty());
    assert_inverted_eq(&cloned.index, &index.index);
}


#[test]
fn test_total_tokens_decremented_on_update() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let test_file = dir.join("test.cs");
    std::fs::write(&test_file, "class Original { OldToken stuff; }").unwrap();

    let clean = crate::clean_path(&test_file.to_string_lossy());
    let mut index = ContentIndex {
        root: ".".to_string(),
        files: vec![clean.clone()],
        index: {
            let mut m = HashMap::new();
            m.insert("original".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m.insert("oldtoken".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m.insert("stuff".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m.insert("class".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m
        },
        total_tokens: 4,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![4],
        path_to_id: Some({
            let mut m = HashMap::new();
            m.insert(PathBuf::from(&clean), 0u32);
            m
        }),
        ..Default::default()
    };

    // Update file with different content
    std::fs::write(&test_file, "class Updated { NewToken stuff; }").unwrap();
    update_file_in_index(&mut index, &PathBuf::from(&clean));

    // total_tokens should equal sum of file_token_counts
    let sum: u64 = index.file_token_counts.iter().map(|&c| c as u64).sum();
    assert_eq!(index.total_tokens, sum,
        "total_tokens ({}) should equal sum of file_token_counts ({})",
        index.total_tokens, sum);
}

#[test]
fn test_total_tokens_decremented_on_remove() {
    let mut index = make_test_index();
    index = build_watch_index_from(index);

    let initial_total = index.total_tokens;
    let file0_tokens = index.file_token_counts[0] as u64;

    remove_file_from_index(&mut index, &PathBuf::from("file0.cs"));

    assert_eq!(index.total_tokens, initial_total - file0_tokens,
        "total_tokens should decrease by file0's token count");
}

#[test]
fn test_total_tokens_consistency_after_multiple_ops() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let file1 = dir.join("a.cs");
    let file2 = dir.join("b.cs");
    std::fs::write(&file1, "class Alpha { }").unwrap();
    std::fs::write(&file2, "class Beta { }").unwrap();

    let mut index = ContentIndex {
        root: ".".to_string(),
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    };

    // Add file1
    let clean1 = PathBuf::from(crate::clean_path(&file1.to_string_lossy()));
    update_file_in_index(&mut index, &clean1);

    // Add file2
    let clean2 = PathBuf::from(crate::clean_path(&file2.to_string_lossy()));
    update_file_in_index(&mut index, &clean2);

    // Update file1 with new content
    std::fs::write(&file1, "class AlphaUpdated { NewMethod(); }").unwrap();
    update_file_in_index(&mut index, &clean1);

    // Remove file2
    remove_file_from_index(&mut index, &clean2);

    // Verify consistency: total_tokens == sum(file_token_counts) for non-removed files
    let sum: u64 = index.file_token_counts.iter().map(|&c| c as u64).sum();
    assert_eq!(index.total_tokens, sum,
        "total_tokens ({}) should equal sum of file_token_counts ({}) after multiple operations",
        index.total_tokens, sum);
}

#[test]
fn test_watch_index_survives_save_load_roundtrip() {
    // Verify that a ContentIndex with path_to_id (watch-mode field)
    // can be saved to disk and loaded back with all data intact.
    // This is critical for save-on-shutdown: if path_to_id doesn't serialize
    // properly, the loaded index would lose incremental updates.
    let tmp = tempfile::tempdir().unwrap();

    // Build a watch-mode index with path_to_id populated
    let index = make_test_index();
    let watch_index = build_watch_index_from(index);

    // Verify watch fields before save
    assert!(watch_index.path_to_id.is_some(), "path_to_id should be populated");
    let orig_files = watch_index.files.len();
    let orig_tokens = watch_index.index.len();
    let orig_path_to_id_len = watch_index.path_to_id.as_ref().unwrap().len();

    // Save to disk
    crate::save_content_index(&watch_index, tmp.path()).expect("save should succeed");

    // Load from disk
    let exts_str = watch_index.extensions.join(",");
    let loaded = crate::load_content_index(&watch_index.root, &exts_str, tmp.path())
        .expect("load should return Ok with the saved index");

    // Verify all core fields survived
    assert_eq!(loaded.files.len(), orig_files, "files count mismatch");
    assert_eq!(loaded.index.len(), orig_tokens, "token count mismatch");
    assert_eq!(loaded.total_tokens, watch_index.total_tokens, "total_tokens mismatch");

    // path_to_id should survive serialization
    assert!(loaded.path_to_id.is_some(), "path_to_id should survive roundtrip");
    assert_eq!(loaded.path_to_id.as_ref().unwrap().len(), orig_path_to_id_len,
        "path_to_id entry count mismatch after roundtrip");
}

// ─── process_batch tests ───────────────────────────────────────────

/// Helper: create a ContentIndex backed by real files in a temp dir,
/// wrapped in `Arc<RwLock>` for `process_batch`.
///
/// Returns the `TempDir` guard, the **canonical** root `PathBuf`
/// (with `\\?\` stripped + forward slashes), and the index. Tests that
/// compare paths against walker / reconcile output must use the returned
/// `root` (not `tmp.path()`) — on Windows CI runners `%TEMP%` is the 8.3
/// short form (`RUNNER~1`) but the indexer canonicalises to the long form
/// (`runneradmin`).
fn make_batch_test_setup()
    -> (tempfile::TempDir, std::path::PathBuf, Arc<RwLock<ContentIndex>>)
{
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::canonicalize_test_root(tmp.path());

    let file_a = dir.join("a.cs");
    let file_b = dir.join("b.cs");
    std::fs::write(&file_a, "class Alpha { HttpClient client; }").unwrap();
    std::fs::write(&file_b, "class Beta { ILogger logger; }").unwrap();

    let clean_a = crate::clean_path(&file_a.to_string_lossy());
    let clean_b = crate::clean_path(&file_b.to_string_lossy());

    let mut inverted = HashMap::new();
    inverted.insert("alpha".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("client".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    inverted.insert("beta".to_string(), vec![Posting { file_id: 1, lines: vec![1] }]);
    inverted.insert("ilogger".to_string(), vec![Posting { file_id: 1, lines: vec![1] }]);
    inverted.insert("logger".to_string(), vec![Posting { file_id: 1, lines: vec![1] }]);
    inverted.insert("class".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![1] },
    ]);

    let mut index = ContentIndex {
        root: dir.to_string_lossy().to_string(),
        // The sharded persistence path enforces format-version match on load
        // (`load_content_index_at_path` rejects `version=0`). Test fixtures
        // that round-trip through save/load MUST stamp the live constant the
        // same way production builders do.
        format_version: code_xray::CONTENT_INDEX_VERSION,
        files: vec![clean_a.clone(), clean_b.clone()],
        index: inverted,
        total_tokens: 20,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![10, 10],
        path_to_id: Some({
            let mut m = HashMap::new();
            m.insert(PathBuf::from(&clean_a), 0u32);
            m.insert(PathBuf::from(&clean_b), 1u32);
            m
        }),
        ..Default::default()
    };
    index.rebuild_file_tokens();

    (tmp, dir, Arc::new(RwLock::new(index)))
}

#[test]
fn test_process_batch_empty() {
    let (_tmp, _root, index) = make_batch_test_setup();
    let mut dirty = HashSet::new();
    let mut removed = HashSet::new();

    let tokens_before = index.read().unwrap().total_tokens;
    let files_before = index.read().unwrap().files.len();

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    assert_eq!(idx.total_tokens, tokens_before, "empty batch should not change total_tokens");
    assert_eq!(idx.files.len(), files_before, "empty batch should not change files");
}

#[test]
fn test_process_batch_dirty_file() {
    let (_tmp, root, index) = make_batch_test_setup();

    // Modify file a.cs with new content (use canonical root so paths line up
    // with path_to_id keys on Windows CI — see make_batch_test_setup docs).
    let file_a = root.join("a.cs");
    std::fs::write(&file_a, "class AlphaUpdated { NewService service; }").unwrap();

    let mut dirty = HashSet::new();
    dirty.insert(file_a);
    let mut removed = HashSet::new();

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    // Old token "httpclient" should be gone
    assert!(!idx.index.contains_key("httpclient"),
        "old token 'httpclient' should be removed after update");
    // New token "alphaupdated" should be present
    assert!(idx.index.contains_key("alphaupdated"),
        "new token 'alphaupdated' should be present after update");
    // File b should be untouched
    assert!(idx.index.contains_key("beta"),
        "token 'beta' from untouched file should remain");
    // dirty set should be drained
    assert!(dirty.is_empty(), "dirty set should be drained after process_batch");
    // trigram should be marked dirty
    assert!(idx.trigram_dirty, "trigram should be marked dirty after update");
    // created_at should be updated to recent time
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(idx.created_at > 0, "created_at should be updated after batch with changes");
    assert!(idx.created_at <= now, "created_at should not be in the future");
    assert!(now - idx.created_at < 10, "created_at should be within last 10 seconds");
}

#[test]
fn test_process_batch_removed_file() {
    let (_tmp, root, index) = make_batch_test_setup();

    let file_a = root.join("a.cs");

    let mut dirty = HashSet::new();
    let mut removed = HashSet::new();
    removed.insert(file_a);

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    // Tokens exclusive to file a should be gone
    assert!(!idx.index.contains_key("httpclient"),
        "token 'httpclient' from removed file should be gone");
    assert!(!idx.index.contains_key("alpha"),
        "token 'alpha' from removed file should be gone");
    // Tokens from file b should remain
    assert!(idx.index.contains_key("beta"),
        "token 'beta' from untouched file should remain");
    // path_to_id should not contain the removed file
    let clean_a = crate::clean_path(&root.join("a.cs").to_string_lossy());
    assert!(!idx.path_to_id.as_ref().unwrap().contains_key(&PathBuf::from(&clean_a)),
        "removed file should not be in path_to_id");
    // removed set should be drained
    assert!(removed.is_empty(), "removed set should be drained after process_batch");
}

#[test]
fn test_process_batch_mixed_dirty_and_removed() {
    let (_tmp, root, index) = make_batch_test_setup();

    // Remove file a, modify file b (canonical root — see make_batch_test_setup docs).
    let file_a = root.join("a.cs");
    let file_b = root.join("b.cs");
    std::fs::write(&file_b, "class BetaModified { NewToken value; }").unwrap();

    let mut dirty = HashSet::new();
    dirty.insert(file_b);
    let mut removed = HashSet::new();
    removed.insert(file_a);

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    // File a tokens gone
    assert!(!idx.index.contains_key("httpclient"),
        "removed file's token should be gone");
    assert!(!idx.index.contains_key("alpha"),
        "removed file's token should be gone");
    // File b old tokens gone, new tokens present
    assert!(!idx.index.contains_key("ilogger"),
        "old token from modified file should be gone");
    assert!(idx.index.contains_key("betamodified"),
        "new token from modified file should be present");
    assert!(idx.index.contains_key("newtoken"),
        "new token from modified file should be present");
    // Both sets should be drained
    assert!(dirty.is_empty(), "dirty should be drained");
    assert!(removed.is_empty(), "removed should be drained");
}

#[test]
fn test_process_batch_new_file_in_dirty() {
    let (tmp, _root, index) = make_batch_test_setup();

    // Create a brand new file
    let file_c = tmp.path().join("c.cs");
    std::fs::write(&file_c, "class Gamma { UniqueToken gamma; }").unwrap();

    let mut dirty = HashSet::new();
    dirty.insert(file_c);
    let mut removed = HashSet::new();

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    // New tokens should be present
    assert!(idx.index.contains_key("gamma"),
        "new file token 'gamma' should be present");
    assert!(idx.index.contains_key("uniquetoken"),
        "new file token 'uniquetoken' should be present");
    // Old files untouched
    assert!(idx.index.contains_key("alpha"),
        "old token 'alpha' should remain");
    assert!(idx.index.contains_key("beta"),
        "old token 'beta' should remain");
    // New file should be in path_to_id
    let clean_c = crate::clean_path(&tmp.path().join("c.cs").to_string_lossy());
    assert!(idx.path_to_id.as_ref().unwrap().contains_key(&PathBuf::from(&clean_c)),
        "new file should be in path_to_id");
    assert_eq!(idx.files.len(), 3, "should have 3 files after adding new one");
}

#[test]
fn test_process_batch_total_tokens_consistent() {
    let (tmp, _root, index) = make_batch_test_setup();

    // Modify file a
    let file_a = tmp.path().join("a.cs");
    std::fs::write(&file_a, "class X { }").unwrap();

    let mut dirty = HashSet::new();
    dirty.insert(file_a);
    let mut removed = HashSet::new();

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    // Verify total_tokens == sum of file_token_counts
    let sum: u64 = idx.file_token_counts.iter().map(|&c| c as u64).sum();
    assert_eq!(idx.total_tokens, sum,
        "total_tokens ({}) should equal sum of file_token_counts ({})", idx.total_tokens, sum);
}

// ─── shrink_if_oversized tests ──────────────────────────────────────

#[test]
fn test_shrink_if_oversized_no_shrink_needed() {
    // When capacity is close to len, no shrink should occur
    let mut index = make_test_index();
    index.path_to_id = Some(HashMap::new());
    // The index has small data — capacity should be close to len
    let cap_before: usize = index.index.values().map(|v| v.capacity()).sum();
    shrink_if_oversized(&mut index);
    let cap_after: usize = index.index.values().map(|v| v.capacity()).sum();
    // Capacity should not have changed (already tight)
    assert_eq!(cap_before, cap_after, "No shrink needed when capacity ≈ len");
}

#[test]
fn test_shrink_if_oversized_shrinks_posting_vecs() {
    let mut index = ContentIndex {
        root: ".".to_string(),
        files: vec!["f.cs".to_string()],
        index: HashMap::new(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };

    // Create a vec with large excess capacity
    let mut postings = Vec::with_capacity(100);
    postings.push(Posting { file_id: 0, lines: vec![1] });
    // capacity = 100, len = 1 → capacity > 2 * len → should shrink
    assert!(postings.capacity() > postings.len() * 2);
    index.index.insert("token".to_string(), postings);

    shrink_if_oversized(&mut index);

    let postings = index.index.get("token").unwrap();
    // After shrink_to_fit, capacity may still be > len but should be much less than 100
    assert!(postings.capacity() <= postings.len() * 2 || postings.capacity() < 100,
        "Posting vec should have been shrunk, capacity={}, len={}", postings.capacity(), postings.len());
}

#[test]
fn test_shrink_if_oversized_shrinks_main_index() {
    let mut index = ContentIndex {
        root: ".".to_string(),
        files: vec![],
        index: HashMap::with_capacity(1000),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    // Only 1 entry but capacity = 1000
    index.index.insert("token".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    assert!(index.index.capacity() > index.index.len() * 2);

    shrink_if_oversized(&mut index);

    // After shrink, capacity should be much closer to len
    assert!(index.index.capacity() < 1000,
        "Main index HashMap should have been shrunk, capacity={}", index.index.capacity());
}

#[test]
fn test_shrink_if_oversized_shrinks_path_to_id() {
    let mut index = ContentIndex {
        root: ".".to_string(),
        files: vec![],
        index: HashMap::new(),
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::with_capacity(500)),
        ..Default::default()
    };
    index.path_to_id.as_mut().unwrap().insert(PathBuf::from("f.cs"), 0);
    let p2id = index.path_to_id.as_ref().unwrap();
    assert!(p2id.capacity() > p2id.len() * 2);

    shrink_if_oversized(&mut index);

    let p2id = index.path_to_id.as_ref().unwrap();
    assert!(p2id.capacity() < 500,
        "path_to_id should have been shrunk, capacity={}", p2id.capacity());
}

#[test]
fn test_shrink_if_oversized_skips_when_no_path_to_id() {
    let mut index = ContentIndex {
        root: ".".to_string(),
        files: vec![],
        index: HashMap::new(),
        extensions: vec!["cs".to_string()],
        path_to_id: None,
        ..Default::default()
    };
    // Should not panic
    shrink_if_oversized(&mut index);
    assert!(index.path_to_id.is_none());
}


#[test]
fn test_process_batch_returns_false_on_poisoned_content_lock() {
    // Poison the RwLock by panicking inside a write guard
    let index = Arc::new(RwLock::new(ContentIndex {
        root: ".".to_string(),
        files: vec![],
        index: HashMap::new(),
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));

    // Poison the lock
    let index_clone = index.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = index_clone.write().unwrap();
        panic!("intentional panic to poison RwLock");
    }));

    // Verify the lock is actually poisoned
    assert!(index.write().is_err(), "Lock should be poisoned");

    let mut dirty = HashSet::new();
    dirty.insert(PathBuf::from("test.cs"));
    let mut removed = HashSet::new();

    // process_batch should return false on poisoned lock
    let result = process_batch(&index, &None, &mut dirty, &mut removed);
    assert!(!result, "process_batch should return false when content lock is poisoned");
}

#[test]
fn test_process_batch_returns_false_on_poisoned_def_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let test_file = tmp.path().join("test.cs");
    std::fs::write(&test_file, "class Test {}").unwrap();

    let index = Arc::new(RwLock::new(ContentIndex {
        root: tmp.path().to_string_lossy().to_string(),
        files: vec![],
        index: HashMap::new(),
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));

    let def_index = Arc::new(RwLock::new(crate::definitions::DefinitionIndex::default()));

    // Poison the def lock
    let def_clone = def_index.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = def_clone.write().unwrap();
        panic!("intentional panic to poison def RwLock");
    }));

    assert!(def_index.write().is_err(), "Def lock should be poisoned");

    let mut dirty = HashSet::new();
    dirty.insert(test_file);
    let mut removed = HashSet::new();

    let result = process_batch(&index, &Some(def_index), &mut dirty, &mut removed);
    assert!(!result, "process_batch should return false when def lock is poisoned");
}

#[test]
fn test_process_batch_returns_true_on_healthy_locks() {
    let (tmp, _root, index) = make_batch_test_setup();
    let test_file = tmp.path().join("new_healthy.cs");
    std::fs::write(&test_file, "class Healthy { }").unwrap();

    let mut dirty = HashSet::new();
    dirty.insert(test_file);
    let mut removed = HashSet::new();

    let result = process_batch(&index, &None, &mut dirty, &mut removed);
    assert!(result, "process_batch should return true when locks are healthy");
}


// ─── periodic_autosave tests ────────────────────────────────────────

#[test]
fn test_periodic_autosave_saves_both_indexes() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    // Create a content index with real data
    let content_index = Arc::new(RwLock::new(ContentIndex {
        root: tmp.path().to_string_lossy().to_string(),
        files: vec!["file.cs".to_string()],
        index: {
            let mut m = HashMap::new();
            m.insert("token".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m
        },
        total_tokens: 1,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![1],
        ..Default::default()
    }));

    // Create a definition index
    let def_index = Arc::new(RwLock::new(crate::definitions::DefinitionIndex {
        root: tmp.path().to_string_lossy().to_string(),
        extensions: vec!["cs".to_string()],
        files: vec!["file.cs".to_string()],
        definitions: vec![crate::definitions::DefinitionEntry {
            file_id: 0,
            name: "TestClass".to_string(),
            kind: crate::definitions::DefinitionKind::Class,
            line_start: 1,
            line_end: 10,
            parent: None,
            signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        }],
        path_to_id: {
            let mut m = HashMap::new();
            m.insert(PathBuf::from("file.cs"), 0u32);
            m
        },
        ..Default::default()
    }));

    // Call periodic_autosave
    periodic_autosave(&content_index, &Some(def_index), index_base);

    // Verify content index was saved to disk
    let content_path = crate::index::content_index_path_for(
        &tmp.path().to_string_lossy(), "cs", index_base
    );
    assert!(content_path.exists(), "Content index should be saved to disk: {:?}", content_path);

    // Verify definition index was saved to disk
    let def_path = crate::definitions::definition_index_path_for(
        &tmp.path().to_string_lossy(), "cs", index_base
    );
    assert!(def_path.exists(), "Definition index should be saved to disk: {:?}", def_path);
}

// ─── should_invalidate_file_index tests (P0-2 regression) ───────────────────

#[test]
fn test_should_invalidate_file_index_create() {
    use notify::EventKind;
    use notify::event::CreateKind;
    use crate::mcp::watcher::should_invalidate_file_index;
    assert!(should_invalidate_file_index(&EventKind::Create(CreateKind::Any)));
    assert!(should_invalidate_file_index(&EventKind::Create(CreateKind::File)));
}

#[test]
fn test_should_invalidate_file_index_remove() {
    use notify::EventKind;
    use notify::event::RemoveKind;
    use crate::mcp::watcher::should_invalidate_file_index;
    assert!(should_invalidate_file_index(&EventKind::Remove(RemoveKind::Any)));
    assert!(should_invalidate_file_index(&EventKind::Remove(RemoveKind::File)));
}

#[test]
fn test_should_invalidate_file_index_rename_triggers_rebuild() {
    use notify::EventKind;
    use notify::event::{ModifyKind, RenameMode};
    use crate::mcp::watcher::should_invalidate_file_index;
    // Regression test for P0-2: rename events must trigger file index rebuild
    assert!(should_invalidate_file_index(&EventKind::Modify(ModifyKind::Name(RenameMode::Any))));
    assert!(should_invalidate_file_index(&EventKind::Modify(ModifyKind::Name(RenameMode::From))));
    assert!(should_invalidate_file_index(&EventKind::Modify(ModifyKind::Name(RenameMode::To))));
}

#[test]
fn test_should_invalidate_file_index_modify_any() {
    use notify::EventKind;
    use notify::event::ModifyKind;
    use crate::mcp::watcher::should_invalidate_file_index;
    assert!(should_invalidate_file_index(&EventKind::Modify(ModifyKind::Any)));
}

#[test]
fn test_should_invalidate_file_index_data_change_invalidates() {
    use notify::EventKind;
    use notify::event::{ModifyKind, DataChange};
    use crate::mcp::watcher::should_invalidate_file_index;
    // MCP-WCH-001: data-only changes now also invalidate the file-list
    // index (safe over-approximation). Previously only Create/Remove/
    // Modify(Name)/Modify(Any) invalidated, which left newly-created
    // files invisible to xray_fast on Windows/macOS until the periodic
    // rescan ran (notify-rs delivers Modify(Other)/Modify(Metadata)
    // for new files in those backends).
    assert!(should_invalidate_file_index(&EventKind::Modify(ModifyKind::Data(DataChange::Content))));
}

#[test]
fn test_should_invalidate_file_index_modify_other_and_metadata_invalidate() {
    use notify::EventKind;
    use notify::event::{ModifyKind, MetadataKind};
    use crate::mcp::watcher::should_invalidate_file_index;
    // MCP-WCH-001 regression test: Windows/ReadDirectoryChangesW and
    // macOS/FSEvents-degraded routinely deliver Modify(Other) and
    // Modify(Metadata) for newly-created files. Both must invalidate
    // the file-list index or xray_fast will miss the new file.
    assert!(should_invalidate_file_index(&EventKind::Modify(ModifyKind::Other)));
    assert!(should_invalidate_file_index(&EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))));
}

#[test]
fn test_should_invalidate_file_index_access_does_not_invalidate() {
    use notify::EventKind;
    use crate::mcp::watcher::should_invalidate_file_index;
    assert!(!should_invalidate_file_index(&EventKind::Access(notify::event::AccessKind::Any)));
}


#[test]
fn test_periodic_autosave_skips_empty_indexes() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    // Create empty indexes
    let content_index = Arc::new(RwLock::new(ContentIndex::default()));
    let def_index = Arc::new(RwLock::new(crate::definitions::DefinitionIndex::default()));

    // Call periodic_autosave — should not write anything
    periodic_autosave(&content_index, &Some(def_index), index_base);

    // Verify no files written
    let entries: Vec<_> = std::fs::read_dir(index_base)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(".word-search") || name.ends_with(".code-structure")
        })
        .collect();
    assert!(entries.is_empty(), "Empty indexes should not be saved to disk");
}

// Regression: an index whose live_file_count() == 0 but whose allocator
// (`idx.files`) has grown is NOT pristine — it represents a "had files,
// all removed" state. The autosave gate must still checkpoint it so a
// forced kill doesn't resurrect the previous on-disk state next session.
// See user-stories/stale-content-index-files-counter.md (review finding).
#[test]
fn test_periodic_autosave_checkpoints_post_removal_empty_index() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();
    let root = tmp.path().to_string_lossy().to_string();

    // Content index: allocator grew to 1 slot, then file was removed
    // (slot tombstoned, path_to_id empty). live_file_count() == 0
    // but `files.len() == 1`.
    let content_index = Arc::new(RwLock::new(ContentIndex {
        root: root.clone(),
        files: vec![String::new()],
        index: HashMap::new(),
        total_tokens: 0,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![0],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));
    let def_index = Arc::new(RwLock::new(crate::definitions::DefinitionIndex {
        root: root.clone(),
        files: vec![String::new()],
        path_to_id: HashMap::new(),
        ..Default::default()
    }));

    periodic_autosave(&content_index, &Some(def_index), index_base);

    let content_path = crate::index::content_index_path_for(&root, "cs", index_base);
    assert!(
        content_path.exists(),
        "Post-removal-empty content index MUST be checkpointed so it doesn't \
         resurrect on forced-kill restart"
    );
}

#[test]
fn test_periodic_autosave_no_def_index() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let content_index = Arc::new(RwLock::new(ContentIndex {
        root: tmp.path().to_string_lossy().to_string(),
        files: vec!["file.cs".to_string()],
        index: {
            let mut m = HashMap::new();
            m.insert("token".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m
        },
        total_tokens: 1,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![1],
        ..Default::default()
    }));

    // No definition index
    periodic_autosave(&content_index, &None, index_base);

    // Content index should be saved
    let content_path = crate::index::content_index_path_for(
        &tmp.path().to_string_lossy(), "cs", index_base
    );
    assert!(content_path.exists(), "Content index should be saved even without def index");
}


// ─── Tests for non-blocking content index building blocks ───────────────

#[test]
fn test_tokenize_file_standalone_returns_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("test.cs");
    std::fs::write(&file, "class UserService { HttpClient client; }").unwrap();

    let result = tokenize_file_standalone(&file).unwrap();
    assert_eq!(result.path, file);
    assert!(result.total_tokens > 0, "should have tokens");
    assert!(result.tokens.contains_key("userservice"), "should contain 'userservice' token");
    assert!(result.tokens.contains_key("httpclient"), "should contain 'httpclient' token");
    assert!(result.tokens.contains_key("client"), "should contain 'client' token");
}

#[test]
fn test_tokenize_file_standalone_nonexistent_file_returns_none() {
    let result = tokenize_file_standalone(Path::new("/nonexistent/path/file.cs"));
    assert!(result.is_none(), "nonexistent file should return None");
}

#[test]
fn test_tokenize_file_standalone_line_numbers_correct() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("test.cs");
    std::fs::write(&file, "alpha\nbeta\nalpha").unwrap();

    let result = tokenize_file_standalone(&file).unwrap();
    let alpha_lines = result.tokens.get("alpha").unwrap();
    assert_eq!(alpha_lines, &vec![1u32, 3], "alpha should appear on lines 1 and 3");
    let beta_lines = result.tokens.get("beta").unwrap();
    assert_eq!(beta_lines, &vec![2u32], "beta should appear on line 2");
}

#[test]
fn test_apply_tokenized_file_new_file() {
    let mut index = ContentIndex {
        files: vec!["existing.cs".to_string()],
        index: HashMap::new(),
        total_tokens: 5,
        file_token_counts: vec![5],
        path_to_id: Some({
            let mut m = HashMap::new();
            m.insert(PathBuf::from("existing.cs"), 0u32);
            m
        }),
        ..Default::default()
    };

    let result = TokenizedFileResult {
        path: PathBuf::from("new_file.cs"),
        tokens: {
            let mut t = HashMap::new();
            t.insert("hello".to_string(), vec![1u32]);
            t.insert("world".to_string(), vec![1u32, 2]);
            t
        },
        total_tokens: 3,
    };

    apply_tokenized_file(&mut index, result, true);

    assert_eq!(index.files.len(), 2, "should have 2 files");
    assert_eq!(index.files[1], "new_file.cs");
    assert_eq!(index.total_tokens, 5 + 3, "total_tokens should be updated");
    assert_eq!(index.file_token_counts[1], 3);
    assert!(index.index.contains_key("hello"), "should have 'hello' in inverted index");
    assert!(index.index.contains_key("world"), "should have 'world' in inverted index");

    // Verify postings have correct file_id
    let hello_postings = &index.index["hello"];
    assert_eq!(hello_postings[0].file_id, 1);
}

#[test]
fn test_apply_tokenized_file_existing_file() {
    let mut index = ContentIndex {
        files: vec!["file.cs".to_string()],
        index: HashMap::new(),
        total_tokens: 0,
        file_token_counts: vec![0],
        path_to_id: Some({
            let mut m = HashMap::new();
            m.insert(PathBuf::from("file.cs"), 0u32);
            m
        }),
        ..Default::default()
    };

    let result = TokenizedFileResult {
        path: PathBuf::from("file.cs"),
        tokens: {
            let mut t = HashMap::new();
            t.insert("updated".to_string(), vec![1u32]);
            t
        },
        total_tokens: 1,
    };

    apply_tokenized_file(&mut index, result, true);

    assert_eq!(index.files.len(), 1, "should still have 1 file (existing)");
    assert_eq!(index.total_tokens, 1, "total_tokens should be updated");
    assert_eq!(index.file_token_counts[0], 1);
    let postings = &index.index["updated"];
    assert_eq!(postings[0].file_id, 0, "should use existing file_id");
}

#[test]
fn test_apply_tokenized_file_no_path_to_id() {
    let mut index = ContentIndex {
        path_to_id: None,
        ..Default::default()
    };

    let result = TokenizedFileResult {
        path: PathBuf::from("file.cs"),
        tokens: HashMap::new(),
        total_tokens: 0,
    };

    // Should not panic — just return early
    apply_tokenized_file(&mut index, result, true);
    assert_eq!(index.total_tokens, 0);
}

#[test]
fn test_nonblocking_update_content_index_tokens_consistent() {
    let (tmp, _root, index) = make_batch_test_setup();

    // Modify file a
    let file_a = tmp.path().join("a.cs");
    std::fs::write(&file_a, "class NewClass { int value; }").unwrap();

    let mut dirty = HashSet::new();
    dirty.insert(file_a);
    let mut removed = HashSet::new();

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    // Verify total_tokens == sum of file_token_counts (consistency invariant)
    let sum: u64 = idx.file_token_counts.iter().map(|&c| c as u64).sum();
    assert_eq!(idx.total_tokens, sum,
        "total_tokens ({}) should equal sum of file_token_counts ({}) after nonblocking update",
        idx.total_tokens, sum);
}

#[test]
fn test_nonblocking_update_content_index_dirty_tokenize_failure_preserves_old_postings() {
    // Simulate a dirty event for a file that becomes unreadable before the
    // watcher tokenizes it. The safe behavior is stale-but-consistent: keep the
    // old postings/counts, and let a later watcher/rescan event repair it.
    let (_tmp, root, index) = make_batch_test_setup();
    let file_a = root.join("a.cs");
    let clean_a = crate::clean_path(&file_a.to_string_lossy());
    std::fs::remove_file(&file_a).unwrap();

    let before = {
        let idx = index.read().unwrap();
        (idx.created_at, idx.trigram_dirty)
    };

    let mut dirty = HashSet::new();
    dirty.insert(PathBuf::from(&clean_a));
    let mut removed = HashSet::new();

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    assert!(idx.index.contains_key("alpha"), "failed dirty retokenize must leave old postings intact");
    assert!(idx.index["class"].iter().any(|posting| posting.file_id == 0));
    assert_eq!(idx.file_token_counts[0], 10);
    assert_eq!(idx.created_at, before.0, "failed retokenize must not advance retry watermark");
    assert_eq!(idx.trigram_dirty, before.1, "failed retokenize must not mark trigram dirty");
    let sum: u64 = idx.file_token_counts.iter().map(|&count| count as u64).sum();
    assert_eq!(idx.total_tokens, sum);
}

#[test]
fn test_reindex_paths_sync_dirty_tokenize_failure_reports_no_content_update() {
    let (_tmp, root, index) = make_batch_test_setup();
    let file_a = root.join("a.cs");
    let clean_a = crate::clean_path(&file_a.to_string_lossy());
    std::fs::remove_file(&file_a).unwrap();

    let stats = reindex_paths_sync(
        &index,
        &None,
        &[PathBuf::from(&clean_a)],
        &[],
        &["cs".to_string()],
    );

    assert_eq!(stats.content_updated, 0, "failed dirty tokenization is not an applied content update");
    assert!(!stats.content_lock_poisoned);
    let idx = index.read().unwrap();
    assert!(idx.index.contains_key("alpha"));
    assert_eq!(idx.file_token_counts[0], 10);
}

#[test]
fn test_nonblocking_update_content_index_new_file_tokens_consistent() {
    let (tmp, _root, index) = make_batch_test_setup();

    // Add a new file
    let file_c = tmp.path().join("c.cs");
    std::fs::write(&file_c, "class Gamma { string name; }").unwrap();

    let mut dirty = HashSet::new();
    dirty.insert(file_c);
    let mut removed = HashSet::new();

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    let sum: u64 = idx.file_token_counts.iter().map(|&c| c as u64).sum();
    assert_eq!(idx.total_tokens, sum,
        "total_tokens ({}) should equal sum of file_token_counts ({}) after adding new file",
        idx.total_tokens, sum);
    assert_eq!(idx.files.len(), 3, "should have 3 files after adding new file");
}

#[test]
fn test_nonblocking_update_content_index_remove_tokens_consistent() {
    let (tmp, _root, index) = make_batch_test_setup();

    // Remove file a
    let file_a = tmp.path().join("a.cs");
    let clean_a = crate::clean_path(&file_a.to_string_lossy());

    let mut dirty = HashSet::new();
    let mut removed = HashSet::new();
    removed.insert(PathBuf::from(&clean_a));

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    let sum: u64 = idx.file_token_counts.iter().map(|&c| c as u64).sum();
    assert_eq!(idx.total_tokens, sum,
        "total_tokens ({}) should equal sum of file_token_counts ({}) after removal",
        idx.total_tokens, sum);
}

// ─────────────────────────────────────────────────────────────────────
// Tests for `reindex_paths_sync` — synchronous reindexing used by xray_edit
// to eliminate the 500ms watcher debounce race window.
// ─────────────────────────────────────────────────────────────────────

/// Helper: create a minimal ContentIndex with `path_to_id` populated for the
/// given files, so `update_content_index` can find purge targets.
fn make_indexed_content(files: &[(&Path, &str)], extensions: Vec<String>) -> ContentIndex {
    let mut path_to_id = HashMap::new();
    let mut file_strs = Vec::new();
    let mut file_token_counts = Vec::new();
    for (i, (path, _content)) in files.iter().enumerate() {
        let clean = crate::clean_path(&path.to_string_lossy());
        path_to_id.insert(PathBuf::from(&clean), i as u32);
        file_strs.push(clean);
        file_token_counts.push(0);
    }
    ContentIndex {
        root: ".".to_string(),
        files: file_strs,
        index: HashMap::new(),
        total_tokens: 0,
        extensions,
        file_token_counts,
        path_to_id: Some(path_to_id),
        ..Default::default()
    }
}

#[test]
fn test_sync_reindex_existing_file_updates_content() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a.cs");
    std::fs::write(&file, "class OldThingX { OldFieldQ stuff; }").unwrap();

    let index = Arc::new(RwLock::new(make_indexed_content(
        &[(&file, "")],
        vec!["cs".to_string()],
    )));

    // Seed the inverted index with stale tokens (simulating a prior parse).
    {
        let mut idx = index.write().unwrap();
        idx.index.insert("oldthingx".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
        idx.index.insert("oldfieldq".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
        idx.file_token_counts[0] = 2;
        idx.total_tokens = 2;
    }

    // Modify the file on disk and call sync reindex.
    std::fs::write(&file, "class NewThingZ { NewFieldP stuff; }").unwrap();
    let stats = reindex_paths_sync(
        &index,
        &None,
        std::slice::from_ref(&file),
        &[],
        &["cs".to_string()],
    );

    assert_eq!(stats.content_updated, 1, "one file should be content-indexed");
    assert_eq!(stats.def_updated, 0, "no def index → def_updated=0");
    assert_eq!(stats.skipped_filtered, 0, "matching extension → not filtered");
    assert!(!stats.content_lock_poisoned, "lock should not be poisoned");

    let idx = index.read().unwrap();
    assert!(!idx.index.contains_key("oldthingx"), "stale token oldthingx should be purged");
    assert!(!idx.index.contains_key("oldfieldq"), "stale token oldfieldq should be purged");
    assert!(idx.index.contains_key("newthingz"), "new token newthingz should be present");
    assert!(idx.index.contains_key("newfieldp"), "new token newfieldp should be present");
}

#[test]
fn test_sync_reindex_initializes_path_lookup_for_plain_index() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("plain.rs");
    std::fs::write(&file, "fn old_plain_token() {}").unwrap();
    let clean = crate::clean_path(&file.to_string_lossy());

    let mut inverted = HashMap::new();
    inverted.insert("old_plain_token".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
    let index = Arc::new(RwLock::new(ContentIndex {
        root: crate::clean_path(&tmp.path().to_string_lossy()),
        files: vec![clean],
        index: inverted,
        total_tokens: 1,
        extensions: vec!["rs".to_string()],
        file_token_counts: vec![1],
        path_to_id: None,
        file_tokens: Vec::new(),
        ..Default::default()
    }));

    std::fs::write(&file, "fn new_plain_token() {}").unwrap();
    let stats = reindex_paths_sync(
        &index,
        &None,
        std::slice::from_ref(&file),
        &[],
        &["rs".to_string()],
    );

    assert_eq!(stats.content_updated, 1);
    let idx = index.read().unwrap();
    assert!(idx.path_to_id.is_some(), "sync reindex should initialize path lookup");
    assert!(!idx.file_tokens_authoritative, "plain sync reindex should not enable watch reverse-map mode");
    assert!(idx.file_tokens.is_empty(), "plain sync reindex should not build the reverse map");
    assert!(!idx.index.contains_key("old_plain_token"));
    assert!(idx.index.contains_key("new_plain_token"));
    assert_eq!(
        idx.total_tokens,
        idx.file_token_counts.iter().map(|&count| count as u64).sum::<u64>()
    );
    drop(idx);

    std::fs::write(&file, "fn newer_plain_token() {}").unwrap();
    let stats = reindex_paths_sync(
        &index,
        &None,
        std::slice::from_ref(&file),
        &[],
        &["rs".to_string()],
    );

    assert_eq!(stats.content_updated, 1);
    let idx = index.read().unwrap();
    assert!(!idx.file_tokens_authoritative, "repeated plain sync reindex should stay out of watch reverse-map mode");
    assert!(idx.file_tokens.is_empty(), "repeated plain sync reindex should keep using fallback purge");
    assert!(!idx.index.contains_key("new_plain_token"));
    assert!(idx.index.contains_key("newer_plain_token"));
    assert_eq!(
        idx.total_tokens,
        idx.file_token_counts.iter().map(|&count| count as u64).sum::<u64>()
    );
}

#[test]
fn test_sync_reindex_new_file_adds_to_content_index() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("brand_new.cs");
    std::fs::write(&file, "class FreshlyCreatedZ {}").unwrap();

    // Index starts empty (no path_to_id entry for this file).
    let index = Arc::new(RwLock::new(ContentIndex {
        root: ".".to_string(),
        files: Vec::new(),
        index: HashMap::new(),
        total_tokens: 0,
        extensions: vec!["cs".to_string()],
        file_token_counts: Vec::new(),
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));

    let stats = reindex_paths_sync(
        &index,
        &None,
        std::slice::from_ref(&file),
        &[],
        &["cs".to_string()],
    );

    assert_eq!(stats.content_updated, 1);
    assert_eq!(stats.skipped_filtered, 0);
    let idx = index.read().unwrap();
    assert!(idx.index.contains_key("freshlycreatedz"),
        "new file's tokens should be added to the index");
}

#[test]
fn test_sync_reindex_skips_outside_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let txt_file = tmp.path().join("notes.txt");
    std::fs::write(&txt_file, "irrelevant text").unwrap();

    let index = Arc::new(RwLock::new(ContentIndex {
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));

    let stats = reindex_paths_sync(
        &index,
        &None,
        &[txt_file],
        &[],
        &["cs".to_string()],
    );

    assert_eq!(stats.content_updated, 0, "wrong-ext file must NOT be content-indexed");
    assert_eq!(stats.skipped_filtered, 1, "txt file with cs-only filter → 1 skipped");
    let idx = index.read().unwrap();
    assert!(idx.index.is_empty(), "index must remain empty");
}

#[test]
fn test_sync_reindex_skips_git_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let git_dir = tmp.path().join(".git");
    std::fs::create_dir_all(&git_dir).unwrap();
    let inside_git = git_dir.join("config.cs"); // matching ext but inside .git
    std::fs::write(&inside_git, "class GitInternalX {}").unwrap();

    let index = Arc::new(RwLock::new(ContentIndex {
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));

    let stats = reindex_paths_sync(
        &index,
        &None,
        &[inside_git],
        &[],
        &["cs".to_string()],
    );

    assert_eq!(stats.content_updated, 0, ".git/* must NOT be content-indexed");
    assert_eq!(stats.skipped_filtered, 1, "file inside .git/ → 1 skipped");
}

#[test]
fn test_sync_reindex_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("idem.cs");
    std::fs::write(&file, "class IdemContentY {}").unwrap();

    let index = Arc::new(RwLock::new(ContentIndex {
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));

    // First call: populate.
    let stats1 = reindex_paths_sync(&index, &None, std::slice::from_ref(&file), &[], &["cs".to_string()]);
    assert_eq!(stats1.content_updated, 1);
    let (keys1, tokens1, counts1) = {
        let idx = index.read().unwrap();
        let mut keys: Vec<String> = idx.index.keys().cloned().collect();
        keys.sort();
        (keys, idx.total_tokens, idx.file_token_counts.clone())
    };

    // Second call: same file unchanged. Watcher race scenario — sync ran first,
    // then the FS watcher fires for the same change. Index state must be identical.
    let stats2 = reindex_paths_sync(&index, &None, std::slice::from_ref(&file), &[], &["cs".to_string()]);
    assert_eq!(stats2.content_updated, 1);
    let (keys2, tokens2, counts2) = {
        let idx = index.read().unwrap();
        let mut keys: Vec<String> = idx.index.keys().cloned().collect();
        keys.sort();
        (keys, idx.total_tokens, idx.file_token_counts.clone())
    };

    assert_eq!(keys1, keys2, "inverted-index keys must be identical after re-run");
    assert_eq!(tokens1, tokens2, "total_tokens must be identical");
    assert_eq!(counts1, counts2, "file_token_counts must be identical");
}

#[test]
fn test_sync_reindex_no_def_index_works() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("nodef.cs");
    std::fs::write(&file, "class NoDefIndexW {}").unwrap();

    let index = Arc::new(RwLock::new(ContentIndex {
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));

    // def_index = None — must NOT panic, def_updated must be 0.
    let stats = reindex_paths_sync(
        &index,
        &None,
        &[file],
        &[],
        &["cs".to_string()],
    );

    assert_eq!(stats.content_updated, 1);
    assert_eq!(stats.def_updated, 0, "no def_index → def_updated must be 0");
    assert!(!stats.def_lock_poisoned);
}

#[test]
fn test_sync_reindex_with_def_index_updates_both() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("dual.rs");
    std::fs::write(&file, "pub fn def_index_fn_q() {}").unwrap();

    let content = Arc::new(RwLock::new(ContentIndex {
        extensions: vec!["rs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));
    let def = Some(Arc::new(RwLock::new(crate::definitions::DefinitionIndex::default())));

    let stats = reindex_paths_sync(
        &content,
        &def,
        &[file],
        &[],
        &["rs".to_string()],
    );

    assert_eq!(stats.content_updated, 1, "content index updated");
    assert_eq!(stats.def_updated, 1, "def index updated when present");
    assert!(!stats.content_lock_poisoned);
    assert!(!stats.def_lock_poisoned);
}

#[test]
fn test_sync_reindex_empty_input_is_noop() {
    let index = Arc::new(RwLock::new(ContentIndex {
        extensions: vec!["cs".to_string()],
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    }));
    let stats = reindex_paths_sync(&index, &None, &[], &[], &["cs".to_string()]);
    assert_eq!(stats.content_updated, 0);
    assert_eq!(stats.def_updated, 0);
    assert_eq!(stats.skipped_filtered, 0);
    assert!(!stats.content_lock_poisoned);
}

#[test]
fn test_sync_reindex_removed_file_purges_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("doomed.cs");
    std::fs::write(&file, "class DoomedClassP {}").unwrap();

    // Pre-populate index with this file.
    let index = Arc::new(RwLock::new(make_indexed_content(
        &[(&file, "")],
        vec!["cs".to_string()],
    )));
    {
        let mut idx = index.write().unwrap();
        idx.index.insert("doomedclassp".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
        idx.file_token_counts[0] = 1;
        idx.total_tokens = 1;
    }

    // Delete on disk and sync-reindex with the path in `removed`.
    std::fs::remove_file(&file).unwrap();
    let stats = reindex_paths_sync(
        &index,
        &None,
        &[],
        &[file],
        &["cs".to_string()],
    );

    assert_eq!(stats.content_updated, 0, "no dirty files");
    assert_eq!(stats.skipped_filtered, 0);

    let idx = index.read().unwrap();
    assert!(!idx.index.contains_key("doomedclassp"),
        "removed file's tokens must be purged from the inverted index");
}

#[test]
fn test_reindex_stats_subtimings_are_non_negative() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::canonicalize_test_root(tmp.path());
    let file = dir.join("timing.cs");
    std::fs::write(&file, "class Timing { int x; }").unwrap();

    let index = Arc::new(RwLock::new(ContentIndex::default()));
    let stats = reindex_paths_sync(
        &index,
        &None,
        std::slice::from_ref(&file),
        &[],
        &["cs".to_string()],
    );

    assert_eq!(stats.content_updated, 1);
    assert!(stats.elapsed_ms >= 0.0, "elapsed_ms must be non-negative");
    assert!(stats.tokenize_ms >= 0.0, "tokenize_ms must be non-negative: {}", stats.tokenize_ms);
    assert!(stats.content_lock_wait_ms >= 0.0, "content_lock_wait_ms must be non-negative");
    assert!(stats.content_update_ms >= 0.0, "content_update_ms must be non-negative");
    assert!(stats.def_lock_wait_ms >= 0.0, "def_lock_wait_ms must be non-negative");
    assert!(stats.def_update_ms >= 0.0, "def_update_ms must be non-negative");
    // Sub-timings should roughly sum to elapsed (with rounding tolerance)
    let sum = stats.tokenize_ms + stats.content_lock_wait_ms + stats.content_update_ms
        + stats.def_lock_wait_ms + stats.def_update_ms;
    assert!(
        (sum - stats.elapsed_ms).abs() < 1.0,
        "sub-timing sum ({:.2}) should approximately equal elapsed_ms ({:.2})",
        sum, stats.elapsed_ms
    );
}

// ── Regression: `wait_for_indexes_ready` (MAJOR-8) ──────────────────────
//
// Tiny sub-millisecond poll / cap values so these tests run in well
// under 100 ms each. We verify the three branches of the wait-loop:
//   1. Fast path: both ready flags already true → Ready immediately.
//   2. Ready arrives asynchronously from a helper thread.
//   3. Generation change aborts the wait.
//   4. Hard cap elapses with ready flags still false → TimedOut.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn wait_for_ready_returns_immediately_when_both_flags_already_true() {
    let content_ready = AtomicBool::new(true);
    let def_ready = AtomicBool::new(true);
    let generation = AtomicU64::new(0);

    let t0 = Instant::now();
    let outcome = super::wait_for_indexes_ready(
        &content_ready,
        &def_ready,
        &generation,
        0,
        Duration::from_millis(50),
        Duration::from_secs(1),
    );
    let elapsed = t0.elapsed();

    assert_eq!(outcome, super::WaitOutcome::Ready);
    assert!(
        elapsed < Duration::from_millis(20),
        "fast path should not sleep, got {:?}",
        elapsed
    );
}

#[test]
fn wait_for_ready_observes_async_flag_flip() {
    let content_ready = Arc::new(AtomicBool::new(false));
    let def_ready = Arc::new(AtomicBool::new(false));
    let generation = Arc::new(AtomicU64::new(0));

    // Flip both flags after a short delay — simulates a background index
    // builder finishing its final swap while the watcher polls.
    let cr = Arc::clone(&content_ready);
    let dr = Arc::clone(&def_ready);
    let flipper = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        cr.store(true, Ordering::Release);
        dr.store(true, Ordering::Release);
    });

    let outcome = super::wait_for_indexes_ready(
        &content_ready,
        &def_ready,
        &generation,
        0,
        Duration::from_millis(5),
        Duration::from_secs(1),
    );

    flipper.join().unwrap();
    assert_eq!(outcome, super::WaitOutcome::Ready);
}

#[test]
fn wait_for_ready_aborts_on_generation_change() {
    let content_ready = Arc::new(AtomicBool::new(false));
    let def_ready = Arc::new(AtomicBool::new(false));
    let generation = Arc::new(AtomicU64::new(0));

    // Bump generation from another thread; the watcher must notice and
    // return GenerationChanged instead of timing out.
    let g = Arc::clone(&generation);
    let bumper = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        g.store(1, Ordering::Release);
    });

    let outcome = super::wait_for_indexes_ready(
        &content_ready,
        &def_ready,
        &generation,
        0, // my_generation = 0, flipped to 1 by bumper
        Duration::from_millis(5),
        Duration::from_secs(1),
    );

    bumper.join().unwrap();
    assert_eq!(outcome, super::WaitOutcome::GenerationChanged);
}

#[test]
fn wait_for_ready_times_out_when_flags_never_flip() {
    let content_ready = AtomicBool::new(false);
    let def_ready = AtomicBool::new(true); // one true, one false — still incomplete
    let generation = AtomicU64::new(0);

    let t0 = Instant::now();
    let outcome = super::wait_for_indexes_ready(
        &content_ready,
        &def_ready,
        &generation,
        0,
        Duration::from_millis(5),
        Duration::from_millis(40),
    );
    let elapsed = t0.elapsed();

    assert_eq!(outcome, super::WaitOutcome::TimedOut);
    assert!(
        elapsed >= Duration::from_millis(40),
        "should wait at least the cap before timing out, got {:?}",
        elapsed
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "should not wait much longer than the cap, got {:?}",
        elapsed
    );
}

// ─── scan_dir_state ─────────────────────────────────────────────────
//
// Phase 1 of the periodic-rescan rollout: a single shared filesystem
// walk that classifies every regular file into the two views consumed
// by reconciliation (`ext_matched`) and FileIndex (`all_files`).

#[test]
fn scan_dir_state_classifies_ext_matched_subset_of_all_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.cs"), "class A {}").unwrap();
    std::fs::write(dir.join("b.cs"), "class B {}").unwrap();
    std::fs::write(dir.join("readme.md"), "# readme").unwrap();
    std::fs::write(dir.join("data.json"), "{}").unwrap();

    let state = super::scan_dir_state(
        &dir.to_string_lossy(),
        &["cs".to_string()],
        false,
    );

    assert_eq!(state.all_files.len(), 4, "all four regular files must appear in all_files");
    assert_eq!(state.ext_matched.len(), 2, "only .cs files must appear in ext_matched");
    for path in state.ext_matched.keys() {
        assert!(state.all_files.contains_key(path),
            "ext_matched must be a strict subset of all_files (missing: {:?})", path);
    }
}

#[test]
fn scan_dir_state_excludes_hidden_paths_like_content_build() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join(".hidden_dir")).unwrap();
    std::fs::write(dir.join("visible.cs"), "class Visible {}").unwrap();
    std::fs::write(dir.join(".hidden.cs"), "class Hidden {}").unwrap();
    std::fs::write(dir.join(".hidden_dir").join("nested.cs"), "class Nested {}").unwrap();

    let state = super::scan_dir_state(
        &dir.to_string_lossy(),
        &["cs".to_string()],
        false,
    );

    assert_eq!(
        state.ext_matched.len(),
        1,
        "scan_dir_state must match content build hidden-file policy (got {:?})",
        state.ext_matched.keys().collect::<Vec<_>>()
    );
    assert!(state
        .ext_matched
        .keys()
        .all(|p| p.to_string_lossy().ends_with("visible.cs")));
    assert!(
        state.all_files.keys().all(|p| !p.to_string_lossy().contains(".hidden")),
        "hidden files must not appear in all_files either"
    );
}


#[test]
fn scan_dir_state_extension_match_is_case_insensitive() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("upper.CS"), "class U {}").unwrap();
    std::fs::write(dir.join("lower.cs"), "class L {}").unwrap();

    let state = super::scan_dir_state(
        &dir.to_string_lossy(),
        &["cs".to_string()],
        false,
    );

    assert_eq!(state.ext_matched.len(), 2,
        "extension comparison must be case-insensitive (got {:?})",
        state.ext_matched.keys().collect::<Vec<_>>());
}

#[test]
fn scan_dir_state_excludes_dot_git_directory() {
    // The watcher event loop already skips `.git/` (massive event floods on
    // git operations). The shared walker must do the same so reconciliation
    // and the upcoming periodic rescan see the same view as the live event
    // stream — otherwise rescan would re-add `.git/*` files that the watcher
    // never reported as created.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join(".git").join("objects")).unwrap();
    std::fs::write(dir.join(".git").join("config"), "[core]").unwrap();
    std::fs::write(dir.join(".git").join("objects").join("blob"), "x").unwrap();
    std::fs::write(dir.join("real.cs"), "class R {}").unwrap();

    let state = super::scan_dir_state(
        &dir.to_string_lossy(),
        &["cs".to_string(), "config".to_string()],
        false,
    );

    assert!(state.all_files.iter().all(|(p, _)| !p.to_string_lossy().contains("/.git/")),
        ".git/* must NOT appear in all_files (got {:?})",
        state.all_files.keys().collect::<Vec<_>>());
    assert!(state.ext_matched.iter().all(|(p, _)| !p.to_string_lossy().contains("/.git/")),
        ".git/* must NOT appear in ext_matched even when extension matches");
    assert_eq!(state.ext_matched.len(), 1, "only real.cs survives");
}

#[test]
fn scan_dir_state_recurses_into_subdirectories() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("nested").join("deep")).unwrap();
    std::fs::write(dir.join("top.cs"), "class T {}").unwrap();
    std::fs::write(dir.join("nested").join("mid.cs"), "class M {}").unwrap();
    std::fs::write(dir.join("nested").join("deep").join("low.cs"), "class L {}").unwrap();

    let state = super::scan_dir_state(
        &dir.to_string_lossy(),
        &["cs".to_string()],
        false,
    );

    assert_eq!(state.ext_matched.len(), 3,
        "walker must recurse (got {:?})",
        state.ext_matched.keys().collect::<Vec<_>>());
}

#[test]
fn scan_dir_state_path_keys_are_clean_path_normalised() {
    // path_to_id is keyed on `clean_path`-normalised PathBufs, so DirState
    // must use the same normalisation or set-difference comparisons in
    // `reconcile_content_index` / `periodic_rescan_once` will spuriously
    // report drift for paths that differ only in separator style.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("file.cs"), "class F {}").unwrap();

    let state = super::scan_dir_state(
        &dir.to_string_lossy(),
        &["cs".to_string()],
        false,
    );

    for path in state.all_files.keys() {
        let s = path.to_string_lossy();
        assert!(!s.contains('\\'),
            "scan_dir_state must return clean_path-normalised forward-slash paths, got {:?}", s);
    }
}

// ─── periodic_rescan_once ────────────────────────────────────────────
//
// Phase 2 of the periodic-rescan rollout: drive content + def + file
// reconciliation off a single shared filesystem snapshot. The four
// tests below cover the acceptance contract from
// `docs/todo_approved_2026-04-21_watcher-periodic-rescan.md`:
//   1. no-op (no drift, counter not bumped)
//   2. file added directly to disk → drift detected, counter bumped,
//      content index updated
//   3. file removed directly from disk → drift detected, counter bumped,
//      content index updated
//   4. file_index drift sets file_index_dirty regardless of content state

#[test]
fn periodic_rescan_no_drift_does_not_bump_counter() {
    let (_tmp, root, index) = make_batch_test_setup();
    // Mark index as fresh so the mtime threshold treats existing files
    // as up-to-date (compute_content_drift uses created_at-2s margin).
    {
        let mut idx = index.write().unwrap();
        idx.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 5;
    }
    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    // Pre-populate file_index with the two existing files so file-index
    // drift check is also clean.
    let file_index = Arc::new(RwLock::new(Some(crate::FileIndex {
        root: root.to_string_lossy().to_string(),
        format_version: crate::FILE_INDEX_VERSION,
        created_at: 0,
        max_age_secs: 86400,
        entries: vec![
            crate::FileEntry { path: "a.cs".to_string(), size: 0, modified: 0, is_dir: false },
            crate::FileEntry { path: "b.cs".to_string(), size: 0, modified: 0, is_dir: false },
        ],
        respect_git_exclude: false,
    })));

    let autosave_dirty = Arc::new(AtomicBool::new(false));
    let outcome = super::periodic_rescan_once(
        &index, &None, &file_index, &file_index_dirty,
        &root.to_string_lossy(),
        &["cs".to_string()],
        &["cs".to_string()],
        &stats,
        false,
        &autosave_dirty,
    );

    assert!(!outcome.drift_detected, "no drift expected, got {:?}", outcome);
    assert_eq!(stats.periodic_rescan_total.load(Ordering::Relaxed), 1);
    assert_eq!(stats.periodic_rescan_drift_events.load(Ordering::Relaxed), 0,
        "drift counter must NOT bump when state is clean");
    assert!(!file_index_dirty.load(Ordering::Relaxed),
        "file_index_dirty must stay false on clean rescan");
    assert!(!autosave_dirty.load(Ordering::Relaxed),
        "autosave_dirty must stay false when no content drift detected");
}

#[test]
fn periodic_rescan_detects_added_file_and_reconciles_content() {
    let (_tmp, root, index) = make_batch_test_setup();
    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    let file_index = Arc::new(RwLock::new(None)); // not built yet

    // Simulate the bug: write a file directly to disk, BYPASSING the
    // notify event stream entirely (no watcher running here).
    let new_file = root.join("c_added.cs");
    std::fs::write(&new_file, "class Gamma { Logger log; }").unwrap();

    let autosave_dirty = Arc::new(AtomicBool::new(false));
    let outcome = super::periodic_rescan_once(
        &index, &None, &file_index, &file_index_dirty,
        &root.to_string_lossy(),
        &["cs".to_string()],
        &["cs".to_string()],
        &stats,
        false,
        &autosave_dirty,
    );

    assert!(outcome.drift_detected, "added file must trigger drift");
    assert_eq!(outcome.content_added, 1,
        "exactly one new file should be flagged, got outcome={:?}", outcome);
    assert_eq!(stats.periodic_rescan_drift_events.load(Ordering::Relaxed), 1,
        "drift counter must bump once for the recovered event");
    assert!(autosave_dirty.load(Ordering::Relaxed),
        "autosave_dirty must be set when content drift triggers reconcile");

    // Verify the reconciler actually inserted the file into path_to_id.
    let clean_new = crate::clean_path(&new_file.to_string_lossy());
    let idx = index.read().unwrap();
    assert!(
        idx.path_to_id.as_ref().unwrap().contains_key(&PathBuf::from(&clean_new)),
        "reconcile_content_index must have indexed the new file"
    );
}

#[test]
fn periodic_rescan_uses_definition_extensions_subset() {
    let (_tmp, root, index) = make_batch_test_setup();
    std::fs::write(root.join("data.json"), "{\"name\":\"gamma\"}").unwrap();

    let mut def_index = crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 0,
        respect_git_exclude: false,
    });
    let future_created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    def_index.created_at = future_created_at;
    let def_index = Arc::new(RwLock::new(def_index));

    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    let file_index = Arc::new(RwLock::new(None));
    let autosave_dirty = Arc::new(AtomicBool::new(false));

    let outcome = super::periodic_rescan_once(
        &index,
        &Some(Arc::clone(&def_index)),
        &file_index,
        &file_index_dirty,
        &root.to_string_lossy(),
        &["cs".to_string(), "json".to_string()],
        &["cs".to_string()],
        &stats,
        false,
        &autosave_dirty,
    );

    assert_eq!(outcome.content_added, 1, "content reconcile should see data.json");
    let def_guard = def_index.read().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        def_guard.created_at,
        future_created_at,
        "definition reconcile must not treat content-only extensions as definition changes"
    );
    assert!(
        def_guard
            .path_to_id
            .keys()
            .all(|p| p.extension().and_then(|e| e.to_str()) == Some("cs")),
        "definition index must only track .cs files, got {:?}",
        def_guard.path_to_id.keys().collect::<Vec<_>>()
    );
}


#[test]
fn periodic_rescan_detected_dirty_tokenize_failure_does_not_set_autosave_dirty() {
    let (_tmp, root, index) = make_batch_test_setup();
    let file_a = root.join("a.cs");
    let file_b = root.join("b.cs");
    let clean_a = crate::clean_path(&file_a.to_string_lossy());
    let clean_b = crate::clean_path(&file_b.to_string_lossy());

    // Reduce the fixture to a single indexed file so the rescan has exactly one
    // content drift: a modified a.cs. That keeps the assertion sharp; no added
    // or removed file should be able to set autosave_dirty on its own.
    std::fs::remove_file(&file_b).unwrap();
    {
        let mut idx = index.write().unwrap();
        idx.files.truncate(1);
        idx.file_token_counts.truncate(1);
        idx.total_tokens = idx.file_token_counts.iter().map(|&count| count as u64).sum();
        if let Some(ref mut p2id) = idx.path_to_id {
            p2id.remove(&PathBuf::from(&clean_b));
        }
        for postings in idx.index.values_mut() {
            postings.retain(|posting| posting.file_id == 0);
        }
        idx.index.retain(|_, postings| !postings.is_empty());
        idx.rebuild_file_tokens();
        idx.created_at = 0;
        idx.trigram_dirty = false;
    }

    // Make a.cs too large to index. scan_dir_state still sees a real modified
    // .cs file, but tokenize_file_standalone returns None via read_file_lossy's
    // MAX_INDEX_FILE_BYTES guard. This models the reviewer-found path where
    // drift is detected but reconcile applies zero content changes.
    let oversized = std::fs::OpenOptions::new().write(true).open(&file_a).unwrap();
    oversized.set_len(crate::MAX_INDEX_FILE_BYTES + 1).unwrap();
    drop(oversized);

    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    let file_index = Arc::new(RwLock::new(None));
    let autosave_dirty = Arc::new(AtomicBool::new(false));
    let outcome = super::periodic_rescan_once(
        &index, &None, &file_index, &file_index_dirty,
        &root.to_string_lossy(),
        &["cs".to_string()],
        &["cs".to_string()],
        &stats,
        false,
        &autosave_dirty,
    );

    assert!(outcome.drift_detected, "oversized dirty file must still be detected as drift");
    assert_eq!(outcome.content_added, 0);
    assert_eq!(outcome.content_removed, 0);
    assert_eq!(outcome.content_modified, 1);
    assert_eq!(stats.periodic_rescan_drift_events.load(Ordering::Relaxed), 1);
    assert!(file_index_dirty.load(Ordering::Relaxed),
        "file-list invalidation is still detection-based so xray_fast can rebuild");
    assert!(!autosave_dirty.load(Ordering::Relaxed),
        "failed content apply must not checkpoint the stale content snapshot");

    let idx = index.read().unwrap();
    assert!(idx.path_to_id.as_ref().unwrap().contains_key(&PathBuf::from(&clean_a)));
    assert!(idx.index.contains_key("alpha"), "old postings stay searchable until a later successful retry");
    assert_eq!(idx.file_token_counts[0], 10);
    assert_eq!(idx.created_at, 0, "failed apply must leave the retry watermark untouched");
    assert!(!idx.trigram_dirty, "failed apply must not mark derived trigram state dirty");
}

#[test]
fn periodic_rescan_definition_parse_failure_sets_autosave_dirty() {
    let (_tmp, root, index) = make_batch_test_setup();
    let file_a = root.join("a.cs");
    let file_b = root.join("b.cs");
    let clean_a = crate::clean_path(&file_a.to_string_lossy());
    let clean_b = crate::clean_path(&file_b.to_string_lossy());

    // Isolate one dirty file exactly like the content-only regression above.
    // Content reconcile will apply zero changes, while definition reconcile will
    // remove stale definitions for the same file after parsing fails.
    std::fs::remove_file(&file_b).unwrap();
    {
        let mut idx = index.write().unwrap();
        idx.files.truncate(1);
        idx.file_token_counts.truncate(1);
        idx.total_tokens = idx.file_token_counts.iter().map(|&count| count as u64).sum();
        if let Some(ref mut p2id) = idx.path_to_id {
            p2id.remove(&PathBuf::from(&clean_b));
        }
        for postings in idx.index.values_mut() {
            postings.retain(|posting| posting.file_id == 0);
        }
        idx.index.retain(|_, postings| !postings.is_empty());
        idx.rebuild_file_tokens();
        idx.created_at = 0;
        idx.trigram_dirty = false;
    }

    let mut name_index = HashMap::new();
    name_index.insert("alpha".to_string(), vec![0]);
    let mut kind_index = HashMap::new();
    kind_index.insert(crate::definitions::DefinitionKind::Class, vec![0]);
    let mut file_def_index = HashMap::new();
    file_def_index.insert(0u32, vec![0]);
    let mut path_to_id = HashMap::new();
    path_to_id.insert(PathBuf::from(&clean_a), 0u32);
    let def_index = Arc::new(RwLock::new(crate::definitions::DefinitionIndex {
        root: root.to_string_lossy().to_string(),
        created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![clean_a.clone()],
        definitions: vec![crate::definitions::DefinitionEntry {
            file_id: 0,
            name: "Alpha".to_string(),
            kind: crate::definitions::DefinitionKind::Class,
            line_start: 1,
            line_end: 1,
            parent: None,
            signature: None,
            modifiers: vec![],
            attributes: vec![],
            base_types: vec![],
        }],
        name_index,
        kind_index,
        file_index: file_def_index,
        path_to_id,
        ..Default::default()
    }));

    let oversized = std::fs::OpenOptions::new().write(true).open(&file_a).unwrap();
    oversized.set_len(crate::MAX_INDEX_FILE_BYTES + 1).unwrap();
    drop(oversized);

    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    let file_index = Arc::new(RwLock::new(None));
    let autosave_dirty = Arc::new(AtomicBool::new(false));
    let outcome = super::periodic_rescan_once(
        &index, &Some(Arc::clone(&def_index)), &file_index, &file_index_dirty,
        &root.to_string_lossy(),
        &["cs".to_string()],
        &["cs".to_string()],
        &stats,
        false,
        &autosave_dirty,
    );

    assert!(outcome.drift_detected);
    assert_eq!(outcome.content_modified, 1);
    assert!(autosave_dirty.load(Ordering::Relaxed),
        "definition parse failure removes stale defs and must be checkpointed");

    let idx = index.read().unwrap();
    assert!(idx.index.contains_key("alpha"),
        "content index keeps old postings because content tokenization failed");
    assert_eq!(idx.created_at, 0,
        "content watermark must remain retryable even though def index changed");
    drop(idx);

    let def_idx = def_index.read().unwrap();
    assert!(!def_idx.file_index.contains_key(&0),
        "definition reconcile removes stale definitions when dirty parse fails");
    assert!(!def_idx.name_index.contains_key("alpha"));
    assert!(def_idx.path_to_id.contains_key(&PathBuf::from(&clean_a)),
        "dirty parse failure is not a deletion; path mapping stays for retry");
    assert!(def_idx.created_at > 0,
        "definition reconcile advances its own watermark and needs autosave");
}


#[test]
fn periodic_rescan_detects_removed_file_and_reconciles_content() {
    let (_tmp, root, index) = make_batch_test_setup();
    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    let file_index = Arc::new(RwLock::new(None));

    // Delete b.cs directly from disk.
    std::fs::remove_file(root.join("b.cs")).unwrap();

    let autosave_dirty = Arc::new(AtomicBool::new(false));
    let outcome = super::periodic_rescan_once(
        &index, &None, &file_index, &file_index_dirty,
        &root.to_string_lossy(),
        &["cs".to_string()],
        &["cs".to_string()],
        &stats,
        false,
        &autosave_dirty,
    );

    assert!(outcome.drift_detected);
    assert_eq!(outcome.content_removed, 1);
    assert_eq!(stats.periodic_rescan_drift_events.load(Ordering::Relaxed), 1);

    let clean_b = crate::clean_path(&root.join("b.cs").to_string_lossy());
    let idx = index.read().unwrap();
    assert!(
        !idx.path_to_id.as_ref().unwrap().contains_key(&PathBuf::from(&clean_b)),
        "reconcile_content_index must have purged the deleted file"
    );
}

#[test]
fn periodic_rescan_file_index_drift_sets_dirty_flag() {
    // FileIndex tracks ALL files (incl. extensions outside --ext).
    // A new .json file added directly to disk must set
    // file_index_dirty even though it doesn't touch the content index.
    let (_tmp, root, index) = make_batch_test_setup();
    {
        let mut idx = index.write().unwrap();
        idx.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 5;
    }
    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    let file_index = Arc::new(RwLock::new(Some(crate::FileIndex {
        root: root.to_string_lossy().to_string(),
        format_version: crate::FILE_INDEX_VERSION,
        created_at: 0,
        max_age_secs: 86400,
        entries: vec![
            crate::FileEntry { path: "a.cs".to_string(), size: 0, modified: 0, is_dir: false },
            crate::FileEntry { path: "b.cs".to_string(), size: 0, modified: 0, is_dir: false },
        ],
        respect_git_exclude: false,
    })));

    std::fs::write(root.join("config.json"), "{}").unwrap();

    let autosave_dirty = Arc::new(AtomicBool::new(false));
    let outcome = super::periodic_rescan_once(
        &index, &None, &file_index, &file_index_dirty,
        &root.to_string_lossy(),
        &["cs".to_string()], // .json is OUTSIDE --ext on purpose
        &["cs".to_string()],
        &stats,
        false,
        &autosave_dirty,
    );

    assert!(outcome.drift_detected, "file-list drift must be detected");
    assert_eq!(outcome.content_added, 0, ".json must NOT touch content index");
    assert_eq!(outcome.file_index_added, 1);
    assert!(file_index_dirty.load(Ordering::Relaxed),
        "file_index_dirty must be set so the next xray_fast rebuilds");
}

// ─── start_periodic_rescan thread ────────────────────────────────────
//
// Phase 3 of the periodic-rescan rollout: thread that ticks
// `periodic_rescan_once` on a configurable interval. Tests cover the
// shutdown and clamping contracts.

#[test]
fn periodic_rescan_min_interval_is_ten_seconds() {
    // Guard against accidentally setting the floor too low and
    // schedule-walking a large workspace every second.
    assert_eq!(super::MIN_RESCAN_INTERVAL_SEC, 10);
}

#[test]
fn start_periodic_rescan_runs_at_least_one_tick_and_exits_on_generation_change() {
    let (tmp, _root, index) = make_batch_test_setup();
    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    let file_index = Arc::new(RwLock::new(None));
    let generation = Arc::new(AtomicU64::new(0));

    // Use the minimum interval so the test takes ≤ ~12 s instead of 5 min.
    let autosave_dirty = Arc::new(AtomicBool::new(false));
    super::start_periodic_rescan(
        Arc::clone(&index),
        None,
        Arc::clone(&file_index),
        Arc::clone(&file_index_dirty),
        tmp.path().to_path_buf(),
        vec!["cs".to_string()],
        vec!["cs".to_string()],
        super::MIN_RESCAN_INTERVAL_SEC,
        Arc::clone(&generation),
        0,
        Arc::clone(&stats),
        false,
        autosave_dirty,
    );

    // Wait for the first tick (interval + small slack for thread scheduling).
    let deadline = Instant::now()
        + Duration::from_secs(super::MIN_RESCAN_INTERVAL_SEC + 5);
    while Instant::now() < deadline
        && stats.periodic_rescan_total.load(Ordering::Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        stats.periodic_rescan_total.load(Ordering::Relaxed) >= 1,
        "rescan thread must tick at least once within {} s",
        super::MIN_RESCAN_INTERVAL_SEC + 5
    );

    // Bump generation — thread must exit within ~RESCAN_SHUTDOWN_POLL.
    generation.fetch_add(1, Ordering::Release);
    let exit_deadline = Instant::now() + Duration::from_secs(3);
    let ticks_at_signal = stats.periodic_rescan_total.load(Ordering::Relaxed);
    // Sleep one additional interval-floor — if the thread is still alive
    // it would tick again; if it exited cleanly the counter stays put.
    std::thread::sleep(Duration::from_secs(super::MIN_RESCAN_INTERVAL_SEC + 2));
    assert!(
        Instant::now() < exit_deadline.checked_add(Duration::from_secs(super::MIN_RESCAN_INTERVAL_SEC + 5)).unwrap(),
        "test shouldn't have wandered past its own deadline"
    );
    let ticks_after_wait = stats.periodic_rescan_total.load(Ordering::Relaxed);
    assert_eq!(
        ticks_after_wait, ticks_at_signal,
        "thread must stop ticking after generation change (ticks: {} → {})",
        ticks_at_signal, ticks_after_wait
    );
}

// ─── Phase 4: integration — recover from a "lost" notify event ───────
//
// These tests validate the full Phase 3 thread end-to-end: the
// rescan thread detects on-disk changes that the notify event stream
// would have delivered (in a real run) and reconciles the indexes.
// We simulate a perfectly-lost event by NOT spawning `start_watcher`
// at all — only `start_periodic_rescan`. If the periodic path catches
// the file, the architecture is sound regardless of how flaky the
// underlying notify backend is on a given platform.
//
// Each test waits ~MIN_RESCAN_INTERVAL_SEC for one tick (10 s today).
// Slow but unavoidable — the floor exists to prevent self-DoS in
// production. Marked `#[ignore]` would defeat the purpose of an
// integration check on the bug we're fixing, so they run on every
// `cargo test`.

#[test]
fn periodic_rescan_thread_recovers_lost_create_event() {
    // Acceptance #4 of docs/todo_approved_2026-04-21_watcher-periodic-rescan.md:
    // simulate a fully-lost notify event by spawning ONLY the periodic
    // rescan thread — no watcher. A new file written directly to disk
    // must appear in both ContentIndex and FileIndex within one tick.
    let (_tmp, root, index) = make_batch_test_setup();
    let stats = Arc::new(super::WatcherStats::new());
    let file_index_dirty = Arc::new(AtomicBool::new(false));
    let file_index = Arc::new(RwLock::new(None));
    let generation = Arc::new(AtomicU64::new(0));

    let autosave_dirty = Arc::new(AtomicBool::new(false));
    super::start_periodic_rescan(
        Arc::clone(&index),
        None,
        Arc::clone(&file_index),
        Arc::clone(&file_index_dirty),
        root.clone(),
        vec!["cs".to_string()],
        vec!["cs".to_string()],
        super::MIN_RESCAN_INTERVAL_SEC,
        Arc::clone(&generation),
        0,
        Arc::clone(&stats),
        false,
        autosave_dirty,
    );

    // Drop a new .cs file directly on disk *without* notifying any
    // watcher — this is the bug scenario from
    // docs/bug-reports/bug-2026-04-21-watcher-misses-new-files-both-indexes.md.
    let new_file = root.join("gamma.cs");
    std::fs::write(&new_file, "class Gamma { Logger log; }").unwrap();
    let clean_new = crate::clean_path(&new_file.to_string_lossy());

    // Wait for the first tick + a small margin for thread scheduling
    // and the reconcile walk on a 3-file tree.
    let deadline = Instant::now()
        + Duration::from_secs(super::MIN_RESCAN_INTERVAL_SEC + 10);
    while Instant::now() < deadline
        && stats.periodic_rescan_drift_events.load(Ordering::Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(200));
    }

    // Stop the thread before assertions so a failure doesn't leak it
    // across the rest of the suite.
    generation.fetch_add(1, Ordering::Release);

    assert!(
        stats.periodic_rescan_drift_events.load(Ordering::Relaxed) >= 1,
        "rescan thread must register drift after a lost create event \
         (drift_events={}, total_ticks={})",
        stats.periodic_rescan_drift_events.load(Ordering::Relaxed),
        stats.periodic_rescan_total.load(Ordering::Relaxed)
    );

    let idx = index.read().unwrap();
    assert!(
        idx.files.iter().any(|f| f == &clean_new),
        "ContentIndex.files must contain reconciled new file {} \
         (got {} files: {:?})",
        clean_new, idx.files.len(), idx.files
    );
    drop(idx);

    assert!(
        file_index_dirty.load(Ordering::Relaxed),
        "file_index_dirty must be set so the next xray_fast rebuilds"
    );
}


// ─── record_watcher_event_error ────────────────────────────
//
// P2 follow-up from docs/code-reviews/2026-04-21_audit-3day-hidden-bugs.md:
// the `Ok(Err(e))` arm of the watcher loop was previously untested, so a
// 4-hour regression where `events_errors.fetch_add` was wired to the
// wrong counter shipped without any test catching it. Extracting the
// arm into `record_watcher_event_error` gives us a direct unit test.

#[test]
fn record_watcher_event_error_bumps_events_errors_and_nothing_else() {
    let stats = super::WatcherStats::new();
    let err = notify::Error::generic("simulated backend failure");

    super::record_watcher_event_error(&stats, &err);

    assert_eq!(stats.events_errors.load(Ordering::Relaxed), 1,
        "events_errors must bump on a notify backend error");
    // Guard against regressions that route the error into the wrong
    // counter (the exact class of bug that slipped through for 4 hours).
    assert_eq!(stats.events_total.load(Ordering::Relaxed), 0);
    assert_eq!(stats.events_empty_paths.load(Ordering::Relaxed), 0);
    assert_eq!(stats.periodic_rescan_total.load(Ordering::Relaxed), 0);
    assert_eq!(stats.periodic_rescan_drift_events.load(Ordering::Relaxed), 0);

    // Second error must accumulate (not clobber).
    super::record_watcher_event_error(&stats, &err);
    assert_eq!(stats.events_errors.load(Ordering::Relaxed), 2);
}

// ─── Stale `files` counter regression tests ─────────────────────────
//
// Regression coverage for `user-stories/stale-content-index-files-counter.md`:
// `idx.files` is a file_id allocator (append-only, never shrinks). On removal
// we tombstone the slot (clear the String); the live count is `path_to_id.len()`
// surfaced via `live_file_count()`. These tests pin the contract end-to-end so
// `xray_info`, `IndexMeta`, and memory estimates can no longer drift up after
// reconciliation removes files.

#[test]
fn live_file_count_matches_path_to_id_after_removal() {
    let (_tmp, root, index) = make_batch_test_setup();
    let file_a = root.join("a.cs");
    let clean_a = crate::clean_path(&file_a.to_string_lossy());

    let mut dirty = HashSet::new();
    let mut removed = HashSet::new();
    removed.insert(PathBuf::from(&clean_a));

    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    assert_eq!(idx.live_file_count(), 1,
        "after removing 1 of 2 files, live_file_count must report 1, not the Vec capacity 2");
    assert_eq!(idx.files.len(), 2,
        "files Vec is append-only — capacity must stay at 2 (file_id stability)");
    assert!(idx.files[0].is_empty(),
        "removed file's slot must be tombstoned (empty string), got {:?}", idx.files[0]);
    assert!(!idx.files[1].is_empty(),
        "surviving file's slot must NOT be tombstoned");
    assert_eq!(idx.path_to_id.as_ref().unwrap().len(), 1,
        "path_to_id must reflect the live set");
}

#[test]
fn live_file_count_falls_back_to_files_when_no_path_to_id() {
    // Cold CLI build (no --watch) has path_to_id = None and no removals can
    // occur. live_file_count must fall back to files.len() (filtering empties).
    let mut idx = ContentIndex {
        root: ".".to_string(),
        files: vec!["a.cs".to_string(), "b.cs".to_string()],
        path_to_id: None,
        ..Default::default()
    };
    assert_eq!(idx.live_file_count(), 2,
        "no path_to_id → live count = non-empty files");

    // Defensive: legacy on-disk index containing tombstoned slots from an
    // old session must still report the correct live count after a cold load.
    idx.files.push(String::new());
    assert_eq!(idx.live_file_count(), 2,
        "empty tombstone slot must NOT be counted");
}

#[test]
fn build_watch_index_skips_tombstoned_slots() {
    // A legacy index loaded with --watch must not resurrect tombstoned files
    // into path_to_id (would create a junk PathBuf("") entry).
    let mut idx = ContentIndex {
        root: ".".to_string(),
        files: vec!["live.cs".to_string(), String::new(), "other.cs".to_string()],
        ..Default::default()
    };
    // Mimic the persisted state: trigram empty, path_to_id None on disk.
    idx.path_to_id = None;

    let watched = build_watch_index_from(idx);
    let p2id = watched.path_to_id.as_ref().expect("path_to_id must be Some");

    assert_eq!(p2id.len(), 2, "tombstone slot must NOT be inserted");
    assert!(p2id.contains_key(&PathBuf::from("live.cs")));
    assert!(p2id.contains_key(&PathBuf::from("other.cs")));
    assert!(!p2id.contains_key(&PathBuf::from("")),
        "empty PathBuf must never appear in path_to_id (regression: would loop forever in reconcile)");
    // file_id stability is preserved — "other.cs" must keep id=2, not be remapped to 1.
    assert_eq!(p2id[&PathBuf::from("other.cs")], 2);
}

#[test]
fn content_index_meta_uses_live_count_after_removal() {
    let (_tmp, root, index) = make_batch_test_setup();
    let file_a = root.join("a.cs");
    let clean_a = crate::clean_path(&file_a.to_string_lossy());

    let mut dirty = HashSet::new();
    let mut removed = HashSet::new();
    removed.insert(PathBuf::from(&clean_a));
    process_batch(&index, &None, &mut dirty, &mut removed);

    let idx = index.read().unwrap();
    let meta = crate::index::content_index_meta(&idx);
    assert_eq!(meta.files, 1,
        "IndexMeta.files MUST be the live count, not the Vec capacity. \
         This is the cross-session ratchet bug: meta is what later sessions \
         echo back via `serve: content loaded (files=N)` log.");
}


// ─── MCP-WCH-006: post-reconcile checkpoint ──────────────────────────────
//
// These tests cover both the pure decision helper
// `post_reconcile_checkpoint_needed` and the durability sequence it gates:
// after `start_watcher` runs `reconcile_content_index`, any add/remove
// activity must reach the on-disk `.meta` immediately — not only after the
// next 10-minute `periodic_autosave` tick. See
// `user-stories/meta-checkpoint-durability.md` (Hole #1).
//
// Two reviewer-caught regressions during PR review (commit-reviewer
// 2026-04-25), both fixed in this same change set:
//   HIGH (first pass): gate on add/remove COUNTS, not net live-count
//     delta — offline replace (delete A.cs + add B.cs) keeps live count
//     constant but mutates index.
//   MEDIUM (second pass): include MODIFIED in the gate — modify-only
//     reconcile rewrites postings in-place and would otherwise let
//     session C serve stale search results after a force-kill.

#[test]
fn test_post_reconcile_checkpoint_needed_triggers_on_content_add() {
    assert!(post_reconcile_checkpoint_needed(1, 0, 0, false),
        "add-only delta must trigger save");
    assert!(post_reconcile_checkpoint_needed(7, 0, 0, false),
        "multi-add must trigger save");
}

#[test]
fn test_post_reconcile_checkpoint_needed_triggers_on_content_removal() {
    assert!(post_reconcile_checkpoint_needed(0, 0, 1, false),
        "remove-only delta must trigger save (the original ratchet scenario)");
    assert!(post_reconcile_checkpoint_needed(0, 0, 5, false),
        "multi-remove must trigger save");
}

#[test]
fn test_post_reconcile_checkpoint_needed_triggers_on_content_modify() {
    // Reviewer-caught MEDIUM (commit-reviewer 2026-04-25, second pass):
    // modify-only reconcile rewrites postings in-place. Without this case
    // in the gate, force-kill of session B → session C loads stale postings
    // and serves outdated search results for the modified file. The bug
    // is silent (no counter mismatch in xray_info) but visible to xray_grep.
    assert!(post_reconcile_checkpoint_needed(0, 1, 0, false),
        "modify-only delta must trigger save — stale postings on disk \
         would serve outdated search results until the next periodic save");
    assert!(post_reconcile_checkpoint_needed(0, 4, 0, false),
        "multi-modify must trigger save for the same reason");
}

#[test]
fn test_post_reconcile_checkpoint_needed_triggers_on_content_replace() {
    // Offline replace: delete A.cs + add B.cs → net live count unchanged but
    // index allocator grew and tombstoned a slot. Reviewer-caught HIGH
    // (commit-reviewer 2026-04-25, first pass): pre-fix `before != after`
    // gate would have skipped this and let session C load stale
    // `idx.files[]` after a force-kill of session B.
    assert!(post_reconcile_checkpoint_needed(1, 0, 1, false),
        "replace (1 add + 1 remove) MUST trigger save — the index allocator \
         grew and a slot was tombstoned even though live count is unchanged");
    assert!(post_reconcile_checkpoint_needed(3, 0, 3, false),
        "multi-replace MUST trigger save for the same reason");
}

#[test]
fn test_post_reconcile_checkpoint_needed_triggers_on_def_change() {
    // Def change with no content delta still must save: def-index .meta
    // and binary payload encode per-file definitions, not just a count.
    assert!(post_reconcile_checkpoint_needed(0, 0, 0, true),
        "def-only change (e.g. modified .cs body, unchanged file set) must save");
}

#[test]
fn test_post_reconcile_checkpoint_needed_skipped_on_noop() {
    // True steady-state startup: nothing changed in either index → must NOT
    // save. Required to keep cold-start cost zero on in-sync workspaces.
    assert!(!post_reconcile_checkpoint_needed(0, 0, 0, false),
        "no-op reconcile must NOT trigger a save (would cost 1.5-3s on a 60K repo)");
}

// ---------------------------------------------------------------------
// PR-B (Hole #2 / MCP-WCH-007): autosave_due gate unit tests.
//
// `autosave_due(have_unsaved, since_last_save) -> bool` is the pure
// timing decision behind the `start_watcher` two-tier autosave. It must:
//   - Stay silent on idle workspaces (no unsaved + below max ceiling).
//   - Fire after AUTOSAVE_QUIET_INTERVAL when there's pending work
//     (bursty edits then idle — the data-loss hole this PR closes).
//   - Fire unconditionally past AUTOSAVE_MAX_INTERVAL (defensive
//     ceiling matching legacy behavior).
// ---------------------------------------------------------------------

#[test]
fn test_autosave_due_skipped_when_idle_and_below_max() {
    // No unsaved work, well below the max ceiling → must NOT save. This
    // is the steady-state idle workspace case (no edits in hours,
    // periodic_autosave should not write to disk).
    assert!(!autosave_due(false, Duration::from_secs(0)));
    assert!(!autosave_due(false, Duration::from_secs(29)));
    assert!(!autosave_due(false, Duration::from_secs(120)));
    assert!(!autosave_due(false, AUTOSAVE_MAX_INTERVAL - Duration::from_secs(1)));
}

#[test]
fn test_autosave_due_skipped_when_unsaved_but_below_quiet() {
    // We just applied a batch (have_unsaved=true) but the quiet interval
    // hasn't elapsed yet — must NOT save. This avoids per-event write
    // amplification on workspaces with rapid sustained activity (an
    // editor save burst, a `git checkout` storm, a build that touches
    // hundreds of files in <1s).
    assert!(!autosave_due(true, Duration::from_secs(0)));
    assert!(!autosave_due(true, Duration::from_secs(15)));
    assert!(!autosave_due(true, AUTOSAVE_QUIET_INTERVAL - Duration::from_secs(1)));
}

#[test]
fn test_autosave_due_fires_on_quiet_interval_with_unsaved() {
    // The bursty-edit-then-idle case (delete 50 files, idle for 5min,
    // force-kill). Pre-PR-B the legacy 10-min gate would have lost all
    // 50; with the quiet gate we save ~300s after the last batch.
    assert!(autosave_due(true, AUTOSAVE_QUIET_INTERVAL));
    assert!(autosave_due(true, AUTOSAVE_QUIET_INTERVAL + Duration::from_secs(1)));
    assert!(autosave_due(true, Duration::from_secs(400)));
}

#[test]
fn test_autosave_due_fires_on_max_interval_even_when_idle() {
    // Defensive ceiling. Real `periodic_autosave` short-circuits via
    // its own `!idx.files.is_empty()` allocator-capacity gate when the
    // index has never held a file, so this is a no-op there — but the
    // gate itself must still fire to preserve legacy behavior.
    assert!(autosave_due(false, AUTOSAVE_MAX_INTERVAL));
    assert!(autosave_due(false, AUTOSAVE_MAX_INTERVAL + Duration::from_secs(1)));
    assert!(autosave_due(true, AUTOSAVE_MAX_INTERVAL));
}

#[test]
fn test_autosave_due_constants_have_expected_values() {
    // Pin the chosen budget so a future tweak lands intentionally
    // (and is reviewed against the durability/perf trade-off described
    // in `user-stories/meta-checkpoint-durability.md` Hole #2).
    assert_eq!(AUTOSAVE_QUIET_INTERVAL, Duration::from_secs(300),
        "300s bounds force-kill data loss to ~5min of unflushed activity; \
         prevents continuous autosave on large repos where serialization exceeds 30s");
    assert_eq!(AUTOSAVE_MAX_INTERVAL, Duration::from_secs(600),
        "10min preserves legacy behavior for steady-state idle workspaces");
    assert!(AUTOSAVE_QUIET_INTERVAL < AUTOSAVE_MAX_INTERVAL,
        "quiet interval must be tighter than max ceiling, otherwise have_unsaved gate is dead");
}

#[test]
fn test_autosave_success_clears_consumed_external_dirty_without_rearming() {
    let autosave_dirty = AtomicBool::new(true);
    let mut have_unsaved = false;

    let had_autosave_dirty = begin_autosave_attempt(&autosave_dirty);
    assert!(had_autosave_dirty);
    assert!(!autosave_dirty.load(Ordering::Relaxed));

    finish_autosave_attempt(true, had_autosave_dirty, &mut have_unsaved);

    assert!(!have_unsaved,
        "successful autosave must not copy the pre-save external dirty bit back into have_unsaved");
    assert!(!autosave_due(
        have_unsaved || autosave_dirty.load(Ordering::Relaxed),
        AUTOSAVE_QUIET_INTERVAL,
    ));
}

#[test]
fn test_autosave_success_preserves_external_dirty_set_during_save() {
    let autosave_dirty = AtomicBool::new(true);
    let mut have_unsaved = false;

    let had_autosave_dirty = begin_autosave_attempt(&autosave_dirty);
    autosave_dirty.store(true, Ordering::Relaxed);

    finish_autosave_attempt(true, had_autosave_dirty, &mut have_unsaved);

    assert!(!have_unsaved,
        "post-swap external dirty stays in the atomic channel, not the local one");
    assert!(autosave_dirty.load(Ordering::Relaxed),
        "external dirty set during serialization must survive for the next save");
    assert!(autosave_due(
        have_unsaved || autosave_dirty.load(Ordering::Relaxed),
        AUTOSAVE_QUIET_INTERVAL,
    ));
}

#[test]
fn test_autosave_failure_preserves_consumed_external_dirty_for_retry() {
    let autosave_dirty = AtomicBool::new(true);
    let mut have_unsaved = false;

    let had_autosave_dirty = begin_autosave_attempt(&autosave_dirty);
    finish_autosave_attempt(false, had_autosave_dirty, &mut have_unsaved);

    assert!(have_unsaved,
        "failed save must preserve retry pressure after consuming the external dirty bit");
    assert!(autosave_due(
        have_unsaved || autosave_dirty.load(Ordering::Relaxed),
        AUTOSAVE_QUIET_INTERVAL,
    ));
}

#[test]
fn test_periodic_autosave_returns_true_on_success() {
    // Sanity contract: a successful save must return true so callers
    // in start_watcher can clear `have_unsaved`. Pre-PR-B retry-fix
    // periodic_autosave returned (), so callers cleared have_unsaved
    // unconditionally and a transient write error meant we waited the
    // full AUTOSAVE_MAX_INTERVAL (10 min) before retrying.
    let (_tmp, _root, content_index) = make_batch_test_setup();
    let index_base = _tmp.path().join("idx");
    std::fs::create_dir_all(&index_base).unwrap();
    let result = periodic_autosave(&content_index, &None, &index_base);
    assert!(result, "successful save must return true");
}

#[test]
fn test_periodic_autosave_returns_true_on_empty_indexes() {
    // The allocator-capacity gate skips both saves when the index has
    // never held any file. That "nothing to do" outcome is durably
    // consistent with on-disk state by definition — must return true
    // so the caller clears `have_unsaved` and doesn't busy-retry on an
    // empty workspace.
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path().to_path_buf();
    let empty_content = Arc::new(RwLock::new(ContentIndex::default()));
    let result = periodic_autosave(&empty_content, &None, &index_base);
    assert!(result,
        "empty (never-allocated) indexes are durably consistent — must return true");
}

#[test]
fn test_periodic_autosave_returns_false_on_poisoned_lock() {
    // Failure semantics: if the read lock is poisoned (a writer panicked
    // mid-update) we cannot safely save the index. Must return false so
    // the caller leaves `have_unsaved=true` for the next quiet-interval
    // tick to retry. Reviewer-caught (commit-reviewer 2026-04-25, PR-B
    // first review): pre-fix periodic_autosave returned () and callers
    // cleared the flag unconditionally, hiding the failure for up to
    // AUTOSAVE_MAX_INTERVAL.
    let (_tmp, _root, content_index) = make_batch_test_setup();
    let index_base = _tmp.path().join("idx");
    std::fs::create_dir_all(&index_base).unwrap();

    // Poison the lock by panicking while holding the write side.
    let idx_clone = Arc::clone(&content_index);
    let h = std::thread::spawn(move || {
        let _guard = idx_clone.write().unwrap();
        panic!("intentional poisoning for test");
    });
    let _ = h.join(); // expect Err — ignore, lock is now poisoned.
    assert!(content_index.read().is_err(),
        "lock should be poisoned after panicking writer");

    let result = periodic_autosave(&content_index, &None, &index_base);
    assert!(!result,
        "poisoned read lock must surface as save failure (return false) \
         so the caller retries instead of clearing `have_unsaved`");
}

#[test]
fn test_periodic_autosave_clone_snapshot_is_independent() {
    // Verify that clone-then-serialize produces a snapshot independent
    // of the original: mutations to the live index after clone must NOT
    // appear in the saved file. This guards the correctness of the
    // lock-free serialization path.
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let content_index = Arc::new(RwLock::new(ContentIndex {
        root: tmp.path().to_string_lossy().to_string(),
        format_version: code_xray::CONTENT_INDEX_VERSION,
        files: vec!["original.cs".to_string()],
        index: {
            let mut m = HashMap::new();
            m.insert("original_token".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m
        },
        total_tokens: 1,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![1],
        ..Default::default()
    }));

    // Take a snapshot (simulating what periodic_autosave does)
    let snapshot = content_index.read().unwrap().clone();

    // Mutate the live index AFTER the snapshot was taken
    {
        let mut idx = content_index.write().unwrap();
        idx.files.push("mutated.cs".to_string());
        idx.index.insert("mutated_token".to_string(), vec![Posting { file_id: 1, lines: vec![2] }]);
        idx.total_tokens = 2;
        idx.file_token_counts.push(1);
    }

    // Save the snapshot (should contain only the ORIGINAL state)
    crate::save_content_index(&snapshot, index_base).unwrap();

    // Load back and verify it matches the snapshot, not the mutated live index
    let content_path = crate::index::content_index_path_for(
        &tmp.path().to_string_lossy(), "cs", index_base
    );
    assert!(content_path.exists(), "saved index file must exist");

    let loaded = crate::load_content_index(
        &tmp.path().to_string_lossy(), "cs", index_base
    ).unwrap();
    assert_eq!(loaded.files.len(), 1, "snapshot must have 1 file (original), not 2 (mutated)");
    assert!(loaded.index.contains_key("original_token"),
        "snapshot must contain original_token");
    assert!(!loaded.index.contains_key("mutated_token"),
        "snapshot must NOT contain mutated_token (added after clone)");
    assert_eq!(loaded.total_tokens, 1, "snapshot total_tokens must be 1 (original)");

    // Verify the live index has the mutation
    let live = content_index.read().unwrap();
    assert_eq!(live.files.len(), 2, "live index must have 2 files after mutation");
    assert!(live.index.contains_key("mutated_token"),
        "live index must contain mutated_token");
}



#[test]
fn test_post_reconcile_checkpoint_writes_meta_after_removal() {
    // End-to-end durability test for Hole #1:
    //   1. Build an index with 2 files on disk.
    //   2. Save it (mimics the previous session's `.meta` baseline).
    //   3. Remove one file from disk (mimics offline-edit between sessions).
    //   4. Run the post-reconcile sequence the way `start_watcher` does:
    //      reconcile (returns add/remove counts) → check
    //      `post_reconcile_checkpoint_needed` → save.
    //   5. Assert `.meta` on disk reflects the new live count (1, not 2)
    //      WITHOUT waiting for the 10-minute `AUTOSAVE_INTERVAL`.
    let (_tmp, dir, index) = make_batch_test_setup();
    let index_base = _tmp.path().join("idx");
    std::fs::create_dir_all(&index_base).unwrap();

    // Save the pre-removal baseline so an old `.meta` exists on disk.
    {
        let idx = index.read().unwrap();
        crate::save_content_index(&idx, &index_base).unwrap();
    }

    // Future-date `created_at` so the surviving b.cs is NOT spuriously
    // flagged as modified by this reconcile (its mtime falls below the
    // future-dated `created_at - 2s` threshold). Without this the test
    // would still pass on the gate (modified contributes via OR) but
    // would no longer cleanly isolate the remove-only case it claims
    // to validate. Same +5s pattern as the periodic-rescan tests above.
    {
        let mut idx = index.write().unwrap();
        idx.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() + 5;
    }

    let content_path = crate::index::content_index_path_for(
        &dir.to_string_lossy(), "cs", &index_base,
    );
    let meta_path = {
        let mut p = content_path.as_os_str().to_owned();
        p.push(".meta");
        std::path::PathBuf::from(p)
    };
    let meta_before: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&meta_path).unwrap()
    ).unwrap();
    assert_eq!(meta_before["files"], 2, "baseline meta should record 2 files");

    // Simulate offline removal of one file between sessions.
    std::fs::remove_file(dir.join("a.cs")).unwrap();

    // Run the post-reconcile sequence start_watcher uses.
    let dir_str = crate::clean_path(&dir.to_string_lossy());
    let extensions = vec!["cs".to_string()];
    let (added, modified, removed) = crate::mcp::watcher::reconcile_content_index(
        &index, &dir_str, &extensions, false,
    );
    assert_eq!(added, 0, "no files added");
    assert_eq!(modified, 0, "surviving b.cs must not be flagged as modified \
        (future-dated created_at puts threshold above its mtime)");
    assert_eq!(removed, 1, "one file removed offline");
    assert_eq!(index.read().unwrap().live_file_count(), 1,
        "post-reconcile live count should be 1");
    assert!(post_reconcile_checkpoint_needed(added, modified, removed, false),
        "removal must trigger checkpoint");

    periodic_autosave(&index, &None, &index_base);

    // Verify `.meta` on disk now matches the new live count — this is the
    // file the next session loads at startup. Without the post-reconcile
    // save, a forced-kill within AUTOSAVE_INTERVAL would resurrect 2.
    let meta_after: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&meta_path).unwrap()
    ).unwrap();
    assert_eq!(meta_after["files"], 1,
        "post-reconcile checkpoint must rewrite .meta with the new live count");
}

#[test]
fn test_post_reconcile_checkpoint_writes_postings_after_modify_only() {
    // End-to-end durability test for the modify-only case (reviewer-caught
    // MEDIUM, commit-reviewer 2026-04-25 second pass).
    //
    // Scenario: session B starts, reconcile finds an offline-modified file
    // (no add, no remove), rewrites postings in RAM. Session B is force-
    // killed before the next periodic save. Session C must load the new
    // postings from disk, not the pre-modify ones.
    //
    //   1. Build an index with 2 files containing token "alpha".
    //   2. Save it (mimics the previous session's binary index baseline).
    //   3. Bump file mtime so reconcile sees it as modified, then OVERWRITE
    //      one file's content so its postings change (alpha → gamma).
    //   4. Run the post-reconcile sequence: reconcile (returns
    //      `modified=1`) → check `post_reconcile_checkpoint_needed` → save.
    //   5. Reload the index from disk and assert the new token "gamma" is
    //      in the on-disk inverted index for that file.
    //
    // Pre-MEDIUM-fix: gate was `added > 0 || removed > 0 || def_changed`,
    // so a pure-modify reconcile would skip the save and the on-disk
    // index would still serve stale "alpha" postings until the next
    // periodic autosave (up to 10 minutes) or shutdown.
    let (_tmp, dir, index) = make_batch_test_setup();
    let index_base = _tmp.path().join("idx");
    std::fs::create_dir_all(&index_base).unwrap();

    {
        let idx = index.read().unwrap();
        crate::save_content_index(&idx, &index_base).unwrap();
    }

    // Set created_at to now + 5s so reconcile only flags files modified
    // AFTER this baseline (the threshold is `created_at - 2s` whole
    // seconds, so future-dating by +5s puts the threshold ≥ 3s in the
    // future and any pre-existing fixture mtime falls safely below it).
    // Same trick the periodic-rescan tests use to avoid sleep-based
    // timing flakiness on Windows where mtime sub-second precision can
    // straddle a 1-2s sleep window.
    {
        let mut idx = index.write().unwrap();
        idx.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() + 5;
    }

    // Sleep past the future-dated threshold. created_at = floor(now)+5,
    // threshold = created_at - 2 = floor(now)+3 (whole seconds). The
    // overwrite below sets mtime ≈ now + sleep; we need
    // floor(now + sleep) > floor(now) + 3, i.e. sleep > 3s + (1 - frac(now)).
    // Worst case frac(now)≈0 → need sleep > 4s. Round to 5s for headroom
    // (reconcile uses strict `>` and FS mtime resolution can be 1s on some
    // filesystems).
    std::thread::sleep(std::time::Duration::from_millis(5000));
    std::fs::write(dir.join("a.cs"), "class Gamma { Telemetry tracer; }").unwrap();

    let dir_str = crate::clean_path(&dir.to_string_lossy());
    let extensions = vec!["cs".to_string()];
    let (added, modified, removed) = crate::mcp::watcher::reconcile_content_index(
        &index, &dir_str, &extensions, false,
    );
    assert_eq!(added, 0, "no files added");
    assert_eq!(modified, 1, "one file modified offline");
    assert_eq!(removed, 0, "no files removed");
    assert!(post_reconcile_checkpoint_needed(added, modified, removed, false),
        "modify-only must trigger checkpoint (MEDIUM fix)");

    periodic_autosave(&index, &None, &index_base);

    // Reload the on-disk index and assert it carries the new postings.
    // This proves the save committed the modify-only delta to disk.
    let content_path = crate::index::content_index_path_for(
        &dir.to_string_lossy(), "cs", &index_base,
    );
    let reloaded: ContentIndex = crate::index::load_content_index_at_path(&content_path)
        .expect("reload on-disk index");
    assert!(reloaded.index.contains_key("gamma"),
        "post-reconcile checkpoint must persist new 'gamma' token to disk \
         (modify-only case — pre-MEDIUM-fix would still serve stale 'alpha')");
    assert!(reloaded.index.contains_key("telemetry"),
        "post-reconcile checkpoint must persist new 'telemetry' token to disk");
}



