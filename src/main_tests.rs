    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::io::Write;
    use regex::Regex;

    // ─── clean_path tests ────────────────────────────────
    // NOTE: test_clean_path_strips_prefix and test_clean_path_no_prefix
    // are canonical in lib.rs::lib_tests — only unique tests remain here.

    #[test]
    fn test_clean_path_unix_style() {
        assert_eq!(clean_path("/usr/bin/ls"), "/usr/bin/ls");
    }

    // ─────────────────────────────────────────────────────────────────

    // NOTE: test_tokenize_basic, test_tokenize_code, test_tokenize_min_length
    // are canonical in lib.rs::lib_tests — only unique tests remain here.

    #[test]
    fn test_tokenize_with_numbers() {
        let tokens = tokenize("var x2 = getValue(item3);", 2);
        assert!(tokens.contains(&"x2".to_string()));
        assert!(tokens.contains(&"getvalue".to_string()));
        assert!(tokens.contains(&"item3".to_string()));
    }

    #[test]
    fn test_tokenize_underscores() {
        let tokens = tokenize("my_variable = some_func()", 2);
        assert!(tokens.contains(&"my_variable".to_string()));
        assert!(tokens.contains(&"some_func".to_string()));
    }

    #[test]
    fn test_tokenize_empty_string() {
        let tokens = tokenize("", 2);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_file_index_not_stale() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let index = FileIndex {
            root: ".".to_string(),
            created_at: now,
            max_age_secs: 3600,
            entries: vec![],
        };
        assert!(!index.is_stale());
    }

    #[test]
    fn test_file_index_stale() {
        let index = FileIndex {
            root: ".".to_string(),
            created_at: 0, // epoch = definitely stale
            max_age_secs: 3600,
            entries: vec![],
        };
        assert!(index.is_stale());
    }

    #[test]
    fn test_content_index_not_stale() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let index = ContentIndex {
            root: ".".to_string(),
            created_at: now,
            ..Default::default()
        };
        assert!(!index.is_stale());
    }

    #[test]
    fn test_content_index_stale() {
        let index = ContentIndex {
            root: ".".to_string(),
            ..Default::default()
        };
        assert!(index.is_stale());
    }

    #[test]
    fn test_build_and_search_content_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("file1.cs")).unwrap();
        writeln!(f1, "using System;").unwrap();
        writeln!(f1, "public class HttpClient {{ }}").unwrap();

        let mut f2 = fs::File::create(dir.join("file2.cs")).unwrap();
        writeln!(f2, "private HttpClient _client;").unwrap();
        writeln!(f2, "private ILogger _logger;").unwrap();

        let index = build_content_index(&ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            threads: 1,
            ..Default::default()
        }).unwrap();

        assert_eq!(index.files.len(), 2);
        assert!(index.index.contains_key("httpclient"));

        let postings = &index.index["httpclient"];
        assert_eq!(postings.len(), 2, "HttpClient should appear in both files");
    }

    #[test]
    fn test_build_file_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        fs::write(dir.join("file1.cs"), "public class Foo {}").unwrap();
        fs::write(dir.join("file2.rs"), "fn main() {}").unwrap();

        let index = build_index(&IndexArgs {
            dir: dir.to_string_lossy().to_string(),
            threads: 1,
            ..Default::default()
        }).unwrap();

        assert!(index.entries.len() >= 2, "Should find at least 2 files");
        assert!(!index.is_stale());

        let names: Vec<&str> = index.entries.iter()
            .filter_map(|e| std::path::Path::new(&e.path).file_name().and_then(|n| n.to_str()))
            .collect();
        assert!(names.contains(&"file1.cs"), "Should contain file1.cs");
        assert!(names.contains(&"file2.rs"), "Should contain file2.rs");
    }

    #[test]
    fn test_build_file_index_follows_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let target = tmp.path().join("target");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&target).unwrap();

        // Put a file in the target directory
        fs::write(target.join("linked.cs"), "public class Linked {}").unwrap();

        // Create a symlink: root/ext -> target
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&target, root.join("ext")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, root.join("ext")).unwrap();

        let index = build_index(&IndexArgs {
            dir: root.to_string_lossy().to_string(),
            threads: 1,
            ..Default::default()
        }).unwrap();

        let names: Vec<&str> = index.entries.iter()
            .filter_map(|e| std::path::Path::new(&e.path).file_name().and_then(|n| n.to_str()))
            .collect();
        assert!(names.contains(&"linked.cs"), "Should find file through symlink, got: {:?}", names);
    }

    #[test]
    fn test_build_content_index_follows_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let target = tmp.path().join("target");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&target).unwrap();

        // Put a file with unique content in the target directory
        fs::write(target.join("linked.rs"), "fn symlink_unique_token_xyzzy() {}").unwrap();

        // Create a symlink: root/ext -> target
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&target, root.join("ext")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, root.join("ext")).unwrap();

        let index = build_content_index(&ContentIndexArgs {
            dir: root.to_string_lossy().to_string(),
            ext: "rs".to_string(),
            threads: 1,
            ..Default::default()
        }).unwrap();

        // The unique token should be found in the index
        assert!(
            index.index.contains_key("symlink_unique_token_xyzzy"),
            "Should find content through symlink, tokens: {:?}",
            index.index.keys().collect::<Vec<_>>()
        );
    }

    // ── ContentIndex staleness tests / Serialization roundtrip tests ────────

    #[test]
    fn test_file_index_serialization_roundtrip() {
        let index = FileIndex {
            root: "C:\\test".to_string(),
            created_at: 1000000,
            max_age_secs: 3600,
            entries: vec![
                FileEntry {
                    path: "C:\\test\\file1.txt".to_string(),
                    size: 1024,
                    modified: 999999,
                    is_dir: false,
                },
                FileEntry {
                    path: "C:\\test\\subdir".to_string(),
                    size: 0,
                    modified: 999998,
                    is_dir: true,
                },
            ],
        };
        let encoded = bincode::serialize(&index).unwrap();
        let decoded: FileIndex = bincode::deserialize(&encoded).unwrap();
        assert_eq!(decoded.root, "C:\\test");
        assert_eq!(decoded.entries.len(), 2);
        assert_eq!(decoded.entries[0].path, "C:\\test\\file1.txt");
        assert_eq!(decoded.entries[0].size, 1024);
        assert!(!decoded.entries[0].is_dir);
        assert!(decoded.entries[1].is_dir);
    }

    #[test]
    fn test_content_index_serialization_roundtrip() {
        let mut idx = HashMap::new();
        idx.insert(
            "httpclient".to_string(),
            vec![Posting {
                file_id: 0,
                lines: vec![5, 12, 30],
            }],
        );
        let index = ContentIndex {
            root: "C:\\test".to_string(),
            created_at: 1000000,
            files: vec!["C:\\test\\Program.cs".to_string()],
            index: idx,
            total_tokens: 100,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![50],
            ..Default::default()
        };
        let encoded = bincode::serialize(&index).unwrap();
        let decoded: ContentIndex = bincode::deserialize(&encoded).unwrap();
        assert_eq!(decoded.root, "C:\\test");
        assert_eq!(decoded.files.len(), 1);
        assert_eq!(decoded.total_tokens, 100);
        assert_eq!(decoded.file_token_counts, vec![50]);
        let postings = decoded.index.get("httpclient").unwrap();
        assert_eq!(postings.len(), 1);
        assert_eq!(postings[0].file_id, 0);
        assert_eq!(postings[0].lines, vec![5, 12, 30]);
    }

    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_tf_idf_more_relevant_file_scores_higher() {
        // File A: small file, HttpClient is 50% of tokens -> high TF
        // File B: big file, HttpClient is 1% of tokens -> low TF
        let total_docs = 1000.0_f64;
        let doc_freq = 100.0_f64;
        let idf = (total_docs / doc_freq).ln();

        let tf_a = 5.0 / 10.0;  // 50% of file A
        let tf_b = 5.0 / 500.0; // 1% of file B

        let score_a = tf_a * idf;
        let score_b = tf_b * idf;

        assert!(score_a > score_b, "Smaller, more focused file should rank higher");
        assert!(score_a > 0.0);
        assert!(score_b > 0.0);
    }

    #[test]
    fn test_tf_idf_rare_term_scores_higher() {
        // Same TF, but term A appears in 10 docs, term B in 900 docs
        let total_docs = 1000.0_f64;
        let tf = 0.1;

        let idf_rare = (total_docs / 10.0).ln();
        let idf_common = (total_docs / 900.0).ln();

        let score_rare = tf * idf_rare;
        let score_common = tf * idf_common;

        assert!(score_rare > score_common, "Rare term should score higher");
    }

    // ── Integration test: build file index / Multi-term search tests ───────

    #[test]
    fn test_multi_term_or_search() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("both.cs")).unwrap();
        writeln!(f1, "class Foo {{ HttpClient client; ILogger logger; }}").unwrap();

        let mut f2 = fs::File::create(dir.join("only_client.cs")).unwrap();
        writeln!(f2, "class Bar {{ HttpClient client; }}").unwrap();

        let mut f3 = fs::File::create(dir.join("only_logger.cs")).unwrap();
        writeln!(f3, "class Baz {{ ILogger logger; }}").unwrap();

        let mut f4 = fs::File::create(dir.join("neither.cs")).unwrap();
        writeln!(f4, "class Empty {{ int x; }}").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        // OR: files with "httpclient" OR "ilogger"
        let term1_postings = index.index.get("httpclient");
        let term2_postings = index.index.get("ilogger");

        assert!(term1_postings.is_some());
        assert!(term2_postings.is_some());

        // Collect all file_ids from both terms (union = OR)
        let mut or_files: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for p in term1_postings.unwrap() { or_files.insert(p.file_id); }
        for p in term2_postings.unwrap() { or_files.insert(p.file_id); }

        // both.cs, only_client.cs, only_logger.cs = 3 files
        assert_eq!(or_files.len(), 3, "OR should match 3 files");

        // AND: intersection
        let t1_files: std::collections::HashSet<u32> = term1_postings.unwrap().iter().map(|p| p.file_id).collect();
        let t2_files: std::collections::HashSet<u32> = term2_postings.unwrap().iter().map(|p| p.file_id).collect();
        let and_files: Vec<u32> = t1_files.intersection(&t2_files).cloned().collect();

        // Only both.cs
        assert_eq!(and_files.len(), 1, "AND should match 1 file");
    }

    #[test]
    fn test_multi_term_and_search() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("all_three.cs")).unwrap();
        writeln!(f1, "HttpClient Task ILogger").unwrap();

        let mut f2 = fs::File::create(dir.join("two_of_three.cs")).unwrap();
        writeln!(f2, "HttpClient Task SomeOther").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        // Check all three terms exist
        let terms = ["httpclient", "task", "ilogger"];
        for term in &terms {
            assert!(index.index.contains_key(*term), "Term '{}' should be in index", term);
        }

        // AND: only all_three.cs should have all 3 terms
        let file_sets: Vec<std::collections::HashSet<u32>> = terms.iter()
            .map(|t| index.index.get(*t).unwrap().iter().map(|p| p.file_id).collect())
            .collect();

        let intersection = file_sets.iter().skip(1).fold(file_sets[0].clone(), |acc, s| {
            acc.intersection(s).cloned().collect()
        });

        assert_eq!(intersection.len(), 1, "Only 1 file should contain all 3 terms");
    }

    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_regex_token_matching() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("caches.cs")).unwrap();
        writeln!(f1, "ITenantCache IUserCache ISessionCache INotAMatch").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        // Regex "i.*cache" should match itenantcache, iusercache, isessioncache
        let re = Regex::new("(?i)^i.*cache$").unwrap();
        let matching_tokens: Vec<&String> = index.index.keys()
            .filter(|k| re.is_match(k))
            .collect();

        assert!(matching_tokens.len() >= 3,
            "Should match at least 3 cache tokens, got {}: {:?}", matching_tokens.len(), matching_tokens);

        // "inotamatch" should NOT match the cache regex
        assert!(
            !matching_tokens.contains(&&"inotamatch".to_string()),
            "inotamatch should not match i.*cache pattern"
        );
    }

    #[test]
    fn test_regex_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("simple.cs")).unwrap();
        writeln!(f1, "class Foo {{ int x; }}").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        let re = Regex::new("(?i)^zzzznonexistent$").unwrap();
        let matching: Vec<&String> = index.index.keys()
            .filter(|k| re.is_match(k))
            .collect();

        assert_eq!(matching.len(), 0, "Non-existent pattern should match 0 tokens");
    }

    #[test]
    fn test_regex_matches_partial_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("async.cs")).unwrap();
        writeln!(f1, "GetAsync PostAsync SendAsync SyncMethod").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        // Pattern ".*async" should match getasync, postasync, sendasync
        let re = Regex::new("(?i)^.*async$").unwrap();
        let matching: Vec<&String> = index.index.keys()
            .filter(|k| re.is_match(k))
            .collect();

        assert!(matching.len() >= 3, "Should match at least 3 async tokens, got {}: {:?}", matching.len(), matching);
        assert!(
            !matching.contains(&&"syncmethod".to_string()),
            "syncmethod should not match .*async$ pattern"
        );
    }

    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_exclude_dir_filters_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("zzztests")).unwrap();
        fs::create_dir_all(dir.join("zzztests").join("zzzE2E")).unwrap();

        let mut f1 = fs::File::create(dir.join("src").join("main.cs")).unwrap();
        writeln!(f1, "class Main {{ HttpClient client; }}").unwrap();

        let mut f2 = fs::File::create(dir.join("zzztests").join("test1.cs")).unwrap();
        writeln!(f2, "class Test1 {{ HttpClient client; }}").unwrap();

        let mut f3 = fs::File::create(dir.join("zzztests").join("zzzE2E").join("e2e.cs")).unwrap();
        writeln!(f3, "class E2ETest {{ HttpClient client; }}").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        assert_eq!(index.files.len(), 3, "Should index 3 files");

        // Simulate exclude_dir filtering (using unique name to avoid matching temp path)
        let exclude_dirs = ["zzztests".to_string()];
        let postings = index.index.get("httpclient").unwrap();
        let filtered: Vec<_> = postings.iter()
            .filter(|p| {
                let path = &index.files[p.file_id as usize];
                !exclude_dirs.iter().any(|excl| path.to_lowercase().contains(&excl.to_lowercase()))
            })
            .collect();

        // Only src/main.cs should remain (zzztests/ and zzztests/zzzE2E/ excluded)
        assert_eq!(filtered.len(), 1, "After excluding 'zzztests' dir, should have 1 file");
        assert!(
            index.files[filtered[0].file_id as usize].contains("main.cs"),
            "Remaining file should be main.cs"
        );
    }

    #[test]
    fn test_exclude_pattern_filters_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("Service.cs")).unwrap();
        writeln!(f1, "class Service {{ HttpClient c; }}").unwrap();

        let mut f2 = fs::File::create(dir.join("ServiceMock.cs")).unwrap();
        writeln!(f2, "class ServiceMock {{ HttpClient c; }}").unwrap();

        let mut f3 = fs::File::create(dir.join("ServiceTests.cs")).unwrap();
        writeln!(f3, "class ServiceTests {{ HttpClient c; }}").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        let postings = index.index.get("httpclient").unwrap();

        // Exclude Mock and Tests
        let excludes = ["mock".to_string(), "tests".to_string()];
        let filtered: Vec<_> = postings.iter()
            .filter(|p| {
                let path = &index.files[p.file_id as usize];
                !excludes.iter().any(|excl| path.to_lowercase().contains(&excl.to_lowercase()))
            })
            .collect();

        assert_eq!(filtered.len(), 1, "After excluding Mock and Tests, should have 1 file");
        assert!(
            index.files[filtered[0].file_id as usize].contains("Service.cs")
                && !index.files[filtered[0].file_id as usize].contains("Mock"),
            "Remaining file should be Service.cs (not Mock or Tests)"
        );
    }

    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_context_lines_calculation() {
        // Test the context window logic directly
        let total_lines = 10;
        let match_line: usize = 5; // 0-indexed
        let ctx = 2;

        let start = match_line.saturating_sub(ctx);
        let end = (match_line + ctx).min(total_lines - 1);

        assert_eq!(start, 3, "Context should start at line 3 (2 before line 5)");
        assert_eq!(end, 7, "Context should end at line 7 (2 after line 5)");
    }

    #[test]
    fn test_context_lines_at_file_boundaries() {
        // Match at line 1 (index 0) with context 3 -> should not go below 0
        let match_line: usize = 0;
        let ctx = 3;
        let total_lines = 10;

        let start = match_line.saturating_sub(ctx);
        let end = (match_line + ctx).min(total_lines - 1);

        assert_eq!(start, 0, "Context should not go below 0");
        assert_eq!(end, 3, "Context should extend to line 3");

        // Match at last line with context 3 -> should not exceed total
        let match_line2: usize = 9;
        let start2 = match_line2.saturating_sub(ctx);
        let end2 = (match_line2 + ctx).min(total_lines - 1);

        assert_eq!(start2, 6, "Context before should be line 6");
        assert_eq!(end2, 9, "Context should not exceed total_lines - 1");
    }

    #[test]
    fn test_context_merges_overlapping_ranges() {
        // Two matches close together should merge context
        let match_lines = vec![4usize, 6usize]; // 0-indexed
        let ctx = 2;
        let total_lines = 15;

        let mut lines_to_show: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for &m in &match_lines {
            let start = m.saturating_sub(ctx);
            let end = (m + ctx).min(total_lines - 1);
            for i in start..=end {
                lines_to_show.insert(i);
            }
        }

        // Lines 2-8 should be in the set (merged ranges)
        let result: Vec<usize> = lines_to_show.into_iter().collect();
        assert_eq!(result, vec![2, 3, 4, 5, 6, 7, 8], "Overlapping contexts should merge");
    }
    // ─── cleanup_indexes_for_dir tests ───────────────────

    #[test]
    fn test_cleanup_indexes_for_dir_removes_matching() {
        let tmp = tempfile::tempdir().unwrap();
        let idx_base = tmp.path().join("indexes");
        fs::create_dir_all(&idx_base).unwrap();

        // Create a test directory to act as the "root"
        let test_root = tmp.path().join("myproject");
        fs::create_dir_all(&test_root).unwrap();
        fs::write(test_root.join("hello.cs"), "class Hello {}").unwrap();

        // Build and save indexes for this directory
        let root_str = test_root.to_string_lossy().to_string();

        // Save a file index
        let file_idx = build_index(&IndexArgs {
            dir: root_str.clone(),
            threads: 1,
            ..Default::default()
        }).unwrap();
        save_index(&file_idx, &idx_base).unwrap();

        // Save a content index
        let content_idx = build_content_index(&ContentIndexArgs {
            dir: root_str.clone(),
            threads: 1,
            ..Default::default()
        }).unwrap();
        save_content_index(&content_idx, &idx_base).unwrap();

        // Verify index files exist
        let count_before: usize = fs::read_dir(&idx_base).unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|ext|
                ext == "file-list" || ext == "word-search" || ext == "code-structure"))
            .count();
        assert!(count_before >= 2, "Expected at least 2 index files, got {}", count_before);

        // Run cleanup for that directory
        let removed = cleanup_indexes_for_dir(&root_str, &idx_base);
        assert_eq!(removed, count_before, "Should remove all indexes for the directory");

        // Verify no index files remain
        let count_after: usize = fs::read_dir(&idx_base).unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|ext|
                ext == "file-list" || ext == "word-search" || ext == "code-structure"))
            .count();
        assert_eq!(count_after, 0, "No index files should remain");
    }

    #[test]
    fn test_cleanup_indexes_for_dir_preserves_other() {
        let tmp = tempfile::tempdir().unwrap();
        let idx_base = tmp.path().join("indexes");
        fs::create_dir_all(&idx_base).unwrap();

        // Create two test directories
        let dir_a = tmp.path().join("project_a");
        let dir_b = tmp.path().join("project_b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        fs::write(dir_a.join("a.cs"), "class A {}").unwrap();
        fs::write(dir_b.join("b.cs"), "class B {}").unwrap();

        let root_a = dir_a.to_string_lossy().to_string();
        let root_b = dir_b.to_string_lossy().to_string();

        // Build indexes for both directories
        let idx_a = build_index(&IndexArgs {
            dir: root_a.clone(),
            threads: 1,
            ..Default::default()
        }).unwrap();
        save_index(&idx_a, &idx_base).unwrap();

        let idx_b = build_index(&IndexArgs {
            dir: root_b.clone(),
            threads: 1,
            ..Default::default()
        }).unwrap();
        save_index(&idx_b, &idx_base).unwrap();

        // Cleanup only dir_a
        let removed = cleanup_indexes_for_dir(&root_a, &idx_base);
        assert_eq!(removed, 1, "Should remove exactly 1 index for dir_a");

        // dir_b index should still exist
        let remaining: usize = fs::read_dir(&idx_base).unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|ext|
                ext == "file-list" || ext == "word-search" || ext == "code-structure"))
            .count();
        assert_eq!(remaining, 1, "dir_b index should still exist");
    }

    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_phrase_xray_finds_exact_phrase() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("has_phrase.cs")).unwrap();
        writeln!(f1, "using System;").unwrap();
        writeln!(f1, "var client = new HttpClient();").unwrap();
        writeln!(f1, "client.GetAsync(\"/api\");").unwrap();

        let mut f2 = fs::File::create(dir.join("has_words_but_not_phrase.cs")).unwrap();
        writeln!(f2, "// HttpClient is useful").unwrap();
        writeln!(f2, "// but we use new patterns here").unwrap();
        writeln!(f2, "var x = new Something();").unwrap();

        let mut f3 = fs::File::create(dir.join("no_match.cs")).unwrap();
        writeln!(f3, "class Empty {{ }}").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        // Simulate phrase search: tokenize, AND search, then verify
        let phrase = "new HttpClient";
        let phrase_lower = phrase.to_lowercase();
        let phrase_tokens = tokenize(&phrase_lower, 2);

        assert_eq!(phrase_tokens, vec!["new", "httpclient"]);

        // AND search: find files with both "new" AND "httpclient"
        let mut candidate_ids: Option<std::collections::HashSet<u32>> = None;
        for token in &phrase_tokens {
            if let Some(postings) = index.index.get(token.as_str()) {
                let ids: std::collections::HashSet<u32> = postings.iter().map(|p| p.file_id).collect();
                candidate_ids = Some(match candidate_ids {
                    Some(existing) => existing.intersection(&ids).cloned().collect(),
                    None => ids,
                });
            }
        }
        let candidates = candidate_ids.unwrap_or_default();
        // Both files 1 and 2 have "new" and "httpclient" (but not as adjacent phrase in file 2)
        assert!(!candidates.is_empty(), "Should find at least 1 candidate");

        // Verify: only file 1 has the exact phrase
        let mut verified = Vec::new();
        for &fid in &candidates {
            let path = &index.files[fid as usize];
            if let Ok(content) = fs::read_to_string(path)
                && content.to_lowercase().contains(&phrase_lower) {
                    verified.push(fid);
                }
        }

        assert_eq!(verified.len(), 1, "Only 1 file should contain exact phrase 'new HttpClient'");
        assert!(
            index.files[verified[0] as usize].contains("has_phrase"),
            "The verified file should be has_phrase.cs"
        );
    }

    #[test]
    fn test_phrase_search_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("file.cs")).unwrap();
        writeln!(f1, "class Foo {{ int x; string y; }}").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        let phrase = "new HttpClient";
        let phrase_lower = phrase.to_lowercase();
        let phrase_tokens = tokenize(&phrase_lower, 2);

        // "new" and "httpclient" are not in the index for this file
        let has_all = phrase_tokens.iter().all(|t| index.index.contains_key(t.as_str()));
        assert!(!has_all, "Not all phrase tokens should exist in index");
    }

    #[test]
    fn test_phrase_search_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let mut f1 = fs::File::create(dir.join("mixed.cs")).unwrap();
        writeln!(f1, "var c = New HTTPCLIENT();").unwrap();

        let args = ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            no_ignore: true,
            threads: 1,
            ..Default::default()
        };
        let index = build_content_index(&args).unwrap();

        let phrase = "new HttpClient";
        let phrase_lower = phrase.to_lowercase();

        // Verify case-insensitive match
        let fid = 0u32;
        let content = fs::read_to_string(&index.files[fid as usize]).unwrap();
        assert!(
            content.to_lowercase().contains(&phrase_lower),
            "Case-insensitive phrase match should work"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Tests for `code_xray::is_path_within` — symlink-aware workspace
    // boundary check used by xray_edit (sync reindex), xray_fast (cache reuse),
    // and xray_grep (validate_search_dir).
    //
    // Regression: previously each call site canonicalized the input path,
    // which resolved symlinked subdirectories like `docs/personal` to their
    // real target (e.g. `D:\Personal`) and falsely classified them as outside
    // the workspace. `is_path_within` does a logical comparison first to match
    // what the indexer (`WalkBuilder::follow_links`) actually sees on disk.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn test_is_path_within_through_symlinked_subdir() {
        // Setup: <tmp>/root + <tmp>/external/file.md, then root/personal -> external.
        // A file accessed via root/personal/file.md must be classified as
        // belonging to root (the indexer sees it under the logical path).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let external = tmp.path().join("external");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("file.md"), "# external doc").unwrap();

        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&external, root.join("personal")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&external, root.join("personal")).unwrap();

        let logical_path = root.join("personal").join("file.md");
        let logical_str = logical_path.to_string_lossy().to_string();
        let root_str = root.to_string_lossy().to_string();

        assert!(
            code_xray::is_path_within(&logical_str, &root_str),
            "File reached via symlinked subdir must be inside workspace. \
             logical_path={}, root={}",
            logical_str, root_str
        );
    }

    #[test]
    fn test_is_path_within_genuine_outside_rejected() {
        // Two real sibling directories — one is NOT a subdirectory of the other.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("file.md"), "x").unwrap();

        let outside_file = outside.join("file.md").to_string_lossy().to_string();
        let root_str = root.to_string_lossy().to_string();

        assert!(
            !code_xray::is_path_within(&outside_file, &root_str),
            "Sibling-directory file must NOT be classified inside workspace. \
             outside_file={}, root={}",
            outside_file, root_str
        );
    }

    #[test]
    fn test_is_path_within_traversal_rejected() {
        // Path containing `..` segments that escape the workspace must be
        // rejected even when the textual prefix happens to match (the helper
        // forces a canonical fallback whenever `..` is present).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        // Real file outside root that the traversal resolves to.
        fs::write(tmp.path().join("escape.txt"), "x").unwrap();

        let traversal = sub.join("..").join("..").join("escape.txt");
        let traversal_str = traversal.to_string_lossy().to_string();
        let root_str = root.to_string_lossy().to_string();

        assert!(
            !code_xray::is_path_within(&traversal_str, &root_str),
            "Path traversal escaping workspace must be rejected. \
             traversal={}, root={}",
            traversal_str, root_str
        );
    }

    #[test]
    fn test_is_path_within_exact_root_accepted() {
        // The workspace root itself is considered "within" the workspace.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let root_str = root.to_string_lossy().to_string();

        assert!(
            code_xray::is_path_within(&root_str, &root_str),
            "Workspace root itself must be inside the workspace. root={}",
            root_str
        );
        // Also accept root with trailing slash.
        let with_slash = format!("{}/", root_str);
        assert!(
            code_xray::is_path_within(&with_slash, &root_str),
            "Workspace root with trailing slash must be inside the workspace."
        );
    }

