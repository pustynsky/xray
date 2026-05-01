//! Persistence for DefinitionIndex: save/load/find on disk.

use std::path::PathBuf;

use tracing::{debug, warn};

use crate::{canonicalize_or_warn, clean_path, path_eq};

use super::types::DefinitionIndex;

pub fn definition_index_path_for(dir: &str, exts: &str, index_base: &std::path::Path) -> PathBuf {
    let canonical = canonicalize_or_warn(dir);
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
    let head = index.build_head();
    // Borrowed entries — cheap O(n) pointer collect, no per-entry clone.
    let definitions: Vec<&super::types::DefinitionEntry> = index.definitions.iter().collect();
    crate::index::save_sharded(&path, &head, definitions, "definition-index")?;
    crate::index::save_index_meta(&path, &crate::index::definition_index_meta(index));
    Ok(())
}

pub fn load_definition_index(dir: &str, exts: &str, index_base: &std::path::Path) -> Result<DefinitionIndex, crate::SearchError> {
    let path = definition_index_path_for(dir, exts, index_base);
    load_definition_index_at_path(&path)
}

/// Load a definition index from an explicit path. Performs the same fast
/// version-check the directory-keyed `load_definition_index` does, then
/// dispatches by file magic: `SHARD_MAGIC` -> sharded parallel decode,
/// `LZ4_MAGIC` -> legacy single-frame load. Legacy fallback exists only
/// for tests that still write via raw `save_compressed`; production saves
/// always use the sharded path.
pub fn load_definition_index_at_path(path: &std::path::Path) -> Result<DefinitionIndex, crate::SearchError> {
    use std::io::Read;

    // Fast version check BEFORE full deserialization — reads ~100 bytes via LZ4
    // streaming decompression (legacy) or 12 bytes of plain header (sharded).
    // Prevents OOM/abort from old indexes with shifted layout.
    match crate::index::read_format_version_from_index_file(path) {
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
            warn!(target: "xray::definitions", path = %path.display(), "definition-index: cannot read format version, treating as outdated");
            return Err(crate::SearchError::IndexLoad {
                path: path.display().to_string(),
                message: "cannot read format version (legacy or corrupt index)".to_string(),
            });
        }
        Some(_) => {} // version matches, proceed to full load
    }

    // Dispatch by magic so test harnesses that still use raw `save_compressed`
    // (LZ4_MAGIC, single frame) keep loading. Production save path emits
    // SHARD_MAGIC.
    let mut magic = [0u8; 4];
    {
        let mut f = std::fs::File::open(path).map_err(|e| crate::SearchError::IndexLoad {
            path: path.display().to_string(),
            message: format!("cannot open file: {}", e),
        })?;
        f.read_exact(&mut magic).map_err(|e| crate::SearchError::IndexLoad {
            path: path.display().to_string(),
            message: format!("read error (magic): {}", e),
        })?;
    }

    if &magic == crate::index::SHARD_MAGIC {
        let (head, definitions) = crate::index::load_sharded::<
            super::types::DefinitionIndexHead,
            super::types::DefinitionEntry,
        >(path, "definition-index")?;
        Ok(DefinitionIndex::from_head_and_entries(head, definitions))
    } else {
        // Legacy LZ4_MAGIC or pre-LZ4 plain bincode — keep working for tests.
        crate::index::load_compressed(path, "definition-index")
    }
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
            if !path_eq(&meta_root, &dir_str) {
                continue; // root doesn't match — skip without loading
            }
            // Check extension superset from metadata
            if !expected_exts.is_empty() {
                let has_all = expected_exts.iter().all(|ext|
                    meta.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
                );
                if !has_all {
                    debug!(target: "xray::definitions", path = %path.display(), cached = ?meta.extensions, expected = ?expected_exts, "find_definition_index: skipping (extensions mismatch via meta)");
                    continue;
                }
            }
            // Metadata matches — check version before full load (reads ~100 bytes, 1 LZ4 block)
            match crate::index::read_format_version_from_index_file(&path) {
                Some(v) if v != super::types::DEFINITION_INDEX_VERSION => {
                    debug!(target: "xray::definitions", path = %path.display(), found = v, expected = super::types::DEFINITION_INDEX_VERSION, "find_definition_index: skipping (format version mismatch)");
                    continue;
                }
                None => {
                    warn!(target: "xray::definitions", path = %path.display(), "find_definition_index: cannot read version, skipping");
                    continue;
                }
                Some(_) => {}
            }
            match load_definition_index_at_path(&path) {
                Ok(index) => return Some(index),
                Err(e) => {
                    warn!(target: "xray::definitions", path = %path.display(), error = %e, "find_definition_index: metadata matched but load failed");
                    continue;
                }
            }
        }

        // ── Fallback: no .meta sidecar — try lightweight root + version check ──
        if let Some(root) = crate::index::read_root_from_index_file_pub(&path) {
            let root_canonical = std::fs::canonicalize(&root)
                .map(|p| clean_path(&p.to_string_lossy()))
                .unwrap_or_else(|_| clean_path(&root));
            if !path_eq(&root_canonical, &dir_str) {
                continue; // root doesn't match — skip without loading
            }
        }
        // Check version before full deserialization (reads ~100 bytes, 1 LZ4 block)
        match crate::index::read_format_version_from_index_file(&path) {
            Some(v) if v != super::types::DEFINITION_INDEX_VERSION => {
                debug!(target: "xray::definitions", path = %path.display(), found = v, expected = super::types::DEFINITION_INDEX_VERSION, "find_definition_index: skipping (format version mismatch, fallback)");
                continue;
            }
            None => {
                warn!(target: "xray::definitions", path = %path.display(), "find_definition_index: cannot read version, skipping (fallback)");
                continue;
            }
            Some(_) => {}
        }
        // Root + version OK — load full index
        match load_definition_index_at_path(&path) {
            Ok(index) => {
                let idx_root = std::fs::canonicalize(&index.root)
                    .map(|p| clean_path(&p.to_string_lossy()))
                    .unwrap_or_else(|_| index.root.clone());
                if path_eq(&idx_root, &dir_str) {
                    // Validate that cached index has ALL expected extensions
                    if !expected_exts.is_empty() {
                        let has_all = expected_exts.iter().all(|ext|
                            index.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
                        );
                        if !has_all {
                            debug!(target: "xray::definitions", path = %path.display(), cached = ?index.extensions, expected = ?expected_exts, "find_definition_index: skipping (extensions mismatch via full load)");
                            continue;
                        }
                    }
                    return Some(index);
                }
            }
            Err(e) => {
                warn!(target: "xray::definitions", path = %path.display(), error = %e, "find_definition_index: skipping (load error)");
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
