//! info and info_json commands.

use std::fs;

use crate::{current_unix_secs, index_dir, index::load_compressed, index::load_content_index_at_path, definitions::load_definition_index_at_path, FileIndex};
#[allow(unused_imports)] // Used in tests (info_tests.rs) and in cmd_info_json
use crate::index::{load_index_meta, save_index_meta, IndexDetails, IndexMeta};

/// INFO-001: shared age computation that respects the clock-failure semantics
/// of [`current_unix_secs`]. Returns the number of seconds elapsed since
/// `created_at`, or `u64::MAX` if the system clock is invalid (treats the
/// cache as "ancient" so downstream stale checks bias toward force-rebuild
/// instead of the previous "every cache reports as 0.0 h ago" silent bug).
fn age_secs_since(created_at: u64) -> u64 {
    current_unix_secs()
        .map(|now| now.saturating_sub(created_at))
        .unwrap_or(u64::MAX)
}

/// Check if an index is stale based on created_at and max_age_secs from metadata.
fn is_stale_from_meta(created_at: u64, max_age_secs: u64) -> bool {
    if max_age_secs == 0 {
        return false;
    }
    // INFO-001: when the clock is invalid we cannot prove freshness, so
    // bias toward stale=true. Operators reading `xray info` then see a
    // [STALE] marker and rebuild instead of trusting an unverifiable cache.
    match current_unix_secs() {
        Some(now) => now.saturating_sub(created_at) > max_age_secs,
        None => true,
    }
}

/// Compute age in hours from created_at timestamp. Returns `f64::INFINITY`
/// on clock failure so callers' `{:.1}h ago` format strings display a
/// visibly-broken `infh ago` instead of the previous misleading `0.0h ago`.
fn age_hours(created_at: u64) -> f64 {
    let secs = age_secs_since(created_at);
    if secs == u64::MAX {
        return f64::INFINITY;
    }
    secs as f64 / 3600.0
}

pub fn cmd_info() {
    let dir = index_dir();
    if !dir.exists() {
        eprintln!("No indexes found. Use 'xray index -d <dir>' to create one.");
        return;
    }

    eprintln!("Index directory: {}", dir.display());
    eprintln!();

    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to read index directory: {}", e);
            return;
        }
    };

    let mut found = false;
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());

        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("?");

        // Skip .meta sidecar files
        if filename.ends_with(".meta") {
            continue;
        }

        if ext == Some("file-list") {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            // Try .meta first
            if let Some(meta) = load_index_meta(&path) {
                found = true;
                let stale = is_stale_from_meta(meta.created_at, meta.max_age_secs);
                let stale_str = if stale { " [STALE]" } else { "" };
                let entries = match &meta.details {
                    crate::index::IndexDetails::FileList { entries } => *entries,
                    _ => 0,
                };
                println!(
                    "  [FILE] {} -- {} entries, {:.1} MB, {:.1}h ago{} ({})",
                    meta.root, entries,
                    size as f64 / 1_048_576.0, age_hours(meta.created_at), stale_str, filename
                );
            } else {
                // Fallback: full deserialization + auto-create .meta for next time
                match load_compressed::<FileIndex>(&path, "file-index") {
                    Ok(index) => {
                        found = true;
                        let stale = if index.is_stale() { " [STALE]" } else { "" };
                        println!(
                            "  [FILE] {} -- {} entries, {:.1} MB, {:.1}h ago{} ({})",
                            index.root, index.entries.len(),
                            size as f64 / 1_048_576.0, age_hours(index.created_at), stale, filename
                        );
                        // Auto-create .meta so next call is instant
                        save_index_meta(&path, &crate::index::file_index_meta(&index));
                    }
                    Err(e) => {
                        eprintln!("  Warning: failed to load {}: {}", path.display(), e);
                    }
                }
            }
        } else if ext == Some("word-search") {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if let Some(meta) = load_index_meta(&path) {
                found = true;
                let stale = is_stale_from_meta(meta.created_at, meta.max_age_secs);
                let stale_str = if stale { " [STALE]" } else { "" };
                let total_tokens = match &meta.details {
                    crate::index::IndexDetails::Content { total_tokens, .. } => *total_tokens,
                    _ => 0,
                };
                println!(
                    "  [CONTENT] {} -- {} files, {} tokens, exts: [{}], {:.1} MB, {:.1}h ago{} ({})",
                    meta.root, meta.files, total_tokens,
                    meta.extensions.join(", "),
                    size as f64 / 1_048_576.0, age_hours(meta.created_at), stale_str, filename
                );
            } else {
                match load_content_index_at_path(&path) {
                    Ok(index) => {
                        found = true;
                        let stale = if index.is_stale() { " [STALE]" } else { "" };
                        println!(
                            "  [CONTENT] {} -- {} files, {} tokens, exts: [{}], {:.1} MB, {:.1}h ago{} ({})",
                            index.root, index.files.len(), index.total_tokens,
                            index.extensions.join(", "),
                            size as f64 / 1_048_576.0, age_hours(index.created_at), stale, filename
                        );
                        save_index_meta(&path, &crate::index::content_index_meta(&index));
                    }
                    Err(e) => {
                        eprintln!("  Warning: failed to load {}: {}", path.display(), e);
                    }
                }
            }
        } else if ext == Some("code-structure") {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if let Some(meta) = load_index_meta(&path) {
                found = true;
                let (defs, calls) = match &meta.details {
                    crate::index::IndexDetails::Definition { definitions, call_sites, .. } => (*definitions, *call_sites),
                    _ => (0, 0),
                };
                println!(
                    "  [DEF] {} -- {} files, {} defs, {} call sites, exts: [{}], {:.1} MB, {:.1}h ago ({})",
                    meta.root, meta.files, defs, calls,
                    meta.extensions.join(", "),
                    size as f64 / 1_048_576.0, age_hours(meta.created_at), filename
                );
            } else {
                match load_definition_index_at_path(&path) {
                    Ok(index) => {
                        found = true;
                        let call_sites: usize = index.method_calls.values().map(|v| v.len()).sum();
                        let active_defs: usize = index.file_index.values().map(|v| v.len()).sum();
                        println!(
                            "  [DEF] {} -- {} files, {} defs, {} call sites, exts: [{}], {:.1} MB, {:.1}h ago ({})",
                            index.root, index.files.len(), active_defs,
                            call_sites,
                            index.extensions.join(", "),
                            size as f64 / 1_048_576.0, age_hours(index.created_at), filename
                        );
                        save_index_meta(&path, &crate::index::definition_index_meta(&index));
                    }
                    Err(e) => {
                        eprintln!("  Warning: failed to load {}: {}", path.display(), e);
                    }
                }
            }
        } else if ext == Some("git-history") {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if let Some(meta) = load_index_meta(&path) {
                found = true;
                let (branch, commits, authors, head_hash) = match &meta.details {
                    crate::index::IndexDetails::GitHistory { branch, commits, authors, head_hash } => {
                        (branch.as_str(), *commits, *authors, head_hash.as_str())
                    }
                    _ => ("?", 0, 0, "?"),
                };
                println!(
                    "  [GIT] branch={}, {} commits, {} files, {} authors, HEAD={}, {:.1} MB, {:.1}h ago ({})",
                    branch,
                    commits,
                    meta.files,
                    authors,
                    &head_hash[..head_hash.len().min(8)],
                    size as f64 / 1_048_576.0,
                    age_hours(meta.created_at),
                    filename
                );
            } else if let Ok(cache) = crate::git::cache::GitHistoryCache::load_from_disk(&path) {
                found = true;
                println!(
                    "  [GIT] branch={}, {} commits, {} files, {} authors, HEAD={}, {:.1} MB, {:.1}h ago ({})",
                    cache.branch,
                    cache.commits.len(),
                    cache.file_commits.len(),
                    cache.authors.len(),
                    &cache.head_hash[..cache.head_hash.len().min(8)],
                    size as f64 / 1_048_576.0,
                    age_hours(cache.built_at),
                    filename
                );
                save_index_meta(&path, &crate::index::git_cache_meta(&cache));
            }
        }
    }

    if !found {
        eprintln!("No indexes found.");
    }
}

/// Convert an IndexMeta to a JSON value for cmd_info_json output.
#[cfg(test)]
fn meta_to_json(meta: &IndexMeta, size: u64, filename: &str) -> serde_json::Value {
    let size_mb = (size as f64 / 1_048_576.0 * 10.0).round() / 10.0;
    let age_h = (age_hours(meta.created_at) * 10.0).round() / 10.0;

    match &meta.details {
        IndexDetails::FileList { entries } => {
            serde_json::json!({
                "type": "file",
                "root": meta.root,
                "entries": entries,
                "sizeMb": size_mb,
                "ageHours": age_h,
                "stale": is_stale_from_meta(meta.created_at, meta.max_age_secs),
                "filename": filename,
            })
        }
        IndexDetails::Content { total_tokens, parse_errors, lossy_file_count, .. } => {
            let mut info = serde_json::json!({
                "type": "content",
                "root": meta.root,
                "files": meta.files,
                "totalTokens": total_tokens,
                "extensions": meta.extensions,
                "sizeMb": size_mb,
                "ageHours": age_h,
                "stale": is_stale_from_meta(meta.created_at, meta.max_age_secs),
                "filename": filename,
            });
            if let Some(pe) = parse_errors
                && *pe > 0 {
                    info["readErrors"] = serde_json::json!(pe);
                }
            if let Some(lf) = lossy_file_count
                && *lf > 0 {
                    info["lossyUtf8Files"] = serde_json::json!(lf);
                }
            info
        }
        IndexDetails::Definition { definitions, call_sites, parse_errors, lossy_file_count } => {
            let mut def_info = serde_json::json!({
                "type": "definition",
                "root": meta.root,
                "files": meta.files,
                "definitions": definitions,
                "callSites": call_sites,
                "extensions": meta.extensions,
                "sizeMb": size_mb,
                "ageHours": age_h,
                "filename": filename,
            });
            if let Some(pe) = parse_errors
                && *pe > 0 {
                    def_info["readErrors"] = serde_json::json!(pe);
                }
            if let Some(lf) = lossy_file_count
                && *lf > 0 {
                    def_info["lossyUtf8Files"] = serde_json::json!(lf);
                }
            def_info
        }
        IndexDetails::GitHistory { commits, authors, branch, head_hash } => {
            serde_json::json!({
                "type": "git-history",
                "commits": commits,
                "files": meta.files,
                "authors": authors,
                "headHash": head_hash,
                "branch": branch,
                "sizeMb": size_mb,
                "ageHours": age_h,
                "filename": filename,
            })
        }
    }
}

/// Return index info as JSON value for a specific directory.
/// Reads .meta sidecar files when available (zero-allocation),
/// falls back to full deserialization for old indexes without .meta.
#[cfg(test)]
pub(crate) fn cmd_info_json_for_dir(dir: &std::path::Path) -> serde_json::Value {
    if !dir.exists() {
        return serde_json::json!({ "indexes": [], "directory": dir.display().to_string() });
    }

    let mut indexes = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());

            let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("?").to_string();

            // Skip .meta sidecar files
            if filename.ends_with(".meta") {
                continue;
            }

            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

            if ext == Some("file-list") {
                if let Some(meta) = load_index_meta(&path) {
                    indexes.push(meta_to_json(&meta, size, &filename));
                } else if let Ok(index) = load_compressed::<FileIndex>(&path, "file-index") {
                    let age_secs = age_secs_since(index.created_at);
                    indexes.push(serde_json::json!({
                        "type": "file",
                        "root": index.root,
                        "entries": index.entries.len(),
                        "sizeMb": (size as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                        "ageHours": (age_secs as f64 / 3600.0 * 10.0).round() / 10.0,
                        "stale": index.is_stale(),
                        "filename": filename,
                    }));
                    // Auto-create .meta so next call is instant
                    save_index_meta(&path, &crate::index::file_index_meta(&index));
                }
            } else if ext == Some("word-search") {
                if let Some(meta) = load_index_meta(&path) {
                    indexes.push(meta_to_json(&meta, size, &filename));
                } else if let Ok(index) = load_content_index_at_path(&path) {
                    let age_secs = age_secs_since(index.created_at);
                    indexes.push(serde_json::json!({
                        "type": "content",
                        "root": index.root,
                        "files": index.files.len(),
                        "totalTokens": index.total_tokens,
                        "extensions": index.extensions,
                        "sizeMb": (size as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                        "ageHours": (age_secs as f64 / 3600.0 * 10.0).round() / 10.0,
                        "stale": index.is_stale(),
                        "filename": filename,
                    }));
                    save_index_meta(&path, &crate::index::content_index_meta(&index));
                }
            } else if ext == Some("code-structure") {
                if let Some(meta) = load_index_meta(&path) {
                    indexes.push(meta_to_json(&meta, size, &filename));
                } else if let Ok(index) = load_definition_index_at_path(&path) {
                    let age_secs = age_secs_since(index.created_at);
                    let call_sites: usize = index.method_calls.values().map(|v| v.len()).sum();
                    let active_defs: usize = index.file_index.values().map(|v| v.len()).sum();
                    let mut def_info = serde_json::json!({
                        "type": "definition",
                        "root": index.root,
                        "files": index.files.len(),
                        "definitions": active_defs,
                        "callSites": call_sites,
                        "extensions": index.extensions,
                        "sizeMb": (size as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                        "ageHours": (age_secs as f64 / 3600.0 * 10.0).round() / 10.0,
                    });
                    if index.parse_errors > 0 {
                        def_info["readErrors"] = serde_json::json!(index.parse_errors);
                    }
                    if index.lossy_file_count > 0 {
                        def_info["lossyUtf8Files"] = serde_json::json!(index.lossy_file_count);
                    }
                    def_info["filename"] = serde_json::json!(filename);
                    indexes.push(def_info);
                    save_index_meta(&path, &crate::index::definition_index_meta(&index));
                }
            } else if ext == Some("git-history") {
                if let Some(meta) = load_index_meta(&path) {
                    indexes.push(meta_to_json(&meta, size, &filename));
                } else if let Ok(cache) = crate::git::cache::GitHistoryCache::load_from_disk(&path) {
                    let age_secs = age_secs_since(cache.built_at);
                    indexes.push(serde_json::json!({
                        "type": "git-history",
                        "commits": cache.commits.len(),
                        "files": cache.file_commits.len(),
                        "authors": cache.authors.len(),
                        "headHash": cache.head_hash,
                        "branch": cache.branch,
                        "sizeMb": (size as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                        "ageHours": (age_secs as f64 / 3600.0 * 10.0).round() / 10.0,
                        "filename": filename,
                    }));
                    save_index_meta(&path, &crate::index::git_cache_meta(&cache));
                }
            }
        }
    }

    serde_json::json!({
        "directory": dir.display().to_string(),
        "indexes": indexes,
    })
}

#[cfg(test)]
#[path = "info_tests.rs"]
mod tests;
