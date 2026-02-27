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
