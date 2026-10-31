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

fn make_rs_macro_ctx(files: &[(&str, &str)]) -> (HandlerContext, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let canonical_root = std::fs::canonicalize(tmp.path()).unwrap();
    let root = crate::clean_path(&canonical_root.to_string_lossy());
    let mut definitions = DefinitionIndex {
        root: root.clone(),
        extensions: vec!["rs".to_string()],
        format_version: DEFINITION_INDEX_VERSION,
        ..Default::default()
    };
    for (name, source) in files {
        let path = canonical_root.join(name);
        std::fs::write(&path, source).unwrap();
        update_file_definitions(&mut definitions, &path);
    }
    let content = crate::build_content_index(&crate::ContentIndexArgs {
        dir: root.clone(), ext: "rs".to_string(), max_age_hours: 24,
        hidden: false, no_ignore: false, respect_git_exclude: false,
        threads: 1, min_token_len: 2,
    }).unwrap();
    (HandlerContext {
        index: Arc::new(RwLock::new(content)),
        def_index: Some(Arc::new(RwLock::new(definitions))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(root))),
        server_ext: "rs".to_string(),
        ..Default::default()
    }, tmp)
}

fn rust_macro_graph(ctx: &HandlerContext, args: Value) -> Value {
    let result = dispatch_tool(ctx, "xray_callers", &args);
    assert!(!result.is_error, "{}", result.content[0].text);
    serde_json::from_str(&result.content[0].text).unwrap()
}

const RUST_MACRO_GRAPH_SOURCE: &str = r#"
async fn main() {
    tokio::select! {
        client = accept() => {
            tokio::spawn(async move {
                handle(client).await;
            });
        }
        _ = shutdown() => {},
    }
}
async fn handle(client: ()) {}
fn accept() {}
fn shutdown() {}
fn mentions() { println!("handle()"); let handle = 0; }
"#;

#[test]
fn test_rust_macro_graph_is_symmetric_without_text_false_positives() {
    let (ctx, _tmp) = make_rs_macro_ctx(&[("main.rs", RUST_MACRO_GRAPH_SOURCE)]);
    let down = rust_macro_graph(&ctx, json!({"method": ["main"], "direction": "down", "depth": 1}));
    assert!(down["callTree"].as_array().unwrap().iter().any(|node| node["method"] == "handle"), "{down}");
    let up = rust_macro_graph(&ctx, json!({"method": ["handle"], "direction": "up", "depth": 1}));
    let callers = up["callTree"].as_array().unwrap();
    assert_eq!(callers.len(), 1, "{up}");
    assert_eq!(callers[0]["method"], "main");
    let expected_line = RUST_MACRO_GRAPH_SOURCE.lines().position(|line| line.contains("handle(client).await")).unwrap() as u32 + 1;
    assert_eq!(callers[0]["callSite"], expected_line);
    let handle = down["callTree"].as_array().unwrap().iter().find(|node| node["method"] == "handle").unwrap();
    assert_eq!(handle["callSiteLine"], expected_line);
    for output in [&up, &down] {
        assert_eq!(output["resultStatus"]["complete"], true, "{output}");
        assert_eq!(output["resultStatus"]["safeForExhaustiveClaims"], true);
    }
}

#[test]
fn test_rust_macro_graph_reports_unknown_macro_gaps_in_both_directions_and_batches() {
    let (ctx, _tmp) = make_rs_macro_ctx(&[
        ("main.rs", RUST_MACRO_GRAPH_SOURCE),
        ("opaque.rs", "fn hidden() { emit_calls!(); }"),
    ]);
    for direction in ["up", "down"] {
        let methods = if direction == "up" { json!(["handle"]) } else { json!(["hidden"]) };
        for methods in [methods, json!(["handle", "hidden"])] {
            let output = rust_macro_graph(&ctx, json!({"method": methods, "direction": direction, "depth": 1}));
            assert_eq!(output["resultStatus"]["status"], "partial", "{output}");
            assert_eq!(output["resultStatus"]["complete"], false);
            assert_eq!(output["resultStatus"]["safeForExhaustiveClaims"], false);
            assert_eq!(output["resultStatus"]["totalKnown"], false);
            assert!(output["resultStatus"]["reasons"].as_array().unwrap()
                .iter().any(|reason| reason == "unexpanded_rust_macro"), "{output}");
            assert!(output["summary"]["unresolvedCallSites"].as_u64().unwrap() > 0);
        }
    }
    let scoped = rust_macro_graph(&ctx, json!({
        "method": ["handle"], "direction": "up", "excludeFile": ["opaque.rs"], "depth": 1
    }));
    assert_eq!(scoped["resultStatus"]["complete"], true, "{scoped}");
}

#[test]
fn test_rust_macro_graph_zero_match_and_production_scope_are_truthful() {
    let (ctx, _tmp) = make_rs_macro_ctx(&[
        ("main.rs", "fn handle() {}"),
        ("opaque.rs", "#[test]\nfn hidden() { emit_calls!(); }"),
    ]);
    let output = rust_macro_graph(&ctx, json!({"method": ["handle"], "direction": "up"}));
    assert!(output["callTree"].as_array().unwrap().is_empty());
    assert_eq!(output["resultStatus"]["status"], "partial", "{output}");
    assert_eq!(output["resultStatus"]["safeForExhaustiveClaims"], false);
    let production = rust_macro_graph(&ctx, json!({
        "method": ["handle"], "direction": "up", "productionOnly": true
    }));
    assert_eq!(production["resultStatus"]["complete"], true, "{production}");
    assert_eq!(production["resultStatus"]["status"], "not_found");
}

#[test]
fn test_rust_macro_graph_cache_roundtrip_and_old_version_rejection() {
    let limited = format!("fn limited() {{ {} handle() {} }}", "dbg!(".repeat(128), ")".repeat(128));
    let (ctx, tmp) = make_rs_macro_ctx(&[
        ("main.rs", RUST_MACRO_GRAPH_SOURCE),
        ("opaque.rs", "fn hidden() { emit_calls!(); }"),
        ("nested.rs", "fn outer() { fn inner() { handle(); } }"),
        ("limited.rs", &limited),
    ]);
    let lock = ctx.def_index.as_ref().unwrap();
    let index = lock.read().unwrap();
    save_definition_index(&index, tmp.path()).unwrap();
    let loaded = load_definition_index(&index.root, "rs", tmp.path()).unwrap();
    assert_eq!(loaded.method_calls, index.method_calls);
    for reason in ["unindexed_rust_nested_callable", "rust_call_analysis_limit"] {
        assert!(loaded.method_calls.values().flatten().any(|call| call.call_kind.unresolved_reason() == Some(reason)));
    }
    let mut old_head = index.build_head();
    old_head.format_version = 11;
    let path = definition_index_path_for(&index.root, "rs", tmp.path());
    let entries: Vec<_> = index.definitions.iter().collect();
    crate::index::save_sharded(&path, &old_head, entries, "definition-index").unwrap();
    let error = load_definition_index(&index.root, "rs", tmp.path()).unwrap_err();
    assert!(error.to_string().contains("format version mismatch"), "{error}");
    assert_eq!(DEFINITION_INDEX_VERSION, 12);
    drop(index);
    *lock.write().unwrap() = loaded;
    let down = rust_macro_graph(&ctx, json!({"method": ["main"], "direction": "down", "depth": 1}));
    assert!(down["callTree"].as_array().unwrap().iter().any(|node| node["method"] == "handle"));
    let up = rust_macro_graph(&ctx, json!({"method": ["handle"], "direction": "up", "depth": 1}));
    assert_eq!(up["resultStatus"]["complete"], false);
    assert!(up["summary"]["unresolvedCallReasons"]["unexpanded_rust_macro"].as_u64().unwrap() > 0);
}

#[test]
fn test_rust_macro_graph_up_gaps_follow_caller_scope_not_target_file_scope() {
    let (ctx, _tmp) = make_rs_macro_ctx(&[
        ("target.rs", "fn handle() {}"),
        ("caller.rs", "fn known() { handle(); }\nfn hidden() { emit_calls!(); }"),
    ]);
    let output = rust_macro_graph(&ctx, json!({
        "method": ["handle"], "direction": "up", "excludeFile": ["target.rs"]
    }));
    assert!(output["callTree"].as_array().unwrap().iter().any(|node| node["method"] == "known"));
    assert_eq!(output["resultStatus"]["complete"], false, "{output}");
    assert_eq!(output["summary"]["unresolvedCallSitesScope"], "eligibleRustCallersAndTraversedGraph");
    let excluded = rust_macro_graph(&ctx, json!({
        "method": ["handle"], "direction": "up", "excludeFile": ["caller.rs"]
    }));
    assert_eq!(excluded["resultStatus"]["complete"], true, "{excluded}");
    let other_language = rust_macro_graph(&ctx, json!({
        "method": ["handle"], "direction": "up", "ext": ["cs"]
    }));
    assert_eq!(other_language["resultStatus"]["complete"], true, "{other_language}");
}

#[test]
fn test_rust_macro_graph_multiline_call_names_preserve_both_directions() {
    let source = "struct Client;\nimpl Client {\nfn new() -> Self { Self }\nfn send(&self) -> &Self { self }\nfn finish(&self) {}\nfn drive(&self) {\nself\n.send();\nself\n.finish();\nClient\n::new();\n}\n}\nfn chain() {\nClient\n::new()\n.send()\n.finish();\n}";
    let (ctx, _tmp) = make_rs_macro_ctx(&[("main.rs", source)]);
    let down = rust_macro_graph(&ctx, json!({"method": ["drive"], "direction": "down", "depth": 1}));
    for (name, line) in [("new", 12), ("send", 8), ("finish", 10)] {
        let up = rust_macro_graph(&ctx, json!({"method": [name], "direction": "up", "depth": 1}));
        let caller = up["callTree"].as_array().unwrap().iter().find(|node| node["method"] == "drive");
        assert!(caller.is_some(), "{name}: {up}");
        assert_eq!(caller.unwrap()["callSite"], line, "{up}");
        let callee = down["callTree"].as_array().unwrap().iter().find(|node| node["method"] == name)
            .unwrap_or_else(|| panic!("{name}: {down}"));
        assert_eq!(callee["callSiteLine"], line, "{down}");
        let chain_line = match name { "new" => 17, "send" => 18, _ => 19 };
        let chain = up["callTree"].as_array().unwrap().iter()
            .find(|node| node["method"] == "chain").unwrap_or_else(|| panic!("{name}: {up}"));
        assert_eq!(chain["callSite"], chain_line, "{up}");
    }
}

#[test]
fn test_rust_macro_graph_external_and_missing_targets_have_truthful_up_gaps() {
    let (ctx, _tmp) = make_rs_macro_ctx(&[(
        "main.rs", "fn known() { external(); }\nfn hidden() { emit_calls!(); }",
    )]);
    let single = rust_macro_graph(&ctx, json!({"method": ["external"], "direction": "up"}));
    assert!(single["callTree"].as_array().unwrap().iter().any(|node| node["method"] == "known"));
    assert_eq!(single["resultStatus"]["complete"], false, "{single}");
    assert_eq!(single["summary"]["unresolvedCallSitesScope"], "eligibleRustCallersAndTraversedGraph");
    let batch = rust_macro_graph(&ctx, json!({"method": ["external", "absent"], "direction": "up"}));
    assert_eq!(batch["resultStatus"]["complete"], false, "{batch}");
    for result in batch["results"].as_array().unwrap() {
        assert!(result["unresolvedCallReasons"]["unexpanded_rust_macro"].as_u64().unwrap() > 0);
    }
    let excluded = rust_macro_graph(&ctx, json!({"method": ["external"], "direction": "up", "ext": ["cs"]}));
    assert_eq!(excluded["resultStatus"]["complete"], true, "{excluded}");
}

#[test]
fn test_rust_macro_graph_unindexed_nested_functions_report_gaps_without_false_edges() {
    let source = "fn target() {}\nfn outer() { fn inner() { target(); } inner(); }";
    let (ctx, _tmp) = make_rs_macro_ctx(&[("main.rs", source)]);
    for (method, direction) in [("outer", "down"), ("target", "up")] {
        let result = rust_macro_graph(&ctx, json!({"method": [method], "direction": direction}));
        assert!(result["callTree"].as_array().unwrap().is_empty(), "{result}");
        assert_eq!(result["resultStatus"]["complete"], false, "{result}");
        assert_eq!(result["summary"]["unresolvedCallReasons"]["unindexed_rust_nested_callable"], 1);
    }
}

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
        "method": ["process"],
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
        def_extensions: vec!["rs".to_string()],
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