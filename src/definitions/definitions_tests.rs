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
        attribute_index: HashMap::new(), base_type_index: HashMap::new(),
        file_index: { let mut m = HashMap::new(); m.insert(0, vec![0]); m },
        path_to_id: HashMap::new(), method_calls: HashMap::new(), code_stats: HashMap::new(), parse_errors: 0, lossy_file_count: 0, empty_file_ids: Vec::new(), extension_methods: HashMap::new(), selector_index: HashMap::new(), template_children: HashMap::new(),
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
