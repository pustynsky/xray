use super::*;
use serde_json::json;
use std::path::PathBuf;

/// Helper: create a HandlerContext with server_dir pointing to a temp directory.
fn make_ctx(dir: &std::path::Path) -> HandlerContext {
    HandlerContext {
        server_dir: dir.to_string_lossy().to_string(),
        ..HandlerContext::default()
    }
}

/// Helper: create a temp file with given content, return (dir, filename, full_path).
fn create_temp_file(content: &str) -> (tempfile::TempDir, String, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let filename = "test_file.txt";
    let path = tmp.path().join(filename);
    std::fs::write(&path, content).unwrap();
    (tmp, filename.to_string(), path)
}

/// Helper: create a temp file with a custom name, return full path.
fn create_named_temp_file(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

// ─── Mode A: Line-range operations ──────────────────────────────────

#[test]
fn test_mode_a_replace_single_line() {
    let (tmp, filename, path) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "replaced" }
        ]
    }));

    assert!(!result.is_error, "Expected success, got error: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("replaced"), "File should contain replaced line");
    assert!(content.contains("line1"), "Line 1 should be preserved");
    assert!(content.contains("line3"), "Line 3 should be preserved");
    assert!(!content.contains("line2"), "Line 2 should be replaced");
}

#[test]
fn test_mode_a_replace_range() {
    let (tmp, filename, path) = create_temp_file("line1\nline2\nline3\nline4\nline5\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 4, "content": "new_content" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("line1"));
    assert!(content.contains("new_content"));
    assert!(content.contains("line5"));
    assert!(!content.contains("line2"));
    assert!(!content.contains("line3"));
    assert!(!content.contains("line4"));
}

#[test]
fn test_mode_a_insert_before_line() {
    let (tmp, filename, path) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    // endLine < startLine = insert mode
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 1, "content": "inserted" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    assert_eq!(lines[0], "line1");
    assert_eq!(lines[1], "inserted");
    assert_eq!(lines[2], "line2");
    assert_eq!(lines[3], "line3");
}

#[test]
fn test_mode_a_delete_lines() {
    let (tmp, filename, path) = create_temp_file("line1\nline2\nline3\nline4\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 3, "content": "" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("line1"));
    assert!(content.contains("line4"));
    assert!(!content.contains("line2"));
    assert!(!content.contains("line3"));
}

#[test]
fn test_mode_a_multiple_operations_bottom_up() {
    let (tmp, filename, path) = create_temp_file("a\nb\nc\nd\ne\n");
    let ctx = make_ctx(tmp.path());

    // Replace line 4 and line 2 — should work regardless of order
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "B" },
            { "startLine": 4, "endLine": 4, "content": "D" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    assert_eq!(lines[0], "a");
    assert_eq!(lines[1], "B");
    assert_eq!(lines[2], "c");
    assert_eq!(lines[3], "D");
    assert_eq!(lines[4], "e");
}

#[test]
fn test_mode_a_overlap_error() {
    let (tmp, filename, _) = create_temp_file("a\nb\nc\nd\ne\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 4, "content": "X" },
            { "startLine": 3, "endLine": 5, "content": "Y" }
        ]
    }));

    assert!(result.is_error, "Overlapping operations should fail");
}

#[test]
fn test_mode_a_out_of_range_error() {
    let (tmp, filename, _) = create_temp_file("a\nb\nc\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 10, "endLine": 10, "content": "X" }
        ]
    }));

    assert!(result.is_error, "Out-of-range startLine should fail");
}

#[test]
fn test_mode_a_expected_line_count_mismatch() {
    let (tmp, filename, _) = create_temp_file("a\nb\nc\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 1, "endLine": 1, "content": "X" }
        ],
        "expectedLineCount": 100
    }));

    assert!(result.is_error, "expectedLineCount mismatch should fail");
    let text = &result.content[0].text;
    assert!(text.contains("Expected 100 lines"), "Error should mention expected count");
}

// ─── Mode B: Text-match edits ────────────────────────────────────────

#[test]
fn test_mode_b_literal_replace_all() {
    let (tmp, filename, path) = create_temp_file("foo bar foo baz foo\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "foo", "replace": "qux" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "qux bar qux baz qux\n");
}

#[test]
fn test_mode_b_literal_replace_specific_occurrence() {
    let (tmp, filename, path) = create_temp_file("foo bar foo baz foo\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "foo", "replace": "qux", "occurrence": 2 }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "foo bar qux baz foo\n");
}

#[test]
fn test_mode_b_regex_replace() {
    let (tmp, filename, path) = create_temp_file("count: 10\nmax: 20\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": r"\d+", "replace": "0" }
        ],
        "regex": true
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "count: 0\nmax: 0\n");
}

#[test]
fn test_mode_b_text_not_found_error() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "nonexistent", "replace": "x" }
        ]
    }));

    assert!(result.is_error, "Text not found should fail");
    let text = &result.content[0].text;
    assert!(text.contains("not found"), "Error should mention not found");
}

// ─── General tests ───────────────────────────────────────────────────

#[test]
fn test_file_not_found_error() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": "nonexistent.txt",
        "edits": [
            { "search": "x", "replace": "y" }
        ]
    }));

    assert!(result.is_error, "Nonexistent file should fail");
    let text = &result.content[0].text;
    assert!(text.contains("not found"), "Error should mention not found");
}

#[test]
fn test_both_operations_and_edits_error() {
    let (tmp, filename, _) = create_temp_file("hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [{ "startLine": 1, "endLine": 1, "content": "X" }],
        "edits": [{ "search": "hello", "replace": "bye" }]
    }));

    assert!(result.is_error, "Both operations and edits should fail");
    let text = &result.content[0].text;
    assert!(text.contains("not both"), "Error should mention 'not both'");
}

#[test]
fn test_neither_operations_nor_edits_error() {
    let (tmp, filename, _) = create_temp_file("hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename
    }));

    assert!(result.is_error, "Neither operations nor edits should fail");
}

#[test]
fn test_dry_run_does_not_write() {
    let (tmp, filename, path) = create_temp_file("original content\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "original", "replace": "modified" }
        ],
        "dryRun": true
    }));

    assert!(!result.is_error);
    // File should NOT be modified
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "original content\n", "dryRun should not modify file");

    // Response should contain diff
    let text = &result.content[0].text;
    assert!(text.contains("diff"), "dryRun should return diff");
    assert!(text.contains("dryRun"), "Response should mention dryRun");
}

#[test]
fn test_unified_diff_format() {
    let (tmp, filename, _) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "REPLACED" }
        ],
        "dryRun": true
    }));

    assert!(!result.is_error);
    let text = &result.content[0].text;
    // Unified diff should contain --- and +++ headers
    assert!(text.contains("a/"), "Diff should have a/ header");
    assert!(text.contains("b/"), "Diff should have b/ header");
}

#[test]
fn test_crlf_preservation() {
    let (tmp, filename, path) = create_temp_file("line1\r\nline2\r\nline3\r\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "REPLACED" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    // Should preserve CRLF line endings
    assert!(content.contains("\r\n"), "CRLF should be preserved");
    assert!(content.contains("REPLACED\r\n"), "Replaced line should have CRLF ending");
}

#[test]
fn test_empty_file() {
    let (tmp, filename, path) = create_temp_file("");
    let ctx = make_ctx(tmp.path());

    // Insert into empty file
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 1, "endLine": 0, "content": "new content" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("new content"));
}

#[test]
fn test_single_line_file() {
    let (tmp, filename, path) = create_temp_file("only line");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 1, "endLine": 1, "content": "replaced line" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "replaced line");
}

#[test]
fn test_missing_path_error() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "edits": [{ "search": "x", "replace": "y" }]
    }));

    assert!(result.is_error, "Missing path should fail");
    let text = &result.content[0].text;
    assert!(text.contains("path"), "Error should mention 'path'");
}

#[test]
fn test_mode_b_occurrence_out_of_range() {
    let (tmp, filename, _) = create_temp_file("foo bar foo\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "foo", "replace": "qux", "occurrence": 5 }
        ]
    }));

    assert!(result.is_error, "Occurrence beyond count should fail");
}

#[test]
fn test_response_contains_stats() {
    let (tmp, filename, _) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "X\nY" }
        ]
    }));

    assert!(!result.is_error);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["applied"], 1);
    assert!(parsed["newLineCount"].as_u64().unwrap() > 0);
    assert!(parsed["diff"].as_str().is_some());
}

#[test]
fn test_absolute_path_works() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("abs_test.txt");
    std::fs::write(&path, "hello\n").unwrap();

    // Use a different server_dir to confirm absolute path bypasses it
    let ctx = make_ctx(std::path::Path::new("."));

    let result = handle_search_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [
            { "search": "hello", "replace": "world" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "world\n");
}

#[test]
fn test_directory_path_error() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": tmp.path().to_string_lossy(),
        "edits": [
            { "search": "x", "replace": "y" }
        ]
    }));

    assert!(result.is_error, "Directory path should fail");
    let text = &result.content[0].text;
    assert!(text.contains("directory"), "Error should mention directory");
}

#[test]
fn test_mode_a_multiline_content_replace() {
    let (tmp, filename, path) = create_temp_file("a\nb\nc\nd\ne\n");
    let ctx = make_ctx(tmp.path());

    // Replace single line with multiple lines
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "x\ny\nz" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    assert_eq!(lines[0], "a");
    assert_eq!(lines[1], "x");
    assert_eq!(lines[2], "y");
    assert_eq!(lines[3], "z");
    assert_eq!(lines[4], "c");
}

#[test]
fn test_mode_b_multiple_edits_sequential() {
    let (tmp, filename, path) = create_temp_file("int x = 10;\nint y = 20;\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "10", "replace": "100" },
            { "search": "20", "replace": "200" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "int x = 100;\nint y = 200;\n");
}
// ─── Additional edge-case tests ──────────────────────────────────────

#[test]
fn test_mode_b_regex_capture_groups() {
    let (tmp, filename, path) = create_temp_file("func getData() {}\nfunc setData() {}\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": r"func (\w+)\(\)", "replace": "fn $1()" }
        ],
        "regex": true
    }));

    assert!(!result.is_error, "Regex capture groups should work: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("fn getData()"), "Capture group $1 should be substituted");
    assert!(content.contains("fn setData()"), "Second match should also be substituted");
    assert!(!content.contains("func"), "Original 'func' should be replaced");
}

#[test]
fn test_mode_a_insert_at_end_of_file() {
    let (tmp, filename, path) = create_temp_file("line1\nline2\n");
    let ctx = make_ctx(tmp.path());

    // Insert after the last line (startLine = 4 because split('\n') on "line1\nline2\n" gives ["line1", "line2", ""])
    // Actually, for a file "line1\nline2\n", split('\n') gives ["line1", "line2", ""] — 3 elements
    // Insert at position 4 (after element 3) = append
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 4, "endLine": 3, "content": "appended" }
        ]
    }));

    assert!(!result.is_error, "Insert at end should work: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("appended"), "Content should be appended");
}

#[test]
fn test_mode_a_replace_last_line() {
    let (tmp, filename, path) = create_temp_file("first\nsecond\nthird");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 3, "endLine": 3, "content": "LAST" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "first\nsecond\nLAST");
}

#[test]
fn test_mode_b_no_changes_when_same_text() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello", "replace": "hello" }
        ]
    }));

    assert!(!result.is_error);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["diff"], "(no changes)", "Same text should produce no diff");
}

#[test]
fn test_mode_b_multiline_search_replace() {
    let (tmp, filename, path) = create_temp_file("start\nold_line1\nold_line2\nend\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "old_line1\nold_line2", "replace": "new_block" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("new_block"), "Multi-line search should be replaced");
    assert!(!content.contains("old_line1"), "Old content should be gone");
    assert!(content.contains("start\n"), "Content before should be preserved");
    assert!(content.contains("end\n"), "Content after should be preserved");
}

#[test]
fn test_large_file_smoke() {
    let mut content = String::new();
    for i in 1..=200 {
        content.push_str(&format!("line {}\n", i));
    }
    let (tmp, filename, path) = create_temp_file(&content);
    let ctx = make_ctx(tmp.path());

    // Replace line 100 and line 150 (bottom-up)
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 100, "endLine": 100, "content": "REPLACED_100" },
            { "startLine": 150, "endLine": 150, "content": "REPLACED_150" }
        ]
    }));

    assert!(!result.is_error);
    let result_content = std::fs::read_to_string(&path).unwrap();
    assert!(result_content.contains("REPLACED_100"), "Line 100 should be replaced");
    assert!(result_content.contains("REPLACED_150"), "Line 150 should be replaced");
    assert!(result_content.contains("line 99\n"), "Line 99 should be preserved");
    assert!(result_content.contains("line 101\n"), "Line 101 should be preserved");
}

#[test]
fn test_mode_a_expected_line_count_match() {
    let (tmp, filename, path) = create_temp_file("a\nb\nc\n");
    let ctx = make_ctx(tmp.path());

    // "a\nb\nc\n" split by '\n' gives ["a", "b", "c", ""] = 4 elements
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "B" }
        ],
        "expectedLineCount": 4
    }));

    assert!(!result.is_error, "Correct expectedLineCount should pass");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("B"), "Edit should be applied");
}

#[test]
fn test_mode_b_empty_search_error() {
    let (tmp, filename, _) = create_temp_file("hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "", "replace": "x" }
        ]
    }));

    assert!(result.is_error, "Empty search string should fail");
    let text = &result.content[0].text;
    assert!(text.contains("empty"), "Error should mention empty search");
}

// ═══════════════════════════════════════════════════════════════════════
// Multi-file tests (Phase 1)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_multi_file_all_succeed() {
    let tmp = tempfile::tempdir().unwrap();
    let path1 = create_named_temp_file(tmp.path(), "file1.txt", "old text here\n");
    let path2 = create_named_temp_file(tmp.path(), "file2.txt", "old text there\n");
    let path3 = create_named_temp_file(tmp.path(), "file3.txt", "old text everywhere\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "paths": ["file1.txt", "file2.txt", "file3.txt"],
        "edits": [
            { "search": "old", "replace": "new" }
        ]
    }));

    assert!(!result.is_error, "Multi-file edit should succeed: {:?}", result);

    // All files should be modified
    assert_eq!(std::fs::read_to_string(&path1).unwrap(), "new text here\n");
    assert_eq!(std::fs::read_to_string(&path2).unwrap(), "new text there\n");
    assert_eq!(std::fs::read_to_string(&path3).unwrap(), "new text everywhere\n");

    // Response should have results array and summary
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["summary"]["filesEdited"], 3);
    assert_eq!(parsed["summary"]["totalApplied"], 3);
    assert_eq!(parsed["results"].as_array().unwrap().len(), 3);
}

#[test]
fn test_multi_file_one_fails_aborts_all() {
    let tmp = tempfile::tempdir().unwrap();
    let path1 = create_named_temp_file(tmp.path(), "good1.txt", "old text\n");
    let _path2 = create_named_temp_file(tmp.path(), "good2.txt", "no match here\n");
    let ctx = make_ctx(tmp.path());

    // file2 doesn't contain "old" → edit fails → ALL files should be unchanged
    let result = handle_search_edit(&ctx, &json!({
        "paths": ["good1.txt", "good2.txt"],
        "edits": [
            { "search": "old", "replace": "new" }
        ]
    }));

    assert!(result.is_error, "Should fail when one file has no match");

    // CRITICAL: file1 should NOT be modified (transactional abort)
    assert_eq!(std::fs::read_to_string(&path1).unwrap(), "old text\n",
        "File1 should be unchanged after transactional abort");
}

#[test]
fn test_multi_file_dry_run() {
    let tmp = tempfile::tempdir().unwrap();
    let path1 = create_named_temp_file(tmp.path(), "dry1.txt", "hello world\n");
    let path2 = create_named_temp_file(tmp.path(), "dry2.txt", "hello there\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "paths": ["dry1.txt", "dry2.txt"],
        "edits": [
            { "search": "hello", "replace": "goodbye" }
        ],
        "dryRun": true
    }));

    assert!(!result.is_error, "dryRun should succeed: {:?}", result);

    // Files should NOT be modified
    assert_eq!(std::fs::read_to_string(&path1).unwrap(), "hello world\n");
    assert_eq!(std::fs::read_to_string(&path2).unwrap(), "hello there\n");

    // Response should have dryRun = true
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["summary"]["dryRun"], true);
}

#[test]
fn test_multi_file_max_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    // Create 21 paths (over the 20 limit)
    let paths: Vec<String> = (0..21).map(|i| format!("file{}.txt", i)).collect();

    let result = handle_search_edit(&ctx, &json!({
        "paths": paths,
        "edits": [
            { "search": "x", "replace": "y" }
        ]
    }));

    assert!(result.is_error, "Should fail with >20 files");
    let text = &result.content[0].text;
    assert!(text.contains("maximum"), "Error should mention maximum");
}

#[test]
fn test_multi_file_mutual_exclusive_with_path() {
    let (tmp, filename, _) = create_temp_file("hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "paths": [filename],
        "edits": [
            { "search": "hello", "replace": "bye" }
        ]
    }));

    assert!(result.is_error, "path + paths should fail");
    let text = &result.content[0].text;
    assert!(text.contains("not both"), "Error should mention mutual exclusivity");
}

#[test]
fn test_multi_file_empty_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "paths": [],
        "edits": [
            { "search": "x", "replace": "y" }
        ]
    }));

    assert!(result.is_error, "Empty paths array should fail");
    let text = &result.content[0].text;
    assert!(text.contains("empty"), "Error should mention empty");
}

#[test]
fn test_multi_file_file_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    create_named_temp_file(tmp.path(), "exists.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "paths": ["exists.txt", "missing.txt"],
        "edits": [
            { "search": "hello", "replace": "bye" }
        ]
    }));

    assert!(result.is_error, "Missing file in paths should fail");
    let text = &result.content[0].text;
    assert!(text.contains("not found"), "Error should mention not found");
}

// ═══════════════════════════════════════════════════════════════════════
// Insert after/before tests (Phase 2)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_insert_after_found() {
    let (tmp, filename, path) = create_temp_file("using System;\nusing System.IO;\n\nclass Foo {}\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertAfter": "using System.IO;",
                "content": "using System.Linq;"
            }
        ]
    }));

    assert!(!result.is_error, "Insert after should succeed: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    assert_eq!(lines[0], "using System;");
    assert_eq!(lines[1], "using System.IO;");
    assert_eq!(lines[2], "using System.Linq;");
    assert_eq!(lines[3], "");
    assert_eq!(lines[4], "class Foo {}");
}

#[test]
fn test_insert_before_found() {
    let (tmp, filename, path) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertBefore": "line2",
                "content": "inserted_before"
            }
        ]
    }));

    assert!(!result.is_error, "Insert before should succeed: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    assert_eq!(lines[0], "line1");
    assert_eq!(lines[1], "inserted_before");
    assert_eq!(lines[2], "line2");
    assert_eq!(lines[3], "line3");
}

#[test]
fn test_insert_after_not_found() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertAfter": "nonexistent anchor",
                "content": "new line"
            }
        ]
    }));

    assert!(result.is_error, "Insert after non-existent anchor should fail");
    let text = &result.content[0].text;
    assert!(text.contains("not found"), "Error should mention not found");
}

#[test]
fn test_insert_after_specific_occurrence() {
    let (tmp, filename, path) = create_temp_file("marker\nother\nmarker\nend\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertAfter": "marker",
                "content": "INSERTED",
                "occurrence": 2
            }
        ]
    }));

    assert!(!result.is_error, "Insert after 2nd occurrence should succeed: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    // First "marker" should NOT have insertion after it
    assert_eq!(lines[0], "marker");
    assert_eq!(lines[1], "other");
    // Second "marker" should have insertion after it
    assert_eq!(lines[2], "marker");
    assert_eq!(lines[3], "INSERTED");
    assert_eq!(lines[4], "end");
}

#[test]
fn test_insert_after_with_search_replace_error() {
    let (tmp, filename, _) = create_temp_file("hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "hello",
                "replace": "bye",
                "insertAfter": "hello",
                "content": "new"
            }
        ]
    }));

    assert!(result.is_error, "search/replace + insertAfter should fail");
    let text = &result.content[0].text;
    assert!(text.contains("mutually exclusive"), "Error should mention mutual exclusivity");
}

#[test]
fn test_insert_after_missing_content_error() {
    let (tmp, filename, _) = create_temp_file("hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertAfter": "hello"
            }
        ]
    }));

    assert!(result.is_error, "insertAfter without content should fail");
    let text = &result.content[0].text;
    assert!(text.contains("content"), "Error should mention missing content");
}

#[test]
fn test_insert_before_and_after_error() {
    let (tmp, filename, _) = create_temp_file("hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertBefore": "hello",
                "insertAfter": "hello",
                "content": "new"
            }
        ]
    }));

    assert!(result.is_error, "insertBefore + insertAfter should fail");
    let text = &result.content[0].text;
    assert!(text.contains("mutually exclusive"), "Error should mention mutual exclusivity");
}

#[test]
fn test_insert_after_at_last_line() {
    let (tmp, filename, path) = create_temp_file("first\nlast");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertAfter": "last",
                "content": "appended"
            }
        ]
    }));

    assert!(!result.is_error, "Insert after last line should succeed: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("last\nappended"), "Content should be appended after last line");
}

#[test]
fn test_insert_before_at_first_line() {
    let (tmp, filename, path) = create_temp_file("first line\nsecond line\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertBefore": "first line",
                "content": "header"
            }
        ]
    }));

    assert!(!result.is_error, "Insert before first line should succeed: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    assert_eq!(lines[0], "header");
    assert_eq!(lines[1], "first line");
    assert_eq!(lines[2], "second line");
}

// ═══════════════════════════════════════════════════════════════════════
// expectedContext tests (Phase 3)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_expected_context_match() {
    let (tmp, filename, path) = create_temp_file("var semaphore = new SemaphoreSlim(10);\nDoWork();\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "SemaphoreSlim(10)",
                "replace": "SemaphoreSlim(30)",
                "expectedContext": "var semaphore = new"
            }
        ]
    }));

    assert!(!result.is_error, "expectedContext match should succeed: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("SemaphoreSlim(30)"), "Edit should be applied");
}

#[test]
fn test_expected_context_mismatch() {
    let (tmp, filename, _) = create_temp_file("var semaphore = new SemaphoreSlim(10);\nDoWork();\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "SemaphoreSlim(10)",
                "replace": "SemaphoreSlim(30)",
                "expectedContext": "this context does not exist"
            }
        ]
    }));

    assert!(result.is_error, "expectedContext mismatch should fail");
    let text = &result.content[0].text;
    assert!(text.contains("Expected context"), "Error should mention expected context");
}

#[test]
fn test_expected_context_optional() {
    let (tmp, filename, path) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    // No expectedContext → should work as before
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "hello",
                "replace": "goodbye"
            }
        ]
    }));

    assert!(!result.is_error, "Without expectedContext should work normally");
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "goodbye world\n");
}

#[test]
fn test_expected_context_with_insert_after() {
    let (tmp, filename, path) = create_temp_file("using System;\nusing System.IO;\n\nclass Foo {}\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertAfter": "using System.IO;",
                "content": "using System.Linq;",
                "expectedContext": "using System;"
            }
        ]
    }));

    assert!(!result.is_error, "expectedContext with insertAfter should succeed: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("using System.Linq;"), "Insert should work with context check");
}

#[test]
fn test_expected_context_with_insert_after_mismatch() {
    let (tmp, filename, _) = create_temp_file("using System;\nusing System.IO;\n\nclass Foo {}\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "insertAfter": "using System.IO;",
                "content": "using System.Linq;",
                "expectedContext": "wrong context text"
            }
        ]
    }));

    assert!(result.is_error, "expectedContext mismatch with insertAfter should fail");
}


// ═══════════════════════════════════════════════════════════════════════
// skipIfNotFound tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_skip_if_not_found_single_file() {
    let (tmp, filename, path) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    // Text not found, but skipIfNotFound=true → should succeed without changing file
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "nonexistent", "replace": "x", "skipIfNotFound": true }
        ]
    }));

    assert!(!result.is_error, "skipIfNotFound should not error: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "hello world\n", "File should be unchanged");
}

#[test]
fn test_skip_if_not_found_false_still_errors() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    // skipIfNotFound=false (default) → should error
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "nonexistent", "replace": "x", "skipIfNotFound": false }
        ]
    }));

    assert!(result.is_error, "skipIfNotFound=false should error");
}

#[test]
fn test_skip_if_not_found_default_is_false() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    // No skipIfNotFound → default is false → should error
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "nonexistent", "replace": "x" }
        ]
    }));

    assert!(result.is_error, "Default (no skipIfNotFound) should error");
}

#[test]
fn test_skip_if_not_found_multi_file_partial_match() {
    let tmp = tempfile::tempdir().unwrap();
    let path1 = create_named_temp_file(tmp.path(), "has_it.txt", "old text here\n");
    let path2 = create_named_temp_file(tmp.path(), "no_it.txt", "different content\n");
    let ctx = make_ctx(tmp.path());

    // file1 has "old", file2 doesn't → with skipIfNotFound=true, both should succeed
    let result = handle_search_edit(&ctx, &json!({
        "paths": ["has_it.txt", "no_it.txt"],
        "edits": [
            { "search": "old", "replace": "new", "skipIfNotFound": true }
        ]
    }));

    assert!(!result.is_error, "skipIfNotFound multi-file should succeed: {:?}", result);

    // file1 should be modified
    assert_eq!(std::fs::read_to_string(&path1).unwrap(), "new text here\n");
    // file2 should be unchanged
    assert_eq!(std::fs::read_to_string(&path2).unwrap(), "different content\n");
}

#[test]
fn test_skip_if_not_found_multi_file_without_flag_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let path1 = create_named_temp_file(tmp.path(), "has_it.txt", "old text here\n");
    let _path2 = create_named_temp_file(tmp.path(), "no_it.txt", "different content\n");
    let ctx = make_ctx(tmp.path());

    // file2 doesn't have "old" → without skipIfNotFound, should fail (transactional abort)
    let result = handle_search_edit(&ctx, &json!({
        "paths": ["has_it.txt", "no_it.txt"],
        "edits": [
            { "search": "old", "replace": "new" }
        ]
    }));

    assert!(result.is_error, "Without skipIfNotFound, multi-file should fail");
    // file1 should NOT be modified (transactional abort)
    assert_eq!(std::fs::read_to_string(&path1).unwrap(), "old text here\n");
}

#[test]
fn test_skip_if_not_found_insert_after_anchor_missing() {
    let (tmp, filename, path) = create_temp_file("line1\nline2\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertAfter": "nonexistent anchor", "content": "new line", "skipIfNotFound": true }
        ]
    }));

    assert!(!result.is_error, "skipIfNotFound with insertAfter should not error: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "line1\nline2\n", "File should be unchanged");
}

#[test]
fn test_skip_if_not_found_regex_pattern_missing() {
    let (tmp, filename, path) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "nonexistent\\d+", "replace": "x", "skipIfNotFound": true }
        ],
        "regex": true
    }));

    assert!(!result.is_error, "skipIfNotFound with regex should not error: {:?}", result);
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "hello world\n", "File should be unchanged");
}
