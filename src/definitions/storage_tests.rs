#![allow(clippy::field_reassign_with_default)] // tests prefer mutate-after-default for readability
use super::*;
use crate::definitions::DefinitionIndex;

/// Create a minimal DefinitionIndex with given root and extensions, save to disk.
fn save_test_def_index(root: &str, exts: &[&str], index_base: &std::path::Path) {
    let idx = DefinitionIndex {
        root: root.to_string(),
        format_version: crate::definitions::DEFINITION_INDEX_VERSION,
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

// ─── find_definition_index_for_dir meta-based optimization tests ─────

/// Verify that find_definition_index_for_dir skips non-matching indexes
/// without loading the full index when .meta sidecar files are present.
#[test]
fn test_find_def_index_uses_meta_to_skip_non_matching_root() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let dir_a = tmp.path().join("project_a");
    let dir_b = tmp.path().join("project_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let root_a = clean_path(&dir_a.to_string_lossy());
    let root_b = clean_path(&dir_b.to_string_lossy());

    // Save def index for project_a
    save_test_def_index(&root_a, &["rs"], index_base);

    // Searching for project_b should NOT find project_a's index
    let result = find_definition_index_for_dir(&root_b, index_base, &[]);
    assert!(result.is_none(),
        "Should not find project_a's index when searching for project_b");

    // Searching for project_a SHOULD find it
    let result = find_definition_index_for_dir(&root_a, index_base, &[]);
    assert!(result.is_some(),
        "Should find project_a's index when searching for project_a");
}

/// Verify that find_definition_index_for_dir works when .meta file is missing.
#[test]
fn test_find_def_index_works_without_meta_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save def index (creates both .code-structure and .code-structure.meta)
    save_test_def_index(&root_str, &["rs", "md"], index_base);

    // Delete the .meta sidecar file
    for entry in std::fs::read_dir(index_base).unwrap().flatten() {
        let path = entry.path();
        if path.to_string_lossy().ends_with(".meta") {
            std::fs::remove_file(&path).unwrap();
        }
    }

    // Should still find the index via fallback
    let expected = vec!["rs".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_some(),
        "Should find index even without .meta sidecar (fallback path)");
}

/// Verify that meta-based filtering correctly rejects extension mismatches.
#[test]
fn test_find_def_index_meta_rejects_extension_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let index_base = tmp.path();

    let root_dir = tmp.path().join("project");
    std::fs::create_dir_all(&root_dir).unwrap();
    let root_str = clean_path(&root_dir.to_string_lossy());

    // Save def index with only "rs" extension
    save_test_def_index(&root_str, &["rs"], index_base);

    // Request "rs,sql" — meta should reject because "sql" is not in cached extensions
    let expected = vec!["rs".to_string(), "sql".to_string()];
    let result = find_definition_index_for_dir(&root_str, index_base, &expected);
    assert!(result.is_none(),
        "Meta-based filtering should reject when cached extensions don't include all expected");
}

// ─── Definition index format_version tests ──────────────────────────

#[test]
fn test_def_index_format_version_correct_loads_ok() {
    use crate::definitions::DEFINITION_INDEX_VERSION;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let mut idx = crate::definitions::DefinitionIndex::default();
    idx.format_version = DEFINITION_INDEX_VERSION;
    idx.root = root.clone();
    idx.extensions = vec!["rs".to_string()];

    let path = definition_index_path_for(&root, "rs", tmp.path());
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = load_definition_index(&root, "rs", tmp.path());
    assert!(result.is_ok(), "Loading definition index with correct version should succeed");
    assert_eq!(result.unwrap().format_version, DEFINITION_INDEX_VERSION);
}

#[test]
fn test_def_index_format_version_mismatch_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let mut idx = crate::definitions::DefinitionIndex::default();
    idx.format_version = 999; // wrong version
    idx.root = root.clone();
    idx.extensions = vec!["rs".to_string()];

    let path = definition_index_path_for(&root, "rs", tmp.path());
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = load_definition_index(&root, "rs", tmp.path());
    assert!(result.is_err(), "Loading definition index with wrong version should fail");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("format version mismatch"), "Error should mention version mismatch, got: {}", err);
}

#[test]
fn test_def_index_format_version_legacy_zero_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_string_lossy().to_string();
    let mut idx = crate::definitions::DefinitionIndex::default();
    idx.format_version = 0; // legacy
    idx.root = root.clone();
    idx.extensions = vec!["rs".to_string()];

    let path = definition_index_path_for(&root, "rs", tmp.path());
    crate::index::save_compressed(&path, &idx, "test").unwrap();

    let result = load_definition_index(&root, "rs", tmp.path());
    assert!(result.is_err(), "Loading legacy definition index (version 0) should fail");
}
