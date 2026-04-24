//! Tests for xray_grep `lineRegex=true` mode (line-based regex matching).
//!
//! Unlike token-based regex (the default `regex=true`), `lineRegex=true` applies
//! the user-supplied pattern to each line of each candidate file. This enables:
//! - Line anchors (`^`, `$`)
//! - Whitespace inside patterns
//! - Non-token characters (punctuation, brackets, etc.)
//!
//! Candidate files are pre-filtered by `dir`/`ext`/`file`/`excludeDir`/`exclude`,
//! so search scope can be narrowed down without false negatives.

use super::*;
use super::grep::handle_xray_grep;
use super::handlers_test_utils::cleanup_tmp;
use std::io::Write;
use std::sync::{Arc, RwLock};

/// Build an isolated workspace with markdown + rust files for line-regex testing.
fn make_line_regex_ctx() -> (HandlerContext, std::path::PathBuf) {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_line_regex_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    std::fs::create_dir_all(tmp_dir.join("docs")).unwrap();
    std::fs::create_dir_all(tmp_dir.join("src")).unwrap();

    // Markdown file with multiple heading levels
    {
        let mut f = std::fs::File::create(tmp_dir.join("docs").join("guide.md")).unwrap();
        writeln!(f, "# Top-Level Title").unwrap();
        writeln!(f, "Some intro paragraph here.").unwrap();
        writeln!(f, "## First Section").unwrap();
        writeln!(f, "Body text mentioning ## inline (not a heading).").unwrap();
        writeln!(f, "## Second Section").unwrap();
        writeln!(f, "More content.").unwrap();
        writeln!(f, "### Subsection").unwrap();
        writeln!(f, "End of file.").unwrap();
    }

    // Another markdown file with only one heading
    {
        let mut f = std::fs::File::create(tmp_dir.join("docs").join("notes.md")).unwrap();
        writeln!(f, "# Notes").unwrap();
        writeln!(f, "## Important").unwrap();
        writeln!(f, "remember to test edge cases").unwrap();
    }

    // Rust source file with closing braces (used for `\}$` test)
    {
        let mut f = std::fs::File::create(tmp_dir.join("src").join("lib.rs")).unwrap();
        writeln!(f, "pub fn alpha() {{").unwrap();
        writeln!(f, "    let x = 1;").unwrap();
        writeln!(f, "}}").unwrap();
        writeln!(f, "pub fn beta() {{ return; }}").unwrap();
        writeln!(f, "// trailing comment").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "md,rs".to_string(),
        threads: 1,
        ..Default::default()
    }).unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "md,rs".to_string(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };
    (ctx, tmp_dir)
}

// ─── Anchor tests ──────────────────────────────────────────────────

#[test]
fn line_regex_caret_anchor_finds_markdown_headings() {
    let (ctx, tmp) = make_line_regex_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^## ",
        "regex": true,
        "lineRegex": true,
        "ext": "md",
        "showLines": true
    }));
    assert!(!result.is_error, "lineRegex with `^## ` should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert_eq!(mode, "lineRegex", "Expected searchMode=lineRegex, got: {}", mode);
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(total, 2, "Should find 2 markdown files containing `## ` headings, got {}", total);

    // Verify the returned line numbers point only at heading lines (not the inline `## inline` mention)
    let files = output["files"].as_array().unwrap();
    let guide = files.iter().find(|f| f["path"].as_str().unwrap_or("").contains("guide.md")).unwrap();
    let line_content = guide["lineContent"].as_array().unwrap();
    // Should match exactly 2 lines in guide.md ("## First Section" and "## Second Section")
    // Inline mention "## inline" is in the middle of a line, not at column 0 — must NOT match.
    assert_eq!(line_content.len(), 2,
        "Expected exactly 2 heading lines in guide.md (no false-positive on inline `##`), got: {:?}",
        line_content);
    cleanup_tmp(&tmp);
}

#[test]
fn line_regex_dollar_anchor_finds_lines_ending_with_brace() {
    let (ctx, tmp) = make_line_regex_ctx();
    // Match lines ending with `}` (with no trailing space).
    let result = handle_xray_grep(&ctx, &json!({
        "terms": r"\}$",
        "regex": true,
        "lineRegex": true,
        "ext": "rs"
    }));
    assert!(!result.is_error, "lineRegex with `\\}}$` should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(total, 1, "Should find 1 .rs file with closing braces, got {}", total);
    cleanup_tmp(&tmp);
}

// ─── Multi-pattern tests ──────────────────────────────────────────

#[test]
fn line_regex_multi_pattern_or() {
    let (ctx, tmp) = make_line_regex_ctx();
    // OR mode (default): files matching EITHER `^# ` (top-level heading) OR `^### ` (subsection)
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^# ,^### ",
        "regex": true,
        "lineRegex": true,
        "ext": "md"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert_eq!(mode, "lineRegex", "OR mode should report searchMode=lineRegex");
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    // Both .md files have `^# ` (top-level), only guide.md has `^### `. OR → 2 files.
    assert_eq!(total, 2, "OR mode should find 2 files, got {}", total);
    cleanup_tmp(&tmp);
}

#[test]
fn line_regex_multi_pattern_and() {
    let (ctx, tmp) = make_line_regex_ctx();
    // AND mode: files matching BOTH `^# ` AND `^### `. Only guide.md has both.
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^# ,^### ",
        "regex": true,
        "lineRegex": true,
        "mode": "and",
        "ext": "md"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let mode = output["summary"]["searchMode"].as_str().unwrap_or("");
    assert_eq!(mode, "lineRegex-and", "AND mode should report searchMode=lineRegex-and");
    let total = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(total, 1, "AND mode should find only guide.md, got {}", total);
    let files = output["files"].as_array().unwrap();
    assert!(files[0]["path"].as_str().unwrap().contains("guide.md"));
    cleanup_tmp(&tmp);
}

// ─── Filter scoping tests ─────────────────────────────────────────

#[test]
fn line_regex_ext_filter_scopes_search() {
    let (ctx, tmp) = make_line_regex_ctx();
    // `^# ` exists in markdown only — ext=rs should yield 0 results
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^# ",
        "regex": true,
        "lineRegex": true,
        "ext": "rs"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 0, "ext=rs should exclude markdown files");
    cleanup_tmp(&tmp);
}

#[test]
fn line_regex_dir_filter_scopes_search() {
    let (ctx, tmp) = make_line_regex_ctx();
    // dir=src restricts to src/lib.rs only — `^# ` (markdown heading) won't match
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^# ",
        "regex": true,
        "lineRegex": true,
        "dir": tmp.join("src").to_string_lossy().to_string()
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 0, "dir=src should exclude docs/*.md");
    cleanup_tmp(&tmp);
}

#[test]
fn line_regex_file_filter_scopes_search() {
    let (ctx, tmp) = make_line_regex_ctx();
    // file=guide.md restricts to one specific file
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^## ",
        "regex": true,
        "lineRegex": true,
        "file": "guide.md"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 1, "file=guide.md should restrict to one file");
    cleanup_tmp(&tmp);
}

// ─── Mode validation tests ────────────────────────────────────────

#[test]
fn line_regex_no_matches_returns_zero() {
    let (ctx, tmp) = make_line_regex_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^XYZ_NEVER_EXISTS_PATTERN_$",
        "regex": true,
        "lineRegex": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 0);
    assert_eq!(output["summary"]["totalOccurrences"], 0);
    cleanup_tmp(&tmp);
}

#[test]
fn line_regex_count_only_omits_files() {
    let (ctx, tmp) = make_line_regex_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^## ",
        "regex": true,
        "lineRegex": true,
        "ext": "md",
        "countOnly": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 2);
    assert!(output.get("files").is_none(), "countOnly should suppress files array");
    cleanup_tmp(&tmp);
}

#[test]
fn line_regex_show_lines_emits_line_content() {
    let (ctx, tmp) = make_line_regex_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^## ",
        "regex": true,
        "lineRegex": true,
        "file": "guide.md",
        "showLines": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();
    assert!(!files.is_empty());
    let line_content = files[0]["lineContent"].as_array().unwrap();
    assert!(!line_content.is_empty(), "showLines=true should emit lineContent");
    // Each entry should have line numbers and source text
    for entry in line_content {
        assert!(entry["lines"].is_array() || entry["startLine"].is_number(),
            "lineContent entry should describe matched lines: {:?}", entry);
    }
    cleanup_tmp(&tmp);
}

// ─── Mutex / auto-promotion tests ─────────────────────────────────

#[test]
fn line_regex_mutex_with_phrase() {
    let (ctx, tmp) = make_line_regex_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^## ",
        "regex": true,
        "lineRegex": true,
        "phrase": true
    }));
    assert!(result.is_error, "lineRegex + phrase=true must be rejected");
    let msg = &result.content[0].text;
    assert!(msg.contains("phrase") || msg.contains("lineRegex"),
        "Error message should mention the conflict, got: {}", msg);
    cleanup_tmp(&tmp);
}

#[test]
fn line_regex_auto_enables_regex() {
    // lineRegex=true without explicit regex=true should still work — regex is auto-promoted.
    let (ctx, tmp) = make_line_regex_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^## ",
        "lineRegex": true,
        "ext": "md"
    }));
    assert!(!result.is_error,
        "lineRegex=true should auto-enable regex (no explicit regex=true required), got: {}",
        result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalFiles"], 2,
        "Auto-promoted regex should still match `^## ` headings in both .md files");
    cleanup_tmp(&tmp);
}

#[test]
fn line_regex_invalid_pattern_returns_error() {
    let (ctx, tmp) = make_line_regex_ctx();
    let result = handle_xray_grep(&ctx, &json!({
        "terms": "[unclosed",
        "regex": true,
        "lineRegex": true
    }));
    assert!(result.is_error, "Invalid regex pattern should produce an error");
    cleanup_tmp(&tmp);
}

/// MINOR-23 regression: when `showLines=true` accumulates more than
/// `MAX_CONTENT_CACHE_BYTES` of file content, the cache must stop growing,
/// matched line numbers stay complete, and the response surfaces a
/// `lineContentTruncated` hint so the client knows previews are partial.
///
/// Cap is 4 KiB under `cfg(test)` (256 MiB in production builds), so this
/// test produces a few KB of fixtures rather than gigabytes.
#[test]
fn line_regex_show_lines_caps_content_cache() {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!(
        "xray_line_regex_cap_{}_{}",
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Three 2 KiB files, each containing the same matching line. Total = 6 KiB,
    // which exceeds the 4 KiB test-cap, so at least one file's preview must be
    // dropped. We pad with a short repeating ASCII line so file sizes are
    // deterministic and easy to reason about.
    let payload = "ZZ\n".repeat(682); // ~ 2046 bytes per file
    for i in 0..3 {
        let path = tmp_dir.join(format!("big_{}.md", i));
        let mut f = std::fs::File::create(&path).unwrap();
        // Matching line + padding
        writeln!(f, "## hit_{}", i).unwrap();
        f.write_all(payload.as_bytes()).unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "md".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            tmp_dir.to_string_lossy().to_string(),
        ))),
        server_ext: "md".to_string(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    let result = handle_xray_grep(&ctx, &json!({
        "terms": "^## hit_",
        "regex": true,
        "lineRegex": true,
        "showLines": true,
    }));
    assert!(
        !result.is_error,
        "lineRegex with cap-exceeded showLines should not error: {}",
        result.content[0].text
    );
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Contract 1: matched line numbers must be complete — all 3 files match.
    let total_files = output["summary"]["totalFiles"].as_u64().unwrap();
    assert_eq!(
        total_files, 3,
        "All matching files must be reported even when content cache is capped"
    );

    // Contract 2: the truncation hint must be surfaced.
    let truncated = output["summary"]["lineContentTruncated"]
        .as_bool()
        .unwrap_or(false);
    assert!(
        truncated,
        "summary.lineContentTruncated must be true when cache cap is exceeded; got summary: {}",
        output["summary"]
    );
    let reason = output["summary"]["lineContentTruncationReason"]
        .as_str()
        .unwrap_or("");
    assert!(
        reason.contains("cache"),
        "truncationReason should mention the cache budget; got: {:?}",
        reason
    );

    // Contract 3: at least one file must lack `lineContent` (cache was capped).
    let files = output["files"].as_array().unwrap();
    let without_line_content =
        files.iter().filter(|f| f.get("lineContent").is_none()).count();
    assert!(
        without_line_content >= 1,
        "At least one file must lack lineContent (cache exhausted); got 0 of {} files",
        files.len()
    );

    cleanup_tmp(&tmp_dir);
}

// ─── linePatterns: literal-comma-safe array form ──────────────────────────

/// Build an isolated workspace with a small log file used by `linePatterns`
/// tests where literal `,` inside a pattern matters (CSV regex, log prefixes).
fn make_line_patterns_log_ctx() -> (HandlerContext, std::path::PathBuf) {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir()
        .join(format!("xray_line_patterns_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    {
        let mut f = std::fs::File::create(tmp_dir.join("app.log")).unwrap();
        writeln!(f, "INFO: starting up").unwrap();
        writeln!(f, "ERROR,WARN: bad config").unwrap();
        writeln!(f, "DEBUG: reading file").unwrap();
        writeln!(f, "ERROR,WARN: missing key").unwrap();
        writeln!(f, "TRACE: done").unwrap();
    }
    {
        let mut f = std::fs::File::create(tmp_dir.join("data.csv")).unwrap();
        writeln!(f, "alpha,beta").unwrap();
        writeln!(f, "no comma here").unwrap();
        writeln!(f, "x,y").unwrap();
        writeln!(f, "trailing,").unwrap();
    }

    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: tmp_dir.to_string_lossy().to_string(),
        ext: "log,csv".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            tmp_dir.to_string_lossy().to_string(),
        ))),
        server_ext: "log,csv".to_string(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };
    (ctx, tmp_dir)
}

#[test]
fn line_patterns_literal_comma_in_log_prefix() {
    // `^ERROR,WARN:` as a single pattern — `,` is literal, not a separator.
    // The legacy `terms="^ERROR,WARN:"` path would split into two regexes
    // (`^ERROR` and `WARN:`), matching far more lines than intended.
    let (ctx, tmp) = make_line_patterns_log_ctx();
    let result = handle_xray_grep(
        &ctx,
        &json!({
            "linePatterns": ["^ERROR,WARN:"],
            "lineRegex": true,
            "ext": "log",
            "showLines": true,
        }),
    );
    assert!(
        !result.is_error,
        "linePatterns literal-comma should not error: {}",
        result.content[0].text
    );
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let occ = output["summary"]["totalOccurrences"].as_u64().unwrap_or(0);
    assert_eq!(
        occ, 2,
        "Expected 2 ERROR,WARN: lines, payload: {}",
        result.content[0].text
    );
    cleanup_tmp(&tmp);
}

#[test]
fn line_patterns_csv_two_columns_regex() {
    // `^[^,]+,[^,]+$` matches exactly-two-column CSV rows. The comma inside
    // the pattern is structural (the `,` between columns), and would be
    // mangled by `terms`-based comma-splitting.
    let (ctx, tmp) = make_line_patterns_log_ctx();
    let result = handle_xray_grep(
        &ctx,
        &json!({
            "linePatterns": ["^[^,]+,[^,]+$"],
            "lineRegex": true,
            "ext": "csv",
            "showLines": true,
        }),
    );
    assert!(
        !result.is_error,
        "linePatterns CSV regex should not error: {}",
        result.content[0].text
    );
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let occ = output["summary"]["totalOccurrences"].as_u64().unwrap_or(0);
    // alpha,beta + x,y match; "no comma here" and "trailing," do not.
    assert_eq!(
        occ, 2,
        "Expected 2 two-column rows, payload: {}",
        result.content[0].text
    );
    cleanup_tmp(&tmp);
}

#[test]
fn line_patterns_multiple_patterns_or_semantics() {
    // Multiple entries in `linePatterns` are independent regexes, OR-combined
    // by default (same as `terms` comma-OR semantics).
    let (ctx, tmp) = make_line_patterns_log_ctx();
    let result = handle_xray_grep(
        &ctx,
        &json!({
            "linePatterns": ["^INFO:", "^TRACE:"],
            "lineRegex": true,
            "ext": "log",
            "showLines": true,
        }),
    );
    assert!(
        !result.is_error,
        "linePatterns multi-OR should not error: {}",
        result.content[0].text
    );
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let occ = output["summary"]["totalOccurrences"].as_u64().unwrap_or(0);
    assert_eq!(
        occ, 2,
        "Expected 1 INFO + 1 TRACE = 2, payload: {}",
        result.content[0].text
    );
    cleanup_tmp(&tmp);
}

#[test]
fn line_patterns_and_terms_mutually_exclusive() {
    // Passing both `terms` and `linePatterns` is rejected with a hint that
    // names the precise difference (`,` separator vs literal).
    let (ctx, tmp) = make_line_patterns_log_ctx();
    let result = handle_xray_grep(
        &ctx,
        &json!({
            "terms": "^INFO:",
            "linePatterns": ["^TRACE:"],
            "lineRegex": true,
            "ext": "log",
        }),
    );
    assert!(
        result.is_error,
        "terms + linePatterns must be rejected, got success: {}",
        result.content[0].text
    );
    let body = &result.content[0].text;
    assert!(
        body.contains("mutually exclusive") && body.contains("linePatterns"),
        "Error must explain mutual exclusivity, got: {}",
        body
    );
    cleanup_tmp(&tmp);
}

#[test]
fn line_patterns_requires_line_regex_true() {
    // `linePatterns` is meaningful only in line-regex mode. Passing it without
    // `lineRegex=true` is rejected eagerly so a typo doesn't silently fall
    // through to substring/regex modes that ignore the array.
    let (ctx, tmp) = make_line_patterns_log_ctx();
    let result = handle_xray_grep(
        &ctx,
        &json!({
            "linePatterns": ["^INFO:"],
            "ext": "log",
        }),
    );
    assert!(
        result.is_error,
        "linePatterns without lineRegex must be rejected, got success: {}",
        result.content[0].text
    );
    let body = &result.content[0].text;
    assert!(
        body.contains("lineRegex=true"),
        "Error must point at lineRegex=true, got: {}",
        body
    );
    cleanup_tmp(&tmp);
}

#[test]
fn line_patterns_terms_comma_split_back_compat() {
    // Back-compat guard: when `linePatterns` is NOT supplied, the old
    // `terms.split(',')` behaviour for lineRegex is preserved.
    let (ctx, tmp) = make_line_patterns_log_ctx();
    let result = handle_xray_grep(
        &ctx,
        &json!({
            "terms": "^INFO:,^TRACE:",
            "lineRegex": true,
            "ext": "log",
        }),
    );
    assert!(
        !result.is_error,
        "Back-compat comma-split terms must still work: {}",
        result.content[0].text
    );
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let occ = output["summary"]["totalOccurrences"].as_u64().unwrap_or(0);
    assert_eq!(
        occ, 2,
        "Back-compat OR over `^INFO:,^TRACE:` must still match 2 lines, payload: {}",
        result.content[0].text
    );
    cleanup_tmp(&tmp);
}
