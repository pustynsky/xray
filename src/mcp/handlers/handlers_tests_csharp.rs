//! C# definitions tests -- extracted from handlers_tests_csharp.rs.
//! Split from handlers_tests.rs for maintainability.

use super::*;
use super::handlers_test_utils::{cleanup_tmp, make_ctx_with_defs};
use crate::definitions::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

// ─── Helpers ─────────────────────────────────────────────────────────

/// Helper: create a context with real temp .cs files and a definition index.
fn make_ctx_with_real_files() -> (HandlerContext, std::path::PathBuf) {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_test_cs_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);
    let file0_path = tmp_dir.join("MyService.cs");
    { let mut f = std::fs::File::create(&file0_path).unwrap(); for i in 1..=15 { writeln!(f, "// line {}", i).unwrap(); } }
    let file1_path = tmp_dir.join("BigFile.cs");
    { let mut f = std::fs::File::create(&file1_path).unwrap(); for i in 1..=25 { writeln!(f, "// big line {}", i).unwrap(); } }
    let file0_str = file0_path.to_string_lossy().to_string();
    let file1_str = file1_path.to_string_lossy().to_string();
    let definitions = vec![
        DefinitionEntry { file_id: 0, name: "MyService".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 15, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 0, name: "DoWork".to_string(), kind: DefinitionKind::Method, line_start: 3, line_end: 8, parent: Some("MyService".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "BigClass".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 25, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        DefinitionEntry { file_id: 1, name: "Process".to_string(), kind: DefinitionKind::Method, line_start: 5, line_end: 24, parent: Some("BigClass".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
    ];
    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, u32> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() { let idx = i as u32; name_index.entry(def.name.to_lowercase()).or_default().push(idx); kind_index.entry(def.kind).or_default().push(idx); file_index.entry(def.file_id).or_default().push(idx); }
    path_to_id.insert(file0_path, 0); path_to_id.insert(file1_path, 1);
    let def_index = DefinitionIndex { root: tmp_dir.to_string_lossy().to_string(), created_at: 0, extensions: vec!["cs".to_string()], files: vec![file0_str.clone(), file1_str.clone()], definitions, name_index, kind_index, file_index, path_to_id, ..Default::default() };
    let content_index = ContentIndex { root: tmp_dir.to_string_lossy().to_string(), files: vec![file0_str, file1_str], extensions: vec!["cs".to_string()], file_token_counts: vec![0, 0], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(content_index)), def_index: Some(Arc::new(RwLock::new(def_index))), server_dir: tmp_dir.to_string_lossy().to_string(), ..Default::default() };
    (ctx, tmp_dir)
}

// ─── search_callers tests ────────────────────────────────────────────
#[test]
fn test_contains_line_finds_method() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "QueryService",
        "containsLine": 391
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "Should find containing definitions");
    assert_eq!(defs[0]["name"], "RunQueryBatchAsync");
    assert_eq!(defs[0]["kind"], "method");
}

#[test]
fn test_contains_line_returns_parent() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "QueryService",
        "containsLine": 800
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    let method = defs.iter().find(|d| d["kind"] == "method").unwrap();
    assert_eq!(method["name"], "QueryInternalAsync");
    assert_eq!(method["parent"], "QueryService");
}

#[test]
fn test_contains_line_no_match() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "QueryService",
        "containsLine": 999
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    assert!(defs.is_empty(), "Should find no definitions for line 999");
}

// ─── find_containing_method tests ────────────────────────────────────

#[test]
fn test_find_containing_method_innermost() {
    let ctx = make_ctx_with_defs();
    let def_idx = ctx.def_index.as_ref().unwrap().read().unwrap();
    let result = find_containing_method(&def_idx, 2, 391);
    assert!(result.is_some());
    let (name, parent, _line, _di) = result.unwrap();
    assert_eq!(name, "RunQueryBatchAsync");
    assert_eq!(parent.as_deref(), Some("QueryService"));
}

#[test]
fn test_find_containing_method_none() {
    let ctx = make_ctx_with_defs();
    let def_idx = ctx.def_index.as_ref().unwrap().read().unwrap();
    let result = find_containing_method(&def_idx, 2, 999);
    assert!(result.is_none());
}
#[test]
fn test_search_definitions_regex_name_filter() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "Execute.*",
        "regex": true
    }));
    assert!(!result.is_error, "search_definitions regex should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Should find ExecuteQueryAsync (exists in ResilientClient and ProxyClient)
    assert!(!defs.is_empty(), "Regex 'Execute.*' should match ExecuteQueryAsync definitions");

    // All returned definitions should match the regex
    for def in defs {
        let name = def["name"].as_str().unwrap();
        assert!(name.to_lowercase().starts_with("execute"),
            "Definition '{}' should match regex 'Execute.*'", name);
    }

    // Should NOT contain definitions that don't match
    for def in defs {
        let name = def["name"].as_str().unwrap();
        assert!(name != "QueryService" && name != "RunQueryBatchAsync",
            "Definition '{}' should NOT match regex 'Execute.*'", name);
    }
}

#[test]
fn test_search_definitions_audit_mode() {
    let ctx = make_ctx_with_defs();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "audit": true
    }));
    assert!(!result.is_error, "audit mode should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();

    // Audit output should have the audit object
    let audit = &output["audit"];
    assert!(audit.is_object(), "Expected 'audit' object in output");
    assert!(audit["totalFiles"].as_u64().is_some(), "Expected totalFiles in audit");
    assert!(audit["filesWithDefinitions"].as_u64().is_some(), "Expected filesWithDefinitions in audit");
    assert!(audit["filesWithoutDefinitions"].as_u64().is_some(), "Expected filesWithoutDefinitions in audit");
    assert!(audit["readErrors"].as_u64().is_some(), "Expected readErrors in audit");
    assert!(audit["lossyUtf8Files"].as_u64().is_some(), "Expected lossyUtf8Files in audit");
    assert!(audit["suspiciousFiles"].as_u64().is_some(), "Expected suspiciousFiles count in audit");
    assert!(audit["suspiciousThresholdBytes"].as_u64().is_some(), "Expected suspiciousThresholdBytes in audit");

    // Should also have suspiciousFiles array at top level
    assert!(output["suspiciousFiles"].is_array(), "Expected suspiciousFiles array in output");

    // Verify the counts make sense for our test context (3 files, all with definitions)
    assert_eq!(audit["totalFiles"].as_u64().unwrap(), 3);
    assert_eq!(audit["filesWithDefinitions"].as_u64().unwrap(), 3);
}

#[test]
fn test_search_definitions_exclude_dir() {
    // Create a context with definitions in two different directories
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:\\src\\main\\UserService.cs".to_string(),
            "C:\\src\\tests\\UserServiceTests.cs".to_string(),
        ],
        index: HashMap::new(), total_tokens: 100,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50, 50],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "GetUser".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("UserService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "UserServiceTests".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "TestGetUser".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 30,
            parent: Some("UserServiceTests".to_string()), signature: None,
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
    path_to_id.insert(PathBuf::from("C:\\src\\main\\UserService.cs"), 0);
    path_to_id.insert(PathBuf::from("C:\\src\\tests\\UserServiceTests.cs"), 1);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            "C:\\src\\main\\UserService.cs".to_string(),
            "C:\\src\\tests\\UserServiceTests.cs".to_string(),
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

    // Exclude "tests" directory
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "excludeDir": ["tests"]
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // All returned definitions should be from non-test directories
    for def in defs {
        let file = def["file"].as_str().unwrap_or("");
        assert!(!file.to_lowercase().contains("tests"),
            "excludeDir should filter out definitions from 'tests' dir, but found file: {}", file);
    }

    // Should still have the main definitions
    assert!(!defs.is_empty(), "Should have definitions from non-excluded directories");
    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"UserService"), "Should contain UserService from main dir");
    assert!(names.contains(&"GetUser"), "Should contain GetUser from main dir");
    assert!(!names.contains(&"UserServiceTests"), "Should NOT contain UserServiceTests from tests dir");
    assert!(!names.contains(&"TestGetUser"), "Should NOT contain TestGetUser from tests dir");
}

#[test]
fn test_search_definitions_combined_name_parent_kind_filter() {
    let ctx = make_ctx_with_defs();

    // Filter: name=ExecuteQueryAsync, parent=ResilientClient, kind=method
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "ExecuteQueryAsync",
        "parent": "ResilientClient",
        "kind": "method"
    }));
    assert!(!result.is_error, "Combined filter should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Should return exactly 1 definition: ExecuteQueryAsync in ResilientClient
    assert_eq!(defs.len(), 1,
        "Expected exactly 1 result for name+parent+kind filter, got {}: {:?}",
        defs.len(), defs);
    assert_eq!(defs[0]["name"], "ExecuteQueryAsync");
    assert_eq!(defs[0]["parent"], "ResilientClient");
    assert_eq!(defs[0]["kind"], "method");

    // Verify: same name+kind but different parent should NOT match
    let result2 = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "ExecuteQueryAsync",
        "parent": "NonExistentClass",
        "kind": "method"
    }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let defs2 = output2["definitions"].as_array().unwrap();
    assert_eq!(defs2.len(), 0,
        "Non-matching parent should return 0 results, got {}", defs2.len());
}

#[test]
fn test_search_definitions_struct_kind() {
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\src\\Models.cs".to_string()],
        index: HashMap::new(), total_tokens: 50,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserModel".to_string(),
            kind: DefinitionKind::Struct, line_start: 1, line_end: 20,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 25, line_end: 80,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "GetUser".to_string(),
            kind: DefinitionKind::Method, line_start: 30, line_end: 45,
            parent: Some("UserService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "OrderInfo".to_string(),
            kind: DefinitionKind::Struct, line_start: 85, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
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
    path_to_id.insert(PathBuf::from("C:\\src\\Models.cs"), 0);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\Models.cs".to_string()],
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

    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "kind": "struct"
    }));
    assert!(!result.is_error, "kind=struct should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Should return only struct definitions
    assert_eq!(defs.len(), 2, "Expected 2 struct definitions, got {}", defs.len());
    for def in defs {
        assert_eq!(def["kind"], "struct",
            "All results should be structs, but got kind={}", def["kind"]);
    }
    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"UserModel"), "Should contain UserModel struct");
    assert!(names.contains(&"OrderInfo"), "Should contain OrderInfo struct");
}

#[test]
fn test_search_definitions_base_type_filter() {
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\src\\Controllers.cs".to_string()],
        index: HashMap::new(), total_tokens: 50,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserController".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![],
            base_types: vec!["ControllerBase".to_string()],
        },
        DefinitionEntry {
            file_id: 0, name: "OrderService".to_string(),
            kind: DefinitionKind::Class, line_start: 55, line_end: 100,
            parent: None, signature: None, modifiers: vec![], attributes: vec![],
            base_types: vec!["IOrderService".to_string()],
        },
        DefinitionEntry {
            file_id: 0, name: "AdminController".to_string(),
            kind: DefinitionKind::Class, line_start: 105, line_end: 150,
            parent: None, signature: None, modifiers: vec![], attributes: vec![],
            base_types: vec!["ControllerBase".to_string(), "IAdminAccess".to_string()],
        },
        DefinitionEntry {
            file_id: 0, name: "PlainClass".to_string(),
            kind: DefinitionKind::Class, line_start: 155, line_end: 170,
            parent: None, signature: None, modifiers: vec![], attributes: vec![],
            base_types: vec![],
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
    path_to_id.insert(PathBuf::from("C:\\src\\Controllers.cs"), 0);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\Controllers.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index,
        file_index, path_to_id, method_calls: HashMap::new(),
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    // Filter by baseType=ControllerBase — should return UserController and AdminController
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "baseType": "ControllerBase"
    }));
    assert!(!result.is_error, "baseType filter should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 2, "Expected 2 definitions with baseType=ControllerBase, got {}", defs.len());
    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"UserController"), "Should contain UserController");
    assert!(names.contains(&"AdminController"), "Should contain AdminController");

    // Filter by baseType=IOrderService — should return only OrderService
    let result2 = dispatch_tool(&ctx, "search_definitions", &json!({
        "baseType": "IOrderService"
    }));
    assert!(!result2.is_error);
    let output2: Value = serde_json::from_str(&result2.content[0].text).unwrap();
    let defs2 = output2["definitions"].as_array().unwrap();
    assert_eq!(defs2.len(), 1, "Expected 1 definition with baseType=IOrderService, got {}", defs2.len());
    assert_eq!(defs2[0]["name"], "OrderService");

    // Filter by non-existent baseType — should return empty
    let result3 = dispatch_tool(&ctx, "search_definitions", &json!({
        "baseType": "NonExistentBase"
    }));
    assert!(!result3.is_error);
    let output3: Value = serde_json::from_str(&result3.content[0].text).unwrap();
    let defs3 = output3["definitions"].as_array().unwrap();
    assert!(defs3.is_empty(), "Non-existent baseType should return empty, got {}", defs3.len());
}

#[test]
fn test_search_definitions_enum_member_kind() {
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\src\\Enums.cs".to_string()],
        index: HashMap::new(), total_tokens: 50,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![50],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "OrderStatus".to_string(),
            kind: DefinitionKind::Enum, line_start: 1, line_end: 20,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "Pending".to_string(),
            kind: DefinitionKind::EnumMember, line_start: 3, line_end: 3,
            parent: Some("OrderStatus".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "Completed".to_string(),
            kind: DefinitionKind::EnumMember, line_start: 4, line_end: 4,
            parent: Some("OrderStatus".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "Cancelled".to_string(),
            kind: DefinitionKind::EnumMember, line_start: 5, line_end: 5,
            parent: Some("OrderStatus".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "OrderHelper".to_string(),
            kind: DefinitionKind::Class, line_start: 25, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "GetStatus".to_string(),
            kind: DefinitionKind::Method, line_start: 30, line_end: 40,
            parent: Some("OrderHelper".to_string()), signature: None,
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
    path_to_id.insert(PathBuf::from("C:\\src\\Enums.cs"), 0);

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\Enums.cs".to_string()],
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

    // Filter by kind=enumMember
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "kind": "enumMember"
    }));
    assert!(!result.is_error, "kind=enumMember should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Should return exactly 3 enum members: Pending, Completed, Cancelled
    assert_eq!(defs.len(), 3, "Expected 3 enumMember definitions, got {}", defs.len());
    for def in defs {
        assert_eq!(def["kind"], "enumMember",
            "All results should be enumMember, but got kind={}", def["kind"]);
    }
    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"Pending"), "Should contain Pending enum member");
    assert!(names.contains(&"Completed"), "Should contain Completed enum member");
    assert!(names.contains(&"Cancelled"), "Should contain Cancelled enum member");

    // Verify parent is set correctly
    for def in defs {
        assert_eq!(def["parent"], "OrderStatus",
            "Enum members should have parent=OrderStatus, got {}", def["parent"]);
    }
}

// ─── includeBody tests (require real files) ──────────────────────────

#[test] fn test_search_definitions_include_body() {
    let (ctx, tmp) = make_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({"name": "DoWork", "includeBody": true}));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 1);
    let body = defs[0]["body"].as_array().unwrap();
    assert_eq!(body.len(), 6);
    assert_eq!(defs[0]["bodyStartLine"], 3);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_definitions_include_body_default_false() {
    let (ctx, tmp) = make_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({"name": "DoWork"}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["definitions"].as_array().unwrap()[0].get("body").is_none());
    cleanup_tmp(&tmp);
}

#[test] fn test_search_definitions_max_body_lines_truncation() {
    let (ctx, tmp) = make_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({"name": "Process", "includeBody": true, "maxBodyLines": 5}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs[0]["body"].as_array().unwrap().len(), 5);
    assert_eq!(defs[0]["bodyTruncated"], true);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_definitions_max_total_body_lines_budget() {
    let (ctx, tmp) = make_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({"name": "DoWork,Process", "includeBody": true, "maxTotalBodyLines": 10}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalBodyLinesReturned"].as_u64().unwrap();
    assert!(total <= 10);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_definitions_contains_line_with_body() {
    let (ctx, tmp) = make_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({"file": "MyService", "containsLine": 5, "includeBody": true}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    assert_eq!(defs[0]["name"], "DoWork");
    assert!(defs[0]["body"].as_array().unwrap().len() > 0);
    cleanup_tmp(&tmp);
}

#[test] fn test_search_definitions_file_cache() {
    let (ctx, tmp) = make_ctx_with_real_files();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({"parent": "MyService", "includeBody": true}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    for def in defs { assert!(def.get("body").is_some()); }
    cleanup_tmp(&tmp);
}

#[test] fn test_search_definitions_stale_file_warning() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join(format!("search_test_stale_cs_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let fp = tmp.join("Stale.cs");
    { let mut f = std::fs::File::create(&fp).unwrap(); for i in 1..=10 { writeln!(f, "// stale line {}", i).unwrap(); } }
    let fs = fp.to_string_lossy().to_string();
    let definitions = vec![DefinitionEntry { file_id: 0, name: "StaleClass".to_string(), kind: DefinitionKind::Class, line_start: 5, line_end: 20, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] }];
    let mut ni: HashMap<String, Vec<u32>> = HashMap::new(); let mut ki: HashMap<DefinitionKind, Vec<u32>> = HashMap::new(); let mut fi: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() { ni.entry(def.name.to_lowercase()).or_default().push(i as u32); ki.entry(def.kind).or_default().push(i as u32); fi.entry(def.file_id).or_default().push(i as u32); }
    let di = DefinitionIndex { root: tmp.to_string_lossy().to_string(), created_at: 0, extensions: vec!["cs".to_string()], files: vec![fs.clone()], definitions, name_index: ni, kind_index: ki, file_index: fi, ..Default::default() };
    let ci = ContentIndex { root: tmp.to_string_lossy().to_string(), files: vec![fs], extensions: vec!["cs".to_string()], file_token_counts: vec![0], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(ci)), def_index: Some(Arc::new(RwLock::new(di))), server_dir: tmp.to_string_lossy().to_string(), ..Default::default() };
    let result = dispatch_tool(&ctx, "search_definitions", &json!({"name": "StaleClass", "includeBody": true}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert!(output["definitions"].as_array().unwrap()[0].get("bodyWarning").is_some());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test] fn test_search_definitions_body_error() {
    let definitions = vec![DefinitionEntry { file_id: 0, name: "GhostClass".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 10, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] }];
    let mut ni: HashMap<String, Vec<u32>> = HashMap::new(); let mut ki: HashMap<DefinitionKind, Vec<u32>> = HashMap::new(); let mut fi: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, def) in definitions.iter().enumerate() { ni.entry(def.name.to_lowercase()).or_default().push(i as u32); ki.entry(def.kind).or_default().push(i as u32); fi.entry(def.file_id).or_default().push(i as u32); }
    let ne = "C:\\nonexistent\\path\\Ghost.cs".to_string();
    let di = DefinitionIndex { root: ".".to_string(), created_at: 0, extensions: vec!["cs".to_string()], files: vec![ne.clone()], definitions, name_index: ni, kind_index: ki, file_index: fi, ..Default::default() };
    let ci = ContentIndex { root: ".".to_string(), files: vec![ne], extensions: vec!["cs".to_string()], file_token_counts: vec![0], ..Default::default() };
    let ctx = HandlerContext { index: Arc::new(RwLock::new(ci)), def_index: Some(Arc::new(RwLock::new(di))), ..Default::default() };
    let result = dispatch_tool(&ctx, "search_definitions", &json!({"name": "GhostClass", "includeBody": true}));
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["definitions"].as_array().unwrap()[0]["bodyError"], "failed to read file");
}

// ─── search_reindex_definitions success test ─────────────────────────

#[test]
fn test_reindex_definitions_success() {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tmp_dir = std::env::temp_dir().join(format!("search_reindex_def_cs_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create a minimal .cs file so the reindex has something to parse
    let cs_file = tmp_dir.join("Sample.cs");
    {
        let mut f = std::fs::File::create(&cs_file).unwrap();
        writeln!(f, "public class SampleClass {{").unwrap();
        writeln!(f, "    public void DoWork() {{ }}").unwrap();
        writeln!(f, "}}").unwrap();
    }

    let dir_str = tmp_dir.to_string_lossy().to_string();

    // Build an initial (empty) definition index
    let def_index = DefinitionIndex {
        root: dir_str.clone(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };

    let content_index = ContentIndex {
        root: dir_str.clone(),
        files: vec![], index: HashMap::new(), total_tokens: 0,
        extensions: vec!["cs".to_string()], file_token_counts: vec![],
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_dir: dir_str.clone(),
        index_base: tmp_dir.join(".index"),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_reindex_definitions", &json!({}));
    assert!(!result.is_error, "Reindex definitions should succeed: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(output["status"], "ok", "Status should be 'ok'");
    assert!(output["files"].as_u64().unwrap() >= 1, "Should have parsed at least 1 file");
    assert!(output["definitions"].as_u64().unwrap() >= 1, "Should have found at least 1 definition");
    assert!(output["rebuildTimeMs"].as_f64().is_some(), "Should report rebuild time");

    cleanup_tmp(&tmp_dir);
}

// ─── File filter path separator normalization tests (T77) ────────────

/// Helper: create a context with backslash paths in definition index
/// to test file filter separator normalization.
fn make_ctx_with_backslash_paths() -> HandlerContext {
    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 50,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "GetUser".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 20,
            parent: Some("UserService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "OrderProcessor".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 80,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 1, name: "ProcessOrder".to_string(),
            kind: DefinitionKind::Method, line_start: 30, line_end: 45,
            parent: Some("OrderProcessor".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
    let path_to_id: HashMap<PathBuf, u32> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    // Paths stored with BACKSLASHES (simulating Windows paths from clean_path before fix,
    // or manually constructed test data)
    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec![
            r"src\Services\UserService.cs".to_string(),
            r"src\Processing\OrderProcessor.cs".to_string(),
        ],
        definitions, name_index, kind_index,
        file_index, path_to_id,
        ..Default::default()
    };

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            r"src\Services\UserService.cs".to_string(),
            r"src\Processing\OrderProcessor.cs".to_string(),
        ],
        index: HashMap::new(), total_tokens: 0, extensions: vec!["cs".to_string()],
        file_token_counts: vec![100, 100], ..Default::default()
    };

    HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    }
}

#[test]
fn test_search_definitions_file_filter_forward_slash() {
    // T77: file filter with forward slashes should match backslash-stored paths
    let ctx = make_ctx_with_backslash_paths();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "src/Services/UserService"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalResults"].as_u64().unwrap();
    assert_eq!(total, 2, "Forward-slash file filter should match backslash paths (UserService class + GetUser method)");
}

#[test]
fn test_search_definitions_file_filter_backslash() {
    // T77: file filter with backslashes should also still work
    let ctx = make_ctx_with_backslash_paths();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": r"src\Services\UserService"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalResults"].as_u64().unwrap();
    assert_eq!(total, 2, "Backslash file filter should match backslash paths");
}

#[test]
fn test_search_definitions_file_filter_mixed_separators() {
    // T77: mixed separators should work
    let ctx = make_ctx_with_backslash_paths();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": r"src/Services\UserService"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalResults"].as_u64().unwrap();
    assert_eq!(total, 2, "Mixed-separator file filter should match");
}

#[test]
fn test_search_definitions_file_filter_no_match() {
    // Sanity check: non-matching file filter returns 0
    let ctx = make_ctx_with_backslash_paths();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "src/NonExistent/Path"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let total = output["summary"]["totalResults"].as_u64().unwrap();
    assert_eq!(total, 0, "Non-matching file filter should return 0 results");
}

#[test]
fn test_search_definitions_contains_line_forward_slash() {
    // T77: containsLine with forward-slash file filter should work
    let ctx = make_ctx_with_backslash_paths();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "src/Services/UserService",
        "containsLine": 15
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "containsLine with forward-slash file should find definitions");
    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"GetUser") || names.contains(&"UserService"),
        "Should find GetUser or UserService containing line 15, got: {:?}", names);
}

#[test]
fn test_search_definitions_contains_line_backslash() {
    // T77: containsLine with backslash file filter should also work
    let ctx = make_ctx_with_backslash_paths();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": r"src\Services\UserService",
        "containsLine": 15
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "containsLine with backslash file should find definitions");
}

#[test]
fn test_search_definitions_contains_line_mixed_separators() {
    // T77: containsLine with mixed separators should work
    let ctx = make_ctx_with_backslash_paths();
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": r"src/Services\UserService",
        "containsLine": 15
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    assert!(!defs.is_empty(), "containsLine with mixed separators should find definitions");
}
#[test]
fn test_search_definitions_comma_separated_name_filter() {
    // Analytics Test 1.5: Comma-separated name OR lookup.
    // name="ClassA,ClassB,ClassC" should return results from all matching names.
    let ctx = make_ctx_with_defs();

    // Search for multiple names at once: ResilientClient,QueryService,ProxyClient
    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "ResilientClient,QueryService,ProxyClient",
        "kind": "class"
    }));
    assert!(!result.is_error, "Comma-separated name filter should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Should find all 3 classes
    assert_eq!(defs.len(), 3,
        "Expected 3 classes for comma-separated name filter, got {}: {:?}",
        defs.len(), defs.iter().map(|d| d["name"].as_str().unwrap_or("?")).collect::<Vec<_>>());

    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"ResilientClient"), "Should contain ResilientClient");
    assert!(names.contains(&"QueryService"), "Should contain QueryService");
    assert!(names.contains(&"ProxyClient"), "Should contain ProxyClient");

    // Verify termBreakdown is present in summary
    let summary = &output["summary"];
    assert!(summary["termBreakdown"].is_object(),
        "Summary should contain termBreakdown for comma-separated name queries");
}

#[test]
fn test_search_definitions_comma_separated_name_partial_match() {
    // Analytics Test 1.5 variant: some terms match, some don't.
    let ctx = make_ctx_with_defs();

    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "ResilientClient,NonExistentClass123,QueryService",
        "kind": "class"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Should find 2 classes (NonExistentClass123 doesn't exist)
    assert_eq!(defs.len(), 2,
        "Expected 2 classes (NonExistentClass123 shouldn't match), got {}", defs.len());
    let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
    assert!(names.contains(&"ResilientClient"));
    assert!(names.contains(&"QueryService"));
}

#[test]
fn test_search_definitions_case_insensitive_name() {
    // Analytics Test 42.4: Wrong casing (lowercase input).
    // name="resilientclient" should find "ResilientClient" (case-insensitive).
    let ctx = make_ctx_with_defs();

    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "resilientclient",
        "kind": "class"
    }));
    assert!(!result.is_error, "Case-insensitive name search should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    assert_eq!(defs.len(), 1, "Expected 1 class for case-insensitive search, got {}", defs.len());
    assert_eq!(defs[0]["name"], "ResilientClient",
        "Lowercase 'resilientclient' should match PascalCase 'ResilientClient'");
}

#[test]
fn test_search_definitions_case_insensitive_name_mixed() {
    // Variant: mixed case input like "QUERYSERVICE" should find "QueryService"
    let ctx = make_ctx_with_defs();

    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "QUERYSERVICE",
        "kind": "class"
    }));
    assert!(!result.is_error);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    assert_eq!(defs.len(), 1, "Expected 1 class for UPPERCASE search, got {}", defs.len());
    assert_eq!(defs[0]["name"], "QueryService");
}

#[test]
fn test_contains_line_outside_any_definition() {
    // Analytics Test 3.3: containsLine on a line that's inside the file
    // but outside any class/method definition (e.g., import line).
    // ResilientClient class spans 1-300, ExecuteQueryAsync spans 240-260.
    // Line 301 is past the end of the class — outside any definition.
    let ctx = make_ctx_with_defs();

    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "ResilientClient",
        "containsLine": 301
    }));
    assert!(!result.is_error, "containsLine=301 should not error");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();
    assert!(defs.is_empty(),
        "Line 301 (past end of class) should return 0 results, got {}", defs.len());
}

#[test]
fn test_contains_line_inside_class_but_outside_method() {
    // Line 270 is inside ResilientClient (1-300) but outside ExecuteQueryAsync (240-260).
    // Should return only the class, not a method.
    let ctx = make_ctx_with_defs();

    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "file": "ResilientClient",
        "containsLine": 270
    }));
    assert!(!result.is_error, "containsLine=270 should not error");
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["containingDefinitions"].as_array().unwrap();

    assert!(!defs.is_empty(), "Line 270 should be inside ResilientClient class");
    // Should find the class but NOT a method
    let class_defs: Vec<&Value> = defs.iter().filter(|d| d["kind"] == "class").collect();
    let method_defs: Vec<&Value> = defs.iter().filter(|d| d["kind"] == "method").collect();
    assert_eq!(class_defs.len(), 1, "Should find exactly 1 containing class");
    assert_eq!(class_defs[0]["name"], "ResilientClient");
    assert!(method_defs.is_empty(),
        "Line 270 is between methods, should not find any method. Got: {:?}",
        method_defs.iter().map(|d| d["name"].as_str()).collect::<Vec<_>>());
}

#[test]
fn test_search_definitions_empty_intersection_all_valid_params() {
    // Analytics Test 10.1: All valid parameters but targeting non-existent
    // combinations should return 0 results without crash.
    let ctx = make_ctx_with_defs();

    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "name": "ExecuteQueryAsync",
        "kind": "method",
        "file": "NonExistentPath",
        "parent": "NonExistentClass"
    }));
    assert!(!result.is_error, "Empty intersection should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 0,
        "All valid params with non-intersecting filters should return 0 results, got {}", defs.len());
}

#[test]
fn test_search_definitions_sort_by_cognitive_complexity() {
    // Analytics Test 8.1: sortBy="cognitiveComplexity" returns results
    // sorted by complexity descending.
    // We need to set up code_stats in the definition index.
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["C:\\src\\Services.cs".to_string()],
        index: HashMap::new(), total_tokens: 50,
        extensions: vec!["cs".to_string()],
        file_token_counts: vec![200],
        ..Default::default()
    };

    let definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "SimpleMethod".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 15,
            parent: Some("DataService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "ComplexMethod".to_string(),
            kind: DefinitionKind::Method, line_start: 20, line_end: 80,
            parent: Some("DataService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "MediumMethod".to_string(),
            kind: DefinitionKind::Method, line_start: 85, line_end: 120,
            parent: Some("DataService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "DataService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 200,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
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
    path_to_id.insert(PathBuf::from("C:\\src\\Services.cs"), 0);

    // Create code stats: ComplexMethod(50) > MediumMethod(15) > SimpleMethod(2)
    let mut code_stats: HashMap<u32, CodeStats> = HashMap::new();
    code_stats.insert(0, CodeStats { cognitive_complexity: 2, cyclomatic_complexity: 3, ..Default::default() });   // SimpleMethod
    code_stats.insert(1, CodeStats { cognitive_complexity: 50, cyclomatic_complexity: 25, ..Default::default() });  // ComplexMethod
    code_stats.insert(2, CodeStats { cognitive_complexity: 15, cyclomatic_complexity: 10, ..Default::default() });  // MediumMethod

    let def_index = DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["cs".to_string()],
        files: vec!["C:\\src\\Services.cs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index, path_to_id, method_calls: HashMap::new(),
        code_stats,
        ..Default::default()
    };

    let ctx = HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        ..Default::default()
    };

    let result = dispatch_tool(&ctx, "search_definitions", &json!({
        "sortBy": "cognitiveComplexity",
        "kind": "method",
        "maxResults": 3
    }));
    assert!(!result.is_error, "sortBy should not error: {}", result.content[0].text);
    let output: Value = serde_json::from_str(&result.content[0].text).unwrap();
    let defs = output["definitions"].as_array().unwrap();

    // Should return methods sorted by cognitive complexity descending
    assert_eq!(defs.len(), 3, "Expected 3 methods, got {}", defs.len());
    assert_eq!(defs[0]["name"], "ComplexMethod",
        "First result should be ComplexMethod (cognitive=50), got {}", defs[0]["name"]);
    assert_eq!(defs[1]["name"], "MediumMethod",
        "Second result should be MediumMethod (cognitive=15), got {}", defs[1]["name"]);
    assert_eq!(defs[2]["name"], "SimpleMethod",
        "Third result should be SimpleMethod (cognitive=2), got {}", defs[2]["name"]);

    // Verify code stats are included in the response (nested under "codeStats")
    assert!(defs[0].get("codeStats").is_some(),
        "sortBy should auto-enable includeCodeStats");
    assert_eq!(defs[0]["codeStats"]["cognitiveComplexity"].as_u64().unwrap(), 50);
}
