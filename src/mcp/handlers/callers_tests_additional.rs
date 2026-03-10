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

#[test]
fn test_passes_caller_file_filters_case_insensitive_ext() {
    // Extension comparison should be case-insensitive
    assert!(passes_caller_file_filters("src/File.CS", "cs", &[], &[]));
    assert!(passes_caller_file_filters("src/File.Cs", "CS", &[], &[]));
}

#[test]
fn test_passes_caller_file_filters_combined_exclude() {
    let exclude_dir = vec!["test".to_string()];
    let exclude_file = vec!["mock".to_string()];
    // File in test dir
    assert!(!passes_caller_file_filters("src/test/Service.cs", "cs", &exclude_dir, &exclude_file));
    // File matching exclude_file
    assert!(!passes_caller_file_filters("src/MockService.cs", "cs", &exclude_dir, &exclude_file));
    // Both excluded
    assert!(!passes_caller_file_filters("src/test/MockService.cs", "cs", &exclude_dir, &exclude_file));
    // Neither excluded
    assert!(passes_caller_file_filters("src/main/Service.cs", "cs", &exclude_dir, &exclude_file));
}

#[test]
fn test_passes_caller_file_filters_no_extension() {
    // File without extension should not match any ext filter
    assert!(!passes_caller_file_filters("Makefile", "cs", &[], &[]));
}

#[test]
fn test_passes_caller_file_filters_ext_with_spaces() {
    // Comma-separated ext with spaces should be trimmed
    assert!(passes_caller_file_filters("src/File.cs", "cs, ts", &[], &[]));
    assert!(passes_caller_file_filters("src/File.ts", " cs , ts ", &[], &[]));
}

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
    use search_index::generate_trigrams;

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
    use search_index::generate_trigrams;

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
    use search_index::generate_trigrams;

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
// These test the full search_callers handler with production + test callers

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
    let result = super::super::dispatch_tool(&ctx, "search_callers", &json!({
        "method": "truncate_data",
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
    for i in 2..10 {
        let name = tree[i]["method"].as_str().unwrap();
        assert!(name.starts_with("test_"), "Caller at index {} should be test, got: {}", i, name);
    }
}

#[test]
fn test_callers_deprioritize_tests_truncation() {
    // 2 production + 8 test callers, maxCallersPerLevel=5
    // Should return: 2 production + 3 test (5 total)
    let ctx = make_ctx_test_deprioritization();
    let result = super::super::dispatch_tool(&ctx, "search_callers", &json!({
        "method": "truncate_data",
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
    let result = super::super::dispatch_tool(&ctx, "search_callers", &json!({
        "method": "truncate_data",
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
    let result = super::super::dispatch_tool(&ctx, "search_callers", &json!({
        "method": "truncate_data",
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

    let result = super::super::dispatch_tool(&ctx, "search_callers", &json!({
        "method": "helper_fn",
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

