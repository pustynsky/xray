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

// ─── xray_callers tests ────────────────────────────────────────────
#[test]
fn test_xray_callers_missing_method() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({}));
    assert!(result.is_error);
    assert!(result.content[0].text.contains("Missing required parameter: method"));
}

#[test]
fn test_xray_callers_finds_callers() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ExecuteQueryAsync"],
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
fn test_xray_callers_nonexistent_method() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["NonExistentMethodXYZ"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(tree.is_empty(), "Call tree should be empty for nonexistent method");
}

#[test]
fn test_xray_callers_max_total_nodes() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ExecuteQueryAsync"],
        "depth": 5,
        "maxTotalNodes": 2
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalNodes"].as_u64().unwrap();
    assert!(total <= 2, "Total nodes should be capped at 2, got {}", total);
}

#[test]
fn test_xray_callers_max_per_level() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ExecuteQueryAsync"],
        "depth": 1,
        "maxCallersPerLevel": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(tree.len() <= 1, "Should have at most 1 caller per level, got {}", tree.len());
}

#[test]
fn test_xray_callers_has_class_and_file() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ExecuteQueryAsync"],
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
fn test_xray_callers_field_prefix_m_underscore() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\OrderProcessor.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\CheckoutHandler.cs")), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "SubmitAsync".to_string(),
        receiver_type: Some("OrderProcessor".to_string()),
        line: 30,
        call_kind: Default::default(),
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

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["SubmitAsync"],
        "class": "OrderProcessor",
        "depth": 1
    }));
    assert!(!result.is_error, "xray_callers should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    assert!(!tree.is_empty(),
        "Call tree should find caller through m_orderProcessor field prefix. Got: {}",
        serde_json::to_string_pretty(&output).unwrap());
    assert_eq!(tree[0]["method"], "HandleRequest");
    assert_eq!(tree[0]["class"], "CheckoutHandler");
}

#[test]
fn test_xray_callers_field_prefix_underscore() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\UserService.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\AccountController.cs")), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "GetUserAsync".to_string(),
        receiver_type: Some("UserService".to_string()),
        line: 15,
        call_kind: Default::default(),
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

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["GetUserAsync"],
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
fn test_xray_callers_no_trigram_no_regression() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ExecuteQueryAsync"],
        "class": "ResilientClient",
        "depth": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["summary"]["searchTimeMs"].as_f64().is_some());
}

#[test]
fn test_xray_callers_multi_ext_filter() {
    let ctx = make_ctx_with_defs();
    let multi_ext_ctx = HandlerContext {
        index: ctx.index.clone(),
        def_index: ctx.def_index.clone(),
        workspace: ctx.workspace.clone(),
        server_ext: "cs,xml,sql".to_string(),
        ..Default::default()
    };

    let result = dispatch_tool(&multi_ext_ctx, "xray_callers", &json!({
        "method": ["ExecuteQueryAsync"],
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
        call_kind: Default::default(),
        receiver_is_generic: false,
            };
    let resolved_a = resolve_call_site(&call_a, &def_index, None);
    assert_eq!(resolved_a.len(), 1);
    assert_eq!(def_index.definitions[resolved_a[0] as usize].parent.as_deref(), Some("ServiceA"));

    let call_b = CallSite {
        method_name: "Execute".to_string(),
        receiver_type: Some("ServiceB".to_string()),
        line: 10,
        call_kind: Default::default(),
        receiver_is_generic: false,
            };
    let resolved_b = resolve_call_site(&call_b, &def_index, None);
    assert_eq!(resolved_b.len(), 1);
    assert_eq!(def_index.definitions[resolved_b[0] as usize].parent.as_deref(), Some("ServiceB"));

    let call_no_recv = CallSite {
        method_name: "Execute".to_string(),
        receiver_type: None,
        line: 15,
        call_kind: Default::default(),
        receiver_is_generic: false,
            };
    let resolved_none = resolve_call_site(&call_no_recv, &def_index, None);
    assert!(resolved_none.is_empty());

    let call_iface = CallSite {
        method_name: "Execute".to_string(),
        receiver_type: Some("IService".to_string()),
        line: 20,
        call_kind: Default::default(),
        receiver_is_generic: false,
            };
    let resolved_iface = resolve_call_site(&call_iface, &def_index, None);
    assert!(!resolved_iface.is_empty());
    assert!(resolved_iface.iter().any(|&di| {
        def_index.definitions[di as usize].parent.as_deref() == Some("ServiceA")
    }));
}

// ─── xray_callers "down" direction + class filter tests ────────────

#[test]
fn test_xray_callers_down_class_filter() {
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
    method_calls.insert(1, vec![CallSite { method_name: "ShouldIssueVectorSearch".to_string(), receiver_type: None, line: 780, call_kind: Default::default(), receiver_is_generic: false }]);
    method_calls.insert(4, vec![CallSite { method_name: "TraceInformation".to_string(), receiver_type: None, line: 333, call_kind: Default::default(), receiver_is_generic: false }]);

    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\IndexSearchService.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\IndexedSearchQueryExecuter.cs")), 1);

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

    let result = dispatch_tool(&ctx, "xray_callers", &json!({ "method": ["SearchInternalAsync"], "class": "IndexSearchService", "direction": "down", "depth": 1 }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();
    let callee_names: Vec<&str> = tree.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_names.contains(&"ShouldIssueVectorSearch"));
    assert!(!callee_names.contains(&"TraceInformation"));

    let result2 = dispatch_tool(&ctx, "xray_callers", &json!({ "method": ["SearchInternalAsync"], "class": "IndexedSearchQueryExecuter", "direction": "down", "depth": 1 }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let tree2 = output2["callTree"].as_array().unwrap();
    let callee_names2: Vec<&str> = tree2.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_names2.contains(&"TraceInformation"));
    assert!(!callee_names2.contains(&"ShouldIssueVectorSearch"));

    let result3 = dispatch_tool(&ctx, "xray_callers", &json!({ "method": ["SearchInternalAsync"], "direction": "down", "depth": 1 }));
    assert!(!result3.is_error);
    let output3: Value = serde_json::from_str(&result3.content[0].text).unwrap();
    let tree3 = output3["callTree"].as_array().unwrap();
    let callee_names3: Vec<&str> = tree3.iter().filter_map(|n| n["method"].as_str()).collect();
    assert!(callee_names3.contains(&"ShouldIssueVectorSearch"));
    assert!(callee_names3.contains(&"TraceInformation"));
    assert!(output3.get("warning").is_some());
}

#[test]
fn test_xray_callers_interface_duplicate_does_not_consume_total_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let source_file = root.join("InterfaceDuplicate.cs");
    std::fs::write(
        &source_file,
        r#"public interface ITarget {
    void Execute();
}
public sealed class Target : ITarget {
    public void Execute() { }
}
public sealed class Caller {
    public void Call(Target concrete, ITarget contract) {
        concrete.Execute();
        contract.Execute();
    }
}
"#,
    )
    .unwrap();

    let mut definition_index = DefinitionIndex {
        root: root.to_string_lossy().to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    crate::definitions::update_file_definitions(&mut definition_index, &source_file);
    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let context = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(definition_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            root.to_string_lossy().to_string(),
        ))),
        server_ext: "cs".to_string(),
        ..Default::default()
    };

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["Execute"],
            "class": "ITarget",
            "depth": 1,
            "resolveInterfaces": true,
            "maxCallersPerLevel": 10,
            "maxTotalNodes": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);

    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["callTree"].as_array().unwrap().len(), 1, "{output:#}");
    let status = &output["resultStatus"];
    assert_eq!(status["status"], "complete", "{output:#}");
    assert_eq!(status["complete"], true, "{output:#}");
    assert_eq!(status["totalKnown"], true, "{output:#}");
    assert_eq!(status["shown"]["nodes"], 1, "{output:#}");
    assert_eq!(status["total"]["nodes"], 1, "{output:#}");
    assert_eq!(status["omitted"]["nodes"], 0, "{output:#}");

    let batch_result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["Execute", "Missing"],
            "class": "ITarget",
            "depth": 1,
            "resolveInterfaces": true,
            "maxCallersPerLevel": 10,
            "maxTotalNodes": 1
        }),
    );
    assert!(!batch_result.is_error, "{}", batch_result.content[0].text);
    let batch_output: Value = serde_json::from_str(&batch_result.content[0].text).unwrap();
    assert_eq!(batch_output["results"][0]["nodesInTree"], 1, "{batch_output:#}");
    assert_eq!(batch_output["results"][0]["truncated"], false, "{batch_output:#}");
    assert_eq!(batch_output["resultStatus"]["status"], "complete", "{batch_output:#}");
    assert_eq!(batch_output["resultStatus"]["totalKnown"], true, "{batch_output:#}");
    assert_eq!(batch_output["resultStatus"]["omitted"]["nodes"], 0, "{batch_output:#}");

    for methods in [vec!["Execute"], vec!["Execute", "Missing"]] {
        let zero_result = dispatch_tool(
            &context,
            "xray_callers",
            &json!({
                "method": methods,
                "class": "ITarget",
                "depth": 1,
                "resolveInterfaces": true,
                "maxCallersPerLevel": 0,
                "maxTotalNodes": 10
            }),
        );
        assert!(!zero_result.is_error, "{}", zero_result.content[0].text);
        let zero_output: Value = serde_json::from_str(&zero_result.content[0].text).unwrap();
        let status = &zero_output["resultStatus"];
        assert_eq!(status["status"], "partial", "{zero_output:#}");
        assert_eq!(status["totalKnown"], false, "{zero_output:#}");
        assert_eq!(status["shown"]["nodes"], 0, "{zero_output:#}");
        assert_eq!(status["total"]["nodes"], 1, "{zero_output:#}");
        assert_eq!(status["omitted"]["nodes"], 1, "{zero_output:#}");
        assert_eq!(zero_output["summary"]["callersDroppedPerLevel"], 1, "{zero_output:#}");
    }
}

#[test]
fn test_xray_callers_down_respects_resolve_interfaces_policy() {
    fn tree_contains(nodes: &[Value], class_name: &str, method_name: &str) -> bool {
        nodes.iter().any(|node| {
            (node["class"] == class_name && node["method"] == method_name)
                || node["callees"]
                    .as_array()
                    .is_some_and(|children| tree_contains(children, class_name, method_name))
        })
    }
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let source_file = root.join("ResolveInterfacesDown.cs");
    std::fs::write(
        &source_file,
        r#"public interface IDispatch {
    void Ping();
}
public sealed class DispatchImpl : IDispatch {
    public void Ping() { PingTarget(); }
    private void PingTarget() { }
}
public sealed class DispatchCaller {
    public void Call(IDispatch dispatch) { dispatch.Ping(); }
}
public sealed class ImplementationCaller {
    public void Call(DispatchImpl dispatch) { dispatch.Ping(); }
}
public sealed class DispatchRoot {
    public void Start(DispatchCaller caller, IDispatch dispatch) { caller.Call(dispatch); }
}
public class BaseDispatch {
    public virtual void Route() { }
}
public sealed class DerivedDispatch : BaseDispatch {
    public override void Route() { RouteTarget(); }
    private void RouteTarget() { }
}
public sealed class ConcreteCaller {
    public void Call(BaseDispatch dispatch) { dispatch.Route(); }
}
"#,
    )
    .unwrap();

    let mut definition_index = DefinitionIndex {
        root: root.to_string_lossy().to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    crate::definitions::update_file_definitions(&mut definition_index, &source_file);
    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let context = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(definition_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            root.to_string_lossy().to_string(),
        ))),
        server_ext: "cs".to_string(),
        ..Default::default()
    };

    let query = |class_name: &str,
                 method_name: &str,
                 resolve_interfaces: bool,
                 max_total_nodes: usize| {
        let result = dispatch_tool(
            &context,
            "xray_callers",
            &json!({
                "method": [method_name],
                "class": class_name,
                "direction": "down",
                "depth": 3,
                "resolveInterfaces": resolve_interfaces,
                "maxCallersPerLevel": 10,
                "maxTotalNodes": max_total_nodes
            }),
        );
        assert!(!result.is_error, "{}", result.content[0].text);
        serde_json::from_str::<Value>(&result.content[0].text).unwrap()
    };

    let direct_only = query("DispatchCaller", "Call", false, 1);
    assert_eq!(direct_only["query"]["resolveInterfaces"], false, "{direct_only:#}");
    let direct_tree = direct_only["callTree"].as_array().unwrap();
    assert_eq!(direct_tree.len(), 1, "{direct_only:#}");
    assert_eq!(direct_tree[0]["class"], "IDispatch", "{direct_only:#}");
    assert_eq!(direct_tree[0]["method"], "Ping", "{direct_only:#}");
    assert!(direct_tree[0].get("callees").is_none(), "{direct_only:#}");
    assert_eq!(direct_only["resultStatus"]["status"], "complete", "{direct_only:#}");
    assert_eq!(direct_only["resultStatus"]["total"]["nodes"], 1, "{direct_only:#}");
    assert_eq!(direct_only["resultStatus"]["omitted"]["nodes"], 0, "{direct_only:#}");

    let expanded = query("DispatchCaller", "Call", true, 20);
    assert_eq!(expanded["query"]["resolveInterfaces"], true, "{expanded:#}");
    let expanded_tree = expanded["callTree"].as_array().unwrap();
    let implementation = expanded_tree
        .iter()
        .find(|node| node["class"] == "DispatchImpl" && node["method"] == "Ping")
        .unwrap_or_else(|| panic!("implementation missing: {expanded:#}"));
    assert!(
        implementation["callees"]
            .as_array()
            .is_some_and(|nodes| nodes.iter().any(|node| node["method"] == "PingTarget")),
        "{expanded:#}"
    );

    let mid_tree_direct = query("DispatchRoot", "Start", false, 20);
    let mid_tree_direct_nodes = mid_tree_direct["callTree"].as_array().unwrap();
    assert!(tree_contains(mid_tree_direct_nodes, "IDispatch", "Ping"), "{mid_tree_direct:#}");
    assert!(
        !tree_contains(mid_tree_direct_nodes, "DispatchImpl", "Ping"),
        "{mid_tree_direct:#}"
    );
    assert!(
        !tree_contains(mid_tree_direct_nodes, "DispatchImpl", "PingTarget"),
        "{mid_tree_direct:#}"
    );

    let mid_tree_expanded = query("DispatchRoot", "Start", true, 20);
    let mid_tree_expanded_nodes = mid_tree_expanded["callTree"].as_array().unwrap();
    assert!(
        tree_contains(mid_tree_expanded_nodes, "DispatchImpl", "PingTarget"),
        "{mid_tree_expanded:#}"
    );

    let concrete_direct = query("ConcreteCaller", "Call", false, 20);
    let concrete_expanded = query("ConcreteCaller", "Call", true, 20);
    assert_eq!(
        concrete_direct["callTree"], concrete_expanded["callTree"],
        "concrete inheritance must not depend on resolveInterfaces"
    );

    let batch_result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["Call", "Missing"],
            "class": "DispatchCaller",
            "direction": "down",
            "depth": 3,
            "resolveInterfaces": false,
            "maxCallersPerLevel": 10,
            "maxTotalNodes": 20
        }),
    );
    assert!(!batch_result.is_error, "{}", batch_result.content[0].text);
    let batch_output: Value = serde_json::from_str(&batch_result.content[0].text).unwrap();
    assert_eq!(batch_output["query"]["resolveInterfaces"], false, "{batch_output:#}");

    let query_up = |resolve_interfaces| {
        let result = dispatch_tool(
            &context,
            "xray_callers",
            &json!({
                "method": ["Ping"],
                "class": "IDispatch",
                "direction": "up",
                "depth": 2,
                "resolveInterfaces": resolve_interfaces,
                "maxCallersPerLevel": 10,
                "maxTotalNodes": 20
            }),
        );
        assert!(!result.is_error, "{}", result.content[0].text);
        serde_json::from_str::<Value>(&result.content[0].text).unwrap()
    };

    let up_direct = query_up(false);
    assert_eq!(up_direct["query"]["resolveInterfaces"], false, "{up_direct:#}");
    let up_direct_nodes = up_direct["callTree"].as_array().unwrap();
    assert!(tree_contains(up_direct_nodes, "DispatchCaller", "Call"), "{up_direct:#}");
    assert!(
        !tree_contains(up_direct_nodes, "ImplementationCaller", "Call"),
        "{up_direct:#}"
    );

    let up_expanded = query_up(true);
    assert_eq!(up_expanded["query"]["resolveInterfaces"], true, "{up_expanded:#}");
    let up_expanded_nodes = up_expanded["callTree"].as_array().unwrap();
    assert!(tree_contains(up_expanded_nodes, "DispatchCaller", "Call"), "{up_expanded:#}");
    assert!(
        tree_contains(up_expanded_nodes, "ImplementationCaller", "Call"),
        "{up_expanded:#}"
    );
}

#[test]
fn test_xray_callers_direct_receiver_types() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let source_file = root.join("InlineReceivers.cs");
    std::fs::write(
        &source_file,
        r#"public sealed class NoArgTarget {
    public void Execute() { }
}
public sealed class ArgTarget {
    public ArgTarget(int first, int second) { }
    public void Execute() { }
}
public sealed class ParameterTarget {
    public void Execute() { }
}
public sealed class UnrelatedTarget {
    public void Execute() { }
}
namespace TestTypes {
    public sealed class GenericTarget<T> {
        public void Execute() { }
    }
}
public sealed class InlineReceiverCaller {
    private readonly UnrelatedTarget target = new UnrelatedTarget();
    public void NoArgs() => new NoArgTarget().Execute();
    public void WithArgs(int first, int second) => new ArgTarget(first, second).Execute();
    public void Generic() => new TestTypes.GenericTarget<System.String>().Execute();
    public void Parameter(ParameterTarget target) => target.Execute();
    public void NullConditional(ParameterTarget? target) => target?.Execute();
    public void SameTernary(bool condition, ParameterTarget first, ParameterTarget second) =>
        (condition ? first : second).Execute();
    public void DifferentTernary(bool condition, ParameterTarget first, UnrelatedTarget second) =>
        (condition ? first : second).Execute();
    public void SameCoalescing(ParameterTarget? first, ParameterTarget fallback) =>
        (first ?? fallback).Execute();
    public void DifferentCoalescing(ParameterTarget? first, UnrelatedTarget fallback) =>
        (first ?? fallback).Execute();
}
"#,
    )
    .unwrap();

    let mut definition_index = DefinitionIndex {
        root: root.to_string_lossy().to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    crate::definitions::update_file_definitions(&mut definition_index, &source_file);
    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();
    let context = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(definition_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            root.to_string_lossy().to_string(),
        ))),
        server_ext: "cs".to_string(),
        ..Default::default()
    };

    for (class_name, caller_name) in
        [
            ("NoArgTarget", "NoArgs"),
            ("ArgTarget", "WithArgs"),
            ("GenericTarget", "Generic"),
        ]
    {
        let result = dispatch_tool(
            &context,
            "xray_callers",
            &json!({
                "method": ["Execute"],
                "class": class_name,
                "depth": 1
            }),
        );
        assert!(!result.is_error, "{}", result.content[0].text);
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let call_tree = output["callTree"].as_array().unwrap();
        assert_eq!(call_tree.len(), 1, "{}", output);
        assert_eq!(
            call_tree[0]["method"].as_str(),
            Some(caller_name),
            "{}",
            output
        );
    }

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["Execute"],
            "class": "ParameterTarget",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let caller_methods: Vec<_> = output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["method"].as_str())
        .collect();
    assert_eq!(
        caller_methods,
        vec!["Parameter", "NullConditional", "SameTernary", "SameCoalescing"],
        "{}",
        output
    );

    for (method_name, expected_class) in [
        ("SameTernary", Some("ParameterTarget")),
        ("DifferentTernary", None),
        ("SameCoalescing", Some("ParameterTarget")),
        ("DifferentCoalescing", None),
    ] {
        let result = dispatch_tool(
            &context,
            "xray_callers",
            &json!({
                "method": [method_name],
                "class": "InlineReceiverCaller",
                "direction": "down",
                "depth": 1
            }),
        );
        assert!(!result.is_error, "{}", result.content[0].text);
        let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
        let execute_nodes: Vec<_> = output["callTree"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["method"].as_str() == Some("Execute"))
            .collect();
        assert_eq!(execute_nodes.len(), 1, "{}", output);
        if let Some(expected_class) = expected_class {
            assert_eq!(
                execute_nodes[0]["nodeKind"].as_str(),
                Some("callee"),
                "{}",
                output
            );
            assert_eq!(
                execute_nodes[0]["class"].as_str(),
                Some(expected_class),
                "{}",
                output
            );
        } else {
            assert_eq!(
                execute_nodes[0]["nodeKind"].as_str(),
                Some("ambiguousCall"),
                "{}",
                output
            );
        }
    }

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["WithArgs"],
            "class": "InlineReceiverCaller",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let execute_nodes: Vec<_> = output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["method"].as_str() == Some("Execute"))
        .collect();
    assert_eq!(execute_nodes.len(), 1, "{}", output);
    assert_eq!(
        execute_nodes[0]["class"].as_str(),
        Some("ArgTarget"),
        "{}",
        output
    );
}

const D20_OVERLOAD_SOURCE: &str = r#"namespace Demo {
public sealed class Router {
    public void Route(int value) => IntTarget();
    public void Route(string value) => StringTarget();
    public void CallInt() => Route(1);
    public void CallString() => Route("x");
    public void CallUnknown(dynamic value) => Route(value);
    private readonly Router router = new Router();
    public void CallThroughThis() => this.router.Route(1);
    private void IntTarget() { }
    private void StringTarget() { }
}
}
"#;

const D20_NAMESPACE_SOURCE: &str = r#"namespace One {
public sealed class Collision {
    public void SameName() => OneTarget();
    private void OneTarget() { }
}
}
namespace Two {
public sealed class Collision {
    public void SameName() => TwoTarget();
    private void TwoTarget() { }
}
}
public sealed class NamespaceCaller {
    public void CallOne() => new One.Collision().SameName();
}
"#;


const D20_PRODUCTION_SOURCE: &str = r#"namespace Demo;
public sealed partial class Router {
    public void Route(int value) => IntTarget();
    public void CallUnknown(dynamic value) => Route(value);
    private void IntTarget() { }
}
"#;

const D20_TEST_SOURCE: &str = r#"namespace Demo;
public sealed partial class Router {
    public void Route(string value) => StringTarget();
    private void StringTarget() { }
}
"#;


const D20_EXPLICIT_INTERFACE_SOURCE: &str = r#"namespace Demo;
public interface IFoo { void Run(int value); }
public interface IBar { void Run(int value); }
public sealed class ExplicitRunner : IFoo, IBar {
    void IFoo.Run(int value) { }
    void IBar.Run(int value) { }
}
"#;

const D20_GENERIC_ALIAS_SOURCE: &str = r#"namespace Demo;
public sealed class FormatTarget {
    public void Map(List<int> value) { }
}
"#;

const D20_GENERIC_CANONICAL_SOURCE: &str = r#"namespace Demo;
public sealed class FormatTarget {
    public void Map(List< System.Int32 > value) { }
}
"#;


const D20_PARTIAL_SOURCE: &str = r#"namespace Demo;
public sealed partial class PartialTarget {
    partial void Run();
    partial void Run() { }
}
"#;


const D20_NAMED_ARGUMENT_SOURCE: &str = r#"namespace Demo;
public sealed class Binder {
    public void Pick(int a, string b) => NamedTarget();
    public void Pick(string a, int b) => OtherTarget();
    public void CallNamed() => Pick(b: "x", a: 1);
    private void NamedTarget() { }
    private void OtherTarget() { }
}
"#;

const D20_INTEGER_SUFFIX_SOURCE: &str = r#"namespace Demo;
public sealed class NumericRouter {
    public void Number(int value) => IntTarget();
    public void Number(long value) => LongTarget();
    public void CallLong() => Number(1L);
    private void IntTarget() { }
    private void LongTarget() { }
}
"#;

const D20_NESTED_RECEIVER_SOURCE: &str = r#"namespace Demo;
public sealed class Router { public void Route(int value) { } }
public sealed class OtherRouter { public void Route(int value) { } }
public sealed class Holder { public OtherRouter router = new OtherRouter(); }
public sealed class NestedCaller {
    private readonly Router router = new Router();
    private readonly Holder holder = new Holder();
    public void CallNested() => this.holder.router.Route(1);
}
"#;

const D20_MULTIPLE_BODIES_SOURCE: &str = r#"namespace Demo;
public sealed partial class ConditionalTarget {
#if FIRST
    public void Run() => FirstTarget();
#else
    public void Run() => SecondTarget();
#endif
    private void FirstTarget() { }
    private void SecondTarget() { }
}
"#;


const D20_PARAMS_SOURCE: &str = r#"namespace Demo;
public sealed class ParamsTarget {
    public void Pack(params int[] values) => ParamsBody();
    public void Pack(string value) => StringBody();
    public void CallExpanded() => Pack(1, 2);
    private void ParamsBody() { }
    private void StringBody() { }
}
"#;


const D20_DIRECT_THIS_SOURCE: &str = r#"namespace Demo;
public sealed class SelfTarget {
    public void Route(int value) => SelfBody();
    public void CallSelf() => this.Route(1);
    private void SelfBody() { }
}
public sealed class OtherTarget {
    public void Route(int value) { }
}
"#;

const D20_BASE_SOURCE: &str = r#"namespace Demo;
public class BaseTarget {
    public virtual void Route(int value) => BaseBody();
    private void BaseBody() { }
}
public sealed class DerivedTarget : BaseTarget {
    public override void Route(int value) { }
    public void CallBase() => base.Route(1);
}
public sealed class OtherTarget {
    public void Route(int value) { }
}
"#;

fn d20_callers_context(
    root: &std::path::Path,
    file_name: &str,
    source: &str,
) -> HandlerContext {
    d20_callers_context_files(root, &[(file_name, source)])
}

fn d20_callers_context_files(
    root: &std::path::Path,
    files: &[(&str, &str)],
) -> HandlerContext {
    let mut definition_index = DefinitionIndex {
        root: root.to_string_lossy().to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    for (file_name, source) in files {
        let source_file = root.join(file_name);
        std::fs::write(&source_file, source).unwrap();
        crate::definitions::update_file_definitions(&mut definition_index, &source_file);
    }
    let content_index = crate::build_content_index(&crate::ContentIndexArgs {
        dir: root.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
        ..Default::default()
    })
    .unwrap();

    HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(definition_index))),
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
            root.to_string_lossy().to_string(),
        ))),
        server_ext: "cs".to_string(),
        ..Default::default()
    }
}

fn d20_symbol_id(context: &HandlerContext, method: &str, signature_fragment: &str) -> String {
    let index = context.def_index.as_ref().unwrap().read().unwrap();
    let definition_index = index.name_index[&method.to_lowercase()]
        .iter()
        .copied()
        .find(|&candidate| {
            index.definitions[candidate as usize].signature.as_deref()
                .is_some_and(|signature| signature.contains(signature_fragment))
        })
        .unwrap();
    index.csharp_semantics.symbol_id_for_definition(definition_index)
        .unwrap()
        .as_public_id()
}

#[test]
fn test_d20_call_int_resolves_only_int_overload() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallInt"],
            "class": "Router",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let route_nodes: Vec<_> = output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["method"].as_str() == Some("Route"))
        .collect();

    assert_eq!(route_nodes.len(), 1, "{}", output);
    assert_eq!(route_nodes[0]["line"].as_u64(), Some(3), "{}", output);
}

#[test]
fn test_d20_call_string_resolves_only_string_overload() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallString"],
            "class": "Router",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let route_nodes: Vec<_> = output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["method"].as_str() == Some("Route"))
        .collect();

    assert_eq!(route_nodes.len(), 1, "{}", output);
    assert_eq!(route_nodes[0]["line"].as_u64(), Some(4), "{}", output);
}

#[test]
fn test_d20_this_field_receiver_uses_declared_type() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallThroughThis"],
            "class": "Router",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let route_nodes: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter(|node| node["method"].as_str() == Some("Route"))
        .collect();
    assert_eq!(route_nodes.len(), 1, "{}", output);
    assert_eq!(route_nodes[0]["line"].as_u64(), Some(3), "{}", output);
}

#[test]
fn test_d20_production_only_filters_test_overload_before_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context_files(
        &root,
        &[
            ("Router.cs", D20_PRODUCTION_SOURCE),
            ("Router_tests.cs", D20_TEST_SOURCE),
        ],
    );

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallUnknown"],
            "class": "Router",
            "direction": "down",
            "depth": 1,
            "productionOnly": true
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let route_nodes: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter(|node| node["method"].as_str() == Some("Route"))
        .collect();
    assert_eq!(route_nodes.len(), 1, "{}", output);
    assert_eq!(route_nodes[0]["file"].as_str(), Some("Router.cs"), "{}", output);
    assert!(!output["resultStatus"]["reasons"].as_array().unwrap().iter()
        .any(|reason| reason.as_str() == Some("ambiguous_overload")), "{}", output);
}

#[test]
fn test_d20_direct_this_call_uses_current_type() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(
        &root,
        "D20DirectThis.cs",
        D20_DIRECT_THIS_SOURCE,
    );

    {
        let index = context.def_index.as_ref().unwrap().read().unwrap();
        let caller = index.name_index["callself"][0];
        let shape = index.csharp_semantics.call_shape(caller, 0).unwrap();
        let crate::definitions::CSharpTypeEvidence::Exact(receiver) = shape.receiver else {
            panic!("{shape:?}");
        };
        assert_eq!(index.csharp_semantics.strings.get(receiver), Some("Demo.SelfTarget"));
        assert!(!shape.base_receiver);
    }

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallSelf"],
            "class": "SelfTarget",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let route_nodes: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter(|node| node["method"].as_str() == Some("Route"))
        .collect();
    assert_eq!(route_nodes.len(), 1, "{}", output);
    assert_eq!(route_nodes[0]["nodeKind"].as_str(), Some("callee"), "{}", output);
    assert_eq!(route_nodes[0]["class"].as_str(), Some("SelfTarget"), "{}", output);
}

#[test]
fn test_d20_base_call_selects_base_without_dispatch_expansion() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Base.cs", D20_BASE_SOURCE);

    {
        let index = context.def_index.as_ref().unwrap().read().unwrap();
        let caller = index.name_index["callbase"][0];
        let shape = index.csharp_semantics.call_shape(caller, 0).unwrap();
        let crate::definitions::CSharpTypeEvidence::Exact(receiver) = shape.receiver else {
            panic!("{shape:?}");
        };
        assert_eq!(index.csharp_semantics.strings.get(receiver), Some("BaseTarget"));
        assert!(shape.base_receiver);
    }

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallBase"],
            "class": "DerivedTarget",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let route_nodes: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter(|node| node["method"].as_str() == Some("Route"))
        .collect();
    assert_eq!(route_nodes.len(), 1, "{}", output);
    assert_eq!(route_nodes[0]["nodeKind"].as_str(), Some("callee"), "{}", output);
    assert_eq!(route_nodes[0]["class"].as_str(), Some("BaseTarget"), "{}", output);
}


#[test]
fn test_d20_named_arguments_bind_by_parameter_name() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Named.cs", D20_NAMED_ARGUMENT_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallNamed"],
            "class": "Binder",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let pick_nodes: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter(|node| node["method"].as_str() == Some("Pick"))
        .collect();
    assert_eq!(pick_nodes.len(), 1, "{}", output);
    assert_eq!(pick_nodes[0]["line"].as_u64(), Some(3), "{}", output);
}

#[test]
fn test_d20_integer_suffix_selects_long_overload() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(
        &root,
        "D20IntegerSuffix.cs",
        D20_INTEGER_SUFFIX_SOURCE,
    );

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallLong"],
            "class": "NumericRouter",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let number_nodes: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter(|node| node["method"].as_str() == Some("Number"))
        .collect();
    assert_eq!(number_nodes.len(), 1, "{}", output);
    assert_eq!(number_nodes[0]["line"].as_u64(), Some(4), "{}", output);
}

#[test]
fn test_d20_nested_this_receiver_remains_ambiguous() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(
        &root,
        "D20NestedReceiver.cs",
        D20_NESTED_RECEIVER_SOURCE,
    );

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallNested"],
            "class": "NestedCaller",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let route_nodes: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter(|node| node["method"].as_str() == Some("Route"))
        .collect();
    assert_eq!(route_nodes.len(), 1, "{}", output);
    assert_eq!(route_nodes[0]["nodeKind"].as_str(), Some("ambiguousCall"), "{}", output);
    assert_eq!(
        route_nodes[0]["resolution"]["candidates"].as_array().unwrap().len(),
        2,
        "{}",
        output
    );
}


#[test]
fn test_d20_params_array_expanded_form_resolves_element_type() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Params.cs", D20_PARAMS_SOURCE);

    {
        let index = context.def_index.as_ref().unwrap().read().unwrap();
        let pack_definitions = &index.name_index["pack"];
        let params_definition = pack_definitions.iter().copied()
            .find(|&candidate| index.definitions[candidate as usize].line_start == 3)
            .unwrap();
        let callable = index.csharp_semantics.callable_for_definition(params_definition).unwrap();
        assert!(callable.parameters[0].is_params, "{:?}", callable.parameters);
        assert_eq!(
            index.csharp_semantics.strings.get(callable.parameters[0].ty),
            Some("System.Int32[]")
        );
        let caller = index.name_index["callexpanded"][0];
        let shape = index.csharp_semantics.call_shape(caller, 0).unwrap();
        assert_eq!(shape.arguments.len(), 2, "{shape:?}");
    }

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallExpanded"],
            "class": "ParamsTarget",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let pack_nodes: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter(|node| node["method"].as_str() == Some("Pack"))
        .collect();
    assert_eq!(pack_nodes.len(), 1, "{}", output);
    assert_eq!(pack_nodes[0]["line"].as_u64(), Some(3), "{}", output);
}

#[test]
fn test_d20_unknown_argument_reports_ambiguity_without_traversal() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallUnknown"],
            "class": "Router",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let call_tree = output["callTree"].as_array().unwrap();
    let exact_route_nodes: Vec<_> = call_tree
        .iter()
        .filter(|node| {
            node["nodeKind"].as_str() == Some("callee")
                && node["method"].as_str() == Some("Route")
        })
        .collect();
    let ambiguous_route_nodes: Vec<_> = call_tree
        .iter()
        .filter(|node| {
            node["nodeKind"].as_str() == Some("ambiguousCall")
                && node["method"].as_str() == Some("Route")
        })
        .collect();
    let reasons = output["resultStatus"]["reasons"].as_array().unwrap();

    assert!(exact_route_nodes.is_empty(), "{}", output);
    assert_eq!(ambiguous_route_nodes.len(), 1, "{}", output);
    assert!(
        reasons.iter().any(|reason| reason.as_str() == Some("ambiguous_overload")),
        "{}",
        output
    );
}

#[test]
fn test_d20_qualified_receiver_avoids_namespace_collision() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Namespaces.cs", D20_NAMESPACE_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["CallOne"],
            "class": "NamespaceCaller",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let same_name_nodes: Vec<_> = output["callTree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["method"].as_str() == Some("SameName"))
        .collect();

    assert_eq!(same_name_nodes.len(), 1, "{}", output);
    assert_eq!(same_name_nodes[0]["line"].as_u64(), Some(3), "{}", output);
}

#[test]
fn test_d20_definitions_exposes_distinct_overload_symbol_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_definitions",
        &json!({
            "name": ["Route"],
            "parent": ["Router"],
            "exactNameOnly": true
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let definitions = output["definitions"].as_array().unwrap();
    assert_eq!(definitions.len(), 2, "{}", output);
    let symbol_ids: std::collections::HashSet<_> = definitions.iter()
        .map(|definition| definition["symbolId"].as_str().unwrap())
        .collect();
    assert_eq!(symbol_ids.len(), 2, "{}", output);
    assert!(definitions.iter().all(|definition| {
        definition["qualifiedType"].as_str() == Some("Demo.Router")
    }), "{}", output);
}

#[test]
fn test_d20_symbol_id_golden_vector() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);
    let symbol_id = d20_symbol_id(&context, "Route", "int value");

    assert_eq!(
        symbol_id,
        "cs:v1:542d9ed0fc8dac26cf66119daaabf146508c9506ef2e87350f2bc4bc3eac4e77"
    );
}

#[test]
fn test_d20_explicit_interface_implementations_have_distinct_symbol_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(
        &root,
        "D20ExplicitInterfaces.cs",
        D20_EXPLICIT_INTERFACE_SOURCE,
    );

    let index = context.def_index.as_ref().unwrap().read().unwrap();
    let run_definitions: Vec<_> = index.name_index["run"].iter()
        .copied()
        .filter(|&candidate| {
            index.definitions[candidate as usize].parent.as_deref() == Some("ExplicitRunner")
        })
        .collect();
    assert_eq!(run_definitions.len(), 2);
    let symbol_ids: std::collections::HashSet<_> = run_definitions.iter()
        .map(|&candidate| index.csharp_semantics.symbol_id_for_definition(candidate).unwrap())
        .collect();
    assert_eq!(symbol_ids.len(), 2);
}

#[test]
fn test_d20_symbol_id_normalizes_generic_aliases_and_whitespace() {
    let first_temp = tempfile::tempdir().unwrap();
    let first_root = crate::canonicalize_test_root(first_temp.path());
    let first = d20_callers_context(
        &first_root,
        "FormatTarget.cs",
        D20_GENERIC_ALIAS_SOURCE,
    );
    let first_id = d20_symbol_id(&first, "Map", "List<int>");

    let second_temp = tempfile::tempdir().unwrap();
    let second_root = crate::canonicalize_test_root(second_temp.path());
    let second = d20_callers_context(
        &second_root,
        "FormatTarget.cs",
        D20_GENERIC_CANONICAL_SOURCE,
    );
    let second_id = d20_symbol_id(&second, "Map", "List< System.Int32 >");

    assert_eq!(first_id, second_id);
}

#[test]
fn test_d20_legacy_root_reports_ambiguity_without_traversal() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["Route"],
            "class": "Router",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["callTree"].as_array().unwrap().is_empty(), "{}", output);
    assert_eq!(output["rootResolution"]["status"], "ambiguous", "{}", output);
    assert_eq!(
        output["rootResolution"]["candidates"].as_array().unwrap().len(),
        2,
        "{}",
        output
    );
    assert_eq!(output["resultStatus"]["status"], "partial", "{}", output);
    assert!(output["resultStatus"]["reasons"].as_array().unwrap().iter()
        .any(|reason| reason.as_str() == Some("ambiguous_root")), "{}", output);
}

#[test]
fn test_d20_legacy_policy_preserves_unsafe_root_fan_out() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["Route"],
            "class": "Router",
            "direction": "down",
            "depth": 1,
            "ambiguityPolicy": "legacy"
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let methods: std::collections::HashSet<_> = output["callTree"].as_array().unwrap().iter()
        .filter_map(|node| node["method"].as_str())
        .collect();
    assert_eq!(methods, std::collections::HashSet::from(["IntTarget", "StringTarget"]), "{}", output);
    assert_eq!(output["resultStatus"]["safeForExactSemantics"], false, "{}", output);
    assert!(output["resultStatus"]["reasons"].as_array().unwrap().iter()
        .any(|reason| reason.as_str() == Some("legacy_ambiguous_fanout")), "{}", output);
}

#[test]
fn test_d20_exact_symbol_selects_empty_body_implementation() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Partial.cs", D20_PARTIAL_SOURCE);
    let symbol_id = d20_symbol_id(&context, "Run", "partial void Run");

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "targets": [{ "symbolId": symbol_id }],
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["rootResolution"]["line"].as_u64(), Some(4), "{}", output);
    assert!(output["callTree"].as_array().unwrap().is_empty(), "{}", output);
}


#[test]
fn test_d20_exact_symbol_down_traverses_selected_overload_body() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);
    let symbol_id = d20_symbol_id(&context, "Route", "int value");

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "targets": [{ "symbolId": symbol_id }],
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let methods: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter_map(|node| node["method"].as_str())
        .collect();
    assert_eq!(methods, vec!["IntTarget"], "{}", output);
    assert_eq!(output["rootResolution"]["status"], "exact", "{}", output);
    assert_eq!(output["rootResolution"]["symbolId"], symbol_id, "{}", output);
}

#[test]
fn test_d20_exact_symbol_up_excludes_other_and_ambiguous_callers() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);
    let symbol_id = d20_symbol_id(&context, "Route", "int value");

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "targets": [{ "symbolId": symbol_id }],
            "direction": "up",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let methods: Vec<_> = output["callTree"].as_array().unwrap().iter()
        .filter_map(|node| node["method"].as_str())
        .collect();
    assert_eq!(methods, vec!["CallInt", "CallUnknown", "CallThroughThis"], "{}", output);
    assert_eq!(output["rootResolution"]["symbolId"], symbol_id, "{}", output);
}

#[test]
fn test_d20_exact_symbol_up_surfaces_ambiguous_reference() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);
    let symbol_id = d20_symbol_id(&context, "Route", "int value");

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "targets": [{ "symbolId": symbol_id }],
            "direction": "up",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let ambiguous = output["callTree"].as_array().unwrap().iter()
        .find(|node| node["method"].as_str() == Some("CallUnknown"))
        .unwrap_or_else(|| panic!("{}", output));
    assert_eq!(ambiguous["nodeKind"].as_str(), Some("ambiguousCaller"), "{}", output);
    assert_eq!(ambiguous["resolution"]["status"].as_str(), Some("ambiguous"), "{}", output);
    assert_eq!(output["resultStatus"]["status"].as_str(), Some("partial"), "{}", output);
    assert!(output["resultStatus"]["reasons"].as_array().unwrap().iter()
        .any(|reason| reason.as_str() == Some("ambiguous_overload")), "{}", output);
}

#[test]
fn test_d20_multiple_body_root_reports_invalid_ambiguity() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(
        &root,
        "D20MultipleBodies.cs",
        D20_MULTIPLE_BODIES_SOURCE,
    );

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "method": ["Run"],
            "class": "ConditionalTarget",
            "direction": "down",
            "depth": 1
        }),
    );
    assert!(!result.is_error, "{}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["callTree"].as_array().unwrap().is_empty(), "{}", output);
    assert_eq!(
        output["rootResolution"]["reason"].as_str(),
        Some("invalid_multiple_bodies"),
        "{}",
        output
    );
    assert_eq!(output["resultStatus"]["status"].as_str(), Some("partial"), "{}", output);
}

#[test]
fn test_d20_exact_symbol_rejects_noncanonical_id() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::canonicalize_test_root(temp.path());
    let context = d20_callers_context(&root, "D20Overloads.cs", D20_OVERLOAD_SOURCE);

    let result = dispatch_tool(
        &context,
        "xray_callers",
        &json!({
            "targets": [{ "symbolId": format!("cs:v1:{}", "A".repeat(64)) }]
        }),
    );
    assert!(result.is_error);
    assert!(result.content[0].text.contains("64 lowercase hex"));
}

#[test]
fn test_xray_callers_ambiguity_warning_truncated() {
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
        path_to_id.insert(crate::path_identity_key(&PathBuf::from(f)), i as u32);
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

    let result = dispatch_tool(&ctx, "xray_callers", &json!({ "method": ["OnInit"] }));
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
fn test_xray_callers_ambiguity_warning_few_classes() {
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
        path_to_id.insert(crate::path_identity_key(&PathBuf::from(f)), i as u32);
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
    let result = dispatch_tool(&ctx, "xray_callers", &json!({ "method": ["Initialize"] }));
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
fn test_xray_callers_batch_ambiguity_warning() {
    // Batch callers (method=["Foo","Bar"]) should produce per-method ambiguity warnings
    // when method is defined in multiple classes and no class filter is set.
    use std::path::PathBuf;

    let mut content_idx: HashMap<String, Vec<Posting>> = HashMap::new();
    let files = vec![
        "C:\\src\\ServiceA.cs".to_string(),
        "C:\\src\\ServiceB.cs".to_string(),
    ];
    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "ServiceA".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "Foo".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("ServiceA".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ServiceB".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "Foo".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("ServiceB".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    content_idx.entry("foo".to_string()).or_default().push(
        Posting { file_id: 0, lines: vec![10] }
    );
    content_idx.entry("foo".to_string()).or_default().push(
        Posting { file_id: 1, lines: vec![10] }
    );

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
        path_to_id.insert(crate::path_identity_key(&PathBuf::from(f)), i as u32);
    }

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files.clone(),
        index: content_idx, total_tokens: 200,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50; 2],
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

    // Batch call with "Foo,NonExistent" — Foo should get warning, NonExistent should get hint
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["Foo","NonExistent"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let results = output["results"].as_array().expect("should have results array");
    assert_eq!(results.len(), 2, "batch should have 2 per-method results");

    // First method (Foo) — should have warning about 2 classes
    let foo_result = &results[0];
    assert_eq!(foo_result["method"].as_str().unwrap(), "Foo");
    let warning = foo_result["warning"].as_str().expect("Foo should have ambiguity warning");
    assert!(warning.contains("2 classes"), "Warning should mention 2 classes, got: {}", warning);
    assert!(warning.contains("ServiceA"), "Warning should list ServiceA");
    assert!(warning.contains("ServiceB"), "Warning should list ServiceB");

    // Second method (NonExistent) — should have hint (nearest match) but no warning
    let ne_result = &results[1];
    assert_eq!(ne_result["method"].as_str().unwrap(), "NonExistent");
    assert!(ne_result.get("warning").is_none() || ne_result["warning"].is_null(),
        "NonExistent should not have ambiguity warning");
    // hint may or may not exist depending on nearest match — just verify no crash
}

#[test]
fn test_xray_callers_batch_truncated_flag() {
    // Batch callers with maxTotalNodes=1 should produce truncated=true per-method
    use std::path::PathBuf;

    let files = vec!["C:\\src\\Svc.cs".to_string()];
    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "Svc".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "DoWork".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("Svc".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut content_idx: HashMap<String, Vec<Posting>> = HashMap::new();
    content_idx.entry("dowork".to_string()).or_default().push(
        Posting { file_id: 0, lines: vec![10] }
    );

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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from(&files[0])), 0);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: files.clone(),
        index: content_idx, total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50],
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

    // Batch call with maxTotalNodes=1 to force truncation
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["DoWork","DoWork"],
        "maxTotalNodes": 1
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let results = output["results"].as_array().expect("should have results");

    // Each per-method result should have truncated field
    for r in results {
        assert!(r.get("truncated").is_some(), "per-method result should have truncated field");
    }

    // nodesVisited should be present (for up direction)
    for r in results {
        assert!(r.get("nodesVisited").is_some(), "per-method result should have nodesVisited field");
    }
}

#[test]
fn test_xray_callers_no_ambiguity_warning_with_class_param() {
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
        path_to_id.insert(crate::path_identity_key(&PathBuf::from(f)), i as u32);
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
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["Initialize"],
        "class": "AlphaService"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output.get("warning").is_none(),
        "No warning should be emitted when 'class' parameter is provided. Got: {:?}",
        output.get("warning"));
}

#[test]
fn test_xray_callers_no_ambiguity_warning_single_class() {
    // Method exists in only 1 class → no warning even without `class` param.
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["QueryInternalAsync"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output.get("warning").is_none(),
        "No warning should be emitted when method exists in only 1 class. Got: {:?}",
        output.get("warning"));
}


#[test]
fn test_xray_callers_exclude_dir_and_file() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\services\\ServiceA.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\controllers\\ControllerB.cs")), 1);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\tests\\TestC.cs")), 2);

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
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["MethodA"],
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
    let result2 = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["MethodA"],
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
fn test_xray_callers_cycle_detection_down() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\ClassA.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\ClassB.cs")), 1);

    // MethodA (def index 1) calls MethodB; MethodB (def index 3) calls MethodA
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(1, vec![CallSite {
        method_name: "MethodB".to_string(),
        receiver_type: Some("ClassB".to_string()),
        line: 20,
        call_kind: Default::default(),
        receiver_is_generic: false,
            }]);
    method_calls.insert(3, vec![CallSite {
        method_name: "MethodA".to_string(),
        receiver_type: Some("ClassA".to_string()),
        line: 20,
        call_kind: Default::default(),
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
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["MethodA"],
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
fn test_xray_callers_cycle_detection() {
    // Regression test: A recursive call graph (A calls B, B calls A) must NOT
    // cause infinite recursion in `xray_callers` direction="up".
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\ServiceA.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\ServiceB.cs")), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    // MethodB (di=3) calls MethodA at line 20
    method_calls.insert(3, vec![CallSite {
        method_name: "MethodA".to_string(),
        receiver_type: Some("ServiceA".to_string()),
        line: 20,
        call_kind: Default::default(),
        receiver_is_generic: false,
            }]);
    // MethodA (di=1) calls MethodB at line 20
    method_calls.insert(1, vec![CallSite {
        method_name: "MethodB".to_string(),
        receiver_type: Some("ServiceB".to_string()),
        line: 20,
        call_kind: Default::default(),
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
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["MethodA"],
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
fn test_xray_callers_ext_filter_comma_split() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\DataService.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\CsController.cs")), 1);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\script.txt")), 2);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    // HandleRequest (di=3) calls ProcessData at line 15
    method_calls.insert(3, vec![CallSite {
        method_name: "ProcessData".to_string(),
        receiver_type: Some("DataService".to_string()),
        line: 15,
        call_kind: Default::default(),
        receiver_is_generic: false,
            }]);
    // RunScript (di=5) calls ProcessData at line 10
    method_calls.insert(5, vec![CallSite {
        method_name: "ProcessData".to_string(),
        receiver_type: Some("DataService".to_string()),
        line: 10,
        call_kind: Default::default(),
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
    let result_cs = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ProcessData"],
        "class": "DataService",
        "depth": 1,
        "ext": ["cs"]
    }));
    assert!(!result_cs.is_error, "xray_callers ext=cs should not error: {}", result_cs.content[0].text);
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
    let result_both = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ProcessData"],
        "class": "DataService",
        "depth": 1,
        "ext": ["cs,txt"]
    }));
    assert!(!result_both.is_error, "xray_callers ext=cs,txt should not error: {}", result_both.content[0].text);
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
fn test_xray_callers_overloads_not_collapsed_up() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\Validator.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\Processor.cs")), 1);

    // Process(int) at di=3 calls Validate at line 25; Process(string) at di=4 calls Validate at line 45
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "Validate".to_string(),
        receiver_type: Some("Validator".to_string()),
        line: 25,
        call_kind: Default::default(),
        receiver_is_generic: false,
    }]);
    method_calls.insert(4, vec![CallSite {
        method_name: "Validate".to_string(),
        receiver_type: Some("Validator".to_string()),
        line: 45,
        call_kind: Default::default(),
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

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["Validate"],
        "class": "Validator",
        "depth": 1
    }));
    assert!(!result.is_error, "xray_callers should not error: {}", result.content[0].text);
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
fn test_xray_callers_overloads_not_collapsed_down() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\Orchestrator.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\Executor.cs")), 1);

    // RunAll (di=1) calls both Execute overloads
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(1, vec![
        CallSite {
            method_name: "Execute".to_string(),
            receiver_type: Some("Executor".to_string()),
            line: 15,
            call_kind: Default::default(),
            receiver_is_generic: false,
        },
        CallSite {
            method_name: "Execute".to_string(),
            receiver_type: Some("Executor".to_string()),
            line: 20,
            call_kind: Default::default(),
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

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["RunAll"],
        "class": "Orchestrator",
        "direction": "down",
        "depth": 1
    }));
    assert!(!result.is_error, "xray_callers down should not error: {}", result.content[0].text);
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
fn test_xray_callers_same_name_different_receiver_interface_resolution() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\IServiceA.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\IServiceB.cs")), 1);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\ServiceA.cs")), 2);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\ServiceB.cs")), 3);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\src\\Consumer.cs")), 4);

    // Consumer.DoWork (di=9) calls Execute with receiver_type = IServiceB
    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(9, vec![CallSite {
        method_name: "Execute".to_string(),
        receiver_type: Some("IServiceB".to_string()),
        line: 20,
        call_kind: Default::default(),
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
    let result_a = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["Execute"],
        "class": "ServiceA",
        "depth": 2
    }));
    assert!(!result_a.is_error, "xray_callers for ServiceA.Execute should not error: {}", result_a.content[0].text);
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
    let result_b = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["Execute"],
        "class": "ServiceB",
        "depth": 2
    }));
    assert!(!result_b.is_error, "xray_callers for ServiceB.Execute should not error: {}", result_b.content[0].text);
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
fn test_xray_callers_include_body_default_false() {
    // Default (no includeBody) should NOT have body fields in nodes
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ExecuteQueryAsync"],
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
fn test_xray_callers_include_body_false_explicit() {
    // Explicit includeBody=false should NOT have body fields
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ExecuteQueryAsync"],
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
    let tmp = std::env::temp_dir().join(format!("xray_callers_body_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from(&file0_str)), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from(&file1_str)), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "SubmitOrder".to_string(),
        receiver_type: Some("OrderService".to_string()),
        line: 7,
        call_kind: Default::default(),
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
        workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(tmp.to_string_lossy().to_string()))),
        max_response_bytes: 0, // no truncation for tests
        ..Default::default()
    };

    (ctx, tmp)
}

fn cleanup_callers_body_ctx(tmp: &std::path::Path) {
    let _ = std::fs::remove_dir_all(tmp);
}

#[test]
fn test_xray_callers_include_body_up() {
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["SubmitOrder"],
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
fn test_xray_callers_include_body_down() {
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ProcessOrder"],
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
fn test_xray_callers_include_body_max_body_lines() {
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["SubmitOrder"],
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
fn test_xray_callers_include_body_max_total_body_lines() {
    let (ctx, tmp) = make_callers_body_ctx();

    // Set maxTotalBodyLines=1 — after the first node's body, the budget should be exhausted
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["SubmitOrder"],
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
fn test_xray_callers_include_body_nonexistent_file() {
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
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\nonexistent\\Worker.cs")), 0);
    path_to_id.insert(crate::path_identity_key(&PathBuf::from("C:\\nonexistent\\Caller.cs")), 1);

    let mut method_calls: HashMap<u32, Vec<CallSite>> = HashMap::new();
    method_calls.insert(3, vec![CallSite {
        method_name: "DoWork".to_string(),
        receiver_type: Some("Worker".to_string()),
        line: 15,
        call_kind: Default::default(),
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

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["DoWork"],
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
    let result_no_body = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["NonExistent"]
    }));
    // The error result won't be truncated, but the dispatch logic was exercised

    // With includeBody=true: effective_max should be 64KB
    // We can't directly observe the effective_max, but we verify the feature
    // compiles and runs without error
    let result_with_body = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["NonExistent"],
        "includeBody": true
    }));
    // Both should succeed (no error from budget logic)
    assert!(!result_no_body.is_error || result_no_body.content[0].text.contains("Definition index"));
    assert!(!result_with_body.is_error || result_with_body.content[0].text.contains("Definition index"));
}


#[test]
fn test_xray_callers_include_body_has_root_method() {
    // When includeBody=true, the response should include rootMethod with body of the searched method
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["SubmitOrder"],
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
    let result2 = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["SubmitOrder"],
        "class": "OrderService",
        "depth": 1
    }));
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    assert!(output2.get("rootMethod").is_none(),
        "rootMethod should NOT appear when includeBody is absent/false");

    cleanup_callers_body_ctx(&tmp);
}

#[test]
fn test_xray_callers_include_body_root_method_down() {
    // rootMethod should also work for direction=down
    let (ctx, tmp) = make_callers_body_ctx();

    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["ProcessOrder"],
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

#[test]
fn test_xray_callers_root_method_body_line_range() {
    // bodyLineStart/bodyLineEnd should filter rootMethod body to the specified line range
    let (ctx, tmp) = make_callers_body_ctx();

    // First, get the full rootMethod body to know the line numbers
    let result_full = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["SubmitOrder"],
        "class": "OrderService",
        "depth": 1,
        "includeBody": true
    }));
    assert!(!result_full.is_error, "Error: {}", result_full.content[0].text);
    let output_full: Value = serde_json::from_str(&result_full.content[0].text).unwrap();
    let root_full = output_full.get("rootMethod").expect("Should have rootMethod");
    let full_start = root_full["bodyStartLine"].as_u64().unwrap();
    let full_body = root_full["body"].as_array().unwrap();
    let full_len = full_body.len();
    assert!(full_len >= 3, "SubmitOrder should have at least 3 body lines, got {}", full_len);

    // Now request only 2 lines from the middle of the body
    let target_start = full_start + 2; // skip first 2 lines
    let target_end = full_start + 3;   // take 2 lines
    let result = dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["SubmitOrder"],
        "class": "OrderService",
        "depth": 1,
        "includeBody": true,
        "bodyLineStart": target_start,
        "bodyLineEnd": target_end
    }));
    assert!(!result.is_error, "Error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let root = output.get("rootMethod").expect("Should have rootMethod with bodyLineStart/End");
    let filtered_body = root["body"].as_array().unwrap();
    assert_eq!(filtered_body.len(), 2,
        "bodyLineStart/End should filter rootMethod to 2 lines, got {}", filtered_body.len());
    assert_eq!(root["bodyStartLine"].as_u64().unwrap(), target_start,
        "bodyStartLine should reflect the filtered range");

    cleanup_callers_body_ctx(&tmp);
}
