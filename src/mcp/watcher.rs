use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{error, info, warn};

use crate::{clean_path, tokenize, ContentIndex, Posting, DEFAULT_MIN_TOKEN_LEN};
use crate::definitions::{self, DefinitionIndex};

/// Start a file watcher thread that incrementally updates the in-memory index
pub fn start_watcher(
    index: Arc<RwLock<ContentIndex>>,
    def_index: Option<Arc<RwLock<DefinitionIndex>>>,
    dir: PathBuf,
    extensions: Vec<String>,
    debounce_ms: u64,
    index_base: PathBuf,
) -> notify::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&dir, RecursiveMode::Recursive)?;

    let dir_str = clean_path(&dir.to_string_lossy());

    info!(dir = %dir_str, debounce_ms, "File watcher started");

    std::thread::spawn(move || {
        let _watcher = watcher; // move watcher into thread to keep it alive
        let _index_base = index_base; // keep for potential future use (e.g., periodic save)

        // ── Reconciliation: catch files added/modified/removed while server was offline ──
        // Watcher is already listening — events during reconciliation are buffered in rx channel.
        reconcile_content_index(&index, &dir_str, &extensions);
        if let Some(ref def_idx) = def_index {
            match def_idx.write() {
                Ok(mut idx) => {
                    definitions::reconcile_definition_index(&mut idx, &dir_str, &extensions);
                }
                Err(e) => {
                    error!(error = %e, "Failed to acquire def index write lock for reconciliation");
                }
            }
        }

        let mut dirty_files: HashSet<PathBuf> = HashSet::new();
        let mut removed_files: HashSet<PathBuf> = HashSet::new();

        loop {
            match rx.recv_timeout(Duration::from_millis(debounce_ms)) {
                Ok(Ok(event)) => {
                    // Collect changed files
                    for path in &event.paths {
                        // Skip .git directory — git operations generate massive event floods
                        // and .git/config matches the "config" extension filter
                        if is_inside_git_dir(path) {
                            continue;
                        }
                        if !matches_extensions(path, &extensions) {
                            continue;
                        }
                        match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) => {
                                removed_files.remove(path);
                                dirty_files.insert(path.clone());
                            }
                            EventKind::Remove(_) => {
                                dirty_files.remove(path);
                                removed_files.insert(path.clone());
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "File watcher error");
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Debounce window expired — process batch
                    if dirty_files.is_empty() && removed_files.is_empty() {
                        continue;
                    }

                    // Incremental update — always used (bulk path removed to fix
                    // definition index skip bug and improve git pull performance).
                    // Uses batch_purge_files for O(total_postings) instead of
                    // O(N × total_postings) when many files change at once.
                    let update_count = dirty_files.len();
                    let remove_count = removed_files.len();

                    // Collect cleaned paths once for both indexes
                    let removed_clean: Vec<PathBuf> = removed_files.drain()
                        .map(|p| PathBuf::from(clean_path(&p.to_string_lossy())))
                        .collect();
                    let dirty_clean: Vec<PathBuf> = dirty_files.drain()
                        .map(|p| PathBuf::from(clean_path(&p.to_string_lossy())))
                        .collect();

                    let batch_start = std::time::Instant::now();

                    // Update content index using batch_purge for O(total_postings) instead of O(N × total_postings)
                    match index.write() {
                        Ok(mut idx) => {
                            // Collect file_ids of all existing files to purge in one pass
                            let mut purge_ids: HashSet<u32> = HashSet::new();
                            if let Some(ref p2id) = idx.path_to_id {
                                for path in &removed_clean {
                                    if let Some(&fid) = p2id.get(path) {
                                        purge_ids.insert(fid);
                                    }
                                }
                                for path in &dirty_clean {
                                    if let Some(&fid) = p2id.get(path) {
                                        purge_ids.insert(fid);
                                    }
                                }
                            }

                            // Batch purge: ONE pass over inverted index removes all stale postings
                            if !purge_ids.is_empty() {
                                // Subtract old token counts before purge
                                for &fid in &purge_ids {
                                    let old_count = if (fid as usize) < idx.file_token_counts.len() {
                                        idx.file_token_counts[fid as usize] as u64
                                    } else {
                                        0u64
                                    };
                                    idx.total_tokens = idx.total_tokens.saturating_sub(old_count);
                                }
                                batch_purge_files(&mut idx.index, &purge_ids);
                            }

                            // Process removed files: update path_to_id and zero token counts
                            for path in &removed_clean {
                                // Two-step borrow: look up fid first, then mutate
                                let fid = idx.path_to_id.as_ref()
                                    .and_then(|p2id| p2id.get(path).copied());
                                if let Some(fid) = fid {
                                    if (fid as usize) < idx.file_token_counts.len() {
                                        idx.file_token_counts[fid as usize] = 0;
                                    }
                                    if let Some(ref mut p2id) = idx.path_to_id {
                                        p2id.remove(path);
                                    }
                                }
                            }

                            // Process dirty files: re-tokenize and insert new postings
                            // (purge already done above via batch_purge_files)
                            for path in &dirty_clean {
                                reindex_file_after_purge(&mut idx, path);
                            }

                            // Mark trigram index as dirty — will be rebuilt lazily on next substring search
                            idx.trigram_dirty = true;

                            // Conditionally shrink collections after retain() to release excess capacity.
                            // Only shrink when capacity > 2 × len to avoid unnecessary realloc storms.
                            for postings in idx.index.values_mut() {
                                if postings.capacity() > postings.len() * 2 {
                                    postings.shrink_to_fit();
                                }
                            }
                            if idx.index.capacity() > idx.index.len() * 2 {
                                idx.index.shrink_to_fit();
                            }
                            if let Some(ref mut p2id) = idx.path_to_id {
                                if p2id.capacity() > p2id.len() * 2 {
                                    p2id.shrink_to_fit();
                                }
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to acquire content index write lock");
                        }
                    }

                    // Update definition index (if available)
                    if let Some(ref def_idx) = def_index {
                        match def_idx.write() {
                            Ok(mut idx) => {
                                for path in &removed_clean {
                                    definitions::remove_file_from_def_index(&mut idx, path);
                                }
                                for path in &dirty_clean {
                                    definitions::update_file_definitions(&mut idx, path);
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to acquire definition index write lock");
                            }
                        }
                    }

                    let elapsed_ms = batch_start.elapsed().as_secs_f64() * 1000.0;
                    info!(
                        updated = update_count,
                        removed = remove_count,
                        elapsed_ms = format_args!("{:.1}", elapsed_ms),
                        "Incremental index update complete"
                    );
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    info!("Watcher channel disconnected, stopping");
                    break;
                }
            }
        }
    });

    Ok(())
}

/// Check if a path is inside a `.git` directory.
/// Filters out git internal files that would otherwise match extension filters
/// (e.g., `.git/config` matches "config" extension).
fn is_inside_git_dir(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".git")
}

fn matches_extensions(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Build a ContentIndex with path_to_id populated (for watch mode).
///
/// Previously also built a forward index (file_id → Vec<token>) which consumed
/// ~1.5 GB of RAM due to cloning every token string for every file it appears in.
/// Now we only build path_to_id; on file change we scan the inverted index directly
/// to remove stale postings (~50-100ms per file, acceptable for watcher debounce).
pub fn build_watch_index_from(mut index: ContentIndex) -> ContentIndex {
    let mut path_to_id: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();

    for (i, path) in index.files.iter().enumerate() {
        path_to_id.insert(PathBuf::from(path), i as u32);
    }

    // Drop any legacy forward index loaded from disk
    index.forward = None;
    index.path_to_id = Some(path_to_id);
    index
}

/// Update a single file in the index (incremental).
///
/// Uses brute-force scan of the inverted index to remove old postings for the file,
/// avoiding the need for a forward index (which consumed ~1.5 GB of RAM).
fn update_file_in_index(index: &mut ContentIndex, path: &Path) {
    let path_str = path.to_string_lossy().to_string();

    // Read the file (lossy UTF-8 for non-UTF8 files like Windows-1252)
    let (content, _was_lossy) = match crate::read_file_lossy(path) {
        Ok(r) => r,
        Err(_) => return, // File might have been deleted between event and processing
    };

    if let Some(ref mut path_to_id) = index.path_to_id {
        if let Some(&file_id) = path_to_id.get(path) {
            // EXISTING FILE — remove old tokens, add new ones
            // Subtract old token count from total before re-tokenizing
            let old_count = if (file_id as usize) < index.file_token_counts.len() {
                index.file_token_counts[file_id as usize] as u64
            } else {
                0u64
            };
            index.total_tokens = index.total_tokens.saturating_sub(old_count);

            // Remove all postings for this file_id from the inverted index (brute-force scan).
            // This replaces the forward index lookup — O(total_tokens) but saves ~1.5 GB RAM.
            purge_file_from_inverted_index(&mut index.index, file_id);

            // Re-tokenize file
            let mut file_tokens: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
            let mut file_total: u32 = 0;
            for (line_num, line) in content.lines().enumerate() {
                for token in tokenize(line, DEFAULT_MIN_TOKEN_LEN) {
                    index.total_tokens += 1;
                    file_total += 1;
                    file_tokens.entry(token).or_default().push((line_num + 1) as u32);
                }
            }

            // Add new tokens to inverted index
            for (token, lines) in &file_tokens {
                index.index.entry(token.clone())
                    .or_default()
                    .push(Posting { file_id, lines: lines.clone() });
            }

            // Update file token count
            if (file_id as usize) < index.file_token_counts.len() {
                index.file_token_counts[file_id as usize] = file_total;
            } else {
                warn!(file_id, len = index.file_token_counts.len(), "file_token_counts out of bounds, TF-IDF scores may be stale");
            }
        } else {
            // NEW FILE — assign new file_id
            let file_id = index.files.len() as u32;
            index.files.push(path_str.clone());
            path_to_id.insert(path.to_path_buf(), file_id);

            let mut file_tokens: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
            let mut file_total: u32 = 0;
            for (line_num, line) in content.lines().enumerate() {
                for token in tokenize(line, DEFAULT_MIN_TOKEN_LEN) {
                    index.total_tokens += 1;
                    file_total += 1;
                    file_tokens.entry(token).or_default().push((line_num + 1) as u32);
                }
            }

            for (token, lines) in &file_tokens {
                index.index.entry(token.clone())
                    .or_default()
                    .push(Posting { file_id, lines: lines.clone() });
            }

            index.file_token_counts.push(file_total);
        }
    }
}

/// Remove all postings for a SET of file_ids from the inverted index in ONE pass.
///
/// O(total_postings) regardless of how many file_ids — replaces N sequential calls
/// to `purge_file_from_inverted_index` which was O(N × total_postings).
///
/// For git pull with 300 files: ~500ms single pass vs ~30s sequential.
/// For git checkout with 10K files: ~500ms single pass vs ~120s sequential.
fn batch_purge_files(
    inverted: &mut std::collections::HashMap<String, Vec<Posting>>,
    file_ids: &HashSet<u32>,
) {
    if file_ids.is_empty() {
        return;
    }
    inverted.retain(|_token, postings| {
        postings.retain(|p| !file_ids.contains(&p.file_id));
        !postings.is_empty()
    });
}

/// Re-tokenize a file and insert new postings into the index.
///
/// This function assumes the file's old postings have ALREADY been purged
/// (via `batch_purge_files`). It only reads the file, tokenizes, and inserts.
/// For new files (not in path_to_id), it assigns a new file_id.
fn reindex_file_after_purge(index: &mut ContentIndex, path: &Path) {
    let path_str = path.to_string_lossy().to_string();

    let (content, _was_lossy) = match crate::read_file_lossy(path) {
        Ok(r) => r,
        Err(_) => return,
    };

    if let Some(ref mut path_to_id) = index.path_to_id {
        let file_id = if let Some(&fid) = path_to_id.get(path) {
            // Existing file — already purged, just re-tokenize
            fid
        } else {
            // New file — assign new file_id
            let fid = index.files.len() as u32;
            index.files.push(path_str);
            path_to_id.insert(path.to_path_buf(), fid);
            index.file_token_counts.push(0); // will be updated below
            fid
        };

        let mut file_tokens: HashMap<String, Vec<u32>> = HashMap::new();
        let mut file_total: u32 = 0;
        for (line_num, line) in content.lines().enumerate() {
            for token in tokenize(line, DEFAULT_MIN_TOKEN_LEN) {
                index.total_tokens += 1;
                file_total += 1;
                file_tokens.entry(token).or_default().push((line_num + 1) as u32);
            }
        }

        for (token, lines) in &file_tokens {
            index.index.entry(token.clone())
                .or_default()
                .push(Posting { file_id, lines: lines.clone() });
        }

        if (file_id as usize) < index.file_token_counts.len() {
            index.file_token_counts[file_id as usize] = file_total;
        }
    }
}

/// Remove all postings for a given file_id from the inverted index.
/// This is a brute-force O(total_tokens) scan that replaces the forward index lookup.
/// Typically takes ~50-100ms for 400K tokens, which is acceptable for single-file events.
///
/// For batch operations (git pull, git checkout), prefer `batch_purge_files` which
/// removes multiple file_ids in a single pass — O(total_postings) regardless of N.
fn purge_file_from_inverted_index(
    inverted: &mut std::collections::HashMap<String, Vec<Posting>>,
    file_id: u32,
) {
    inverted.retain(|_token, postings| {
        postings.retain(|p| p.file_id != file_id);
        !postings.is_empty()
    });
}

/// Remove a file from the index.
///
/// Uses brute-force scan of the inverted index instead of forward index lookup,
/// saving ~1.5 GB of RAM at the cost of ~50-100ms per file removal.
fn remove_file_from_index(index: &mut ContentIndex, path: &Path) {
    if let Some(ref mut path_to_id) = index.path_to_id
        && let Some(&file_id) = path_to_id.get(path) {
            // Subtract this file's token count from total
            let old_count = if (file_id as usize) < index.file_token_counts.len() {
                index.file_token_counts[file_id as usize] as u64
            } else {
                0u64
            };
            index.total_tokens = index.total_tokens.saturating_sub(old_count);
            // Zero out the file's token count (file stays in vec as tombstone)
            if (file_id as usize) < index.file_token_counts.len() {
                index.file_token_counts[file_id as usize] = 0;
            }

            // Remove all postings for this file from inverted index (brute-force scan)
            purge_file_from_inverted_index(&mut index.index, file_id);

            path_to_id.remove(path);
            // Don't remove from files vec to preserve file_id stability
        }
}

/// Reconcile content index with filesystem after loading from disk cache.
///
/// Walks the filesystem and compares with the in-memory index to find:
/// - **Added** files: exist on disk but not in `path_to_id` → tokenize and add
/// - **Modified** files: exist in both but `mtime > index.created_at` → re-tokenize
/// - **Deleted** files: exist in `path_to_id` but not on disk → remove
///
/// Uses a 2-second safety margin on `created_at` to handle clock precision.
/// Takes the write lock on the index for the duration of the update.
///
/// Returns `(added, modified, removed)` counts.
fn reconcile_content_index(
    index: &Arc<RwLock<ContentIndex>>,
    dir: &str,
    extensions: &[String],
) {
    let start = std::time::Instant::now();
    let dir_path = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));

    // Read created_at before acquiring write lock
    let created_at = match index.read() {
        Ok(idx) => idx.created_at,
        Err(e) => {
            error!(error = %e, "Failed to read content index for reconciliation");
            return;
        }
    };

    let threshold = UNIX_EPOCH + Duration::from_secs(created_at.saturating_sub(2));

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

    // Acquire write lock and perform reconciliation
    match index.write() {
        Ok(mut idx) => {
            // Collect indexed paths for deletion check
            let indexed_paths: HashSet<PathBuf> = idx.path_to_id
                .as_ref()
                .map(|p2id| p2id.keys().cloned().collect())
                .unwrap_or_default();

            let mut added = 0usize;
            let mut modified = 0usize;
            let mut removed = 0usize;

            // Check for new and modified files
            for (path, mtime) in &disk_files {
                let in_index = idx.path_to_id
                    .as_ref()
                    .is_some_and(|p2id| p2id.contains_key(path));

                if !in_index {
                    // NEW file
                    update_file_in_index(&mut idx, path);
                    added += 1;
                } else if *mtime > threshold {
                    // MODIFIED file
                    update_file_in_index(&mut idx, path);
                    modified += 1;
                }
            }

            // Check for deleted files
            for path in &indexed_paths {
                if !disk_files.contains_key(path) {
                    remove_file_from_index(&mut idx, path);
                    removed += 1;
                }
            }

            // Mark trigram as dirty if anything changed
            if added > 0 || modified > 0 || removed > 0 {
                idx.trigram_dirty = true;
            }

            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

            if added > 0 || modified > 0 || removed > 0 {
                info!(
                    scanned,
                    added,
                    modified,
                    removed,
                    elapsed_ms = format_args!("{:.1}", elapsed_ms),
                    "Content index reconciliation complete"
                );
            } else {
                info!(
                    scanned,
                    elapsed_ms = format_args!("{:.1}", elapsed_ms),
                    "Content index reconciliation: all files up to date"
                );
            }

            crate::index::log_memory(&format!(
                "watcher: content reconciliation (scanned={}, added={}, modified={}, removed={}, {:.0}ms)",
                scanned, added, modified, removed, elapsed_ms
            ));
        }
        Err(e) => {
            error!(error = %e, "Failed to acquire content index write lock for reconciliation");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::TrigramIndex;

    fn make_test_index() -> ContentIndex {
        let mut idx = HashMap::new();
        idx.insert("httpclient".to_string(), vec![Posting {
            file_id: 0,
            lines: vec![5, 12],
        }]);
        idx.insert("ilogger".to_string(), vec![Posting {
            file_id: 0,
            lines: vec![3],
        }, Posting {
            file_id: 1,
            lines: vec![1],
        }]);

        ContentIndex {
            root: ".".to_string(),
            created_at: 0,
            max_age_secs: 3600,
            files: vec!["file0.cs".to_string(), "file1.cs".to_string()],
            index: idx,
            total_tokens: 100,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![50, 30],
            trigram: TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: None, read_errors: 0, lossy_file_count: 0,
        }
    }

    #[test]
    fn test_build_watch_index_no_forward_but_has_path_to_id() {
        let index = make_test_index();
        let watch_index = build_watch_index_from(index);

        // Forward index is no longer built (saves ~1.5 GB RAM)
        assert!(watch_index.forward.is_none());
        assert!(watch_index.path_to_id.is_some());
    }

    #[test]
    fn test_build_watch_index_populates_path_to_id() {
        let index = make_test_index();
        let watch_index = build_watch_index_from(index);

        let path_to_id = watch_index.path_to_id.as_ref().unwrap();
        assert_eq!(path_to_id.get(&PathBuf::from("file0.cs")), Some(&0));
        assert_eq!(path_to_id.get(&PathBuf::from("file1.cs")), Some(&1));
    }

    #[test]
    fn test_incremental_update_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let new_file = dir.join("new_file.cs");
        std::fs::write(&new_file, "class NewClass { HttpClient client; }").unwrap();

        let mut index = make_test_index();
        index.path_to_id = Some(HashMap::new());
        // Populate path_to_id for existing files
        for (i, path) in index.files.iter().enumerate() {
            index.path_to_id.as_mut().unwrap().insert(PathBuf::from(path), i as u32);
        }

        let clean_path = PathBuf::from(crate::clean_path(&new_file.to_string_lossy()));
        update_file_in_index(&mut index, &clean_path);

        // New file should be added
        assert_eq!(index.files.len(), 3);
        assert!(index.index.contains_key("newclass"));
        assert!(index.index.contains_key("httpclient"));
    }

    #[test]
    fn test_incremental_update_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let test_file = dir.join("test.cs");
        std::fs::write(&test_file, "class Original { OldToken stuff; }").unwrap();

        let clean = crate::clean_path(&test_file.to_string_lossy());
        let mut index = ContentIndex {
            root: ".".to_string(),
            created_at: 0,
            max_age_secs: 3600,
            files: vec![clean.clone()],
            index: {
                let mut m = HashMap::new();
                m.insert("original".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
                m.insert("oldtoken".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
                m
            },
            total_tokens: 10,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![5],
            trigram: TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: Some({
                let mut m = HashMap::new();
                m.insert(PathBuf::from(&clean), 0u32);
                m
            }),
            read_errors: 0,
            lossy_file_count: 0,
        };

        // Now update the file content
        std::fs::write(&test_file, "class Updated { NewToken stuff; }").unwrap();
        update_file_in_index(&mut index, &PathBuf::from(&clean));

        // Old tokens should be gone, new tokens should be present
        assert!(!index.index.contains_key("original"), "old token 'original' should be removed");
        assert!(!index.index.contains_key("oldtoken"), "old token 'oldtoken' should be removed");
        assert!(index.index.contains_key("updated"), "new token 'updated' should be present");
        assert!(index.index.contains_key("newtoken"), "new token 'newtoken' should be present");
    }

    #[test]
    fn test_remove_file() {
        let mut index = make_test_index();
        // Build path_to_id (no forward index needed)
        index = build_watch_index_from(index);

        // Remove file0.cs
        remove_file_from_index(&mut index, &PathBuf::from("file0.cs"));

        // httpclient was only in file0 — should be gone from index
        assert!(!index.index.contains_key("httpclient"), "httpclient should be removed with file0");

        // ilogger was in both files — should still exist for file1
        let ilogger = index.index.get("ilogger").unwrap();
        assert_eq!(ilogger.len(), 1);
        assert_eq!(ilogger[0].file_id, 1);

        // path_to_id should not contain file0 anymore
        let path_to_id = index.path_to_id.as_ref().unwrap();
        assert!(!path_to_id.contains_key(&PathBuf::from("file0.cs")));
        // files vec still has file0 for ID stability
        assert_eq!(index.files.len(), 2);
    }

    #[test]
    fn test_matches_extensions() {
        let exts = vec!["cs".to_string(), "rs".to_string()];
        assert!(matches_extensions(Path::new("foo.cs"), &exts));
        assert!(matches_extensions(Path::new("bar.RS"), &exts));
        assert!(!matches_extensions(Path::new("baz.txt"), &exts));
        assert!(!matches_extensions(Path::new("no_ext"), &exts));
    }

    #[test]
    fn test_is_inside_git_dir() {
        // Should detect .git directory in various positions
        assert!(is_inside_git_dir(Path::new(".git/config")));
        assert!(is_inside_git_dir(Path::new(".git/HEAD")));
        assert!(is_inside_git_dir(Path::new("repo/.git/config")));
        assert!(is_inside_git_dir(Path::new("repo/.git/modules/sub/config")));
        assert!(is_inside_git_dir(Path::new("C:/Projects/repo/.git/objects/pack/pack-abc.idx")));

        // Should NOT flag normal files
        assert!(!is_inside_git_dir(Path::new("src/main.rs")));
        assert!(!is_inside_git_dir(Path::new("my-git-tool/config.xml")));
        assert!(!is_inside_git_dir(Path::new(".gitignore")));
        assert!(!is_inside_git_dir(Path::new(".github/workflows/ci.yml")));
        assert!(!is_inside_git_dir(Path::new("docs/git-workflow.md")));
    }

    #[test]
    fn test_purge_file_from_inverted_index_removes_single_file() {
        let mut inverted = HashMap::new();
        inverted.insert("token_a".to_string(), vec![
            Posting { file_id: 0, lines: vec![1, 5] },
            Posting { file_id: 1, lines: vec![3] },
        ]);
        inverted.insert("token_b".to_string(), vec![
            Posting { file_id: 0, lines: vec![2] },
        ]);
        inverted.insert("token_c".to_string(), vec![
            Posting { file_id: 1, lines: vec![10] },
        ]);

        purge_file_from_inverted_index(&mut inverted, 0);

        // token_a should still exist but only for file_id 1
        let token_a = inverted.get("token_a").unwrap();
        assert_eq!(token_a.len(), 1);
        assert_eq!(token_a[0].file_id, 1);

        // token_b was only in file_id 0 → should be removed entirely
        assert!(!inverted.contains_key("token_b"), "token_b should be removed when its only file is purged");

        // token_c should be untouched
        assert!(inverted.contains_key("token_c"));
        assert_eq!(inverted["token_c"][0].file_id, 1);
    }

    #[test]
    fn test_purge_file_from_inverted_index_nonexistent_file() {
        let mut inverted = HashMap::new();
        inverted.insert("token".to_string(), vec![
            Posting { file_id: 0, lines: vec![1] },
        ]);

        // Purging a file_id that doesn't exist should be a no-op
        purge_file_from_inverted_index(&mut inverted, 99);

        assert_eq!(inverted.len(), 1);
        assert_eq!(inverted["token"][0].file_id, 0);
    }

    #[test]
    fn test_purge_file_from_inverted_index_empty_index() {
        let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
        purge_file_from_inverted_index(&mut inverted, 0);
        assert!(inverted.is_empty());
    }

    #[test]
    fn test_build_watch_drops_legacy_forward_index() {
        let mut index = make_test_index();
        // Simulate a legacy index loaded from disk with a forward index populated
        index.forward = Some({
            let mut m = HashMap::new();
            m.insert(0u32, vec!["httpclient".to_string(), "ilogger".to_string()]);
            m.insert(1u32, vec!["ilogger".to_string()]);
            m
        });

        let watch_index = build_watch_index_from(index);

        // Forward index should be dropped (saves ~1.5 GB RAM)
        assert!(watch_index.forward.is_none(), "forward index should be None after build_watch_index_from");
        // path_to_id should still be populated
        assert!(watch_index.path_to_id.is_some());
        assert_eq!(watch_index.path_to_id.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_remove_file_without_forward_index() {
        // Verify that remove works via brute-force scan (no forward index)
        let mut index = make_test_index();
        index.forward = None; // explicitly no forward index
        index.path_to_id = Some({
            let mut m = HashMap::new();
            m.insert(PathBuf::from("file0.cs"), 0u32);
            m.insert(PathBuf::from("file1.cs"), 1u32);
            m
        });

        remove_file_from_index(&mut index, &PathBuf::from("file0.cs"));

        // httpclient was only in file0 — should be gone
        assert!(!index.index.contains_key("httpclient"));
        // ilogger was in both files — should still exist for file1
        let ilogger = index.index.get("ilogger").unwrap();
        assert_eq!(ilogger.len(), 1);
        assert_eq!(ilogger[0].file_id, 1);
    }

    #[test]
    fn test_update_existing_file_without_forward_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let test_file = dir.join("test.cs");
        std::fs::write(&test_file, "class Original { OldToken stuff; }").unwrap();

        let clean = crate::clean_path(&test_file.to_string_lossy());
        let mut index = ContentIndex {
            root: ".".to_string(),
            created_at: 0,
            max_age_secs: 3600,
            files: vec![clean.clone()],
            index: {
                let mut m = HashMap::new();
                m.insert("original".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
                m.insert("oldtoken".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
                m
            },
            total_tokens: 10,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![5],
            trigram: TrigramIndex::default(),
            trigram_dirty: false,
            forward: None, // no forward index!
            path_to_id: Some({
                let mut m = HashMap::new();
                m.insert(PathBuf::from(&clean), 0u32);
                m
            }),
            read_errors: 0,
            lossy_file_count: 0,
        };

        // Update file content
        std::fs::write(&test_file, "class Updated { NewToken stuff; }").unwrap();
        update_file_in_index(&mut index, &PathBuf::from(&clean));

        // Old tokens removed via brute-force scan, new tokens added
        assert!(!index.index.contains_key("original"), "old token should be removed");
        assert!(!index.index.contains_key("oldtoken"), "old token should be removed");
        assert!(index.index.contains_key("updated"), "new token should be present");
        assert!(index.index.contains_key("newtoken"), "new token should be present");
    }

    #[test]
    fn test_batch_purge_files_removes_multiple_files() {
        let mut inverted = HashMap::new();
        inverted.insert("token_a".to_string(), vec![
            Posting { file_id: 0, lines: vec![1] },
            Posting { file_id: 1, lines: vec![2] },
            Posting { file_id: 2, lines: vec![3] },
        ]);
        inverted.insert("token_b".to_string(), vec![
            Posting { file_id: 0, lines: vec![5] },
            Posting { file_id: 2, lines: vec![6] },
        ]);
        inverted.insert("token_c".to_string(), vec![
            Posting { file_id: 1, lines: vec![10] },
        ]);

        let mut ids = HashSet::new();
        ids.insert(0);
        ids.insert(2);
        batch_purge_files(&mut inverted, &ids);

        // token_a should only have file_id 1
        let token_a = inverted.get("token_a").unwrap();
        assert_eq!(token_a.len(), 1);
        assert_eq!(token_a[0].file_id, 1);

        // token_b was only in files 0 and 2 → should be removed entirely
        assert!(!inverted.contains_key("token_b"), "token_b should be removed");

        // token_c was only in file 1 → should be untouched
        assert!(inverted.contains_key("token_c"));
        assert_eq!(inverted["token_c"][0].file_id, 1);
    }

    #[test]
    fn test_batch_purge_files_empty_set() {
        let mut inverted = HashMap::new();
        inverted.insert("token".to_string(), vec![
            Posting { file_id: 0, lines: vec![1] },
        ]);

        batch_purge_files(&mut inverted, &HashSet::new());

        // Should be a no-op
        assert_eq!(inverted.len(), 1);
        assert_eq!(inverted["token"][0].file_id, 0);
    }

    #[test]
    fn test_batch_purge_files_single_file_equivalent_to_purge_single() {
        // Verify that batch_purge with 1 file_id gives same result as purge_file_from_inverted_index
        let mut inverted1 = HashMap::new();
        inverted1.insert("token_a".to_string(), vec![
            Posting { file_id: 0, lines: vec![1] },
            Posting { file_id: 1, lines: vec![2] },
        ]);
        inverted1.insert("token_b".to_string(), vec![
            Posting { file_id: 0, lines: vec![5] },
        ]);

        let mut inverted2 = inverted1.clone();

        // Single purge
        purge_file_from_inverted_index(&mut inverted1, 0);

        // Batch purge with 1 element
        let mut ids = HashSet::new();
        ids.insert(0);
        batch_purge_files(&mut inverted2, &ids);

        // Results should be identical
        assert_eq!(inverted1.len(), inverted2.len());
        for (key, val1) in &inverted1 {
            let val2 = inverted2.get(key).unwrap();
            assert_eq!(val1.len(), val2.len());
            for (p1, p2) in val1.iter().zip(val2.iter()) {
                assert_eq!(p1.file_id, p2.file_id);
                assert_eq!(p1.lines, p2.lines);
            }
        }
    }

    #[test]
    fn test_total_tokens_decremented_on_update() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let test_file = dir.join("test.cs");
        std::fs::write(&test_file, "class Original { OldToken stuff; }").unwrap();

        let clean = crate::clean_path(&test_file.to_string_lossy());
        let mut index = ContentIndex {
            root: ".".to_string(),
            created_at: 0,
            max_age_secs: 3600,
            files: vec![clean.clone()],
            index: {
                let mut m = HashMap::new();
                m.insert("original".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
                m.insert("oldtoken".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
                m.insert("stuff".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
                m.insert("class".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
                m
            },
            total_tokens: 4,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![4],
            trigram: TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: Some({
                let mut m = HashMap::new();
                m.insert(PathBuf::from(&clean), 0u32);
                m
            }),
            read_errors: 0,
            lossy_file_count: 0,
        };

        // Update file with different content
        std::fs::write(&test_file, "class Updated { NewToken stuff; }").unwrap();
        update_file_in_index(&mut index, &PathBuf::from(&clean));

        // total_tokens should equal sum of file_token_counts
        let sum: u64 = index.file_token_counts.iter().map(|&c| c as u64).sum();
        assert_eq!(index.total_tokens, sum,
            "total_tokens ({}) should equal sum of file_token_counts ({})",
            index.total_tokens, sum);
    }

    #[test]
    fn test_total_tokens_decremented_on_remove() {
        let mut index = make_test_index();
        index = build_watch_index_from(index);

        let initial_total = index.total_tokens;
        let file0_tokens = index.file_token_counts[0] as u64;

        remove_file_from_index(&mut index, &PathBuf::from("file0.cs"));

        assert_eq!(index.total_tokens, initial_total - file0_tokens,
            "total_tokens should decrease by file0's token count");
    }

    #[test]
    fn test_total_tokens_consistency_after_multiple_ops() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let file1 = dir.join("a.cs");
        let file2 = dir.join("b.cs");
        std::fs::write(&file1, "class Alpha { }").unwrap();
        std::fs::write(&file2, "class Beta { }").unwrap();

        let mut index = ContentIndex {
            root: ".".to_string(),
            created_at: 0,
            max_age_secs: 3600,
            files: vec![],
            index: HashMap::new(),
            total_tokens: 0,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![],
            trigram: TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: Some(HashMap::new()),
            read_errors: 0,
            lossy_file_count: 0,
        };

        // Add file1
        let clean1 = PathBuf::from(crate::clean_path(&file1.to_string_lossy()));
        update_file_in_index(&mut index, &clean1);

        // Add file2
        let clean2 = PathBuf::from(crate::clean_path(&file2.to_string_lossy()));
        update_file_in_index(&mut index, &clean2);

        // Update file1 with new content
        std::fs::write(&file1, "class AlphaUpdated { NewMethod(); }").unwrap();
        update_file_in_index(&mut index, &clean1);

        // Remove file2
        remove_file_from_index(&mut index, &clean2);

        // Verify consistency: total_tokens == sum(file_token_counts) for non-removed files
        let sum: u64 = index.file_token_counts.iter().map(|&c| c as u64).sum();
        assert_eq!(index.total_tokens, sum,
            "total_tokens ({}) should equal sum of file_token_counts ({}) after multiple operations",
            index.total_tokens, sum);
    }

    #[test]
    fn test_watch_index_survives_save_load_roundtrip() {
        // Verify that a ContentIndex with path_to_id (watch-mode field)
        // can be saved to disk and loaded back with all data intact.
        // This is critical for save-on-shutdown: if path_to_id doesn't serialize
        // properly, the loaded index would lose incremental updates.
        //
        // Note: forward index was intentionally dropped in build_watch_index_from
        // (memory optimization commit b43473c) to save ~1.5 GB RAM.
        // Only path_to_id is preserved for watch-mode operation.
        let tmp = tempfile::tempdir().unwrap();

        // Build a watch-mode index with path_to_id populated (forward is None)
        let index = make_test_index();
        let watch_index = build_watch_index_from(index);

        // Verify watch fields before save
        assert!(watch_index.forward.is_none(), "forward should be None (dropped to save RAM)");
        assert!(watch_index.path_to_id.is_some(), "path_to_id should be populated");
        let orig_files = watch_index.files.len();
        let orig_tokens = watch_index.index.len();
        let orig_path_to_id_len = watch_index.path_to_id.as_ref().unwrap().len();

        // Save to disk
        crate::save_content_index(&watch_index, tmp.path()).expect("save should succeed");

        // Load from disk
        let exts_str = watch_index.extensions.join(",");
        let loaded = crate::load_content_index(&watch_index.root, &exts_str, tmp.path())
            .expect("load should return Ok with the saved index");

        // Verify all core fields survived
        assert_eq!(loaded.files.len(), orig_files, "files count mismatch");
        assert_eq!(loaded.index.len(), orig_tokens, "token count mismatch");
        assert_eq!(loaded.total_tokens, watch_index.total_tokens, "total_tokens mismatch");

        // forward should remain None after roundtrip (not used since memory optimization)
        assert!(loaded.forward.is_none(), "forward should remain None after roundtrip");

        // path_to_id should survive serialization
        assert!(loaded.path_to_id.is_some(), "path_to_id should survive roundtrip");
        assert_eq!(loaded.path_to_id.as_ref().unwrap().len(), orig_path_to_id_len,
            "path_to_id entry count mismatch after roundtrip");
    }
}