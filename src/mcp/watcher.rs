use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use notify::event::ModifyKind;
use tracing::{debug, error, info, warn};

use crate::{canonicalize_or_warn, clean_path, tokenize, ContentIndex, FileIndex, Posting, DEFAULT_MIN_TOKEN_LEN};
use crate::definitions::{self, DefinitionIndex};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Lightweight, lock-free counters describing what the file watcher has
/// observed since startup. Shared via `Arc` between the watcher thread and
/// `xray_info` so operators can diagnose missed events (`notify` is
/// best-effort on every platform — see
/// `docs/bug-reports/bug-2026-04-21-watcher-misses-new-files-both-indexes.md`).
///
/// All fields use `Ordering::Relaxed` because they are pure observability
/// signals — no other data hangs off them.
#[derive(Debug, Default)]
pub struct WatcherStats {
    /// Total `Ok(event)` notifications pulled from the `notify` channel.
    pub events_total: AtomicU64,
    /// Subset of `events_total` where `event.paths` was empty. Some
    /// `notify` backends emit such events (e.g. `EventKind::Create(Any)`
    /// without a path), and they cannot trigger any index invalidation —
    /// a non-zero count here is a strong hint that index drift is caused
    /// by upstream backend behaviour, not by our event-loop logic.
    pub events_empty_paths: AtomicU64,
    /// `Err(notify_error)` notifications pulled from the channel.
    pub events_errors: AtomicU64,
    /// Number of times [`periodic_rescan_once`] detected drift between
    /// the on-disk filesystem state and at least one in-memory index.
    /// Non-zero in production means the `notify` event stream missed
    /// (or never received) a filesystem event — the rescan was the
    /// fail-safe that recovered. Phase 2 of the periodic-rescan rollout.
    pub periodic_rescan_drift_events: AtomicU64,
    /// Total number of [`periodic_rescan_once`] invocations completed.
    /// Useful for sanity-checking the rescan thread is alive when
    /// `periodic_rescan_drift_events` stays at zero.
    pub periodic_rescan_total: AtomicU64,
}

impl WatcherStats {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Start a file watcher thread that incrementally updates the in-memory index
/// Returns `true` if the given event kind should invalidate the file-list index.
/// Covers create, remove, and rename (cross-platform: Linux/inotify emits
/// `Modify(Name(_))` for renames, Windows emits Remove+Create pairs).
pub(crate) fn should_invalidate_file_index(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Name(_))
            | EventKind::Modify(ModifyKind::Any)
    )
}

/// Outcome of [`wait_for_indexes_ready`]. Public to the crate so watcher
/// thread and tests can share the same decision tree.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    /// Both `content_ready` and `def_ready` observed `true` within the cap.
    Ready,
    /// `watcher_generation` changed away from `my_generation` — caller
    /// should exit without reconciling.
    GenerationChanged,
    /// Hard cap reached before both flags flipped; caller should proceed
    /// to reconciliation (partial index is safer than silently skipping).
    TimedOut,
}

/// Wait for the initial index build to complete before reconciliation
/// (MAJOR-8). Polls the two `AtomicBool` flags every `poll` until both
/// are `true`, the generation counter changes, or `cap` elapses.
///
/// Extracted for testability — the watcher thread invokes it with a
/// 50 ms poll / 300 s cap; tests drive it with sub-millisecond poll and
/// cap values.
pub(crate) fn wait_for_indexes_ready(
    content_ready: &AtomicBool,
    def_ready: &AtomicBool,
    watcher_generation: &AtomicU64,
    my_generation: u64,
    poll: Duration,
    cap: Duration,
) -> WaitOutcome {
    let start = Instant::now();
    loop {
        if content_ready.load(Ordering::Acquire) && def_ready.load(Ordering::Acquire) {
            return WaitOutcome::Ready;
        }
        if watcher_generation.load(Ordering::Acquire) != my_generation {
            return WaitOutcome::GenerationChanged;
        }
        if start.elapsed() >= cap {
            return WaitOutcome::TimedOut;
        }
        std::thread::sleep(poll);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn start_watcher(
    index: Arc<RwLock<ContentIndex>>,
    def_index: Option<Arc<RwLock<DefinitionIndex>>>,
    dir: PathBuf,
    extensions: Vec<String>,
    debounce_ms: u64,
    index_base: PathBuf,
    content_ready: Arc<AtomicBool>,
    def_ready: Arc<AtomicBool>,
    file_index_dirty: Arc<AtomicBool>,
    watcher_generation: Arc<AtomicU64>,
    my_generation: u64,
    stats: Arc<WatcherStats>,
) -> notify::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&dir, RecursiveMode::Recursive)?;

    let dir_str = clean_path(&dir.to_string_lossy());

    info!(dir = %dir_str, debounce_ms, "File watcher started");

    std::thread::spawn(move || {
        let _watcher = watcher; // move watcher into thread to keep it alive

        // Check generation before starting reconciliation
        if watcher_generation.load(Ordering::Acquire) != my_generation {
            info!(my_generation, "Watcher generation changed before reconciliation, exiting");
            return;
        }

        // ── Wait for the initial index build (MAJOR-8) ──────────────────
        // `cmd_serve` spawns background threads that build the content and
        // definition indexes in parallel with the watcher. If reconciliation
        // starts before those threads finish their final swap, the watcher
        // would either reconcile against an empty index (wasting work that
        // the background swap then overwrites) or lose events that arrived
        // during the build window. Wait for both ready flags with a small
        // poll interval; respect generation changes and a hard cap so a
        // stuck builder cannot keep the watcher hung forever.
        const READY_POLL: Duration = Duration::from_millis(50);
        // Hard cap well above a realistic cold-start build (~30s for
        // large trees). After this, log and proceed: reconciliation on
        // a partial index is still safer than silently skipping events.
        const READY_WAIT_CAP: Duration = Duration::from_secs(300);
        let wait_start = Instant::now();
        match wait_for_indexes_ready(
            &content_ready,
            &def_ready,
            &watcher_generation,
            my_generation,
            READY_POLL,
            READY_WAIT_CAP,
        ) {
            WaitOutcome::Ready => {
                let waited = wait_start.elapsed();
                if waited > READY_POLL {
                    info!(
                        waited_ms = waited.as_millis() as u64,
                        "Watcher waited for initial index build before reconciliation"
                    );
                }
            }
            WaitOutcome::GenerationChanged => {
                info!(my_generation, "Watcher generation changed during startup wait, exiting");
                return;
            }
            WaitOutcome::TimedOut => {
                warn!(
                    waited_s = wait_start.elapsed().as_secs(),
                    content_ready = content_ready.load(Ordering::Acquire),
                    def_ready = def_ready.load(Ordering::Acquire),
                    "Watcher proceeded to reconciliation before initial index build completed (hard cap reached)"
                );
            }
        }

        // ── Reconciliation: catch files added/modified/removed while server was offline ──
        // Watcher is already listening — events during reconciliation are buffered in rx channel.
        // Non-blocking: MCP requests work on old data during reconciliation.
        // Only the brief write lock in Phase 4 blocks readers.
        reconcile_content_index(&index, &dir_str, &extensions);
        if let Some(ref def_idx) = def_index {
            // Non-blocking reconciliation: parse files OUTSIDE the lock, apply INSIDE.
            // def_ready stays true — MCP requests work on old data during parsing.
            definitions::reconcile_definition_index_nonblocking(
                def_idx, &dir_str, &extensions
            );
        }

        let mut batch_start: Option<Instant> = None;
        const MAX_ACCUMULATE: Duration = Duration::from_secs(3);

        let mut dirty_files: HashSet<PathBuf> = HashSet::new();
        let mut removed_files: HashSet<PathBuf> = HashSet::new();
        let mut last_autosave = std::time::Instant::now();
        const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(600); // 10 minutes

        loop {
            match rx.recv_timeout(Duration::from_millis(debounce_ms)) {
                Ok(Ok(event)) => {
                    // Observability (Phase 0 of periodic-rescan rollout):
                    // every event we *do* receive is counted, and events with
                    // an empty `paths` vec are flagged separately because
                    // they cannot drive any index invalidation downstream.
                    stats.events_total.fetch_add(1, Ordering::Relaxed);
                    if event.paths.is_empty() {
                        stats.events_empty_paths.fetch_add(1, Ordering::Relaxed);
                        debug!(
                            kind = ?event.kind,
                            "watcher: event with empty paths (cannot invalidate index)"
                        );
                    } else {
                        debug!(
                            kind = ?event.kind,
                            paths_len = event.paths.len(),
                            "watcher: event received"
                        );
                    }
                    // Collect changed files
                    for path in &event.paths {
                        // Skip .git directory — git operations generate massive event floods
                        // and .git/config matches the "config" extension filter
                        if is_inside_git_dir(path) {
                            continue;
                        }
                        // Invalidate file-list index on ANY create/delete/rename,
                        // BEFORE the extension filter. FileIndex indexes ALL files
                        // (not just --ext), so changes to .json, .yaml, etc. must
                        // also trigger a rebuild.
                        if should_invalidate_file_index(&event.kind) {
                            file_index_dirty.store(true, Ordering::Relaxed);
                        }
                        if !matches_extensions(path, &extensions) {
                            continue;
                        }
                        match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) => {
                                removed_files.remove(path);
                                dirty_files.insert(path.clone());
                                if batch_start.is_none() {
                                    batch_start = Some(Instant::now());
                                }
                            }
                            EventKind::Remove(_) => {
                                dirty_files.remove(path);
                                removed_files.insert(path.clone());
                    stats.events_errors.fetch_add(1, Ordering::Relaxed);
                                if batch_start.is_none() {
                                    batch_start = Some(Instant::now());
                                }
                            }
                            _ => {}
                        }
                    }
                    // Force flush if accumulating too long (prevents debounce starvation)
                    if let Some(start) = batch_start
                        && start.elapsed() >= MAX_ACCUMULATE {
                            if !process_batch(&index, &def_index, &mut dirty_files, &mut removed_files) {
                                error!("RwLock poisoned, watcher thread exiting");
                                break;
                            }
                            batch_start = None;
                        }
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "File watcher error");
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Check generation on each timeout (watcher restart)
                    if watcher_generation.load(Ordering::Acquire) != my_generation {
                        info!(dir = %dir_str, my_generation, "Watcher generation changed, exiting");
                        break;
                    }
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
                    batch_start = None;
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

/// Statistics returned by `reindex_paths_sync` describing the work done.
/// All counts are post-filter (after `--ext` and `.git/` exclusions).
#[derive(Debug, Clone, Default)]
pub(crate) struct ReindexStats {
    /// Number of dirty files that passed filters and were content-indexed.
    pub content_updated: usize,
    /// Number of dirty files that passed filters and were def-indexed (0 if def_index is None).
    pub def_updated: usize,
    /// Number of dirty files skipped due to filters (`--ext` mismatch or inside `.git/`).
    pub skipped_filtered: usize,
    /// Wall-clock time of the sync reindex in milliseconds.
    pub elapsed_ms: f64,
    /// True iff content index lock was poisoned — caller should report a warning.
    pub content_lock_poisoned: bool,
    /// True iff def index lock was poisoned — caller should report a warning.
    pub def_lock_poisoned: bool,
}

/// Synchronously reindex a small set of paths (typically 1–20 files), bypassing
/// the FS watcher debounce window. Used by `xray_edit` to ensure subsequent
/// `xray_grep`/`xray_definitions` queries see the new content immediately.
///
/// Applies the SAME filters as the watcher (`matches_extensions`, `is_inside_git_dir`)
/// for symmetry — files outside `--ext` or inside `.git/` are skipped (counted in
/// `skipped_filtered`).
///
/// Reuses the watcher's non-blocking implementation (`update_content_index` +
/// `update_definition_index`): tokenize/parse OUTSIDE the lock, apply INSIDE.
/// Write-lock window is < 1 ms per file.
///
/// Idempotent — safe to call concurrently with the watcher (which may pick up
/// the FS event later); double-update produces an identical index state.
pub(crate) fn reindex_paths_sync(
    index: &Arc<RwLock<ContentIndex>>,
    def_index: &Option<Arc<RwLock<DefinitionIndex>>>,
    dirty: &[PathBuf],
    removed: &[PathBuf],
    extensions: &[String],
) -> ReindexStats {
    let start = std::time::Instant::now();
    let mut stats = ReindexStats::default();

    // Apply the SAME filters as the watcher event loop ([watcher.rs:91-103]):
    //   1. Skip paths inside .git/ (git operations generate massive event floods).
    //   2. Skip paths whose extension is not in `--ext`.
    let mut dirty_clean: Vec<PathBuf> = Vec::with_capacity(dirty.len());
    for path in dirty {
        if is_inside_git_dir(path) || !matches_extensions(path, extensions) {
            stats.skipped_filtered += 1;
            continue;
        }
        dirty_clean.push(PathBuf::from(clean_path(&path.to_string_lossy())));
    }
    let mut removed_clean: Vec<PathBuf> = Vec::with_capacity(removed.len());
    for path in removed {
        if is_inside_git_dir(path) || !matches_extensions(path, extensions) {
            stats.skipped_filtered += 1;
            continue;
        }
        removed_clean.push(PathBuf::from(clean_path(&path.to_string_lossy())));
    }

    if dirty_clean.is_empty() && removed_clean.is_empty() {
        stats.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        return stats;
    }

    // Apply via the same non-blocking helpers used by the watcher.
    if !update_content_index(index, &removed_clean, &dirty_clean) {
        stats.content_lock_poisoned = true;
    } else {
        stats.content_updated = dirty_clean.len();
    }

    if def_index.is_some() {
        if !update_definition_index(def_index, &removed_clean, &dirty_clean) {
            stats.def_lock_poisoned = true;
        } else {
            stats.def_updated = dirty_clean.len();
        }
    }

    stats.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    stats
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

    // MINOR-10: drain-before-apply is intentional and MUST stay in this order.
    //
    // SAFETY: we `drain()` the input `HashSet`s into owned `Vec`s BEFORE calling
    // `update_content_index` / `update_definition_index`. Rationale:
    //   * Both `update_*` helpers may return `false` on a poisoned lock; the
    //     caller of `process_batch` must NOT see the same file event again
    //     in the next batch — draining ensures the pending sets are empty
    //     even on early return.
    //   * Applying an index update is the side-effect the watcher performs
    //     "at most once" per debounced batch. A retry after partial apply
    //     would double-index postings (we rely on the FS watcher, not the
    //     pending set, to redeliver missed events).
    //   * Path normalization (`clean_path`) happens once per path here
    //     instead of inside every hot loop in the `update_*` helpers.

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
/// **Non-blocking:** Tokenizes all dirty files OUTSIDE the lock (Phase 1),
/// determines purge IDs under a brief READ lock (Phase 2), then applies
/// purge + insertions under WRITE lock (Phase 3).
/// Write lock time: from `500ms + N × 5ms` → `500ms + N × 0.1ms`.
///
/// Returns `false` if the RwLock is poisoned (prior panic), signaling the caller to stop.
fn update_content_index(
    index: &Arc<RwLock<ContentIndex>>,
    removed_clean: &[PathBuf],
    dirty_clean: &[PathBuf],
) -> bool {
    // ── Phase 1: Tokenize all dirty files OUTSIDE the lock (~5ms × N) ──
    // During this phase, MCP requests work normally on the current index data.
    let tokenized: Vec<TokenizedFileResult> = dirty_clean.iter()
        .filter_map(|path| tokenize_file_standalone(path))
        .collect();

    // ── Phase 2: Determine purge IDs (READ LOCK — instant) ──
    let purge_ids: HashSet<u32> = match index.read() {
        Ok(idx) => {
            let mut ids = HashSet::new();
            if let Some(ref p2id) = idx.path_to_id {
                for path in removed_clean {
                    if let Some(&fid) = p2id.get(path) {
                        ids.insert(fid);
                    }
                }
                for path in dirty_clean {
                    if let Some(&fid) = p2id.get(path) {
                        ids.insert(fid);
                    }
                }
            }
            ids
        }
        Err(e) => {
            error!(error = %e, "Failed to acquire content index read lock (poisoned)");
            return false;
        }
    };
    // READ lock released here

    // ── Phase 3: Apply under WRITE LOCK (~500ms purge + ~0.1ms × N insert) ──
    match index.write() {
        Ok(mut idx) => {
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

            // Apply pre-tokenized results (just insert pre-computed postings)
            for result in tokenized {
                apply_tokenized_file(&mut idx, result);
            }

            // Mark trigram index as dirty — will be rebuilt lazily on next substring search
            idx.trigram_dirty = true;

            // Update created_at — watcher detects subsequent changes via fsnotify, so now() is safe
            idx.created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::ZERO)
                .as_secs();

            // Conditionally shrink collections after retain() to release excess capacity.
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
/// **Non-blocking:** Parses all dirty files OUTSIDE the write lock (Phase 1),
/// then applies results + removals under a brief write lock (Phase 2).
/// Write lock time: from `N × 30ms` → `N × 0.1ms`.
///
/// Returns `false` if the RwLock is poisoned (prior panic), signaling the caller to stop.
fn update_definition_index(
    def_index: &Option<Arc<RwLock<DefinitionIndex>>>,
    removed_clean: &[PathBuf],
    dirty_clean: &[PathBuf],
) -> bool {
    let Some(def_idx) = def_index else { return true };

    // ── Phase 1: Parse all dirty files OUTSIDE the lock (~30ms × N) ──
    // During this phase, MCP requests work normally on the current index data.
    let parsed: Vec<definitions::ParsedFileResult> = dirty_clean.iter()
        .enumerate()
        .filter_map(|(i, path)| definitions::parse_file_standalone(path, i as u32))
        .collect();

    // Track which dirty paths produced a ParsedFileResult
    let parsed_paths: HashSet<PathBuf> = parsed.iter().map(|r| r.path.clone()).collect();

    // ── Phase 2: Apply under brief WRITE LOCK (~0.1ms × N + removals) ──
    match def_idx.write() {
        Ok(mut idx) => {
            // Remove deleted files
            for path in removed_clean {
                definitions::remove_file_from_def_index(&mut idx, path);
            }
            // Apply parsed results
            for result in parsed {
                definitions::apply_parsed_result(&mut idx, result);
            }
            // Clean up dirty files that didn't produce a ParsedFileResult
            // (e.g., read error, unsupported extension). Remove stale definitions.
            for path in dirty_clean {
                if !parsed_paths.contains(path)
                    && let Some(&fid) = idx.path_to_id.get(path) {
                        definitions::remove_file_definitions(&mut idx, fid);
                    }
            }

            // Update created_at — watcher detects subsequent changes via fsnotify, so now() is safe
            idx.created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::ZERO)
                .as_secs();
        }
        Err(e) => {
            error!(error = %e, "Failed to acquire definition index write lock (poisoned)");
            return false;
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

/// Result of tokenizing a file outside the ContentIndex lock.
/// Contains pre-computed token → line_numbers map ready to be applied.
struct TokenizedFileResult {
    path: PathBuf,
    tokens: HashMap<String, Vec<u32>>,  // token → line numbers
    total_tokens: u32,
}

/// Tokenize a file WITHOUT any lock on ContentIndex.
///
/// Reads the file (lossy UTF-8), tokenizes, and returns the result.
/// Used by non-blocking update paths to move I/O outside the write lock.
fn tokenize_file_standalone(path: &Path) -> Option<TokenizedFileResult> {
    let (content, _was_lossy) = crate::read_file_lossy(path).ok()?;
    let mut tokens: HashMap<String, Vec<u32>> = HashMap::new();
    let mut total: u32 = 0;
    for (line_num, line) in content.lines().enumerate() {
        for token in tokenize(line, DEFAULT_MIN_TOKEN_LEN) {
            total += 1;
            tokens.entry(token).or_default().push((line_num + 1) as u32);
        }
    }
    Some(TokenizedFileResult {
        path: path.to_path_buf(),
        tokens,
        total_tokens: total,
    })
}

/// Apply a pre-tokenized file to the ContentIndex under write lock.
///
/// For existing files (already in path_to_id), assumes old postings have been
/// purged via `batch_purge_files`. For new files, assigns a new file_id.
fn apply_tokenized_file(index: &mut ContentIndex, result: TokenizedFileResult) {
    let file_id = if let Some(ref mut p2id) = index.path_to_id {
        if let Some(&fid) = p2id.get(&result.path) {
            fid  // existing file (already purged via batch_purge)
        } else {
            // new file — assign new file_id
            let fid = index.files.len() as u32;
            index.files.push(result.path.to_string_lossy().to_string());
            p2id.insert(result.path, fid);
            index.file_token_counts.push(0); // will be updated below
            fid
        }
    } else {
        return;
    };

    // Insert pre-computed postings into inverted index
    for (token, lines) in result.tokens {
        index.total_tokens += lines.len() as u64;
        index.index.entry(token)
            .or_default()
            .push(Posting { file_id, lines });
    }

    // Update file token count
    if (file_id as usize) < index.file_token_counts.len() {
        index.file_token_counts[file_id as usize] = result.total_tokens;
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
pub(crate) fn is_inside_git_dir(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == ".git")
}

pub(crate) fn matches_extensions(path: &Path, extensions: &[String]) -> bool {
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
///
/// Used by tests for verifying single-file update behavior.
/// Production code uses the non-blocking `tokenize_file_standalone` + `apply_tokenized_file` path.
#[cfg(test)]
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


/// Remove all postings for a given file_id from the inverted index.
/// This is a brute-force O(total_tokens) scan that replaces the forward index lookup.
/// Typically takes ~50-100ms for 400K tokens, which is acceptable for single-file events.
///
/// For batch operations (git pull, git checkout), prefer `batch_purge_files` which
/// removes multiple file_ids in a single pass — O(total_postings) regardless of N.
///
/// Used by tests and by `update_file_in_index` / `remove_file_from_index`.
#[cfg(test)]
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
///
/// Used by tests. Production code uses batch_purge-based removal.
#[cfg(test)]
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

/// Snapshot of the on-disk state of a workspace directory, captured by a
/// single [`scan_dir_state`] walk. Two views over the same set of files:
///
/// * `all_files` — every regular file under `dir`, with `.git/` excluded
///   (matches the file-list / `xray_fast` view).
/// * `ext_matched` — strict subset of `all_files` whose extension is in
///   `extensions` (matches the content / definition-index view).
///
/// Captured once so callers (`reconcile_content_index` today, the upcoming
/// `periodic_rescan_once` in Phase 2) avoid two filesystem walks per
/// reconciliation cycle.
#[derive(Debug, Default, Clone)]
pub(crate) struct DirState {
    #[allow(dead_code)] // consumed by `periodic_rescan_once`; binary callers land in Phase 3
    pub all_files: HashMap<PathBuf, SystemTime>,
    pub ext_matched: HashMap<PathBuf, SystemTime>,
}

/// Walk `dir` once and classify every regular file into the two
/// [`DirState`] views.
///
/// Walker config is held centralised here so the watcher's
/// reconciliation paths and the upcoming periodic rescan share a single
/// source of truth: `follow_links(true).hidden(false).git_ignore(true)
/// .git_exclude(false)`. `.git/` is excluded explicitly via
/// [`is_inside_git_dir`] because `WalkBuilder` does not skip it by default.
///
/// Path keys are normalised through [`clean_path`] so they can be
/// compared 1:1 against `path_to_id` keys (which use the same
/// normalisation).
///
/// Errors from individual `WalkBuilder` entries are silently skipped to
/// preserve the previous behaviour of `reconcile_content_index` —
/// reconciliation must not abort on a single I/O glitch.
pub(crate) fn scan_dir_state(dir: &str, extensions: &[String]) -> DirState {
    let dir_path = canonicalize_or_warn(dir);

    let mut walker = WalkBuilder::new(&dir_path);
    walker.follow_links(true).hidden(false).git_ignore(true).git_exclude(false);

    let mut all_files: HashMap<PathBuf, SystemTime> = HashMap::new();
    let mut ext_matched: HashMap<PathBuf, SystemTime> = HashMap::new();

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

        let mtime = entry.metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(UNIX_EPOCH);
        let clean = PathBuf::from(clean_path(&path.to_string_lossy()));

        let ext_match = path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)));

        if ext_match {
            ext_matched.insert(clean.clone(), mtime);
        }
        all_files.insert(clean, mtime);
    }

    DirState { all_files, ext_matched }
}

/// Outcome of a single [`periodic_rescan_once`] call. Returned for tests
/// and for `xray_info` / log emission. All counts are post-filter
/// (`.git/` excluded, extension-matching where applicable).
#[derive(Debug, Default, Clone)]
#[allow(dead_code)] // fields read by the rescan thread in Phase 3 (CLI flags + interval log)
pub(crate) struct RescanOutcome {
    /// Wall-clock time of the rescan in milliseconds.
    pub elapsed_ms: f64,
    /// `path_to_id` entries missing from disk for `--ext`-matching files.
    pub content_removed: usize,
    /// `--ext`-matching files on disk missing from `path_to_id`.
    pub content_added: usize,
    /// `--ext`-matching files present in both but with `mtime` newer
    /// than `index.created_at - 2s`.
    pub content_modified: usize,
    /// Files in the file-list view that disappeared from disk.
    pub file_index_removed: usize,
    /// Files on disk missing from the file-list view.
    pub file_index_added: usize,
    /// `true` iff at least one of the above counters is non-zero.
    pub drift_detected: bool,
}

/// Compare the on-disk state in `state.ext_matched` against the
/// in-memory `ContentIndex` and return drift counts.
///
/// Pure read-side helper — no mutation. The 2-second `created_at`
/// safety margin matches `reconcile_content_index` so a fresh write
/// landing within the same second as the previous reconciliation is
/// still detected.
#[allow(dead_code)] // wired into binary call path in Phase 3 (rescan thread)
fn compute_content_drift(
    index: &Arc<RwLock<ContentIndex>>,
    ext_matched: &HashMap<PathBuf, SystemTime>,
) -> (usize, usize, usize) {
    let idx = match index.read() {
        Ok(g) => g,
        Err(e) => {
            error!(error = %e, "periodic_rescan: content index read lock poisoned");
            return (0, 0, 0);
        }
    };
    let Some(ref p2id) = idx.path_to_id else {
        // Watcher disabled or index built without path_to_id — nothing
        // to compare against, so report zero drift (the rescan thread
        // will not be running in that mode anyway).
        return (0, 0, 0);
    };
    let threshold = UNIX_EPOCH + Duration::from_secs(idx.created_at.saturating_sub(2));
    let mut added = 0usize;
    let mut modified = 0usize;
    for (path, mtime) in ext_matched {
        if let Some(_fid) = p2id.get(path) {
            if *mtime > threshold {
                modified += 1;
            }
        } else {
            added += 1;
        }
    }
    let mut removed = 0usize;
    for path in p2id.keys() {
        if !ext_matched.contains_key(path) {
            removed += 1;
        }
    }
    (added, removed, modified)
}

/// Compare the on-disk state in `state.all_files` against the in-memory
/// `FileIndex` and return `(added, removed)` counts.
///
/// If the `FileIndex` slot is `None` (lazy: not yet built — happens
/// before the first `xray_fast` call), drift is reported as
/// `(all_files.len(), 0)` so the rescan thread sets `file_index_dirty`
/// and forces a build on the next request. This matches the watcher
/// startup contract (`file_index_dirty = true` initially).
#[allow(dead_code)] // wired into binary call path in Phase 3 (rescan thread)
fn compute_file_index_drift(
    file_index: &Arc<RwLock<Option<FileIndex>>>,
    all_files: &HashMap<PathBuf, SystemTime>,
) -> (usize, usize) {
    let guard = match file_index.read() {
        Ok(g) => g,
        Err(e) => {
            error!(error = %e, "periodic_rescan: file index read lock poisoned");
            return (0, 0);
        }
    };
    let Some(ref fi) = *guard else {
        // Not yet built — treat every disk file as "added" so the
        // caller marks the index dirty.
        return (all_files.len(), 0);
    };
    // FileIndex stores forward-slash relative paths inside `entries`,
    // and `all_files` keys are absolute clean_path-normalised. Compare
    // by suffix-matching `path` against the absolute key — robust to
    // `root` prefix differences.
    let in_index: HashSet<&str> = fi.entries.iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.path.as_str())
        .collect();

    let mut added = 0usize;
    let on_disk_set: HashSet<String> = all_files.keys()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    for disk in &on_disk_set {
        if !in_index.iter().any(|idx_path| disk.ends_with(idx_path)) {
            added += 1;
        }
    }
    let mut removed = 0usize;
    for idx_path in &in_index {
        if !on_disk_set.iter().any(|disk| disk.ends_with(*idx_path)) {
            removed += 1;
        }
    }
    (added, removed)
}

/// Single rescan tick of the periodic-rescan fail-safe (Phase 2 of the
/// rollout in `docs/todo_approved_2026-04-21_watcher-periodic-rescan.md`).
///
/// Walks the workspace once, compares the on-disk state against the
/// three in-memory indexes, and — when any drift is detected —
/// (a) sets `file_index_dirty` so the next `xray_fast` rebuilds the
/// file-list view, and (b) delegates to the existing reconcilers to
/// fix the content and definition indexes.
///
/// **Idempotent and non-blocking.** Safe to call concurrently with the
/// `notify` event-loop; the worst case is double work, which produces
/// the same final index state.
///
/// Counter semantics:
/// * `periodic_rescan_total` bumps on every call.
/// * `periodic_rescan_drift_events` bumps only when at least one
///   index was out of sync — operators reading `xray_info` can
///   distinguish "rescan ran, all good" from "rescan recovered a
///   missed event" at a glance.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)] // wired into the watcher thread in Phase 3
pub(crate) fn periodic_rescan_once(
    index: &Arc<RwLock<ContentIndex>>,
    def_index: &Option<Arc<RwLock<DefinitionIndex>>>,
    file_index: &Arc<RwLock<Option<FileIndex>>>,
    file_index_dirty: &Arc<AtomicBool>,
    dir: &str,
    extensions: &[String],
    stats: &Arc<WatcherStats>,
) -> RescanOutcome {
    let start = Instant::now();
    stats.periodic_rescan_total.fetch_add(1, Ordering::Relaxed);

    let state = scan_dir_state(dir, extensions);
    let (content_added, content_removed, content_modified) =
        compute_content_drift(index, &state.ext_matched);
    let (file_index_added, file_index_removed) =
        compute_file_index_drift(file_index, &state.all_files);

    let content_drift = content_added + content_removed + content_modified > 0;
    let file_drift = file_index_added + file_index_removed > 0;
    let drift_detected = content_drift || file_drift;

    if file_drift {
        file_index_dirty.store(true, Ordering::Relaxed);
    }
    if content_drift {
        // Delegate to the existing reconcilers. They each perform
        // their own walk today (acceptable on a 5-min cadence;
        // collapsing to a single walk is a follow-up). Bailing out
        // when nothing changed is their internal fast path.
        reconcile_content_index(index, dir, extensions);
        if let Some(di) = def_index {
            definitions::reconcile_definition_index_nonblocking(di, dir, extensions);
        }
    }

    if drift_detected {
        stats.periodic_rescan_drift_events.fetch_add(1, Ordering::Relaxed);
        warn!(
            content_added,
            content_removed,
            content_modified,
            file_index_added,
            file_index_removed,
            "periodic rescan detected drift — notify event stream missed at least one event"
        );
    } else {
        debug!(
            ext_files = state.ext_matched.len(),
            all_files = state.all_files.len(),
            "periodic rescan: no drift"
        );
    }

    RescanOutcome {
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        content_added,
        content_removed,
        content_modified,
        file_index_added,
        file_index_removed,
        drift_detected,
    }
}

/// Reconcile content index with filesystem after loading from disk cache.
///
/// Walks the filesystem and compares with the in-memory index to find:
/// - **Added** files: exist on disk but not in `path_to_id` → tokenize and add
/// - **Modified** files: exist in both but `mtime > index.created_at` → re-tokenize
/// - **Deleted** files: exist in `path_to_id` but not on disk → remove
///
/// **Non-blocking:** Uses a 4-phase pattern:
/// - Phase 1: Walk filesystem (NO lock)
/// - Phase 2: Determine changes (READ lock — instant)
/// - Phase 3: Tokenize all new/modified files (NO lock)
/// - Phase 4: batch_purge + apply (WRITE lock — brief)
///
/// Uses a 2-second safety margin on `created_at` to handle clock precision.
fn reconcile_content_index(
    index: &Arc<RwLock<ContentIndex>>,
    dir: &str,
    extensions: &[String],
) {
    let start = std::time::Instant::now();
    // Capture walk start time for created_at update (not now() at end — avoids race condition
    // where files modified during tokenization phase would be missed by next reconciliation)
    let walk_start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_secs();

    // ── Phase 1: Walk filesystem (NO LOCK) ──
    // Single shared walk — `disk_files` here is the ext-matched view.
    // The full `all_files` view is intentionally unused at this call site
    // (FileIndex is reconciled elsewhere); Phase 2 (`periodic_rescan_once`)
    // will consume both views from the same `DirState` snapshot.
    let dir_state = scan_dir_state(dir, extensions);
    let disk_files = &dir_state.ext_matched;

    let scanned = disk_files.len();

    // ── Phase 2: Determine changes (READ LOCK — instant) ──
    let (to_tokenize, to_remove, purge_ids, added, modified) = match index.read() {
        Ok(idx) => {
            let threshold = UNIX_EPOCH + Duration::from_secs(idx.created_at.saturating_sub(2));

            let mut to_tokenize: Vec<PathBuf> = Vec::new();
            let mut to_remove: Vec<PathBuf> = Vec::new();
            let mut purge_ids: HashSet<u32> = HashSet::new();
            let mut added = 0usize;
            let mut modified = 0usize;

            if let Some(ref p2id) = idx.path_to_id {
                // Check for new and modified files
                for (path, mtime) in disk_files {
                    if let Some(&fid) = p2id.get(path) {
                        // Existing file — check if modified
                        if *mtime > threshold {
                            to_tokenize.push(path.clone());
                            purge_ids.insert(fid);
                            modified += 1;
                        }
                    } else {
                        // New file
                        to_tokenize.push(path.clone());
                        added += 1;
                    }
                }

                // Check for deleted files
                for (path, &fid) in p2id.iter() {
                    if !disk_files.contains_key(path) {
                        to_remove.push(path.clone());
                        purge_ids.insert(fid);
                    }
                }
            }

            (to_tokenize, to_remove, purge_ids, added, modified)
        }
        Err(e) => {
            error!(error = %e, "Failed to read content index for reconciliation");
            return;
        }
    };
    // READ lock released here

    let removed = to_remove.len();

    if to_tokenize.is_empty() && to_remove.is_empty() {
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        info!(
            scanned,
            elapsed_ms = format_args!("{:.1}", elapsed_ms),
            "Content index reconciliation: all files up to date"
        );
        return;
    }

    // ── Phase 3: Tokenize all new/modified files (NO LOCK) ──
    // During this phase, MCP requests work normally on the old index data.
    let tokenized: Vec<TokenizedFileResult> = to_tokenize.iter()
        .filter_map(|path| tokenize_file_standalone(path))
        .collect();

    // ── Phase 4: Apply under WRITE LOCK (~500ms purge + ~0.1ms × N insert) ──
    match index.write() {
        Ok(mut idx) => {
            // Batch purge stale postings
            if !purge_ids.is_empty() {
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
            for path in &to_remove {
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

            // Apply pre-tokenized results
            for result in tokenized {
                apply_tokenized_file(&mut idx, result);
            }

            // Mark trigram as dirty and update created_at if anything changed
            if added > 0 || modified > 0 || removed > 0 {
                idx.created_at = walk_start;
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
                    "Content index reconciliation complete (non-blocking)"
                );
            } else {
                info!(
                    scanned,
                    elapsed_ms = format_args!("{:.1}", elapsed_ms),
                    "Content index reconciliation: all files up to date"
                );
            }

            crate::index::log_memory(&format!(
                "watcher: content reconciliation non-blocking (scanned={}, added={}, modified={}, removed={}, {:.0}ms)",
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
