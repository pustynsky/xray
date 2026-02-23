//! Incremental updates for DefinitionIndex (used by file watcher).

use std::path::Path;

use tracing::warn;

use crate::read_file_lossy;
use super::types::*;
use super::parser_csharp::parse_csharp_definitions;
use super::parser_typescript::parse_typescript_definitions;
use super::parser_sql::parse_sql_definitions;

/// Update definitions for a single file (incremental).
/// Removes old definitions for the file, parses it again, adds new ones.
pub fn update_file_definitions(index: &mut DefinitionIndex, path: &Path) {
    let path_str = path.to_string_lossy().to_string();

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
        "cs" => {
            let mut cs_parser = tree_sitter::Parser::new();
            cs_parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).ok();
            {
                let (defs, calls, stats, _ext) = parse_csharp_definitions(&mut cs_parser, &content, file_id);
                (defs, calls, stats)
            }
        }
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
        "sql" => {
            parse_sql_definitions(&content, file_id)
        }
        _ => (Vec::new(), Vec::new(), Vec::new()),
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

    // Check for excessive tombstone growth
    let active_count: usize = index.file_index.values().map(|v| v.len()).sum();
    let total_count = index.definitions.len();
    if total_count > 0 && total_count > active_count * 2 {
        warn!(
            total = total_count,
            active = active_count,
            waste_pct = ((total_count - active_count) * 100) / total_count,
            "Definition index has significant tombstone growth, consider restart to compact"
        );
    }
}

/// Remove a file entirely from the definition index
pub fn remove_file_from_def_index(index: &mut DefinitionIndex, path: &Path) {
    if let Some(&file_id) = index.path_to_id.get(path) {
        remove_file_definitions(index, file_id);
        index.path_to_id.remove(path);
    }
}