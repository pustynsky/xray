//! Persistence for DefinitionIndex: save/load/find on disk.

use std::path::PathBuf;

use crate::clean_path;

use super::types::DefinitionIndex;

pub fn definition_index_path_for(dir: &str, exts: &str, index_base: &std::path::Path) -> PathBuf {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
    let hash = search_index::stable_hash(&[
        canonical.to_string_lossy().as_bytes(),
        exts.as_bytes(),
        b"definitions", // distinguish from content index
    ]);
    let prefix = search_index::extract_semantic_prefix(&canonical);
    index_base.join(format!("{}_{:08x}.code-structure", prefix, hash as u32))
}

pub fn save_definition_index(index: &DefinitionIndex, index_base: &std::path::Path) -> Result<(), crate::SearchError> {
    std::fs::create_dir_all(index_base)?;
    let exts_str = index.extensions.join(",");
    let path = definition_index_path_for(&index.root, &exts_str, index_base);
    crate::index::save_compressed(&path, index, "definition-index")?;
    crate::index::save_index_meta(&path, &crate::index::definition_index_meta(index));
    Ok(())
}

#[allow(dead_code)]
pub fn load_definition_index(dir: &str, exts: &str, index_base: &std::path::Path) -> Result<DefinitionIndex, crate::SearchError> {
    let path = definition_index_path_for(dir, exts, index_base);
    crate::index::load_compressed(&path, "definition-index")
}

/// Try to find any definition index for a directory.
///
/// When `expected_exts` is non-empty, the cached index must contain ALL
/// of the expected extensions (superset check). If the cached index is
/// missing any expected extension, it is skipped so the caller can
/// trigger a full rebuild with the correct extensions.
///
/// This prevents a stale cache (e.g., built with `--ext cs` only) from
/// being used when the server now requires `--ext cs,sql`.
#[allow(dead_code)]
pub fn find_definition_index_for_dir(dir: &str, index_base: &std::path::Path, expected_exts: &[String]) -> Option<DefinitionIndex> {
    let canonical = std::fs::canonicalize(dir).ok()?;
    let dir_str = clean_path(&canonical.to_string_lossy());
    let entries = std::fs::read_dir(index_base).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "code-structure") {
            match crate::index::load_compressed::<DefinitionIndex>(&path, "definition-index") {
                Ok(index) => {
                    let idx_root = std::fs::canonicalize(&index.root)
                        .map(|p| clean_path(&p.to_string_lossy()))
                        .unwrap_or_else(|_| index.root.clone());
                    if idx_root.eq_ignore_ascii_case(&dir_str) {
                        // Validate that cached index has ALL expected extensions
                        if !expected_exts.is_empty() {
                            let has_all = expected_exts.iter().all(|ext|
                                index.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
                            );
                            if !has_all {
                                eprintln!("[find_definition_index] Skipping {} — extensions mismatch (cached: {:?}, expected: {:?})",
                                    path.display(), index.extensions, expected_exts);
                                continue;
                            }
                        }
                        return Some(index);
                    }
                }
                Err(e) => {
                    eprintln!("[find_definition_index] Skipping {}: {}", path.display(), e);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
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
}