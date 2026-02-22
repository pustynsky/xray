//! info and info_json commands.

use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{index_dir, index::load_compressed, ContentIndex, FileIndex};
use crate::index::{load_index_meta, save_index_meta, IndexMeta};

/// Check if an index is stale based on created_at and max_age_secs from metadata.
fn is_stale_from_meta(created_at: u64, max_age_secs: u64) -> bool {
    if max_age_secs == 0 {
        return false;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    now.saturating_sub(created_at) > max_age_secs
}

/// Compute age in hours from created_at timestamp.
fn age_hours(created_at: u64) -> f64 {
    let age_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
        .saturating_sub(created_at);
    age_secs as f64 / 3600.0
}

pub fn cmd_info() {
    let dir = index_dir();
    if !dir.exists() {
        eprintln!("No indexes found. Use 'search index -d <dir>' to create one.");
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
                println!(
                    "  [FILE] {} -- {} entries, {:.1} MB, {:.1}h ago{} ({})",
                    meta.root, meta.entries.unwrap_or(0),
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
                println!(
                    "  [CONTENT] {} -- {} files, {} tokens, exts: [{}], {:.1} MB, {:.1}h ago{} ({})",
                    meta.root, meta.files, meta.total_tokens.unwrap_or(0),
                    meta.extensions.join(", "),
                    size as f64 / 1_048_576.0, age_hours(meta.created_at), stale_str, filename
                );
            } else {
                match load_compressed::<ContentIndex>(&path, "content-index") {
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
                println!(
                    "  [DEF] {} -- {} files, {} defs, {} call sites, exts: [{}], {:.1} MB, {:.1}h ago ({})",
                    meta.root, meta.files, meta.definitions.unwrap_or(0),
                    meta.call_sites.unwrap_or(0),
                    meta.extensions.join(", "),
                    size as f64 / 1_048_576.0, age_hours(meta.created_at), filename
                );
            } else {
                match load_compressed::<crate::definitions::DefinitionIndex>(&path, "definition-index") {
                    Ok(index) => {
                        found = true;
                        let call_sites: usize = index.method_calls.values().map(|v| v.len()).sum();
                        println!(
                            "  [DEF] {} -- {} files, {} defs, {} call sites, exts: [{}], {:.1} MB, {:.1}h ago ({})",
                            index.root, index.files.len(), index.definitions.len(),
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
                println!(
                    "  [GIT] branch={}, {} commits, {} files, {} authors, HEAD={}, {:.1} MB, {:.1}h ago ({})",
                    meta.branch.as_deref().unwrap_or("?"),
                    meta.commits.unwrap_or(0),
                    meta.files,
                    meta.authors.unwrap_or(0),
                    &meta.head_hash.as_deref().unwrap_or("?")[..meta.head_hash.as_deref().unwrap_or("?").len().min(8)],
                    size as f64 / 1_048_576.0,
                    age_hours(meta.created_at),
                    filename
                );
            } else {
                if let Ok(cache) = crate::git::cache::GitHistoryCache::load_from_disk(&path) {
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
    }

    if !found {
        eprintln!("No indexes found.");
    }
}

/// Convert an IndexMeta to a JSON value for cmd_info_json output.
fn meta_to_json(meta: &IndexMeta, size: u64, filename: &str) -> serde_json::Value {
    let size_mb = (size as f64 / 1_048_576.0 * 10.0).round() / 10.0;
    let age_h = (age_hours(meta.created_at) * 10.0).round() / 10.0;

    match meta.index_type.as_str() {
        "file-list" => {
            serde_json::json!({
                "type": "file",
                "root": meta.root,
                "entries": meta.entries.unwrap_or(0),
                "sizeMb": size_mb,
                "ageHours": age_h,
                "stale": is_stale_from_meta(meta.created_at, meta.max_age_secs),
                "filename": filename,
            })
        }
        "content" => {
            serde_json::json!({
                "type": "content",
                "root": meta.root,
                "files": meta.files,
                "totalTokens": meta.total_tokens.unwrap_or(0),
                "extensions": meta.extensions,
                "sizeMb": size_mb,
                "ageHours": age_h,
                "stale": is_stale_from_meta(meta.created_at, meta.max_age_secs),
                "filename": filename,
            })
        }
        "definition" => {
            let mut def_info = serde_json::json!({
                "type": "definition",
                "root": meta.root,
                "files": meta.files,
                "definitions": meta.definitions.unwrap_or(0),
                "callSites": meta.call_sites.unwrap_or(0),
                "extensions": meta.extensions,
                "sizeMb": size_mb,
                "ageHours": age_h,
                "filename": filename,
            });
            if let Some(pe) = meta.parse_errors {
                if pe > 0 {
                    def_info["readErrors"] = serde_json::json!(pe);
                }
            }
            if let Some(lf) = meta.lossy_file_count {
                if lf > 0 {
                    def_info["lossyUtf8Files"] = serde_json::json!(lf);
                }
            }
            def_info
        }
        "git-history" => {
            serde_json::json!({
                "type": "git-history",
                "commits": meta.commits.unwrap_or(0),
                "files": meta.files,
                "authors": meta.authors.unwrap_or(0),
                "headHash": meta.head_hash.as_deref().unwrap_or(""),
                "branch": meta.branch.as_deref().unwrap_or(""),
                "sizeMb": size_mb,
                "ageHours": age_h,
                "filename": filename,
            })
        }
        _ => serde_json::json!({
            "type": meta.index_type,
            "root": meta.root,
            "sizeMb": size_mb,
            "ageHours": age_h,
            "filename": filename,
        }),
    }
}

/// Return index info as JSON value (for MCP handler and CLI)
pub fn cmd_info_json() -> serde_json::Value {
    cmd_info_json_for_dir(&index_dir())
}

/// Return index info as JSON value for a specific directory.
/// Reads .meta sidecar files when available (zero-allocation),
/// falls back to full deserialization for old indexes without .meta.
#[cfg_attr(not(test), allow(dead_code))]
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
                    let age_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or(Duration::ZERO)
                        .as_secs()
                        .saturating_sub(index.created_at);
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
                } else if let Ok(index) = load_compressed::<ContentIndex>(&path, "content-index") {
                    let age_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or(Duration::ZERO)
                        .as_secs()
                        .saturating_sub(index.created_at);
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
                } else if let Ok(index) = load_compressed::<crate::definitions::DefinitionIndex>(&path, "definition-index") {
                    let age_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or(Duration::ZERO)
                        .as_secs()
                        .saturating_sub(index.created_at);
                    let call_sites: usize = index.method_calls.values().map(|v| v.len()).sum();
                    let mut def_info = serde_json::json!({
                        "type": "definition",
                        "root": index.root,
                        "files": index.files.len(),
                        "definitions": index.definitions.len(),
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
                    let age_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or(Duration::ZERO)
                        .as_secs()
                        .saturating_sub(cache.built_at);
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
mod tests {
    use super::*;

    /// Helper: build a minimal GitHistoryCache with test data and save it to a temp directory.
    fn create_test_git_history_cache(dir: &std::path::Path) -> std::path::PathBuf {
        use crate::git::cache::{GitHistoryCacheBuilder, parse_git_log_stream};

        // Use the public streaming parser to build a cache
        let git_log = "\
COMMIT:aabbccddee00112233445566778899aabbccddee␞1700000000␞alice@example.com␞Alice␞Initial commit
src/main.rs

COMMIT:112233445566778899aabbccddeeff0011223344␞1700001000␞bob@example.com␞Bob␞Add feature
src/main.rs
src/lib.rs
";
        let mut builder = GitHistoryCacheBuilder::new();
        let reader = std::io::BufReader::new(git_log.as_bytes());
        parse_git_log_stream(reader, &mut builder).unwrap();

        let cache = builder.build(
            "aabbccddee00112233445566778899aabbccddee".to_string(),
            "main".to_string(),
        );

        let cache_path = dir.join("test_12345678.git-history");
        cache.save_to_disk(&cache_path).unwrap();
        cache_path
    }

    #[test]
    fn test_info_json_includes_git_history() {
        let tmp = std::env::temp_dir().join(format!("search_info_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let _cache_path = create_test_git_history_cache(&tmp);

        let result = cmd_info_json_for_dir(&tmp);

        let indexes = result["indexes"].as_array().expect("indexes should be an array");
        let git_entries: Vec<_> = indexes.iter()
            .filter(|idx| idx["type"] == "git-history")
            .collect();

        assert_eq!(git_entries.len(), 1, "Expected exactly 1 git-history entry");

        let entry = &git_entries[0];
        assert_eq!(entry["type"], "git-history");
        assert_eq!(entry["commits"], 2);
        assert_eq!(entry["files"], 2); // src/main.rs and src/lib.rs
        assert_eq!(entry["authors"], 2); // Alice and Bob
        assert_eq!(entry["branch"], "main");
        assert_eq!(entry["headHash"], "aabbccddee00112233445566778899aabbccddee");
        assert!(entry["sizeMb"].as_f64().unwrap() >= 0.0);
        assert!(entry["ageHours"].as_f64().unwrap() >= 0.0);
        assert!(entry["filename"].as_str().unwrap().ends_with(".git-history"));

        // Cleanup
        std::fs::remove_dir_all(&tmp).unwrap_or_default();
    }

    #[test]
    fn test_info_json_empty_dir_no_git_history() {
        let tmp = std::env::temp_dir().join(format!("search_info_empty_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let result = cmd_info_json_for_dir(&tmp);
        let indexes = result["indexes"].as_array().expect("indexes should be an array");
        let git_entries: Vec<_> = indexes.iter()
            .filter(|idx| idx["type"] == "git-history")
            .collect();
        assert_eq!(git_entries.len(), 0, "Empty dir should have no git-history entries");

        // Cleanup
        std::fs::remove_dir_all(&tmp).unwrap_or_default();
    }

    #[test]
    fn test_info_json_nonexistent_dir() {
        let nonexistent = std::path::Path::new("/nonexistent_search_info_test_dir_12345");
        let result = cmd_info_json_for_dir(nonexistent);
        let indexes = result["indexes"].as_array().expect("indexes should be an array");
        assert!(indexes.is_empty());
    }

    #[test]
    fn test_info_json_git_history_corrupt_file_skipped() {
        let tmp = std::env::temp_dir().join(format!("search_info_corrupt_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // Write a corrupt .git-history file
        let corrupt_path = tmp.join("corrupt_12345678.git-history");
        std::fs::write(&corrupt_path, b"THIS_IS_NOT_A_VALID_GIT_CACHE").unwrap();

        let result = cmd_info_json_for_dir(&tmp);
        let indexes = result["indexes"].as_array().expect("indexes should be an array");
        let git_entries: Vec<_> = indexes.iter()
            .filter(|idx| idx["type"] == "git-history")
            .collect();
        assert_eq!(git_entries.len(), 0, "Corrupt git-history file should be skipped");

        // Cleanup
        std::fs::remove_dir_all(&tmp).unwrap_or_default();
    }

    #[test]
    fn test_meta_sidecar_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("search_meta_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // Create a fake index file and write a .meta sidecar
        let index_path = tmp.join("test_12345678.word-search");
        std::fs::write(&index_path, b"fake index data").unwrap();

        let meta = crate::index::IndexMeta {
            index_type: "content".to_string(),
            root: "C:/Repos/TestProject".to_string(),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            max_age_secs: 86400,
            files: 42,
            unique_tokens: Some(1000),
            total_tokens: Some(50000),
            extensions: vec!["cs".to_string(), "xml".to_string()],
            definitions: None,
            call_sites: None,
            parse_errors: None,
            lossy_file_count: None,
            entries: None,
            commits: None,
            authors: None,
            branch: None,
            head_hash: None,
        };

        crate::index::save_index_meta(&index_path, &meta);

        // Verify .meta file exists
        let meta_path = index_path.with_extension("word-search.meta");
        assert!(meta_path.exists(), "Meta file should exist at {}", meta_path.display());

        // Verify it loads back
        let loaded = crate::index::load_index_meta(&index_path);
        assert!(loaded.is_some(), "Should be able to load meta from sidecar");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.index_type, "content");
        assert_eq!(loaded.root, "C:/Repos/TestProject");
        assert_eq!(loaded.files, 42);
        assert_eq!(loaded.unique_tokens, Some(1000));
        assert_eq!(loaded.total_tokens, Some(50000));
        assert_eq!(loaded.extensions, vec!["cs", "xml"]);

        // Verify cmd_info_json_for_dir reads from .meta
        let result = cmd_info_json_for_dir(&tmp);
        let indexes = result["indexes"].as_array().expect("indexes should be an array");
        // Should find the content index via .meta (even though the .word-search file is fake)
        let content_entries: Vec<_> = indexes.iter()
            .filter(|idx| idx["type"] == "content")
            .collect();
        assert_eq!(content_entries.len(), 1, "Should find 1 content index via .meta");
        assert_eq!(content_entries[0]["root"], "C:/Repos/TestProject");
        assert_eq!(content_entries[0]["files"], 42);
        assert_eq!(content_entries[0]["totalTokens"], 50000);

        // Cleanup
        std::fs::remove_dir_all(&tmp).unwrap_or_default();
    }

    #[test]
    fn test_meta_to_json_all_types() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Content
        let meta = IndexMeta {
            index_type: "content".to_string(),
            root: "C:/test".to_string(),
            created_at: now,
            max_age_secs: 3600,
            files: 100,
            unique_tokens: Some(5000),
            total_tokens: Some(100000),
            extensions: vec!["cs".to_string()],
            definitions: None,
            call_sites: None,
            parse_errors: None,
            lossy_file_count: None,
            entries: None,
            commits: None,
            authors: None,
            branch: None,
            head_hash: None,
        };
        let json = meta_to_json(&meta, 1_048_576, "test.word-search");
        assert_eq!(json["type"], "content");
        assert_eq!(json["files"], 100);
        assert_eq!(json["totalTokens"], 100000);
        assert_eq!(json["sizeMb"], 1.0);

        // Definition
        let meta = IndexMeta {
            index_type: "definition".to_string(),
            root: "C:/test".to_string(),
            created_at: now,
            max_age_secs: 0,
            files: 50,
            unique_tokens: None,
            total_tokens: None,
            extensions: vec!["cs".to_string()],
            definitions: Some(1000),
            call_sites: Some(5000),
            parse_errors: Some(3),
            lossy_file_count: None,
            entries: None,
            commits: None,
            authors: None,
            branch: None,
            head_hash: None,
        };
        let json = meta_to_json(&meta, 2_097_152, "test.code-structure");
        assert_eq!(json["type"], "definition");
        assert_eq!(json["definitions"], 1000);
        assert_eq!(json["callSites"], 5000);
        assert_eq!(json["readErrors"], 3);

        // Git history
        let meta = IndexMeta {
            index_type: "git-history".to_string(),
            root: String::new(),
            created_at: now,
            max_age_secs: 0,
            files: 1000,
            unique_tokens: None,
            total_tokens: None,
            extensions: Vec::new(),
            definitions: None,
            call_sites: None,
            parse_errors: None,
            lossy_file_count: None,
            entries: None,
            commits: Some(5000),
            authors: Some(20),
            branch: Some("main".to_string()),
            head_hash: Some("abc123def456".to_string()),
        };
        let json = meta_to_json(&meta, 524288, "test.git-history");
        assert_eq!(json["type"], "git-history");
        assert_eq!(json["commits"], 5000);
        assert_eq!(json["authors"], 20);
        assert_eq!(json["branch"], "main");
    }
}