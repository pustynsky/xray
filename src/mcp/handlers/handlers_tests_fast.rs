//! Tests for xray_fast -- extracted from handlers_tests.rs.

use super::*;
use super::fast::handle_xray_fast;
use super::handlers_test_utils::cleanup_tmp;
use crate::ContentIndex;
use std::sync::{Arc, RwLock};
// --- xray_fast comma-separated tests ---

fn make_xray_fast_ctx() -> (HandlerContext, std::path::PathBuf) {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_test_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);
    for name in &["OrderProcessor.cs", "OrderValidator.cs", "InventoryTracker.cs", "ConfigurationHelper.cs", "UserService.cs", "OtherFile.txt"] {
        let p = tmp_dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "// {}", name).unwrap();
    }
    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str, index_base: idx_base, ..Default::default() };
    (ctx, tmp_dir)
}

#[test] fn test_xray_fast_single_pattern() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": "OrderProcessor"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 1);
    cleanup_tmp(&tmp);
}

#[test] fn test_xray_fast_comma_separated_multi_term() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": "OrderProcessor,OrderValidator,InventoryTracker,ConfigurationHelper"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 4);
    cleanup_tmp(&tmp);
}

#[test] fn test_xray_fast_comma_separated_with_ext_filter() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": "OrderProcessor,OtherFile", "ext": "cs"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 1);
    cleanup_tmp(&tmp);
}

#[test] fn test_xray_fast_comma_separated_no_matches() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": "NonExistentClass,AnotherMissing"}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 0);
    cleanup_tmp(&tmp);
}

#[test] fn test_xray_fast_comma_separated_partial_matches() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": "OrderProcessor,NonExistent,InventoryTracker"}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 2);
    cleanup_tmp(&tmp);
}

#[test] fn test_xray_fast_comma_separated_with_spaces() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": " OrderProcessor , InventoryTracker "}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 2);
    cleanup_tmp(&tmp);
}

#[test] fn test_xray_fast_comma_separated_count_only() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": "OrderProcessor,InventoryTracker", "countOnly": true}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 2);
    assert!(output["files"].as_array().unwrap().is_empty());
    cleanup_tmp(&tmp);
}

#[test] fn test_xray_fast_comma_separated_ignore_case() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": "orderprocessor,inventorytracker", "ignoreCase": true}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 2);
    cleanup_tmp(&tmp);
}
// ═══════════════════════════════════════════════════════════════════════
// Batch 2 tests — Strengthen Partial Coverage
// ═══════════════════════════════════════════════════════════════════════

/// T15 — xray_fast dirsOnly and filesOnly filters.
#[test]
fn test_xray_fast_dirs_only_and_files_only() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_dironly_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let sub = tmp_dir.join("Models");
    let _ = std::fs::create_dir_all(&sub);
    let file_in_sub = sub.join("ModelItem.cs");
    { let mut f = std::fs::File::create(&file_in_sub).unwrap(); writeln!(f, "// model").unwrap(); }
    let file_at_root = tmp_dir.join("ModelsHelper.cs");
    { let mut f = std::fs::File::create(&file_at_root).unwrap(); writeln!(f, "// helper").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str, index_base: idx_base, ..Default::default() };

    let result_dirs = handle_xray_fast(&ctx, &json!({"pattern": "Models", "dirsOnly": true}));
    assert!(!result_dirs.is_error, "dirsOnly should not error: {}", result_dirs.content[0].text);
    let output_dirs: Value = serde_json::from_str(&result_dirs.content[0].text).unwrap();
    let dir_files = output_dirs["files"].as_array().unwrap();
    for entry in dir_files {
        assert_eq!(entry["isDir"], true, "dirsOnly should only return directories, got: {}", entry);
    }
    assert!(output_dirs["summary"]["totalMatches"].as_u64().unwrap() >= 1,
        "Should find at least one directory matching 'Models'");

    let result_files = handle_xray_fast(&ctx, &json!({"pattern": "Models", "filesOnly": true}));
    assert!(!result_files.is_error);
    let output_files: Value = serde_json::from_str(&result_files.content[0].text).unwrap();
    let file_entries = output_files["files"].as_array().unwrap();
    for entry in file_entries {
        assert_eq!(entry["isDir"], false, "filesOnly should only return files, got: {}", entry);
    }
    assert!(output_files["summary"]["totalMatches"].as_u64().unwrap() >= 1,
        "Should find at least one file matching 'Models'");

    cleanup_tmp(&tmp_dir);
}

/// Regression test: xray_fast should reuse the server_dir's file-list index
/// when dir is a subdirectory, instead of creating a new orphan index file.
/// Bug: LLM calling xray_fast(dir="docs/design/rest-api") created
/// a separate file-list index for the subdirectory.
#[test]
fn test_xray_fast_subdir_reuses_parent_index() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_subdir_reuse_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create a directory structure: root/docs/design/rest-api/
    let subdir = tmp_dir.join("docs").join("design").join("rest-api");
    std::fs::create_dir_all(&subdir).unwrap();
    { let mut f = std::fs::File::create(subdir.join("api-spec.md")).unwrap(); writeln!(f, "# API Spec").unwrap(); }
    { let mut f = std::fs::File::create(tmp_dir.join("README.md")).unwrap(); writeln!(f, "# Root README").unwrap(); }

    // Build and save a file-list index for the ROOT directory (server_dir)
    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);

    let content_index = ContentIndex {
        root: dir_str.clone(), extensions: vec!["md".to_string()], ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: dir_str.clone(),
        index_base: idx_base.clone(),
        ..Default::default()
    };

    // Count index files BEFORE the subdir call
    let count_before: usize = std::fs::read_dir(&idx_base).unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension()
            .is_some_and(|ext| ext == "file-list"))
        .count();

    // Call xray_fast with dir pointing to a SUBDIRECTORY
    let subdir_str = subdir.to_string_lossy().to_string();
    let result = handle_xray_fast(&ctx, &json!({
        "pattern": "*",
        "dir": subdir_str
    }));
    assert!(!result.is_error, "xray_fast with subdir should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalMatches"].as_u64().unwrap() >= 1,
        "Should find at least 1 entry in the subdirectory");

    // CRITICAL: count index files AFTER — should NOT have created a new file-list index
    let count_after: usize = std::fs::read_dir(&idx_base).unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension()
            .is_some_and(|ext| ext == "file-list"))
        .count();
    assert_eq!(count_before, count_after,
        "No new file-list index should be created for subdirectory. Before: {}, After: {}",
        count_before, count_after);

    cleanup_tmp(&tmp_dir);
}

/// Verify that xray_fast still auto-builds an index when dir is genuinely
/// outside the server_dir (not a subdirectory).
#[test]
fn test_xray_fast_outside_dir_still_builds_index() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let server_dir = std::env::temp_dir().join(format!("xray_fast_outside_srv_{}_{}", std::process::id(), id));
    let other_dir = std::env::temp_dir().join(format!("xray_fast_outside_other_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&server_dir);
    let _ = std::fs::create_dir_all(&other_dir);

    { let mut f = std::fs::File::create(server_dir.join("server.txt")).unwrap(); writeln!(f, "server").unwrap(); }
    { let mut f = std::fs::File::create(other_dir.join("other.txt")).unwrap(); writeln!(f, "other").unwrap(); }

    let srv_str = server_dir.to_string_lossy().to_string();
    let other_str = other_dir.to_string_lossy().to_string();

    // Build index for server_dir only
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: srv_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = server_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);

    let content_index = ContentIndex {
        root: srv_str.clone(), extensions: vec!["txt".to_string()], ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: srv_str.clone(),
        index_base: idx_base.clone(),
        ..Default::default()
    };

    // Call xray_fast with dir pointing OUTSIDE server_dir
    let result = handle_xray_fast(&ctx, &json!({
        "pattern": "*",
        "dir": other_str
    }));
    assert!(!result.is_error, "xray_fast with outside dir should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalMatches"].as_u64().unwrap() >= 1,
        "Should find files in the outside directory");

    // Should have created a NEW file-list index (2 total: server_dir + other_dir)
    let file_list_count: usize = std::fs::read_dir(&idx_base).unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension()
            .is_some_and(|ext| ext == "file-list"))
        .count();
    assert_eq!(file_list_count, 2,
        "Should have 2 file-list indexes (server_dir + other_dir), got {}", file_list_count);

    cleanup_tmp(&server_dir);
    cleanup_tmp(&other_dir);
}

/// Regression test: maxDepth with subdirectory should compute depth relative to dir,
/// not relative to index.root. Without this fix, maxDepth=1 with dir=src would
/// show entries relative to root (wrong depth calculation).
#[test]
fn test_xray_fast_subdir_max_depth_relative_to_dir() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_subdir_maxdepth_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Structure:
    //   root/
    //     src/
    //       App.cs            ← depth 0 from src (should appear with maxDepth=1)
    //       models/
    //         User.cs          ← depth 1 from src (should appear with maxDepth=1)
    //         nested/
    //           Deep.cs        ← depth 2 from src (should NOT appear with maxDepth=1)
    //     tests/
    //       Test.cs            ← should NOT appear (outside src)
    let src = tmp_dir.join("src");
    let models = src.join("models");
    let nested = models.join("nested");
    let tests_dir = tmp_dir.join("tests");
    for d in [&src, &models, &nested, &tests_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    { let mut f = std::fs::File::create(src.join("App.cs")).unwrap(); writeln!(f, "// App").unwrap(); }
    { let mut f = std::fs::File::create(models.join("User.cs")).unwrap(); writeln!(f, "// User").unwrap(); }
    { let mut f = std::fs::File::create(nested.join("Deep.cs")).unwrap(); writeln!(f, "// Deep").unwrap(); }
    { let mut f = std::fs::File::create(tests_dir.join("Test.cs")).unwrap(); writeln!(f, "// Test").unwrap(); }

    // Build index for ROOT
    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);

    let content_index = ContentIndex {
        root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default()
    };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: dir_str.clone(),
        index_base: idx_base.clone(),
        ..Default::default()
    };

    // Call xray_fast with dir=src, maxDepth=1
    let src_str = src.to_string_lossy().to_string();
    let result = handle_xray_fast(&ctx, &json!({
        "pattern": "*",
        "dir": src_str,
        "maxDepth": 1
    }));
    assert!(!result.is_error, "should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();

    // App.cs (depth 1 from src = immediate child) — MUST be included
    assert!(paths.iter().any(|p| p.contains("App.cs")),
        "App.cs (immediate child of src) should be included with maxDepth=1. Got paths: {:?}", paths);

    // User.cs (depth 2 from src = grandchild) — MUST be excluded with maxDepth=1
    assert!(!paths.iter().any(|p| p.contains("User.cs")),
        "User.cs (depth 2 from src) should be excluded with maxDepth=1. Got paths: {:?}", paths);

    // Deep.cs (depth 3 from src) — MUST be excluded
    assert!(!paths.iter().any(|p| p.contains("Deep.cs")),
        "Deep.cs (depth 3 from src) should be excluded with maxDepth=1. Got paths: {:?}", paths);

    // Test.cs (outside src) — MUST be excluded by subdir_entry_filter
    assert!(!paths.iter().any(|p| p.contains("Test.cs")),
        "Test.cs (outside src) should not appear. Got paths: {:?}", paths);

    // Now test maxDepth=2: should include User.cs but still exclude Deep.cs
    let result2 = handle_xray_fast(&ctx, &json!({
        "pattern": "*",
        "dir": src_str,
        "maxDepth": 2
    }));
    assert!(!result2.is_error, "maxDepth=2 should not error: {}", result2.content[0].text);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let files2 = output2["files"].as_array().unwrap();
    let paths2: Vec<&str> = files2.iter().map(|f| f["path"].as_str().unwrap()).collect();

    assert!(paths2.iter().any(|p| p.contains("User.cs")),
        "User.cs (depth 2 from src) should be included with maxDepth=2. Got paths: {:?}", paths2);
    assert!(!paths2.iter().any(|p| p.contains("Deep.cs")),
        "Deep.cs (depth 3 from src) should still be excluded with maxDepth=2. Got paths: {:?}", paths2);
    assert!(!paths2.iter().any(|p| p.contains("Test.cs")),
        "Test.cs (outside src) should not appear with maxDepth=2. Got paths: {:?}", paths2);

    cleanup_tmp(&tmp_dir);
}

/// T16 — xray_fast regex mode.
#[test]
fn test_xray_fast_regex_mode() {
    let (ctx, tmp) = make_xray_fast_ctx();

    let result = handle_xray_fast(&ctx, &json!({"pattern": ".*Tracker\\.cs$", "regex": true}));
    assert!(!result.is_error, "regex search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 1,
        "Regex '.*Tracker\\.cs$' should match exactly InventoryTracker.cs");
    let files = output["files"].as_array().unwrap();
    assert!(files[0]["path"].as_str().unwrap().contains("InventoryTracker"),
        "Matched file should be InventoryTracker.cs");

    let result2 = handle_xray_fast(&ctx, &json!({"pattern": "Order.*\\.cs$", "regex": true}));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    assert_eq!(output2["summary"]["totalMatches"], 2,
        "Regex 'Order.*\\.cs");

    cleanup_tmp(&tmp);
}
/// T76 — xray_fast empty pattern without dir → error.
#[test]
fn test_xray_fast_empty_pattern() {
    let (ctx, tmp) = make_xray_fast_ctx();

    // Empty pattern WITHOUT dir → error
    let result = handle_xray_fast(&ctx, &json!({"pattern": ""}));
    assert!(result.is_error, "Empty pattern without dir should return an error");
    assert!(result.content[0].text.to_lowercase().contains("empty"),
        "Error should mention 'empty', got: {}", result.content[0].text);
    assert!(result.content[0].text.contains("Do NOT fall back"),
        "Error should warn against fallback, got: {}", result.content[0].text);

    cleanup_tmp(&tmp);
}
/// xray_fast ranking: exact stem match sorts first, then prefix, then contains.
#[test]
fn test_xray_fast_ranking_exact_stem_first() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_rank_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create files with names that test different match tiers
    for name in &["UserService.cs", "UserServiceFactory.cs", "IUserService.cs", "UserServiceHelpers.cs"] {
        let p = tmp_dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "// {}", name).unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str, index_base: idx_base, ..Default::default() };

    let result = handle_xray_fast(&ctx, &json!({"pattern": "UserService"}));
    assert!(!result.is_error, "xray_fast should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    assert!(files.len() >= 3, "Should find at least 3 files matching 'UserService', got {}", files.len());

    // First result should be "UserService.cs" — exact stem match (tier 0)
    let first_path = files[0]["path"].as_str().unwrap();
    assert!(first_path.contains("UserService.cs") && !first_path.contains("Factory") && !first_path.contains("Helper") && !first_path.contains("IUser"),
        "First result should be exact stem match 'UserService.cs', got '{}'", first_path);

    // IUserService.cs should be after prefix matches (UserServiceFactory, UserServiceHelpers)
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
    let iuser_pos = paths.iter().position(|p| p.contains("IUserService"));
    let factory_pos = paths.iter().position(|p| p.contains("UserServiceFactory"));

    if let (Some(iuser), Some(factory)) = (iuser_pos, factory_pos) {
        assert!(factory < iuser,
            "Prefix match 'UserServiceFactory' (pos {}) should come before contains match 'IUserService' (pos {})",
            factory, iuser);
    }

    cleanup_tmp(&tmp_dir);
}

/// xray_fast ranking: among prefix matches, shorter stems sort first.
#[test]
fn test_xray_fast_ranking_shorter_stem_first() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_rank_len_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    for name in &["OrderA.cs", "OrderABC.cs", "OrderABCDEF.cs"] {
        let p = tmp_dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "// {}", name).unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str, index_base: idx_base, ..Default::default() };

    let result = handle_xray_fast(&ctx, &json!({"pattern": "Order"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    assert_eq!(files.len(), 3, "Should find exactly 3 files");

    // All are prefix matches (tier 1). Shorter stems should come first.
    let stems: Vec<&str> = files.iter().map(|f| {
        let path = f["path"].as_str().unwrap();
        std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("")
    }).collect();

    for i in 0..stems.len() - 1 {
        assert!(stems[i].len() <= stems[i + 1].len(),
            "Stems should be sorted by length: '{}' ({}) should come before '{}' ({})",
            stems[i], stems[i].len(), stems[i + 1], stems[i + 1].len());
    }

    cleanup_tmp(&tmp_dir);
}
/// BUG-5: xray_fast with pattern="" without dir should return error.
#[test]
fn test_xray_fast_empty_pattern_returns_error() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": ""}));
    assert!(result.is_error, "Empty pattern without dir should return an error");
    assert!(result.content[0].text.to_lowercase().contains("empty"),
        "Error should mention 'empty', got: {}", result.content[0].text);
    assert!(result.content[0].text.contains("Do NOT fall back"),
        "Error should warn against fallback, got: {}", result.content[0].text);
    cleanup_tmp(&tmp);
}

// ═══════════════════════════════════════════════════════════════════════
// Wildcard listing tests
// ═══════════════════════════════════════════════════════════════════════

/// Wildcard pattern="*" returns all files and directories.
#[test]
fn test_xray_fast_wildcard_star() {
    let (ctx, tmp) = make_xray_fast_ctx();
    // make_xray_fast_ctx creates 6 files: OrderProcessor.cs, OrderValidator.cs,
    // InventoryTracker.cs, ConfigurationHelper.cs, UserService.cs, OtherFile.txt
    let result = handle_xray_fast(&ctx, &json!({"pattern": "*"}));
    assert!(!result.is_error, "Wildcard '*' should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let matches = output["summary"]["totalMatches"].as_u64().unwrap();
    assert!(matches >= 6, "Wildcard '*' should match at least 6 entries (files), got {}", matches);
    cleanup_tmp(&tmp);
}

/// Wildcard pattern="*" with dirsOnly returns only directories.
#[test]
fn test_xray_fast_wildcard_star_dirs_only() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_wc_dirs_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create subdirectories and files
    for sub in &["Actions", "Cache", "ContentScan", "Evaluation"] {
        let _ = std::fs::create_dir_all(tmp_dir.join(sub));
        let f_path = tmp_dir.join(sub).join("dummy.cs");
        { let mut f = std::fs::File::create(&f_path).unwrap(); writeln!(f, "// dummy").unwrap(); }
    }
    { let mut f = std::fs::File::create(&tmp_dir.join("RootFile.cs")).unwrap(); writeln!(f, "// root").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str, index_base: idx_base, ..Default::default() };

    let result = handle_xray_fast(&ctx, &json!({"pattern": "*", "dirsOnly": true}));
    assert!(!result.is_error, "Wildcard + dirsOnly should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let matches = output["summary"]["totalMatches"].as_u64().unwrap();
    assert!(matches >= 4, "Should find at least 4 directories, got {}", matches);

    // All results should be directories
    for entry in output["files"].as_array().unwrap() {
        assert_eq!(entry["isDir"], true, "dirsOnly should only return directories, got: {}", entry);
    }

    cleanup_tmp(&tmp_dir);
}

/// Empty pattern with dir specified → wildcard listing (not an error).
#[test]
fn test_xray_fast_empty_pattern_with_dir() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_wc_empty_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create some files
    for name in &["Alpha.cs", "Beta.cs", "Gamma.txt"] {
        let mut f = std::fs::File::create(&tmp_dir.join(name)).unwrap();
        writeln!(f, "// {}", name).unwrap();
    }
    let sub = tmp_dir.join("SubDir");
    let _ = std::fs::create_dir_all(&sub);
    { let mut f = std::fs::File::create(&sub.join("Inner.cs")).unwrap(); writeln!(f, "// inner").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str.clone(), index_base: idx_base, ..Default::default() };

    // Empty pattern + dir → wildcard (not an error)
    let result = handle_xray_fast(&ctx, &json!({"pattern": "", "dir": dir_str}));
    assert!(!result.is_error, "Empty pattern WITH dir should be wildcard, not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let matches = output["summary"]["totalMatches"].as_u64().unwrap();
    assert!(matches >= 4, "Empty pattern + dir should list all entries (at least 4: 3 files + 1 dir + 1 inner file), got {}", matches);

    cleanup_tmp(&tmp_dir);
}

/// Empty pattern without dir → still an error (unchanged behavior).
#[test]
fn test_xray_fast_empty_pattern_without_dir_still_errors() {
    let (ctx, tmp) = make_xray_fast_ctx();
    let result = handle_xray_fast(&ctx, &json!({"pattern": ""}));
    assert!(result.is_error, "Empty pattern without dir should still be an error");
    assert!(result.content[0].text.contains("Do NOT fall back"),
        "Error should contain anti-fallback warning, got: {}", result.content[0].text);
    cleanup_tmp(&tmp);
}


/// BUG: dirsOnly + ext filter should NOT filter out directories.
/// Previously, ext="cs" combined with dirsOnly=true returned 0 results because
/// directories have no file extension. The fix skips the ext filter for dirsOnly.
#[test]
fn test_xray_fast_dirs_only_ignores_ext_filter() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_dirsext_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create a subdirectory "Services" with a .cs file inside
    let sub = tmp_dir.join("Services");
    let _ = std::fs::create_dir_all(&sub);
    let file_in_sub = sub.join("OrderService.cs");
    { let mut f = std::fs::File::create(&file_in_sub).unwrap(); writeln!(f, "// svc").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str, index_base: idx_base, ..Default::default() };

    // dirsOnly=true + ext="cs" should find the "Services" directory (ext is ignored for dirs)
    let result = handle_xray_fast(&ctx, &json!({"pattern": "Services", "dirsOnly": true, "ext": "cs"}));
    assert!(!result.is_error, "dirsOnly + ext should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalMatches"].as_u64().unwrap() >= 1,
        "dirsOnly + ext should find directories (ext ignored). Got 0 matches.");
    // Verify the hint is emitted
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("ext filter ignored"),
        "Should emit a hint about ext being ignored, got: '{}'", hint);
    // All results should be directories
    for entry in output["files"].as_array().unwrap() {
        assert_eq!(entry["isDir"], true, "dirsOnly should only return directories");
    }

    cleanup_tmp(&tmp_dir);
}

/// Regression guard: dirsOnly without ext should continue to work.
#[test]
fn test_xray_fast_dirs_only_without_ext() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_dirsnoext_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let sub = tmp_dir.join("Controllers");
    let _ = std::fs::create_dir_all(&sub);
    let file_in_sub = sub.join("ApiController.cs");
    { let mut f = std::fs::File::create(&file_in_sub).unwrap(); writeln!(f, "// ctrl").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str, index_base: idx_base, ..Default::default() };

    // dirsOnly=true without ext should work fine
    let result = handle_xray_fast(&ctx, &json!({"pattern": "Controllers", "dirsOnly": true}));
    assert!(!result.is_error, "dirsOnly without ext should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["totalMatches"].as_u64().unwrap() >= 1,
        "Should find 'Controllers' directory");
    // No hint about ext being ignored (ext was not provided)
    assert!(output["summary"]["hint"].is_null(),
        "Should NOT emit ext-ignored hint when ext is not provided");

    cleanup_tmp(&tmp_dir);
}

/// Verify ext filter still works correctly for filesOnly (regression).
#[test]
fn test_xray_fast_files_only_with_ext_still_filters() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_filesext_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    { let mut f = std::fs::File::create(&tmp_dir.join("Report.cs")).unwrap(); writeln!(f, "// cs").unwrap(); }
    { let mut f = std::fs::File::create(&tmp_dir.join("Report.txt")).unwrap(); writeln!(f, "// txt").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs { dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0 }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), server_dir: dir_str, index_base: idx_base, ..Default::default() };

    // filesOnly + ext="cs" should only return Report.cs, not Report.txt
    let result = handle_xray_fast(&ctx, &json!({"pattern": "Report", "filesOnly": true, "ext": "cs"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 1,
        "filesOnly + ext='cs' should match only Report.cs, not Report.txt");
    let files = output["files"].as_array().unwrap();
    assert!(files[0]["path"].as_str().unwrap().contains("Report.cs"),
        "Should match Report.cs");

    cleanup_tmp(&tmp_dir);
}


// ═══════════════════════════════════════════════════════════════════════
// fileCount enrichment + sorting + maxDepth tests
// ═══════════════════════════════════════════════════════════════════════

/// dirsOnly + wildcard returns fileCount for each directory, sorted by fileCount descending.
#[test]
fn test_xray_fast_dirsonly_wildcard_filecount() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_fc_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create directory structure:
    //   BigModule/       (3 files)
    //   SmallModule/     (1 file)
    //   EmptyModule/     (0 files — dir only)
    //   BigModule/Sub/   (2 files — nested, so BigModule total = 5)
    let big = tmp_dir.join("BigModule");
    let small = tmp_dir.join("SmallModule");
    let empty = tmp_dir.join("EmptyModule");
    let sub = big.join("Sub");
    for d in &[&big, &small, &empty, &sub] {
        let _ = std::fs::create_dir_all(d);
    }
    // BigModule: 3 files at top
    for name in &["A.cs", "B.cs", "C.cs"] {
        let mut f = std::fs::File::create(big.join(name)).unwrap();
        writeln!(f, "// {}", name).unwrap();
    }
    // BigModule/Sub: 2 files
    for name in &["D.cs", "E.cs"] {
        let mut f = std::fs::File::create(sub.join(name)).unwrap();
        writeln!(f, "// {}", name).unwrap();
    }
    // SmallModule: 1 file
    {
        let mut f = std::fs::File::create(small.join("F.cs")).unwrap();
        writeln!(f, "// F").unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: dir_str,
        index_base: idx_base,
        ..Default::default()
    };

    let result = handle_xray_fast(&ctx, &json!({"pattern": "*", "dirsOnly": true}));
    assert!(!result.is_error, "dirsOnly wildcard should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    // Should have at least 4 directories: BigModule, BigModule/Sub, SmallModule, EmptyModule
    assert!(files.len() >= 4, "Should find at least 4 directories, got {}", files.len());

    // All entries should have fileCount
    for entry in files {
        assert!(entry.get("fileCount").is_some(),
            "Each directory entry should have fileCount, got: {}", entry);
    }

    // Find BigModule and SmallModule entries
    let big_entry = files.iter().find(|e| {
        let p = e["path"].as_str().unwrap();
        p.ends_with("BigModule") && !p.contains("Sub")
    });
    let small_entry = files.iter().find(|e| e["path"].as_str().unwrap().ends_with("SmallModule"));
    let empty_entry = files.iter().find(|e| e["path"].as_str().unwrap().ends_with("EmptyModule"));

    assert!(big_entry.is_some(), "BigModule should be in results");
    assert!(small_entry.is_some(), "SmallModule should be in results");
    assert!(empty_entry.is_some(), "EmptyModule should be in results");

    // BigModule should have fileCount=5 (3 direct + 2 in Sub)
    let big_fc = big_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(big_fc, 5, "BigModule fileCount should be 5 (3 direct + 2 in Sub), got {}", big_fc);

    // SmallModule should have fileCount=1
    let small_fc = small_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(small_fc, 1, "SmallModule fileCount should be 1, got {}", small_fc);

    // EmptyModule should have fileCount=0
    let empty_fc = empty_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(empty_fc, 0, "EmptyModule fileCount should be 0, got {}", empty_fc);

    // Sorted by fileCount descending: BigModule (5) should be before SmallModule (1)
    let big_pos = files.iter().position(|e| {
        let p = e["path"].as_str().unwrap();
        p.ends_with("BigModule") && !p.contains("Sub")
    }).unwrap();
    let small_pos = files.iter().position(|e| e["path"].as_str().unwrap().ends_with("SmallModule")).unwrap();
    assert!(big_pos < small_pos,
        "BigModule (fileCount=5, pos={}) should come before SmallModule (fileCount=1, pos={})",
        big_pos, small_pos);

    cleanup_tmp(&tmp_dir);
}

/// Non-wildcard dirsOnly does NOT add fileCount.
#[test]
fn test_xray_fast_dirsonly_non_wildcard_no_filecount() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_fc_nw_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let sub = tmp_dir.join("Services");
    let _ = std::fs::create_dir_all(&sub);
    { let mut f = std::fs::File::create(sub.join("Svc.cs")).unwrap(); writeln!(f, "// svc").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: dir_str,
        index_base: idx_base,
        ..Default::default()
    };

    let result = handle_xray_fast(&ctx, &json!({"pattern": "Services", "dirsOnly": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();
    assert!(!files.is_empty(), "Should find Services directory");

    // Non-wildcard: should NOT have fileCount
    for entry in files {
        assert!(entry.get("fileCount").is_none(),
            "Non-wildcard dirsOnly should not have fileCount, got: {}", entry);
    }

    cleanup_tmp(&tmp_dir);
}

/// maxDepth=1 returns only immediate subdirectories.
#[test]
fn test_xray_fast_max_depth() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_md_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create: src/ → src/controllers/ → src/controllers/deep/
    let src = tmp_dir.join("src");
    let controllers = src.join("controllers");
    let deep = controllers.join("deep");
    for d in &[&src, &controllers, &deep] {
        let _ = std::fs::create_dir_all(d);
    }
    // Files at each level
    { let mut f = std::fs::File::create(src.join("main.rs")).unwrap(); writeln!(f, "// main").unwrap(); }
    { let mut f = std::fs::File::create(controllers.join("ctrl.rs")).unwrap(); writeln!(f, "// ctrl").unwrap(); }
    { let mut f = std::fs::File::create(deep.join("inner.rs")).unwrap(); writeln!(f, "// inner").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["rs".to_string()], ..Default::default() };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: dir_str.clone(),
        index_base: idx_base,
        ..Default::default()
    };

    // maxDepth=1: only immediate children (src/)
    let result = handle_xray_fast(&ctx, &json!({"pattern": "*", "dirsOnly": true, "maxDepth": 1}));
    assert!(!result.is_error, "maxDepth should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    // Should find "src" but NOT "src/controllers" or "src/controllers/deep"
    let paths: Vec<&str> = files.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths.iter().any(|p| p.ends_with("src")),
        "maxDepth=1 should find 'src', got: {:?}", paths);
    assert!(!paths.iter().any(|p| p.contains("controllers")),
        "maxDepth=1 should NOT find 'src/controllers', got: {:?}", paths);

    // maxDepth=2: should find src and src/controllers, but not deep
    let result2 = handle_xray_fast(&ctx, &json!({"pattern": "*", "dirsOnly": true, "maxDepth": 2}));
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let files2 = output2["files"].as_array().unwrap();
    let paths2: Vec<&str> = files2.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths2.iter().any(|p| p.contains("controllers") && !p.contains("deep")),
        "maxDepth=2 should find 'src/controllers', got: {:?}", paths2);
    assert!(!paths2.iter().any(|p| p.contains("deep")),
        "maxDepth=2 should NOT find 'src/controllers/deep', got: {:?}", paths2);

    // No maxDepth: all directories
    let result3 = handle_xray_fast(&ctx, &json!({"pattern": "*", "dirsOnly": true}));
    let output3: Value = serde_json::from_str(&result3.content[0].text).unwrap();
    let files3 = output3["files"].as_array().unwrap();
    let paths3: Vec<&str> = files3.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths3.iter().any(|p| p.contains("deep")),
        "No maxDepth should find 'src/controllers/deep', got: {:?}", paths3);

    cleanup_tmp(&tmp_dir);
}

/// Truncation hint is emitted when dirsOnly matches > 150 directories without maxDepth.
#[test]
fn test_xray_fast_dirsonly_truncation_hint() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_hint_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create 160 directories to trigger the hint (> 150)
    for i in 0..160 {
        let sub = tmp_dir.join(format!("dir_{:03}", i));
        let _ = std::fs::create_dir_all(&sub);
        let mut f = std::fs::File::create(sub.join("file.cs")).unwrap();
        writeln!(f, "// {}", i).unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: dir_str,
        index_base: idx_base,
        ..Default::default()
    };

    // Without maxDepth: should get truncation hint
    let result = handle_xray_fast(&ctx, &json!({"pattern": "*", "dirsOnly": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let hint = output["summary"]["hint"].as_str().unwrap_or("");
    assert!(hint.contains("maxDepth"),
        "Should suggest maxDepth when >150 dirs. Hint: '{}'", hint);

    // With maxDepth: should NOT get truncation hint
    let result2 = handle_xray_fast(&ctx, &json!({"pattern": "*", "dirsOnly": true, "maxDepth": 1}));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let hint2 = output2["summary"]["hint"].as_str().unwrap_or("");
    assert!(!hint2.contains("maxDepth"),
        "Should NOT suggest maxDepth when maxDepth is already set. Hint: '{}'", hint2);

    cleanup_tmp(&tmp_dir);
}


/// Regression: maxDepth works when server_dir differs from index.root
/// (e.g., server_dir="." but index.root is the full absolute path).
/// Bug: base_depth was computed from `dir` (which defaults to server_dir),
/// not from `index.root`. When server_dir=".", base_depth=0 but entry paths
/// have full paths with 3+ slashes, so all entries were filtered out.
#[test]
fn test_xray_fast_max_depth_server_dir_mismatch() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_md_mismatch_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create: src/ → src/sub/
    let src = tmp_dir.join("src");
    let sub = src.join("sub");
    for d in &[&src, &sub] {
        let _ = std::fs::create_dir_all(d);
    }
    { let mut f = std::fs::File::create(src.join("a.rs")).unwrap(); writeln!(f, "// a").unwrap(); }
    { let mut f = std::fs::File::create(sub.join("b.rs")).unwrap(); writeln!(f, "// b").unwrap(); }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["rs".to_string()], ..Default::default() };

    // KEY: server_dir is "." (like real MCP), but index.root is the full path
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: ".".to_string(),
        index_base: idx_base,
        ..Default::default()
    };

    // maxDepth=1 should return root + src (not 0 results)
    let result = handle_xray_fast(&ctx, &json!({"pattern": "*", "dirsOnly": true, "maxDepth": 1}));
    assert!(!result.is_error, "maxDepth with server_dir mismatch should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    let paths: Vec<&str> = files.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(!paths.is_empty(),
        "maxDepth=1 should return results even when server_dir='.' differs from index.root. Got 0 results.");
    assert!(paths.iter().any(|p| p.ends_with("src")),
        "maxDepth=1 should find 'src', got: {:?}", paths);
    assert!(!paths.iter().any(|p| p.contains("sub")),
        "maxDepth=1 should NOT find 'src/sub', got: {:?}", paths);

    cleanup_tmp(&tmp_dir);
}


/// Regression test: fileCount must work when `dir` points to a subdirectory
/// (different from server_dir). The LLM typically queries subdirectories like
/// `dir=src/Clients`. The index stores absolute paths. The dir_prefix used for
/// filtering file counts must resolve correctly against index.root.
#[test]
fn test_xray_fast_filecount_with_subdir() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_fc_reldir_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Structure:
    //   src/
    //     ModuleA/       (2 files)
    //     ModuleB/       (1 file)
    //   tests/
    //     TestA/         (1 file)
    let src = tmp_dir.join("src");
    let mod_a = src.join("ModuleA");
    let mod_b = src.join("ModuleB");
    let tests_dir = tmp_dir.join("tests");
    let test_a = tests_dir.join("TestA");
    for d in &[&mod_a, &mod_b, &test_a] {
        let _ = std::fs::create_dir_all(d);
    }
    for name in &["X.cs", "Y.cs"] {
        let mut f = std::fs::File::create(mod_a.join(name)).unwrap();
        writeln!(f, "// {}", name).unwrap();
    }
    {
        let mut f = std::fs::File::create(mod_b.join("Z.cs")).unwrap();
        writeln!(f, "// Z").unwrap();
    }
    {
        let mut f = std::fs::File::create(test_a.join("T.cs")).unwrap();
        writeln!(f, "// T").unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: dir_str.clone(),
        index_base: idx_base,
        ..Default::default()
    };

    // Pass the absolute path of the subdirectory (simulating LLM's dir param
    // after load_index resolves it). The key test: dir != server_dir triggers
    // dir_prefix filtering, and fileCount must still be correct.
    let src_str = src.to_string_lossy().to_string();
    let result = handle_xray_fast(&ctx, &json!({
        "pattern": "*",
        "dir": src_str,
        "dirsOnly": true
    }));
    assert!(!result.is_error, "should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    // Should find at least: src, ModuleA, ModuleB
    assert!(files.len() >= 3, "Should find at least 3 directories under src, got {}", files.len());

    // ModuleA should have fileCount=2
    let mod_a_entry = files.iter().find(|e| e["path"].as_str().unwrap().ends_with("ModuleA"));
    assert!(mod_a_entry.is_some(), "ModuleA should be in results");
    let mod_a_fc = mod_a_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(mod_a_fc, 2, "ModuleA fileCount should be 2, got {}", mod_a_fc);

    // ModuleB should have fileCount=1
    let mod_b_entry = files.iter().find(|e| e["path"].as_str().unwrap().ends_with("ModuleB"));
    assert!(mod_b_entry.is_some(), "ModuleB should be in results");
    let mod_b_fc = mod_b_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(mod_b_fc, 1, "ModuleB fileCount should be 1, got {}", mod_b_fc);

    // src itself should have fileCount=3 (all files under src recursively)
    let src_entry = files.iter().find(|e| {
        let p = e["path"].as_str().unwrap();
        p.ends_with("src") && !p.contains("Module")
    });
    assert!(src_entry.is_some(), "src directory should be in results");
    let src_fc = src_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(src_fc, 3, "src fileCount should be 3 (2 in ModuleA + 1 in ModuleB), got {}", src_fc);

    // TestA should NOT be in results (it's under tests/, not src/)
    let test_entry = files.iter().find(|e| e["path"].as_str().unwrap().contains("TestA"));
    assert!(test_entry.is_none(), "TestA should NOT be in results when dir=src");

    // fileCount should NOT be 0 for directories with files (regression)
    for entry in files {
        let path = entry["path"].as_str().unwrap();
        if path.ends_with("ModuleA") || path.ends_with("ModuleB") {
            let fc = entry["fileCount"].as_u64().unwrap();
            assert!(fc > 0, "fileCount should be > 0 for {}, got 0 (regression!)", path);
        }
    }

    // Sorted by fileCount descending
    let mod_a_pos = files.iter().position(|e| e["path"].as_str().unwrap().ends_with("ModuleA")).unwrap();
    let mod_b_pos = files.iter().position(|e| e["path"].as_str().unwrap().ends_with("ModuleB")).unwrap();
    assert!(mod_a_pos < mod_b_pos,
        "ModuleA (fc=2, pos={}) should come before ModuleB (fc=1, pos={})", mod_a_pos, mod_b_pos);

    cleanup_tmp(&tmp_dir);
}

/// Test that fileCount works correctly with absolute dir paths too (regression-proof).
#[test]
fn test_xray_fast_filecount_with_absolute_dir() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_fc_absdir_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let sub = tmp_dir.join("sub");
    let _ = std::fs::create_dir_all(&sub);
    for name in &["A.cs", "B.cs"] {
        let mut f = std::fs::File::create(sub.join(name)).unwrap();
        writeln!(f, "// {}", name).unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: dir_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: dir_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: dir_str.clone(),
        index_base: idx_base,
        ..Default::default()
    };

    // Pass absolute path for sub directory
    let sub_str = sub.to_string_lossy().to_string();
    let result = handle_xray_fast(&ctx, &json!({
        "pattern": "*",
        "dir": sub_str,
        "dirsOnly": true
    }));
    assert!(!result.is_error, "should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    // Should find at least the sub directory itself
    let sub_entry = files.iter().find(|e| e["path"].as_str().unwrap().ends_with("sub"));
    assert!(sub_entry.is_some(), "sub directory should be in results");
    let sub_fc = sub_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(sub_fc, 2, "sub fileCount should be 2, got {}", sub_fc);

    cleanup_tmp(&tmp_dir);
}


/// Regression test: when `dir` is the absolute path of a subdirectory that
/// equals index.root (load_index built the index FOR that subdir),
/// dir_prefix should be empty — fileCount should count all files in the index.
#[test]
fn test_xray_fast_filecount_when_dir_equals_root() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_fast_fc_rootdir_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Structure:
    //   src/
    //     FileA.cs
    //     sub/
    //       FileB.cs
    let src = tmp_dir.join("src");
    let sub = src.join("sub");
    let _ = std::fs::create_dir_all(&sub);
    {
        let mut f = std::fs::File::create(src.join("FileA.cs")).unwrap();
        writeln!(f, "// A").unwrap();
    }
    {
        let mut f = std::fs::File::create(sub.join("FileB.cs")).unwrap();
        writeln!(f, "// B").unwrap();
    }

    // Build index specifically for the src/ subdirectory (simulates load_index("src"))
    let src_str = src.to_string_lossy().to_string();
    let file_index = crate::build_index(&crate::IndexArgs {
        dir: src_str.clone(), max_age_hours: 24, hidden: false, no_ignore: false, threads: 0,
    }).unwrap();
    let idx_base = tmp_dir.join(".index");
    let _ = crate::save_index(&file_index, &idx_base);
    let content_index = ContentIndex { root: src_str.clone(), extensions: vec!["cs".to_string()], ..Default::default() };

    // KEY: server_dir is the PARENT (tmp_dir), but index.root is src/ (the subdir).
    // Simulates: load_index built index for /project/src → index.root = /project/src.
    // The dir parameter must be the absolute path (load_index resolves relative paths
    // against CWD, which wouldn't point to our test dir).
    let parent_str = tmp_dir.to_string_lossy().to_string();
    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        server_dir: parent_str,
        index_base: idx_base,
        ..Default::default()
    };

    // Pass absolute src path — tests that dir == index.root → dir_prefix = "" → correct counts
    let result = handle_xray_fast(&ctx, &json!({
        "pattern": "*",
        "dir": src_str,
        "dirsOnly": true
    }));
    assert!(!result.is_error, "should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let files = output["files"].as_array().unwrap();

    // Should find src and sub directories
    let src_entry = files.iter().find(|e| {
        let p = e["path"].as_str().unwrap();
        p.ends_with("src") && !p.contains("sub")
    });
    let sub_entry = files.iter().find(|e| e["path"].as_str().unwrap().ends_with("sub"));

    assert!(src_entry.is_some(), "src directory should be in results");
    assert!(sub_entry.is_some(), "sub directory should be in results");

    // src should have fileCount=2 (FileA + FileB recursively)
    let src_fc = src_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(src_fc, 2, "src fileCount should be 2, got {} (regression: dir_prefix was root/src/ instead of empty)", src_fc);

    // sub should have fileCount=1
    let sub_fc = sub_entry.unwrap()["fileCount"].as_u64().unwrap();
    assert_eq!(sub_fc, 1, "sub fileCount should be 1, got {}", sub_fc);

    cleanup_tmp(&tmp_dir);
}
