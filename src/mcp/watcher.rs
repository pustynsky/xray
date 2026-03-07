use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{error, info, warn};

use crate::{clean_path, tokenize, ContentIndex, Posting, DEFAULT_MIN_TOKEN_LEN};
use crate::definitions::{self, DefinitionIndex};

use std::sync::atomic::{AtomicBool, Ordering};

/// Start a file watcher thread that incrementally updates the in-memory index
pub fn start_watcher(
    index: Arc<RwLock<ContentIndex>>,
    def_index: Option<Arc<RwLock<DefinitionIndex>>>,
    dir: PathBuf,
    extensions: Vec<String>,
    debounce_ms: u64,
    index_base: PathBuf,
    content_ready: Arc<AtomicBool>,
    def_ready: Arc<AtomicBool>,
) -> notify::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&dir, RecursiveMode::Recursive)?;

    let dir_str = clean_path(&dir.to_string_lossy());

    info!(dir = %dir_str, debounce_ms, "File watcher started");

    std::thread::spawn(move || {
        let _watcher = watcher; // move watcher into thread to keep it alive

        // ── Reconciliation: catch files added/modified/removed while server was offline ──
        // Watcher is already listening — events during reconciliation are buffered in rx channel.
        // Reset readiness flags during reconciliation so MCP requests get
        // an instant "building" message instead of blocking on the write lock.
        content_ready.store(false, Ordering::Release);
        reconcile_content_index(&index, &dir_str, &extensions);
        content_ready.store(true, Ordering::Release);

        if let Some(ref def_idx) = def_index {
            def_ready.store(false, Ordering::Release);
            match def_idx.write() {
                Ok(mut idx) => {
                    definitions::reconcile_definition_index(&mut idx, &dir_str, &extensions);
                }
                Err(e) => {
                    error!(error = %e, "Failed to acquire def index write lock for reconciliation");
                }
            }
            def_ready.store(true, Ordering::Release);
        }

        let mut dirty_files: HashSet<PathBuf> = HashSet::new();
        let mut removed_files: HashSet<PathBuf> = HashSet::new();
        let mut last_autosave = std::time::Instant::now();
        const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(600); // 10 minutes

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
                        // Check periodic autosave
                        if last_autosave.elapsed() >= AUTOSAVE_INTERVAL {
                            periodic_autosave(&index, &def_index, &index_base);
                            last_autosave = std::time::Instant::now();
                        }
                        continue;
                    }
                    if !process_batch(&index, &def_index, &mut dirty_files, &mut removed_files) {
                        error!("RwLock poisoned, watcher thread exiting to avoid infinite error loop");
                        break;
                    }
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

/// Process a batch of dirty (modified/created) and removed files by updating
/// the content index and definition index.
///
/// This is the core incremental update logic, extracted from the watcher event
/// loop to enable isolated unit testing.
///
/// Uses `batch_purge_files` for O(total_postings) instead of O(N × total_postings)
/// when many files change at once (e.g., git pull with 300+ files).
///
/// Returns `false` if a poisoned RwLock is detected, signaling the caller to exit.
fn process_batch(
    index: &Arc<RwLock<ContentIndex>>,
    def_index: &Option<Arc<RwLock<DefinitionIndex>>>,
    dirty_files: &mut HashSet<PathBuf>,
    removed_files: &mut HashSet<PathBuf>,
) -> bool {
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
    if !update_content_index(index, &removed_clean, &dirty_clean) {
        return false;
    }

    // Update definition index (if available)
    if !update_definition_index(def_index, &removed_clean, &dirty_clean) {
        return false;
    }

    let elapsed_ms = batch_start.elapsed().as_secs_f64() * 1000.0;
    info!(
        updated = update_count,
        removed = remove_count,
        elapsed_ms = format_args!("{:.1}", elapsed_ms),
        "Incremental index update complete"
    );
    true
}

/// Update the content index: purge stale postings, remove deleted files,
/// re-tokenize modified/new files, and shrink oversized collections.
///
/// Returns `false` if the RwLock is poisoned (prior panic), signaling the caller to stop.
fn update_content_index(
    index: &Arc<RwLock<ContentIndex>>,
    removed_clean: &[PathBuf],
    dirty_clean: &[PathBuf],
) -> bool {
    match index.write() {
        Ok(mut idx) => {
            // Collect file_ids of all existing files to purge in one pass
            let mut purge_ids: HashSet<u32> = HashSet::new();
            if let Some(ref p2id) = idx.path_to_id {
                for path in removed_clean {
                    if let Some(&fid) = p2id.get(path) {
                        purge_ids.insert(fid);
                    }
                }
                for path in dirty_clean {
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
            for path in removed_clean {
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
            for path in dirty_clean {
                reindex_file_after_purge(&mut idx, path);
            }

            // Mark trigram index as dirty — will be rebuilt lazily on next substring search
            idx.trigram_dirty = true;

            // Conditionally shrink collections after retain() to release excess capacity.
            // Only shrink when capacity > 2 × len to avoid unnecessary realloc storms.
            shrink_if_oversized(&mut idx);
        }
        Err(e) => {
            error!(error = %e, "Failed to acquire content index write lock (poisoned)");
            return false;
        }
    }
    true
}

/// Update the definition index: remove deleted files, re-parse modified/new files.
///
/// Returns `false` if the RwLock is poisoned (prior panic), signaling the caller to stop.
fn update_definition_index(
    def_index: &Option<Arc<RwLock<DefinitionIndex>>>,
    removed_clean: &[PathBuf],
    dirty_clean: &[PathBuf],
) -> bool {
    if let Some(def_idx) = def_index {
        match def_idx.write() {
            Ok(mut idx) => {
                for path in removed_clean {
                    definitions::remove_file_from_def_index(&mut idx, path);
                }
                for path in dirty_clean {
                    definitions::update_file_definitions(&mut idx, path);
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to acquire definition index write lock (poisoned)");
                return false;
            }
        }
    }
    true
}

/// Conditionally shrink collections after retain() to release excess capacity.
/// Only shrinks when capacity > 2 × len to avoid unnecessary realloc storms.
fn shrink_if_oversized(idx: &mut ContentIndex) {
    for postings in idx.index.values_mut() {
        if postings.capacity() > postings.len() * 2 {
            postings.shrink_to_fit();
        }
    }
    if idx.index.capacity() > idx.index.len() * 2 {
        idx.index.shrink_to_fit();
    }
    if let Some(ref mut p2id) = idx.path_to_id
        && p2id.capacity() > p2id.len() * 2 {
            p2id.shrink_to_fit();
        }
}

/// Periodically save in-memory indexes to disk to protect against data loss
/// from forced process termination (e.g., VS Code killing the MCP server).
///
/// Takes READ locks only — MCP queries are NOT blocked during save.
/// Watcher incremental updates (which need write locks) will be briefly delayed.
fn periodic_autosave(
    index: &Arc<RwLock<ContentIndex>>,
    def_index: &Option<Arc<RwLock<DefinitionIndex>>>,
    index_base: &std::path::Path,
) {
    let start = std::time::Instant::now();
    let mut saved = Vec::new();

    // Save content index
    match index.read() {
        Ok(idx) => {
            if !idx.files.is_empty() {
                if let Err(e) = crate::save_content_index(&idx, index_base) {
                    warn!(error = %e, "Periodic autosave: failed to save content index");
                } else {
                    saved.push(format!("content({} files)", idx.files.len()));
                }
            }
        }
        Err(e) => warn!(error = %e, "Periodic autosave: failed to read content index"),
    }

    // Save definition index
    if let Some(def_idx) = def_index {
        match def_idx.read() {
            Ok(idx) => {
                if !idx.files.is_empty() {
                    if let Err(e) = crate::definitions::save_definition_index(&idx, index_base) {
                        warn!(error = %e, "Periodic autosave: failed to save definition index");
                    } else {
                        saved.push(format!("def({} defs)", idx.definitions.len()));
                    }
                }
            }
            Err(e) => warn!(error = %e, "Periodic autosave: failed to read definition index"),
        }
    }

    if !saved.is_empty() {
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        info!(
            elapsed_ms = format_args!("{:.1}", elapsed_ms),
            saved = %saved.join(", "),
            "Periodic autosave complete"
        );
    }
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
#[path = "watcher_tests.rs"]
mod tests;
