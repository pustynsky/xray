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