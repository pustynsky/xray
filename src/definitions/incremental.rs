//! Incremental updates for DefinitionIndex (used by file watcher).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use tracing::{info, warn};

use crate::{canonicalize_or_warn, clean_path, read_file_lossy};
use super::{index_file_defs, types::*};
#[cfg(feature = "lang-csharp")]
use super::parser_csharp::parse_csharp_definitions;
#[cfg(feature = "lang-typescript")]
use super::parser_typescript::parse_typescript_definitions;
use super::parser_sql::parse_sql_definitions;
#[cfg(feature = "lang-rust")]
use super::parser_rust::parse_rust_definitions;

/// Parse a file WITHOUT accessing the DefinitionIndex.
///
/// Returns a `ParsedFileResult` containing all parsed data ready to be applied.
/// The `temp_file_id` is a placeholder — it will be remapped during `apply_parsed_result()`.
/// This function is safe to call without any lock.
pub fn parse_file_standalone(path: &Path, temp_file_id: u32) -> Option<ParsedFileResult> {
    let (content, was_lossy) = read_file_lossy(path).ok()?;
    if was_lossy {
        warn!("File contains non-UTF8 bytes (lossy conversion applied): {}", path.display());
    }

    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let ext_lower = ext.to_lowercase();

    #[cfg_attr(not(feature = "lang-csharp"), allow(unused_mut))]
    let mut extension_methods = HashMap::new();

    let (defs, calls, stats) = match ext_lower.as_str() {
        #[cfg(feature = "lang-csharp")]
        "cs" => {
            let mut cs_parser = tree_sitter::Parser::new();
            cs_parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).ok();
            let (defs, calls, stats, ext_methods) =
                parse_csharp_definitions(&mut cs_parser, &content, temp_file_id);
            extension_methods = ext_methods;
            (defs, calls, stats)
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
            parse_typescript_definitions(&mut ts_parser, &content, temp_file_id)
        }
        "sql" => {
            parse_sql_definitions(&content, temp_file_id)
        }
        #[cfg(feature = "lang-rust")]
        "rs" => {
            let mut rs_parser = tree_sitter::Parser::new();
            rs_parser.set_language(&tree_sitter_rust::LANGUAGE.into()).ok();
            parse_rust_definitions(&mut rs_parser, &content, temp_file_id)
        }
        _ => return None,
    };

    Some(ParsedFileResult {
        path: path.to_path_buf(),
        definitions: defs,
        call_sites: calls,
        code_stats: stats,
        extension_methods,
    })
}

/// Parse a file using pre-created parsers (for parallel parsing).
/// Unlike `parse_file_standalone()` which creates a new parser per call,
/// this function reuses parsers across files within the same thread.
fn parse_file_with_parsers(
    path: &Path,
    temp_file_id: u32,
    #[cfg(feature = "lang-csharp")] cs_parser: &mut tree_sitter::Parser,
    #[cfg(feature = "lang-typescript")] ts_parser: &mut Option<tree_sitter::Parser>,
    #[cfg(feature = "lang-typescript")] tsx_parser: &mut Option<tree_sitter::Parser>,
    #[cfg(feature = "lang-rust")] rs_parser: &mut Option<tree_sitter::Parser>,
) -> Option<ParsedFileResult> {
    let (content, was_lossy) = read_file_lossy(path).ok()?;
    if was_lossy {
        warn!("File contains non-UTF8 bytes (lossy conversion applied): {}", path.display());
    }

    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let ext_lower = ext.to_lowercase();
    #[cfg_attr(not(feature = "lang-csharp"), allow(unused_mut))]
    let mut extension_methods = HashMap::new();

    let (defs, calls, stats) = match ext_lower.as_str() {
        #[cfg(feature = "lang-csharp")]
        "cs" => {
            let (defs, calls, stats, ext_methods) =
                parse_csharp_definitions(cs_parser, &content, temp_file_id);
            extension_methods = ext_methods;
            (defs, calls, stats)
        }
        #[cfg(feature = "lang-typescript")]
        "ts" => {
            let parser = ts_parser.get_or_insert_with(|| {
                let mut p = tree_sitter::Parser::new();
                p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).ok();
                p
            });
            parse_typescript_definitions(parser, &content, temp_file_id)
        }
        #[cfg(feature = "lang-typescript")]
        "tsx" => {
            let parser = tsx_parser.get_or_insert_with(|| {
                let mut p = tree_sitter::Parser::new();
                p.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into()).ok();
                p
            });
            parse_typescript_definitions(parser, &content, temp_file_id)
        }
        "sql" => {
            parse_sql_definitions(&content, temp_file_id)
        }
        #[cfg(feature = "lang-rust")]
        "rs" => {
            let parser = rs_parser.get_or_insert_with(|| {
                let mut p = tree_sitter::Parser::new();
                p.set_language(&tree_sitter_rust::LANGUAGE.into()).ok();
                p
            });
            parse_rust_definitions(parser, &content, temp_file_id)
        }
        _ => return None,
    };

    Some(ParsedFileResult {
        path: path.to_path_buf(),
        definitions: defs,
        call_sites: calls,
        code_stats: stats,
        extension_methods,
    })
}

/// Apply pre-parsed file results to the index.
/// This is the ONLY function that needs `&mut DefinitionIndex`.
/// Typically runs in <1ms per file.
pub fn apply_parsed_result(
    index: &mut DefinitionIndex,
    result: ParsedFileResult,
) {
    let path = &result.path;
    let path_str = path.to_string_lossy().to_string();

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

    // Remap temp file_id to actual file_id in all definitions
    let mut defs = result.definitions;
    for def in &mut defs {
        def.file_id = file_id;
    }

    // Apply definitions, call sites, code stats
    index_file_defs(index, file_id, defs, result.call_sites, result.code_stats);

    // Merge extension methods (C#-specific)
    for (method_name, classes) in result.extension_methods {
        index.extension_methods.entry(method_name).or_default().extend(classes);
    }
}

/// Update definitions for a single file (incremental).
/// Removes old definitions for the file, parses it again, adds new ones.
/// This is a convenience wrapper around `parse_file_standalone()` + `apply_parsed_result()`.
#[cfg(test)]
pub fn update_file_definitions(index: &mut DefinitionIndex, path: &Path) {
    // Determine a temp file_id for parsing (we use 0 since it will be remapped in apply)
    let temp_file_id = 0u32;

    match parse_file_standalone(path, temp_file_id) {
        Some(result) => apply_parsed_result(index, result),
        None => {
            // File couldn't be read or extension not supported.
            // If the file was previously indexed, remove its old definitions
            // to avoid stale data (e.g., file became unreadable).
            if let Some(&file_id) = index.path_to_id.get(path) {
                remove_file_definitions(index, file_id);
            }
        }
    }
}

/// Remove all definitions for a file from the index
pub fn remove_file_definitions(index: &mut DefinitionIndex, file_id: u32) {
    // DEF-S-002: clear stale `empty_file_ids` entry FIRST, before the early
    // return below. A file that was previously empty has no `file_index` entry,
    // so the early `None => return` would skip this cleanup otherwise — leaving
    // its (file_id, size) tuple in `empty_file_ids` forever and inflating the
    // on-disk index plus audit reports.
    index.empty_file_ids.retain(|(id, _)| *id != file_id);

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
#[cfg(test)]
pub fn reconcile_definition_index(
    index: &mut DefinitionIndex,
    dir: &str,
    extensions: &[String],
) -> (usize, usize, usize) {
    let start = std::time::Instant::now();
    let walk_start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_secs();

    let dir_path = canonicalize_or_warn(dir);

    // Threshold: files modified after (created_at - 2s) are considered potentially stale
    let threshold = UNIX_EPOCH + Duration::from_secs(index.created_at.saturating_sub(2));

    // Walk filesystem to collect all matching files with their mtime
    let mut disk_files: HashMap<PathBuf, SystemTime> = HashMap::new();

    let mut walker = WalkBuilder::new(&dir_path);
    walker.follow_links(true).hidden(false).git_ignore(true).git_exclude(false);

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

    // Update created_at if anything changed (use walk_start, not now(), to avoid race condition)
    if added > 0 || modified > 0 || removed > 0 {
        index.created_at = walk_start;
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

/// Non-blocking reconciliation: parse files OUTSIDE the lock, apply INSIDE.
///
/// Unlike `reconcile_definition_index()` which holds a write lock for the entire duration
/// (including parsing), this function only holds locks briefly:
/// - Phase 1: Walk filesystem (NO lock) ~3s
/// - Phase 2: Read lock to determine changed files (~instant)
/// - Phase 3: Parse all changed files (NO lock) — the slow part
/// - Phase 4: Write lock to apply results (<500ms)
///
/// During Phase 3, MCP requests work normally on the old index data.
pub fn reconcile_definition_index_nonblocking(
    def_index: &Arc<RwLock<DefinitionIndex>>,
    dir: &str,
    extensions: &[String],
) -> (usize, usize, usize) {
    let start = std::time::Instant::now();
    // Capture walk start time for created_at update (not now() at end — avoids race condition
    // where files modified during parsing phase would be missed by next reconciliation)
    let walk_start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_secs();

    let dir_path = canonicalize_or_warn(dir);

    // ── Phase 1: Walk filesystem (NO lock needed) ──
    let mut disk_files: HashMap<PathBuf, SystemTime> = HashMap::new();

    let mut walker = WalkBuilder::new(&dir_path);
    walker.follow_links(true).hidden(false).git_ignore(true).git_exclude(false);

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

    // ── Phase 2: Determine changed files (READ lock — instant) ──
    let (_threshold, to_update, to_remove, added, modified) = {
        let idx = match def_index.read() {
            Ok(idx) => idx,
            Err(e) => {
                tracing::error!(error = %e, "Failed to acquire def index read lock for reconciliation");
                return (0, 0, 0);
            }
        };
        let threshold = UNIX_EPOCH + Duration::from_secs(idx.created_at.saturating_sub(2));

        let mut to_update: Vec<PathBuf> = Vec::new();
        let mut to_remove: Vec<PathBuf> = Vec::new();
        let mut added = 0usize;
        let mut modified = 0usize;

        for (path, mtime) in &disk_files {
            if !idx.path_to_id.contains_key(path) {
                to_update.push(path.clone());
                added += 1;
            } else if *mtime > threshold {
                to_update.push(path.clone());
                modified += 1;
            }
        }

        for path in idx.path_to_id.keys() {
            if !disk_files.contains_key(path) {
                to_remove.push(path.clone());
            }
        }

        (threshold, to_update, to_remove, added, modified)
    };
    // READ lock released here

    let removed = to_remove.len();

    if to_update.is_empty() && to_remove.is_empty() {
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        info!(
            scanned,
            elapsed_ms = format_args!("{:.1}", elapsed_ms),
            "Definition index reconciliation (non-blocking): all files up to date"
        );
        return (0, 0, 0);
    }

    // ── Phase 3: Parse ALL files in parallel (NO lock needed) ──
    // During this phase, MCP requests work normally on the old index data!
    // Collect to_update paths as a HashSet for post-parse cleanup.
    let to_update_set: HashSet<PathBuf> = to_update.iter().cloned().collect();
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let chunk_size = to_update.len().div_ceil(num_threads).max(1);

    let parsed_results: Vec<ParsedFileResult> = if to_update.len() <= 1 {
        // Single file — no need for thread overhead
        to_update.iter()
            .enumerate()
            .filter_map(|(i, path)| parse_file_standalone(path, i as u32))
            .collect()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = to_update.chunks(chunk_size)
                .enumerate()
                .map(|(chunk_idx, chunk)| {
                    s.spawn(move || {
                        // Create parsers ONCE per thread (like build_definition_index)
                        #[cfg(feature = "lang-csharp")]
                        let mut cs_parser = {
                            let mut p = tree_sitter::Parser::new();
                            p.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).ok();
                            p
                        };
                        #[cfg(feature = "lang-typescript")]
                        let mut ts_parser: Option<tree_sitter::Parser> = None;
                        #[cfg(feature = "lang-typescript")]
                        let mut tsx_parser: Option<tree_sitter::Parser> = None;
                        #[cfg(feature = "lang-rust")]
                        let mut rs_parser: Option<tree_sitter::Parser> = None;

                        chunk.iter()
                            .enumerate()
                            .filter_map(|(i, path)| {
                                let temp_id = (chunk_idx * chunk_size + i) as u32;
                                parse_file_with_parsers(
                                    path, temp_id,
                                    #[cfg(feature = "lang-csharp")]
                                    &mut cs_parser,
                                    #[cfg(feature = "lang-typescript")]
                                    &mut ts_parser,
                                    #[cfg(feature = "lang-typescript")]
                                    &mut tsx_parser,
                                    #[cfg(feature = "lang-rust")]
                                    &mut rs_parser,
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();

            handles.into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    };

    // ── Phase 4: Apply results (WRITE lock — brief, <500ms) ──
    {
        let mut idx = match def_index.write() {
            Ok(idx) => idx,
            Err(e) => {
                tracing::error!(error = %e, "Failed to acquire def index write lock for reconciliation");
                return (0, 0, 0);
            }
        };

        // Remove deleted files
        for path in &to_remove {
            if let Some(&file_id) = idx.path_to_id.get(path) {
                remove_file_definitions(&mut idx, file_id);
                idx.path_to_id.remove(path);
            }
        }

        // Apply parsed results
        let mut applied_paths: HashSet<PathBuf> = HashSet::with_capacity(parsed_results.len());
        for result in parsed_results {
            applied_paths.insert(result.path.clone());
            apply_parsed_result(&mut idx, result);
        }

        // Clean up files that were in to_update but didn't produce a ParsedFileResult
        // (e.g., read error). Without this, stale definitions remain for unreadable files.
        for path in &to_update_set {
            if !applied_paths.contains(path)
                && let Some(&file_id) = idx.path_to_id.get(path) {
                    remove_file_definitions(&mut idx, file_id);
                }
        }

        // Update created_at if anything changed (use walk_start, not now(), to avoid race condition)
        if added > 0 || modified > 0 || removed > 0 {
            idx.created_at = walk_start;
        }
    }
    // WRITE lock released here

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    info!(
        scanned,
        added,
        modified,
        removed,
        elapsed_ms = format_args!("{:.1}", elapsed_ms),
        "Definition index reconciliation complete (non-blocking)"
    );

    crate::index::log_memory(&format!(
        "watcher: def reconciliation non-blocking (scanned={}, added={}, modified={}, removed={}, {:.0}ms)",
        scanned, added, modified, removed, elapsed_ms
    ));

    (added, modified, removed)
}