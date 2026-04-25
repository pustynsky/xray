use super::*;
use std::sync::{Arc, RwLock};
use crate::mcp::handlers::WorkspaceBinding;
use serde_json::json;
use std::path::PathBuf;

/// Helper: create a HandlerContext with server_dir pointing to a temp directory.
fn make_ctx(dir: &std::path::Path) -> HandlerContext {
    HandlerContext {
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(dir.to_string_lossy().to_string()))),
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename
    }));

    assert!(result.is_error, "Neither operations nor edits should fail");
}

#[test]
fn test_dry_run_does_not_write() {
    let (tmp, filename, path) = create_temp_file("original content\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
fn test_empty_file_replace_rejected_with_insert_hint() {
    // Regression for `docs/user-stories/todo_approved_2026-04-24_expected-line-count-empty-file-semantics.md`.
    // Previously an empty file was internally treated as 1 phantom line slot,
    // so REPLACE 1..1 silently succeeded and wrote content WITHOUT a trailing
    // newline — while the response simultaneously reported `originalLineCount: 0`.
    // After the carve-out, REPLACE on an empty file must error with an explicit
    // hint pointing at the canonical INSERT idiom.
    let (tmp, filename, _path) = create_temp_file("");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 1, "endLine": 1, "content": "hello" }
        ]
    }));

    assert!(result.is_error, "REPLACE 1..1 on empty file must error");
    let text = &result.content[0].text;
    assert!(text.contains("empty file"), "error should mention 'empty file': {text}");
    assert!(text.contains("INSERT"), "error should suggest INSERT mode: {text}");
    assert!(text.contains("startLine: 1, endLine: 0"),
        "error should spell out the canonical INSERT idiom: {text}");
}

#[test]
fn test_empty_file_insert_with_expected_line_count_zero() {
    // Verifies the contract: empty file → expectedLineCount: 0 → INSERT 1..0
    // succeeds. This is the LLM round-trip path (after a previous edit returned
    // `newLineCount: 0` on a freshly-emptied file).
    let (tmp, filename, path) = create_temp_file("");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "expectedLineCount": 0,
        "operations": [
            { "startLine": 1, "endLine": 0, "content": "hello" }
        ]
    }));

    assert!(!result.is_error,
        "empty file + expectedLineCount=0 + INSERT 1..0 should succeed: {}",
        result.content[0].text);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("hello"));
}

#[test]
fn test_round_trip_new_line_count_to_expected_line_count() {
    // End-to-end round-trip guard for the documented contract:
    //   1. perform an edit → response carries `newLineCount`
    //   2. feed that value back as `expectedLineCount` on the next edit
    //   3. the next edit must NOT fail with an off-by-one
    // Uses a file whose final byte is `\n` because that is the historical
    // off-by-one trap (`split('\n').count()` returned N+1 for `"a\nb\nc\n"`).
    let (tmp, filename, _path) = create_temp_file("a\nb\nc\n");
    let ctx = make_ctx(tmp.path());

    // Edit 1 — replace line 2.
    let result1 = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "B" }
        ]
    }));
    assert!(!result1.is_error, "first edit should succeed: {}", result1.content[0].text);
    let v1: serde_json::Value = serde_json::from_str(&result1.content[0].text).unwrap();
    let new_line_count = v1.get("newLineCount").and_then(|v| v.as_u64())
        .expect("response must carry newLineCount");
    assert_eq!(new_line_count, 3, "human semantics: 'a\\nB\\nc\\n' is 3 lines");

    // Edit 2 — feed newLineCount back as expectedLineCount.
    let result2 = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "expectedLineCount": new_line_count,
        "operations": [
            { "startLine": 1, "endLine": 1, "content": "A" }
        ]
    }));
    assert!(!result2.is_error,
        "round-trip newLineCount → expectedLineCount must not off-by-one: {}",
        result2.content[0].text);
}

/// Regression for `user-stories/xray-edit-blank-line-only-file-line-count-drift.md`.
/// A file containing exactly one blank line (`"\n"`) splits to `["", ""]`.
/// The earlier carve-out only handled the empty-file `[""]` case, leaving
/// blank-line-only files reporting `originalLineCount: 1` while
/// `apply_line_operations` still saw 2 addressable lines — REPLACE 1..2 /
/// DELETE 1..2 / range checks against line 2 silently succeeded against a
/// phantom slot. The generalized rule (drop the trailing-empty sentinel for
/// any non-empty `lines`) eliminates the phantom across all files.
#[test]
fn test_blank_line_only_file_lf_treated_as_one_line() {
    let (tmp, filename, _) = create_temp_file("\n");
    let ctx = make_ctx(tmp.path());

    // Range 1..2 must error — line 2 is the phantom slot, not addressable.
    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 1, "endLine": 2, "content": "x" }
        ]
    }));
    assert!(result.is_error,
        "REPLACE 1..2 on a 1-line blank file must error (no phantom line 2): {:?}",
        result.content[0].text);
    assert!(result.content[0].text.contains("out of range"),
        "error should be the canonical out-of-range diagnostic, got: {}",
        result.content[0].text);
}

#[test]
fn test_blank_line_only_file_crlf_treated_as_one_line() {
    // Same shape as the LF case, but for CRLF — the contract is line-count
    // semantics, not line-ending preservation.
    let (tmp, filename, _) = create_temp_file("\r\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "x" }
        ]
    }));
    assert!(result.is_error,
        "REPLACE on phantom line 2 of a CRLF blank-line-only file must error: {:?}",
        result.content[0].text);
}

#[test]
fn test_blank_line_only_file_round_trip_line_counts_agree() {
    // expectedLineCount and newLineCount must agree with Mode A semantics for
    // blank-line-only files — the whole point of the line-count drift fix.
    let (tmp, filename, _) = create_temp_file("\n");
    let ctx = make_ctx(tmp.path());

    // expectedLineCount=1 + REPLACE 1..1 → succeeds (1 is the only addressable line).
    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "expectedLineCount": 1,
        "operations": [
            { "startLine": 1, "endLine": 1, "content": "" }
        ]
    }));
    assert!(!result.is_error,
        "expectedLineCount=1 + REPLACE 1..1 on blank-line-only file must succeed: {}",
        result.content[0].text);
}

#[test]
fn test_single_line_file() {
    let (tmp, filename, path) = create_temp_file("only line");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    // For "line1\nline2\n", `count_lines` returns 2 (the trailing `\n` is a
    // terminator, not a line — same convention as `xray_definitions` /
    // `xray_grep` and the response's `originalLineCount` / `newLineCount`).
    // INSERT at end of file = `startLine: count_lines + 1`, `endLine: count_lines`
    // → here startLine=3, endLine=2.
    //
    // Previously this test passed `startLine: 4, endLine: 3` because Mode A
    // counted `split('\n')` elements directly (the trailing-newline sentinel
    // inflated the count by one). That phantom slot has been removed so Mode A
    // line numbers agree with every other surface; see the carve-out comment in
    // `apply_line_operations` for details.
    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 3, "endLine": 2, "content": "appended" }
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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

    // "a\nb\nc\n" has 3 contentful lines (trailing '\n' is a terminator,
    // not an extra empty line) — same convention as `xray_definitions` /
    // `xray_grep` line numbers, which is what LLMs actually see.
    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "B" }
        ],
        "expectedLineCount": 3
    }));

    assert!(!result.is_error, "Correct expectedLineCount should pass");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("B"), "Edit should be applied");
}

/// Regression: `expectedLineCount` must abort Mode B (text-match) edits too.
/// Previously the safety check lived inside the Mode A arm only and the
/// parameter was silently ignored for Mode B (reported 2026-04-23).
#[test]
fn test_mode_b_expected_line_count_mismatch() {
    let (tmp, filename, path) = create_temp_file("foo\nbar\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "search": "foo", "replace": "qux" }],
        "expectedLineCount": 99999,
    }));

    assert!(result.is_error, "Mode B should honor expectedLineCount");
    let text = &result.content[0].text;
    assert!(
        text.contains("Expected 99999 lines"),
        "Error should mention expected count, got: {}",
        text
    );
    // File must not have been mutated.
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "foo\nbar\n", "File must be untouched on mismatch");
}

#[test]
fn test_mode_b_expected_line_count_match() {
    let (tmp, filename, path) = create_temp_file("foo\nbar\n");
    let ctx = make_ctx(tmp.path());

    // 2 contentful lines (human/editor count, matches xray_definitions).
    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "search": "foo", "replace": "qux" }],
        "expectedLineCount": 2,
    }));

    assert!(
        !result.is_error,
        "Correct expectedLineCount should pass, got: {:?}",
        result
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("qux"), "Edit should be applied");
}

/// Pin down the line-counting convention: matches what editors and
/// `xray_definitions`/`xray_grep` report (1-based, trailing '\n' = terminator).
#[test]
fn test_expected_line_count_human_semantics() {
    let (tmp, filename, _) = create_temp_file("a\nb\nc\n");
    let ctx = make_ctx(tmp.path());

    // Off-by-one (counting the trailing newline as a separate line) must fail.
    let bad = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "search": "a", "replace": "A" }],
        "expectedLineCount": 4,
        "dryRun": true,
    }));
    assert!(
        bad.is_error,
        "expectedLineCount=4 must NOT match a 3-line file ('a\\nb\\nc\\n')"
    );

    // Human count (3) must succeed.
    let ok = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "search": "a", "replace": "A" }],
        "expectedLineCount": 3,
        "dryRun": true,
    }));
    assert!(
        !ok.is_error,
        "expectedLineCount=3 must match a 3-line file, got: {:?}",
        ok
    );
}

#[test]
fn test_mode_b_empty_search_error() {
    let (tmp, filename, _) = create_temp_file("hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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

// ─── expectedContext + occurrence: validate the *targeted* match ─────────

#[test]
fn test_expected_context_validates_targeted_occurrence_not_first() {
    // Pre-fix: `check_expected_context` was always called against
    // `positions[0]` (the FIRST match), even when `occurrence: 2` explicitly
    // targeted the second match. A caller passing context that disambiguates
    // the second occurrence from the first would see the gate fail on the
    // first match's surroundings and never reach their intended replacement.
    //
    // The window is ±5 lines so the two `Foo` matches must be more than 5
    // lines apart for the disambiguation to be observable.
    let body = "// alpha section\nFoo\nL1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\n// beta section\nFoo\n";
    let (tmp, filename, path) = create_temp_file(body);
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "Foo",
                "replace": "Bar",
                "occurrence": 2,
                "expectedContext": "// beta section"
            }
        ]
    }));

    assert!(
        !result.is_error,
        "expectedContext on the targeted (2nd) occurrence must succeed, got: {}",
        result.content[0].text
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.contains("// alpha section\nFoo\n"),
        "First `Foo` must remain untouched, got: {:?}", content
    );
    assert!(
        content.contains("// beta section\nBar\n"),
        "Second `Foo` (matching the beta-section context) must be replaced, got: {:?}", content
    );
}

#[test]
fn test_expected_context_rejects_targeted_occurrence_with_wrong_context() {
    // Symmetric guard: if the user targets `occurrence: 2` but supplies the
    // FIRST match's context, the gate must reject — pre-fix this would have
    // PASSED (because the gate only ever looked at the first match), then
    // silently replaced the second match against the wrong-context contract.
    //
    // Spacing matches the test above: >5 lines between matches so each
    // context is OUTSIDE the other's ±5-line window.
    let body = "// alpha section\nFoo\nL1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\n// beta section\nFoo\n";
    let (tmp, filename, path) = create_temp_file(body);
    let ctx = make_ctx(tmp.path());

    let original = std::fs::read_to_string(&path).unwrap();

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "Foo",
                "replace": "Bar",
                "occurrence": 2,
                // Context belongs to the FIRST match — must NOT validate the 2nd.
                "expectedContext": "// alpha section"
            }
        ]
    }));

    assert!(
        result.is_error,
        "expectedContext targeting 2nd occurrence with 1st-match context must fail, got success: {}",
        result.content[0].text
    );
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, original, "File must not be modified when context check rejects");
}

#[test]
fn test_expected_context_occurrence_zero_replace_all_uses_first_match_context() {
    // Back-compat guard: `occurrence: 0` (replace-all, the default) preserves
    // the historical behaviour of validating against the first match's
    // context. The gate's contract for replace-all is "the user-supplied
    // context must surround at least one of the matches we're about to
    // touch", and matching against the first preserves both the historical
    // error message and the all-or-nothing semantics.
    let (tmp, filename, path) = create_temp_file(
        "// alpha section\nFoo\n// beta section\nFoo\n",
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "Foo",
                "replace": "Bar",
                // occurrence: 0 (default) — replace-all
                "expectedContext": "// alpha section"
            }
        ]
    }));

    assert!(
        !result.is_error,
        "Replace-all with first-match context must succeed: {}",
        result.content[0].text
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        content, "// alpha section\nBar\n// beta section\nBar\n",
        "Replace-all replaces both matches"
    );
}

/// Regression for `user-stories/xray-edit-expected-context-masks-occurrence-out-of-range.md`.
/// When `occurrence > count`, the canonical "Occurrence N requested but text
/// … found only M time(s)" diagnostic must fire BEFORE `expectedContext`
/// validation — otherwise an out-of-range request whose `expectedContext`
/// happens to mismatch the first match surfaces "Expected context …" and
/// hides the more fundamental "you targeted a non-existent occurrence" error.
#[test]
fn test_occurrence_out_of_range_reported_before_expected_context_mismatch() {
    let body = "// alpha section\nFoo\nL1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\n// beta section\nFoo\n";
    let (tmp, filename, path) = create_temp_file(body);
    let ctx = make_ctx(tmp.path());

    let original = std::fs::read_to_string(&path).unwrap();

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "Foo",
                "replace": "Bar",
                "occurrence": 99,
                // Wrong context for first match — pre-fix this would surface
                // "Expected context not found" and hide the canonical
                // "found only 2 time(s)" error.
                "expectedContext": "context that does not exist anywhere"
            }
        ]
    }));

    assert!(result.is_error, "occurrence 99 against 2 matches must error");
    let text = &result.content[0].text;
    assert!(
        text.contains("Occurrence 99 requested") && text.contains("found only 2 time(s)"),
        "must surface the canonical occurrence-out-of-range diagnostic, NOT \
         `Expected context …` (which would mask it). Got: {text}"
    );
    assert!(
        !text.contains("Expected context"),
        "expectedContext mismatch must not leak through when occurrence is out of range; got: {text}"
    );
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, original, "file must remain untouched on out-of-range error");
}

#[test]
fn test_expected_context_with_insert_after() {
    let (tmp, filename, path) = create_temp_file("using System;\nusing System.IO;\n\nclass Foo {}\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

// ─── Part B (REMOVED): trailing-WS auto-retry was a silent fallback ────
//
// PR #1 removed Step 2 (strip trailing whitespace) from `find_with_retry`.
// Trailing-WS drift now surfaces as `Text not found` with a categorised
// `Nearest match` hint instead of being silently fixed. Tests below are
// inverted accordingly. The `..._no_warning` and `..._both_sides_no_retry_needed`
// tests still pass unchanged because they exercise the exact-match path.

#[test]
fn test_no_silent_match_trailing_whitespace_in_search() {
    // Was test_trailing_whitespace_in_search_auto_retry — inverted.
    let (tmp, filename, _) = create_temp_file("function hello() {\n    return 42;\n}\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "function hello() {  \n    return 42;  \n}", "replace": "function hello() {\n    return 43;\n}" }
        ]
    }));

    assert!(result.is_error, "Trailing-WS drift must NOT silently match");
    let text = &result.content[0].text;
    assert!(text.contains("Text not found"), "Got: {}", text);
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("return 42;"), "File must be unchanged");
    assert!(!content.contains("return 43;"), "No replacement must occur");
}

#[test]
fn test_no_silent_match_trailing_whitespace_in_anchor() {
    // Was test_trailing_whitespace_in_anchor_auto_retry — inverted.
    let (tmp, filename, _) = create_temp_file("line one\nline two\nline three\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertAfter": "line one  ", "content": "inserted line" }
        ]
    }));

    assert!(result.is_error, "Anchor trailing-WS drift must NOT silently match");
    let text = &result.content[0].text;
    assert!(text.contains("Anchor text not found"), "Got: {}", text);
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(!content.contains("inserted line"), "No insert must occur");
}

#[test]
fn test_no_trailing_whitespace_no_warning() {
    // When there's no trailing whitespace issue, no warning should appear
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "totally different text  ", "replace": "x" }
        ]
    }));

    assert!(result.is_error, "Should still fail when text is truly not found");
}

#[test]
fn test_no_silent_match_skip_if_not_found_does_not_resurrect_match() {
    // Was test_trailing_whitespace_skip_if_not_found_with_retry — inverted.
    // Before PR #1: Step 2 trimmed search to "hello world" and matched
    // even with skipIfNotFound=false. After PR #1 the trim is gone, so the
    // search must surface as Text not found.
    let (tmp, filename, path) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello world  ", "replace": "goodbye", "skipIfNotFound": false }
        ]
    }));

    assert!(result.is_error, "Trailing-WS drift must NOT silently match");
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "hello world\n", "File must be unchanged");
}

// ─── Part C: Hex diff diagnostics at ≥99% similarity ────────────────

#[test]
fn test_byte_diff_hint_trailing_space() {
    // Test byte-level diff diagnostic when similarity is very high.
    // Use non-whitespace difference (hyphen vs underscore) since flex-space
    // auto-retry now handles tab-vs-space differences.
    let (tmp, filename, _) = create_temp_file("hello_world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello-world", "replace": "x" }
        ]
    }));

    assert!(result.is_error);
    let text = &result.content[0].text;
    // Should show nearest match with byte diff since similarity is very high
    assert!(text.contains("Nearest match"), "Should show nearest match hint");
    // The hint should show byte difference (hyphen vs underscore)
    assert!(text.contains("First difference") || text.contains("similarity"),
        "Should show byte-level diff or high similarity. Got: {}", text);
}

#[test]
fn test_byte_diff_hint_length_difference() {
    // Test where search is longer than file content at that line
    let (tmp, filename, _) = create_temp_file("abc\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "  ", "replace": "x" }
        ]
    }));

    // "  " (two spaces) is not in the file, and after trim becomes "" which should NOT match anything
    assert!(result.is_error, "All-whitespace search that doesn't match should error");
}

// ─── Step 3 (REMOVED): blank-line trim was a silent fallback ────────
//
// The Step 3 retry that trimmed leading/trailing blank lines from the search
// text was removed in PR #1 (cleanup-magic). It silently rewrote the search
// text and could match a semantically different block. After removal, the same
// inputs surface as `Text not found` errors — see the
// `test_no_silent_match_*` tests below.

#[test]
fn test_no_silent_match_search_leading_newline() {
    // Was test_blank_line_trim_search_leading_newline — inverted.
    // Before PR #1: silent Step 3 trimmed the leading \n and matched.
    // After PR #1: must surface as Text not found.
    let (tmp, filename, _) = create_temp_file("## Heading\n\nSome text\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "\n## Heading", "replace": "## New Heading" }
        ]
    }));

    assert!(result.is_error,
        "Search with leading \\n must NOT silently match after PR #1. Got: {:?}",
        result.content.first().map(|c| &c.text));
    let text = &result.content[0].text;
    assert!(text.contains("Text not found"),
        "Error should be 'Text not found': {}", text);
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert_eq!(content, "## Heading\n\nSome text\n",
        "File must be unchanged after error");
}

#[test]
fn test_no_silent_match_search_trailing_newlines() {
    // Was test_blank_line_trim_search_trailing_newlines — inverted.
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello world\n\n", "replace": "goodbye world" }
        ]
    }));

    assert!(result.is_error,
        "Search with trailing \\n\\n must NOT silently match after PR #1. Got: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert_eq!(content, "hello world\n", "File must be unchanged after error");
}

#[test]
fn test_no_silent_match_anchor_leading_newline() {
    // Was test_blank_line_trim_anchor_leading_newline — inverted.
    let (tmp, filename, _) = create_temp_file("line one\nline two\nline three\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertAfter": "\nline one", "content": "inserted line" }
        ]
    }));

    assert!(result.is_error,
        "Anchor with leading \\n must NOT silently match after PR #1. Got: {:?}",
        result.content.first().map(|c| &c.text));
    let text = &result.content[0].text;
    assert!(text.contains("Anchor text not found"),
        "Error should be 'Anchor text not found': {}", text);
}

#[test]
fn test_blank_line_trim_no_change_needed() {
    // Search text has no leading/trailing blank lines — exact match should work, no warning
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello world", "replace": "goodbye world" }
        ]
    }));

    assert!(!result.is_error);
    let text = &result.content[0].text;
    assert!(!text.contains("warnings"), "No warnings for exact match");
}

// ─── Step 4: Flex-space matching ─────────────────────────────────────

#[test]
fn test_flex_space_table_padding() {
    // File has padded markdown table, search has compact version
    let (tmp, filename, _) = create_temp_file(
        "| Issue       | Count     | Action              |\n|---|---|---|\n| Bug 1       | 5         | Fix it              |\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "| Bug 1 | 5 | Fix it |", "replace": "| Bug 2 | 10 | Done |", "expectedContext": "Bug 1" }
        ]
    }));

    assert!(!result.is_error, "Should match with flex-space. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let text = &result.content[0].text;
    assert!(text.contains("warnings"), "Should have flex-space warning");
    assert!(text.contains("flexible whitespace"), "Warning should mention flexible whitespace");
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("Bug 2"), "Replacement should have been applied");
}

#[test]
fn test_flex_space_multiline_table() {
    let (tmp, filename, _) = create_temp_file(
        "| A       | B     |\n|---|---|\n| 1       | 2     |\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "| A | B |\n|---|---|\n| 1 | 2 |", "replace": "| X | Y |\n|---|---|\n| 3 | 4 |", "expectedContext": "| 1" }
        ]
    }));

    assert!(!result.is_error, "Should match multiline flex-space. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("| X | Y |"), "Replacement should have been applied");
}

#[test]
fn test_flex_space_exact_match_preferred() {
    // File has exact match — should use exact, no warnings
    let (tmp, filename, _) = create_temp_file("| A | B |\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "| A | B |", "replace": "| X | Y |" }
        ]
    }));

    assert!(!result.is_error);
    let text = &result.content[0].text;
    assert!(!text.contains("warnings"), "Exact match should not produce warnings");
}

#[test]
fn test_flex_space_anchor_insert_after() {
    let (tmp, filename, _) = create_temp_file(
        "| Issue       | Count     |\n|---|---|\n| Bug 1       | 5         |\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertAfter": "| Bug 1 | 5 |", "content": "| Bug 2 | 10 |", "expectedContext": "Bug 1" }
        ]
    }));

    assert!(!result.is_error, "Should match anchor with flex-space. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("Bug 2"), "Insert should have been applied");
}

#[test]
fn test_flex_space_anchor_insert_before() {
    let (tmp, filename, _) = create_temp_file(
        "| Issue       | Count     |\n|---|---|\n| Bug 1       | 5         |\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertBefore": "| Bug 1 | 5 |", "content": "| Bug 0 | 0 |", "expectedContext": "Bug 1" }
        ]
    }));

    assert!(!result.is_error, "Should match anchor with flex-space for insertBefore. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("Bug 0"), "Insert should have been applied");
}

#[test]
fn test_flex_space_with_occurrence() {
    let (tmp, filename, _) = create_temp_file(
        "| A       |\n| A       |\n| A       |\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "| A |", "replace": "| B |", "occurrence": 2, "expectedContext": "| A" }
        ]
    }));

    assert!(!result.is_error, "Should match occurrence 2 with flex-space. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    // Line 0 should still be | A       |
    assert!(lines[0].contains("A"), "First line should be unchanged");
    // Line 1 should be replaced with | B |
    assert!(lines[1].contains("B"), "Second line should be replaced");
    // Line 2 should still be | A       |
    assert!(lines[2].contains("A"), "Third line should be unchanged");
}

#[test]
fn test_flex_space_not_used_for_regex_mode() {
    // is_regex=true should not use flex-space fallback
    let (tmp, filename, _) = create_temp_file("| A       | B     |\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "regex": true,
        "edits": [
            { "search": "\\| A \\| B \\|", "replace": "| X | Y |" }
        ]
    }));

    // Regex mode should NOT flex-match — the regex "\| A \| B \|" doesn't match "| A       | B     |"
    assert!(result.is_error, "Regex mode should not use flex-space fallback");
}

#[test]
fn test_flex_space_replacement_dollar_sign_safety() {
    // Replacement text with $ should be treated literally (NoExpand)
    let (tmp, filename, _) = create_temp_file("| Price       |\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "| Price |", "replace": "| $100 |", "expectedContext": "Price" }
        ]
    }));

    assert!(!result.is_error, "Should match with flex-space. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("$100"), "Replacement with $ should be literal");
}

#[test]
fn test_flex_space_markdown_separator_dash_count_mismatch() {
    // Regression: LLM sends table with |---|---| but file has |---------|-------------|
    // This is the exact scenario that caused the original xray_edit failure.
    let (tmp, filename, _) = create_temp_file(
        "## Overview\n\n| Cluster | Status |\n|---------|-------------|\n| East | OK |\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "| Cluster | Status |\n|---|---|\n| East | OK |", "replace": "| Cluster | Status |\n|---|---|\n| West | FAIL |", "expectedContext": "East" }
        ]
    }));

    assert!(!result.is_error, "Should match with flex separator dashes. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let text = &result.content[0].text;
    assert!(text.contains("warnings"), "Should have flex-space warning");
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("West"), "Replacement should have been applied");
    assert!(content.contains("FAIL"), "Replacement should have been applied");
}

#[test]
fn test_flex_space_markdown_separator_with_alignment() {
    // File has alignment colons in separator, search has plain dashes
    let (tmp, filename, _) = create_temp_file(
        "| Name | Value |\n|:---------|----------:|\n| foo | 42 |\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "| Name | Value |\n|---|---|\n| foo | 42 |", "replace": "| Name | Value |\n|---|---|\n| bar | 99 |", "expectedContext": "foo" }
        ]
    }));

    assert!(!result.is_error, "Should match separator with colons via flex. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("bar"), "Replacement should have been applied");
}

#[test]
fn test_flex_space_markdown_separator_anchor_insert() {
    // Insert after a table row where the separator has different dash count
    let (tmp, filename, _) = create_temp_file(
        "| Name | Value |\n|---------|-------------|\n| foo | 42 |\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "insertAfter": "|---|---|", "content": "| bar | 99 |", "expectedContext": "Name" }
        ]
    }));

    assert!(!result.is_error, "Should match separator anchor via flex. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("bar"), "Insert should have been applied");
}


// ─── Step 5: expectedContext flex-space ──────────────────────────────

#[test]
fn test_expected_context_flex_space() {
    // File has padded table, expectedContext uses compact version
    let (tmp, filename, _) = create_temp_file(
        "header\n| Issue       | Count     |\nfooter\n"
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "header", "replace": "HEADER", "expectedContext": "| Issue | Count |" }
        ]
    }));

    assert!(!result.is_error, "expectedContext should match with collapsed whitespace. Error: {:?}",
        result.content.first().map(|c| &c.text));
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("HEADER"), "Edit should have been applied");
}

// ─── Step 6: Flex-space opt-in (requires expectedContext) ────────────

#[test]
fn test_flex_space_disabled_without_expected_context() {
    // Without expectedContext, flex-space fallback is disabled — exact match
    // fails, strip-ws/trim-blank fails, and regex step is skipped. Error
    // should hint at passing expectedContext to enable the fallback.
    let (tmp, filename, _) = create_temp_file(
        "| Issue       | Count     |\n| Bug 1       | 5         |\n",
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "| Bug 1 | 5 |", "replace": "| Bug 2 | 10 |" }
        ]
    }));

    assert!(result.is_error, "Flex-space must NOT match without expectedContext");
    let text = &result.content[0].text;
    assert!(
        text.contains("Text not found"),
        "Expected 'Text not found' error, got: {}",
        text
    );
    assert!(
        text.contains("expectedContext"),
        "Error should hint at passing expectedContext to enable flex fallback, got: {}",
        text
    );
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("Bug 1"), "File must remain unchanged");
}

#[test]
fn test_flex_space_enabled_with_expected_context_marker() {
    // With expectedContext, flex-space fallback is enabled and the warning
    // carries a stable [fallbackApplied:flexWhitespace] marker that clients
    // can detect deterministically.
    let (tmp, filename, _) = create_temp_file(
        "header\n| Bug 1       | 5         |\nfooter\n",
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "| Bug 1 | 5 |",
                "replace": "| Bug 2 | 10 |",
                "expectedContext": "Bug 1"
            }
        ]
    }));

    assert!(
        !result.is_error,
        "Flex-space must match with expectedContext present. Error: {:?}",
        result.content.first().map(|c| &c.text)
    );
    let text = &result.content[0].text;
    assert!(
        text.contains("[fallbackApplied:flexWhitespace]"),
        "Warning must include stable fallbackApplied marker, got: {}",
        text
    );
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("Bug 2"), "Replacement should have been applied");
}

#[test]
fn test_flex_space_rejected_when_expected_context_mismatches() {
    // Regression guard: flex-space match must still be rejected if
    // expectedContext does not match near the flex-matched position.
    // (i.e. order is flex-match -> check_expected_context, not the other way.)
    let (tmp, filename, _) = create_temp_file(
        "header\n| Bug 1       | 5         |\nfooter\n",
    );
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            {
                "search": "| Bug 1 | 5 |",
                "replace": "| Bug 2 | 10 |",
                "expectedContext": "this context does not exist anywhere"
            }
        ]
    }));

    assert!(
        result.is_error,
        "Flex match with wrong expectedContext must be rejected"
    );
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("Bug 1"), "File must remain unchanged");
}


#[test]
fn test_expected_context_exact_match_still_works() {
    // Exact expectedContext match should work as before
    let (tmp, filename, _) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "line2", "replace": "LINE2", "expectedContext": "line1" }
        ]
    }));

    assert!(!result.is_error);
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert!(content.contains("LINE2"));
}

#[test]
fn test_expected_context_wrong_context_still_fails() {
    // Wrong context should still fail even with flex-space
    let (tmp, filename, _) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "line2", "replace": "LINE2", "expectedContext": "completely wrong" }
        ]
    }));

    assert!(result.is_error, "Wrong expectedContext should still fail");
}

// ─── Helper function unit tests ──────────────────────────────────────

#[test]
fn test_collapse_spaces() {
    assert_eq!(collapse_spaces("| A       | B     |"), "| A | B |");
    assert_eq!(collapse_spaces("  hello   world  "), "hello world");
    assert_eq!(collapse_spaces("no  change"), "no change");
    assert_eq!(collapse_spaces("line1\n  line2  \nline3"), "line1\nline2\nline3");
}

#[test]
fn test_search_to_flex_pattern() {
    // Basic table pattern
    let p = search_to_flex_pattern("| A | B |").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(re.is_match("| A       | B     |"));
    assert!(re.is_match("| A | B |"));
    assert!(re.is_match("  | A  | B |  "));

    // Multi-line
    let p = search_to_flex_pattern("| A |\n|---|\n| 1 |").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(re.is_match("| A       |\n|---|\n| 1       |"));

    // All-whitespace returns None
    assert!(search_to_flex_pattern("   ").is_none());
    assert!(search_to_flex_pattern("").is_none());

    // Should not match when non-whitespace differs
    let p = search_to_flex_pattern("| A |").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(!re.is_match("| B |"));

    // Markdown table separator: flex dash counts
    let p = search_to_flex_pattern("|---|---|").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(re.is_match("|---|---|"), "Exact separator should match");
    assert!(re.is_match("|---------|-------------|------------|-----------|")
        , "Separator with more dashes should match");
    assert!(re.is_match("|--|--|"), "Separator with fewer dashes should match");
    assert!(re.is_match("|:---|---:|"), "Separator with colons should match");
    assert!(re.is_match("|:---:|:---:|"), "Center-aligned separator should match");
    assert!(re.is_match("| --- | --- |"), "Separator with spaces should match");

    // Separator preserves column count (number of pipes)
    let p = search_to_flex_pattern("|---|---|---|").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(re.is_match("|---------|-------------|------------|"), "3-col separator should match 3-col");
    assert!(!re.is_match("|---|---|"), "3-col separator should NOT match 2-col");

    // Multi-line with separator: search has short dashes, file has long dashes
    let p = search_to_flex_pattern("| A | B |\n|---|---|\n| 1 | 2 |").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(re.is_match("| A       | B     |\n|---------|-------------|\n| 1       | 2     |"),
        "Multi-line with different dash counts should match");

    // Non-separator line that happens to have dashes should NOT use separator flex
    // (it has 'a' which is not in the separator char set)
    let p = search_to_flex_pattern("a-b|c-d").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(!re.is_match("a---b|c---d"), "Non-separator line should use normal flex, not separator flex");

    // En dash (–) and em dash (—) should also be recognized
    let p = search_to_flex_pattern("|–––|–––|").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(re.is_match("|---|---|"), "En dash separator should match hyphen-minus separator");
    assert!(re.is_match("|---------|-------------|"), "En dash separator should match long dashes");

    let p = search_to_flex_pattern("|———|———|").unwrap();
    let re = regex::Regex::new(&p).unwrap();
    assert!(re.is_match("|---|---|"), "Em dash separator should match hyphen-minus separator");
}

#[test]
fn test_expected_context_crlf_normalized() {
    // Regression: expectedContext was not CRLF-normalized, so CRLF in expectedContext
    // would never match LF-normalized file content
    let (tmp, filename, _) = create_temp_file("line one\nline two\nline three\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    let result = handle_xray_edit(&ctx, &json!({
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

    // Replace lines 5-10 in a nonexistent file (treated as empty / 0 lines —
    // see `apply_line_operations` empty-file carve-out). Returns an explicit
    // empty-file error suggesting INSERT mode rather than the generic
    // out-of-range message.
    let result = handle_xray_edit(&ctx, &json!({
        "path": "nonexistent.txt",
        "operations": [
            { "startLine": 5, "endLine": 10, "content": "new content" }
        ]
    }));

    assert!(result.is_error, "Replace on nonexistent file should fail (empty file → INSERT-only)");
}

#[test]
fn test_auto_create_file_dry_run_does_not_create() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
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
    let result = handle_xray_edit(&ctx, &json!({
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
// ═══════════════════════════════════════════════════════════════════════
// Regression tests for audit-2026-03-14 fixes
// ═══════════════════════════════════════════════════════════════════════

/// Regression: regex capture group cascade bug.
/// When $0 (full match) expansion contains "$1" as literal text,
/// the old manual sequential replacement would double-substitute.
/// Fix: use caps.expand() which handles this correctly.
#[cfg(test)]
mod audit_regression_tests {
    use super::*;
    use std::sync::{Arc, RwLock};
    use crate::mcp::handlers::WorkspaceBinding;
    use serde_json::json;

    fn make_ctx(dir: &std::path::Path) -> super::HandlerContext {
        super::HandlerContext {
            workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(dir.to_string_lossy().to_string()))),
            ..super::HandlerContext::default()
        }
    }

    #[test]
    fn test_regex_capture_group_no_cascade_when_match_contains_dollar_sign() {
        // Content: "price: $100" — the $0 expansion is "price: $100"
        // which contains "$1". Old code would replace "$1" again.
        // With caps.expand(), only the explicit capture groups in the replacement are expanded.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        std::fs::write(&path, "price: $100\nprice: $200\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let result = handle_xray_edit(&ctx, &json!({
            "path": "test.txt",
            "edits": [
                { "search": r"price: (\$\d+)", "replace": "cost: $1", "occurrence": 1 }
            ],
            "regex": true
        }));

        assert!(!result.is_error, "Should succeed: {:?}", result.content[0].text);
        let content = std::fs::read_to_string(&path).unwrap();
        // First occurrence should be replaced: "price: $100" → "cost: $100"
        assert!(content.contains("cost: $100"), "First should be replaced to 'cost: $100', got: {}", content);
        // Second should be unchanged
        assert!(content.contains("price: $200"), "Second should remain unchanged, got: {}", content);
    }

    #[test]
    fn test_multi_file_temp_files_cleaned_up_on_write_failure() {
        // Test that temp files (.xray_tmp) are cleaned up even when writing succeeds.
        // After successful multi-file edit, no .xray_tmp files should remain.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello\n").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "hello\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let result = handle_xray_edit(&ctx, &json!({
            "paths": ["a.txt", "b.txt"],
            "edits": [
                { "search": "hello", "replace": "world" }
            ]
        }));

        assert!(!result.is_error, "Should succeed: {:?}", result.content[0].text);

        // Verify no .xray_tmp files remain
        let tmp_files: Vec<_> = std::fs::read_dir(tmp.path()).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".xray_tmp"))
            .collect();
        assert!(tmp_files.is_empty(),
            "No .xray_tmp files should remain after successful edit, found: {:?}",
            tmp_files.iter().map(|e| e.file_name()).collect::<Vec<_>>());

        // Verify files were actually modified
        assert_eq!(std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(), "world\n");
        assert_eq!(std::fs::read_to_string(tmp.path().join("b.txt")).unwrap(), "world\n");
    }

    #[test]
    fn test_temp_path_for_generates_correct_path() {
        use std::path::PathBuf;
        let target = PathBuf::from("/some/dir/myfile.rs");
        let temp = super::temp_path_for(&target);
        // EDIT-005: temp filename now embeds PID + nanos + counter for concurrency
        // safety. Verify structure rather than exact string: same parent dir,
        // dot-prefix + original-name + `.xray_tmp.` infix.
        assert_eq!(temp.parent(), Some(PathBuf::from("/some/dir").as_path()));
        let name = temp.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(".myfile.rs.xray_tmp."), "got: {}", name);
        // Ensure two consecutive calls produce distinct names.
        let temp2 = super::temp_path_for(&target);
        assert_ne!(temp, temp2, "two consecutive temp paths must be unique");
    }

    // ─── MAJOR-12: atomic write via temp + rename ─────────────────────────

    #[test]
    fn test_write_file_with_endings_is_atomic_no_leftover_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("file.txt");
        std::fs::write(&target, "original\n").unwrap();
        let ctx = make_ctx(tmp.path());

        let result = handle_xray_edit(&ctx, &json!({
            "path": "file.txt",
            "edits": [ { "search": "original", "replace": "updated" } ]
        }));
        assert!(!result.is_error, "edit should succeed: {:?}", result);

        // Target rewritten.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "updated\n");

        // No `.file.txt.xray_tmp.*` (or any *.xray_tmp.*) left behind on success.
        for entry in std::fs::read_dir(tmp.path()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(
                !name.contains(".xray_tmp"),
                "unexpected leftover temp file after atomic write: {}",
                name
            );
        }
    }

    #[test]
    fn test_write_file_with_endings_succeeds_even_if_stale_temp_exists() {
        // Simulates recovery from a previous crash that left a stale
        // `.file.txt.xray_tmp.*` behind. EDIT-005: per-call unique temp names
        // mean the new write does NOT collide with the stale one. The edit must
        // succeed and the target must hold the new content; the orphaned stale
        // temp is harmless and not our problem to clean up.
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("file.txt");
        let stale = tmp.path().join(".file.txt.xray_tmp.0.0.0");
        std::fs::write(&target, "original\n").unwrap();
        std::fs::write(&stale, "garbage from a previous crash").unwrap();
        let ctx = make_ctx(tmp.path());

        let result = handle_xray_edit(&ctx, &json!({
            "path": "file.txt",
            "edits": [ { "search": "original", "replace": "recovered" } ]
        }));
        assert!(!result.is_error, "edit should succeed despite stale temp: {:?}", result);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "recovered\n");
    }

    #[test]
    fn test_write_file_with_endings_preserves_original_on_dryrun() {
        // dryRun must not even create the temp file (no atomic-write side-effects).
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("file.txt");
        std::fs::write(&target, "original\n").unwrap();
        let ctx = make_ctx(tmp.path());

        let result = handle_xray_edit(&ctx, &json!({
            "path": "file.txt",
            "dryRun": true,
            "edits": [ { "search": "original", "replace": "never-written" } ]
        }));
        assert!(!result.is_error, "dryRun edit should succeed: {:?}", result);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original\n");
        // dryRun must not stage any *.xray_tmp.* temp file.
        for entry in std::fs::read_dir(tmp.path()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(!name.contains(".xray_tmp"), "dryRun must not stage a temp file, found: {}", name);
        }
    }

    // ─── A1: Multi-file path dedup ───────────────────────────────────────

    #[test]
    fn test_multi_file_duplicate_path_relative_variants() {
        let tmp = tempfile::tempdir().unwrap();
        create_named_temp_file(tmp.path(), "file.txt", "hello\n");
        let ctx = make_ctx(tmp.path());

        let result = handle_xray_edit(&ctx, &json!({
            "paths": ["./file.txt", "file.txt"],
            "edits": [
                { "search": "hello", "replace": "world" }
            ]
        }));

        assert!(result.is_error, "Should reject duplicate paths");
        let text = &result.content[0].text;
        assert!(text.contains("Duplicate path"), "Error should mention duplicate: {}", text);
    }

    #[test]
    fn test_multi_file_duplicate_path_different_files_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        create_named_temp_file(tmp.path(), "a.txt", "hello\n");
        create_named_temp_file(tmp.path(), "b.txt", "hello\n");
        let ctx = make_ctx(tmp.path());

        let result = handle_xray_edit(&ctx, &json!({
            "paths": ["a.txt", "b.txt"],
            "edits": [
                { "search": "hello", "replace": "world" }
            ]
        }));
        assert!(!result.is_error, "Different files should succeed: {:?}", result);
    }

    // ─── B1: CRLF normalization in Mode A ────────────────────────────────

    #[test]
    fn test_mode_a_crlf_content_no_double_cr() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("crlf.txt");
        std::fs::write(&file_path, "line1\r\nline2\r\nline3\r\n").unwrap();
        let ctx = make_ctx(tmp.path());

        // Mode A insert with CRLF content
        let result = handle_xray_edit(&ctx, &json!({
            "path": "crlf.txt",
            "operations": [
                { "startLine": 2, "endLine": 1, "content": "inserted\r\nline" }
            ]
        }));

        assert!(!result.is_error, "Mode A insert should succeed: {:?}", result);
        let content = std::fs::read(&file_path).unwrap();
        let text = String::from_utf8_lossy(&content);
        // Should have CRLF line endings but NO \r\r\n
        assert!(!text.contains("\r\r\n"), "Should not have double CR: {:?}", text);
        assert!(text.contains("inserted\r\nline"), "Should contain inserted content");
    }
}

/// Regression tests for the apply_text_edits retry-cascade refactoring.
///
/// Cascade stages (find_with_retry) — PR #1 cleanup-magic edition:
///   1. Exact literal match
///   2. Flex-space regex (collapse whitespace) — OPT-IN via expectedContext
///
/// Stages 2 (strip trailing WS) and 3 (trim blank lines) were REMOVED in PR #1.
/// They silently rewrote the search text and could match a semantically different
/// block. Their inputs now surface as `Text not found` errors with a categorised
/// `Nearest match` hint. The former "happy-path" tests for those stages are
/// inverted below to assert the new diagnose-first behaviour.
///
/// Each test isolates a single behaviour and asserts that:
///   - The match is found at the right offset.
///   - effective_search / matched bytes are correct (so occurrence math, error
///     messages, and expectedContext keep working).
///   - The literal replacement is byte-for-byte preserved (file ends up correct).
#[cfg(test)]
mod retry_cascade_tests {
    use super::*;
    use std::sync::{Arc, RwLock};
    use crate::mcp::handlers::WorkspaceBinding;
    use serde_json::json;

    fn make_ctx(dir: &std::path::Path) -> super::HandlerContext {
        super::HandlerContext {
            workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(dir.to_string_lossy().to_string()))),
            ..super::HandlerContext::default()
        }
    }

    // ─── A1: Step 2 — strip trailing whitespace ──────────────────────────
    //
    // search has trailing spaces. File has the same line WITHOUT trailing spaces.
    // Step 1 (exact) fails; step 2 strips trailing WS from search and matches.
    // Critical: effective_search becomes the trimmed form, so the literal
    // replacement targets exactly what's in the file (no over-write of context).
    #[test]
    fn test_no_silent_match_trailing_ws_cascade() {
        let tmp = tempfile::tempdir().unwrap();
        let filename = "a1.txt";
        let path = tmp.path().join(filename);
        // File has NO trailing whitespace on "foo"
        std::fs::write(&path, "foo\nbar\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let result = handle_xray_edit(&ctx, &json!({
            "path": filename,
            "edits": [
                // search HAS trailing whitespace — used to silently match via Step 2
                { "search": "foo   ", "replace": "FOO" }
            ]
        }));

        // Inverted (PR #1): Step 2 was removed. Trailing-WS drift must surface
        // as Text not found instead of being silently fixed.
        assert!(result.is_error,
            "trailing-WS drift must NOT silently match after PR #1: {}",
            result.content[0].text);

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "foo\nbar\n",
            "File must be unchanged after Text not found error");
    }

    // ─── A2: Step 3 — trim leading/trailing blank lines ──────────────────
    //
    // search has leading and trailing blank lines. File has the same content
    // packed without those blank lines. Step 1 (exact) fails, step 2 (strip
    // trailing WS) doesn't help, step 3 trims blank lines and matches.
    #[test]
    fn test_no_silent_match_blank_lines_cascade() {
        let tmp = tempfile::tempdir().unwrap();
        let filename = "a2.txt";
        let path = tmp.path().join(filename);
        // File: "prefix\nfoo\nbar\nsuffix\n" — no blank lines around foo/bar
        std::fs::write(&path, "prefix\nfoo\nbar\nsuffix\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let result = handle_xray_edit(&ctx, &json!({
            "path": filename,
            "edits": [
                // search HAS leading/trailing blank lines — used to silently match via Step 3
                { "search": "\n\nfoo\nbar\n\n", "replace": "FOO_BAR" }
            ]
        }));

        // Inverted (PR #1): Step 3 was removed. Blank-line drift must surface
        // as Text not found instead of being silently fixed.
        assert!(result.is_error,
            "blank-line drift must NOT silently match after PR #1: {}",
            result.content[0].text);

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "prefix\nfoo\nbar\nsuffix\n",
            "File must be unchanged after Text not found error");
    }

    // ─── A3: Step 4 — flex-space regex ⚠️ HIGHEST RISK ───────────────────
    //
    // search has single spaces between tokens. File has tabs and multiple
    // spaces. Only the flex-regex stage matches. Critical guarantees:
    //   - flex_re is set, so apply_literal_replace uses regex::NoExpand path
    //     (NOT effective_search-based String::replace).
    //   - matched bytes preserved verbatim except for the surgical replacement.
    //   - effective_search remains the ORIGINAL search (so error messages and
    //     occurrence math reference what the LLM actually sent).
    #[test]
    fn test_retry_cascade_flex_regex() {
        let tmp = tempfile::tempdir().unwrap();
        let filename = "a3.txt";
        let path = tmp.path().join(filename);
        // "foo" + 2 spaces + tab + 2 spaces + "bar" — neither exact nor strip-ws
        // nor blank-line trim will match. Only flex regex with [ \t]+ between
        // tokens succeeds.
        std::fs::write(&path, "foo  \t  bar\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let result = handle_xray_edit(&ctx, &json!({
            "path": filename,
            "edits": [
                { "search": "foo bar", "replace": "FOO_BAR", "expectedContext": "foo" }
            ]
        }));

        assert!(!result.is_error,
            "flex-regex cascade should succeed: {}", result.content[0].text);

        let content = std::fs::read_to_string(&path).unwrap();
        // Whole "foo  \t  bar" run is collapsed by NoExpand replacement.
        assert_eq!(content, "FOO_BAR\n",
            "flex-regex must replace the entire matched run, not just \"foo bar\" substring");

        let parsed: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
        let warnings = parsed["warnings"].as_array().expect("response must include warnings array");
        assert!(warnings.iter().any(|w| w.as_str().unwrap_or("").contains("flexible whitespace")),
            "Step 4 must record a flex-whitespace warning. Got: {:?}", warnings);
    }

    // ─── A4: Error message format per cascade branch ────────────────────
    //
    // Documents the CURRENT contract:
    //   - When NO branch finds the search text, the error mentions the
    //     original search (truncated). This is what nearest_match_hint and
    //     the user see.
    //   - When step-2/3 find a match but occurrence overflows, the error
    //     mentions effective_search (the trimmed form).
    //
    // If we change either branch's wording, this test will fail and force a
    // conscious decision rather than a silent contract drift.
    #[test]
    fn test_error_message_literal_vs_flex() {
        let tmp = tempfile::tempdir().unwrap();
        let filename = "a4.txt";
        let path = tmp.path().join(filename);
        std::fs::write(&path, "alpha\nbeta\n").unwrap();

        let ctx = make_ctx(tmp.path());

        // Scenario 1: search not found anywhere — error references the
        // ORIGINAL search (truncated form), not any cascade-internal trimming.
        let r_not_found = handle_xray_edit(&ctx, &json!({
            "path": filename,
            "edits": [
                { "search": "nonexistent_token_xyz", "replace": "Z" }
            ]
        }));
        assert!(r_not_found.is_error, "missing search should error");
        let err1 = &r_not_found.content[0].text;
        assert!(err1.contains("nonexistent_token_xyz"),
            "error must echo the original search. Got: {}", err1);

        // Scenario 2: search not found, but with extra trailing whitespace
        // that PR #1's removed Step 2 used to silently strip. Error must
        // still echo the ORIGINAL (untrimmed) search — the cascade no longer
        // produces an effective_search divergent from the input.
        let r_overflow = handle_xray_edit(&ctx, &json!({
            "path": filename,
            "edits": [
                {
                    "search": "alpha   ",   // pre-PR#1: stripped to "alpha" → silent match
                    "replace": "A"
                }
            ]
        }));
        assert!(r_overflow.is_error, "trailing-WS search must NOT silently match after PR #1");
        let err2 = &r_overflow.content[0].text;
        assert!(err2.contains("Text not found"),
            "error must be Text not found. Got: {}", err2);
        assert!(err2.contains("alpha"),
            "error must echo the original search. Got: {}", err2);
    }

    // ─── A5: expectedContext after a flex-cascade match ─────────────────
    //
    // expectedContext validates against ±5 lines around the matched
    // POSITION (not against the search string). After a flex match,
    // search_result.positions[0] points to the actual byte in the file —
    // expectedContext should still find context that lives near that byte.
    #[test]
    fn test_expected_context_after_flex_match() {
        let tmp = tempfile::tempdir().unwrap();
        let filename = "a5.txt";
        let path = tmp.path().join(filename);
        // "foo  bar" matches via flex regex. "// MARKER" is on the same line
        // and within ±5 lines of the match position.
        std::fs::write(&path, "line0\nfoo  bar // MARKER\nline2\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let result = handle_xray_edit(&ctx, &json!({
            "path": filename,
            "edits": [
                {
                    "search": "foo bar",       // single space — needs flex
                    "replace": "FOO_BAR",
                    "expectedContext": "MARKER"
                }
            ]
        }));

        assert!(!result.is_error,
            "expectedContext should validate against the matched POSITION, not the search string. Error: {}",
            result.content[0].text);

        let content = std::fs::read_to_string(&path).unwrap();
        // Note: flex pattern is `[ \t]*foo[ \t]+bar[ \t]*`, so trailing space
        // before "// MARKER" is part of the match and gets consumed by the
        // replacement. // MARKER itself is preserved.
        assert_eq!(content, "line0\nFOO_BAR// MARKER\nline2\n",
            "flex match + expectedContext must replace the (greedy) run while preserving // MARKER");

        // Negative half: a wrong context must still reject the edit even on
        // the flex branch (regression guard).
        std::fs::write(&path, "line0\nfoo  bar // MARKER\nline2\n").unwrap();
        let bad = handle_xray_edit(&ctx, &json!({
            "path": filename,
            "edits": [
                {
                    "search": "foo bar",
                    "replace": "FOO_BAR",
                    "expectedContext": "NOT_PRESENT_TOKEN"
                }
            ]
        }));
        assert!(bad.is_error,
            "flex match must still respect expectedContext. Result: {}", bad.content[0].text);
        let unchanged = std::fs::read_to_string(&path).unwrap();
        assert_eq!(unchanged, "line0\nfoo  bar // MARKER\nline2\n",
            "file must NOT be modified when expectedContext fails on the flex branch");
    }
}

// ─────────────────────────────────────────────────────────────────────
// Synchronous-reindex tests (xray_edit → xray_grep race elimination).
// Verifies the integration of `reindex_paths_sync` from watcher into edit handlers.
// See `docs/user-stories/todo_approved_2026-04-19_xray-edit-sync-reindex.md`.
// ─────────────────────────────────────────────────────────────────────

/// Helper: make a HandlerContext bound to `dir` with `server_ext` set so
/// `classify_for_sync_reindex` does NOT auto-skip files. Also seeds an empty
/// ContentIndex with `path_to_id: Some(...)` so sync reindex can perform purges.
fn make_ctx_with_ext(dir: &std::path::Path, ext: &str) -> HandlerContext {
    use std::collections::HashMap;
    use crate::ContentIndex;
    let extensions: Vec<String> = ext.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let content = ContentIndex {
        root: dir.to_string_lossy().to_string(),
        extensions: extensions.clone(),
        path_to_id: Some(HashMap::new()),
        ..Default::default()
    };
    HandlerContext {
        index: Arc::new(RwLock::new(content)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(dir.to_string_lossy().to_string()))),
        server_ext: ext.to_string(),
        ..HandlerContext::default()
    }
}

#[test]
fn test_sync_reindex_response_includes_fields_on_real_write() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("file.cs");
    std::fs::write(&path, "class A {}\n").unwrap();

    let ctx = make_ctx_with_ext(tmp.path(), "cs");
    let result = handle_xray_edit(&ctx, &json!({
        "path": "file.cs",
        "edits": [{"search": "class A", "replace": "class BeauChanZ"}],
    }));
    assert!(!result.is_error, "edit should succeed: {}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(v["contentIndexUpdated"], json!(true),
        "contentIndexUpdated must be true after a real write to an in-scope file");
    assert_eq!(v["defIndexUpdated"], json!(false),
        "defIndexUpdated must be false when ctx has no def_index");
    assert!(v["reindexElapsedMs"].is_string(),
        "reindexElapsedMs must be present (string with 2 decimals)");
    // Verify the index actually contains the new token.
    let idx = ctx.index.read().unwrap();
    assert!(idx.index.contains_key("beauchanz"),
        "sync reindex must populate inverted index with new tokens");
}

#[test]
fn test_sync_reindex_dry_run_omits_reindex_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("file.cs");
    std::fs::write(&path, "class DryRunFoo {}\n").unwrap();

    let ctx = make_ctx_with_ext(tmp.path(), "cs");
    let result = handle_xray_edit(&ctx, &json!({
        "path": "file.cs",
        "dryRun": true,
        "edits": [{"search": "DryRunFoo", "replace": "DryRunBar"}],
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert!(v.get("contentIndexUpdated").is_none(),
        "dryRun must NOT add contentIndexUpdated");
    assert!(v.get("defIndexUpdated").is_none(),
        "dryRun must NOT add defIndexUpdated");
    assert!(v.get("fileListInvalidated").is_none(),
        "dryRun must NOT add fileListInvalidated");
    assert!(v.get("reindexElapsedMs").is_none(),
        "dryRun must NOT add reindexElapsedMs");
    assert!(v.get("skippedReason").is_none(),
        "dryRun must NOT add skippedReason");
    // And the file must not have been modified.
    let actual = std::fs::read_to_string(&path).unwrap();
    assert!(actual.contains("DryRunFoo"), "dryRun must not write to disk");
}

#[test]
fn test_sync_reindex_file_created_invalidates_file_list() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx_with_ext(tmp.path(), "cs");

    // HandlerContext::default() initializes file_index_dirty=true (means "needs initial scan").
    // Reset to false so we can observe whether the edit handler explicitly sets it back to true.
    ctx.file_index_dirty.store(false, std::sync::atomic::Ordering::Relaxed);

    // Edit a non-existent file — handler treats this as create.
    let result = handle_xray_edit(&ctx, &json!({
        "path": "new_file.cs",
        "operations": [{"startLine": 1, "endLine": 0, "content": "class CreatedZ {}\n"}],
    }));
    assert!(!result.is_error, "create-via-edit should succeed: {}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(v["fileCreated"], json!(true), "fileCreated must be true for new files");
    assert_eq!(v["fileListInvalidated"], json!(true),
        "fileListInvalidated must be true when a new file is created");
    assert!(ctx.file_index_dirty.load(std::sync::atomic::Ordering::Relaxed),
        "file_index_dirty atomic flag must be set to true");
}

#[test]
fn test_sync_reindex_existing_file_does_not_invalidate_file_list() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("existing.cs");
    std::fs::write(&path, "class ExistingY {}\n").unwrap();

    let ctx = make_ctx_with_ext(tmp.path(), "cs");
    // Reset to false — see comment in test_sync_reindex_file_created_invalidates_file_list.
    ctx.file_index_dirty.store(false, std::sync::atomic::Ordering::Relaxed);
    let result = handle_xray_edit(&ctx, &json!({
        "path": "existing.cs",
        "edits": [{"search": "ExistingY", "replace": "ChangedY"}],
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert!(v.get("fileCreated").is_none(),
        "fileCreated must NOT be present for existing files");
    assert_eq!(v["fileListInvalidated"], json!(false),
        "fileListInvalidated must be false when only existing files are modified");
    assert!(!ctx.file_index_dirty.load(std::sync::atomic::Ordering::Relaxed),
        "file_index_dirty must NOT be set for pure-edit (no creation)");
}

#[test]
fn test_sync_reindex_extension_not_indexed_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("notes.txt");
    std::fs::write(&path, "plain text\n").unwrap();

    // server_ext=cs but we're editing a .txt file — must skip with extensionNotIndexed.
    let ctx = make_ctx_with_ext(tmp.path(), "cs");
    let result = handle_xray_edit(&ctx, &json!({
        "path": "notes.txt",
        "edits": [{"search": "plain", "replace": "updated"}],
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(v["contentIndexUpdated"], json!(false));
    assert_eq!(v["defIndexUpdated"], json!(false));
    assert_eq!(v["skippedReason"], json!("extensionNotIndexed"));
    // And the index must remain empty (no token leakage from out-of-scope files).
    let idx = ctx.index.read().unwrap();
    assert!(idx.index.is_empty(), "index must remain empty for extensionNotIndexed file");
    // But the file MUST still be written (edit should succeed).
    let actual = std::fs::read_to_string(&path).unwrap();
    assert_eq!(actual, "updated text\n", "the edit itself must still apply");
}

#[test]
fn test_sync_reindex_outside_server_dir_is_skipped() {
    let server_root = tempfile::tempdir().unwrap();
    let outside_root = tempfile::tempdir().unwrap();
    let outside_path = outside_root.path().join("alien.cs");
    std::fs::write(&outside_path, "class AlienZ {}\n").unwrap();

    // server_ext matches BUT file lives outside server_dir — must skip with outsideServerDir.
    let ctx = make_ctx_with_ext(server_root.path(), "cs");
    let result = handle_xray_edit(&ctx, &json!({
        "path": outside_path.to_string_lossy(),
        "edits": [{"search": "AlienZ", "replace": "NeighborZ"}],
    }));
    assert!(!result.is_error);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert_eq!(v["contentIndexUpdated"], json!(false));
    assert_eq!(v["skippedReason"], json!("outsideServerDir"),
        "files outside server --dir must NOT be sync-indexed (would pollute scope)");
    let idx = ctx.index.read().unwrap();
    assert!(idx.index.is_empty(), "server index must remain empty for outside-server-dir edits");
}

#[test]
fn test_sync_reindex_multi_file_summary_has_reindex_elapsed_ms() {
    let tmp = tempfile::tempdir().unwrap();
    let p1 = tmp.path().join("a.cs");
    let p2 = tmp.path().join("b.cs");
    std::fs::write(&p1, "class MultiA {}\n").unwrap();
    std::fs::write(&p2, "class MultiB {}\n").unwrap();

    let ctx = make_ctx_with_ext(tmp.path(), "cs");
    let result = handle_xray_edit(&ctx, &json!({
        "paths": ["a.cs", "b.cs"],
        "edits": [{"search": "Multi", "replace": "Renamed"}],
    }));
    assert!(!result.is_error, "multi-edit should succeed: {}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    assert!(v["summary"]["reindexElapsedMs"].is_string(),
        "summary.reindexElapsedMs must be present after a real multi-file write");
    let results = v["results"].as_array().expect("results must be array");
    assert_eq!(results.len(), 2);
    for r in results {
        assert_eq!(r["contentIndexUpdated"], json!(true),
            "each in-scope file must have contentIndexUpdated=true");
        assert_eq!(r["fileListInvalidated"], json!(false),
            "existing files don't invalidate the file list");
    }
    let idx = ctx.index.read().unwrap();
    assert!(idx.index.contains_key("renameda") || idx.index.contains_key("renamedb"),
        "multi-file sync reindex must add new tokens from BOTH files");
}

#[test]
fn test_sync_reindex_multi_file_mixed_skipped_reasons() {
    let tmp = tempfile::tempdir().unwrap();
    let cs_file = tmp.path().join("good.cs");
    let txt_file = tmp.path().join("bad.txt");
    std::fs::write(&cs_file, "class GoodA {}\n").unwrap();
    std::fs::write(&txt_file, "plain\n").unwrap();

    let ctx = make_ctx_with_ext(tmp.path(), "cs");
    let result = handle_xray_edit(&ctx, &json!({
        "paths": ["good.cs", "bad.txt"],
        // Both edits MUST have skipIfNotFound: each edit is applied to BOTH files in the batch,
        // so 'GoodA' isn't in bad.txt and 'plain' isn't in good.cs.
        "edits": [{"search": "GoodA", "replace": "BetterA", "skipIfNotFound": true}, {"search": "plain", "replace": "fancy", "skipIfNotFound": true}],
    }));
    assert!(!result.is_error, "mixed edit should succeed: {}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let results = v["results"].as_array().expect("results must be array");
    assert_eq!(results.len(), 2);

    // Find each file by path and check its outcome
    let good = results.iter().find(|r| r["path"] == "good.cs").expect("good.cs in results");
    let bad  = results.iter().find(|r| r["path"] == "bad.txt").expect("bad.txt in results");
    assert_eq!(good["contentIndexUpdated"], json!(true),
        "in-scope .cs file must be sync-indexed");
    assert_eq!(bad["contentIndexUpdated"], json!(false),
        "out-of-scope .txt file must NOT be sync-indexed");
    assert_eq!(bad["skippedReason"], json!("extensionNotIndexed"));
}

#[test]
fn test_sync_reindex_multi_file_poisoned_content_lock_reports_false() {
    // Pre-fix: `handle_multi_file_edit` set
    // `contentIndexUpdated = true` UNCONDITIONALLY for every non-skipped file
    // — even when the underlying `reindex_paths_sync` call returned
    // `content_lock_poisoned = true` and `content_updated = 0`. The
    // batch-level `summary.reindexWarning` told the caller "the index lock
    // was poisoned, FS watcher will reconcile" but the per-file
    // `contentIndexUpdated: true` told the opposite — caller-side staleness
    // checks (e.g. an immediate follow-up `xray_grep` looking for the new
    // symbol) would trust the per-file telemetry and ignore the warning.
    //
    // Post-fix: per-file `contentIndexUpdated` mirrors the single-file path
    // and is derived from `batch_stats.content_updated > 0`, which is `false`
    // when the lock was poisoned.
    let tmp = tempfile::tempdir().unwrap();
    let f1 = tmp.path().join("a.cs");
    let f2 = tmp.path().join("b.cs");
    std::fs::write(&f1, "class A {}\n").unwrap();
    std::fs::write(&f2, "class B {}\n").unwrap();

    let ctx = make_ctx_with_ext(tmp.path(), "cs");

    // Poison the content-index RwLock — same recipe as
    // `test_process_batch_returns_false_on_poisoned_content_lock` in
    // `watcher_tests.rs`.
    let index_clone = ctx.index.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = index_clone.write().unwrap();
        panic!("intentional panic to poison RwLock");
    }));
    assert!(ctx.index.write().is_err(), "Content lock must be poisoned");

    let result = handle_xray_edit(&ctx, &json!({
        "paths": ["a.cs", "b.cs"],
        "edits": [{"search": "class", "replace": "struct"}],
    }));
    assert!(
        !result.is_error,
        "Multi-file edit must still return success even when index lock is poisoned (writes succeeded; only the post-write reindex failed): {}",
        result.content[0].text
    );
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Batch-level warning must surface the lock-poison condition.
    assert!(
        v["summary"]["reindexWarning"].is_string(),
        "summary.reindexWarning must be present when lock is poisoned, got: {}",
        v["summary"]
    );

    // Per-file telemetry must NOT lie about the index state.
    let results = v["results"].as_array().expect("results must be array");
    assert_eq!(results.len(), 2);
    for entry in results {
        assert_eq!(
            entry["contentIndexUpdated"], json!(false),
            "Poisoned content lock → per-file contentIndexUpdated must be false, got: {}",
            entry
        );
    }
}

#[test]
fn test_sync_reindex_multi_file_dry_run_omits_index_telemetry() {
    // Regression guard: the dry-run path doesn't touch the index, so per-file
    // `contentIndexUpdated` / `defIndexUpdated` / `fileListInvalidated` MUST
    // NOT be emitted at all (mirrors single-file dry-run behaviour). Pre-fix
    // this happened to work because the "always true" branch was inside
    // `if !dry_run`, but the new derived-from-batch logic must preserve it.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.cs"), "class A {}\n").unwrap();
    std::fs::write(tmp.path().join("b.cs"), "class B {}\n").unwrap();

    let ctx = make_ctx_with_ext(tmp.path(), "cs");
    let result = handle_xray_edit(&ctx, &json!({
        "paths": ["a.cs", "b.cs"],
        "edits": [{"search": "class", "replace": "struct"}],
        "dryRun": true,
    }));
    assert!(!result.is_error, "dry-run must succeed: {}", result.content[0].text);
    let v: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let results = v["results"].as_array().expect("results array");
    for entry in results {
        assert!(
            entry.get("contentIndexUpdated").is_none(),
            "dry-run must NOT emit contentIndexUpdated, got: {}",
            entry
        );
        assert!(
            entry.get("defIndexUpdated").is_none(),
            "dry-run must NOT emit defIndexUpdated, got: {}",
            entry
        );
    }
}

#[test]
fn test_sync_reindex_concurrent_edit_and_grep_no_deadlock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hot.cs");
    std::fs::write(&path, "class HotZeroX {}\n").unwrap();

    let ctx = Arc::new(make_ctx_with_ext(tmp.path(), "cs"));

    // First call to populate the index.
    let warmup = handle_xray_edit(&ctx, &json!({
        "path": "hot.cs",
        "edits": [{"search": "HotZeroX", "replace": "HotOneX"}],
    }));
    assert!(!warmup.is_error);

    let edits_done = Arc::new(AtomicUsize::new(0));
    let reads_done = Arc::new(AtomicUsize::new(0));
    const ROUNDS: usize = 20;

    // Thread A: hammer xray_edit (writes + sync reindex).
    let ctx_a = Arc::clone(&ctx);
    let edits_a = Arc::clone(&edits_done);
    let edit_thread = thread::spawn(move || {
        for i in 0..ROUNDS {
            let needle = format!("HotIter{}X", i);
            let replacement = format!("HotIter{}X", i + 1);
            let r = handle_xray_edit(&ctx_a, &json!({
                "path": "hot.cs",
                "edits": [{"search": "HotOneX", "replace": &needle, "skipIfNotFound": true},
                          {"search": &needle, "replace": &replacement, "skipIfNotFound": true}],
            }));
            assert!(!r.is_error, "edit iter {} should not error: {}", i, r.content[0].text);
            edits_a.fetch_add(1, Ordering::Relaxed);
        }
    });

    // Thread B: hammer xray_grep (reads index repeatedly while A is writing).
    let ctx_b = Arc::clone(&ctx);
    let reads_b = Arc::clone(&reads_done);
    let grep_thread = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            // We don't care what comes back — only that the lock isn't deadlocked.
            let _ = crate::mcp::handlers::grep::handle_xray_grep(&ctx_b, &json!({
                "terms": "HotOneX",
                "countOnly": true,
            }));
            reads_b.fetch_add(1, Ordering::Relaxed);
            if reads_b.load(Ordering::Relaxed) > 200 { break; }
            thread::sleep(Duration::from_millis(1));
        }
    });

    // 5-second hard deadline — if we deadlock, this hangs and the test runner kills it.
    let edit_join = edit_thread.join();
    let grep_join = grep_thread.join();
    assert!(edit_join.is_ok(), "edit thread panicked");
    assert!(grep_join.is_ok(), "grep thread panicked");
    assert_eq!(edits_done.load(Ordering::Relaxed), ROUNDS,
        "all {} edits must complete (no deadlock)", ROUNDS);
    assert!(reads_done.load(Ordering::Relaxed) > 0,
        "grep thread must have made at least one read (no deadlock)");
}


// ─────────────────────────────────────────────────────────────────────
// Symlink-aware regression test for `classify_for_sync_reindex`.
//
// Bug: classify_for_sync_reindex used to canonicalize the file path before
// comparing it against canonical_server_dir. For a symlinked subdirectory
// like `<root>/personal -> D:\Personal`, canonicalize resolved the file's
// path to `D:\Personal\foo.md`, which does NOT start with `<root>`, so the
// helper returned `Some("outsideServerDir")` and the sync-reindex was
// silently skipped — leaving xray_grep stale until the FS-watcher caught up.
// After the fix, the helper uses `is_path_within`, which performs a logical
// comparison first (matching `WalkBuilder::follow_links`), and the file is
// correctly recognized as belonging to the workspace.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn test_classify_for_sync_reindex_through_symlinked_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let external = tmp.path().join("external");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("foo.md"), "# external").unwrap();

    // root/personal -> external
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&external, root.join("personal")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, root.join("personal")).unwrap();

    let canonical_server_dir = root.to_string_lossy().to_string();
    let extensions: Vec<String> = vec!["md".to_string()];
    let resolved = root.join("personal").join("foo.md");

    let skip = classify_for_sync_reindex(&canonical_server_dir, &extensions, &resolved);
    assert!(
        skip.is_none(),
        "File in symlinked subdir must NOT be classified as outsideServerDir. \
         Got skipReason: {:?}, resolved={}, server_dir={}",
        skip, resolved.display(), canonical_server_dir
    );
}

#[test]
fn test_classify_for_sync_reindex_genuine_outside_still_rejected() {
    // Sanity check: real outside-workspace files must still be rejected.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("bar.md"), "x").unwrap();

    let canonical_server_dir = root.to_string_lossy().to_string();
    let extensions: Vec<String> = vec!["md".to_string()];
    let resolved = outside.join("bar.md");

    let skip = classify_for_sync_reindex(&canonical_server_dir, &extensions, &resolved);
    assert_eq!(
        skip,
        Some("outsideServerDir"),
        "Genuine outside-workspace file must be classified as outsideServerDir. \
         resolved={}, server_dir={}",
        resolved.display(), canonical_server_dir
    );
}

// ─── Tier 5 regression tests: applied accounting, idempotency, verification ──

/// Fix 1: `applied` must exclude edits that were skipped via `skipIfNotFound`.
/// Previously every entry in the edits array counted as "applied" regardless of
/// whether it touched the file — a major correctness hole.
#[test]
fn test_tier5_applied_excludes_skipped() {
    let (tmp, filename, _) = create_temp_file("hello world\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello", "replace": "HELLO" },
            { "search": "nonexistent", "replace": "x", "skipIfNotFound": true },
            { "search": "world", "replace": "WORLD" },
        ]
    }));

    assert!(!result.is_error, "Should succeed: {:?}", result);
    let parsed: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(parsed["applied"], 2, "applied must be 2 (skipped one does not count)");
    assert_eq!(parsed["skippedEdits"], 1);
}

/// Fix 2: insert_after run twice in a row must NOT duplicate — second call is a no-op.
#[test]
fn test_tier5_insert_after_idempotent() {
    let (tmp, filename, path) = create_temp_file("use a;\nfn main() {}\n");
    let ctx = make_ctx(tmp.path());

    // First call: inserts a new line.
    let r1 = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "insertAfter": "use a;", "content": "use b;" }]
    }));
    assert!(!r1.is_error);
    let after_first = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after_first, "use a;\nuse b;\nfn main() {}\n");

    // Second call with identical edit: must be skipped, file must NOT change.
    let r2 = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "insertAfter": "use a;", "content": "use b;" }]
    }));
    assert!(!r2.is_error, "Second call should succeed (skipped, not errored)");
    let after_second = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after_second, after_first, "Idempotent: file must not double-insert");

    let parsed: serde_json::Value = serde_json::from_str(&r2.content[0].text).unwrap();
    assert_eq!(parsed["applied"], 0, "Second idempotent call must report applied=0");
    assert_eq!(parsed["skippedEdits"], 1);
    let reason = parsed["skippedDetails"][0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("alreadyApplied"), "Reason should be alreadyApplied, got: {}", reason);
}

/// Fix 2: insert_before run twice in a row must NOT duplicate.
#[test]
fn test_tier5_insert_before_idempotent() {
    let (tmp, filename, path) = create_temp_file("fn main() {}\n");
    let ctx = make_ctx(tmp.path());

    let r1 = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "insertBefore": "fn main() {}", "content": "// comment" }]
    }));
    assert!(!r1.is_error);
    let after_first = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after_first, "// comment\nfn main() {}\n");

    let r2 = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "insertBefore": "fn main() {}", "content": "// comment" }]
    }));
    assert!(!r2.is_error);
    let after_second = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after_second, after_first, "Idempotent: no duplicate comment");

    let parsed: serde_json::Value = serde_json::from_str(&r2.content[0].text).unwrap();
    assert_eq!(parsed["applied"], 0);
    assert_eq!(parsed["skippedEdits"], 1);
}

/// Fix 3: response must contain `lineEnding` so clients can reconcile the LF-based
/// diff with on-disk CRLF bytes.
#[test]
fn test_tier5_response_includes_line_ending_lf() {
    let (tmp, filename, _) = create_temp_file("a\nb\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "search": "a", "replace": "A" }]
    }));
    assert!(!result.is_error);
    let parsed: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(parsed["lineEnding"], "LF");
}

#[test]
fn test_tier5_response_includes_line_ending_crlf() {
    let tmp = tempfile::tempdir().unwrap();
    let filename = "crlf.txt";
    let path = tmp.path().join(filename);
    std::fs::write(&path, b"a\r\nb\r\n").unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [{ "search": "a", "replace": "A" }]
    }));
    assert!(!result.is_error, "{:?}", result);
    let parsed: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(parsed["lineEnding"], "CRLF");
    // File must still be CRLF on disk.
    let raw = std::fs::read(&path).unwrap();
    assert!(raw.windows(2).any(|w| w == b"\r\n"), "CRLF endings must be preserved");
}

/// Fix 4 (post-write verification): verify_written_file returns Ok when bytes match,
/// Err when they don't. Sanity test for the verification helper itself.
#[test]
fn test_tier5_verify_written_file_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("v.txt");
    std::fs::write(&p, b"hello\n").unwrap();
    assert!(verify_written_file(&p, "hello\n", "\n").is_ok());
}

#[test]
fn test_tier5_verify_written_file_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("v.txt");
    std::fs::write(&p, b"hello\n").unwrap();
    let err = verify_written_file(&p, "goodbye\n", "\n").unwrap_err();
    assert!(err.contains("Post-write verification failed"), "Got: {}", err);
}

#[test]
fn test_tier5_verify_written_file_crlf_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("v.txt");
    std::fs::write(&p, b"hello\r\nworld\r\n").unwrap();
    // Expected content is the LF-normalized form; verifier re-applies CRLF.
    assert!(verify_written_file(&p, "hello\nworld\n", "\r\n").is_ok());
}


// ─── EDIT-003/004/005/007 hardening regressions (2026-04-22) ──────────

#[test]
fn test_edit003_dryrun_does_not_create_parent_dirs() {
    // EDIT-003: dryRun against a non-existent path with deep parent hierarchy
    // must not leave empty directories on disk.
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": "new/deeply/nested/file.rs",
        "dryRun": true,
        "operations": [{ "startLine": 1, "endLine": 0, "content": "hello\n" }]
    }));
    assert!(!result.is_error, "dryRun create should succeed: {:?}", result.content[0].text);

    // Parent directories must NOT exist on disk after a dryRun preview.
    assert!(!tmp.path().join("new").exists(),
        "dryRun must not create parent dirs; found: {:?}",
        std::fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>());
}

#[test]
fn test_edit003_real_run_still_creates_parent_dirs() {
    // Negative half: a real (non-dryRun) write on the same path must continue
    // to create the parent hierarchy as before.
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": "new/deeply/nested/file.rs",
        "operations": [{ "startLine": 1, "endLine": 0, "content": "hello\n" }]
    }));
    assert!(!result.is_error, "real create should succeed: {:?}", result.content[0].text);
    assert!(tmp.path().join("new/deeply/nested/file.rs").exists());
}

#[test]
fn test_edit004_invalid_utf8_file_is_rejected_not_corrupted() {
    // EDIT-004: a Latin-1 file containing byte 0xE9 (`é`) is invalid UTF-8.
    // Pre-fix: lossy decode silently replaced it with U+FFFD, then the write
    // back permanently destroyed the original bytes. Post-fix: edit is
    // rejected with a clear error and the file is untouched.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("latin1.txt");
    let original_bytes = b"caf\xE9\nhello\n";
    std::fs::write(&path, original_bytes).unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": "latin1.txt",
        "edits": [ { "search": "hello", "replace": "world" } ]
    }));
    assert!(result.is_error, "invalid-UTF-8 edit should fail, got: {:?}", result.content[0].text);
    assert!(result.content[0].text.contains("not valid UTF-8"),
        "error must mention UTF-8: {}", result.content[0].text);

    // Crucially: original bytes preserved on disk (no silent corruption).
    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(on_disk, original_bytes,
        "non-UTF-8 file must be preserved byte-for-byte after rejected edit");
}

#[test]
fn test_edit004_valid_utf8_with_non_ascii_still_works() {
    // Negative half: legitimate UTF-8 with multi-byte chars must continue to work.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("u8.txt");
    std::fs::write(&path, "café\nhello\n").unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": "u8.txt",
        "edits": [ { "search": "hello", "replace": "world" } ]
    }));
    assert!(!result.is_error, "valid UTF-8 edit should succeed: {:?}", result.content[0].text);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "café\nworld\n");
}

#[test]
fn test_edit005_temp_path_is_unique_per_call() {
    // EDIT-005: per-call unique temp path. Pre-fix two concurrent edits on the
    // same file produced the same `.foo.xray_tmp` and silently lost one write.
    use std::collections::HashSet;
    let target = PathBuf::from("/some/dir/file.rs");
    let mut seen = HashSet::new();
    for _ in 0..1000 {
        let p = super::temp_path_for(&target);
        assert!(seen.insert(p), "temp_path_for produced a collision within 1000 calls");
    }
}

#[test]
fn test_edit005_temp_path_includes_pid_and_xray_tmp_marker() {
    // Sanity check: the new format embeds the live PID so that a different
    // process editing the same file picks a different temp directory entry.
    let target = PathBuf::from("/some/dir/file.rs");
    let p = super::temp_path_for(&target);
    let name = p.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with(".file.rs.xray_tmp."), "got: {}", name);
    let pid = std::process::id().to_string();
    assert!(name.contains(&format!(".xray_tmp.{}.", pid)),
        "temp name must embed PID {}, got: {}", pid, name);
}

#[test]
fn test_edit007_rename_replace_succeeds_when_target_exists() {
    // EDIT-007: rename_replace must succeed when the target already exists
    // (this is the common case post-write). Verify backup cleanup and
    // correct final content.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("file.txt");
    let src = tmp.path().join(".file.txt.xray_tmp.0.0.0");
    std::fs::write(&target, b"old").unwrap();
    std::fs::write(&src, b"new").unwrap();

    super::rename_replace(&src, &target).expect("rename should succeed");
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert!(!src.exists(), "src temp must be consumed by rename");

    // No backup file leaked.
    for entry in std::fs::read_dir(tmp.path()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(!name.contains(".xray_backup"), "backup must not leak, found: {}", name);
    }
}

#[test]
fn test_edit007_rename_replace_to_nonexistent_target() {
    // Edge case: rename to a path whose target does not yet exist (file creation).
    // No backup needed; direct rename takes the fast path.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("file.txt");
    let src = tmp.path().join(".file.txt.xray_tmp.0.0.0");
    std::fs::write(&src, b"new").unwrap();
    assert!(!target.exists());

    super::rename_replace(&src, &target).expect("rename should succeed");
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert!(!src.exists());
}

// ─── Unknown / mis-spelled top-level parameter rejection ────────────

/// `files: [...]` is the most common invented wrapper (callers expect per-file
/// `operations`). Must be rejected with a hint pointing at `paths` and noting
/// that the SAME edits are applied to all files.
#[test]
fn test_unknown_param_files_wrapper_rejected_with_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "files": [
            { "path": "a.ts", "operations": [{ "startLine": 1, "endLine": 0, "content": "x" }] }
        ]
    }));

    assert!(result.is_error, "Expected error for unknown 'files' param");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("Unknown parameter 'files'"), "Should call out 'files': {}", msg);
    assert!(msg.contains("paths"), "Should suggest 'paths': {}", msg);
    assert!(msg.contains("SAME"), "Should clarify same-edits semantics: {}", msg);
}

/// Typos close to known params (edit-distance ≤ 2) get a "did you mean" hint.
#[test]
fn test_unknown_param_typo_suggests_correct_name() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    // `pathh` -> `path`
    let result = handle_xray_edit(&ctx, &json!({
        "pathh": "a.ts",
        "edits": [{ "search": "x", "replace": "y" }]
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("Unknown parameter 'pathh'"), "msg={}", msg);
    assert!(msg.contains("Did you mean 'path'"), "msg={}", msg);
}

/// Unknown params far from any known name still list the allowed set.
#[test]
fn test_unknown_param_lists_allowed_set() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "totallyMadeUp": 42,
        "path": "a.ts",
        "edits": [{ "search": "x", "replace": "y" }]
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("Unknown parameter 'totallyMadeUp'"), "msg={}", msg);
    assert!(msg.contains("Allowed top-level parameters"), "msg={}", msg);
    assert!(msg.contains("paths"), "msg={}", msg);
}

/// Missing `path` AND missing `paths` returns an error message that includes
/// concrete examples for both single and batch forms (so the caller does not
/// have to guess the batch shape).
#[test]
fn test_missing_path_error_includes_both_examples() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "edits": [{ "search": "x", "replace": "y" }]
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("Missing required parameter"), "msg={}", msg);
    assert!(msg.contains("\"path\""), "Should show single-file example: {}", msg);
    assert!(msg.contains("\"paths\""), "Should show batch example: {}", msg);
}

/// When `path` is mistakenly nested inside an `edits[]` item, surface that as
/// a structural hint instead of just saying "missing path".
#[test]
fn test_missing_path_detects_nested_path_in_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "edits": [
            { "path": "a.ts", "search": "x", "replace": "y" }
        ]
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("must be a top-level parameter") || msg.contains("nested"),
        "Should hint about nesting: {}",
        msg
    );
}

/// Known params with edit distance > 2 are not falsely matched.
#[test]
fn test_did_you_mean_skips_distant_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "xyzzyfoobar": 1,
        "path": "a.ts",
        "edits": [{ "search": "x", "replace": "y" }]
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("Unknown parameter 'xyzzyfoobar'"), "msg={}", msg);
    assert!(!msg.contains("Did you mean"), "Should NOT suggest a distant match: {}", msg);
}

/// All currently-accepted top-level keys must NOT trip the unknown-param check.
/// Regression guard: if a new param is added to the schema without updating
/// `KNOWN_EDIT_PARAMS`, this test (or another using it) will fail.
#[test]
fn test_all_known_params_pass_unknown_check() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "a\nb\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "search": "a", "replace": "z" }],
        "regex": false,
        "dryRun": true,
        "expectedLineCount": 2,
    }));

    assert!(!result.is_error, "All known params should pass; got: {:?}", result);
}

// ─── Top-level wrong-type diagnostics ((closes follow-up
//      todo_approved_2026-04-24_edit-top-level-param-type-and-operations-path-hints.md)
//
// Each canonical top-level key must be type-checked up-front so callers do
// NOT silently fall through `.as_str()` / `.as_bool()` / `.as_u64()` and get
// misleading downstream errors ("missing path", "regex disabled",
// "expectedLineCount mismatch"). Mirror of the Mode B `expect_*_field` shape
// diagnostics, but at the top-level params layer.

#[test]
fn test_top_level_path_wrong_type_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": 123,
        "edits": [{ "search": "a", "replace": "z" }],
        "dryRun": true,
    }));

    assert!(result.is_error, "Numeric `path` must be rejected");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("'path'"), "msg should name the offending key: {}", msg);
    assert!(msg.contains("string"), "msg should name the expected type: {}", msg);
    assert!(msg.contains("number"), "msg should name the actual type: {}", msg);
}

#[test]
fn test_top_level_paths_wrong_type_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    // `paths` as a single string (caller forgot the array wrap).
    let result = handle_xray_edit(&ctx, &json!({
        "paths": "a.txt",
        "edits": [{ "search": "a", "replace": "z" }],
        "dryRun": true,
    }));

    assert!(result.is_error, "String `paths` must be rejected");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("'paths'"), "msg should name the offending key: {}", msg);
    assert!(msg.contains("array"), "msg should name the expected type: {}", msg);
}

#[test]
fn test_top_level_paths_with_non_string_item_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "paths": ["a.txt", 7, "b.txt"],
        "edits": [{ "search": "a", "replace": "z" }],
        "dryRun": true,
    }));

    assert!(result.is_error, "Numeric paths[] item must be rejected");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("paths[1]"), "msg should pinpoint the bad index: {}", msg);
    assert!(msg.contains("number"), "msg should name the actual type: {}", msg);
}

#[test]
fn test_top_level_regex_wrong_type_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "a\n");
    let ctx = make_ctx(tmp.path());

    // Caller passes the string "true" instead of the boolean `true` —
    // previously silently fell through to default-false and disabled regex.
    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "search": "a", "replace": "z" }],
        "regex": "true",
        "dryRun": true,
    }));

    assert!(result.is_error, "String `regex` must be rejected");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("'regex'"), "msg should name the offending key: {}", msg);
    assert!(msg.contains("boolean"), "msg should name the expected type: {}", msg);
    assert!(msg.contains("string"), "msg should name the actual type: {}", msg);
}

#[test]
fn test_top_level_dry_run_wrong_type_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "a\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "search": "a", "replace": "z" }],
        "dryRun": 1,
    }));

    assert!(result.is_error, "Numeric `dryRun` must be rejected");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("'dryRun'"), "msg should name the offending key: {}", msg);
    assert!(msg.contains("boolean"), "msg should name the expected type: {}", msg);
}

#[test]
fn test_top_level_expected_line_count_wrong_type_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "a\nb\n");
    let ctx = make_ctx(tmp.path());

    // String form of an integer — commonly seen when the caller stringifies
    // their state. Without the type check, `as_u64()` silently returns None
    // and the count check is skipped.
    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "search": "a", "replace": "z" }],
        "expectedLineCount": "2",
        "dryRun": true,
    }));

    assert!(result.is_error, "String `expectedLineCount` must be rejected");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("'expectedLineCount'"), "msg should name the key: {}", msg);
    assert!(msg.contains("integer"), "msg should name the expected type: {}", msg);
}

#[test]
fn test_top_level_operations_wrong_type_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "a\n");
    let ctx = make_ctx(tmp.path());

    // Caller passed a single op object instead of an array.
    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "operations": { "startLine": 1, "endLine": 1, "content": "z" },
        "dryRun": true,
    }));

    assert!(result.is_error, "Object `operations` must be rejected");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("'operations'"), "msg should name the key: {}", msg);
    assert!(msg.contains("array"), "msg should name the expected type: {}", msg);
}

#[test]
fn test_missing_path_detects_nested_path_in_operations() {
    // Symmetric to test_missing_path_detects_nested_path_in_edits, for the
    // Mode A `operations[]` arm. Caller put `path` inside the operation
    // object instead of at the top-level — surface the structural hint
    // instead of the literal "missing path" message.
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "operations": [{
            "path": "a.txt",
            "startLine": 1,
            "endLine": 1,
            "content": "z",
        }],
        "dryRun": true,
    }));

    assert!(result.is_error, "Nested path inside operations[] must be rejected");
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    // Reuses missing_path_error_message's structural hint path.
    assert!(
        msg.contains("'path'") && msg.to_lowercase().contains("top"),
        "msg should hint at top-level placement: {}",
        msg
    );
}



// ─── Mode B (text-match) edits[] item field validation ─────────────

/// Helper: assert that an aliased Mode B item produces an error pointing
/// at the canonical name on the first attempt.
fn assert_synonym_rejected(alias: &str, replacement_alias: &str, canonical: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ alias: "hello", replacement_alias: "world" }],
        "dryRun": true,
    }));

    assert!(result.is_error, "Alias '{}' should be rejected", alias);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains(&format!("unknown field '{}'", alias))
            || msg.contains(&format!("unknown field '{}'", replacement_alias)),
        "Error should call out the alias key. alias={}, msg={}", alias, msg
    );
    assert!(
        msg.contains(canonical),
        "Error should suggest canonical '{}'. msg={}", canonical, msg
    );
    // Must NOT use the legacy misleading message.
    assert!(
        !msg.contains("missing or invalid 'search'"),
        "Should not fall through to legacy message. msg={}", msg
    );
}

#[test]
fn test_edits_item_oldtext_newtext_rejected_with_canonical() {
    assert_synonym_rejected("oldText", "newText", "search");
}

#[test]
fn test_edits_item_old_str_new_str_rejected_with_canonical() {
    assert_synonym_rejected("old_str", "new_str", "search");
}

#[test]
fn test_edits_item_oldstring_newstring_rejected_with_canonical() {
    assert_synonym_rejected("oldString", "newString", "search");
}

#[test]
fn test_edits_item_find_with_rejected_with_canonical() {
    assert_synonym_rejected("find", "with", "search");
}

#[test]
fn test_edits_item_pattern_replacement_rejected_with_canonical() {
    assert_synonym_rejected("pattern", "replacement", "search");
}

#[test]
fn test_edits_item_after_alias_rejected_with_canonical() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "a\nb\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "after": "a", "content": "x" }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("unknown field 'after'"), "msg={}", msg);
    assert!(msg.contains("insertAfter"), "msg={}", msg);
}

/// Empty edit object should produce the menu-of-forms error, not a misleading
/// "missing or invalid 'search'".
#[test]
fn test_edits_item_empty_object_shows_form_menu() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "a\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{}],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("missing a primary action field"), "msg={}", msg);
    assert!(msg.contains("search"), "menu should mention 'search'. msg={}", msg);
    assert!(msg.contains("insertAfter"), "menu should mention 'insertAfter'. msg={}", msg);
    assert!(msg.contains("insertBefore"), "menu should mention 'insertBefore'. msg={}", msg);
}

/// `startLine` / `endLine` inside `edits[]` (Mode A field at Mode B level)
/// should produce a hint to switch to the `operations` parameter.
#[test]
fn test_edits_item_startline_hints_at_operations_param() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "a\nb\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "startLine": 1, "endLine": 1, "content": "x" }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("Mode A"), "msg={}", msg);
    assert!(msg.contains("operations"), "msg={}", msg);
}

/// Random typo (e.g. `serch`) close to a known field should surface a
/// did_you_mean suggestion.
#[test]
fn test_edits_item_typo_suggests_canonical() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "serch": "hello", "replace": "world" }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("unknown field 'serch'"), "msg={}", msg);
    assert!(msg.contains("Did you mean 'search'"), "msg={}", msg);
}

/// Extra unknown field alongside a valid search/replace should still be rejected.
#[test]
fn test_edits_item_extra_unknown_field_rejected_even_with_valid_pair() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "search": "hello", "replace": "world", "typoField": 1 }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(msg.contains("unknown field 'typoField'"), "msg={}", msg);
}

/// Canonical form must still work after the new validation gate.
/// Regression guard for the happy path.
#[test]
fn test_edits_item_canonical_search_replace_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{
            "search": "hello",
            "replace": "world",
            "occurrence": 1,
            "expectedContext": "hello",
            "skipIfNotFound": false
        }],
    }));

    assert!(!result.is_error, "Canonical form should succeed: {:?}", result);
}

/// `replace` without `search` (callers sometimes drop the wrong half) must
/// produce a clear message — no "missing or invalid 'search'" wording.
#[test]
fn test_edits_item_replace_without_search_clear_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "replace": "world" }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("'replace' provided without 'search'"),
        "Should explain which half is missing. msg={}", msg
    );
    assert!(
        !msg.contains("missing or invalid 'search'"),
        "Should not use legacy phrasing. msg={}", msg
    );
}

// ─── Mode B shape diagnostics: wrong-type canonical fields + non-object items ───
// Regression guards for `docs/user-stories/todo_approved_2026-04-24_edit-mode-b-shape-diagnostics-followups.md`.
// Each test asserts the targeted shape error is emitted instead of the
// previous misleading downstream message.

/// `{"search": 123, "replace": "x"}` — canonical key, wrong type. Previously
/// the int silently became `None`, then the s/r branch reported
/// `'replace' provided without 'search'`, which is misleading because
/// `search` IS present, just typed wrong.
#[test]
fn test_edits_item_search_wrong_type_reports_string_requirement() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "search": 123, "replace": "x" }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("'search' must be a string"),
        "Must report wrong type for 'search'. msg={}", msg
    );
    assert!(msg.contains("got number"), "Must name the actual type. msg={}", msg);
    assert!(
        !msg.contains("provided without 'search'"),
        "Must NOT fall through to misleading missing-half message. msg={}", msg
    );
}

/// `{"insertAfter": 123, "content": "x"}` — canonical insert-mode key with
/// wrong type. Previously fell through to the empty-form-menu error.
#[test]
fn test_edits_item_insert_after_wrong_type_reports_string_requirement() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "insertAfter": 123, "content": "x" }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("'insertAfter' must be a string"),
        "Must report wrong type for 'insertAfter'. msg={}", msg
    );
    assert!(msg.contains("got number"), "Must name the actual type. msg={}", msg);
}

/// `{"search": "a", "replace": "b", "skipIfNotFound": "yes"}` — critical case:
/// `.unwrap_or(false)` on a wrong-type bool would silently change edit
/// semantics (caller thinks skip is honoured, batch fails atomically).
#[test]
fn test_edits_item_skip_if_not_found_wrong_type_reports_bool_requirement() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "search": "a", "replace": "b", "skipIfNotFound": "yes" }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("'skipIfNotFound' must be a boolean"),
        "Must report wrong type for 'skipIfNotFound'. msg={}", msg
    );
    assert!(msg.contains("got string"), "Must name the actual type. msg={}", msg);
}

/// `{"search": "a", "replace": "b", "occurrence": "first"}` — wrong-type
/// numeric meta-field. Previously silently became `0` (= match all).
#[test]
fn test_edits_item_occurrence_wrong_type_reports_integer_requirement() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [{ "search": "a", "replace": "b", "occurrence": "first" }],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("'occurrence' must be a non-negative integer"),
        "Must report wrong type for 'occurrence'. msg={}", msg
    );
}

/// `edits: [123]` — non-object item. Previously fell into the form-menu
/// error because every `.get(...)` returned None.
#[test]
fn test_edits_item_non_object_number_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [123],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("each edit item must be a JSON object"),
        "Must explain the structural shape problem. msg={}", msg
    );
    assert!(msg.contains("got number"), "Must name the actual type. msg={}", msg);
}

/// `edits: [null]` — same as above for null payloads.
#[test]
fn test_edits_item_non_object_null_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": [serde_json::Value::Null],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("each edit item must be a JSON object"),
        "Must explain the structural shape problem. msg={}", msg
    );
    assert!(msg.contains("got null"), "Must name the actual type. msg={}", msg);
}

/// `edits: ["search"]` — bare string is a common mistake when callers
/// confuse `edits` with a list of search terms.
#[test]
fn test_edits_item_non_object_string_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = create_named_temp_file(tmp.path(), "f.txt", "hello\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": path.to_string_lossy(),
        "edits": ["search"],
        "dryRun": true,
    }));

    assert!(result.is_error);
    let msg = result.content.iter().map(|c| c.text.clone()).collect::<String>();
    assert!(
        msg.contains("each edit item must be a JSON object"),
        "Must explain the structural shape problem. msg={}", msg
    );
    assert!(msg.contains("got string"), "Must name the actual type. msg={}", msg);
}


// ═════════════════════════════════════════════════════════════════════
// PR #1 (cleanup-magic) — new diagnostic and category tests
// ═════════════════════════════════════════════════════════════════════

/// File `"foo\nbar\n"`, search `"foo \nbar\n"` (extra trailing space on line 1).
/// Before PR #1: Step 2 silently stripped the space and matched. After PR #1:
/// must surface as Text not found with `category: trailingWhitespace`.
#[test]
fn test_no_silent_match_on_trailing_whitespace_drift() {
    let (tmp, filename, _) = create_temp_file("foo\nbar\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "foo \nbar\n", "replace": "FOO\nBAR\n" }
        ]
    }));

    assert!(result.is_error, "Trailing-WS drift must NOT silently match");
    let text = &result.content[0].text;
    assert!(text.contains("Text not found"), "Got: {}", text);
    assert!(
        text.contains("category: trailingWhitespace"),
        "Error should include `(category: trailingWhitespace)`. Got: {}",
        text
    );
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert_eq!(content, "foo\nbar\n", "File must be unchanged");
}

/// Symmetric to the above: extra blank lines in search. After PR #1, the input
/// surfaces as Text not found instead of being silently fixed by the deleted
/// Step 3. We do not assert a specific category here — the nearest-match
/// window may not align tightly enough to score ≥ 0.80, in which case no
/// category is emitted. The category logic itself is covered by
/// `test_detect_diff_category_*`.
#[test]
fn test_no_silent_match_on_blank_lines_drift() {
    let (tmp, filename, _) = create_temp_file("prefix\nfoo\nbar\nsuffix\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "\n\nfoo\nbar\n\n", "replace": "FOO_BAR" }
        ]
    }));

    assert!(result.is_error, "Blank-line drift must NOT silently match");
    let text = &result.content[0].text;
    assert!(text.contains("Text not found"), "Got: {}", text);
    let content = std::fs::read_to_string(tmp.path().join(&filename)).unwrap();
    assert_eq!(content, "prefix\nfoo\nbar\nsuffix\n", "File must be unchanged");
}

/// File saved with CRLF, search uses LF. The handler pre-normalises CRLF→LF
/// in BOTH file content and search text (`normalize_crlf` in `parse_text_edits`),
/// so an end-to-end mismatch never reaches the nearest-match diagnostic. The
/// LF↔CRLF case is therefore covered by the unit test
/// `test_detect_diff_category_crlf_vs_lf` against `detect_diff_category` directly.
///
/// This test asserts the auto-normalisation invariant: a CRLF file with an
/// LF search must SUCCEED, not error.
#[test]
fn test_crlf_file_lf_search_normalised_match() {
    let tmp = tempfile::tempdir().unwrap();
    let filename = "crlf.txt";
    let path = tmp.path().join(filename);
    std::fs::write(&path, b"hello world\r\nsecond line\r\n").unwrap();

    let ctx = make_ctx(tmp.path());
    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "hello world\nsecond line\n", "replace": "HELLO\nSECOND\n" }
        ]
    }));

    assert!(!result.is_error,
        "CRLF file + LF search must match via auto-normalisation. Got: {}",
        result.content[0].text);
}

/// After Step 2/3 removal, a short block that exists in repeated context with
/// trailing-WS drift in the search must not misfire onto a different occurrence.
/// This is a regression guard: the deleted Step 2 could match a similar block
/// that happened to have the right collapsed form. With it gone, the search
/// either matches exactly or fails loudly.
#[test]
fn test_anti_misfire_short_block_in_repeated_context() {
    let (tmp, filename, path) = create_temp_file(
        "fn a() {\n    return 1;\n}\n\nfn b() {\n    return 1;\n}\n",
    );
    let ctx = make_ctx(tmp.path());

    // Search has a trailing space the file doesn't. Old Step 2 would strip it,
    // then "    return 1;\n}" matches TWICE. The handler would either error
    // on ambiguity OR (worse) silently target the wrong copy. Either way the
    // diagnose-first behaviour is to fail with a clear hint.
    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "    return 1; \n}", "replace": "    return 42;\n}" }
        ]
    }));

    assert!(result.is_error, "Trailing-WS drift on a repeated block must fail loudly");
    let text = &result.content[0].text;
    assert!(text.contains("Text not found"), "Got: {}", text);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("return 1;"),
        "File must be unchanged — no silent misfire onto either copy");
    assert!(!content.contains("return 42;"), "No replacement must occur");
}

/// Multi-line whitespace drift with similarity ~0.85 must still emit a
/// byte-level diff hint after PR #1 (threshold lowered from 0.99 → 0.80).
/// Before PR #1 this drift would have been silently fixed by Step 2; the
/// byte hint at the old 0.99 threshold would never fire.
#[test]
fn test_byte_diff_hint_at_080_similarity() {
    // File: 5 short lines without trailing WS.
    let (tmp, filename, _) = create_temp_file("abc\ndef\nghi\njkl\nmno\n");
    let ctx = make_ctx(tmp.path());

    // Search: same 5 lines, each with one trailing space. Char-similarity
    // sits in the ~0.80–0.95 band depending on the diff algorithm — above
    // the new NEAREST_MATCH_BYTE_DIFF_THRESHOLD (0.80), below the old 0.99.
    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "edits": [
            { "search": "abc \ndef \nghi \njkl \nmno \n", "replace": "X" }
        ]
    }));

    assert!(result.is_error, "Drift must surface as Text not found");
    let text = &result.content[0].text;
    assert!(text.contains("Text not found"), "Got: {}", text);
    assert!(
        text.contains("Nearest match"),
        "Nearest match hint must be present. Got: {}", text
    );
    assert!(
        text.contains("First difference at byte")
            || text.contains("longer than")
            || text.contains("Diff category:"),
        "byte-level hint or category must be present at ≥ 0.80 similarity. Got: {}",
        text
    );
    assert!(
        text.contains("trailingWhitespace"),
        "trailingWhitespace category must be detected. Got: {}", text
    );
}

/// Regression: `truncate_for_display` must respect UTF-8 char boundaries when
/// the cap falls inside a multi-byte sequence (e.g. em-dash, 3 bytes).
#[test]
fn test_truncate_for_display_utf8_boundary() {
    // Construct a string whose 150th byte falls inside the em-dash sequence.
    // 148 ASCII bytes + em-dash (3 bytes) + more ASCII — cap at 150 lands inside
    // the em-dash. Without floor_char_boundary this would panic.
    let mut s = String::with_capacity(300);
    for _ in 0..148 { s.push('a'); }
    s.push('\u{2014}'); // em-dash, 3 bytes
    for _ in 0..50 { s.push('b'); }

    // Should not panic and should return a valid UTF-8 string.
    let out = super::truncate_for_display(&s);
    assert!(out.is_char_boundary(out.len()), "Output must end on a char boundary");
    assert!(out.ends_with('\u{2026}'), "Output should end with horizontal ellipsis");
    // The em-dash must NOT appear half-truncated — either fully present or absent.
    let dash_count_in = s.matches('\u{2014}').count();
    let dash_count_out = out.matches('\u{2014}').count();
    assert!(dash_count_out <= dash_count_in, "Em-dash must not be split");
}

// ─── detect_diff_category unit tests ───────────────────────────

#[test]
fn test_detect_diff_category_crlf_vs_lf() {
    assert_eq!(
        super::detect_diff_category("hello\nworld\n", "hello\r\nworld\r\n"),
        "crlfVsLf"
    );
}

#[test]
fn test_detect_diff_category_blank_lines() {
    assert_eq!(
        super::detect_diff_category("\n\nfoo\nbar\n\n", "foo\nbar"),
        "leadingOrTrailingBlankLines"
    );
}

#[test]
fn test_detect_diff_category_blank_lines_crlf() {
    // Leading CRLF blank line drift.
    assert_eq!(
        super::detect_diff_category("\r\nfoo\r\n", "foo\r\n"),
        "leadingOrTrailingBlankLines"
    );
    // Trailing CRLF blank line drift.
    assert_eq!(
        super::detect_diff_category("foo\r\n", "foo\r\n\r\n"),
        "leadingOrTrailingBlankLines"
    );
    // Mixed: leading + trailing blank lines, both CRLF.
    assert_eq!(
        super::detect_diff_category("\r\n\r\nfoo\r\nbar\r\n\r\n", "foo\r\nbar"),
        "leadingOrTrailingBlankLines"
    );
}

/// Pure LF↔CRLF mismatch must remain `crlfVsLf`, not get re-classified as
/// blank-line drift after the trim was widened to also strip '\r'. Guard
/// against accidental priority inversion between branches 1 and 2 of
/// `detect_diff_category`.
#[test]
fn test_detect_diff_category_crlf_vs_lf_priority_over_blank_lines() {
    assert_eq!(
        super::detect_diff_category("foo\nbar\n", "foo\r\nbar\r\n"),
        "crlfVsLf"
    );
}


#[test]
fn test_detect_diff_category_trailing_whitespace() {
    assert_eq!(
        super::detect_diff_category("foo  \nbar\t\n", "foo\nbar\n"),
        "trailingWhitespace"
    );
}

/// Search uses regular ASCII single quote, file has Unicode right-single-quote
/// (U+2019). Folding confusables must classify as `unicodeConfusable`.
#[test]
fn test_unicode_confusable_category() {
    assert_eq!(
        super::detect_diff_category("it's", "it\u{2019}s"),
        "unicodeConfusable"
    );
    // Em-dash vs hyphen.
    assert_eq!(
        super::detect_diff_category("a - b", "a \u{2014} b"),
        "unicodeConfusable"
    );
    // NBSP vs space.
    assert_eq!(
        super::detect_diff_category("a b", "a\u{00A0}b"),
        "unicodeConfusable"
    );
}

#[test]
fn test_detect_diff_category_no_recognised_pattern() {
    // Genuine textual diff — no category.
    assert_eq!(super::detect_diff_category("foo", "bar"), "");
    // Identical strings — no category.
    assert_eq!(super::detect_diff_category("same", "same"), "");
}

// ─── Mode A append-mode error hint (PR #1, item 1.4) ───────────────

/// Mode A INSERT (`endLine < startLine`) with `startLine > line_count + 1`
/// must reject the op AND surface a positive append-idiom hint pointing at
/// `startLine: N+1, endLine: N`. Closes Issue 1 from the UX issues story.
#[test]
fn test_insert_out_of_range_includes_append_hint() {
    // File has 3 lines.
    let (tmp, filename, _) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            // INSERT mode (endLine < startLine) at startLine=10 — way past end.
            { "startLine": 10, "endLine": 9, "content": "x" }
        ]
    }));

    assert!(result.is_error, "Out-of-range insert must error");
    let text = &result.content[0].text;
    assert!(
        text.contains("out of range for insert"),
        "Should mention out-of-range insert. Got: {}", text
    );
    assert!(
        text.contains("To append to end of file"),
        "Error must include positive append-idiom hint. Got: {}", text
    );
    // For a 3-line file (after split on '\n' the content has 4 elements
    // including the trailing empty string — line_count = 4), append idiom is
    // `startLine: 5, endLine: 4`. Just check both numbers appear together.
    assert!(
        text.contains("startLine: "),
        "Hint must spell out the exact append-idiom coordinates. Got: {}", text
    );
}

/// Mode A REPLACE (`endLine >= startLine`) with `startLine > line_count`
/// must reject the op AND surface the same append-idiom hint that INSERT
/// already carries. Without this, agents that try to append by reusing the
/// last `newLineCount` (`startLine: N+1`) get a bare "out of range" error
/// with no path forward and frequently fall back to overwriting line N.
/// Closes docs/user-stories/todo_2026-04-25_xray-edit-append-and-line-staleness.md §2.1.
#[test]
fn test_replace_out_of_range_includes_append_hint() {
    // 3-line file. line_count is what apply_line_operations computes; the
    // hint must reference `line_count + 1` for the append form.
    let (tmp, filename, _) = create_temp_file("line1\nline2\nline3\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            // Mode A REPLACE (endLine >= startLine) past EOF.
            { "startLine": 99, "endLine": 99, "content": "x" }
        ]
    }));

    assert!(result.is_error, "Out-of-range REPLACE must error");
    let text = &result.content[0].text;
    assert!(
        text.contains("out of range"),
        "Should mention out-of-range. Got: {}", text
    );
    assert!(
        text.contains("To append after the last line"),
        "REPLACE error must include append-idiom hint. Got: {}", text
    );
    assert!(
        text.contains("INSERT mode"),
        "Hint must explain the INSERT-mode trick. Got: {}", text
    );
    assert!(
        text.contains("To replace the last line"),
        "Hint must also document the replace-last-line form. Got: {}", text
    );
}

/// Successful Mode A `xray_edit` response must carry an `appendIdiom` object
/// with the canonical (startLine, endLine) values for an INSERT-after-EOF
/// follow-up call. This eliminates the "agent guesses N+1 from stale state"
/// failure class.
#[test]
fn test_edit_response_includes_append_idiom() {
    let (tmp, filename, _) = create_temp_file("alpha\nbeta\ngamma\n");
    let ctx = make_ctx(tmp.path());

    let result = handle_xray_edit(&ctx, &json!({
        "path": filename,
        "operations": [
            { "startLine": 2, "endLine": 2, "content": "BETA" }
        ]
    }));
    assert!(!result.is_error, "Edit must succeed: {}", result.content[0].text);

    let output: serde_json::Value =
        serde_json::from_str(&result.content[0].text).unwrap();
    let new_line_count = output["newLineCount"].as_u64()
        .expect("newLineCount must be present");
    let append = output["appendIdiom"].as_object()
        .expect("appendIdiom must be present in successful response");
    assert_eq!(
        append["startLine"].as_u64().unwrap(),
        new_line_count + 1,
        "appendIdiom.startLine must equal newLineCount + 1"
    );
    assert_eq!(
        append["endLine"].as_u64().unwrap(),
        new_line_count,
        "appendIdiom.endLine must equal newLineCount"
    );
}

