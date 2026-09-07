//! Incremental updates for DefinitionIndex (used by file watcher).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use tracing::{info, warn};

use crate::{canonicalize_or_warn, clean_path, is_inside_git_dir};
use super::{
    definition_input_key, index_file_defs_with_semantics,
    index_parsed_angular_components, read_definition_source_snapshot, types::*,
};
use super::csharp_semantics::CSharpFileContribution;
#[cfg(feature = "lang-csharp")]
use super::parser_csharp::parse_csharp_definitions_with_semantics;
#[cfg(feature = "lang-typescript")]
use super::parser_typescript::parse_typescript_definitions_with_components;
use super::parser_sql::parse_sql_definitions;
#[cfg(feature = "lang-rust")]
use super::parser_rust::parse_rust_definitions;

/// Parse a file WITHOUT accessing the DefinitionIndex.
///
/// Returns a `ParsedFileResult` containing all parsed data ready to be applied.
/// The `temp_file_id` is a placeholder — it will be remapped during `apply_parsed_result()`.
/// This function is safe to call without any lock.
#[cfg(test)]
pub fn parse_file_standalone(path: &Path, temp_file_id: u32) -> Option<ParsedFileResult> {
    try_parse_file_standalone(path, temp_file_id).ok().flatten()
}

pub(crate) fn try_parse_file_standalone(
    path: &Path,
    temp_file_id: u32,
) -> std::io::Result<Option<ParsedFileResult>> {
    let (content, was_lossy, fingerprint) = read_definition_source_snapshot(path)?;
    if was_lossy {
        warn!("File contains non-UTF8 bytes (lossy conversion applied): {}", path.display());
    }

    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let ext_lower = ext.to_lowercase();

    #[cfg_attr(not(feature = "lang-csharp"), allow(unused_mut))]
    let mut extension_methods = HashMap::new();
    #[cfg_attr(not(feature = "lang-csharp"), allow(unused_mut))]
    let mut csharp_semantics = CSharpFileContribution::default();
    #[cfg_attr(not(feature = "lang-typescript"), allow(unused_mut))]
    let mut angular_components = Vec::new();

    let (defs, calls, stats) = match ext_lower.as_str() {
        #[cfg(feature = "lang-csharp")]
        "cs" => {
            let mut cs_parser = tree_sitter::Parser::new();
            cs_parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).ok();
            let (defs, calls, stats, ext_methods, semantics) =
                parse_csharp_definitions_with_semantics(&mut cs_parser, &content, temp_file_id);
            extension_methods = ext_methods;
            csharp_semantics = semantics;
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
            let (parsed, components) = parse_typescript_definitions_with_components(
                &mut ts_parser,
                &content,
                temp_file_id,
            );
            angular_components = components;
            parsed
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
        _ => return Ok(None),
    };

    Ok(Some(ParsedFileResult {
        path: path.to_path_buf(),
        definitions: defs,
        call_sites: calls,
        code_stats: stats,
        extension_methods,
        csharp_semantics,
        angular_components,
        fingerprint,
    }))
}

pub(crate) fn is_transient_definition_input_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::Interrupted
    )
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
) -> std::io::Result<Option<ParsedFileResult>> {
    let (content, was_lossy, fingerprint) = read_definition_source_snapshot(path)?;
    if was_lossy {
        warn!("File contains non-UTF8 bytes (lossy conversion applied): {}", path.display());
    }

    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let ext_lower = ext.to_lowercase();
    #[cfg_attr(not(feature = "lang-csharp"), allow(unused_mut))]
    let mut extension_methods = HashMap::new();
    #[cfg_attr(not(feature = "lang-csharp"), allow(unused_mut))]
    let mut csharp_semantics = CSharpFileContribution::default();
    #[cfg_attr(not(feature = "lang-typescript"), allow(unused_mut))]
    let mut angular_components = Vec::new();

    let (defs, calls, stats) = match ext_lower.as_str() {
        #[cfg(feature = "lang-csharp")]
        "cs" => {
            let (defs, calls, stats, ext_methods, semantics) =
                parse_csharp_definitions_with_semantics(cs_parser, &content, temp_file_id);
            extension_methods = ext_methods;
            csharp_semantics = semantics;
            (defs, calls, stats)
        }
        #[cfg(feature = "lang-typescript")]
        "ts" => {
            let parser = ts_parser.get_or_insert_with(|| {
                let mut p = tree_sitter::Parser::new();
                p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).ok();
                p
            });
            let (parsed, components) =
                parse_typescript_definitions_with_components(parser, &content, temp_file_id);
            angular_components = components;
            parsed
        }
        #[cfg(feature = "lang-typescript")]
        "tsx" => {
            let parser = tsx_parser.get_or_insert_with(|| {
                let mut p = tree_sitter::Parser::new();
                p.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into()).ok();
                p
            });
            let (parsed, components) =
                parse_typescript_definitions_with_components(parser, &content, temp_file_id);
            angular_components = components;
            parsed
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
        _ => return Ok(None),
    };

    Ok(Some(ParsedFileResult {
        path: path.to_path_buf(),
        definitions: defs,
        call_sites: calls,
        code_stats: stats,
        extension_methods,
        csharp_semantics,
        angular_components,
        fingerprint,
    }))
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
    let input_key = definition_input_key(path);

    // Get or assign file_id
    let file_id = if let Some(&id) = index.path_to_id.get(&crate::path_identity_key(path)) {
        // Existing file — remove old definitions
        remove_file_definitions(index, id);
        id
    } else {
        // New file
        let id = index.files.len() as u32;
        index.files.push(path_str);
        index.path_to_id.insert(crate::path_identity_key(path), id);
        id
    };

    // Remap temp file_id to actual file_id in all definitions
    let mut defs = result.definitions;
    for def in &mut defs {
        def.file_id = file_id;
    }

    let base_def_idx = index.definitions.len() as u32;
    let definition_count = defs.len();
    let angular_components = result.angular_components;

    let mut csharp_semantics = result.csharp_semantics;
    if csharp_semantics.extension_methods.is_empty() {
        csharp_semantics.extension_methods = result.extension_methods.clone();
    }


    // Apply definitions, call sites, code stats
    index_file_defs_with_semantics(
        index,
        file_id,
        defs,
        result.call_sites,
        result.code_stats,
        csharp_semantics,
    );
    index_parsed_angular_components(
        index,
        base_def_idx,
        definition_count,
        angular_components,
    );

    index.extension_methods = index.csharp_semantics.merged_extension_methods();
    index.input_fingerprints.insert(input_key, result.fingerprint);
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
            if let Some(&file_id) = index.path_to_id.get(&crate::path_identity_key(path)) {
                remove_file_definitions(index, file_id);
            }
        }
    }
    compact_definitions_if_needed(index);
}

/// Remove all definitions for a file from the index.
/// Call `compact_definitions_if_needed` after the surrounding update batch completes.
pub fn remove_file_definitions(index: &mut DefinitionIndex, file_id: u32) {
    // DEF-S-002: clear stale `empty_file_ids` entry FIRST, before the early
    // return below. A file that was previously empty has no `file_index` entry,
    // so the early `None => return` would skip this cleanup otherwise — leaving
    // its (file_id, size) tuple in `empty_file_ids` forever and inflating the
    // on-disk index plus audit reports.
    index.empty_file_ids.retain(|(id, _)| *id != file_id);

    let def_indices = match index.file_index.remove(&file_id) {
        Some(indices) => indices,
        None => {
            index.csharp_semantics.remove_file_contribution(
                file_id,
                &std::collections::HashSet::new(),
            );
            index.extension_methods = index.csharp_semantics.merged_extension_methods();
            return;
        }
    };

    let indices_set: std::collections::HashSet<u32> = def_indices.iter().cloned().collect();

    index.csharp_semantics.remove_file_contribution(file_id, &indices_set);
    index.extension_methods = index.csharp_semantics.merged_extension_methods();

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
    let affected_template_paths: Vec<String> = index
        .template_owners
        .iter()
        .filter(|(_, owners)| owners.iter().any(|idx| indices_set.contains(idx)))
        .map(|(path, _)| path.clone())
        .collect();
    index.template_owners.retain(|_, v| {
        v.retain(|idx| !indices_set.contains(idx));
        !v.is_empty()
    });
    for path in affected_template_paths {
        if !index.template_owners.contains_key(&path) {
            index.input_fingerprints.remove(&path);
            index
                .pending_definition_inputs
                .remove(&crate::path_identity_key(Path::new(&path)));
        }
    }
    index.template_parents.retain(|_, v| {
        v.retain(|idx| !indices_set.contains(idx));
        !v.is_empty()
    });

    index.angular_components.retain(|k, _| !indices_set.contains(k));
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

}

/// Remove a file entirely from the definition index
pub fn remove_file_from_def_index(index: &mut DefinitionIndex, path: &Path) {
    let path_key = crate::path_identity_key(path);
    if let Some(&file_id) = index.path_to_id.get(&path_key) {
        remove_file_definitions(index, file_id);
        // Tombstone the files[] slot. file_id is never reused, so the entry
        // stays in the Vec as an empty string — no longer counted as a live
        // file (see DefinitionIndex::live_file_count).
        if (file_id as usize) < index.files.len() {
            index.files[file_id as usize].clear();
        }
        index.path_to_id.remove(&path_key);
        index.input_fingerprints.remove(&definition_input_key(path));
    }
    index.pending_definition_inputs.remove(&path_key);
}

pub(crate) const MAX_TRANSIENT_DEFINITION_ATTEMPTS: u8 = 3;

pub(crate) struct TransientDefinitionResolution {
    pub retry_paths: Vec<PathBuf>,
    pub exhausted_paths: Vec<PathBuf>,
}

pub(crate) fn resolve_transient_definition_inputs(
    index: &mut DefinitionIndex,
    transient_paths: &HashMap<PathBuf, Option<DefinitionInputRevision>>,
    resolved_paths: impl IntoIterator<Item = PathBuf>,
) -> TransientDefinitionResolution {
    for path in resolved_paths {
        index
            .pending_definition_inputs
            .remove(&crate::path_identity_key(&path));
    }

    let mut retry_paths = Vec::new();
    let mut exhausted_paths = Vec::new();
    let mut paths: Vec<_> = transient_paths.iter().collect();
    paths.sort_unstable_by(|left, right| left.0.cmp(right.0));
    for (path, observed_revision) in paths {
        let key = crate::path_identity_key(path);
        let attempts = index
            .pending_definition_inputs
            .get(&key)
            .map_or(0, |pending| pending.attempts)
            .saturating_add(1);
        if attempts >= MAX_TRANSIENT_DEFINITION_ATTEMPTS {
            let was_indexed = index.path_to_id.contains_key(&key);
            remove_file_from_def_index(index, path);
            index.pending_definition_inputs.insert(
                key,
                PendingDefinitionInput {
                    attempts: MAX_TRANSIENT_DEFINITION_ATTEMPTS,
                    observed_revision: *observed_revision,
                },
            );
            if was_indexed {
                exhausted_paths.push(path.clone());
            }
        } else {
            index.pending_definition_inputs.insert(
                key,
                PendingDefinitionInput {
                    attempts,
                    observed_revision: *observed_revision,
                },
            );
            retry_paths.push(path.clone());
        }
    }

    TransientDefinitionResolution {
        retry_paths,
        exhausted_paths,
    }
}

pub fn compact_definitions_if_needed(index: &mut DefinitionIndex) -> bool {
    let active_count: usize = index.file_index.values().map(Vec::len).sum();
    let total_count = index.definitions.len();
    let tombstone_count = total_count.saturating_sub(active_count);
    if total_count == 0 || tombstone_count < active_count {
        return false;
    }

    info!(
        total = total_count,
        active = active_count,
        waste_pct = (tombstone_count * 100) / total_count,
        "Definition index tombstone threshold exceeded, compacting"
    );
    compact_definitions(index);
    true
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
    let started = std::time::Instant::now();
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
    remap_index_values(&mut index.template_owners, &remap);
    remap_index_values(&mut index.template_parents, &remap);

    // Remap HashMap<u32, _> keyed indexes
    index.method_calls = index.method_calls.drain()
        .filter_map(|(k, v)| remap.get(&k).map(|&new_k| (new_k, v)))
        .collect();
    index.code_stats = index.code_stats.drain()
        .filter_map(|(k, v)| remap.get(&k).map(|&new_k| (new_k, v)))
        .collect();
    index.angular_components = index.angular_components.drain()
        .filter_map(|(k, v)| remap.get(&k).map(|&new_k| (new_k, v)))
        .collect();
    index.template_children = index.template_children.drain()
        .filter_map(|(k, v)| remap.get(&k).map(|&new_k| (new_k, v)))
        .collect();

    index.csharp_semantics.remap_definitions(&remap, new_defs.len());

    let after = new_defs.len();
    index.definitions = new_defs;

    let removed = before - after;
    let compact_ms = started.elapsed().as_secs_f64() * 1000.0;
    info!(before, after, removed, compact_ms, "Definition index compacted");

    if crate::index::debug_log_enabled() {
        let mut fields = vec![
            ("compactMs", format!("{:.1}", compact_ms)),
            ("before", before.to_string()),
            ("after", after.to_string()),
            ("removed", removed.to_string()),
            ("wastePct", format!("{:.1}", removed as f64 * 100.0 / before as f64)),
        ];
        let memory = crate::index::get_process_memory_info();
        if let Some(ws) = memory.get("workingSetMB").and_then(|v| v.as_f64()) {
            fields.push(("wsMB", format!("{:.1}", ws)));
        }
        if let Some(peak) = memory.get("peakWorkingSetMB").and_then(|v| v.as_f64()) {
            fields.push(("peakWsMB", format!("{:.1}", peak)));
        }
        if let Some(commit) = memory.get("commitMB").and_then(|v| v.as_f64()) {
            fields.push(("commitMB", format!("{:.1}", commit)));
        }
        crate::index::log_phase("definitionCompact", &fields);
    }
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
    respect_git_exclude: bool,
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
    walker.follow_links(true).hidden(false).git_ignore(true).git_exclude(respect_git_exclude);

    for entry in walker.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if is_inside_git_dir(path) {
            continue;
        }
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
    let disk_file_keys: HashSet<PathBuf> = disk_files.keys()
        .map(|path| crate::path_identity_key(path))
        .collect();

    // Collect indexed paths for deletion check
    let indexed_paths: HashSet<PathBuf> = index.path_to_id.keys().cloned().collect();

    let mut added = 0usize;
    let mut modified = 0usize;
    let mut removed = 0usize;

    // Check for new and modified files
    for (path, mtime) in &disk_files {
        if !index.path_to_id.contains_key(&crate::path_identity_key(path)) {
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
        if !disk_file_keys.contains(path) {
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
#[derive(Default)]
struct ParsedDefinitionFiles {
    parsed_results: Vec<ParsedFileResult>,
    transient_paths: Vec<PathBuf>,
}

fn collect_definition_parse_result(
    batch: &mut ParsedDefinitionFiles,
    path: &Path,
    result: std::io::Result<Option<ParsedFileResult>>,
) {
    match result {
        Ok(Some(parsed)) => batch.parsed_results.push(parsed),
        Ok(None) => {}
        Err(error) if is_transient_definition_input_error(&error) => {
            batch.transient_paths.push(path.to_path_buf());
        }
        Err(error) => {
            tracing::debug!(
                file = %path.display(),
                %error,
                "Definition input could not be parsed during reconciliation"
            );
        }
    }
}

fn parse_definition_files(paths: &[PathBuf]) -> ParsedDefinitionFiles {
    if paths.len() <= 1 {
        let mut batch = ParsedDefinitionFiles::default();
        for (index, path) in paths.iter().enumerate() {
            collect_definition_parse_result(
                &mut batch,
                path,
                try_parse_file_standalone(path, index as u32),
            );
        }
        return batch;
    }

    let num_threads = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4);
    let chunk_size = paths.len().div_ceil(num_threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk_size)
            .enumerate()
            .map(|(chunk_index, chunk)| {
                scope.spawn(move || {
                    #[cfg(feature = "lang-csharp")]
                    let mut cs_parser = {
                        let mut parser = tree_sitter::Parser::new();
                        parser
                            .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
                            .ok();
                        parser
                    };
                    #[cfg(feature = "lang-typescript")]
                    let mut ts_parser: Option<tree_sitter::Parser> = None;
                    #[cfg(feature = "lang-typescript")]
                    let mut tsx_parser: Option<tree_sitter::Parser> = None;
                    #[cfg(feature = "lang-rust")]
                    let mut rs_parser: Option<tree_sitter::Parser> = None;
                    let mut batch = ParsedDefinitionFiles::default();

                    for (index, path) in chunk.iter().enumerate() {
                        let temp_id = (chunk_index * chunk_size + index) as u32;
                        let result = parse_file_with_parsers(
                            path,
                            temp_id,
                            #[cfg(feature = "lang-csharp")]
                            &mut cs_parser,
                            #[cfg(feature = "lang-typescript")]
                            &mut ts_parser,
                            #[cfg(feature = "lang-typescript")]
                            &mut tsx_parser,
                            #[cfg(feature = "lang-rust")]
                            &mut rs_parser,
                        );
                        collect_definition_parse_result(&mut batch, path, result);
                    }
                    batch
                })
            })
            .collect();

        let mut merged = ParsedDefinitionFiles::default();
        for batch in handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
        {
            merged.parsed_results.extend(batch.parsed_results);
            merged.transient_paths.extend(batch.transient_paths);
        }
        merged.transient_paths.sort_unstable();
        merged.transient_paths.dedup();
        merged
    })
}

pub fn reconcile_definition_index_nonblocking(
    def_index: &Arc<RwLock<DefinitionIndex>>,
    dir: &str,
    extensions: &[String],
    respect_git_exclude: bool,
) -> (usize, usize, usize) {
    reconcile_definition_index_nonblocking_with_angular_paths(
        def_index,
        dir,
        extensions,
        respect_git_exclude,
        None,
    )
}

pub(crate) fn reconcile_definition_index_nonblocking_with_angular_paths(
    def_index: &Arc<RwLock<DefinitionIndex>>,
    dir: &str,
    extensions: &[String],
    respect_git_exclude: bool,
    precomputed_angular_paths: Option<&[PathBuf]>,
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
    let mut disk_paths_by_key: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut disk_revisions: HashMap<PathBuf, DefinitionInputRevision> = HashMap::new();

    let mut walker = WalkBuilder::new(&dir_path);
    walker.follow_links(true).hidden(false).git_ignore(true).git_exclude(respect_git_exclude);

    for entry in walker.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if is_inside_git_dir(path) {
            continue;
        }
        let ext_match = path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)));
        if !ext_match {
            continue;
        }
        let metadata = entry.metadata().ok();
        let mtime = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(UNIX_EPOCH);
        let clean = PathBuf::from(clean_path(&path.to_string_lossy()));
        let path_key = crate::path_identity_key(&clean);
        if let Some(metadata) = metadata.as_ref() {
            disk_revisions.insert(
                path_key.clone(),
                super::definition_input_revision_from_metadata(metadata),
            );
        }
        disk_paths_by_key.insert(path_key, clean.clone());
        disk_files.insert(clean, mtime);
    }

    let scanned = disk_files.len();

    // ── Phase 2: Determine changed files (READ lock — instant) ──
    let (
        _threshold,
        to_update,
        to_remove,
        added,
        modified,
        added_keys,
        template_inputs,
    ) = {
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
        let mut added_keys = HashSet::new();

        for (path, mtime) in &disk_files {
            let path_key = crate::path_identity_key(path);
            let current_revision = disk_revisions.get(&path_key).copied();
            let quarantined = idx
                .pending_definition_inputs
                .get(&path_key)
                .is_some_and(|pending| {
                    pending.attempts >= MAX_TRANSIENT_DEFINITION_ATTEMPTS
                        && pending.observed_revision == current_revision
                });
            if !idx.path_to_id.contains_key(&path_key) && !quarantined {
                to_update.push(path.clone());
                added_keys.insert(path_key);
                added += 1;
            } else if idx.path_to_id.contains_key(&path_key) && *mtime > threshold {
                to_update.push(path.clone());
                modified += 1;
            }
        }

        for path in idx.path_to_id.keys() {
            if !disk_paths_by_key.contains_key(path) {
                to_remove.push(path.clone());
            }
        }

        let mut update_keys: HashSet<PathBuf> = to_update
            .iter()
            .map(|path| crate::path_identity_key(path))
            .collect();
        for (pending_key, pending) in &idx.pending_definition_inputs {
            if pending.attempts < MAX_TRANSIENT_DEFINITION_ATTEMPTS
                && update_keys.insert(pending_key.clone())
                && let Some(path) = disk_paths_by_key.get(pending_key)
            {
                to_update.push(path.clone());
                modified += 1;
            }
        }

        let template_inputs = if precomputed_angular_paths.is_none() {
            idx.template_owners
                .iter()
                .map(|(owner_key, owners)| {
                    let unavailable_reason = owners.iter().find_map(|definition_index| {
                        match idx.angular_components.get(definition_index) {
                            Some(AngularComponentRecord {
                                template: AngularTemplateSource::UnavailableExternal {
                                    reason,
                                    ..
                                },
                                ..
                            }) => Some(reason.clone()),
                            _ => None,
                        }
                    });
                    (
                        PathBuf::from(owner_key),
                        idx.input_fingerprints.get(owner_key).cloned(),
                        unavailable_reason,
                        idx.pending_definition_inputs
                            .contains_key(&crate::path_identity_key(Path::new(owner_key))),
                    )
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        (
            threshold,
            to_update,
            to_remove,
            added,
            modified,
            added_keys,
            template_inputs,
        )
    };
    // READ lock released here

    let angular_to_update: Vec<PathBuf> = match precomputed_angular_paths {
        Some(paths) => paths.to_vec(),
        None => template_inputs
            .into_iter()
            .filter_map(|(path, fingerprint, unavailable_reason, pending)| {
                (pending
                    || !super::angular_template_snapshot_matches(
                        &path,
                        fingerprint.as_ref(),
                        unavailable_reason.as_deref(),
                    ))
                .then_some(path)
            })
            .collect(),
    };
    let modified = modified + angular_to_update.len();
    let removed = to_remove.len();

    if to_update.is_empty() && to_remove.is_empty() && angular_to_update.is_empty() {
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        info!(
            scanned,
            elapsed_ms = format_args!("{:.1}", elapsed_ms),
            "Definition index reconciliation (non-blocking): all files up to date"
        );
        return (0, 0, 0);
    }

    // ── Phase 3: Parse ALL files in parallel (NO lock needed) ──
    // During this phase, MCP requests work normally on the old index data.
    let mut parsed_batch = parse_definition_files(&to_update);
    if !parsed_batch.transient_paths.is_empty() {
        let retry_batch = parse_definition_files(&parsed_batch.transient_paths);
        parsed_batch
            .parsed_results
            .extend(retry_batch.parsed_results);
        parsed_batch.transient_paths = retry_batch.transient_paths;
    }
    let transient_paths: HashMap<PathBuf, Option<DefinitionInputRevision>> = parsed_batch
        .transient_paths
        .iter()
        .map(|path| (path.clone(), super::definition_input_revision(path)))
        .collect();
    let mut stable_updates: Vec<PathBuf> = to_update
        .iter()
        .filter(|path| !transient_paths.contains_key(*path))
        .cloned()
        .collect();
    let mut stable_removals = to_remove.clone();
    let mut stable_angular_updates = angular_to_update.clone();
    let mut deferred_updates = HashSet::new();
    let mut deferred_removals = HashSet::new();
    let mut deferred_angular_updates = HashSet::new();
    let mut changed_paths = stable_removals.clone();
    changed_paths.extend(stable_updates.iter().cloned());
    changed_paths.extend(stable_angular_updates.iter().cloned());
    let mut parsed_results = parsed_batch.parsed_results;
    let (workspace_root, owner_keys) = {
        let index = match def_index.read() {
            Ok(index) => index,
            Err(error) => {
                tracing::error!(
                    %error,
                    "Failed to acquire def index read lock for Angular reconciliation"
                );
                return (0, 0, 0);
            }
        };
        let owner_keys = changed_paths
            .iter()
            .map(|path| definition_input_key(path))
            .filter(|key| index.template_owners.contains_key(key))
            .collect();
        (index.root.clone(), owner_keys)
    };
    let mut angular_updates = super::prepare_angular_template_updates(
        &workspace_root,
        &mut parsed_results,
        &changed_paths,
        &owner_keys,
    );
    let unstable = super::validate_prepared_definition_inputs(
        &parsed_results,
        &angular_updates,
    );
    if !unstable.is_empty() {
        let rejected_keys = unstable
            .iter()
            .map(|path| definition_input_key(path))
            .collect();
        let rejected_keys = super::retain_stable_prepared_definition_inputs(
            &mut parsed_results,
            &mut angular_updates,
            rejected_keys,
        );
        super::defer_rejected_definition_paths(
            &mut stable_updates,
            &rejected_keys,
            &mut deferred_updates,
        );
        super::defer_rejected_definition_paths(
            &mut stable_removals,
            &rejected_keys,
            &mut deferred_removals,
        );
        super::defer_rejected_definition_paths(
            &mut stable_angular_updates,
            &rejected_keys,
            &mut deferred_angular_updates,
        );
        changed_paths.clear();
        changed_paths.extend(stable_removals.iter().cloned());
        changed_paths.extend(stable_updates.iter().cloned());
        changed_paths.extend(stable_angular_updates.iter().cloned());
        warn!(
            unstable = unstable.len(),
            deferred = deferred_updates.len()
                + deferred_removals.len()
                + deferred_angular_updates.len(),
            "Definition reconcile snapshot changed before validation; applying stable paths"
        );
    }

    let mut input_keys: HashSet<String> = changed_paths
        .iter()
        .map(|path| definition_input_key(path))
        .collect();
    input_keys.extend(
        parsed_results
            .iter()
            .map(|result| definition_input_key(&result.path)),
    );
    input_keys.extend(
        angular_updates
            .iter()
            .map(|update| update.owner_key.clone()),
    );
    let expected_fingerprints = {
        let index = match def_index.read() {
            Ok(index) => index,
            Err(error) => {
                tracing::error!(
                    %error,
                    "Failed to acquire def index read lock for fingerprint reconciliation"
                );
                return (0, 0, 0);
            }
        };
        input_keys
            .into_iter()
            .map(|key| {
                let fingerprint = index.input_fingerprints.get(&key).cloned();
                (key, fingerprint)
            })
            .collect::<HashMap<_, _>>()
    };
    // ── Phase 4: Apply results (WRITE lock — brief, <500ms) ──
    let (applied_added, applied_modified, applied_removed) = {
        let mut idx = match def_index.write() {
            Ok(idx) => idx,
            Err(e) => {
                tracing::error!(error = %e, "Failed to acquire def index write lock for reconciliation");
                return (0, 0, 0);
            }
        };

        let conflicts = super::definition_fingerprint_conflicts(&idx, &expected_fingerprints);
        if !conflicts.is_empty() {
            let rejected_keys = super::retain_stable_prepared_definition_inputs(
                &mut parsed_results,
                &mut angular_updates,
                conflicts,
            );
            super::defer_rejected_definition_paths(
                &mut stable_updates,
                &rejected_keys,
                &mut deferred_updates,
            );
            super::defer_rejected_definition_paths(
                &mut stable_removals,
                &rejected_keys,
                &mut deferred_removals,
            );
            super::defer_rejected_definition_paths(
                &mut stable_angular_updates,
                &rejected_keys,
                &mut deferred_angular_updates,
            );
            changed_paths.clear();
            changed_paths.extend(stable_removals.iter().cloned());
            changed_paths.extend(stable_updates.iter().cloned());
            changed_paths.extend(stable_angular_updates.iter().cloned());
            warn!(
                sources = deferred_updates.len(),
                templates = deferred_angular_updates.len(),
                removed = deferred_removals.len(),
                "Definition reconcile baseline changed before apply; applying stable paths"
            );
        }

        let applied_paths: HashSet<PathBuf> = parsed_results
            .iter()
            .map(|result| result.path.clone())
            .collect();
        let stable_update_set: HashSet<PathBuf> = stable_updates.iter().cloned().collect();
        let stale_cleanup_paths: Vec<PathBuf> = stable_update_set
            .iter()
            .filter(|path| !applied_paths.contains(*path))
            .cloned()
            .collect();

        let resolved_paths = changed_paths
            .iter()
            .chain(angular_updates.iter().map(|update| &update.path))
            .cloned()
            .collect::<Vec<_>>();
        let transient_resolution = resolve_transient_definition_inputs(
            &mut idx,
            &transient_paths,
            resolved_paths,
        );
        super::mark_pending_definition_conflicts(&mut idx, &deferred_updates);
        super::mark_pending_definition_conflicts(&mut idx, &deferred_angular_updates);
        let applied_added = stable_updates
            .iter()
            .filter(|path| added_keys.contains(&crate::path_identity_key(path)))
            .count();
        let applied_modified = stable_updates.len().saturating_sub(applied_added)
            + stable_angular_updates.len();
        let applied_removed = stable_removals.len()
            + transient_resolution.exhausted_paths.len();
        let graph_changed = !applied_paths.is_empty()
            || !stale_cleanup_paths.is_empty()
            || !stable_removals.is_empty()
            || !angular_updates.is_empty()
            || !transient_resolution.exhausted_paths.is_empty();


        // Remove deleted files: drop secondary indexes, tombstone the
        // files[] slot. We never reuse file_id, so the slot stays in the
        // Vec as an empty string — it's no longer counted as a live file
        // (see DefinitionIndex::live_file_count) but file_id assignments
        // remain stable.
        for path in &stable_removals {
            remove_file_from_def_index(&mut idx, path);
        }

        // Apply parsed results
        for result in parsed_results {
            apply_parsed_result(&mut idx, result);
        }

        // Clean up files that were in to_update but didn't produce a ParsedFileResult
        // (e.g., read error). Without this, stale definitions remain for unreadable files.
        for path in &stale_cleanup_paths {
            if let Some(&file_id) = idx.path_to_id.get(&crate::path_identity_key(path)) {
                remove_file_definitions(&mut idx, file_id);
            }
            idx.input_fingerprints.remove(&definition_input_key(path));
            idx.pending_definition_inputs
                .remove(&crate::path_identity_key(path));
        }
        super::apply_prepared_angular_template_updates(&mut idx, angular_updates);
        compact_definitions_if_needed(&mut idx);

        // Update created_at if anything changed (use walk_start, not now(), to avoid race condition)
        if added > 0 || modified > 0 || removed > 0 {
            idx.created_at = walk_start;
        }
        if graph_changed {
            idx.definition_generation = idx.definition_generation.saturating_add(1);
        }
        (applied_added, applied_modified, applied_removed)
    };
    // WRITE lock released here

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    info!(
        scanned,
        added = applied_added,
        modified = applied_modified,
        removed = applied_removed,
        elapsed_ms = format_args!("{:.1}", elapsed_ms),
        "Definition index reconciliation complete (non-blocking)"
    );

    crate::index::log_memory(&format!(
        "watcher: def reconciliation non-blocking (scanned={}, added={}, modified={}, removed={}, {:.0}ms)",
        scanned, applied_added, applied_modified, applied_removed, elapsed_ms
    ));

    (applied_added, applied_modified, applied_removed)
}