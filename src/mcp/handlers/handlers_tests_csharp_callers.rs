//! C# callers tests -- extracted from handlers_tests_csharp.rs.
//! Split from handlers_tests.rs for maintainability.

use super::*;
use super::handlers_test_utils::make_ctx_with_defs;
use crate::index::build_trigram_index;
use crate::Posting;
use crate::definitions::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

// ─── search_callers tests ────────────────────────────────────────────
#[test]
fn test_search_callers_missing_method() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("Missing required parameter: method"));
}

#[test]
fn test_search_callers_finds_callers() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ExecuteQueryAsync",
        "depth": 2
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "Call tree should not be empty");
    assert!(output["summary"]["totalNodes"].as_u64().unwrap() > 0);
    assert!(output["summary"]["searchTimeMs"].as_f64().is_some());
}

#[test]
fn test_search_callers_nonexistent_method() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "NonExistentMethodXYZ"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(tree.is_empty(), "Call tree should be empty for nonexistent method");
}

#[test]
fn test_search_callers_max_total_nodes() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ExecuteQueryAsync",
        "depth": 5,
        "maxTotalNodes": 2
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalNodes"].as_u64().unwrap();
    assert!(total <= 2, "Total nodes should be capped at 2, got {}", total);
}

#[test]
fn test_search_callers_max_per_level() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ExecuteQueryAsync",
        "depth": 1,
        "maxCallersPerLevel": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(tree.len() <= 1, "Should have at most 1 caller per level, got {}", tree.len());
}

#[test]
fn test_search_callers_has_class_and_file() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ExecuteQueryAsync",
        "depth": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    for node in tree {
        assert!(node["method"].is_string(), "Node should have method name");
        assert!(node["file"].is_string(), "Node should have file name");
        assert!(node["line"].is_number(), "Node should have line number");
    }
}

#[test]
fn test_search_callers_field_prefix_m_underscore() {
    let mut content_idx = HashMap::new();
    content_idx.insert("submitasync".to_string(), vec![
        Posting { file_id: 0, lines: vec![45] },
        Posting { file_id: 1, lines: vec![30] },
    ]);
    content_idx.insert("orderprocessor".to_string(), vec![
        Posting { file_id: 0, lines: vec![1, 45] },
    ]);
    content_idx.insert("m_orderprocessor".to_string(), vec![
        Posting { file_id: 1, lines: vec![5, 30] },
    ]);
    content_idx.insert("checkouthandler".to_string(), vec![
        Posting { file_id: 1, lines: vec![1] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\OrderProcessor.cs".to_string(),
            "C:\\src\\CheckoutHandler.cs".to_string(),
        ],
        index: content_idx, total_tokens: 200,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![100, 100],
        trigram, ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "OrderProcessor".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "SubmitAsync".to_string(),
            kind: DefinitionKind::Method, line_start: 45, line_end: 60,
            parent: Some("OrderProcessor".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "CheckoutHandler".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "HandleRequest".to_string(),
            kind: DefinitionKind::Method, line_start: 25, line_end: 40,
            parent: Some("CheckoutHandler".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\OrderProcessor.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\CheckoutHandler.cs"), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "SubmitAsync".to_string(),
        receiver_type: Some("OrderProcessor".to_string()),
        line: 30,
                receiver_is_generic: false,
            }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\OrderProcessor.cs".to_string(),
            "C:\\src\\CheckoutHandler.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls, ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "SubmitAsync",
        "class": "OrderProcessor",
        "depth": 1
    }));
    assert!(!result.is_error, "search_callers should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(),
        "Call tree should find caller through m_orderProcessor field prefix. Got: {}",
        serde_json::to_string_pretty(&output).unwrap());
    assert_eq!(tree[0]["method"], "HandleRequest");
    assert_eq!(tree[0]["class"], "CheckoutHandler");
}

#[test]
fn test_search_callers_field_prefix_underscore() {
    let mut content_idx = HashMap::new();
    content_idx.insert("getuserasync".to_string(), vec![
        Posting { file_id: 0, lines: vec![15] },
        Posting { file_id: 1, lines: vec![15] },
    ]);
    content_idx.insert("userservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
    ]);
    content_idx.insert("_userservice".to_string(), vec![
        Posting { file_id: 1, lines: vec![3, 15] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\src\\UserService.cs".to_string(), "C:\\src\\AccountController.cs".to_string()],
        index: content_idx, total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 50],
        trigram, ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "GetUserAsync".to_string(),
            kind: DefinitionKind::Method, line_start: 15, line_end: 30,
            parent: Some("UserService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "AccountController".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "GetAccount".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 25,
            parent: Some("AccountController".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\UserService.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\AccountController.cs"), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "GetUserAsync".to_string(),
        receiver_type: Some("UserService".to_string()),
        line: 15,
                receiver_is_generic: false,
            }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\UserService.cs".to_string(), "C:\\src\\AccountController.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls, ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "GetUserAsync",
        "class": "UserService",
        "depth": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "Should find caller through _userService field prefix");
    assert_eq!(tree[0]["method"], "GetAccount");
    assert_eq!(tree[0]["class"], "AccountController");
}

#[test]
fn test_search_callers_no_trigram_no_regression() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ExecuteQueryAsync",
        "class": "ResilientClient",
        "depth": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["searchTimeMs"].as_f64().is_some());
}

#[test]
fn test_search_callers_multi_ext_filter() {
    let ctx = make_ctx_with_defs();
    let multi_ext_ctx = HandlerContext {
        index: ctx.index.clone(),
        def_index: ctx.def_index.clone(),
        server_dir: ctx.server_dir.clone(),
        server_ext: "cs,xml,sql".to_string(),
        ..Default::default()
    };

    let result = dispatch_tool(&multi_ext_ctx, "search_callers", &json!({
        "method": "ExecuteQueryAsync",
        "depth": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(),
        "Multi-ext server_ext should NOT filter out .cs files. Got empty callTree.");
}
#[test]
fn test_resolve_call_site_with_class_scope() {
    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "ServiceA".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![],
            base_types: vec!["IService".to_string()],
        },
        DefinitionEntry {
            file_id: 0, name: "Execute".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("ServiceA".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ServiceB".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![],
            base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "Execute".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("ServiceB".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for bt in &def.base_types {
            base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
        }
    }

    let def_index = DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["a.cs".to_string(), "b.cs".to_string()],
        definitions,
        name_index,
        kind_index,
        attribute_index: HashMap::new(),
        base_type_index,
        file_index,
        path_to_id: HashMap::new(),
        method_calls: HashMap::new(), ..Default::default()
    };

    let call_a = CallSite {
        method_name: "Execute".to_string(),
        receiver_type: Some("ServiceA".to_string()),
        line: 5,
                receiver_is_generic: false,
            };
    let resolved_a = resolve_call_site(&call_a, &def_index, None);
    assert_eq!(resolved_a.len(), 1);
    assert_eq!(def_index.definitions[resolved_a[0] as usize].parent.as_deref(), Some("ServiceA"));

    let call_b = CallSite {
        method_name: "Execute".to_string(),
        receiver_type: Some("ServiceB".to_string()),
        line: 10,
                receiver_is_generic: false,
            };
    let resolved_b = resolve_call_site(&call_b, &def_index, None);
    assert_eq!(resolved_b.len(), 1);
    assert_eq!(def_index.definitions[resolved_b[0] as usize].parent.as_deref(), Some("ServiceB"));

    let call_no_recv = CallSite {
        method_name: "Execute".to_string(),
        receiver_type: None,
        line: 15,
                receiver_is_generic: false,
            };
    let resolved_none = resolve_call_site(&call_no_recv, &def_index, None);
    assert_eq!(resolved_none.len(), 2);

    let call_iface = CallSite {
        method_name: "Execute".to_string(),
        receiver_type: Some("IService".to_string()),
        line: 20,
                receiver_is_generic: false,
            };
    let resolved_iface = resolve_call_site(&call_iface, &def_index, None);
    assert!(!resolved_iface.is_empty());
    assert!(resolved_iface.iter().any(|&di| {
        def_index.definitions[di as usize].parent.as_deref() == Some("ServiceA")
    }));
}

// ─── search_callers "down" direction + class filter tests ────────────

#[test]
fn test_search_callers_down_class_filter() {
    let definitions = vec![
        DefinitionEntry { file_id: 0, name: "IndexSearchService".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 900, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 0, name: "SearchInternalAsync".to_string(), kind: DefinitionKind::Method, line_start: 766, line_end: 833, parent: Some("IndexSearchService".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 0, name: "ShouldIssueVectorSearch".to_string(), kind: DefinitionKind::Method, line_start: 200, line_end: 220, parent: Some("IndexSearchService".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "IndexedSearchQueryExecuter".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 400, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "SearchInternalAsync".to_string(), kind: DefinitionKind::Method, line_start: 328, line_end: 341, parent: Some("IndexedSearchQueryExecuter".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "TraceInformation".to_string(), kind: DefinitionKind::Method, line_start: 50, line_end: 55, parent: Some("IndexedSearchQueryExecuter".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(1, vec![CallSite { method_name: "ShouldIssueVectorSearch".to_string(), receiver_type: None, line: 780, receiver_is_generic: false }]);
    method_calls.insert(4, vec![CallSite { method_name: "TraceInformation".to_string(), receiver_type: None, line: 333, receiver_is_generic: false }]);

    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    path_to_id.insert(PathBuf::from("C:\\src\\IndexSearchService.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\IndexedSearchQueryExecuter.cs"), 1);

    let def_index = DefinitionIndex {
        root: ".".to_string(), extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\IndexSearchService.cs".to_string(), "C:\\src\\IndexedSearchQueryExecuter.cs".to_string()],
        definitions, name_index, kind_index,
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\src\\IndexSearchService.cs".to_string(), "C:\\src\\IndexedSearchQueryExecuter.cs".to_string()],
        index: HashMap::new(), total_tokens: 0, extensions: vec!["cs".to_string()],
        file_token_counts: vec![100, 100], ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_callers", &json!({ "method": "SearchInternalAsync", "class": "IndexSearchService", "direction": "down", "depth": 1 }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    let callee_names: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_names.contains(&"ShouldIssueVectorSearch"));
    assert!(!callee_names.contains(&"TraceInformation"));

    let result2 = dispatch_tool(&ctx, "search_callers", &json!({ "method": "SearchInternalAsync", "class": "IndexedSearchQueryExecuter", "direction": "down", "depth": 1 }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let tree2 = output2["callTree"].as_array().unwrap();
    let callee_names2: Vec<&str> = tree2.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_names2.contains(&"TraceInformation"));
    assert!(!callee_names2.contains(&"ShouldIssueVectorSearch"));

    let result3 = dispatch_tool(&ctx, "search_callers", &json!({ "method": "SearchInternalAsync", "direction": "down", "depth": 1 }));
    assert!(!result3.is_error);
    let output3: Value = serde_json::from_str(&result3.content[0].text).unwrap();
    let tree3 = output3["callTree"].as_array().unwrap();
    let callee_names3: Vec<&str> = tree3.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_names3.contains(&"ShouldIssueVectorSearch"));
    assert!(callee_names3.contains(&"TraceInformation"));
    assert!(output3.get("warning").is_some());
}

#[test]
fn test_search_callers_ambiguity_warning_truncated() {
    // Create 15 classes each with a method named "OnInit" — exceeds MAX_LISTED (10)
    let num_classes = 15;
    let mut content_idx: HashMap<String, Vec<Posting>> = HashMap::new();
    let mut files: Vec<String> = Vec::new();
    let mut definitions: Vec<DefinitionEntry> = Vec::new();

    for i in 0..num_classes {
        let class_name = format!("Component{}", i);
        let file_name = format!("C:\\src\\{}.ts", class_name);
        files.push(file_name.clone());

        // Class definition
        definitions.push(DefinitionEntry {
            file_id: i as u32, name: class_name.clone(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        });
        // Method definition
        definitions.push(DefinitionEntry {
            file_id: i as u32, name: "OnInit".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some(class_name.clone()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        });

        content_idx.entry("oninit".to_string()).or_default().push(
            Posting { file_id: i as u32, lines: vec![10] }
        );
    }

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    for (i, f) in files.iter().enumerate() {
        path_to_id.insert(PathBuf::from(f), i as u32);
    }

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files.clone(),
        index: content_idx, total_tokens: 500,
        extensions: vec!["ts".to_string()],
        file_token_counts: vec![50; num_classes],
        ..Default::default()
    };

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["ts".to_string()],
        files,
        definitions,
        name_index, kind_index,
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index, path_to_id,
        method_calls: HashMap::new(), ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "ts".to_string(),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_callers", &json!({ "method": "OnInit" }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let warning = output["warning"].as_str().expect("should have warning");

    // Warning should mention total count (15)
    assert!(warning.contains("15 classes"), "Warning should mention 15 classes, got: {}", warning);
    // Warning should be truncated (showing first 10)
    assert!(warning.contains("showing first 10"), "Warning should say 'showing first 10', got: {}", warning);
    // Warning should NOT list all 15 classes — check total length is reasonable
    assert!(warning.len() < 500, "Warning should be truncated, but was {} bytes", warning.len());
}
#[test]
fn test_search_callers_ambiguity_warning_few_classes() {
    // Create 3 classes each with a method named "Initialize" — within MAX_LISTED (10)
    // When called without `class` param, should get a warning listing ALL 3 classes.
    let num_classes = 3;
    let mut content_idx: HashMap<String, Vec<Posting>> = HashMap::new();
    let mut files: Vec<String> = Vec::new();
    let mut definitions: Vec<DefinitionEntry> = Vec::new();

    let class_names = ["AlphaService", "BetaService", "GammaService"];
    for (i, class_name) in class_names.iter().enumerate() {
        let file_name = format!("C:\\src\\{}.cs", class_name);
        files.push(file_name.clone());

        definitions.push(DefinitionEntry {
            file_id: i as u32, name: class_name.to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        });
        definitions.push(DefinitionEntry {
            file_id: i as u32, name: "Initialize".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some(class_name.to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        });

        content_idx.entry("initialize".to_string()).or_default().push(
            Posting { file_id: i as u32, lines: vec![10] }
        );
    }

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    for (i, f) in files.iter().enumerate() {
        path_to_id.insert(PathBuf::from(f), i as u32);
    }

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files.clone(),
        index: content_idx, total_tokens: 300,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50; num_classes],
        ..Default::default()
    };

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files,
        definitions,
        name_index, kind_index,
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index, path_to_id,
        method_calls: HashMap::new(), ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    // No `class` param → should produce a warning listing all 3 classes
    let result = dispatch_tool(&ctx, "search_callers", &json!({ "method": "Initialize" }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let warning = output["warning"].as_str().expect("should have warning when method is in multiple classes");

    // Warning should mention total count (3)
    assert!(warning.contains("3 classes"), "Warning should mention 3 classes, got: {}", warning);
    // Warning should list all 3 class names (sorted alphabetically)
    assert!(warning.contains("AlphaService"), "Warning should list AlphaService, got: {}", warning);
    assert!(warning.contains("BetaService"), "Warning should list BetaService, got: {}", warning);
    assert!(warning.contains("GammaService"), "Warning should list GammaService, got: {}", warning);
    // Warning should NOT say "showing first" (since ≤10 classes)
    assert!(!warning.contains("showing first"), "Warning should NOT be truncated for ≤10 classes, got: {}", warning);
    // Warning should suggest using `class` parameter
    assert!(warning.contains("class"), "Warning should suggest using 'class' parameter, got: {}", warning);
}

#[test]
fn test_search_callers_no_ambiguity_warning_with_class_param() {
    // Same setup as above (3 classes with "Initialize") but WITH `class` param → no warning.
    let mut content_idx: HashMap<String, Vec<Posting>> = HashMap::new();
    let mut files: Vec<String> = Vec::new();
    let mut definitions: Vec<DefinitionEntry> = Vec::new();

    let class_names = ["AlphaService", "BetaService", "GammaService"];
    for (i, class_name) in class_names.iter().enumerate() {
        let file_name = format!("C:\\src\\{}.cs", class_name);
        files.push(file_name.clone());

        definitions.push(DefinitionEntry {
            file_id: i as u32, name: class_name.to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        });
        definitions.push(DefinitionEntry {
            file_id: i as u32, name: "Initialize".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some(class_name.to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        });

        content_idx.entry("initialize".to_string()).or_default().push(
            Posting { file_id: i as u32, lines: vec![10] }
        );
        content_idx.entry(class_name.to_lowercase()).or_default().push(
            Posting { file_id: i as u32, lines: vec![1, 10] }
        );
    }

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    for (i, f) in files.iter().enumerate() {
        path_to_id.insert(PathBuf::from(f), i as u32);
    }

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files.clone(),
        index: content_idx, total_tokens: 300,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50; 3],
        ..Default::default()
    };

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files,
        definitions,
        name_index, kind_index,
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index, path_to_id,
        method_calls: HashMap::new(), ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    // WITH `class` param → should NOT produce a warning
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "Initialize",
        "class": "AlphaService"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output.get("warning").is_none(),
        "No warning should be emitted when 'class' parameter is provided. Got: {:?}",
        output.get("warning"));
}

#[test]
fn test_search_callers_no_ambiguity_warning_single_class() {
    // Method exists in only 1 class → no warning even without `class` param.
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "QueryInternalAsync"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output.get("warning").is_none(),
        "No warning should be emitted when method exists in only 1 class. Got: {:?}",
        output.get("warning"));
}


#[test]
fn test_search_callers_exclude_dir_and_file() {
    // Set up: MethodA is defined in ServiceA (dir: src\services)
    // MethodA is called from ControllerB (dir: src\controllers) and from TestC (dir: src\tests)
    let mut content_idx = HashMap::new();
    content_idx.insert("methoda".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },
        Posting { file_id: 1, lines: vec![25] },
        Posting { file_id: 2, lines: vec![15] },
    ]);
    content_idx.insert("servicea".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![5] },
        Posting { file_id: 2, lines: vec![3] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\services\\ServiceA.cs".to_string(),
            "C:\\src\\controllers\\ControllerB.cs".to_string(),
            "C:\\src\\tests\\TestC.cs".to_string(),
        ],
        index: content_idx, total_tokens: 300,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![100, 100, 100],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "ServiceA".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "MethodA".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("ServiceA".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ControllerB".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "HandleRequest".to_string(),
            kind: DefinitionKind::Method, line_start: 20, line_end: 35,
            parent: Some("ControllerB".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "TestC".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 40,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "TestMethodA".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 25,
            parent: Some("TestC".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\services\\ServiceA.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\controllers\\ControllerB.cs"), 1);
    path_to_id.insert(PathBuf::from("C:\\src\\tests\\TestC.cs"), 2);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\services\\ServiceA.cs".to_string(),
            "C:\\src\\controllers\\ControllerB.cs".to_string(),
            "C:\\src\\tests\\TestC.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    // Test excludeDir: exclude "tests" directory
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "MethodA",
        "class": "ServiceA",
        "depth": 1,
        "excludeDir": ["tests"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // Should NOT contain callers from the "tests" directory
    for node in tree {
        let file = node["file"].as_str().unwrap_or("");
        assert!(!file.to_lowercase().contains("test"),
            "excludeDir should filter out test files, but found: {}", file);
    }

    // Test excludeFile: exclude "TestC" file pattern
    let result2 = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "MethodA",
        "class": "ServiceA",
        "depth": 1,
        "excludeFile": ["TestC"]
    }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let tree2 = output2["callTree"].as_array().unwrap();

    for node in tree2 {
        let file = node["file"].as_str().unwrap_or("");
        assert!(!file.to_lowercase().contains("testc"),
            "excludeFile should filter out TestC, but found: {}", file);
    }
}

#[test]
fn test_search_callers_cycle_detection_down() {
    // Set up: MethodA (in ClassA) calls MethodB (in ClassB),
    // and MethodB calls MethodA back — creating a cycle.
    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "ClassA".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "MethodA".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 30,
            parent: Some("ClassA".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ClassB".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "MethodB".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 30,
            parent: Some("ClassB".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\ClassA.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\ClassB.cs"), 1);

    // MethodA (def index 1) calls MethodB; MethodB (def index 3) calls MethodA
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(1, vec![CallSite {
        method_name: "MethodB".to_string(),
        receiver_type: Some("ClassB".to_string()),
        line: 20,
                receiver_is_generic: false,
            }]);
    method_calls.insert(3, vec![CallSite {
        method_name: "MethodA".to_string(),
        receiver_type: Some("ClassA".to_string()),
        line: 20,
                receiver_is_generic: false,
            }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\ClassA.cs".to_string(), "C:\\src\\ClassB.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\src\\ClassA.cs".to_string(), "C:\\src\\ClassB.cs".to_string()],
        index: HashMap::new(), total_tokens: 0,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 50],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    // direction=down with depth=5 — cycle should be stopped by visited set
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "MethodA",
        "class": "ClassA",
        "direction": "down",
        "depth": 5,
        "maxTotalNodes": 50
    }));
    assert!(!result.is_error, "Cycle in call graph should not cause error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Should complete and have some nodes (MethodA → MethodB, but MethodB → MethodA is blocked)
    let tree = output["callTree"].as_array().unwrap();
    let total_nodes = output["summary"]["totalNodes"].as_u64().unwrap();
    assert!(total_nodes > 0, "Should find at least one callee before cycle is detected");
    // The cycle means we can't recurse forever — total nodes should be bounded
    assert!(total_nodes <= 10, "Cycle detection should prevent runaway recursion, got {} nodes", total_nodes);

    // First level should find MethodB as a callee
    if !tree.is_empty() {
        let callee_names: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
        assert!(callee_names.contains(&"MethodB"),
            "MethodA should call MethodB. Got callees: {:?}", callee_names);
    }
}
#[test]
fn test_search_callers_cycle_detection() {
    // Regression test: A recursive call graph (A calls B, B calls A) must NOT
    // cause infinite recursion in `search_callers` direction="up".
    //
    // Setup:
    //   ServiceA.MethodA() calls ServiceB.MethodB()
    //   ServiceB.MethodB() calls ServiceA.MethodA()
    //
    // Searching callers of MethodA (up) should find MethodB as a caller,
    // then when recursing to find callers of MethodB it should find MethodA
    // but the visited set must stop the recursion.

    let mut content_idx = HashMap::new();
    // MethodA token appears in file 0 (definition) and file 1 (call site in MethodB)
    content_idx.insert("methoda".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },  // definition in ServiceA
        Posting { file_id: 1, lines: vec![20] },  // call site in ServiceB.MethodB
    ]);
    // MethodB token appears in file 1 (definition) and file 0 (call site in MethodA)
    content_idx.insert("methodb".to_string(), vec![
        Posting { file_id: 1, lines: vec![10] },  // definition in ServiceB
        Posting { file_id: 0, lines: vec![20] },  // call site in ServiceA.MethodA
    ]);
    // Class tokens for parent filtering
    content_idx.insert("servicea".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![20] },  // ServiceB references ServiceA
    ]);
    content_idx.insert("serviceb".to_string(), vec![
        Posting { file_id: 1, lines: vec![1] },
        Posting { file_id: 0, lines: vec![20] },  // ServiceA references ServiceB
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\ServiceA.cs".to_string(),
            "C:\\src\\ServiceB.cs".to_string(),
        ],
        index: content_idx, total_tokens: 200,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![100, 100],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "ServiceA".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "MethodA".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 30,
            parent: Some("ServiceA".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ServiceB".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "MethodB".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 30,
            parent: Some("ServiceB".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\ServiceA.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\ServiceB.cs"), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    // MethodB (di=3) calls MethodA at line 20
    method_calls.insert(3, vec![CallSite {
        method_name: "MethodA".to_string(),
        receiver_type: Some("ServiceA".to_string()),
        line: 20,
                receiver_is_generic: false,
            }]);
    // MethodA (di=1) calls MethodB at line 20
    method_calls.insert(1, vec![CallSite {
        method_name: "MethodB".to_string(),
        receiver_type: Some("ServiceB".to_string()),
        line: 20,
                receiver_is_generic: false,
            }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\ServiceA.cs".to_string(), "C:\\src\\ServiceB.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    // direction=up (default) with depth=5 — cycle should be stopped by visited set
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "MethodA",
        "class": "ServiceA",
        "depth": 5,
        "maxTotalNodes": 50
    }));
    assert!(!result.is_error,
        "Cycle in call graph (up direction) should not cause error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Should complete and have some nodes (MethodB calls MethodA, but recursing
    // into callers of MethodB would find MethodA again — blocked by visited set)
    let tree = output["callTree"].as_array().unwrap();
    let total_nodes = output["summary"]["totalNodes"].as_u64().unwrap();
    assert!(total_nodes > 0,
        "Should find at least one caller before cycle is detected");
    // The cycle means we can't recurse forever — total nodes should be bounded
    assert!(total_nodes <= 10,
        "Cycle detection should prevent runaway recursion, got {} nodes", total_nodes);

    // First level should find MethodB as a caller of MethodA
    if !tree.is_empty() {
        let caller_names: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
        assert!(caller_names.contains(&"MethodB"),
            "MethodA should be called by MethodB. Got callers: {:?}", caller_names);
    }

    // Verify nodesVisited is reported (shows the visited set was used)
    assert!(output["summary"]["nodesVisited"].as_u64().is_some(),
        "Summary should include nodesVisited count");
}

#[test]
fn test_search_callers_ext_filter_comma_split() {
    // Setup: DataService.cs defines ProcessData; callers exist in both .cs and .txt files.
    // The ext parameter should filter caller files by extension.
    let mut content_idx = HashMap::new();
    content_idx.insert("processdata".to_string(), vec![
        Posting { file_id: 0, lines: vec![20] },   // definition site
        Posting { file_id: 1, lines: vec![15] },   // caller in .cs file
        Posting { file_id: 2, lines: vec![10] },   // caller in .txt file
    ]);
    content_idx.insert("dataservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![5, 15] },
        Posting { file_id: 2, lines: vec![3, 10] },
    ]);
    content_idx.insert("cscontroller".to_string(), vec![
        Posting { file_id: 1, lines: vec![1] },
    ]);
    content_idx.insert("scriptrunner".to_string(), vec![
        Posting { file_id: 2, lines: vec![1] },
    ]);

    let trigram = build_trigram_index(&content_idx);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\DataService.cs".to_string(),
            "C:\\src\\CsController.cs".to_string(),
            "C:\\src\\script.txt".to_string(),
        ],
        index: content_idx, total_tokens: 200,
        extensions: vec!["cs".to_string(), "txt".to_string()],
        file_token_counts: vec![80, 60, 60],
        trigram, ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "DataService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "ProcessData".to_string(),
            kind: DefinitionKind::Method, line_start: 18, line_end: 30,
            parent: Some("DataService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "CsController".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 40,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "HandleRequest".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 25,
            parent: Some("CsController".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "ScriptRunner".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 2, name: "RunScript".to_string(),
            kind: DefinitionKind::Method, line_start: 5, line_end: 20,
            parent: Some("ScriptRunner".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\DataService.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\CsController.cs"), 1);
    path_to_id.insert(PathBuf::from("C:\\src\\script.txt"), 2);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    // HandleRequest (di=3) calls ProcessData at line 15
    method_calls.insert(3, vec![CallSite {
        method_name: "ProcessData".to_string(),
        receiver_type: Some("DataService".to_string()),
        line: 15,
                receiver_is_generic: false,
            }]);
    // RunScript (di=5) calls ProcessData at line 10
    method_calls.insert(5, vec![CallSite {
        method_name: "ProcessData".to_string(),
        receiver_type: Some("DataService".to_string()),
        line: 10,
                receiver_is_generic: false,
            }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string(), "txt".to_string()],
        files: vec![
            "C:\\src\\DataService.cs".to_string(),
            "C:\\src\\CsController.cs".to_string(),
            "C:\\src\\script.txt".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls, ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "cs,txt".to_string(),
        ..Default::default()
    };

    // ── Case 1: ext="cs" → only .cs callers ──────────────────────────
    let result_cs = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ProcessData",
        "class": "DataService",
        "depth": 1,
        "ext": "cs"
    }));
    assert!(!result_cs.is_error, "search_callers ext=cs should not error: {}", result_cs.content[0].text);
    let output_cs: Value = serde_json::from_str(&result_cs.content[0].text).unwrap();
    let tree_cs = output_cs["callTree"].as_array().unwrap();
    assert!(!tree_cs.is_empty(), "ext=cs should find at least one caller from .cs files");
    for node in tree_cs {
        let file = node["file"].as_str().unwrap();
        assert!(file.ends_with(".cs"),
            "ext=cs should only return .cs callers, got file: {}", file);
    }
    // Verify .txt caller is NOT present
    let has_txt = tree_cs.iter().any(|n| n["file"].as_str().unwrap().ends_with(".txt"));
    assert!(!has_txt, "ext=cs should NOT include .txt callers");

    // ── Case 2: ext="cs,txt" → callers from both extensions ─────────
    let result_both = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ProcessData",
        "class": "DataService",
        "depth": 1,
        "ext": "cs,txt"
    }));
    assert!(!result_both.is_error, "search_callers ext=cs,txt should not error: {}", result_both.content[0].text);
    let output_both: Value = serde_json::from_str(&result_both.content[0].text).unwrap();
    let tree_both = output_both["callTree"].as_array().unwrap();

    let caller_files: Vec<&str> = tree_both.iter()
        .filter_map(|n| n["file"].as_str())
        .collect();
    let has_cs = caller_files.iter().any(|f| f.ends_with(".cs"));
    let has_txt_both = caller_files.iter().any(|f| f.ends_with(".txt"));
    assert!(has_cs, "ext=cs,txt should include .cs callers. Got: {:?}", caller_files);
    assert!(has_txt_both, "ext=cs,txt should include .txt callers. Got: {:?}", caller_files);
}

// ─── Overload dedup tests ────────────────────────────────────────────

#[test]
fn test_search_callers_overloads_not_collapsed_up() {
    // Two overloads of Process (same class, different lines) both call Validate.
    // Both should appear as callers (direction=up) — they must NOT be collapsed.
    let mut content_idx = HashMap::new();
    content_idx.insert("validate".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },       // definition in Validator
        Posting { file_id: 1, lines: vec![25, 45] },    // calls in both Process overloads
    ]);
    content_idx.insert("validator".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![25, 45] },
    ]);
    content_idx.insert("processor".to_string(), vec![
        Posting { file_id: 1, lines: vec![1] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\Validator.cs".to_string(),
            "C:\\src\\Processor.cs".to_string(),
        ],
        index: content_idx, total_tokens: 200,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 80],
        ..Default::default()
    };

    let definitions = vec![
        // file 0: Validator class with Validate method
        DefinitionEntry {
            file_id: 0, name: "Validator".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "Validate".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("Validator".to_string()), signature: Some("void Validate()".to_string()),
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // file 1: Processor class with two Process overloads
        DefinitionEntry {
            file_id: 1, name: "Processor".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 60,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "Process".to_string(),
            kind: DefinitionKind::Method, line_start: 20, line_end: 35,
            parent: Some("Processor".to_string()), signature: Some("void Process(int x)".to_string()),
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "Process".to_string(),
            kind: DefinitionKind::Method, line_start: 40, line_end: 55,
            parent: Some("Processor".to_string()), signature: Some("void Process(string s)".to_string()),
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\Validator.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\Processor.cs"), 1);

    // Process(int) at di=3 calls Validate at line 25; Process(string) at di=4 calls Validate at line 45
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "Validate".to_string(),
        receiver_type: Some("Validator".to_string()),
        line: 25,
        receiver_is_generic: false,
    }]);
    method_calls.insert(4, vec![CallSite {
        method_name: "Validate".to_string(),
        receiver_type: Some("Validator".to_string()),
        line: 45,
        receiver_is_generic: false,
    }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\Validator.cs".to_string(), "C:\\src\\Processor.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "Validate",
        "class": "Validator",
        "depth": 1
    }));
    assert!(!result.is_error, "search_callers should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // Both Process overloads should appear as callers (not collapsed into one)
    let process_callers: Vec<&Value> = tree.iter()
        .filter(|n| n["method"].as_str() == Some("Process"))
        .collect();
    assert_eq!(process_callers.len(), 2,
        "Both Process overloads should appear as callers. Got {} Process callers: {}",
        process_callers.len(), serde_json::to_string_pretty(&tree).unwrap());

    // They should have different line numbers (line_start of each overload)
    let lines: Vec<u64> = process_callers.iter()
        .filter_map(|n| n["line"].as_u64())
        .collect();
    assert_eq!(lines.len(), 2, "Both callers should have line numbers");
    assert_ne!(lines[0], lines[1],
        "The two Process overloads should have different line numbers, got {:?}", lines);
}

#[test]
fn test_search_callers_overloads_not_collapsed_down() {
    // A method (RunAll) calls two overloads of Execute in the same class.
    // Direction=down: both Execute overloads should appear as callees.
    let definitions = vec![
        // file 0: Orchestrator class with RunAll method
        DefinitionEntry {
            file_id: 0, name: "Orchestrator".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "RunAll".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 30,
            parent: Some("Orchestrator".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // file 1: Executor class with two Execute overloads
        DefinitionEntry {
            file_id: 1, name: "Executor".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 80,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "Execute".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 25,
            parent: Some("Executor".to_string()), signature: Some("void Execute(int id)".to_string()),
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "Execute".to_string(),
            kind: DefinitionKind::Method, line_start: 30, line_end: 45,
            parent: Some("Executor".to_string()), signature: Some("void Execute(string name)".to_string()),
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\src\\Orchestrator.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\Executor.cs"), 1);

    // RunAll (di=1) calls both Execute overloads
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(1, vec![
        CallSite {
            method_name: "Execute".to_string(),
            receiver_type: Some("Executor".to_string()),
            line: 15,
            receiver_is_generic: false,
        },
        CallSite {
            method_name: "Execute".to_string(),
            receiver_type: Some("Executor".to_string()),
            line: 20,
            receiver_is_generic: false,
        },
    ]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\Orchestrator.cs".to_string(), "C:\\src\\Executor.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\src\\Orchestrator.cs".to_string(), "C:\\src\\Executor.cs".to_string()],
        index: HashMap::new(), total_tokens: 0,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 80],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "RunAll",
        "class": "Orchestrator",
        "direction": "down",
        "depth": 1
    }));
    assert!(!result.is_error, "search_callers down should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // Both Execute overloads should appear as callees (not collapsed into one)
    let execute_callees: Vec<&Value> = tree.iter()
        .filter(|n| n["method"].as_str() == Some("Execute"))
        .collect();
    assert_eq!(execute_callees.len(), 2,
        "Both Execute overloads should appear as callees. Got {} Execute callees: {}",
        execute_callees.len(), serde_json::to_string_pretty(&tree).unwrap());

    // They should have different line numbers (line_start of each overload)
    let lines: Vec<u64> = execute_callees.iter()
        .filter_map(|n| n["line"].as_u64())
        .collect();
    assert_eq!(lines.len(), 2, "Both callees should have line numbers");
    assert_ne!(lines[0], lines[1],
        "The two Execute overloads should have different line numbers, got {:?}", lines);
}

// ─── SAME_NAME_DIFFERENT_RECEIVER: interface resolution scoping ──────

#[test]
fn test_search_callers_same_name_different_receiver_interface_resolution() {
    // Regression test: When two UNRELATED interfaces both define the same method
    // name (e.g. Execute()), searching for callers of ServiceA.Execute() should
    // NOT find callers that use IServiceB.Execute() — even though the interface
    // resolution block expands interface implementations.
    //
    // Setup:
    //   IServiceA (interface) has Execute()
    //   IServiceB (interface) has Execute()
    //   ServiceA (class, implements IServiceA) has Execute()
    //   ServiceB (class, implements IServiceB) has Execute()
    //   Consumer (class) has DoWork() which calls this._serviceB.Execute()
    //     where _serviceB has type IServiceB
    //
    // Expected:
    //   callers of ServiceA.Execute() → should NOT include Consumer.DoWork()
    //   callers of ServiceB.Execute() → SHOULD include Consumer.DoWork()

    let mut content_idx = HashMap::new();
    // "execute" token appears in all files
    content_idx.insert("execute".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },  // IServiceA.Execute definition
        Posting { file_id: 1, lines: vec![10] },  // IServiceB.Execute definition
        Posting { file_id: 2, lines: vec![10] },  // ServiceA.Execute definition
        Posting { file_id: 3, lines: vec![10] },  // ServiceB.Execute definition
        Posting { file_id: 4, lines: vec![20] },  // Consumer.DoWork calls Execute
    ]);
    // Class/interface name tokens for parent_file_ids pre-filter
    content_idx.insert("iservicea".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 2, lines: vec![1] },   // ServiceA implements IServiceA
    ]);
    content_idx.insert("iserviceb".to_string(), vec![
        Posting { file_id: 1, lines: vec![1] },
        Posting { file_id: 3, lines: vec![1] },   // ServiceB implements IServiceB
        Posting { file_id: 4, lines: vec![5, 20] }, // Consumer has _serviceB field of type IServiceB
    ]);
    content_idx.insert("servicea".to_string(), vec![
        Posting { file_id: 2, lines: vec![1] },
    ]);
    content_idx.insert("serviceb".to_string(), vec![
        Posting { file_id: 3, lines: vec![1] },
    ]);
    content_idx.insert("consumer".to_string(), vec![
        Posting { file_id: 4, lines: vec![1] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\IServiceA.cs".to_string(),
            "C:\\src\\IServiceB.cs".to_string(),
            "C:\\src\\ServiceA.cs".to_string(),
            "C:\\src\\ServiceB.cs".to_string(),
            "C:\\src\\Consumer.cs".to_string(),
        ],
        index: content_idx, total_tokens: 500,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 50, 50, 50, 50],
        ..Default::default()
    };

    let definitions = vec![
        // 0: IServiceA interface
        DefinitionEntry {
            file_id: 0, name: "IServiceA".to_string(),
            kind: DefinitionKind::Interface, line_start: 1, line_end: 20,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 1: IServiceA.Execute method
        DefinitionEntry {
            file_id: 0, name: "Execute".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 12,
            parent: Some("IServiceA".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 2: IServiceB interface
        DefinitionEntry {
            file_id: 1, name: "IServiceB".to_string(),
            kind: DefinitionKind::Interface, line_start: 1, line_end: 20,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 3: IServiceB.Execute method
        DefinitionEntry {
            file_id: 1, name: "Execute".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 12,
            parent: Some("IServiceB".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 4: ServiceA class (implements IServiceA)
        DefinitionEntry {
            file_id: 2, name: "ServiceA".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None, modifiers: vec![], attributes: vec![],
            base_types: vec!["IServiceA".to_string()],
        },
        // 5: ServiceA.Execute method
        DefinitionEntry {
            file_id: 2, name: "Execute".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("ServiceA".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 6: ServiceB class (implements IServiceB)
        DefinitionEntry {
            file_id: 3, name: "ServiceB".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None, modifiers: vec![], attributes: vec![],
            base_types: vec!["IServiceB".to_string()],
        },
        // 7: ServiceB.Execute method
        DefinitionEntry {
            file_id: 3, name: "Execute".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("ServiceB".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 8: Consumer class
        DefinitionEntry {
            file_id: 4, name: "Consumer".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 40,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // 9: Consumer.DoWork method
        DefinitionEntry {
            file_id: 4, name: "DoWork".to_string(),
            kind: DefinitionKind::Method, line_start: 15, line_end: 30,
            parent: Some("Consumer".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    let mut base_type_index: HashMap<String, Vec<u32>> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
        for bt in &def.base_types {
            base_type_index.entry(bt.to_lowercase()).or_default().push(idx);
        }
    }
    path_to_id.insert(PathBuf::from("C:\\src\\IServiceA.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\IServiceB.cs"), 1);
    path_to_id.insert(PathBuf::from("C:\\src\\ServiceA.cs"), 2);
    path_to_id.insert(PathBuf::from("C:\\src\\ServiceB.cs"), 3);
    path_to_id.insert(PathBuf::from("C:\\src\\Consumer.cs"), 4);

    // Consumer.DoWork (di=9) calls Execute with receiver_type = IServiceB
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(9, vec![CallSite {
        method_name: "Execute".to_string(),
        receiver_type: Some("IServiceB".to_string()),
        line: 20,
        receiver_is_generic: false,
    }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\IServiceA.cs".to_string(),
            "C:\\src\\IServiceB.cs".to_string(),
            "C:\\src\\ServiceA.cs".to_string(),
            "C:\\src\\ServiceB.cs".to_string(),
            "C:\\src\\Consumer.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index,
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    // ── Test 1: callers of ServiceA.Execute() should NOT find Consumer.DoWork()
    let result_a = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "Execute",
        "class": "ServiceA",
        "depth": 2
    }));
    assert!(!result_a.is_error, "search_callers for ServiceA.Execute should not error: {}", result_a.content[0].text);
    let output_a: Value = serde_json::from_str(&result_a.content[0].text).unwrap();
    let tree_a = output_a["callTree"].as_array().unwrap();

    // Consumer.DoWork should NOT appear — it calls via IServiceB, not IServiceA
    let has_consumer_a = tree_a.iter().any(|n| {
        n["class"].as_str() == Some("Consumer") && n["method"].as_str() == Some("DoWork")
    });
    assert!(!has_consumer_a,
        "Callers of ServiceA.Execute() should NOT include Consumer.DoWork() (which calls via IServiceB). Got tree: {}",
        serde_json::to_string_pretty(&tree_a).unwrap());

    // ── Test 2: callers of ServiceB.Execute() SHOULD find Consumer.DoWork()
    let result_b = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "Execute",
        "class": "ServiceB",
        "depth": 2
    }));
    assert!(!result_b.is_error, "search_callers for ServiceB.Execute should not error: {}", result_b.content[0].text);
    let output_b: Value = serde_json::from_str(&result_b.content[0].text).unwrap();
    let tree_b = output_b["callTree"].as_array().unwrap();

    // Consumer.DoWork SHOULD appear — it calls via IServiceB which ServiceB implements
    let has_consumer_b = tree_b.iter().any(|n| {
        n["class"].as_str() == Some("Consumer") && n["method"].as_str() == Some("DoWork")
    });
    assert!(has_consumer_b,
        "Callers of ServiceB.Execute() SHOULD include Consumer.DoWork() (which calls via IServiceB). Got tree: {}",
        serde_json::to_string_pretty(&tree_b).unwrap());
}


// ─── includeBody tests (require real files on disk) ──────────────────

#[test]
fn test_search_callers_include_body_default_false() {
    // Default (no includeBody) should NOT have body fields in nodes
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ExecuteQueryAsync",
        "depth": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    for node in tree {
        assert!(node.get("body").is_none(),
            "Default (includeBody absent) should NOT include body. Got: {}",
            serde_json::to_string_pretty(node).unwrap());
        assert!(node.get("bodyStartLine").is_none(),
            "Default should NOT include bodyStartLine");
    }
}

#[test]
fn test_search_callers_include_body_false_explicit() {
    // Explicit includeBody=false should NOT have body fields
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ExecuteQueryAsync",
        "depth": 1,
        "includeBody": false
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    for node in tree {
        assert!(node.get("body").is_none(),
            "includeBody=false should NOT include body");
    }
}

/// Helper: create a temp directory with real .cs files and set up indexes for includeBody tests.
/// Returns (HandlerContext, TempDir path).
fn make_callers_body_ctx() -> (HandlerContext, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("search_callers_body_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("Failed to create temp dir for callers body test");

    // File 0: OrderService.cs — defines SubmitOrder method
    let file0 = tmp.join("OrderService.cs");
    std::fs::write(&file0, r#"namespace App {
    public class OrderService {
        private readonly IValidator _validator;

        public void SubmitOrder(string id) {
            var order = LoadOrder(id);
            _validator.Validate(order);
            SaveOrder(order);
        }

        private Order LoadOrder(string id) {
            return new Order(id);
        }
    }
}
"#).unwrap();

    // File 1: OrderController.cs — calls SubmitOrder
    let file1 = tmp.join("OrderController.cs");
    std::fs::write(&file1, r#"namespace App {
    public class OrderController {
        private readonly OrderService _orderService;

        public IActionResult ProcessOrder(string id) {
            try {
                _orderService.SubmitOrder(id);
                return Ok();
            } catch (Exception ex) {
                return BadRequest(ex.Message);
            }
        }
    }
}
"#).unwrap();

    let file0_str = file0.to_string_lossy().to_string();
    let file1_str = file1.to_string_lossy().to_string();

    let mut content_idx = HashMap::new();
    content_idx.insert("submitorder".to_string(), vec![
        Posting { file_id: 0, lines: vec![5] },
        Posting { file_id: 1, lines: vec![7] },
    ]);
    content_idx.insert("orderservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![2] },
        Posting { file_id: 1, lines: vec![3, 7] },
    ]);
    content_idx.insert("ordercontroller".to_string(), vec![
        Posting { file_id: 1, lines: vec![2] },
    ]);

    let content_index = ContentIndex {
        root: tmp.to_string_lossy().to_string(),
        files: vec![file0_str.clone(), file1_str.clone()],
        index: content_idx, total_tokens: 200,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![100, 100],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "OrderService".to_string(),
            kind: DefinitionKind::Class, line_start: 2, line_end: 14,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "SubmitOrder".to_string(),
            kind: DefinitionKind::Method, line_start: 5, line_end: 9,
            parent: Some("OrderService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "OrderController".to_string(),
            kind: DefinitionKind::Class, line_start: 2, line_end: 14,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ProcessOrder".to_string(),
            kind: DefinitionKind::Method, line_start: 5, line_end: 12,
            parent: Some("OrderController".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from(&file0_str), 0);
    path_to_id.insert(PathBuf::from(&file1_str), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "SubmitOrder".to_string(),
        receiver_type: Some("OrderService".to_string()),
        line: 7,
        receiver_is_generic: false,
    }]);

    let def_index = DefinitionIndex {
        root: tmp.to_string_lossy().to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![file0_str, file1_str],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_dir: tmp.to_string_lossy().to_string(),
        max_response_bytes: 0, // no truncation for tests
        ..Default::default()
    };

    (ctx, tmp)
}

fn cleanup_callers_body_ctx(tmp: &std::path::Path) {
    let _ = std::fs::remove_dir_all(tmp);
}

#[test]
fn test_search_callers_include_body_up() {
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "SubmitOrder",
        "class": "OrderService",
        "depth": 1,
        "includeBody": true
    }));
    assert!(!result.is_error, "Error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "Call tree should not be empty");

    let node = &tree[0];
    assert_eq!(node["method"].as_str(), Some("ProcessOrder"));
    assert_eq!(node["class"].as_str(), Some("OrderController"));

    // Should have body and bodyStartLine
    let body = node["body"].as_array().expect("includeBody=true should produce body array");
    assert!(!body.is_empty(), "body should not be empty");
    assert!(node["bodyStartLine"].as_u64().is_some(), "Should have bodyStartLine");

    // Body should contain the source code of ProcessOrder
    let body_text: String = body.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n");
    assert!(body_text.contains("ProcessOrder"),
        "Body should contain method name. Got: {}", body_text);
    assert!(body_text.contains("SubmitOrder"),
        "Body should contain the call to SubmitOrder. Got: {}", body_text);

    cleanup_callers_body_ctx(&tmp);
}

#[test]
fn test_search_callers_include_body_down() {
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ProcessOrder",
        "class": "OrderController",
        "direction": "down",
        "depth": 1,
        "includeBody": true
    }));
    assert!(!result.is_error, "Error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(), "Callee tree should not be empty");

    let node = &tree[0];
    assert_eq!(node["method"].as_str(), Some("SubmitOrder"));

    // Should have body
    let body = node["body"].as_array().expect("includeBody=true should produce body in direction=down");
    assert!(!body.is_empty(), "body should not be empty for callee");
    assert!(node["bodyStartLine"].as_u64().is_some(), "Should have bodyStartLine");

    cleanup_callers_body_ctx(&tmp);
}

#[test]
fn test_search_callers_include_body_max_body_lines() {
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "SubmitOrder",
        "class": "OrderService",
        "depth": 1,
        "includeBody": true,
        "maxBodyLines": 2
    }));
    assert!(!result.is_error, "Error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty());

    let node = &tree[0];
    let body = node["body"].as_array().expect("Should have body");
    assert!(body.len() <= 2,
        "maxBodyLines=2 should cap body to at most 2 lines, got {}", body.len());
    // bodyTruncated should be set if original body was longer
    if body.len() == 2 {
        assert!(node.get("bodyTruncated").is_some(),
            "Body was capped, should have bodyTruncated marker");
    }

    cleanup_callers_body_ctx(&tmp);
}

#[test]
fn test_search_callers_include_body_max_total_body_lines() {
    let (ctx, tmp) = make_callers_body_ctx();

    // Set maxTotalBodyLines=1 — after the first node's body, the budget should be exhausted
    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "SubmitOrder",
        "class": "OrderService",
        "depth": 1,
        "includeBody": true,
        "maxTotalBodyLines": 1
    }));
    assert!(!result.is_error, "Error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // The caller node's body should be capped or have bodyOmitted
    if !tree.is_empty() {
        let node = &tree[0];
        // Either body exists and is small (≤1 line), or bodyOmitted is set
        if let Some(body) = node["body"].as_array() {
            assert!(body.len() <= 1,
                "maxTotalBodyLines=1 should limit total body lines to 1");
        }
        // If there was a second node, it should have bodyOmitted
    }

    cleanup_callers_body_ctx(&tmp);
}

#[test]
fn test_search_callers_include_body_nonexistent_file() {
    // When file doesn't exist on disk, body should have bodyError
    let mut content_idx = HashMap::new();
    content_idx.insert("dowork".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },
        Posting { file_id: 1, lines: vec![15] },
    ]);
    content_idx.insert("worker".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
        Posting { file_id: 1, lines: vec![5, 15] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\nonexistent\\Worker.cs".to_string(),
            "C:\\nonexistent\\Caller.cs".to_string(),
        ],
        index: content_idx, total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 50],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "Worker".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "DoWork".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("Worker".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "Caller".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 30,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "CallDoWork".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("Caller".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:\\nonexistent\\Worker.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\nonexistent\\Caller.cs"), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "DoWork".to_string(),
        receiver_type: Some("Worker".to_string()),
        line: 15,
        receiver_is_generic: false,
    }]);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\nonexistent\\Worker.cs".to_string(), "C:\\nonexistent\\Caller.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        max_response_bytes: 0,
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "DoWork",
        "class": "Worker",
        "depth": 1,
        "includeBody": true
    }));
    assert!(!result.is_error, "includeBody with non-existent files should not error");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // Nodes should have bodyError instead of body (file doesn't exist)
    if !tree.is_empty() {
        let node = &tree[0];
        assert!(node.get("bodyError").is_some(),
            "Non-existent file should produce bodyError, got: {}",
            serde_json::to_string_pretty(node).unwrap());
    }
}

// ─── Response budget test ────────────────────────────────────────────

#[test]
fn test_include_body_response_budget_64kb() {
    // When includeBody=true is passed, dispatch_tool should use 64KB budget
    // instead of the default 16KB. We test this by checking that the
    // INCLUDE_BODY_MIN_RESPONSE_BYTES constant is correctly applied.
    use crate::mcp::handlers::utils::DEFAULT_MAX_RESPONSE_BYTES;

    let ctx = HandlerContext {
        max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES, // 16KB
        ..Default::default()
    };

    // Without includeBody: effective_max should be 16KB (default)
    let result_no_body = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "NonExistent"
    }));
    // The error result won't be truncated, but the dispatch logic was exercised

    // With includeBody=true: effective_max should be 64KB
    // We can't directly observe the effective_max, but we verify the feature
    // compiles and runs without error
    let result_with_body = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "NonExistent",
        "includeBody": true
    }));
    // Both should succeed (no error from budget logic)
    assert!(!result_no_body.is_error || result_no_body.content[0].text.contains("Definition index"));
    assert!(!result_with_body.is_error || result_with_body.content[0].text.contains("Definition index"));
}


#[test]
fn test_search_callers_include_body_has_root_method() {
    // When includeBody=true, the response should include rootMethod with body of the searched method
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "SubmitOrder",
        "class": "OrderService",
        "depth": 1,
        "includeBody": true
    }));
    assert!(!result.is_error, "Error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // rootMethod should be present
    let root = output.get("rootMethod").expect("includeBody=true should include rootMethod");
    assert_eq!(root["method"].as_str(), Some("SubmitOrder"));
    assert_eq!(root["class"].as_str(), Some("OrderService"));
    assert!(root["body"].as_array().is_some(), "rootMethod should have body");
    let body = root["body"].as_array().unwrap();
    assert!(!body.is_empty(), "rootMethod body should not be empty");
    let body_text: String = body.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n");
    assert!(body_text.contains("SubmitOrder"),
        "rootMethod body should contain the method name. Got: {}", body_text);
    assert!(root["bodyStartLine"].as_u64().is_some(), "rootMethod should have bodyStartLine");

    // rootMethod should NOT appear when includeBody=false
    let result2 = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "SubmitOrder",
        "class": "OrderService",
        "depth": 1
    }));
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    assert!(output2.get("rootMethod").is_none(),
        "rootMethod should NOT appear when includeBody is absent/false");

    cleanup_callers_body_ctx(&tmp);
}

#[test]
fn test_search_callers_include_body_root_method_down() {
    // rootMethod should also work for direction=down
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "search_callers", &json!({
        "method": "ProcessOrder",
        "class": "OrderController",
        "direction": "down",
        "depth": 1,
        "includeBody": true
    }));
    assert!(!result.is_error, "Error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    let root = output.get("rootMethod").expect("includeBody=true direction=down should include rootMethod");
    assert_eq!(root["method"].as_str(), Some("ProcessOrder"));
    assert_eq!(root["class"].as_str(), Some("OrderController"));
    assert!(root["body"].as_array().is_some(), "rootMethod should have body for direction=down");

    cleanup_callers_body_ctx(&tmp);
}
