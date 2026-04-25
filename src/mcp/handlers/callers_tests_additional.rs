#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use super::*;
use crate::definitions::{DefinitionEntry, DefinitionIndex, DefinitionKind};
use std::collections::HashMap;

fn make_def_index_simple(definitions: Vec<DefinitionEntry>) -> DefinitionIndex {
    let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
    let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["sql".to_string()],
        files: vec!["file.sql".to_string()],
        definitions,
        name_index,
        kind_index,
        file_index,
        ..Default::default()
    }
}

fn sp_def_simple(file_id: u32, name: &str, schema: &str, line: u32) -> DefinitionEntry {
    DefinitionEntry {
        file_id,
        name: name.to_string(),
        kind: DefinitionKind::StoredProcedure,
        line_start: line,
        line_end: line + 10,
        parent: Some(schema.to_string()),
        signature: None,
        modifiers: vec![],
        attributes: vec![],
        base_types: vec![],
    }
}

fn sqlfn_def_simple(file_id: u32, name: &str, schema: &str, line: u32) -> DefinitionEntry {
    DefinitionEntry {
        file_id,
        name: name.to_string(),
        kind: DefinitionKind::SqlFunction,
        line_start: line,
        line_end: line + 10,
        parent: Some(schema.to_string()),
        signature: None,
        modifiers: vec![],
        attributes: vec![],
        base_types: vec![],
    }
}

// ─── find_target_line — SQL kinds ────────────────────────────────

#[test]
fn test_find_target_line_stored_procedure() {
    let definitions = vec![sp_def_simple(0, "usp_GetOrders", "dbo", 5)];
    let def_idx = make_def_index_simple(definitions);
    assert_eq!(find_target_line(&def_idx, "usp_getorders", None), Some(5));
}

#[test]
fn test_find_target_line_sql_function() {
    let definitions = vec![sqlfn_def_simple(0, "fn_CalcTotal", "dbo", 15)];
    let def_idx = make_def_index_simple(definitions);
    assert_eq!(find_target_line(&def_idx, "fn_calctotal", None), Some(15));
}

#[test]
fn test_find_target_line_sp_with_schema_filter() {
    let definitions = vec![
        sp_def_simple(0, "usp_Process", "dbo", 5),
        sp_def_simple(0, "usp_Process", "Sales", 20),
    ];
    let def_idx = make_def_index_simple(definitions);
    assert_eq!(find_target_line(&def_idx, "usp_process", Some("Sales")), Some(20));
    assert_eq!(find_target_line(&def_idx, "usp_process", Some("dbo")), Some(5));
}

#[test]
fn test_find_target_line_case_insensitive_class() {
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "DoWork".to_string(),
        kind: DefinitionKind::Method,
        line_start: 10,
        line_end: 20,
        parent: Some("OrderService".to_string()),
        signature: None,
        modifiers: vec![],
        attributes: vec![],
        base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    // Case-insensitive parent match
    assert_eq!(find_target_line(&def_idx, "dowork", Some("ORDERSERVICE")), Some(10));
    assert_eq!(find_target_line(&def_idx, "dowork", Some("orderservice")), Some(10));
}

// ─── collect_definition_locations — SQL kinds ───────────────────

#[test]
fn test_collect_definition_locations_includes_sp() {
    let definitions = vec![sp_def_simple(0, "usp_GetOrders", "dbo", 5)];
    let def_idx = make_def_index_simple(definitions);
    let locs = collect_definition_locations(&def_idx, "usp_getorders");
    assert_eq!(locs.len(), 1);
    assert!(locs.contains(&(0, 5)));
}

#[test]
fn test_collect_definition_locations_includes_sql_function() {
    let definitions = vec![sqlfn_def_simple(0, "fn_CalcTotal", "dbo", 15)];
    let def_idx = make_def_index_simple(definitions);
    let locs = collect_definition_locations(&def_idx, "fn_calctotal");
    assert_eq!(locs.len(), 1);
    assert!(locs.contains(&(0, 15)));
}

// ─── passes_caller_file_filters — additional edge cases ─────────



// ─── dedup_caller_tree tests ────────────────────────────────────────

#[test]
fn test_dedup_caller_tree_empty() {
    let tree: Vec<serde_json::Value> = vec![];
    let result = dedup_caller_tree(tree);
    assert!(result.is_empty(), "Empty input should produce empty output");
}

#[test]
fn test_dedup_caller_tree_no_duplicates() {
    let tree = vec![
        json!({"class": "ClassA", "method": "doWork", "file": "a.cs", "line": 10}),
        json!({"class": "ClassB", "method": "run", "file": "b.cs", "line": 20}),
        json!({"class": "ClassC", "method": "init", "file": "c.cs", "line": 30}),
    ];
    let result = dedup_caller_tree(tree);
    assert_eq!(result.len(), 3, "All 3 distinct nodes should be retained");
}

#[test]
fn test_dedup_caller_tree_all_duplicates() {
    let tree = vec![
        json!({"class": "ClassA", "method": "doWork", "file": "a.cs", "line": 10}),
        json!({"class": "ClassA", "method": "doWork", "file": "a.cs", "line": 10}),
        json!({"class": "ClassA", "method": "doWork", "file": "a.cs", "line": 10}),
    ];
    let result = dedup_caller_tree(tree);
    assert_eq!(result.len(), 1, "All duplicates should collapse to 1");
}

#[test]
fn test_dedup_caller_tree_mixed() {
    let tree = vec![
        json!({"class": "ClassA", "method": "doWork", "file": "a.cs", "line": 10}),
        json!({"class": "ClassB", "method": "run", "file": "b.cs", "line": 20}),
        json!({"class": "ClassA", "method": "doWork", "file": "a.cs", "line": 10}), // dup of [0]
        json!({"class": "ClassC", "method": "init", "file": "c.cs", "line": 30}),
        json!({"class": "ClassB", "method": "run", "file": "b.cs", "line": 20}),    // dup of [1]
    ];
    let result = dedup_caller_tree(tree);
    assert_eq!(result.len(), 3, "5 nodes with 2 duplicates should become 3");
    // Order should be preserved (first occurrence kept)
    assert_eq!(result[0]["class"].as_str().unwrap(), "ClassA");
    assert_eq!(result[1]["class"].as_str().unwrap(), "ClassB");
    assert_eq!(result[2]["class"].as_str().unwrap(), "ClassC");
}

#[test]
fn test_dedup_caller_tree_missing_fields() {
    // Nodes without class/file/line should use default "?" / 0
    let tree = vec![
        json!({"method": "doWork"}),                     // no class, file, line
        json!({"method": "doWork"}),                     // same defaults → duplicate
        json!({"method": "run", "class": "ClassA"}),     // no file, line
        json!({"method": "run", "class": "ClassA"}),     // same → duplicate
    ];
    let result = dedup_caller_tree(tree);
    assert_eq!(result.len(), 2, "Two distinct default-key nodes expected, got {:?}", result);
}

#[test]
fn test_dedup_caller_tree_same_method_different_line() {
    // Same class+method+file but different line = different callers (not duplicates)
    let tree = vec![
        json!({"class": "ClassA", "method": "doWork", "file": "a.cs", "line": 10}),
        json!({"class": "ClassA", "method": "doWork", "file": "a.cs", "line": 20}),
    ];
    let result = dedup_caller_tree(tree);
    assert_eq!(result.len(), 2, "Same method at different lines should NOT be deduped");
}

// ─── collect_substring_file_ids tests ───────────────────────────────

#[test]
fn test_collect_substring_file_ids_short_term() {
    // Terms shorter than 3 chars should be no-op (trigrams require >= 3 chars)
    use crate::{ContentIndex, Posting};
    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["f.cs".to_string()],
        index: {
            let mut m = std::collections::HashMap::new();
            m.insert("ab".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
            m
        },
        ..Default::default()
    };
    let mut file_ids = std::collections::HashSet::new();
    collect_substring_file_ids("ab", &index, &mut file_ids);
    assert!(file_ids.is_empty(), "Term < 3 chars should not add any file_ids");
}

#[test]
fn test_collect_substring_file_ids_empty_trigram_index() {
    // When trigram index has empty tokens list, should be no-op
    use crate::ContentIndex;
    let index = ContentIndex {
        root: ".".to_string(),
        ..Default::default()
    };
    assert!(index.trigram.tokens.is_empty());
    let mut file_ids = std::collections::HashSet::new();
    collect_substring_file_ids("storage", &index, &mut file_ids);
    assert!(file_ids.is_empty(), "Empty trigram index should not add any file_ids");
}

#[test]
fn test_collect_substring_file_ids_finds_substrings() {
    // Token "m_storagemanager" contains "storage" as a substring (and is longer)
    use crate::{ContentIndex, Posting, TrigramIndex};
    use code_xray::generate_trigrams;

    // Build a trigram index with a token that contains our search term
    let mut trigram_map: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
    let tokens = vec!["m_storagemanager".to_string(), "logger".to_string()];
    for (idx, token) in tokens.iter().enumerate() {
        for tri in generate_trigrams(token) {
            trigram_map.entry(tri).or_default().push(idx as u32);
        }
    }
    // Sort and dedup posting lists
    for list in trigram_map.values_mut() {
        list.sort();
        list.dedup();
    }

    // Build content index with the tokens
    let mut inverted = std::collections::HashMap::new();
    inverted.insert("m_storagemanager".to_string(), vec![Posting { file_id: 0, lines: vec![5] }]);
    inverted.insert("logger".to_string(), vec![Posting { file_id: 1, lines: vec![3] }]);

    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["storage.cs".to_string(), "log.cs".to_string()],
        index: inverted,
        trigram: TrigramIndex { tokens, trigram_map },
        ..Default::default()
    };

    let mut file_ids = std::collections::HashSet::new();
    collect_substring_file_ids("storage", &index, &mut file_ids);
    // "storage" is a substring of "m_storagemanager" (file_id 0)
    // "storage" is NOT a substring of "logger"
    assert!(file_ids.contains(&0), "Should find file_id 0 (m_storagemanager contains 'storage')");
    assert!(!file_ids.contains(&1), "Should not find file_id 1 (logger doesn't contain 'storage')");
}

#[test]
fn test_collect_substring_file_ids_excludes_exact_match() {
    // Exact match (same length) should NOT be included — only strictly LONGER tokens
    use crate::{ContentIndex, Posting, TrigramIndex};
    use code_xray::generate_trigrams;

    let tokens = vec!["storage".to_string()];
    let mut trigram_map: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
    for tri in generate_trigrams("storage") {
        trigram_map.entry(tri).or_default().push(0);
    }

    let mut inverted = std::collections::HashMap::new();
    inverted.insert("storage".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);

    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["f.cs".to_string()],
        index: inverted,
        trigram: TrigramIndex { tokens, trigram_map },
        ..Default::default()
    };

    let mut file_ids = std::collections::HashSet::new();
    collect_substring_file_ids("storage", &index, &mut file_ids);
    // "storage" == "storage" (same length) → should be excluded
    assert!(file_ids.is_empty(), "Exact match should not be included in substring results");
}

#[test]
fn test_collect_substring_file_ids_no_trigram_match() {
    // Term's trigrams are not found in the trigram index → no results
    use crate::{ContentIndex, Posting, TrigramIndex};
    use code_xray::generate_trigrams;

    let tokens = vec!["httpclient".to_string()];
    let mut trigram_map: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
    for tri in generate_trigrams("httpclient") {
        trigram_map.entry(tri).or_default().push(0);
    }

    let mut inverted = std::collections::HashMap::new();
    inverted.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);

    let index = ContentIndex {
        root: ".".to_string(),
        files: vec!["f.cs".to_string()],
        index: inverted,
        trigram: TrigramIndex { tokens, trigram_map },
        ..Default::default()
    };

    let mut file_ids = std::collections::HashSet::new();
    // "zzzzz" trigrams won't be found in the index
    collect_substring_file_ids("zzzzz", &index, &mut file_ids);
    assert!(file_ids.is_empty(), "Non-matching term should produce no results");
}

// ─── is_class_generic tests ────────────────────────────────────────

#[test]
fn test_is_class_generic_with_angle_brackets() {
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "DataList".to_string(),
        kind: DefinitionKind::Class,
        line_start: 1, line_end: 50,
        parent: None,
        signature: Some("public class DataList<T>".to_string()),
        modifiers: vec![], attributes: vec![], base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    assert!(is_class_generic(&def_idx, "datalist"), "Class with <T> in signature should be generic");
}

#[test]
fn test_is_class_generic_without_angle_brackets() {
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "DataList".to_string(),
        kind: DefinitionKind::Class,
        line_start: 1, line_end: 50,
        parent: None,
        signature: Some("internal sealed class DataList : DataRegion".to_string()),
        modifiers: vec![], attributes: vec![], base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    assert!(!is_class_generic(&def_idx, "datalist"), "Class without < in signature should NOT be generic");
}

#[test]
fn test_is_class_generic_no_signature() {
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "DataList".to_string(),
        kind: DefinitionKind::Class,
        line_start: 1, line_end: 50,
        parent: None,
        signature: None,
        modifiers: vec![], attributes: vec![], base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    assert!(!is_class_generic(&def_idx, "datalist"), "Class with no signature should NOT be generic");
}

#[test]
fn test_is_class_generic_not_found() {
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "SomeClass".to_string(),
        kind: DefinitionKind::Class,
        line_start: 1, line_end: 50,
        parent: None,
        signature: Some("public class SomeClass<T>".to_string()),
        modifiers: vec![], attributes: vec![], base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    assert!(!is_class_generic(&def_idx, "nonexistent"), "Non-existent class should return false");
}

#[test]
fn test_is_class_generic_interface_kind() {
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "IRepository".to_string(),
        kind: DefinitionKind::Interface,
        line_start: 1, line_end: 30,
        parent: None,
        signature: Some("public interface IRepository<T>".to_string()),
        modifiers: vec![], attributes: vec![], base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    assert!(is_class_generic(&def_idx, "irepository"), "Interface with generic signature should be detected");
}

// ─── is_test_file tests ─────────────────────────────────────────────

#[test]
fn test_is_test_file_rust_convention() {
    assert!(is_test_file("src/mcp/handlers/utils_tests.rs"));
    assert!(is_test_file("src/lib_tests.rs"));
    assert!(is_test_file("C:\\Repos\\project\\src\\callers_tests.rs"));
}

#[test]
fn test_is_test_file_typescript_conventions() {
    assert!(is_test_file("src/app.test.ts"));
    assert!(is_test_file("src/app.spec.ts"));
    assert!(is_test_file("src/app.test.tsx"));
    assert!(is_test_file("src/app.spec.tsx"));
}

#[test]
fn test_is_test_file_directory_conventions() {
    assert!(is_test_file("src/tests/service.rs"));
    assert!(is_test_file("src/test/service.rs"));
    assert!(is_test_file("C:\\src\\tests\\service.cs"));
    assert!(is_test_file("C:\\src\\test\\service.cs"));
}

#[test]
fn test_is_test_file_production_files() {
    assert!(!is_test_file("src/mcp/handlers/utils.rs"));
    assert!(!is_test_file("src/main.rs"));
    assert!(!is_test_file("C:\\src\\Service.cs"));
    assert!(!is_test_file("src/app.ts"));
}

#[test]
fn test_is_test_file_case_insensitive() {
    assert!(is_test_file("src/App_Tests.rs"));
    assert!(is_test_file("src/App.Test.ts"));
    assert!(is_test_file("src/Tests/Service.cs"));
}

// ─── is_test_caller tests ───────────────────────────────────────────

#[test]
fn test_is_test_caller_by_file_path() {
    // Method in test file should be detected as test caller
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "test_something".to_string(),
        kind: DefinitionKind::Method,
        line_start: 10, line_end: 20,
        parent: None,
        signature: None,
        modifiers: vec![], attributes: vec![], base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    assert!(is_test_caller(&def_idx, 0, "src/utils_tests.rs"));
}

#[test]
fn test_is_test_caller_by_attribute() {
    // Method with #[test] attribute in production file
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "test_something".to_string(),
        kind: DefinitionKind::Method,
        line_start: 10, line_end: 20,
        parent: None,
        signature: None,
        modifiers: vec![], attributes: vec!["test".to_string()], base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    assert!(is_test_caller(&def_idx, 0, "src/utils.rs"));
}

#[test]
fn test_is_test_caller_production() {
    // Production method in production file
    let definitions = vec![DefinitionEntry {
        file_id: 0,
        name: "process_data".to_string(),
        kind: DefinitionKind::Method,
        line_start: 10, line_end: 20,
        parent: Some("DataService".to_string()),
        signature: None,
        modifiers: vec![], attributes: vec![], base_types: vec![],
    }];
    let def_idx = make_def_index_simple(definitions);
    assert!(!is_test_caller(&def_idx, 0, "src/data_service.rs"));
}

// ─── caller_popularity tests ────────────────────────────────────────

#[test]
fn test_caller_popularity_basic() {
    use crate::{ContentIndex, Posting};
    let mut index_map = std::collections::HashMap::new();
    index_map.insert("popular_method".to_string(), vec![
        Posting { file_id: 0, lines: vec![10, 20, 30] },
        Posting { file_id: 1, lines: vec![5, 15] },
    ]);
    index_map.insert("rare_method".to_string(), vec![
        Posting { file_id: 0, lines: vec![50] },
    ]);
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["a.rs".to_string(), "b.rs".to_string()],
        index: index_map,
        ..Default::default()
    };
    assert_eq!(caller_popularity(&content_index, "popular_method"), 5);
    assert_eq!(caller_popularity(&content_index, "rare_method"), 1);
    assert_eq!(caller_popularity(&content_index, "nonexistent"), 0);
}

#[test]
fn test_caller_popularity_case_insensitive() {
    use crate::{ContentIndex, Posting};
    let mut index_map = std::collections::HashMap::new();
    index_map.insert("mymethod".to_string(), vec![
        Posting { file_id: 0, lines: vec![10, 20] },
    ]);
    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec!["a.rs".to_string()],
        index: index_map,
        ..Default::default()
    };
    // Should match case-insensitively
    assert_eq!(caller_popularity(&content_index, "MyMethod"), 2);
    assert_eq!(caller_popularity(&content_index, "MYMETHOD"), 2);
}

// ─── Test deprioritization integration tests ────────────────────────
// These test the full xray_callers handler with production + test callers

/// Build a HandlerContext with a mix of production and test callers.
/// Layout: file0=Utils.rs (production, has target method + 2 production callers),
///         file1=Utils_tests.rs (test file, has 8 test callers)
fn make_ctx_test_deprioritization() -> super::super::HandlerContext {
    use crate::{ContentIndex, Posting};
    use std::sync::{Arc, RwLock};
    use std::path::PathBuf;

    // Target method: truncate_data (in file0=Utils.rs)
    // Production callers: process_data (line 50), inject_metrics (line 80) — both in Utils.rs
    // Test callers: test_1..test_8 — all in Utils_tests.rs
    let mut content_idx = std::collections::HashMap::new();

    // truncate_data token — appears in production file (definition + 2 callers)
    // and in test file (8 callers)
    content_idx.insert("truncate_data".to_string(), vec![
        Posting { file_id: 0, lines: vec![10, 50, 80] },   // file0: def at 10, called from 50, 80
        Posting { file_id: 1, lines: vec![19, 39, 59, 79, 99, 119, 139, 159] }, // file1: 8 test callers (mid-span of each test)
    ]);

    // Production caller names — process_data is "more popular" (more references)
    content_idx.insert("process_data".to_string(), vec![
        Posting { file_id: 0, lines: vec![45, 50, 100, 120, 150] }, // 5 lines = popular
    ]);
    content_idx.insert("inject_metrics".to_string(), vec![
        Posting { file_id: 0, lines: vec![75, 80] }, // 2 lines = less popular
    ]);

    // Test method names — each referenced just once
    for i in 1..=8 {
        let name = format!("test_{}", i);
        content_idx.insert(name, vec![
            Posting { file_id: 1, lines: vec![i * 10 + 5] },
        ]);
    }

    // Also add dataservice token for file filtering
    content_idx.insert("dataservice".to_string(), vec![
        Posting { file_id: 0, lines: vec![1] },
    ]);

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:/src/Utils.rs".to_string(),
            "C:/src/utils_tests.rs".to_string(),
        ],
        index: content_idx,
        total_tokens: 100,
        extensions: vec!["rs".to_string()],
        file_token_counts: vec![50, 50],
        ..Default::default()
    };

    // Build definitions:
    // file0 (Utils.rs): DataService class + truncate_data method + process_data + inject_metrics
    // file1 (utils_tests.rs): test_1..test_8 methods (Rust #[test] attribute)
    let mut definitions = vec![
        // di=0: class
        DefinitionEntry {
            file_id: 0, name: "DataService".to_string(),
            kind: DefinitionKind::Class, line_start: 1, line_end: 200,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // di=1: target method (definition at line 10)
        DefinitionEntry {
            file_id: 0, name: "truncate_data".to_string(),
            kind: DefinitionKind::Method, line_start: 10, line_end: 30,
            parent: Some("DataService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // di=2: production caller #1 — contains truncate_data call at line 50
        DefinitionEntry {
            file_id: 0, name: "process_data".to_string(),
            kind: DefinitionKind::Method, line_start: 45, line_end: 60,
            parent: Some("DataService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        // di=3: production caller #2 — contains truncate_data call at line 80
        DefinitionEntry {
            file_id: 0, name: "inject_metrics".to_string(),
            kind: DefinitionKind::Method, line_start: 75, line_end: 90,
            parent: Some("DataService".to_string()), signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    // di=4..11: test callers (8 tests in utils_tests.rs)
    // Each test spans 20 lines: test_1=[10,29], test_2=[30,49], ..., test_8=[150,169]
    // Call sites at mid-span: 15, 25-not-used, instead use 15,35,55,75,95,115,135,155
    for i in 1u32..=8 {
        let line_start = 10 + (i - 1) * 20;
        let line_end = line_start + 19;
        definitions.push(DefinitionEntry {
            file_id: 1,
            name: format!("test_{}", i),
            kind: DefinitionKind::Function,
            line_start, line_end,
            parent: None, signature: None,
            modifiers: vec![],
            attributes: vec!["test".to_string()], // Rust #[test]
            base_types: vec![],
        });
    }

    let mut name_index: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
    let mut kind_index: std::collections::HashMap<DefinitionKind, Vec<u32>> = std::collections::HashMap::new();
    let mut file_index: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let mut path_to_id: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();

    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }

    path_to_id.insert(PathBuf::from("C:/src/Utils.rs"), 0);
    path_to_id.insert(PathBuf::from("C:/src/utils_tests.rs"), 1);

    let def_index = crate::definitions::DefinitionIndex {
        root: ".".to_string(),
        created_at: 0,
        extensions: vec!["rs".to_string()],
        files: vec![
            "C:/src/Utils.rs".to_string(),
            "C:/src/utils_tests.rs".to_string(),
        ],
        definitions,
        name_index,
        kind_index,
        attribute_index: std::collections::HashMap::new(),
        base_type_index: std::collections::HashMap::new(),
        file_index,
        path_to_id,
        ..Default::default()
    };

    super::super::HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "rs".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_callers_deprioritize_tests_production_first() {
    // 2 production + 8 test callers, maxCallersPerLevel=10
    // Should return: 2 production first, then 8 test callers
    let ctx = make_ctx_test_deprioritization();
    let result = super::super::dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["truncate_data"],
        "depth": 1,
        "maxCallersPerLevel": 10,
        "resolveInterfaces": false
    }));
    assert!(!result.is_error, "Should not error: {}", result.content[0].text);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // Should have all 10 callers (2 prod + 8 test)
    assert_eq!(tree.len(), 10, "Should have 10 callers, got {}: {}",
        tree.len(), serde_json::to_string_pretty(&tree).unwrap());

    // First 2 should be production callers (non-test)
    let first = tree[0]["method"].as_str().unwrap();
    let second = tree[1]["method"].as_str().unwrap();
    assert!(!first.starts_with("test_"), "First caller should be production, got: {}", first);
    assert!(!second.starts_with("test_"), "Second caller should be production, got: {}", second);

    // Remaining should be test callers
    for entry in tree.iter().take(10).skip(2) {
        let name = entry["method"].as_str().unwrap();
        assert!(name.starts_with("test_"), "Caller should be test, got: {}", name);
    }
}

#[test]
fn test_callers_deprioritize_tests_truncation() {
    // 2 production + 8 test callers, maxCallersPerLevel=5
    // Should return: 2 production + 3 test (5 total)
    let ctx = make_ctx_test_deprioritization();
    let result = super::super::dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["truncate_data"],
        "depth": 1,
        "maxCallersPerLevel": 5,
        "resolveInterfaces": false
    }));
    assert!(!result.is_error, "Should not error: {}", result.content[0].text);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    assert_eq!(tree.len(), 5, "Should have 5 callers after truncation, got {}",
        tree.len());

    // First 2 should be production
    let prod_count = tree.iter()
        .filter(|n| !n["method"].as_str().unwrap().starts_with("test_"))
        .count();
    assert_eq!(prod_count, 2, "Should have exactly 2 production callers");

    // Remaining 3 should be test
    let test_count = tree.iter()
        .filter(|n| n["method"].as_str().unwrap().starts_with("test_"))
        .count();
    assert_eq!(test_count, 3, "Should have 3 test callers after truncation");
}

#[test]
fn test_callers_popularity_sort_within_production() {
    // process_data has 5 postings, inject_metrics has 2 postings
    // process_data should come before inject_metrics
    let ctx = make_ctx_test_deprioritization();
    let result = super::super::dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["truncate_data"],
        "depth": 1,
        "maxCallersPerLevel": 10,
        "resolveInterfaces": false
    }));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // production callers should be sorted by popularity DESC
    let first = tree[0]["method"].as_str().unwrap();
    let second = tree[1]["method"].as_str().unwrap();
    assert_eq!(first, "process_data", "More popular caller should be first");
    assert_eq!(second, "inject_metrics", "Less popular caller should be second");
}

#[test]
fn test_callers_impact_analysis_keeps_all_tests() {
    // With impactAnalysis=true, test callers should NOT be truncated
    // 2 production + 8 test callers, maxCallersPerLevel=5
    // Without impactAnalysis: 2 prod + 3 test = 5
    // With impactAnalysis: 2 prod + 8 test = 10 (all tests kept)
    let ctx = make_ctx_test_deprioritization();
    let result = super::super::dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["truncate_data"],
        "depth": 1,
        "maxCallersPerLevel": 5,
        "impactAnalysis": true,
        "resolveInterfaces": false
    }));
    assert!(!result.is_error, "Should not error: {}", result.content[0].text);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // With impactAnalysis, all test callers should be preserved
    let test_count = tree.iter()
        .filter(|n| n["method"].as_str().unwrap().starts_with("test_"))
        .count();
    let prod_count = tree.iter()
        .filter(|n| !n["method"].as_str().unwrap().starts_with("test_"))
        .count();

    assert_eq!(prod_count, 2, "Should have 2 production callers");
    assert_eq!(test_count, 8, "All 8 test callers should be preserved with impactAnalysis=true");

    // testsCovering should also have all 8 tests
    let tests_covering = output["testsCovering"].as_array().unwrap();
    assert_eq!(tests_covering.len(), 8, "testsCovering should have 8 tests");
}

#[test]
fn test_callers_only_tests_no_production() {
    // When all callers are tests, they should still be returned
    use crate::{ContentIndex, Posting};
    use std::sync::{Arc, RwLock};
    use std::path::PathBuf;

    let mut content_idx = std::collections::HashMap::new();
    content_idx.insert("helper_fn".to_string(), vec![
        Posting { file_id: 0, lines: vec![10] },   // definition
        Posting { file_id: 1, lines: vec![20, 30, 40, 50, 60] }, // 5 test callers
    ]);
    for i in 1..=5 {
        content_idx.insert(format!("test_{}", i), vec![
            Posting { file_id: 1, lines: vec![i * 10 + 5] },
        ]);
    }

    let content_index = ContentIndex {
        root: ".".to_string(),
        files: vec![
            "C:/src/helpers.rs".to_string(),
            "C:/src/helpers_tests.rs".to_string(),
        ],
        index: content_idx,
        total_tokens: 50,
        extensions: vec!["rs".to_string()],
        file_token_counts: vec![25, 25],
        ..Default::default()
    };

    let mut definitions = vec![
        DefinitionEntry {
            file_id: 0, name: "helper_fn".to_string(),
            kind: DefinitionKind::Function, line_start: 10, line_end: 20,
            parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];
    for i in 1u32..=5 {
        let line = i * 10;
        definitions.push(DefinitionEntry {
            file_id: 1,
            name: format!("test_{}", i),
            kind: DefinitionKind::Function,
            line_start: line, line_end: line + 8,
            parent: None, signature: None,
            modifiers: vec![],
            attributes: vec!["test".to_string()],
            base_types: vec![],
        });
    }

    let mut name_index: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
    let mut kind_index: std::collections::HashMap<DefinitionKind, Vec<u32>> = std::collections::HashMap::new();
    let mut file_index: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let mut path_to_id: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();
    for (i, def) in definitions.iter().enumerate() {
        let idx = i as u32;
        name_index.entry(def.name.to_lowercase()).or_default().push(idx);
        kind_index.entry(def.kind).or_default().push(idx);
        file_index.entry(def.file_id).or_default().push(idx);
    }
    path_to_id.insert(PathBuf::from("C:/src/helpers.rs"), 0);
    path_to_id.insert(PathBuf::from("C:/src/helpers_tests.rs"), 1);

    let def_index = crate::definitions::DefinitionIndex {
        root: ".".to_string(), created_at: 0,
        extensions: vec!["rs".to_string()],
        files: vec!["C:/src/helpers.rs".to_string(), "C:/src/helpers_tests.rs".to_string()],
        definitions, name_index, kind_index,
        attribute_index: std::collections::HashMap::new(),
        base_type_index: std::collections::HashMap::new(),
        file_index, path_to_id,
        ..Default::default()
    };

    let ctx = super::super::HandlerContext {
        index: Arc::new(RwLock::new(content_index)),
        def_index: Some(Arc::new(RwLock::new(def_index))),
        server_ext: "rs".to_string(),
        ..Default::default()
    };

    let result = super::super::dispatch_tool(&ctx, "xray_callers", &json!({
        "method": ["helper_fn"],
        "depth": 1,
        "maxCallersPerLevel": 3,
        "resolveInterfaces": false
    }));
    assert!(!result.is_error);
    let output: serde_json::Value = serde_json::from_str(&result.content[0].text).unwrap();
    let tree = output["callTree"].as_array().unwrap();

    // 0 production callers + 5 test callers, maxCallersPerLevel=3 → 3 test callers
    assert_eq!(tree.len(), 3, "Should have 3 test callers, got {}", tree.len());
    for node in tree {
        let name = node["method"].as_str().unwrap();
        assert!(name.starts_with("test_"), "All callers should be tests, got: {}", name);
    }
}



// ─── P1 Group B: CallerTreeBuilder state tests ─────────────────────
// These tests verify that the Builder correctly accumulates mutable state
// (tests_found, visited, file_cache, total_body_lines_emitted) across
// recursive build_caller_tree / build_callee_tree invocations.
// Refactoring risk: the Builder pattern was introduced to reduce CC of these
// functions from 48 → ~10. Regression risk: state could be lost or duplicated
// across recursion levels.

#[cfg(test)]
mod builder_state_tests {
    use super::*;
    use crate::definitions::{DefinitionEntry, DefinitionIndex, DefinitionKind};
    use crate::mcp::handlers::{dispatch_tool, HandlerContext, WorkspaceBinding};
    use crate::{ContentIndex, Posting};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    /// Atomic counter for unique tempdir names in this module.
    fn unique_tmp_dir(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "xray_builder_test_{}_{}_{}",
            tag,
            std::process::id(),
            id
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Build (name_index, kind_index, file_index) maps from a flat definitions vec.
    #[allow(clippy::type_complexity)]
    fn build_indexes(
        definitions: &[DefinitionEntry],
    ) -> (
        HashMap<String, Vec<u32>>,
        HashMap<DefinitionKind, Vec<u32>>,
        HashMap<u32, Vec<u32>>,
    ) {
        let mut name_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut kind_index: HashMap<DefinitionKind, Vec<u32>> = HashMap::new();
        let mut file_index: HashMap<u32, Vec<u32>> = HashMap::new();
        for (i, def) in definitions.iter().enumerate() {
            let idx = i as u32;
            name_index.entry(def.name.to_lowercase()).or_default().push(idx);
            kind_index.entry(def.kind).or_default().push(idx);
            file_index.entry(def.file_id).or_default().push(idx);
        }
        (name_index, kind_index, file_index)
    }

    // ─── B1: tests_found accumulates across multiple branches ─────────
    //
    // Graph (direction=up from target_method):
    //
    //   target_method (file0)
    //     ├─ branch_b (file0)  ← test_branch_b (file1, #[test])
    //     └─ branch_c (file0)  ← test_branch_c (file1, #[test])
    //
    // With impactAnalysis=true, both test_branch_b and test_branch_c MUST appear
    // in testsCovering[]. Regression risk: if Builder creates a fresh tests_found
    // per recursion branch instead of accumulating into &mut self, only the LAST
    // branch's tests would survive.
    #[test]
    fn test_builder_accumulates_tests_across_branches() {
        let mut content_idx = HashMap::new();
        // target_method: defined at file0:10, called at file0:25 (branch_b), file0:55 (branch_c)
        content_idx.insert(
            "target_method".to_string(),
            vec![Posting {
                file_id: 0,
                lines: vec![10, 25, 55],
            }],
        );
        // branch_b: defined at file0:20, called at file1:15 (test_branch_b)
        content_idx.insert(
            "branch_b".to_string(),
            vec![
                Posting { file_id: 0, lines: vec![20] },
                Posting { file_id: 1, lines: vec![15] },
            ],
        );
        // branch_c: defined at file0:50, called at file1:35 (test_branch_c)
        content_idx.insert(
            "branch_c".to_string(),
            vec![
                Posting { file_id: 0, lines: vec![50] },
                Posting { file_id: 1, lines: vec![35] },
            ],
        );
        // test methods: each just defined once
        content_idx.insert(
            "test_branch_b".to_string(),
            vec![Posting { file_id: 1, lines: vec![10] }],
        );
        content_idx.insert(
            "test_branch_c".to_string(),
            vec![Posting { file_id: 1, lines: vec![30] }],
        );

        let content_index = ContentIndex {
            root: ".".to_string(),
            files: vec![
                "C:/src/UserService.rs".to_string(),
                "C:/src/user_service_tests.rs".to_string(),
            ],
            index: content_idx,
            total_tokens: 100,
            extensions: vec!["rs".to_string()],
            file_token_counts: vec![50, 50],
            ..Default::default()
        };

        let definitions = vec![
            // di=0: target_method (production, file0)
            DefinitionEntry {
                file_id: 0,
                name: "target_method".to_string(),
                kind: DefinitionKind::Function,
                line_start: 10,
                line_end: 15,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            // di=1: branch_b (production, file0) — contains call at line 25
            DefinitionEntry {
                file_id: 0,
                name: "branch_b".to_string(),
                kind: DefinitionKind::Function,
                line_start: 20,
                line_end: 30,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            // di=2: branch_c (production, file0) — contains call at line 55
            DefinitionEntry {
                file_id: 0,
                name: "branch_c".to_string(),
                kind: DefinitionKind::Function,
                line_start: 50,
                line_end: 60,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            // di=3: test_branch_b (test, file1, #[test])
            DefinitionEntry {
                file_id: 1,
                name: "test_branch_b".to_string(),
                kind: DefinitionKind::Function,
                line_start: 10,
                line_end: 20,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec!["test".to_string()],
                base_types: vec![],
            },
            // di=4: test_branch_c (test, file1, #[test])
            DefinitionEntry {
                file_id: 1,
                name: "test_branch_c".to_string(),
                kind: DefinitionKind::Function,
                line_start: 30,
                line_end: 40,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec!["test".to_string()],
                base_types: vec![],
            },
        ];

        let (name_index, kind_index, file_index) = build_indexes(&definitions);
        let mut path_to_id = HashMap::new();
        path_to_id.insert(PathBuf::from("C:/src/UserService.rs"), 0);
        path_to_id.insert(PathBuf::from("C:/src/user_service_tests.rs"), 1);

        let def_index = DefinitionIndex {
            root: ".".to_string(),
            created_at: 0,
            extensions: vec!["rs".to_string()],
            files: vec![
                "C:/src/UserService.rs".to_string(),
                "C:/src/user_service_tests.rs".to_string(),
            ],
            definitions,
            name_index,
            kind_index,
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index,
            path_to_id,
            ..Default::default()
        };

        let ctx = HandlerContext {
            index: Arc::new(RwLock::new(content_index)),
            def_index: Some(Arc::new(RwLock::new(def_index))),
            server_ext: "rs".to_string(),
            ..Default::default()
        };

        let result = dispatch_tool(
            &ctx,
            "xray_callers",
            &json!({
                "method": ["target_method"],
                "depth": 3,
                "maxCallersPerLevel": 10,
                "impactAnalysis": true,
                "resolveInterfaces": false
            }),
        );
        assert!(
            !result.is_error,
            "Should not error: {}",
            result.content[0].text
        );
        let output: serde_json::Value =
            serde_json::from_str(&result.content[0].text).unwrap();

        // Direct callers should be branch_b + branch_c
        let tree = output["callTree"].as_array().unwrap();
        let direct_caller_names: Vec<&str> = tree
            .iter()
            .map(|n| n["method"].as_str().unwrap())
            .collect();
        assert!(
            direct_caller_names.contains(&"branch_b"),
            "branch_b should be a direct caller, got: {:?}",
            direct_caller_names
        );
        assert!(
            direct_caller_names.contains(&"branch_c"),
            "branch_c should be a direct caller, got: {:?}",
            direct_caller_names
        );

        // testsCovering must contain BOTH test methods (one per branch).
        // This is the regression check: if Builder.tests_found were reset
        // between branches, only one would survive.
        let tests_covering = output["testsCovering"]
            .as_array()
            .expect("testsCovering should be present with impactAnalysis=true");
        let test_names: Vec<&str> = tests_covering
            .iter()
            .map(|t| t["method"].as_str().unwrap())
            .collect();
        assert!(
            test_names.contains(&"test_branch_b"),
            "tests_found must accumulate test_branch_b across branches; got: {:?}",
            test_names
        );
        assert!(
            test_names.contains(&"test_branch_c"),
            "tests_found must accumulate test_branch_c across branches; got: {:?}",
            test_names
        );
        assert_eq!(
            tests_covering.len(),
            2,
            "Exactly 2 tests should be reported across both branches, got: {:?}",
            test_names
        );
    }

    // ─── B2: visited prevents infinite recursion on cycles ───────────
    //
    // Mutual recursion: method_a ↔ method_b. The Builder.visited set must
    // prevent revisiting an already-explored node, so the call returns in
    // bounded time even with depth=10.
    //
    // Regression risk: if the Builder created a fresh visited per recursion
    // level (instead of sharing &mut self.visited), the call would loop until
    // depth limit AND emit duplicate nodes.
    #[test]
    fn test_builder_visited_prevents_cycle() {
        let mut content_idx = HashMap::new();
        // method_a: defined at file0:10, called from file0:35 (inside method_b body)
        content_idx.insert(
            "method_a".to_string(),
            vec![Posting {
                file_id: 0,
                lines: vec![10, 35],
            }],
        );
        // method_b: defined at file0:30, called from file0:15 (inside method_a body)
        content_idx.insert(
            "method_b".to_string(),
            vec![Posting {
                file_id: 0,
                lines: vec![30, 15],
            }],
        );

        let content_index = ContentIndex {
            root: ".".to_string(),
            files: vec!["C:/src/MutualRecursion.rs".to_string()],
            index: content_idx,
            total_tokens: 50,
            extensions: vec!["rs".to_string()],
            file_token_counts: vec![50],
            ..Default::default()
        };

        let definitions = vec![
            // di=0: method_a (calls method_b inside body)
            DefinitionEntry {
                file_id: 0,
                name: "method_a".to_string(),
                kind: DefinitionKind::Function,
                line_start: 10,
                line_end: 20,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            // di=1: method_b (calls method_a inside body)
            DefinitionEntry {
                file_id: 0,
                name: "method_b".to_string(),
                kind: DefinitionKind::Function,
                line_start: 30,
                line_end: 40,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
        ];

        let (name_index, kind_index, file_index) = build_indexes(&definitions);
        let mut path_to_id = HashMap::new();
        path_to_id.insert(PathBuf::from("C:/src/MutualRecursion.rs"), 0);

        let def_index = DefinitionIndex {
            root: ".".to_string(),
            created_at: 0,
            extensions: vec!["rs".to_string()],
            files: vec!["C:/src/MutualRecursion.rs".to_string()],
            definitions,
            name_index,
            kind_index,
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index,
            path_to_id,
            ..Default::default()
        };

        let ctx = HandlerContext {
            index: Arc::new(RwLock::new(content_index)),
            def_index: Some(Arc::new(RwLock::new(def_index))),
            server_ext: "rs".to_string(),
            ..Default::default()
        };

        // Run with high depth — visited must terminate the cycle.
        let start = std::time::Instant::now();
        let result = dispatch_tool(
            &ctx,
            "xray_callers",
            &json!({
                "method": ["method_a"],
                "depth": 10,
                "maxCallersPerLevel": 10,
                "resolveInterfaces": false
            }),
        );
        let elapsed = start.elapsed();

        assert!(
            !result.is_error,
            "Should not error on cycle: {}",
            result.content[0].text
        );
        // Tight upper bound — cycle detection should finish in milliseconds.
        // If visited is broken, this would explode (exponential nodes) and
        // either hit maxTotalNodes guard slowly OR hang. 5s is generous.
        assert!(
            elapsed.as_secs() < 5,
            "Cycle detection took too long: {:?} (visited may be broken)",
            elapsed
        );

        let output: serde_json::Value =
            serde_json::from_str(&result.content[0].text).unwrap();
        let tree = output["callTree"].as_array().unwrap();

        // visited semantics: when a node is revisited, it's NOT recursed into,
        // but the node itself can still appear as a leaf in the tree.
        //
        // With visited working, depth=10 should produce a finite tree where:
        //   - method_b appears ONCE as direct caller of root method_a
        //   - method_a appears at most ONCE as caller of method_b (as a leaf,
        //     because visited.insert("method_a.10") returns false → empty callers)
        //
        // With visited broken, the tree explodes: method_a → method_b → method_a
        // → method_b → ... up to depth=10, so each method appears ~5 times.
        fn count_method(nodes: &[serde_json::Value], target: &str) -> usize {
            let mut count = 0;
            for n in nodes {
                if n["method"].as_str() == Some(target) {
                    count += 1;
                }
                if let Some(callers) = n.get("callers").and_then(|c| c.as_array()) {
                    count += count_method(callers, target);
                }
            }
            count
        }
        let method_a_count = count_method(tree, "method_a");
        let method_b_count = count_method(tree, "method_b");
        assert!(
            method_a_count <= 1,
            "visited must prevent re-recursion: method_a should appear at most once \
             (as leaf caller of method_b); got {} occurrences. tree: {}",
            method_a_count,
            serde_json::to_string_pretty(tree).unwrap()
        );
        assert_eq!(
            method_b_count, 1,
            "method_b should appear exactly once (direct caller of method_a); \
             got {} occurrences. tree: {}",
            method_b_count,
            serde_json::to_string_pretty(tree).unwrap()
        );
    }

    // ─── B3: total body lines budget enforced across recursion ───────
    //
    // Real-file fixture: a single .rs file with 10 functions chained
    // a1 → a2 → ... → a10. With includeBody=true, maxBodyLines=10,
    // maxTotalBodyLines=30, only the first ~3 callers should get body;
    // the rest must show `bodyOmitted = "total body lines budget exceeded"`.
    //
    // Regression risk: if Builder.total_body_lines_emitted were reset between
    // recursion levels, the budget would be ignored and ALL nodes would get
    // bodies, potentially blowing up response size.
    #[test]
    fn test_builder_total_body_lines_budget_across_recursion() {
        let tmp_dir = unique_tmp_dir("budget");
        let file_path = tmp_dir.join("chain.rs");

        // Build a 10-function chain. Each function spans ~10 lines.
        // Function ai is defined at lines [base, base+9], and its body
        // contains a single call to a(i+1).
        // Layout:
        //   a1: lines 1..10 (body line 5 calls a2)
        //   a2: lines 11..20 (body line 15 calls a3)
        //   ...
        //   a10: lines 91..100
        let mut content = String::new();
        for i in 1..=10 {
            content.push_str(&format!("// {} header\n", i));     // line base+0
            content.push_str(&format!("fn a{}() {{\n", i));      // line base+1
            content.push_str("    // padding\n");                // line base+2
            content.push_str("    // padding\n");                // line base+3
            content.push_str("    // padding\n");                // line base+4
            if i < 10 {
                content.push_str(&format!("    a{}();\n", i + 1)); // line base+5
            } else {
                content.push_str("    // terminal\n");
            }
            content.push_str("    // padding\n");                // line base+6
            content.push_str("    // padding\n");                // line base+7
            content.push_str("}\n");                              // line base+8
            content.push('\n');                               // line base+9
        }
        std::fs::write(&file_path, &content).expect("failed to write fixture");
        let file_path_str = file_path.to_string_lossy().to_string();

        // Build content index: each ai appears at its own definition line
        // and at the call site in a(i-1).
        let mut content_idx = HashMap::new();
        for i in 1..=10 {
            let def_line = (i - 1) * 10 + 2; // "fn ai()" line
            let mut postings = vec![def_line];
            if i > 1 {
                let call_line = (i - 2) * 10 + 6; // "    ai();" line in a(i-1)
                postings.push(call_line);
            }
            content_idx.insert(
                format!("a{}", i),
                vec![Posting {
                    file_id: 0,
                    lines: postings.into_iter().map(|n| n as u32).collect(),
                }],
            );
        }

        let content_index = ContentIndex {
            root: tmp_dir.to_string_lossy().to_string(),
            files: vec![file_path_str.clone()],
            index: content_idx,
            total_tokens: 100,
            extensions: vec!["rs".to_string()],
            file_token_counts: vec![100],
            ..Default::default()
        };

        // Build definitions: 10 functions in file0.
        let mut definitions = Vec::new();
        for i in 1u32..=10 {
            let line_start = (i - 1) * 10 + 2;
            let line_end = line_start + 7;
            definitions.push(DefinitionEntry {
                file_id: 0,
                name: format!("a{}", i),
                kind: DefinitionKind::Function,
                line_start,
                line_end,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            });
        }

        let (name_index, kind_index, file_index) = build_indexes(&definitions);
        let mut path_to_id = HashMap::new();
        path_to_id.insert(PathBuf::from(&file_path_str), 0);

        let def_index = DefinitionIndex {
            root: tmp_dir.to_string_lossy().to_string(),
            created_at: 0,
            extensions: vec!["rs".to_string()],
            files: vec![file_path_str.clone()],
            definitions,
            name_index,
            kind_index,
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index,
            path_to_id,
            ..Default::default()
        };

        let ctx = HandlerContext {
            index: Arc::new(RwLock::new(content_index)),
            def_index: Some(Arc::new(RwLock::new(def_index))),
            workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
                tmp_dir.to_string_lossy().to_string(),
            ))),
            server_ext: "rs".to_string(),
            ..Default::default()
        };

        // Find callers of a10 — should walk up the chain a9 ← a8 ← ... ← a1.
        let result = dispatch_tool(
            &ctx,
            "xray_callers",
            &json!({
                "method": ["a10"],
                "depth": 10,
                "maxCallersPerLevel": 5,
                "includeBody": true,
                "maxBodyLines": 10,
                "maxTotalBodyLines": 30,
                "resolveInterfaces": false
            }),
        );
        assert!(
            !result.is_error,
            "Should not error: {}",
            result.content[0].text
        );
        let output: serde_json::Value =
            serde_json::from_str(&result.content[0].text).unwrap();

        // Walk the tree, count: nodes with body, nodes with bodyOmitted-budget.
        fn collect(
            nodes: &[serde_json::Value],
            with_body: &mut usize,
            budget_omitted: &mut usize,
            other_omitted: &mut usize,
        ) {
            for n in nodes {
                if n.get("body").and_then(|b| b.as_array()).is_some_and(|a| !a.is_empty()) {
                    *with_body += 1;
                } else if let Some(reason) = n.get("bodyOmitted").and_then(|b| b.as_str()) {
                    if reason.contains("budget") {
                        *budget_omitted += 1;
                    } else {
                        *other_omitted += 1;
                    }
                }
                if let Some(callers) = n.get("callers").and_then(|c| c.as_array()) {
                    collect(callers, with_body, budget_omitted, other_omitted);
                }
            }
        }
        let mut with_body = 0;
        let mut budget_omitted = 0;
        let mut other_omitted = 0;
        let tree = output["callTree"].as_array().unwrap();
        collect(tree, &mut with_body, &mut budget_omitted, &mut other_omitted);

        // Budget = 30 lines, each body ≈ 8 lines clamped at 10 → ~3-4 nodes
        // get bodies before budget is exhausted.
        assert!(
            with_body > 0,
            "At least one node should have body (budget=30 > one body)"
        );
        assert!(
            with_body <= 5,
            "Budget=30 should limit bodies to at most ~5; got {}",
            with_body
        );
        assert!(
            budget_omitted > 0,
            "At least one node must hit the total-body-lines budget; got with_body={}, budget_omitted={}, other_omitted={}",
            with_body,
            budget_omitted,
            other_omitted
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    // ─── B4: file_cache reused for multiple callers in same file ─────
    //
    // Three callers all live in the same file. With includeBody=true, all 3
    // bodies must be filled correctly. This guards against a regression where
    // Builder.file_cache might be re-created per recursion level (in which
    // case the body content would still load via fs::read but we'd lose the
    // optimization). The semantic check: all bodies non-empty AND content is
    // the *correct* slice for each definition (not aliased / not duplicated).
    #[test]
    fn test_builder_file_cache_reused_same_file() {
        let tmp_dir = unique_tmp_dir("filecache");
        let file_path = tmp_dir.join("orders.rs");

        // 3 callers (process_a, process_b, process_c) all in one file,
        // each calls target_fn. Plus target_fn defined at top.
        // Layout (1-based):
        //   line 1:  fn target_fn() { ... }
        //   line 5:  fn process_a() { target_fn(); /*MARK_A*/ }
        //   line 15: fn process_b() { target_fn(); /*MARK_B*/ }
        //   line 25: fn process_c() { target_fn(); /*MARK_C*/ }
        let mut content = String::new();
        content.push_str("fn target_fn() { /*TARGET*/ }\n"); // line 1
        for _ in 2..5 {
            content.push_str("// pad\n");
        }
        content.push_str("fn process_a() {\n");                 // line 5
        content.push_str("    target_fn();\n");                 // line 6
        content.push_str("    // MARK_A\n");                    // line 7
        content.push_str("}\n");                                 // line 8
        for _ in 9..15 {
            content.push_str("// pad\n");
        }
        content.push_str("fn process_b() {\n");                 // line 15
        content.push_str("    target_fn();\n");                 // line 16
        content.push_str("    // MARK_B\n");                    // line 17
        content.push_str("}\n");                                 // line 18
        for _ in 19..25 {
            content.push_str("// pad\n");
        }
        content.push_str("fn process_c() {\n");                 // line 25
        content.push_str("    target_fn();\n");                 // line 26
        content.push_str("    // MARK_C\n");                    // line 27
        content.push_str("}\n");                                 // line 28

        std::fs::write(&file_path, &content).expect("failed to write fixture");
        let file_path_str = file_path.to_string_lossy().to_string();

        let mut content_idx = HashMap::new();
        content_idx.insert(
            "target_fn".to_string(),
            vec![Posting {
                file_id: 0,
                lines: vec![1, 6, 16, 26],
            }],
        );
        content_idx.insert(
            "process_a".to_string(),
            vec![Posting { file_id: 0, lines: vec![5] }],
        );
        content_idx.insert(
            "process_b".to_string(),
            vec![Posting { file_id: 0, lines: vec![15] }],
        );
        content_idx.insert(
            "process_c".to_string(),
            vec![Posting { file_id: 0, lines: vec![25] }],
        );

        let content_index = ContentIndex {
            root: tmp_dir.to_string_lossy().to_string(),
            files: vec![file_path_str.clone()],
            index: content_idx,
            total_tokens: 100,
            extensions: vec!["rs".to_string()],
            file_token_counts: vec![100],
            ..Default::default()
        };

        let definitions = vec![
            DefinitionEntry {
                file_id: 0,
                name: "target_fn".to_string(),
                kind: DefinitionKind::Function,
                line_start: 1,
                line_end: 1,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            DefinitionEntry {
                file_id: 0,
                name: "process_a".to_string(),
                kind: DefinitionKind::Function,
                line_start: 5,
                line_end: 8,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            DefinitionEntry {
                file_id: 0,
                name: "process_b".to_string(),
                kind: DefinitionKind::Function,
                line_start: 15,
                line_end: 18,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
            DefinitionEntry {
                file_id: 0,
                name: "process_c".to_string(),
                kind: DefinitionKind::Function,
                line_start: 25,
                line_end: 28,
                parent: None,
                signature: None,
                modifiers: vec![],
                attributes: vec![],
                base_types: vec![],
            },
        ];

        let (name_index, kind_index, file_index) = build_indexes(&definitions);
        let mut path_to_id = HashMap::new();
        path_to_id.insert(PathBuf::from(&file_path_str), 0);

        let def_index = DefinitionIndex {
            root: tmp_dir.to_string_lossy().to_string(),
            created_at: 0,
            extensions: vec!["rs".to_string()],
            files: vec![file_path_str.clone()],
            definitions,
            name_index,
            kind_index,
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index,
            path_to_id,
            ..Default::default()
        };

        let ctx = HandlerContext {
            index: Arc::new(RwLock::new(content_index)),
            def_index: Some(Arc::new(RwLock::new(def_index))),
            workspace: Arc::new(RwLock::new(WorkspaceBinding::pinned(
                tmp_dir.to_string_lossy().to_string(),
            ))),
            server_ext: "rs".to_string(),
            ..Default::default()
        };

        let result = dispatch_tool(
            &ctx,
            "xray_callers",
            &json!({
                "method": ["target_fn"],
                "depth": 1,
                "maxCallersPerLevel": 10,
                "includeBody": true,
                "maxBodyLines": 10,
                "maxTotalBodyLines": 100,
                "resolveInterfaces": false
            }),
        );
        assert!(
            !result.is_error,
            "Should not error: {}",
            result.content[0].text
        );
        let output: serde_json::Value =
            serde_json::from_str(&result.content[0].text).unwrap();
        let tree = output["callTree"].as_array().unwrap();

        // All 3 callers must be present and have correctly-sliced bodies
        // (containing their own MARK_X comment). This proves file_cache
        // returned correct content per-definition, not aliased to one slice.
        let mut found_a = false;
        let mut found_b = false;
        let mut found_c = false;
        for node in tree {
            let name = node["method"].as_str().unwrap();
            let body = node["body"]
                .as_array()
                .unwrap_or_else(|| panic!("body must be present for caller {}", name));
            let body_text = body
                .iter()
                .map(|line| line.as_str().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !body_text.is_empty(),
                "Body for {} must be non-empty",
                name
            );
            match name {
                "process_a" => {
                    assert!(
                        body_text.contains("MARK_A"),
                        "process_a body must contain its own marker, got: {:?}",
                        body_text
                    );
                    found_a = true;
                }
                "process_b" => {
                    assert!(
                        body_text.contains("MARK_B"),
                        "process_b body must contain its own marker, got: {:?}",
                        body_text
                    );
                    found_b = true;
                }
                "process_c" => {
                    assert!(
                        body_text.contains("MARK_C"),
                        "process_c body must contain its own marker, got: {:?}",
                        body_text
                    );
                    found_c = true;
                }
                _ => panic!("unexpected caller: {}", name),
            }
        }
        assert!(found_a && found_b && found_c, "All 3 callers must be present");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
