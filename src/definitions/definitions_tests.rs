//! General tests for the definitions module — language-specific tests are in
//! definitions_tests_csharp.rs and definitions_tests_typescript.rs.

use super::*;
use std::collections::HashMap;

#[test]
fn test_definition_kind_roundtrip() {
    let kinds = vec![
        DefinitionKind::Class, DefinitionKind::Interface, DefinitionKind::Method,
        DefinitionKind::StoredProcedure, DefinitionKind::Table,
    ];
    for kind in kinds {
        let s = kind.as_str();
        let parsed: DefinitionKind = s.parse().unwrap();
        assert_eq!(parsed, kind);
    }
}

#[test]
fn test_definition_kind_display() {
    assert_eq!(format!("{}", DefinitionKind::Class), "class");
    assert_eq!(format!("{}", DefinitionKind::StoredProcedure), "storedProcedure");
    assert_eq!(format!("{}", DefinitionKind::EnumMember), "enumMember");
}

#[test]
fn test_definition_kind_parse_invalid() {
    let result = "invalid_kind".parse::<DefinitionKind>();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown definition kind"));
}

#[test]
fn test_definition_kind_parse_case_insensitive() {
    let parsed: DefinitionKind = "CLASS".parse().unwrap();
    assert_eq!(parsed, DefinitionKind::Class);
    let parsed: DefinitionKind = "StoredProcedure".parse().unwrap();
    assert_eq!(parsed, DefinitionKind::StoredProcedure);
}

#[test]
fn test_definition_kind_roundtrip_all_variants() {
    let all_kinds = vec![
        DefinitionKind::Class, DefinitionKind::Interface, DefinitionKind::Enum,
        DefinitionKind::Struct, DefinitionKind::Record, DefinitionKind::Method,
        DefinitionKind::Property, DefinitionKind::Field, DefinitionKind::Constructor,
        DefinitionKind::Delegate, DefinitionKind::Event, DefinitionKind::EnumMember,
        DefinitionKind::StoredProcedure, DefinitionKind::Table, DefinitionKind::View,
        DefinitionKind::SqlFunction, DefinitionKind::UserDefinedType,
        DefinitionKind::Column, DefinitionKind::SqlIndex,
    ];
    for kind in all_kinds {
        let s = kind.to_string();
        let parsed: DefinitionKind = s.parse().unwrap_or_else(|e| panic!("Failed to parse '{}': {}", s, e));
        assert_eq!(parsed, kind, "Roundtrip failed for {:?} -> '{}' -> {:?}", kind, s, parsed);
    }
}

#[cfg(all(feature = "lang-csharp", feature = "lang-sql"))]
#[test]
fn test_definition_index_build_and_search() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("test.cs"), "public class TestClass : BaseClass { public void TestMethod() {} }").unwrap();
    std::fs::write(dir.join("test.sql"), "CREATE TABLE TestTable (Id INT NOT NULL)").unwrap();

    let args = DefIndexArgs { dir: dir.to_string_lossy().to_string(), ext: "cs,sql".to_string(), threads: 1 };
    let index = build_definition_index(&args);

    assert_eq!(index.files.len(), 2);
    assert!(!index.definitions.is_empty());
    assert!(index.name_index.contains_key("testclass"));
    assert!(index.name_index.contains_key("testmethod"));
    assert!(index.kind_index.contains_key(&DefinitionKind::Class));
    assert!(index.kind_index.contains_key(&DefinitionKind::Method));
}

#[test]
fn test_definition_index_serialization() {
    let index = DefinitionIndex {
        root: ".".to_string(), created_at: 1000, extensions: vec!["cs".to_string()],
        files: vec!["test.cs".to_string()],
        definitions: vec![DefinitionEntry {
            file_id: 0, name: "TestClass".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 10, parent: None,
            signature: Some("public class TestClass".to_string()),
            modifiers: vec!["public".to_string()], attributes: Vec::new(), base_types: Vec::new(),
        }],
        name_index: { let mut m = HashMap::new(); m.insert("testclass".to_string(), vec![0]); m },
        kind_index: { let mut m = HashMap::new(); m.insert(DefinitionKind::Class, vec![0]); m },
        file_index: { let mut m = HashMap::new(); m.insert(0, vec![0]); m },
        ..Default::default()
    };

    let encoded = bincode::serialize(&index).unwrap();
    let decoded: DefinitionIndex = bincode::deserialize(&encoded).unwrap();
    assert_eq!(decoded.definitions.len(), 1);
    assert_eq!(decoded.definitions[0].name, "TestClass");
    assert_eq!(decoded.definitions[0].kind, DefinitionKind::Class);
}

// ─── read_file_lossy Tests ──────────────────────────────────────────

#[test]
fn test_read_file_lossy_with_valid_utf8() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file_path = dir.join("valid.cs");
    std::fs::write(&file_path, "public class ValidService {}").unwrap();

    let (content, was_lossy) = search_index::read_file_lossy(&file_path).unwrap();
    assert!(!was_lossy, "Valid UTF-8 file should not be lossy");
    assert!(content.contains("ValidService"));
}

#[test]
fn test_read_file_lossy_with_non_utf8_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file_path = dir.join("invalid.cs");

    // Write a file with 0x92 byte (Windows-1252 right single quote)
    let mut content = b"// Comment: you\x92re a dev\n".to_vec();
    content.extend_from_slice(b"public class TestService {}\n");
    std::fs::write(&file_path, &content).unwrap();

    let (result, was_lossy) = search_index::read_file_lossy(&file_path).unwrap();
    assert!(was_lossy, "Non-UTF8 file should be lossy");
    assert!(result.contains("TestService"), "Should still read the file content");
    assert!(result.contains('\u{FFFD}'), "Should contain replacement character");
}


// ─── Lazy Parser Init & Extension Filtering Tests ─────────────────────

#[cfg(feature = "lang-csharp")]
#[test]
fn test_build_def_index_cs_only_no_ts_parsers() {
    // When ext="cs" only, TS/TSX parsers should NOT be eagerly created
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Create a .cs file and a .ts file
    std::fs::write(dir.join("Service.cs"), "public class UserService { public void Process() {} }").unwrap();
    std::fs::write(dir.join("util.ts"), "export function helper(): void {}").unwrap();

    let idx = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs".to_string(), // only C#
        threads: 1,
    });

    // Should find the C# class and method but NOT the TypeScript function
    assert!(idx.name_index.contains_key("userservice"), "Should find C# class");
    assert!(idx.name_index.contains_key("process"), "Should find C# method");
    assert!(!idx.name_index.contains_key("helper"), "Should NOT find TS function when ext=cs");
}

#[cfg(all(feature = "lang-csharp", feature = "lang-typescript"))]
#[test]
fn test_build_def_index_cs_and_ts() {
    // When ext="cs,ts", both C# and TS files should be parsed
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    std::fs::write(dir.join("Service.cs"), "public class UserService { }").unwrap();
    std::fs::write(dir.join("util.ts"), "export function helper(): void {}").unwrap();

    let idx = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs,ts".to_string(),
        threads: 1,
    });

    assert!(idx.name_index.contains_key("userservice"), "Should find C# class");
    assert!(idx.name_index.contains_key("helper"), "Should find TS function when ext=cs,ts");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn test_build_def_index_ts_only() {
    // When ext="ts,tsx", only TS/TSX files should be parsed
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    std::fs::write(dir.join("Service.cs"), "public class UserService { }").unwrap();
    std::fs::write(dir.join("app.ts"), "export class AppController { run(): void {} }").unwrap();

    let idx = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "ts".to_string(),
        threads: 1,
    });

    assert!(!idx.name_index.contains_key("userservice"), "Should NOT find C# class when ext=ts");
    assert!(idx.name_index.contains_key("appcontroller"), "Should find TS class");
    assert!(idx.name_index.contains_key("run"), "Should find TS method");
}


// ─── Reconciliation Tests ───────────────────────────────────────────

#[cfg(feature = "lang-csharp")]
#[test]
fn test_reconcile_adds_new_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Create initial file and build index
    std::fs::write(dir.join("existing.cs"), "public class ExistingService { }").unwrap();
    let mut index = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
    });
    assert_eq!(index.files.len(), 1);
    assert!(index.name_index.contains_key("existingservice"));

    // Add a new file AFTER index was built
    std::fs::write(dir.join("new_service.cs"), "public class NewService { public void Process() {} }").unwrap();

    // Reconcile should find the new file
    let extensions = vec!["cs".to_string()];
    let (added, _modified, removed) = incremental::reconcile_definition_index(
        &mut index,
        &dir.to_string_lossy(),
        &extensions,
    );

    assert_eq!(added, 1, "Should have added 1 new file");
    assert_eq!(removed, 0, "Should not have removed any files");
    assert_eq!(index.files.len(), 2, "Should now have 2 files");
    assert!(index.name_index.contains_key("newservice"), "New class should be in index");
    assert!(index.name_index.contains_key("process"), "New method should be in index");
}

#[cfg(feature = "lang-csharp")]
#[test]
fn test_reconcile_removes_deleted_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Create two files and build index
    std::fs::write(dir.join("keep.cs"), "public class KeepService { }").unwrap();
    std::fs::write(dir.join("delete.cs"), "public class DeleteService { }").unwrap();
    let mut index = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
    });
    assert_eq!(index.files.len(), 2);
    assert!(index.name_index.contains_key("deleteservice"));

    // Delete one file
    std::fs::remove_file(dir.join("delete.cs")).unwrap();

    // Reconcile should detect the deletion
    let extensions = vec!["cs".to_string()];
    let (added, _modified, removed) = incremental::reconcile_definition_index(
        &mut index,
        &dir.to_string_lossy(),
        &extensions,
    );

    assert_eq!(added, 0, "Should not have added any files");
    assert_eq!(removed, 1, "Should have removed 1 file");
    assert!(index.name_index.contains_key("keepservice"), "Kept class should still be in index");
    // Note: DeleteService definitions may still be in the vec as tombstones,
    // but name_index should no longer reference them
    assert!(!index.name_index.contains_key("deleteservice"), "Deleted class should not be in index");
}

#[cfg(feature = "lang-csharp")]
#[test]
fn test_reconcile_detects_modified_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Create file
    std::fs::write(dir.join("service.cs"), "public class OldService { public void OldMethod() {} }").unwrap();

    // Build index with a created_at in the past (so any current mtime > threshold)
    let mut index = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
    });
    assert!(index.name_index.contains_key("oldservice"));
    assert!(index.name_index.contains_key("oldmethod"));

    // Set created_at to past so the file's mtime will be "newer"
    index.created_at = 1000;

    // Modify the file content
    std::fs::write(dir.join("service.cs"), "public class NewService { public void NewMethod() {} }").unwrap();

    // Reconcile should detect the modification via mtime
    let extensions = vec!["cs".to_string()];
    let (added, modified, removed) = incremental::reconcile_definition_index(
        &mut index,
        &dir.to_string_lossy(),
        &extensions,
    );

    assert_eq!(added, 0, "Should not have added any files");
    assert_eq!(modified, 1, "Should have modified 1 file");
    assert_eq!(removed, 0, "Should not have removed any files");
    assert!(index.name_index.contains_key("newservice"), "Updated class should be in index");
    assert!(index.name_index.contains_key("newmethod"), "Updated method should be in index");
}

#[cfg(feature = "lang-csharp")]
#[test]
fn test_reconcile_skips_unchanged_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Create file
    std::fs::write(dir.join("service.cs"), "public class StableService { }").unwrap();

    let mut index = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
    });
    let original_def_count = index.definitions.len();
    assert!(index.name_index.contains_key("stableservice"));

    // Set created_at to a far future value so no file's mtime will be "newer"
    // (year ~2100, safely within SystemTime range — avoids overflow panic)
    index.created_at = 4_102_444_800;

    // Reconcile — nothing should change
    let extensions = vec!["cs".to_string()];
    let (added, modified, removed) = incremental::reconcile_definition_index(
        &mut index,
        &dir.to_string_lossy(),
        &extensions,
    );

    assert_eq!(added, 0);
    assert_eq!(modified, 0);
    assert_eq!(removed, 0);
    assert_eq!(index.definitions.len(), original_def_count, "No definitions should have been added");
    assert!(index.name_index.contains_key("stableservice"), "Original definition should remain");
}

// ─── Compact Definitions Tests ──────────────────────────────────────

#[test]
fn test_compact_removes_tombstones() {
    use std::path::PathBuf;

    let mut index = DefinitionIndex {
        root: ".".to_string(), extensions: vec!["cs".to_string()],
        files: vec!["file0.cs".to_string(), "file1.cs".to_string()],
        definitions: vec![
            DefinitionEntry { file_id: 0, name: "ClassA".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 10, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 0, name: "MethodA".to_string(), kind: DefinitionKind::Method, line_start: 2, line_end: 5, parent: Some("ClassA".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 1, name: "ClassB".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 20, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        ],
        name_index: { let mut m = HashMap::new(); m.insert("classa".to_string(), vec![0]); m.insert("methoda".to_string(), vec![1]); m.insert("classb".to_string(), vec![2]); m },
        kind_index: { let mut m = HashMap::new(); m.insert(DefinitionKind::Class, vec![0, 2]); m.insert(DefinitionKind::Method, vec![1]); m },
        file_index: { let mut m = HashMap::new(); m.insert(0, vec![0, 1]); m.insert(1, vec![2]); m },
        path_to_id: { let mut m = HashMap::new(); m.insert(PathBuf::from("file0.cs"), 0); m.insert(PathBuf::from("file1.cs"), 1); m },
        ..Default::default()
    };

    // Simulate removing file0's definitions from secondary indexes (but not from Vec)
    remove_file_definitions(&mut index, 0);

    // Now definitions Vec has 3 entries but only 1 is active (def[2] = ClassB)
    assert_eq!(index.definitions.len(), 3, "Vec should still have 3 entries (tombstones)");
    let active: usize = index.file_index.values().map(|v| v.len()).sum();
    assert_eq!(active, 1, "Only 1 active definition");

    // Compact — should remove tombstones
    compact_definitions(&mut index);

    assert_eq!(index.definitions.len(), 1, "After compact, Vec should have 1 entry");
    assert_eq!(index.definitions[0].name, "ClassB", "Remaining def should be ClassB");

    // Verify secondary indexes are remapped
    let class_indices = index.kind_index.get(&DefinitionKind::Class).unwrap();
    assert_eq!(class_indices, &vec![0u32], "ClassB should be at index 0 after compact");
    assert!(!index.kind_index.contains_key(&DefinitionKind::Method), "Method kind should be empty after compact");

    let classb_name = index.name_index.get("classb").unwrap();
    assert_eq!(classb_name, &vec![0u32], "classb in name_index should point to 0");
    assert!(!index.name_index.contains_key("classa"), "classa should be gone from name_index");

    let file1_defs = index.file_index.get(&1).unwrap();
    assert_eq!(file1_defs, &vec![0u32], "file_index[1] should point to 0 after compact");
}

#[test]
fn test_compact_no_tombstones_is_noop() {
    use std::path::PathBuf;

    let mut index = DefinitionIndex {
        root: ".".to_string(), extensions: vec!["cs".to_string()],
        files: vec!["file0.cs".to_string()],
        definitions: vec![
            DefinitionEntry { file_id: 0, name: "ClassA".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 10, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        ],
        name_index: { let mut m = HashMap::new(); m.insert("classa".to_string(), vec![0]); m },
        kind_index: { let mut m = HashMap::new(); m.insert(DefinitionKind::Class, vec![0]); m },
        file_index: { let mut m = HashMap::new(); m.insert(0, vec![0]); m },
        path_to_id: { let mut m = HashMap::new(); m.insert(PathBuf::from("file0.cs"), 0); m },
        ..Default::default()
    };

    compact_definitions(&mut index);

    // Should be unchanged
    assert_eq!(index.definitions.len(), 1);
    assert_eq!(index.definitions[0].name, "ClassA");
    assert_eq!(index.name_index.get("classa").unwrap(), &vec![0u32]);
}

#[test]
fn test_compact_remaps_method_calls_and_code_stats() {
    use std::path::PathBuf;

    let mut index = DefinitionIndex {
        root: ".".to_string(), extensions: vec!["cs".to_string()],
        files: vec!["file0.cs".to_string(), "file1.cs".to_string()],
        definitions: vec![
            DefinitionEntry { file_id: 0, name: "MethodA".to_string(), kind: DefinitionKind::Method, line_start: 1, line_end: 5, parent: Some("ClassA".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 1, name: "MethodB".to_string(), kind: DefinitionKind::Method, line_start: 1, line_end: 10, parent: Some("ClassB".to_string()), signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        ],
        name_index: { let mut m = HashMap::new(); m.insert("methoda".to_string(), vec![0]); m.insert("methodb".to_string(), vec![1]); m },
        kind_index: { let mut m = HashMap::new(); m.insert(DefinitionKind::Method, vec![0, 1]); m },
        file_index: { let mut m = HashMap::new(); m.insert(0, vec![0]); m.insert(1, vec![1]); m },
        path_to_id: { let mut m = HashMap::new(); m.insert(PathBuf::from("file0.cs"), 0); m.insert(PathBuf::from("file1.cs"), 1); m },
        method_calls: {
            let mut m = HashMap::new();
            m.insert(0, vec![CallSite { method_name: "Helper".to_string(), receiver_type: None, line: 3, receiver_is_generic: false }]);
            m.insert(1, vec![CallSite { method_name: "DoWork".to_string(), receiver_type: Some("IService".to_string()), line: 5, receiver_is_generic: false }]);
            m
        },
        code_stats: {
            let mut m = HashMap::new();
            m.insert(0, CodeStats { cyclomatic_complexity: 2, cognitive_complexity: 1, max_nesting_depth: 1, param_count: 0, return_count: 1, call_count: 1, lambda_count: 0 });
            m.insert(1, CodeStats { cyclomatic_complexity: 5, cognitive_complexity: 3, max_nesting_depth: 2, param_count: 2, return_count: 1, call_count: 3, lambda_count: 0 });
            m
        },
        ..Default::default()
    };

    // Remove file0 definitions (MethodA at idx 0)
    remove_file_definitions(&mut index, 0);

    // Compact
    compact_definitions(&mut index);

    // Only MethodB should remain at index 0
    assert_eq!(index.definitions.len(), 1);
    assert_eq!(index.definitions[0].name, "MethodB");

    // method_calls should be remapped: old key 1 → new key 0
    assert!(!index.method_calls.contains_key(&1), "Old key should be gone");
    let calls = index.method_calls.get(&0).unwrap();
    assert_eq!(calls[0].method_name, "DoWork");

    // code_stats should be remapped similarly
    assert!(!index.code_stats.contains_key(&1), "Old key should be gone");
    let stats = index.code_stats.get(&0).unwrap();
    assert_eq!(stats.cyclomatic_complexity, 5);
}

#[test]
fn test_compact_auto_triggers_at_threshold() {
    use std::path::PathBuf;

    // Create an index with 4 definitions across 2 files
    let mut index = DefinitionIndex {
        root: ".".to_string(), extensions: vec!["cs".to_string()],
        files: vec!["file0.cs".to_string(), "file1.cs".to_string()],
        definitions: vec![
            DefinitionEntry { file_id: 0, name: "A".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 1, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 1, name: "B".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 1, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        ],
        name_index: { let mut m = HashMap::new(); m.insert("a".to_string(), vec![0]); m.insert("b".to_string(), vec![1]); m },
        kind_index: { let mut m = HashMap::new(); m.insert(DefinitionKind::Class, vec![0, 1]); m },
        file_index: { let mut m = HashMap::new(); m.insert(0, vec![0]); m.insert(1, vec![1]); m },
        path_to_id: { let mut m = HashMap::new(); m.insert(PathBuf::from("file0.cs"), 0); m.insert(PathBuf::from("file1.cs"), 1); m },
        ..Default::default()
    };

    // Simulate multiple incremental updates to the same file to grow tombstones
    // Each update adds new entries to the Vec without removing old ones
    for i in 0..5 {
        // Simulate file0 being updated — adds 1 new def at end, clears secondary for old
        let _path = PathBuf::from("file0.cs");
        let content = format!("public class Update{} {{ }}", i);
        std::fs::create_dir_all(tmp_dir_for_test()).ok();
        let file_path = tmp_dir_for_test().join("file0.cs");
        std::fs::write(&file_path, &content).unwrap();
        index.path_to_id.insert(file_path.clone(), 0);
        update_file_definitions(&mut index, &file_path);
    }

    // After 5 updates to file0, the Vec should have grown significantly
    // Active count should be small (1 from file0 latest + 1 from file1 = 2)
    let active: usize = index.file_index.values().map(|v| v.len()).sum();
    assert!(active <= 3, "Active count should be small: {}", active);

    // If total > active * 3, auto-compact should have triggered in remove_file_definitions
    // Check that definitions.len() is reasonable
    assert!(index.definitions.len() <= active * 4,
        "After auto-compact, Vec len ({}) should be close to active count ({})",
        index.definitions.len(), active);
}

/// Helper: provides a test-specific temp directory
fn tmp_dir_for_test() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("search_compact_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    dir
}

#[test]
fn test_compact_remaps_selector_index_and_template_children() {

    let mut index = DefinitionIndex {
        root: ".".to_string(), extensions: vec!["ts".to_string()],
        files: vec!["comp-a.ts".to_string(), "comp-b.ts".to_string()],
        definitions: vec![
            DefinitionEntry { file_id: 0, name: "CompA".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 50, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
            DefinitionEntry { file_id: 1, name: "CompB".to_string(), kind: DefinitionKind::Class, line_start: 1, line_end: 30, parent: None, signature: None, modifiers: vec![], attributes: vec![], base_types: vec![] },
        ],
        name_index: { let mut m = HashMap::new(); m.insert("compa".to_string(), vec![0]); m.insert("compb".to_string(), vec![1]); m },
        kind_index: { let mut m = HashMap::new(); m.insert(DefinitionKind::Class, vec![0, 1]); m },
        file_index: { let mut m = HashMap::new(); m.insert(0, vec![0]); m.insert(1, vec![1]); m },
        path_to_id: { let mut m = HashMap::new(); m.insert(PathBuf::from("comp-a.ts"), 0); m.insert(PathBuf::from("comp-b.ts"), 1); m },
        selector_index: { let mut m = HashMap::new(); m.insert("app-comp-a".to_string(), vec![0]); m.insert("app-comp-b".to_string(), vec![1]); m },
        template_children: { let mut m = HashMap::new(); m.insert(0, vec!["app-child".to_string()]); m.insert(1, vec!["app-other".to_string()]); m },
        ..Default::default()
    };

    // Remove file0 (CompA at idx 0)
    remove_file_definitions(&mut index, 0);

    // Verify selector_index and template_children are cleaned (Fix 3)
    assert!(!index.selector_index.contains_key("app-comp-a"), "CompA selector should be removed");
    assert!(index.selector_index.contains_key("app-comp-b"), "CompB selector should remain");
    assert!(!index.template_children.contains_key(&0), "CompA template_children should be removed");
    assert!(index.template_children.contains_key(&1), "CompB template_children should remain");

    // Compact
    compact_definitions(&mut index);

    assert_eq!(index.definitions.len(), 1);
    assert_eq!(index.definitions[0].name, "CompB");

    // selector_index should be remapped: old def_idx 1 → new def_idx 0
    let compb_selector = index.selector_index.get("app-comp-b").unwrap();
    assert_eq!(compb_selector, &vec![0u32], "CompB selector should point to 0 after compact");

    // template_children should be remapped: old key 1 → new key 0
    assert!(index.template_children.contains_key(&0), "CompB children should be at key 0 after compact");
    assert_eq!(index.template_children.get(&0).unwrap(), &vec!["app-other".to_string()]);
}

// ─── collect_source_files() Tests ───────────────────────────────────

#[test]
fn test_collect_source_files_filters_by_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    std::fs::write(dir.join("service.cs"), "public class UserService {}").unwrap();
    std::fs::write(dir.join("readme.md"), "# Readme").unwrap();
    std::fs::write(dir.join("util.ts"), "export function helper() {}").unwrap();

    let extensions = vec!["cs".to_string()];
    let files = collect_source_files(dir, &extensions, 1);

    assert_eq!(files.len(), 1, "Should only find .cs files");
    assert!(files[0].ends_with("service.cs"), "Should find service.cs, got: {}", files[0]);
}

#[test]
fn test_collect_source_files_empty_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let extensions = vec!["cs".to_string()];
    let files = collect_source_files(dir, &extensions, 1);

    assert!(files.is_empty(), "Empty directory should return no files");
}

#[test]
fn test_collect_source_files_multiple_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    std::fs::write(dir.join("service.cs"), "class A {}").unwrap();
    std::fs::write(dir.join("util.ts"), "function b() {}").unwrap();
    std::fs::write(dir.join("data.json"), "{}").unwrap();

    let extensions = vec!["cs".to_string(), "ts".to_string()];
    let files = collect_source_files(dir, &extensions, 1);

    assert_eq!(files.len(), 2, "Should find both .cs and .ts files");
}

#[test]
fn test_collect_source_files_case_insensitive_ext() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    std::fs::write(dir.join("Service.CS"), "class Upper {}").unwrap();

    let extensions = vec!["cs".to_string()];
    let files = collect_source_files(dir, &extensions, 1);

    assert_eq!(files.len(), 1, "Extension matching should be case-insensitive");
}

#[test]
fn test_collect_source_files_respects_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Initialize a git repo so .gitignore is respected by the `ignore` crate
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir)
        .output()
        .expect("git init failed");

    // Create .gitignore that ignores the "ignored" directory
    std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
    std::fs::create_dir(dir.join("ignored")).unwrap();
    std::fs::write(dir.join("ignored").join("hidden.cs"), "class Hidden {}").unwrap();
    std::fs::write(dir.join("visible.cs"), "class Visible {}").unwrap();

    let extensions = vec!["cs".to_string()];
    let files = collect_source_files(dir, &extensions, 1);

    assert_eq!(files.len(), 1, "Should only find 1 file (gitignored file excluded)");
    assert!(files[0].ends_with("visible.cs"), "Should find visible.cs");
}

// ─── index_file_defs() Tests ────────────────────────────────────────

#[test]
fn test_index_file_defs_populates_all_indexes() {
    let mut index = DefinitionIndex::default();
    index.files.push("test.cs".to_string());

    let defs = vec![
        DefinitionEntry {
            file_id: 0, name: "UserService".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 50, parent: None,
            signature: Some("public class UserService".to_string()),
            modifiers: vec!["public".to_string()],
            attributes: vec![],
            base_types: vec!["IService".to_string()],
        },
        DefinitionEntry {
            file_id: 0, name: "Process".to_string(), kind: DefinitionKind::Method,
            line_start: 5, line_end: 20, parent: Some("UserService".to_string()),
            signature: Some("public void Process()".to_string()),
            modifiers: vec!["public".to_string()],
            attributes: vec![],
            base_types: vec![],
        },
    ];

    let call_sites_added = index_file_defs(&mut index, 0, defs, vec![], vec![]);

    // Verify definitions
    assert_eq!(index.definitions.len(), 2);
    assert_eq!(call_sites_added, 0);

    // Verify name_index (lowercased)
    assert!(index.name_index.contains_key("userservice"));
    assert!(index.name_index.contains_key("process"));

    // Verify kind_index
    assert_eq!(index.kind_index[&DefinitionKind::Class], vec![0]);
    assert_eq!(index.kind_index[&DefinitionKind::Method], vec![1]);

    // Verify base_type_index (lowercased)
    assert!(index.base_type_index.contains_key("iservice"));
    assert_eq!(index.base_type_index["iservice"], vec![0]);

    // Verify file_index
    assert_eq!(index.file_index[&0], vec![0, 1]);
}

#[test]
fn test_index_file_defs_handles_attributes_dedup() {
    let mut index = DefinitionIndex::default();
    index.files.push("test.cs".to_string());

    let defs = vec![
        DefinitionEntry {
            file_id: 0, name: "Handler".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 10, parent: None, signature: None,
            modifiers: vec![],
            // Duplicate attributes — should be deduplicated
            attributes: vec![
                "Authorize".to_string(),
                "Authorize(Roles = \"Admin\")".to_string(),
                "Route(\"/api\")".to_string(),
            ],
            base_types: vec![],
        },
    ];

    index_file_defs(&mut index, 0, defs, vec![], vec![]);

    // "authorize" appears twice as attribute, but should only have ONE entry in attribute_index
    let authorize_indices = &index.attribute_index["authorize"];
    assert_eq!(authorize_indices.len(), 1, "Duplicate attribute 'authorize' should be deduplicated");

    // "route" should also be indexed
    assert!(index.attribute_index.contains_key("route"));
}

#[test]
fn test_index_file_defs_maps_call_sites() {
    let mut index = DefinitionIndex::default();
    index.files.push("test.cs".to_string());

    // Pre-populate with one existing definition to test global indexing
    index.definitions.push(DefinitionEntry {
        file_id: 0, name: "Existing".to_string(), kind: DefinitionKind::Class,
        line_start: 1, line_end: 5, parent: None, signature: None,
        modifiers: vec![], attributes: vec![], base_types: vec![],
    });

    let defs = vec![
        DefinitionEntry {
            file_id: 1, name: "NewMethod".to_string(), kind: DefinitionKind::Method,
            line_start: 1, line_end: 10, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let calls = vec![
        (0, vec![
            CallSite { method_name: "DoWork".to_string(), receiver_type: Some("IService".to_string()), line: 3, receiver_is_generic: false },
            CallSite { method_name: "Log".to_string(), receiver_type: None, line: 5, receiver_is_generic: false },
        ]),
    ];

    let call_sites_added = index_file_defs(&mut index, 1, defs, calls, vec![]);

    assert_eq!(call_sites_added, 2, "Should report 2 call sites added");

    // The new definition is at global index 1 (after the pre-existing one)
    // So call site local_idx=0 maps to global_idx = base(1) + 0 = 1
    let method_calls = &index.method_calls[&1];
    assert_eq!(method_calls.len(), 2);
    assert_eq!(method_calls[0].method_name, "DoWork");
    assert_eq!(method_calls[1].method_name, "Log");
}

#[test]
fn test_index_file_defs_maps_code_stats() {
    let mut index = DefinitionIndex::default();
    index.files.push("test.cs".to_string());

    let defs = vec![
        DefinitionEntry {
            file_id: 0, name: "ComplexMethod".to_string(), kind: DefinitionKind::Method,
            line_start: 1, line_end: 30, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    let stats = vec![
        (0, CodeStats {
            cyclomatic_complexity: 15,
            cognitive_complexity: 25,
            max_nesting_depth: 4,
            param_count: 3,
            return_count: 2,
            call_count: 8,
            lambda_count: 1,
        }),
    ];

    index_file_defs(&mut index, 0, defs, vec![], stats);

    // Definition at global index 0
    let cs = &index.code_stats[&0];
    assert_eq!(cs.cyclomatic_complexity, 15);
    assert_eq!(cs.cognitive_complexity, 25);
    assert_eq!(cs.max_nesting_depth, 4);
    assert_eq!(cs.param_count, 3);
    assert_eq!(cs.call_count, 8);
    assert_eq!(cs.lambda_count, 1);
}

#[test]
fn test_index_file_defs_empty_input() {
    let mut index = DefinitionIndex::default();
    index.files.push("empty.cs".to_string());

    let call_sites = index_file_defs(&mut index, 0, vec![], vec![], vec![]);

    assert_eq!(call_sites, 0);
    assert!(index.definitions.is_empty());
    assert!(index.name_index.is_empty());
    assert!(index.kind_index.is_empty());
    assert!(index.file_index.is_empty());
}

#[test]
fn test_index_file_defs_multiple_files_sequential() {
    let mut index = DefinitionIndex::default();
    index.files.push("file0.cs".to_string());
    index.files.push("file1.cs".to_string());

    // Index file 0
    let defs0 = vec![
        DefinitionEntry {
            file_id: 0, name: "ClassA".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 10, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];
    index_file_defs(&mut index, 0, defs0, vec![], vec![]);

    // Index file 1
    let defs1 = vec![
        DefinitionEntry {
            file_id: 1, name: "ClassB".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 20, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];
    index_file_defs(&mut index, 1, defs1, vec![], vec![]);

    // Both should be in the index at correct positions
    assert_eq!(index.definitions.len(), 2);
    assert_eq!(index.definitions[0].name, "ClassA");
    assert_eq!(index.definitions[1].name, "ClassB");
    assert_eq!(index.name_index["classa"], vec![0]);
    assert_eq!(index.name_index["classb"], vec![1]);
    assert_eq!(index.file_index[&0], vec![0]);
    assert_eq!(index.file_index[&1], vec![1]);
    assert_eq!(index.kind_index[&DefinitionKind::Class], vec![0, 1]);
}

#[test]
fn test_collect_source_files_traverses_subdirectories() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    std::fs::create_dir(dir.join("sub1")).unwrap();
    std::fs::create_dir(dir.join("sub1").join("deep")).unwrap();
    std::fs::write(dir.join("root.cs"), "class Root {}").unwrap();
    std::fs::write(dir.join("sub1").join("inner.cs"), "class Inner {}").unwrap();
    std::fs::write(dir.join("sub1").join("deep").join("deep.cs"), "class Deep {}").unwrap();

    let extensions = vec!["cs".to_string()];
    let files = collect_source_files(dir, &extensions, 1);

    assert_eq!(files.len(), 3, "Should find files in root and all subdirectories");
}

#[test]
fn test_index_file_defs_skips_empty_call_site_vecs() {
    let mut index = DefinitionIndex::default();
    index.files.push("test.cs".to_string());

    let defs = vec![
        DefinitionEntry {
            file_id: 0, name: "MethodA".to_string(), kind: DefinitionKind::Method,
            line_start: 1, line_end: 10, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
        DefinitionEntry {
            file_id: 0, name: "MethodB".to_string(), kind: DefinitionKind::Method,
            line_start: 11, line_end: 20, parent: None, signature: None,
            modifiers: vec![], attributes: vec![], base_types: vec![],
        },
    ];

    // MethodA has empty call vec, MethodB has one call
    let calls = vec![
        (0, vec![]), // empty — should NOT be inserted
        (1, vec![CallSite { method_name: "DoWork".to_string(), receiver_type: None, line: 15, receiver_is_generic: false }]),
    ];

    let call_sites_added = index_file_defs(&mut index, 0, defs, calls, vec![]);

    assert_eq!(call_sites_added, 1, "Should only count non-empty call vecs");
    assert!(!index.method_calls.contains_key(&0), "Empty call vec should NOT be stored");
    assert!(index.method_calls.contains_key(&1), "Non-empty call vec should be stored");
}

#[test]
fn test_index_file_defs_duplicate_base_types() {
    let mut index = DefinitionIndex::default();
    index.files.push("test.cs".to_string());

    let defs = vec![
        DefinitionEntry {
            file_id: 0, name: "Handler".to_string(), kind: DefinitionKind::Class,
            line_start: 1, line_end: 10, parent: None, signature: None,
            modifiers: vec![],
            attributes: vec![],
            // Same base type listed twice (possible from parser)
            base_types: vec!["IService".to_string(), "IService".to_string()],
        },
    ];

    index_file_defs(&mut index, 0, defs, vec![], vec![]);

    // base_type_index will have two entries for "iservice" pointing to def 0.
    // This is the existing behavior — base_types are NOT deduplicated in the original.
    // The test documents this behavior explicitly.
    let bt_entries = &index.base_type_index["iservice"];
    assert_eq!(bt_entries.len(), 2, "Duplicate base types are stored as-is (no dedup)");
    assert_eq!(bt_entries, &vec![0, 0]);
}

// ─── Compile-time Guard: DefinitionIndex Field Count ────────────────

/// ██████████████████████████████████████████████████████████████████████
/// ██  COMPILE-TIME GUARD: DefinitionIndex Field Completeness         ██
/// ██████████████████████████████████████████████████████████████████████
///
/// PURPOSE:
///   This test exists to FORCE a compilation error when someone adds a
///   new field to DefinitionIndex. The error message ("missing field ...")
///   brings the developer HERE, where they see instructions to update
///   the incremental update functions.
///
/// WHY THIS MATTERS:
///   DefinitionIndex uses a Vec<DefinitionEntry> indexed by position (u32).
///   Secondary indexes like name_index, kind_index, etc. store these positions.
///   When a file is updated incrementally (--watch mode), old entries become
///   "tombstones" in the Vec. Three functions must handle this:
///
///   1. remove_file_definitions() — cleans secondary indexes when a file is removed
///   2. compact_definitions()     — remaps all def_idx values when Vec is compacted
///   3. update_file_definitions() — populates indexes when a file is re-parsed
///
///   If a NEW index using def_idx is added but these functions are NOT updated:
///   - remove_file_definitions: stale entries accumulate (memory leak)
///   - compact_definitions: old def_idx values point to WRONG definitions (silent corruption!)
///   - update_file_definitions: new index stays empty (missing data)
///
/// WHEN THIS TEST BREAKS:
///   1. You added a new field to DefinitionIndex — good!
///   2. Add the field to the constructor below
///   3. If the field uses def_idx (u32 index into definitions Vec):
///      → Update remove_file_definitions() in incremental.rs
///      → Update compact_definitions() in incremental.rs
///      → Update update_file_definitions() in incremental.rs
///   4. If the field does NOT use def_idx: just add it to Category C below
///
#[test]
fn test_definition_index_field_count_guard() {
    let _guard = DefinitionIndex {
        root: String::new(),
        format_version: 0,
        created_at: 0,
        extensions: Vec::new(),
        files: Vec::new(),
        definitions: Vec::new(),

        // ══════════════════════════════════════════════════════════════
        // CATEGORY A: def_idx as VALUES — HashMap<_, Vec<u32>>
        // On remove: retain() to filter out stale def_indices
        // On compact: remap_index_values(&mut index.NEW_FIELD, &remap)
        // ══════════════════════════════════════════════════════════════
        name_index: HashMap::new(),
        kind_index: HashMap::new(),
        attribute_index: HashMap::new(),
        base_type_index: HashMap::new(),
        file_index: HashMap::new(),
        selector_index: HashMap::new(),

        // ══════════════════════════════════════════════════════════════
        // CATEGORY B: def_idx as KEYS — HashMap<u32, _>
        // On remove: remove(&def_idx) for each removed def
        // On compact: drain().filter_map(|(k,v)| remap.get(&k).map(|&nk| (nk,v)))
        // ══════════════════════════════════════════════════════════════
        method_calls: HashMap::new(),
        code_stats: HashMap::new(),
        template_children: HashMap::new(),

        // ══════════════════════════════════════════════════════════════
        // CATEGORY C: NO def_idx — no compact/remove updates needed
        // ══════════════════════════════════════════════════════════════
        path_to_id: HashMap::new(),
        parse_errors: 0,
        lossy_file_count: 0,
        empty_file_ids: Vec::new(),
        extension_methods: HashMap::new(),
    };
    drop(_guard);
}


// ─── Tests for parse_file_standalone ────────────────────────────────

#[test]
fn test_parse_file_standalone_csharp() {
    let tmp = tempfile::tempdir().unwrap();
    let cs_file = tmp.path().join("UserService.cs");
    std::fs::write(&cs_file, r#"
public class UserService
{
    public void Process() { }
}
"#).unwrap();

    let result = super::incremental::parse_file_standalone(&cs_file, 99);
    assert!(result.is_some(), "Should parse C# file successfully");
    let result = result.unwrap();
    assert!(!result.definitions.is_empty(), "Should have definitions");
    assert_eq!(result.definitions[0].name, "UserService");
    // temp_file_id should be preserved in definitions
    assert_eq!(result.definitions[0].file_id, 99);
    assert_eq!(result.path, cs_file);
    assert!(!result.was_lossy);
}

#[test]
fn test_parse_file_standalone_rust() {
    let tmp = tempfile::tempdir().unwrap();
    let rs_file = tmp.path().join("lib.rs");
    std::fs::write(&rs_file, r#"
pub fn hello() -> String {
    "hello".to_string()
}
"#).unwrap();

    let result = super::incremental::parse_file_standalone(&rs_file, 0);
    assert!(result.is_some(), "Should parse Rust file successfully");
    let result = result.unwrap();
    assert!(result.definitions.iter().any(|d| d.name == "hello"));
}

#[test]
fn test_parse_file_standalone_unknown_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let txt_file = tmp.path().join("readme.txt");
    std::fs::write(&txt_file, "hello world").unwrap();

    let result = super::incremental::parse_file_standalone(&txt_file, 0);
    assert!(result.is_none(), "Unknown extension should return None");
}

#[test]
fn test_parse_file_standalone_nonexistent_file() {
    use std::path::PathBuf;
    let path = PathBuf::from("/tmp/nonexistent_file_12345.cs");
    let result = super::incremental::parse_file_standalone(&path, 0);
    assert!(result.is_none(), "Non-existent file should return None");
}

#[test]
fn test_parse_file_standalone_empty_csharp_file() {
    let tmp = tempfile::tempdir().unwrap();
    let cs_file = tmp.path().join("Empty.cs");
    std::fs::write(&cs_file, "// just a comment, no definitions").unwrap();

    let result = super::incremental::parse_file_standalone(&cs_file, 0);
    assert!(result.is_some(), "Valid extension should return Some even with 0 definitions");
    let result = result.unwrap();
    assert!(result.definitions.is_empty(), "Should have 0 definitions");
}

#[test]
fn test_parse_file_standalone_csharp_with_extension_methods() {
    let tmp = tempfile::tempdir().unwrap();
    let cs_file = tmp.path().join("Extensions.cs");
    std::fs::write(&cs_file, r#"
public static class StringExtensions
{
    public static string Capitalize(this string s) => s;
}
"#).unwrap();

    let result = super::incremental::parse_file_standalone(&cs_file, 0);
    assert!(result.is_some());
    let result = result.unwrap();
    assert!(!result.extension_methods.is_empty(), "Should capture extension methods");
    assert!(result.extension_methods.contains_key("Capitalize"));
}

// ─── Tests for apply_parsed_result ──────────────────────────────────

#[test]
fn test_apply_parsed_result_new_file() {
    use std::path::PathBuf;
    use super::types::*;
    use std::collections::HashMap;

    let mut index = DefinitionIndex::default();

    let result = ParsedFileResult {
        path: PathBuf::from("test.cs"),
        definitions: vec![
            DefinitionEntry {
                file_id: 99, // temp id — should be remapped
                name: "TestClass".to_string(),
                kind: DefinitionKind::Class,
                line_start: 1, line_end: 10,
                parent: None, signature: None,
                modifiers: vec![], attributes: vec![], base_types: vec![],
            },
        ],
        call_sites: vec![],
        code_stats: vec![],
        extension_methods: HashMap::new(),
        was_lossy: false,
    };

    super::incremental::apply_parsed_result(&mut index, result);

    // Verify file was registered
    assert_eq!(index.files.len(), 1);
    assert_eq!(index.files[0], "test.cs");
    assert!(index.path_to_id.contains_key(&PathBuf::from("test.cs")));

    // Verify definition was added with correct file_id (remapped from 99 to 0)
    assert_eq!(index.definitions.len(), 1);
    assert_eq!(index.definitions[0].file_id, 0);
    assert_eq!(index.definitions[0].name, "TestClass");

    // Verify indexes
    assert!(index.name_index.contains_key("testclass"));
    assert_eq!(index.file_index[&0], vec![0]);
}

#[test]
fn test_apply_parsed_result_existing_file_replaces_defs() {
    use std::path::PathBuf;
    use super::types::*;
    use std::collections::HashMap;

    let mut index = DefinitionIndex::default();

    // First: add a file with ClassA
    let result1 = ParsedFileResult {
        path: PathBuf::from("test.cs"),
        definitions: vec![
            DefinitionEntry {
                file_id: 0, name: "ClassA".to_string(), kind: DefinitionKind::Class,
                line_start: 1, line_end: 10, parent: None, signature: None,
                modifiers: vec![], attributes: vec![], base_types: vec![],
            },
        ],
        call_sites: vec![], code_stats: vec![],
        extension_methods: HashMap::new(), was_lossy: false,
    };
    super::incremental::apply_parsed_result(&mut index, result1);
    assert!(index.name_index.contains_key("classa"));

    // Second: update same file with ClassB (ClassA should be removed)
    let result2 = ParsedFileResult {
        path: PathBuf::from("test.cs"),
        definitions: vec![
            DefinitionEntry {
                file_id: 0, name: "ClassB".to_string(), kind: DefinitionKind::Class,
                line_start: 1, line_end: 20, parent: None, signature: None,
                modifiers: vec![], attributes: vec![], base_types: vec![],
            },
        ],
        call_sites: vec![], code_stats: vec![],
        extension_methods: HashMap::new(), was_lossy: false,
    };
    super::incremental::apply_parsed_result(&mut index, result2);

    // ClassA should be gone from name_index, ClassB should be present
    assert!(!index.name_index.contains_key("classa"), "ClassA should be removed");
    assert!(index.name_index.contains_key("classb"), "ClassB should be added");
    // File count should still be 1
    assert_eq!(index.files.len(), 1);
}

#[test]
fn test_apply_parsed_result_merges_extension_methods() {
    use std::path::PathBuf;
    use super::types::*;
    use std::collections::HashMap;

    let mut index = DefinitionIndex::default();

    let mut ext_methods = HashMap::new();
    ext_methods.insert("Capitalize".to_string(), vec!["StringExtensions".to_string()]);

    let result = ParsedFileResult {
        path: PathBuf::from("Extensions.cs"),
        definitions: vec![
            DefinitionEntry {
                file_id: 0, name: "StringExtensions".to_string(), kind: DefinitionKind::Class,
                line_start: 1, line_end: 10, parent: None, signature: None,
                modifiers: vec![], attributes: vec![], base_types: vec![],
            },
        ],
        call_sites: vec![], code_stats: vec![],
        extension_methods: ext_methods,
        was_lossy: false,
    };

    super::incremental::apply_parsed_result(&mut index, result);

    assert!(index.extension_methods.contains_key("Capitalize"));
    assert_eq!(index.extension_methods["Capitalize"], vec!["StringExtensions".to_string()]);
}

#[test]
fn test_apply_parsed_result_remaps_file_id_in_all_defs() {
    use std::path::PathBuf;
    use super::types::*;
    use std::collections::HashMap;

    let mut index = DefinitionIndex::default();
    // Pre-populate with one file to ensure new file gets id=1
    index.files.push("existing.cs".to_string());
    index.path_to_id.insert(PathBuf::from("existing.cs"), 0);

    let result = ParsedFileResult {
        path: PathBuf::from("new.cs"),
        definitions: vec![
            DefinitionEntry {
                file_id: 42, // temp id
                name: "ClassX".to_string(), kind: DefinitionKind::Class,
                line_start: 1, line_end: 5, parent: None, signature: None,
                modifiers: vec![], attributes: vec![], base_types: vec![],
            },
            DefinitionEntry {
                file_id: 42, // temp id
                name: "MethodY".to_string(), kind: DefinitionKind::Method,
                line_start: 2, line_end: 4, parent: Some("ClassX".to_string()),
                signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
            },
        ],
        call_sites: vec![], code_stats: vec![],
        extension_methods: HashMap::new(), was_lossy: false,
    };

    super::incremental::apply_parsed_result(&mut index, result);

    // Both definitions should have file_id=1 (remapped from 42)
    assert_eq!(index.definitions[0].file_id, 1);
    assert_eq!(index.definitions[1].file_id, 1);
    assert_eq!(index.file_index[&1], vec![0, 1]);
}

#[test]
fn test_apply_parsed_result_with_call_sites_and_code_stats() {
    use std::path::PathBuf;
    use super::types::*;
    use std::collections::HashMap;

    let mut index = DefinitionIndex::default();

    let result = ParsedFileResult {
        path: PathBuf::from("service.cs"),
        definitions: vec![
            DefinitionEntry {
                file_id: 0, name: "ServiceClass".to_string(), kind: DefinitionKind::Class,
                line_start: 1, line_end: 30, parent: None, signature: None,
                modifiers: vec![], attributes: vec![], base_types: vec![],
            },
            DefinitionEntry {
                file_id: 0, name: "Process".to_string(), kind: DefinitionKind::Method,
                line_start: 5, line_end: 25, parent: Some("ServiceClass".to_string()),
                signature: None, modifiers: vec![], attributes: vec![], base_types: vec![],
            },
        ],
        call_sites: vec![
            (1, vec![CallSite {
                method_name: "DoWork".to_string(),
                receiver_type: Some("IWorker".to_string()),
                line: 10,
                receiver_is_generic: false,
            }]),
        ],
        code_stats: vec![
            (1, CodeStats {
                cyclomatic_complexity: 5,
                cognitive_complexity: 8,
                max_nesting_depth: 2,
                param_count: 1,
                return_count: 1,
                call_count: 3,
                lambda_count: 0,
            }),
        ],
        extension_methods: HashMap::new(),
        was_lossy: false,
    };

    super::incremental::apply_parsed_result(&mut index, result);

    // Call sites for def_idx=1 (Process method)
    assert!(index.method_calls.contains_key(&1));
    assert_eq!(index.method_calls[&1][0].method_name, "DoWork");

    // Code stats for def_idx=1
    assert!(index.code_stats.contains_key(&1));
    assert_eq!(index.code_stats[&1].cyclomatic_complexity, 5);
}

// ─── Tests for reconcile_definition_index_nonblocking ───────────────

#[test]
fn test_reconcile_nonblocking_adds_new_files() {
    use std::sync::{Arc, RwLock};

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("NewClass.cs"), "public class NewClass { }").unwrap();

    let index = DefinitionIndex {
        root: dir.to_string_lossy().to_string(),
        extensions: vec!["cs".to_string()],
        created_at: 0, // very old — everything is "new"
        ..Default::default()
    };
    let arc_index = Arc::new(RwLock::new(index));

    let (added, _modified, removed) = super::incremental::reconcile_definition_index_nonblocking(
        &arc_index,
        &dir.to_string_lossy(),
        &["cs".to_string()],
    );

    assert!(added > 0, "Should detect new files, got added={}", added);
    assert_eq!(removed, 0);

    let idx = arc_index.read().unwrap();
    assert!(idx.name_index.contains_key("newclass"));
}

#[test]
fn test_reconcile_nonblocking_removes_deleted_files() {
    use std::sync::{Arc, RwLock};
    use std::path::PathBuf;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Create index with a file that doesn't exist on disk
    let mut index = DefinitionIndex {
        root: dir.to_string_lossy().to_string(),
        extensions: vec!["cs".to_string()],
        created_at: 32503680000, // year 3000 — nothing is "modified"
        files: vec!["C:/nonexistent/DeletedFile.cs".to_string()],
        ..Default::default()
    };
    index.path_to_id.insert(PathBuf::from("C:/nonexistent/DeletedFile.cs"), 0);
    // Add a definition for this file
    index.definitions.push(DefinitionEntry {
        file_id: 0, name: "DeletedClass".to_string(), kind: DefinitionKind::Class,
        line_start: 1, line_end: 5, parent: None, signature: None,
        modifiers: vec![], attributes: vec![], base_types: vec![],
    });
    index.name_index.insert("deletedclass".to_string(), vec![0]);
    index.kind_index.insert(DefinitionKind::Class, vec![0]);
    index.file_index.insert(0, vec![0]);

    let arc_index = Arc::new(RwLock::new(index));

    let (_added, _modified, removed) = super::incremental::reconcile_definition_index_nonblocking(
        &arc_index,
        &dir.to_string_lossy(),
        &["cs".to_string()],
    );

    assert_eq!(removed, 1, "Should detect deleted file");

    let idx = arc_index.read().unwrap();
    assert!(!idx.name_index.contains_key("deletedclass"), "Deleted file's definitions should be removed");
    assert!(!idx.path_to_id.contains_key(&PathBuf::from("C:/nonexistent/DeletedFile.cs")));
}

#[test]
fn test_reconcile_nonblocking_no_changes() {
    use std::sync::{Arc, RwLock};

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Empty directory, empty index — nothing to do
    let index = DefinitionIndex {
        root: dir.to_string_lossy().to_string(),
        extensions: vec!["cs".to_string()],
        ..Default::default()
    };
    let arc_index = Arc::new(RwLock::new(index));

    let (added, modified, removed) = super::incremental::reconcile_definition_index_nonblocking(
        &arc_index,
        &dir.to_string_lossy(),
        &["cs".to_string()],
    );

    assert_eq!(added, 0);
    assert_eq!(modified, 0);
    assert_eq!(removed, 0);
}


#[test]
fn test_update_file_definitions_removes_stale_defs_when_file_emptied() {
    // Regression test: when a file that had definitions is modified to have 0 definitions,
    // the old definitions must be removed. Previously parse_file_standalone returned None
    // for empty-defs files, and update_file_definitions did nothing → stale defs remained.
    use std::path::PathBuf;

    let tmp = tempfile::tempdir().unwrap();
    let cs_file = tmp.path().join("Service.cs");

    // Step 1: Create file with a class
    std::fs::write(&cs_file, "public class MyService { }").unwrap();
    let mut index = DefinitionIndex::default();
    let clean = PathBuf::from(crate::clean_path(&cs_file.to_string_lossy()));
    super::incremental::update_file_definitions(&mut index, &clean);
    assert!(index.name_index.contains_key("myservice"), "Should have MyService in index");

    // Step 2: Modify file to have 0 definitions
    std::fs::write(&cs_file, "// now empty, no classes").unwrap();
    super::incremental::update_file_definitions(&mut index, &clean);

    // MyService should be gone (apply_parsed_result removes old defs then adds empty)
    assert!(!index.name_index.contains_key("myservice"),
        "Stale definitions should be removed when file becomes empty");
}


// ─── Additional tests from code review round 2 ─────────────────────

#[test]
fn test_parse_file_standalone_typescript() {
    let tmp = tempfile::tempdir().unwrap();
    let ts_file = tmp.path().join("service.ts");
    std::fs::write(&ts_file, r#"
export class UserService {
    process(): void { }
}
"#).unwrap();

    let result = super::incremental::parse_file_standalone(&ts_file, 0);
    assert!(result.is_some(), "Should parse TypeScript file");
    let result = result.unwrap();
    assert!(result.definitions.iter().any(|d| d.name == "UserService"),
        "Should find UserService class, got: {:?}", result.definitions.iter().map(|d| &d.name).collect::<Vec<_>>());
}

#[test]
fn test_parse_file_standalone_sql() {
    let tmp = tempfile::tempdir().unwrap();
    let sql_file = tmp.path().join("schema.sql");
    std::fs::write(&sql_file, r#"
CREATE TABLE Users (
    Id INT PRIMARY KEY,
    Name NVARCHAR(100)
);
"#).unwrap();

    let result = super::incremental::parse_file_standalone(&sql_file, 0);
    assert!(result.is_some(), "Should parse SQL file");
    let result = result.unwrap();
    assert!(result.definitions.iter().any(|d| d.name == "Users"),
        "Should find Users table, got: {:?}", result.definitions.iter().map(|d| &d.name).collect::<Vec<_>>());
}

#[test]
fn test_reconcile_nonblocking_detects_modified_files() {
    use std::sync::{Arc, RwLock};
    use std::path::PathBuf;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let cs_file = dir.join("Service.cs");
    std::fs::write(&cs_file, "public class OldService { }").unwrap();

    // Build initial index with the file
    let mut index = DefinitionIndex::default();
    let clean = PathBuf::from(crate::clean_path(&cs_file.to_string_lossy()));
    super::incremental::update_file_definitions(&mut index, &clean);
    assert!(index.name_index.contains_key("oldservice"));

    // Set created_at to 0 so all files appear "modified" (mtime > threshold)
    index.created_at = 0;

    // Modify the file content
    std::fs::write(&cs_file, "public class NewService { }").unwrap();

    let arc_index = Arc::new(RwLock::new(index));

    let (added, modified, _removed) = super::incremental::reconcile_definition_index_nonblocking(
        &arc_index,
        &dir.to_string_lossy(),
        &["cs".to_string()],
    );

    // Should detect as modified (not added, since it was already in path_to_id)
    assert!(modified > 0 || added > 0, "Should detect modified file");

    let idx = arc_index.read().unwrap();
    assert!(idx.name_index.contains_key("newservice"),
        "Should have NewService after modification");
    // OldService should be gone (apply_parsed_result removes old defs)
    assert!(!idx.name_index.contains_key("oldservice"),
        "OldService should be removed after modification");
}

#[test]
fn test_update_file_definitions_file_becomes_unreadable() {
    // When a previously indexed file becomes unreadable (deleted/permission error),
    // update_file_definitions should remove old definitions.
    use std::path::PathBuf;

    let tmp = tempfile::tempdir().unwrap();
    let cs_file = tmp.path().join("Service.cs");

    // Step 1: Create and index the file
    std::fs::write(&cs_file, "public class GoneService { }").unwrap();
    let mut index = DefinitionIndex::default();
    let clean = PathBuf::from(crate::clean_path(&cs_file.to_string_lossy()));
    super::incremental::update_file_definitions(&mut index, &clean);
    assert!(index.name_index.contains_key("goneservice"));

    // Step 2: Delete the file (making it unreadable)
    std::fs::remove_file(&cs_file).unwrap();

    // Step 3: Try to update — should remove old definitions
    super::incremental::update_file_definitions(&mut index, &clean);
    assert!(!index.name_index.contains_key("goneservice"),
        "Old definitions should be removed when file becomes unreadable");
}


// ─── Chunked def-build tests ────────────────────────────────────────

/// Verify that chunked def-build (MACRO_CHUNK_SIZE=4096) produces correct results
/// when building an index with multiple files across potentially multiple sub-chunks.
/// Tests that def count, call site count, code stats count, and name_index are correct.
#[cfg(feature = "lang-csharp")]
#[test]
fn test_chunked_def_build_multiple_files_correct_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Create 20 C# files with classes, methods, and cross-references
    for i in 0..20 {
        let content = format!(
            r#"
public class Service{i} {{
    private readonly ILogger _logger;

    public void Process{i}() {{
        _logger.LogInformation("Processing");
    }}

    public int Calculate{i}(int x) {{
        return x * {i};
    }}
}}
"#,
            i = i
        );
        std::fs::write(dir.join(format!("Service{}.cs", i)), content).unwrap();
    }

    let idx = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 4, // Use multiple threads to exercise sub-chunking
    });

    // Should find all 20 classes
    assert_eq!(
        idx.definitions.iter().filter(|d| d.kind == DefinitionKind::Class).count(),
        20,
        "Should find all 20 classes"
    );

    // Should find all 40 methods (2 per class × 20)
    let method_count = idx.definitions.iter()
        .filter(|d| d.kind == DefinitionKind::Method)
        .count();
    assert_eq!(method_count, 40, "Should find all 40 methods (2 per class)");

    // Should have call sites (each method calls _logger.LogInformation)
    let total_call_sites: usize = idx.method_calls.values().map(|v| v.len()).sum();
    assert!(total_call_sites > 0, "Should have call sites from method bodies");

    // Should have code stats for methods
    assert!(!idx.code_stats.is_empty(), "Should have code stats for methods");

    // All 20 class names should be in name_index
    for i in 0..20 {
        let name = format!("service{}", i);
        assert!(idx.name_index.contains_key(&name),
            "Should find class Service{} in name_index", i);
    }

    // file_ids should be consistent: each file_id in definitions should correspond
    // to a valid file in idx.files
    for def in &idx.definitions {
        assert!((def.file_id as usize) < idx.files.len(),
            "file_id {} should be within files range ({})", def.file_id, idx.files.len());
    }
}

/// Verify that chunked def-build with a single thread produces same results
/// as multi-threaded build (exercises different sub-chunk sizes).
#[cfg(feature = "lang-csharp")]
#[test]
fn test_chunked_def_build_single_vs_multi_thread_consistency() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    for i in 0..10 {
        let content = format!(
            "public class Item{i} {{ public void Execute{i}() {{ }} }}", i = i
        );
        std::fs::write(dir.join(format!("Item{}.cs", i)), content).unwrap();
    }

    let idx_single = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 1,
    });

    let idx_multi = build_definition_index(&DefIndexArgs {
        dir: dir.to_string_lossy().to_string(),
        ext: "cs".to_string(),
        threads: 4,
    });

    assert_eq!(idx_single.definitions.len(), idx_multi.definitions.len(),
        "Single-threaded and multi-threaded builds should produce same def count");
    assert_eq!(idx_single.code_stats.len(), idx_multi.code_stats.len(),
        "Single-threaded and multi-threaded builds should produce same code stats count");

    let calls_single: usize = idx_single.method_calls.values().map(|v| v.len()).sum();
    let calls_multi: usize = idx_multi.method_calls.values().map(|v| v.len()).sum();
    assert_eq!(calls_single, calls_multi,
        "Single-threaded and multi-threaded builds should produce same call site count");
}
