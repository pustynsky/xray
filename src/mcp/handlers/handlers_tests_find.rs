//! Tests for search_find -- extracted from handlers_tests.rs.

use super::*;
use super::handlers_test_utils::cleanup_tmp;
use crate::ContentIndex;
use std::sync::{Arc, RwLock};
/// T43-T45 — search_find combined parameters: countOnly, maxDepth, ignoreCase, regex.
#[test]
fn test_search_find_combined_parameters() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_find_combined_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let level1 = tmp_dir.join("level1");
    let level2 = level1.join("level2");
    std::fs::create_dir_all(&level2).unwrap();
    { let mut f = std::fs::File::create(level2.join("deep.cs")).unwrap(); writeln!(f, "// deep").unwrap(); }
    { let mut f = std::fs::File::create(level1.join("shallow.cs")).unwrap(); writeln!(f, "// shallow").unwrap(); }
    { let mut f = std::fs::File::create(tmp_dir.join("TopFile.CS")).unwrap(); writeln!(f, "// top").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let content_index = ContentIndex {
        root: dir_str.clone(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: None,
        server_dir: dir_str.clone(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    let result_count = dispatch_tool(&ctx, "search_find", &json!({
        "pattern": ".cs",
        "countOnly": true,
        "ignoreCase": true
    }));
    assert!(!result_count.is_error, "countOnly should not error: {}", result_count.content[0].text);
    let output_count: Value = serde_json::from_str(&result_count.content[0].text).unwrap();
    assert!(output_count["summary"]["totalMatches"].as_u64().unwrap() >= 3,
        "Should find at least 3 .cs files (case-insensitive)");
    assert!(output_count["files"].as_array().unwrap().is_empty(),
        "countOnly=true should return empty files array");

    let result_depth = dispatch_tool(&ctx, "search_find", &json!({
        "pattern": ".cs",
        "maxDepth": 1,
        "ignoreCase": true
    }));
    assert!(!result_depth.is_error);
    let output_depth: Value = serde_json::from_str(&result_depth.content[0].text).unwrap();
    let depth_matches = output_depth["summary"]["totalMatches"].as_u64().unwrap();
    assert!(depth_matches < 3,
        "maxDepth=1 should find fewer than 3 files, got {}", depth_matches);

    let result_regex = dispatch_tool(&ctx, "search_find", &json!({
        "pattern": "top.*\\.cs",
        "regex": true,
        "ignoreCase": true
    }));
    assert!(!result_regex.is_error, "regex+ignoreCase should not error: {}", result_regex.content[0].text);
    let output_regex: Value = serde_json::from_str(&result_regex.content[0].text).unwrap();
    assert!(output_regex["summary"]["totalMatches"].as_u64().unwrap() >= 1,
        "Case-insensitive regex 'top.*\\.cs' should match TopFile.CS");

    cleanup_tmp(&tmp_dir);
}
// ─── search_find contents=true tests ─────────────────────────────────

#[test]
fn test_search_find_contents_mode() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_find_contents_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create files with distinct content
    {
        let mut f = std::fs::File::create(tmp_dir.join("alpha.txt")).unwrap();
        writeln!(f, "This file contains the magic_searchable_token here.").unwrap();
        writeln!(f, "And a second line with more content.").unwrap();
    }
    {
        let mut f = std::fs::File::create(tmp_dir.join("beta.txt")).unwrap();
        writeln!(f, "This file has completely different content.").unwrap();
        writeln!(f, "No special tokens at all.").unwrap();
    }
    {
        let mut f = std::fs::File::create(tmp_dir.join("gamma.txt")).unwrap();
        writeln!(f, "Another file that also has magic_searchable_token inside.").unwrap();
    }
    {
        let mut f = std::fs::File::create(tmp_dir.join("delta.cs")).unwrap();
        writeln!(f, "// A C# file without the search term").unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let content_index = ContentIndex {
        root: dir_str.clone(),
        extensions: vec!["txt".to_string()],
        ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: None,
        server_dir: dir_str.clone(),
        server_ext: "txt".to_string(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    // Search file contents for "magic_searchable_token" in .txt files
    let result = dispatch_tool(&ctx, "search_find", &json!({
        "pattern": "magic_searchable_token",
        "contents": true,
        "ext": "txt",
        "dir": dir_str
    }));
    assert!(!result.is_error, "search_find contents=true should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Should find exactly 2 files (alpha.txt and gamma.txt)
    let total_matches = output["summary"]["totalMatches"].as_u64().unwrap();
    assert_eq!(total_matches, 2, "Should find exactly 2 files containing the token, got {}", total_matches);

    // Verify the files array has 2 entries with match details
    let files = output["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "files array should have 2 entries");

    // Each matched file should have a "matches" array with line-level details
    for file_entry in files {
        assert!(file_entry["path"].is_string(), "Each result should have a path");
        let matches = file_entry["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "Each matched file should have at least one matching line");
        for m in matches {
            assert!(m["line"].is_u64(), "Each match should have a line number");
            assert!(m["text"].is_string(), "Each match should have text");
            let text = m["text"].as_str().unwrap();
            assert!(text.contains("magic_searchable_token"),
                "Matched line text should contain the search term, got: {}", text);
        }
    }

    // Verify beta.txt is NOT in results (it doesn't contain the token)
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
    for path in &paths {
        assert!(!path.contains("beta"), "beta.txt should not be in results (no match)");
        assert!(!path.contains("delta"), "delta.cs should not be in results (wrong extension)");
    }

    // Test countOnly=true with contents search
    let result_count = dispatch_tool(&ctx, "search_find", &json!({
        "pattern": "magic_searchable_token",
        "contents": true,
        "ext": "txt",
        "dir": dir_str,
        "countOnly": true
    }));
    assert!(!result_count.is_error);
    let output_count: Value = serde_json::from_str(&result_count.content[0].text).unwrap();
    assert_eq!(output_count["summary"]["totalMatches"].as_u64().unwrap(), 2,
        "countOnly should still report 2 matches");
    assert!(output_count["files"].as_array().unwrap().is_empty(),
        "countOnly=true should return empty files array");

    cleanup_tmp(&tmp_dir);
}
