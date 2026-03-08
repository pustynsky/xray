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
    batch_purge_files(&mut inverted, &ids);

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

    batch_purge_files(&mut inverted, &HashSet::new());

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
    batch_purge_files(&mut inverted2, &ids);

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
/// wrapped in Arc<RwLock> for process_batch.
fn make_batch_test_setup() -> (tempfile::TempDir, Arc<RwLock<ContentIndex>>) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

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

    let index = ContentIndex {
        root: dir.to_string_lossy().to_string(),
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

    (tmp, Arc::new(RwLock::new(index)))
}

#[test]
fn test_process_batch_empty() {
    let (_tmp, index) = make_batch_test_setup();
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
    let (tmp, index) = make_batch_test_setup();

    // Modify file a.cs with new content
    let file_a = tmp.path().join("a.cs");
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
}

#[test]
fn test_process_batch_removed_file() {
    let (tmp, index) = make_batch_test_setup();

    let file_a = tmp.path().join("a.cs");

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
    let clean_a = crate::clean_path(&tmp.path().join("a.cs").to_string_lossy());
    assert!(!idx.path_to_id.as_ref().unwrap().contains_key(&PathBuf::from(&clean_a)),
        "removed file should not be in path_to_id");
    // removed set should be drained
    assert!(removed.is_empty(), "removed set should be drained after process_batch");
}

#[test]
fn test_process_batch_mixed_dirty_and_removed() {
    let (tmp, index) = make_batch_test_setup();

    // Remove file a, modify file b
    let file_a = tmp.path().join("a.cs");
    let file_b = tmp.path().join("b.cs");
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
    let (tmp, index) = make_batch_test_setup();

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
    let (tmp, index) = make_batch_test_setup();

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
    let (tmp, index) = make_batch_test_setup();
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

    apply_tokenized_file(&mut index, result);

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

    apply_tokenized_file(&mut index, result);

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
    apply_tokenized_file(&mut index, result);
    assert_eq!(index.total_tokens, 0);
}

#[test]
fn test_nonblocking_update_content_index_tokens_consistent() {
    let (tmp, index) = make_batch_test_setup();

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
fn test_nonblocking_update_content_index_new_file_tokens_consistent() {
    let (tmp, index) = make_batch_test_setup();

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
    let (tmp, index) = make_batch_test_setup();

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
