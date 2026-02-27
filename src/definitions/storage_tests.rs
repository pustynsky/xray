use super::*;
use crate::definitions::DefinitionIndex;

/// Create a minimal DefinitionIndex with given root and extensions, save to disk.
fn save_test_def_index(root: &str, exts: &[&str], index_base: &std::path::Path) {
    let idx = DefinitionIndex {
        root: root.to_string(),
        extensions: exts.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    save_definition_index(&idx, index_base).unwrap();
}

#[test]
fn test_find_def_index_skips_stale_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    // Create a real directory for the root (canonicalize needs it)
    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with only "cs" extension
    save_test_def_index(&root_str, &["cs"], index_base);

    // Request "cs,sql" — should NOT find the old cs-only index
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_none(),
        "Should not find cs-only index when cs,sql is expected");
}

#[test]
fn test_find_def_index_accepts_superset() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with "cs,sql,ts" extensions
    save_test_def_index(&root_str, &["cs", "sql", "ts"], index_base);

    // Request "cs,sql" — should find the superset index
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Should find cs,sql,ts index when cs,sql is expected (superset)");
}

#[test]
fn test_find_def_index_accepts_exact_match() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with "cs,sql" extensions
    save_test_def_index(&root_str, &["cs", "sql"], index_base);

    // Request "cs,sql" — should find the exact match
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Should find cs,sql index when cs,sql is expected (exact match)");
}

#[test]
fn test_find_def_index_empty_expected_accepts_any() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with "cs" extension
    save_test_def_index(&root_str, &["cs"], index_base);

    // Request with empty expected — should accept any
    let result = find_definition_index_for_dir(&root_str, index_base, &[]);
    assert!(result.is_some(),
        "Empty expected_exts should accept any cached index (backward compatible)");
}

#[test]
fn test_find_def_index_case_insensitive_ext_match() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save an index with uppercase extensions
    save_test_def_index(&root_str, &["CS", "SQL"], index_base);

    // Request lowercase — should still match
    let expected = vec!["cs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Extension matching should be case-insensitive");
}
