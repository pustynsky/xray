//! Persistence for DefinitionIndex: save/load/find on disk.

use std::path::PathBuf;

use crate::clean_path;

use super::types::DefinitionIndex;

pub fn definition_index_path_for(dir: &str, exts: &str, index_base: &std::path::Path) -> PathBuf {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
    let hash = search::stable_hash(&[
        canonical.to_string_lossy().as_bytes(),
        exts.as_bytes(),
        b"definitions", // distinguish from content index
    ]);
    let prefix = search::extract_semantic_prefix(&canonical);
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

/// Try to find any definition index for a directory (any extension combo)
#[allow(dead_code)]
pub fn find_definition_index_for_dir(dir: &str, index_base: &std::path::Path) -> Option<DefinitionIndex> {
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