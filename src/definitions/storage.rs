//! Persistence for DefinitionIndex: save/load/find on disk.

use std::path::PathBuf;

use crate::clean_path;

use super::types::DefinitionIndex;

pub fn definition_index_path_for(dir: &str, exts: &str, index_base: &std::path::Path) -> PathBuf {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
    let hash = code_xray::stable_hash(&[
        canonical.to_string_lossy().as_bytes(),
        exts.as_bytes(),
        b"definitions", // distinguish from content index
    ]);
    let prefix = code_xray::extract_semantic_prefix(&canonical);
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

pub fn load_definition_index(dir: &str, exts: &str, index_base: &std::path::Path) -> Result<DefinitionIndex, crate::SearchError> {
    let path = definition_index_path_for(dir, exts, index_base);

    // Fast version check BEFORE full deserialization — reads ~100 bytes via LZ4
    // streaming decompression. Prevents OOM/abort from old indexes with shifted layout.
    match crate::index::read_format_version_from_index_file(&path) {
        Some(v) if v != super::types::DEFINITION_INDEX_VERSION => {
            eprintln!(
                "[definition-index] Format version mismatch (found {}, expected {}), index outdated",
                v, super::types::DEFINITION_INDEX_VERSION
            );
            return Err(crate::SearchError::IndexLoad {
                path: path.display().to_string(),
                message: format!("format version mismatch: found {}, expected {}", v, super::types::DEFINITION_INDEX_VERSION),
            });
        }
        None => {
            eprintln!("[definition-index] Cannot read format version from {}, index outdated", path.display());
            return Err(crate::SearchError::IndexLoad {
                path: path.display().to_string(),
                message: "cannot read format version (legacy or corrupt index)".to_string(),
            });
        }
        Some(_) => {} // version matches, proceed to full load
    }

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
///
/// **Optimization:** Uses `.meta` sidecar files to check root and extensions
/// without loading the full index (~200 bytes vs 100+ MB). Falls back to
/// `read_root_from_index_file` if no sidecar exists. Only loads the full
/// index after metadata confirms a match.
pub fn find_definition_index_for_dir(dir: &str, index_base: &std::path::Path, expected_exts: &[String]) -> Option<DefinitionIndex> {
    let canonical = std::fs::canonicalize(dir).ok()?;
    let dir_str = clean_path(&canonical.to_string_lossy());
    let entries = std::fs::read_dir(index_base).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "code-structure") {
            continue;
        }

        // ── Fast filter via .meta sidecar (~200 bytes, no full deserialization) ──
        if let Some(meta) = crate::index::load_index_meta(&path) {
            // Canonicalize meta root for case-insensitive comparison (Windows-safe)
            let meta_root = std::fs::canonicalize(&meta.root)
                .map(|p| clean_path(&p.to_string_lossy()))
                .unwrap_or_else(|_| meta.root.clone());
            if !meta_root.eq_ignore_ascii_case(&dir_str) {
                continue; // root doesn't match — skip without loading
            }
            // Check extension superset from metadata
            if !expected_exts.is_empty() {
                let has_all = expected_exts.iter().all(|ext|
                    meta.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
                );
                if !has_all {
                    eprintln!("[find_definition_index] Skipping {} — extensions mismatch (cached: {:?}, expected: {:?})",
                        path.display(), meta.extensions, expected_exts);
                    continue;
                }
            }
            // Metadata matches — check version before full load (reads ~100 bytes, 1 LZ4 block)
            match crate::index::read_format_version_from_index_file(&path) {
                Some(v) if v != super::types::DEFINITION_INDEX_VERSION => {
                    eprintln!("[find_definition_index] Skipping {} — format version mismatch (found {}, expected {})",
                        path.display(), v, super::types::DEFINITION_INDEX_VERSION);
                    continue;
                }
                None => {
                    eprintln!("[find_definition_index] Cannot read version from {}, skipping", path.display());
                    continue;
                }
                Some(_) => {}
            }
            match crate::index::load_compressed::<DefinitionIndex>(&path, "definition-index") {
                Ok(index) => return Some(index),
                Err(e) => {
                    eprintln!("[find_definition_index] Metadata matched but load failed for {}: {}", path.display(), e);
                    continue;
                }
            }
        }

        // ── Fallback: no .meta sidecar — try lightweight root + version check ──
        if let Some(root) = crate::index::read_root_from_index_file_pub(&path) {
            let root_canonical = std::fs::canonicalize(&root)
                .map(|p| clean_path(&p.to_string_lossy()))
                .unwrap_or_else(|_| clean_path(&root));
            if !root_canonical.eq_ignore_ascii_case(&dir_str) {
                continue; // root doesn't match — skip without loading
            }
        }
        // Check version before full deserialization (reads ~100 bytes, 1 LZ4 block)
        match crate::index::read_format_version_from_index_file(&path) {
            Some(v) if v != super::types::DEFINITION_INDEX_VERSION => {
                eprintln!("[find_definition_index] Skipping {} — format version mismatch (found {}, expected {})",
                    path.display(), v, super::types::DEFINITION_INDEX_VERSION);
                continue;
            }
            None => {
                eprintln!("[find_definition_index] Cannot read version from {}, skipping", path.display());
                continue;
            }
            Some(_) => {}
        }
        // Root + version OK — load full index
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
    None
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
