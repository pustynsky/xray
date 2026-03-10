//! Tests for search_fast -- extracted from handlers_tests.rs.

use super::*;
use super::fast::handle_search_fast;
use super::handlers_test_utils::cleanup_tmp;
use crate::ContentIndex;
use std::sync::{Arc, RwLock};
// --- search_fast comma-separated tests ---

fn make_search_fast_ctx() -> (HandlerContext, std::path::PathBuf) {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_fast_test_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);
    for name in &["ModelSchemaStorage.cs", "ModelSchemaManager.cs", "ScannerJobState.cs", "WorkspaceInfoUtils.cs", "UserService.cs", "OtherFile.txt"] {
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

#[test] fn test_search_fast_single_pattern() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": "ModelSchemaStorage"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 1);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_fast_comma_separated_multi_term() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": "ModelSchemaStorage,ModelSchemaManager,ScannerJobState,WorkspaceInfoUtils"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 4);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_fast_comma_separated_with_ext_filter() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": "ModelSchemaStorage,OtherFile", "ext": "cs"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 1);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_fast_comma_separated_no_matches() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": "NonExistentClass,AnotherMissing"}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 0);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_fast_comma_separated_partial_matches() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": "ModelSchemaStorage,NonExistent,ScannerJobState"}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 2);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_fast_comma_separated_with_spaces() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": " ModelSchemaStorage , ScannerJobState "}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 2);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_fast_comma_separated_count_only() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": "ModelSchemaStorage,ScannerJobState", "countOnly": true}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 2);
    assert!(output["files"].as_array().unwrap().is_empty());
    cleanup_tmp(&tmp);
}

#[test] fn test_search_fast_comma_separated_ignore_case() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": "modelschemastorage,scannerjobstate", "ignoreCase": true}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 2);
    cleanup_tmp(&tmp);
}
// ═══════════════════════════════════════════════════════════════════════
// Batch 2 tests — Strengthen Partial Coverage
// ═══════════════════════════════════════════════════════════════════════

/// T15 — search_fast dirsOnly and filesOnly filters.
#[test]
fn test_search_fast_dirs_only_and_files_only() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_fast_dironly_{}_{}", std::process::id(), id));
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

    let result_dirs = handle_search_fast(&ctx, &json!({"pattern": "Models", "dirsOnly": true}));
    assert!(!result_dirs.is_error, "dirsOnly should not error: {}", result_dirs.content[0].text);
    let output_dirs: Value = serde_json::from_str(&result_dirs.content[0].text).unwrap();
    let dir_files = output_dirs["files"].as_array().unwrap();
    for entry in dir_files {
        assert_eq!(entry["isDir"], true, "dirsOnly should only return directories, got: {}", entry);
    }
    assert!(output_dirs["summary"]["totalMatches"].as_u64().unwrap() >= 1,
        "Should find at least one directory matching 'Models'");

    let result_files = handle_search_fast(&ctx, &json!({"pattern": "Models", "filesOnly": true}));
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

/// T16 — search_fast regex mode.
#[test]
fn test_search_fast_regex_mode() {
    let (ctx, tmp) = make_search_fast_ctx();

    let result = handle_search_fast(&ctx, &json!({"pattern": ".*State\\.cs$", "regex": true}));
    assert!(!result.is_error, "regex search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 1,
        "Regex '.*State\\.cs$' should match exactly ScannerJobState.cs");
    let files = output["files"].as_array().unwrap();
    assert!(files[0]["path"].as_str().unwrap().contains("ScannerJobState"),
        "Matched file should be ScannerJobState.cs");

    let result2 = handle_search_fast(&ctx, &json!({"pattern": "Model.*\\.cs$", "regex": true}));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    assert_eq!(output2["summary"]["totalMatches"], 2,
        "Regex 'Model.*\\.cs$' should match ModelSchemaStorage.cs and ModelSchemaManager.cs");

    cleanup_tmp(&tmp);
}
/// T76 — search_fast empty pattern edge case.
#[test]
fn test_search_fast_empty_pattern() {
    let (ctx, tmp) = make_search_fast_ctx();

    let result = handle_search_fast(&ctx, &json!({"pattern": ""}));

    if result.is_error {
        assert!(result.content[0].text.contains("Missing") || result.content[0].text.contains("pattern") || result.content[0].text.contains("empty"),
            "Error should mention missing/empty pattern, got: {}", result.content[0].text);
    } else {
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert_eq!(output["summary"]["totalMatches"], 0,
            "Empty pattern should return 0 matches");
    }

    cleanup_tmp(&tmp);
}
/// search_fast ranking: exact stem match sorts first, then prefix, then contains.
#[test]
fn test_search_fast_ranking_exact_stem_first() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_fast_rank_{}_{}", std::process::id(), id));
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

    let result = handle_search_fast(&ctx, &json!({"pattern": "UserService"}));
    assert!(!result.is_error, "search_fast should not error: {}", result.content[0].text);
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

/// search_fast ranking: among prefix matches, shorter stems sort first.
#[test]
fn test_search_fast_ranking_shorter_stem_first() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_fast_rank_len_{}_{}", std::process::id(), id));
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

    let result = handle_search_fast(&ctx, &json!({"pattern": "Order"}));
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
/// BUG-5: search_fast with pattern="" should return error.
#[test]
fn test_search_fast_empty_pattern_returns_error() {
    let (ctx, tmp) = make_search_fast_ctx();
    let result = handle_search_fast(&ctx, &json!({"pattern": ""}));
    assert!(result.is_error, "Empty pattern should return an error");
    assert!(result.content[0].text.to_lowercase().contains("empty"),
        "Error should mention 'empty', got: {}", result.content[0].text);
    cleanup_tmp(&tmp);
}


/// BUG: dirsOnly + ext filter should NOT filter out directories.
/// Previously, ext="cs" combined with dirsOnly=true returned 0 results because
/// directories have no file extension. The fix skips the ext filter for dirsOnly.
#[test]
fn test_search_fast_dirs_only_ignores_ext_filter() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_fast_dirsext_{}_{}", std::process::id(), id));
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
    let result = handle_search_fast(&ctx, &json!({"pattern": "Services", "dirsOnly": true, "ext": "cs"}));
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
fn test_search_fast_dirs_only_without_ext() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_fast_dirsnoext_{}_{}", std::process::id(), id));
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
    let result = handle_search_fast(&ctx, &json!({"pattern": "Controllers", "dirsOnly": true}));
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
fn test_search_fast_files_only_with_ext_still_filters() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_fast_filesext_{}_{}", std::process::id(), id));
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
    let result = handle_search_fast(&ctx, &json!({"pattern": "Report", "filesOnly": true, "ext": "cs"}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["summary"]["totalMatches"], 1,
        "filesOnly + ext='cs' should match only Report.cs, not Report.txt");
    let files = output["files"].as_array().unwrap();
    assert!(files[0]["path"].as_str().unwrap().contains("Report.cs"),
        "Should match Report.cs");

    cleanup_tmp(&tmp_dir);
}
