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


#[test]
fn test_skip_if_not_found_response_contains_skipped_edits_field() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "nonexistent", "replace": "x", "skipIfNotFound": true }
        ]
    }));

    assert!(!result.is_error, "Should succeed: {:?}", result);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["skippedEdits"], 1, "Response should contain skippedEdits: 1");
    assert_eq!(parsed["diff"], "(no changes)", "Diff should show no changes");
}

#[test]
fn test_skip_if_not_found_multi_file_response_shows_skipped_per_file() {
    let tmp = tempfile::tempdir().unwrap();
    let _path1 = create_named_temp_file(tmp.path(), "has_it.txt", "old text here\n");
    let _path2 = create_named_temp_file(tmp.path(), "no_it.txt", "different content\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "paths": ["has_it.txt", "no_it.txt"],
        "edits": [
            { "search": "old", "replace": "new", "skipIfNotFound": true }
        ]
    }));

    assert!(!result.is_error, "Should succeed: {:?}", result);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();

    // File that had the text → no skippedEdits field
    let results = parsed["results"].as_array().unwrap();
    let file1 = &results[0];
    assert!(file1.get("skippedEdits").is_none() || file1["skippedEdits"] == 0,
        "File with match should not have skippedEdits");

    // File that didn't have the text → skippedEdits: 1
    let file2 = &results[1];
    assert_eq!(file2["skippedEdits"], 1, "File without match should have skippedEdits: 1");
    assert_eq!(file2["diff"], "(no changes)", "Skipped file should show no changes");
}


// ═══════════════════════════════════════════════════════════════════════
// Nearest match hint tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_nearest_match_hint_different_quotes() {
    // File has «quotes» but search uses "quotes" — should show nearest match
    let (tmp, filename, _) = create_temp_file("line one\nДевять «израильтян» погибли\nline three\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "Девять \"израильтян\" погибли", "replace": "replaced" }
        ]
    }));

    assert!(result.is_error, "Should fail when text not found");
    let text = &result.content[0].text;
    assert!(text.contains("Nearest match"), "Error should contain nearest match hint");
    assert!(text.contains("line 2"), "Hint should show correct line number");
    assert!(text.contains("similarity"), "Hint should show similarity percentage");
}

#[test]
fn test_nearest_match_hint_partial_overlap() {
    let (tmp, filename, _) = create_temp_file("function processData() {\n    return data;\n}\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "function processdata() {", "replace": "fn processData() {" }
        ]
    }));

    assert!(result.is_error);
    let text = &result.content[0].text;
    // Should find the similar line (case difference)
    assert!(text.contains("Nearest match"), "Should show nearest match for near-miss");
}

#[test]
fn test_nearest_match_hint_no_good_match() {
    let (tmp, filename, _) = create_temp_file("abc\ndef\nghi\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "zzzzzzzzzzzzzzzzz completely different", "replace": "x" }
        ]
    }));

    assert!(result.is_error);
    let text = &result.content[0].text;
    // Similarity should be too low → no hint
    assert!(!text.contains("Nearest match"), "Should NOT show hint for very low similarity");
}

#[test]
fn test_nearest_match_hint_multiline_search() {
    let (tmp, filename, _) = create_temp_file("line1\nfunction oldName() {\n    return 42;\n}\nline5\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "function oldname() {\n    return 42;\n}", "replace": "replaced" }
        ]
    }));

    assert!(result.is_error);
    let text = &result.content[0].text;
    // Multi-line sliding window should find the matching block
    assert!(text.contains("Nearest match"), "Should find nearest match for multiline search");
    assert!(text.contains("line 2"), "Should identify the correct starting line");
}

#[test]
fn test_nearest_match_hint_anchor_not_found() {
    let (tmp, filename, _) = create_temp_file("using System;\nusing System.IO;\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertAfter": "using System.Io;", "content": "using System.Linq;" }
        ]
    }));

    assert!(result.is_error);
    let text = &result.content[0].text;
    // "System.Io" vs "System.IO" — should find nearest match
    assert!(text.contains("Nearest match"), "Anchor not found should show nearest match hint");
}

#[test]
fn test_nearest_match_hint_regex_not_found() {
    let (tmp, filename, _) = create_temp_file("int count = 10;\nmax = 20;\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "xyzzy\\d+", "replace": "0" }
        ],
        "regex": true
    }));

    assert!(result.is_error);
    let text = &result.content[0].text;
    // regex "xyzzy\d+" won't match, but nearest_match_hint uses it as literal text
    // The hint may or may not fire depending on similarity, but the error should contain "Pattern not found"
    assert!(text.contains("Pattern not found"), "Should say pattern not found");
}

// ═══════════════════════════════════════════════════════════════════════
// skippedDetails tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_skipped_details_contains_edit_info() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "nonexistent_text", "replace": "x", "skipIfNotFound": true }
        ]
    }));

    assert!(!result.is_error, "Should succeed: {:?}", result);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();

    // skippedEdits count should be 1
    assert_eq!(parsed["skippedEdits"], 1);

    // skippedDetails should be an array with 1 entry
    let details = parsed["skippedDetails"].as_array()
        .expect("skippedDetails should be an array");
    assert_eq!(details.len(), 1);

    let detail = &details[0];
    assert_eq!(detail["editIndex"], 0, "editIndex should be 0");
    assert_eq!(detail["search"], "nonexistent_text", "search text should be preserved");
    assert_eq!(detail["reason"], "text not found", "reason should describe the issue");
}

#[test]
fn test_skipped_details_multiple_skips() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "missing_one", "replace": "x", "skipIfNotFound": true },
            { "search": "hello", "replace": "goodbye" },
            { "search": "missing_two", "replace": "y", "skipIfNotFound": true },
            { "insertAfter": "missing_anchor", "content": "new", "skipIfNotFound": true }
        ]
    }));

    assert!(!result.is_error, "Should succeed with skipped edits: {:?}", result);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();

    // 3 edits skipped (indexes 0, 2, 3), 1 applied (index 1)
    assert_eq!(parsed["skippedEdits"], 3, "Should report 3 skipped edits");

    let details = parsed["skippedDetails"].as_array()
        .expect("skippedDetails should be an array");
    assert_eq!(details.len(), 3);

    // Check each skipped edit
    assert_eq!(details[0]["editIndex"], 0);
    assert_eq!(details[0]["search"], "missing_one");
    assert_eq!(details[0]["reason"], "text not found");

    assert_eq!(details[1]["editIndex"], 2);
    assert_eq!(details[1]["search"], "missing_two");
    assert_eq!(details[1]["reason"], "text not found");

    assert_eq!(details[2]["editIndex"], 3);
    assert_eq!(details[2]["search"], "missing_anchor");
    assert_eq!(details[2]["reason"], "anchor text not found");
}

#[test]
fn test_skipped_details_multi_file_per_file() {
    let tmp = tempfile::tempdir().unwrap();
    let _path1 = create_named_temp_file(tmp.path(), "has_it.txt", "old text here\n");
    let _path2 = create_named_temp_file(tmp.path(), "no_it.txt", "different content\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "paths": ["has_it.txt", "no_it.txt"],
        "edits": [
            { "search": "old", "replace": "new", "skipIfNotFound": true }
        ]
    }));

    assert!(!result.is_error, "Should succeed: {:?}", result);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();

    let results = parsed["results"].as_array().unwrap();

    // File1: text found, no skipped details
    let file1 = &results[0];
    assert!(file1.get("skippedDetails").is_none(),
        "File with match should not have skippedDetails");

    // File2: text not found, skipped details present
    let file2 = &results[1];
    assert_eq!(file2["skippedEdits"], 1);
    let details = file2["skippedDetails"].as_array().unwrap();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["editIndex"], 0);
    assert_eq!(details[0]["search"], "old");
    assert_eq!(details[0]["reason"], "text not found");
}

#[test]
fn test_skipped_details_regex_skip() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "nonexistent\\d+", "replace": "x", "skipIfNotFound": true }
        ],
        "regex": true
    }));

    assert!(!result.is_error, "Should succeed: {:?}", result);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();

    let details = parsed["skippedDetails"].as_array().unwrap();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["reason"], "regex pattern not found");
}

// ═══════════════════════════════════════════════════════════════════════
// Sequential edit occurrence hint tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_sequential_edit_hint_when_previous_edit_reduces_occurrences() {
    let (tmp, filename, _) = create_temp_file("foo bar foo baz foo\n");
    let ctx = make_ctx(tmp.path());

    // First edit replaces first "foo" with "qux", leaving 2 "foo"s.
    // Second edit requests occurrence=3 of "foo" — only 2 remain.
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "foo", "replace": "qux", "occurrence": 1 },
            { "search": "foo", "replace": "qux", "occurrence": 3 }
        ]
    }));

    assert!(result.is_error, "Should fail when occurrence exceeds count after prior edits");
    let text = &result.content[0].text;
    assert!(text.contains("sequentially"),
        "Error should mention sequential application when edit_index > 0. Got: {}", text);
}

#[test]
fn test_no_sequential_hint_for_first_edit() {
    let (tmp, filename, _) = create_temp_file("foo bar\n");
    let ctx = make_ctx(tmp.path());

    // First edit (index 0) requests occurrence=5 but only 1 exists.
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "foo", "replace": "qux", "occurrence": 5 }
        ]
    }));

    assert!(result.is_error, "Should fail when occurrence exceeds count");
    let text = &result.content[0].text;
    assert!(!text.contains("sequentially"),
        "Error should NOT mention sequential when edit_index == 0. Got: {}", text);
}


// ─── Part A: CRLF normalization in search text ──────────────────────

#[test]
fn test_crlf_in_search_text_is_normalized() {
    // File has LF line endings (normalized by read_and_validate_file)
    let (tmp, filename, _) = create_temp_file("line one\nline two\nline three\n");
    let ctx = make_ctx(tmp.path());

    // Search text uses CRLF — should still match after normalization
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "line one\r\nline two", "replace": "LINE ONE\nLINE TWO" }
        ]
    }));

    assert!(!result.is_error, "CRLF in search text should be normalized to match LF file. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("LINE ONE\nLINE TWO"), "Replacement should have been applied");
}

#[test]
fn test_crlf_in_anchor_text_is_normalized() {
    let (tmp, filename, _) = create_temp_file("using System;\nusing System.IO;\n");
    let ctx = make_ctx(tmp.path());

    // Anchor uses CRLF — should still match
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertAfter": "using System;\r\n", "content": "using System.Linq;" }
        ]
    }));

    // Note: the anchor "using System;\r\n" after CRLF normalization becomes "using System;\n"
    // which should find "using System;\n" in the file content
    assert!(!result.is_error, "CRLF in anchor should be normalized. Error: {:?}",
        result.content.first().map(|c| &c.text));
}

#[test]
fn test_crlf_in_replace_text_is_normalized() {
    let (tmp, filename, _) = create_temp_file("old text\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "old text", "replace": "new\r\ntext" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    // Replace text CRLF should be normalized to LF
    assert!(content.contains("new\ntext"), "Replace CRLF should be normalized to LF");
}

// ─── Part B: Auto-retry with trailing whitespace strip ──────────────

#[test]
fn test_trailing_whitespace_in_search_auto_retry() {
    // File has NO trailing whitespace
    let (tmp, filename, _) = create_temp_file("function hello() {\n    return 42;\n}\n");
    let ctx = make_ctx(tmp.path());

    // Search text has trailing spaces (LLM artifact)
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "function hello() {  \n    return 42;  \n}", "replace": "function hello() {\n    return 43;\n}" }
        ]
    }));

    assert!(!result.is_error, "Should auto-retry with stripped trailing whitespace. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let text = &result.content[0].text;
    assert!(text.contains("warnings"), "Response should contain warnings about whitespace trimming");
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("return 43;"), "Replacement should have been applied");
}

#[test]
fn test_trailing_whitespace_in_anchor_auto_retry() {
    let (tmp, filename, _) = create_temp_file("line one\nline two\nline three\n");
    let ctx = make_ctx(tmp.path());

    // Anchor has trailing spaces
    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertAfter": "line one  ", "content": "inserted line" }
        ]
    }));

    assert!(!result.is_error, "Should auto-retry anchor with stripped trailing whitespace. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let text = &result.content[0].text;
    assert!(text.contains("warnings"), "Response should contain warnings");
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("inserted line"), "Insert should have been applied");
}

#[test]
fn test_no_trailing_whitespace_no_warning() {
    // When there's no trailing whitespace issue, no warning should appear
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello world", "replace": "goodbye world" }
        ]
    }));

    assert!(!result.is_error);
    let text = &result.content[0].text;
    assert!(!text.contains("warnings"), "No warnings when exact match succeeds");
}

#[test]
fn test_trailing_whitespace_both_sides_no_retry_needed() {
    // File HAS trailing whitespace and search text matches exactly
    let (tmp, filename, _) = create_temp_file("hello world  \n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello world  ", "replace": "goodbye world" }
        ]
    }));

    assert!(!result.is_error);
    let text = &result.content[0].text;
    assert!(!text.contains("warnings"), "No warnings when exact match succeeds (both have trailing spaces)");
}

#[test]
fn test_trailing_whitespace_retry_fails_gracefully() {
    // File has completely different content — retry shouldn't help
    let (tmp, filename, _) = create_temp_file("alpha beta gamma\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "totally different text  ", "replace": "x" }
        ]
    }));

    assert!(result.is_error, "Should still fail when text is truly not found");
}

#[test]
fn test_trailing_whitespace_skip_if_not_found_with_retry() {
    // With skipIfNotFound=true, trailing whitespace retry should still work
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello world  ", "replace": "goodbye", "skipIfNotFound": false }
        ]
    }));

    // This should auto-retry and succeed (not skip)
    assert!(!result.is_error, "Should auto-retry successfully. Error: {:?}",
        result.content.first().map(|c| &c.text));
}

// ─── Part C: Hex diff diagnostics at ≥99% similarity ────────────────

#[test]
fn test_byte_diff_hint_trailing_space() {
    // File has "hello" but search has "hello " (trailing space)
    // Since trailing whitespace auto-retry catches this case, we need a case
    // where the difference is NOT trailing whitespace (e.g., tab vs space)
    let (tmp, filename, _) = create_temp_file("hello\tworld\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello world", "replace": "x" }
        ]
    }));

    assert!(result.is_error);
    let text = &result.content[0].text;
    // Should show nearest match with byte diff since similarity is very high
    assert!(text.contains("Nearest match"), "Should show nearest match hint");
    // The hint should show byte difference (tab vs space)
    assert!(text.contains("First difference") || text.contains("similarity"),
        "Should show byte-level diff or high similarity. Got: {}", text);
}

#[test]
fn test_byte_diff_hint_length_difference() {
    // Test where search is longer than file content at that line
    let (tmp, filename, _) = create_temp_file("abc\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "abcd", "replace": "x" }
        ]
    }));

    assert!(result.is_error);
    let text = &result.content[0].text;
    // "abc" vs "abcd" — similarity should be high enough for a hint
    assert!(text.contains("Nearest match"), "Should show nearest match for near-miss. Got: {}", text);
}

#[test]
fn test_describe_byte_common_whitespace() {
    // Unit test for describe_byte helper
    assert!(super::describe_byte(b' ').contains("space"));
    assert!(super::describe_byte(b'\t').contains("tab"));
    assert!(super::describe_byte(b'\n').contains("newline"));
    assert!(super::describe_byte(b'\r').contains("carriage return"));
    assert!(super::describe_byte(b'A').contains("'A'"));
}

#[test]
fn test_strip_trailing_whitespace_per_line() {
    assert_eq!(
        super::strip_trailing_whitespace_per_line("hello  \nworld\t\n"),
        "hello\nworld"
    );
    assert_eq!(
        super::strip_trailing_whitespace_per_line("no trailing"),
        "no trailing"
    );
    assert_eq!(
        super::strip_trailing_whitespace_per_line("  leading preserved  "),
        "  leading preserved"
    );
}

#[test]
fn test_normalize_crlf() {
    assert_eq!(super::normalize_crlf("hello\r\nworld"), "hello\nworld");
    assert_eq!(super::normalize_crlf("no crlf here"), "no crlf here");
    assert_eq!(super::normalize_crlf("a\r\nb\r\nc"), "a\nb\nc");
}

#[test]
fn test_byte_level_diff_hint_different_bytes() {
    let hint = super::byte_level_diff_hint("hello world", "hello\tworld");
    assert!(hint.contains("First difference at byte 5"), "Got: {}", hint);
    assert!(hint.contains("space"), "Should describe space");
    assert!(hint.contains("tab"), "Should describe tab");
}

#[test]
fn test_byte_level_diff_hint_length_difference() {
    let hint = super::byte_level_diff_hint("hello world!", "hello world");
    assert!(hint.contains("Search text is 1 byte(s) longer"), "Got: {}", hint);

    let hint2 = super::byte_level_diff_hint("hello", "hello world");
    assert!(hint2.contains("File text is 6 byte(s) longer"), "Got: {}", hint2);
}

#[test]
fn test_byte_level_diff_hint_identical() {
    let hint = super::byte_level_diff_hint("same", "same");
    assert!(hint.is_empty(), "Should be empty for identical strings");
}


// ─── Self-review regression tests ───────────────────────────────────

#[test]
fn test_all_whitespace_search_does_not_panic() {
    // Regression: search text "  " after trim becomes "", which would cause
    // result.matches("").count() to return huge number. Should gracefully fail.
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "  ", "replace": "x" }
        ]
    }));

    // "  " (two spaces) is not in the file, and after trim becomes "" which should NOT match anything
    assert!(result.is_error, "All-whitespace search that doesn't match should error");
}

#[test]
fn test_expected_context_crlf_normalized() {
    // Regression: expectedContext was not CRLF-normalized, so CRLF in expectedContext
    // would never match LF-normalized file content
    let (tmp, filename, _) = create_temp_file("line one\nline two\nline three\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "line two",
                "replace": "LINE TWO",
                "expectedContext": "line one\r\nline two"
            }
        ]
    }));

    assert!(!result.is_error, "CRLF in expectedContext should be normalized. Error: {:?}",
        result.content.first().map(|c| &c.text));
}


// ═══════════════════════════════════════════════════════════════════════
// Auto-create file tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_auto_create_file_via_mode_a_insert() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": "brand_new_file.txt",
        "operations": [
            { "startLine": 1, "endLine": 0, "content": "hello world\nsecond line" }
        ]
    }));

    assert!(!result.is_error, "Auto-create with insert should succeed: {}", result.content[0].text);
    let text = &result.content[0].text;
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["fileCreated"], true, "Response should have fileCreated: true");
    assert_eq!(parsed["applied"], 1);

    // Verify file exists with correct content
    let file_path = tmp.path().join("brand_new_file.txt");
    assert!(file_path.exists(), "File should have been created");
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("hello world"), "File should contain inserted text");
    assert!(content.contains("second line"), "File should contain second line");
}

#[test]
fn test_auto_create_file_in_nested_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": "deep/nested/dir/new_file.rs",
        "operations": [
            { "startLine": 1, "endLine": 0, "content": "fn main() {}" }
        ]
    }));

    assert!(!result.is_error, "Auto-create with nested dirs should succeed: {}", result.content[0].text);
    let file_path = tmp.path().join("deep/nested/dir/new_file.rs");
    assert!(file_path.exists(), "File should have been created in nested directory");
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("fn main() {}"), "File should contain inserted text");
}

#[test]
fn test_auto_create_mode_b_search_fails_gracefully() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": "nonexistent.txt",
        "edits": [
            { "search": "some text", "replace": "other text" }
        ]
    }));

    assert!(result.is_error, "Search/replace on auto-created empty file should fail");
    let text = &result.content[0].text;
    assert!(text.contains("not found"), "Error should mention text not found: {}", text);
}

#[test]
fn test_auto_create_mode_a_replace_on_nonexistent_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    // Replace lines 5-10 in a nonexistent file (treated as 1-line empty file)
    let result = handle_search_edit(&ctx, &json!({
        "path": "nonexistent.txt",
        "operations": [
            { "startLine": 5, "endLine": 10, "content": "new content" }
        ]
    }));

    assert!(result.is_error, "Replace on nonexistent file should fail (out of range)");
}

#[test]
fn test_auto_create_file_dry_run_does_not_create() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_search_edit(&ctx, &json!({
        "path": "dry_run_file.txt",
        "dryRun": true,
        "operations": [
            { "startLine": 1, "endLine": 0, "content": "hello" }
        ]
    }));

    assert!(!result.is_error, "Dry run should succeed");
    let file_path = tmp.path().join("dry_run_file.txt");
    assert!(!file_path.exists(), "File should NOT be created during dry run");
}

#[test]
fn test_auto_create_multi_file_mixed_existing_and_new() {
    let tmp = tempfile::tempdir().unwrap();
    create_named_temp_file(tmp.path(), "existing.txt", "line1\nline2\n");
    let ctx = make_ctx(tmp.path());

    // Insert into both existing and new file
    let result = handle_search_edit(&ctx, &json!({
        "paths": ["existing.txt", "new_file.txt"],
        "operations": [
            { "startLine": 1, "endLine": 0, "content": "inserted" }
        ]
    }));

    assert!(!result.is_error, "Multi-file with mix of existing and new should succeed: {}", result.content[0].text);
    // Both files should exist
    assert!(tmp.path().join("existing.txt").exists());
    assert!(tmp.path().join("new_file.txt").exists());
    // New file should have the inserted content
    let new_content = std::fs::read_to_string(tmp.path().join("new_file.txt")).unwrap();
    assert!(new_content.contains("inserted"), "New file should contain inserted text");
}