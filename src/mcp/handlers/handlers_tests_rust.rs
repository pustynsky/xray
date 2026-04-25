//! Rust-specific handler tests — definitions, callers, includeBody, containsLine, reindex.

use super::*;
use super::handlers_test_utils::cleanup_tmp;
use crate::definitions::*;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

// ─── Helpers ─────────────────────────────────────────────────────────

/// Helper: create a context with real temp .rs files and a definition index.
fn make_rs_ctx_with_real_files() -> (HandlerContext, std::path::PathBuf) {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_test_rs_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let file0_path = tmp_dir.join("service.rs");
    {
        let mut f = std::fs::File::create(&file0_path).unwrap();
        writeln!(f, "pub struct OrderService {{").unwrap();     // line 1
        writeln!(f, "    repo: String,").unwrap();               // line 2
        writeln!(f, "}}").unwrap();                               // line 3
        writeln!(f).unwrap();                                 // line 4
        writeln!(f, "impl OrderService {{").unwrap();            // line 5
        writeln!(f, "    pub fn new() -> Self {{").unwrap();     // line 6
        writeln!(f, "        OrderService {{ repo: String::new() }}").unwrap(); // line 7
        writeln!(f, "    }}").unwrap();                           // line 8
        writeln!(f, "    pub fn process(&self, id: u32) {{").unwrap(); // line 9
        writeln!(f, "        self.validate(id);").unwrap();       // line 10
        writeln!(f, "    }}").unwrap();                            // line 11
        writeln!(f, "    fn validate(&self, id: u32) {{}}").unwrap(); // line 12
        writeln!(f, "}}").unwrap();                               // line 13
    }

    let file0_str = file0_path.to_string_lossy().to_string();

    // Build definition index using real tree-sitter parsing
    let mut def_index = DefinitionIndex {
        root: tmp_dir.to_string_lossy().to_string(),
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };

    let clean = PathBuf::from(crate::clean_path(&file0_str));
    update_file_definitions(&mut def_index, &clean);

    let content_index = ContentIndex {
        root: tmp_dir.to_string_lossy().to_string(),
        files: vec![file0_str],
        extensions: vec!["rs".to_string()],
        file_token_counts: vec![0],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "rs".to_string(),
        ..Default::default()
    };
    (ctx, tmp_dir)
}

// ─── xray_definitions tests ────────────────────────────────────────

#[test]
fn test_rust_xray_definitions_finds_struct() {
    let (ctx, tmp) = make_rs_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["OrderService"],
        "kind": ["struct"]
    }));
    assert!(!result.is_error, "xray_definitions should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected 1 struct named OrderService, got {}", defs.len());
    assert_eq!(defs[0]["name"], "OrderService");
    assert_eq!(defs[0]["kind"], "struct");
    cleanup_tmp(&tmp);
}

#[test]
fn test_rust_xray_definitions_finds_method() {
    let (ctx, tmp) = make_rs_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["process"],
        "kind": ["method"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1, "Expected 1 method named process, got {}", defs.len());
    assert_eq!(defs[0]["name"], "process");
    assert_eq!(defs[0]["kind"], "method");
    assert_eq!(defs[0]["parent"], "OrderService");
    cleanup_tmp(&tmp);
}

// ─── xray_callers tests ────────────────────────────────────────────

#[test]
fn test_rust_xray_callers_down() {
    let (ctx, tmp) = make_rs_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": "process",
        "class": "OrderService",
        "direction": "down",
        "depth": 1
    }));
    assert!(!result.is_error, "xray_callers down should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    // process() calls self.validate() — should appear in callees
    let callee_methods: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_methods.contains(&"validate"),
        "process() should call validate(), got callees: {:?}", callee_methods);
    cleanup_tmp(&tmp);
}

// ─── includeBody test ────────────────────────────────────────────────

#[test]
fn test_rust_include_body() {
    let (ctx, tmp) = make_rs_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({
        "name": ["process"],
        "includeBody": true
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1);
    let body = defs[0]["body"].as_array();
    assert!(body.is_some(), "Should have body when includeBody=true");
    assert!(!body.unwrap().is_empty(), "Body should have content");
    cleanup_tmp(&tmp);
}

// ─── Incremental update test ─────────────────────────────────────────

#[test]
fn test_rust_incremental_update_through_handler() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_test_rs_incr_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let rs_file = tmp_dir.join("lib.rs");
    {
        let mut f = std::fs::File::create(&rs_file).unwrap();
        writeln!(f, "pub struct OldService;").unwrap();
        writeln!(f, "impl OldService {{").unwrap();
        writeln!(f, "    pub fn old_method(&self) {{}}").unwrap();
        writeln!(f, "}}").unwrap();
    }

    let file_str = crate::clean_path(&rs_file.to_string_lossy());
    let mut def_index = DefinitionIndex {
        root: tmp_dir.to_string_lossy().to_string(),
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    let clean_path = PathBuf::from(&file_str);
    update_file_definitions(&mut def_index, &clean_path);

    let content_index = ContentIndex {
        root: tmp_dir.to_string_lossy().to_string(),
        files: vec![file_str.clone()],
        extensions: vec!["rs".to_string()],
        file_token_counts: vec![0],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp_dir.to_string_lossy().to_string()))),
        server_ext: "rs".to_string(),
        ..Default::default()
    };

    // Verify OldService is found
    let result = dispatch_tool(&ctx, "xray_definitions", &json!({"name": ["OldService"]}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(!output["definitions"].as_array().unwrap().is_empty(), "OldService should be found");

    // Update file
    {
        let mut f = std::fs::File::create(&rs_file).unwrap();
        writeln!(f, "pub struct NewService;").unwrap();
        writeln!(f, "impl NewService {{").unwrap();
        writeln!(f, "    pub fn new_method(&self) {{}}").unwrap();
        writeln!(f, "}}").unwrap();
    }

    // Incremental update
    {
        let mut idx = ctx.def_index.as_ref().unwrap().write().unwrap();
        update_file_definitions(&mut idx, &clean_path);
    }

    // NewService found, OldService gone
    let result_new = dispatch_tool(&ctx, "xray_definitions", &json!({"name": ["NewService"]}));
    let output_new: Value = serde_json::from_str(&result_new.content[0].text).unwrap();
    assert!(!output_new["definitions"].as_array().unwrap().is_empty(), "NewService should be found");

    let result_old = dispatch_tool(&ctx, "xray_definitions", &json!({"name": ["OldService"]}));
    let output_old: Value = serde_json::from_str(&result_old.content[0].text).unwrap();
    assert!(output_old["definitions"].as_array().unwrap().is_empty(), "OldService should be gone");

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

// ─── Reindex test ────────────────────────────────────────────────────

#[test]
fn test_rust_reindex_definitions_success() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("xray_reindex_rs_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let rs_file = tmp_dir.join("sample.rs");
    {
        let mut f = std::fs::File::create(&rs_file).unwrap();
        writeln!(f, "pub struct SampleService;").unwrap();
        writeln!(f, "impl SampleService {{").unwrap();
        writeln!(f, "    pub fn do_work(&self) {{}}").unwrap();
        writeln!(f, "}}").unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();
    let def_index = DefinitionIndex {
        root: dir_str.clone(),
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };
    let content_index = ContentIndex {
        root: dir_str.clone(),
        extensions: vec!["rs".to_string()],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(dir_str.to_string()))),
        index_base: tmp_dir.join(".index"),
        server_ext: "rs".to_string(),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "xray_reindex_definitions", &json!({}));
    assert!(!result.is_error, "Reindex should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["status"], "ok");
    assert!(output["files"].as_u64().unwrap() >= 1);
    assert!(output["definitions"].as_u64().unwrap() >= 1);

    cleanup_tmp(&tmp_dir);
}