//! Incremental updates for DefinitionIndex (used by file watcher).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use tracing::{info, warn};

use crate::{clean_path, read_file_lossy};
use super::types::*;
#[cfg(feature = "lang-csharp")]
use super::parser_csharp::parse_csharp_definitions;
#[cfg(feature = "lang-typescript")]
use super::parser_typescript::parse_typescript_definitions;
#[cfg(feature = "lang-sql")]
use super::parser_sql::parse_sql_definitions;
#[cfg(feature = "lang-rust")]
use super::parser_rust::parse_rust_definitions;

/// Update definitions for a single file (incremental).
/// Removes old definitions for the file, parses it again, adds new ones.
pub fn update_file_definitions(index: &mut DefinitionIndex, path: &Path) {
    let path_str = path.to_string_lossy().to_string();

    #[allow(unused_variables)]
    let (content, was_lossy) = match read_file_lossy(path) {
        Ok(r) => r,
        Err(_) => return,
    };
    if was_lossy {
        warn!("File contains non-UTF8 bytes (lossy conversion applied): {}", path_str);
    }

    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Get or assign file_id
    let file_id = if let Some(&id) = index.path_to_id.get(path) {
        // Existing file — remove old definitions
        remove_file_definitions(index, id);
        id
    } else {
        // New file
        let id = index.files.len() as u32;
        index.files.push(path_str);
        index.path_to_id.insert(path.to_path_buf(), id);
        id
    };

    // Parse the file
    let ext_lower = ext.to_lowercase();
    let (file_defs, file_calls, file_stats) = match ext_lower.as_str() {
        #[cfg(feature = "lang-csharp")]
        "cs" => {
            let mut cs_parser = tree_sitter::Parser::new();
            cs_parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).ok();
            {
                let (defs, calls, stats, _ext) = parse_csharp_definitions(&mut cs_parser, &content, file_id);
                (defs, calls, stats)
            }
        }
        #[cfg(feature = "lang-typescript")]
        "ts" | "tsx" => {
            let mut ts_parser = tree_sitter::Parser::new();
            let ts_lang = if ext_lower == "tsx" {
                tree_sitter_typescript::LANGUAGE_TSX
            } else {
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT
            };
            ts_parser.set_language(&ts_lang.into()).ok();
            parse_typescript_definitions(&mut ts_parser, &content, file_id)
        }
        #[cfg(feature = "lang-sql")]
        "sql" => {
            parse_sql_definitions(&content, file_id)
        }
        #[cfg(feature = "lang-rust")]
        "rs" => {
            let mut rs_parser = tree_sitter::Parser::new();
            rs_parser.set_language(&tree_sitter_rust::LANGUAGE.into()).ok();
            parse_rust_definitions(&mut rs_parser, &content, file_id)
        }
        _ => (Vec::<DefinitionEntry>::new(), Vec::<(usize, Vec<CallSite>)>::new(), Vec::<(usize, CodeStats)>::new()),
    };

    // Add new definitions to index
    let base_def_idx = index.definitions.len() as u32;

    for def in file_defs {
        let def_idx = index.definitions.len() as u32;

        index.name_index.entry(def.name.to_lowercase())
            .or_default()
            .push(def_idx);

        index.kind_index.entry(def.kind)
            .or_default()
            .push(def_idx);

        {
            let mut seen_attrs = std::collections::HashSet::new();
            for attr in &def.attributes {
                let attr_name = attr.split('(').next().unwrap_or(attr).trim().to_lowercase();
                if seen_attrs.insert(attr_name.clone()) {
                    index.attribute_index.entry(attr_name)
                        .or_default()
                        .push(def_idx);
                }
            }
        }

        for bt in &def.base_types {
            index.base_type_index.entry(bt.to_lowercase())
                .or_default()
                .push(def_idx);
        }

        index.file_index.entry(file_id)
            .or_default()
            .push(def_idx);

        index.definitions.push(def);
    }

    // Add call sites for new definitions
    for (local_idx, calls) in file_calls {
        let global_idx = base_def_idx + local_idx as u32;
        if !calls.is_empty() {
            index.method_calls.insert(global_idx, calls);
        }
    }

    // Add code stats for new definitions
    for (local_idx, stats) in file_stats {
        let global_idx = base_def_idx + local_idx as u32;
        index.code_stats.insert(global_idx, stats);
    }
}

/// Remove all definitions for a file from the index
pub fn remove_file_definitions(index: &mut DefinitionIndex, file_id: u32) {
    let def_indices = match index.file_index.remove(&file_id) {
        Some(indices) => indices,
        None => return,
    };

    let indices_set: std::collections::HashSet<u32> = def_indices.iter().cloned().collect();

    // Remove call graph and code stats entries
    for &di in &def_indices {
        index.method_calls.remove(&di);
        index.code_stats.remove(&di);
    }

    index.name_index.retain(|_, v| {
        v.retain(|idx| !indices_set.contains(idx));
        !v.is_empty()
    });

    index.kind_index.retain(|_, v| {
        v.retain(|idx| !indices_set.contains(idx));
        !v.is_empty()
    });

    index.attribute_index.retain(|_, v| {
        v.retain(|idx| !indices_set.contains(idx));
        !v.is_empty()
    });

    index.base_type_index.retain(|_, v| {
        v.retain(|idx| !indices_set.contains(idx));
        !v.is_empty()
    });

    // Clean Angular-specific indexes (selector_index stores Vec<u32> of def_idx,
    // template_children is keyed by def_idx)
    index.selector_index.retain(|_, v| {
        v.retain(|idx| !indices_set.contains(idx));
        !v.is_empty()
    });

    index.template_children.retain(|k, _| !indices_set.contains(k));

    // Conditionally shrink secondary index vecs after retain() to release excess capacity.
    // Only shrink when capacity > 2 × len to avoid unnecessary realloc storms.
    // retain() reduces len but not capacity — shrink_to_fit() reclaims dead allocations.
    for v in index.name_index.values_mut() {
        if v.capacity() > v.len() * 2 { v.shrink_to_fit(); }
    }
    for v in index.kind_index.values_mut() {
        if v.capacity() > v.len() * 2 { v.shrink_to_fit(); }
    }
    for v in index.attribute_index.values_mut() {
        if v.capacity() > v.len() * 2 { v.shrink_to_fit(); }
    }
    for v in index.base_type_index.values_mut() {
        if v.capacity() > v.len() * 2 { v.shrink_to_fit(); }
    }

    // Shrink the HashMaps themselves (only if significantly over-allocated)
    if index.name_index.capacity() > index.name_index.len() * 2 {
        index.name_index.shrink_to_fit();
    }
    if index.kind_index.capacity() > index.kind_index.len() * 2 {
        index.kind_index.shrink_to_fit();
    }
    if index.attribute_index.capacity() > index.attribute_index.len() * 2 {
        index.attribute_index.shrink_to_fit();
    }
    if index.base_type_index.capacity() > index.base_type_index.len() * 2 {
        index.base_type_index.shrink_to_fit();
    }
    if index.method_calls.capacity() > index.method_calls.len() * 2 {
        index.method_calls.shrink_to_fit();
    }
    if index.code_stats.capacity() > index.code_stats.len() * 2 {
        index.code_stats.shrink_to_fit();
    }

    // Auto-compact when tombstone ratio exceeds 3× (67% waste)
    let active_count: usize = index.file_index.values().map(|v| v.len()).sum();
    let total_count = index.definitions.len();
    if total_count > 0 && total_count > active_count * 3 {
        info!(
            total = total_count,
            active = active_count,
            waste_pct = ((total_count - active_count) * 100) / total_count,
            "Definition index tombstone threshold exceeded, compacting"
        );
        compact_definitions(index);
    }
}

/// Remove a file entirely from the definition index
pub fn remove_file_from_def_index(index: &mut DefinitionIndex, path: &Path) {
    if let Some(&file_id) = index.path_to_id.get(path) {
        remove_file_definitions(index, file_id);
        index.path_to_id.remove(path);
    }
}

/// Compact the definition index by removing tombstoned entries from the Vec
/// and remapping all secondary indexes to the new positions.
///
/// Tombstones accumulate when files are updated incrementally: old entries
/// remain in `definitions` Vec but are no longer referenced by `file_index`.
/// This function rebuilds the Vec with only active entries and updates all
/// 9 secondary indexes that reference `def_idx` positions.
///
/// ⚠️ When adding new indexes with def_idx references to DefinitionIndex,
/// update this function to remap the new index as well.
pub fn compact_definitions(index: &mut DefinitionIndex) {
    let active_set: HashSet<u32> = index.file_index.values()
        .flat_map(|v| v.iter().copied()).collect();

    if active_set.len() == index.definitions.len() {
        return; // nothing to compact
    }

    let before = index.definitions.len();

    // Build new Vec + old→new mapping
    let mut new_defs = Vec::with_capacity(active_set.len());
    let mut remap: HashMap<u32, u32> = HashMap::with_capacity(active_set.len());
    for old_idx in 0..index.definitions.len() as u32 {
        if active_set.contains(&old_idx) {
            remap.insert(old_idx, new_defs.len() as u32);
            new_defs.push(index.definitions[old_idx as usize].clone());
        }
    }

    // Remap all secondary indexes that store Vec<u32> values (def_idx references)
    remap_index_values(&mut index.name_index, &remap);
    remap_index_values(&mut index.kind_index, &remap);
    remap_index_values(&mut index.attribute_index, &remap);
    remap_index_values(&mut index.base_type_index, &remap);
    remap_index_values(&mut index.file_index, &remap);
    remap_index_values(&mut index.selector_index, &remap);

    // Remap HashMap<u32, _> keyed indexes
    index.method_calls = index.method_calls.drain()
        .filter_map(|(k, v)| remap.get(&k).map(|&new_k| (new_k, v)))
        .collect();
    index.code_stats = index.code_stats.drain()
        .filter_map(|(k, v)| remap.get(&k).map(|&new_k| (new_k, v)))
        .collect();
    index.template_children = index.template_children.drain()
        .filter_map(|(k, v)| remap.get(&k).map(|&new_k| (new_k, v)))
        .collect();

    let after = new_defs.len();
    index.definitions = new_defs;

    info!(
        before,
        after,
        removed = before - after,
        "Definition index compacted"
    );
}

/// Remap def_idx values in a HashMap<K, Vec<u32>> secondary index.
fn remap_index_values<K: Eq + Hash>(map: &mut HashMap<K, Vec<u32>>, remap: &HashMap<u32, u32>) {
    for v in map.values_mut() {
        for idx in v.iter_mut() {
            if let Some(&new_idx) = remap.get(idx) {
                *idx = new_idx;
            }
        }
    }
}

/// Return the number of active (non-tombstoned) definitions in the index.
/// Active definitions are those referenced by `file_index`.
#[allow(dead_code)]
pub fn active_definition_count(index: &DefinitionIndex) -> usize {
    index.file_index.values().map(|v| v.len()).sum()
}

/// Reconcile definition index with filesystem after loading from disk cache.
///
/// Walks the filesystem and compares with the in-memory index to find:
/// - **Added** files: exist on disk but not in `path_to_id` → parse and add
/// - **Modified** files: exist in both but `mtime > index.created_at` → re-parse
/// - **Deleted** files: exist in `path_to_id` but not on disk → remove
///
/// Uses a 2-second safety margin on `created_at` to handle clock precision.
/// WalkBuilder provides mtime via `entry.metadata()` — no extra `stat()` calls needed.
///
/// Returns `(added, modified, removed)` counts.
pub fn reconcile_definition_index(
    index: &mut DefinitionIndex,
    dir: &str,
    extensions: &[String],
) -> (usize, usize, usize) {
    let start = std::time::Instant::now();

    let dir_path = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));

    // Threshold: files modified after (created_at - 2s) are considered potentially stale
    let threshold = UNIX_EPOCH + Duration::from_secs(index.created_at.saturating_sub(2));

    // Walk filesystem to collect all matching files with their mtime
    let mut disk_files: HashMap<PathBuf, SystemTime> = HashMap::new();

    let mut walker = WalkBuilder::new(&dir_path);
    walker.hidden(false).git_ignore(true);

    for entry in walker.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let ext_match = path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)));
        if !ext_match {
            continue;
        }
        let mtime = entry.metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(UNIX_EPOCH);
        let clean = PathBuf::from(clean_path(&path.to_string_lossy()));
        disk_files.insert(clean, mtime);
    }

    let scanned = disk_files.len();

    // Collect indexed paths for deletion check
    let indexed_paths: HashSet<PathBuf> = index.path_to_id.keys().cloned().collect();

    let mut added = 0usize;
    let mut modified = 0usize;
    let mut removed = 0usize;

    // Check for new and modified files
    for (path, mtime) in &disk_files {
        if !index.path_to_id.contains_key(path) {
            // NEW file — not in index
            update_file_definitions(index, path);
            added += 1;
        } else if *mtime > threshold {
            // MODIFIED file — mtime is newer than index build time
            update_file_definitions(index, path);
            modified += 1;
        }
        // else: unchanged — skip
    }

    // Check for deleted files (in index but not on disk)
    for path in &indexed_paths {
        if !disk_files.contains_key(path) {
            remove_file_from_def_index(index, path);
            removed += 1;
        }
    }

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    if added > 0 || modified > 0 || removed > 0 {
        info!(
            scanned,
            added,
            modified,
            removed,
            elapsed_ms = format_args!("{:.1}", elapsed_ms),
            "Definition index reconciliation complete"
        );
    } else {
        info!(
            scanned,
            elapsed_ms = format_args!("{:.1}", elapsed_ms),
            "Definition index reconciliation: all files up to date"
        );
    }

    crate::index::log_memory(&format!(
        "watcher: def reconciliation (scanned={}, added={}, modified={}, removed={}, {:.0}ms)",
        scanned, added, modified, removed, elapsed_ms
    ));

    (added, modified, removed)
}