use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
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
///
/// MCP-WCH-001: previously this only matched `Create`, `Remove`, `Modify(Name)`,
/// and `Modify(Any)`. notify-rs on Windows (ReadDirectoryChangesW) and on macOS
/// (FSEvents in degraded mode) routinely delivers `Modify(Other)` and
/// `Modify(Metadata)` for newly-created files, which left `file_index_dirty`
/// at `false` and made `xray_fast` miss new files until the 5-min periodic
/// rescan ran. Inverting the predicate to "anything that isn't `Access(_)`"
/// is a safe over-approximation: false positives only cost one cheap rebuild
/// the next time `xray_fast` is called.
pub(crate) fn should_invalidate_file_index(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
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

/// Record a notify backend error on the shared stats handle.
///
/// Extracted from the `start_watcher` event loop so the error-counter
/// path is unit-testable: an earlier regression left
/// `events_errors.fetch_add` wired to the wrong counter for ~4 hours
/// without any test catching it (see
/// `docs/code-reviews/2026-04-21_audit-3day-hidden-bugs.md` P2 finding).
/// The helper is tiny by design — its value is the assertion that the
/// error arm still bumps `events_errors` and nothing else.
pub(crate) fn record_watcher_event_error(stats: &WatcherStats, err: &notify::Error) {
    stats.events_errors.fetch_add(1, Ordering::Relaxed);
    warn!(error = %err, "File watcher error");
}


#[allow(clippy::too_many_arguments)]
pub fn start_watcher(
    index: Arc<RwLock<ContentIndex>>,
    def_index: Option<Arc<RwLock<DefinitionIndex>>>,
    dir: PathBuf,
    content_extensions: Vec<String>,
    definition_extensions: Vec<String>,
    debounce_ms: u64,
    index_base: PathBuf,
    content_ready: Arc<AtomicBool>,
    def_ready: Arc<AtomicBool>,
    file_index_dirty: Arc<AtomicBool>,
    watcher_generation: Arc<AtomicU64>,
    my_generation: u64,
    stats: Arc<WatcherStats>,
    respect_git_exclude: bool,
    autosave_dirty: Arc<AtomicBool>,
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
        //
        // MCP-WCH-006: capture reconciliation deltas so we can checkpoint the
        // new baseline to disk if reconcile actually changed anything. Closes
        // the post-reconcile durability window: PR #208 stopped the in-memory
        // `xray_info` counter from reporting stale post-removal numbers, but
        // the `.meta` on disk is only refreshed every AUTOSAVE_INTERVAL
        // (10 min) by `periodic_autosave`. Without an explicit save here, a
        // forced kill (`Stop-Process -Force`, `cargo install --force`
        // overwriting the binary, OS crash, power loss) within that window
        // resurrects the pre-reconcile state on next start — exactly the
        // ratchet PR #208 fixed at the API layer.
        //
        // We gate the save on ANY content reconcile activity (add/modify/
        // remove) — not just net file-count delta. Three reviewer-caught
        // cases informed this:
        //   1. Pure removal (the original ratchet): `before > after` →
        //      need save so on-disk `files: N` matches RAM.
        //   2. Replace with equal cardinality (delete A.cs + add B.cs):
        //      `before == after` but the file-id allocator and inverted
        //      index diverged → need save (HIGH, commit-reviewer
        //      2026-04-25 first pass).
        //   3. Modify-only (`content_added == 0 && content_removed == 0
        //      && content_modified > 0`): postings purged and rewritten
        //      in-place, `created_at` and `trigram_dirty` updated. Without
        //      saving, force-kill → session C loads stale postings and
        //      serves outdated search results for the modified file
        //      (MEDIUM, commit-reviewer 2026-04-25 second pass).
        //
        // Note on poisoned locks: `reconcile_*` returns (0,0,0) on lock
        // failure, which suppresses the save (no harm: a poisoned content
        // lock will already cause `process_batch` to abort the watcher
        // thread on the next event, and `periodic_autosave` itself also
        // no-ops on read failure).
        let (content_added, content_modified, content_removed) =
            reconcile_content_index(&index, &dir_str, &content_extensions, respect_git_exclude);

        let def_changed = if let Some(ref def_idx) = def_index {
            // Non-blocking reconciliation: parse files OUTSIDE the lock, apply INSIDE.
            // def_ready stays true — MCP requests work on old data during parsing.
            let (added, modified, removed) = definitions::reconcile_definition_index_nonblocking(
                def_idx, &dir_str, &definition_extensions, respect_git_exclude
            );
            added + modified + removed > 0
        } else {
            false
        };

        let mut batch_start: Option<Instant> = None;
        const MAX_ACCUMULATE: Duration = Duration::from_secs(3);

        let mut dirty_files: HashSet<PathBuf> = HashSet::new();
        let mut removed_files: HashSet<PathBuf> = HashSet::new();
        let mut last_autosave = std::time::Instant::now();
        // MCP-WCH-007 (PR-B, Hole #2): tracks whether `process_batch` has
        // applied unsaved changes since the last save. When true, the autosave
        // gate `autosave_due()` fires after AUTOSAVE_QUIET_INTERVAL (5 min)
        // instead of waiting up to AUTOSAVE_MAX_INTERVAL (10 min). Bounds
        // force-kill data loss to minutes, not events.
        let mut have_unsaved = false;

        // MCP-WCH-006: post-reconcile checkpoint. Save iff reconcile actually
        // changed something (any add/modify/remove for content, any
        // add/modify/remove for definitions). Skipped on no-op so
        // steady-state startups stay free.
        if post_reconcile_checkpoint_needed(
            content_added, content_modified, content_removed, def_changed,
        ) {
            info!(
                content_added,
                content_modified,
                content_removed,
                def_changed,
                "Post-reconcile checkpoint: persisting new baseline to disk"
            );
            // Best-effort: warn already logged inside on failure. Cold-start
            // path — if save fails we leave `have_unsaved=true` so the next
            // quiet-interval tick in the watcher loop actually retries
            // (without this flag, callers would clear `have_unsaved` and the
            // retry could not happen until AUTOSAVE_MAX_INTERVAL).
            let saved_ok = periodic_autosave(&index, &def_index, &index_base);
            last_autosave = std::time::Instant::now();
            if !saved_ok {
                have_unsaved = true;
            }
        }

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
                        if !matches_extensions(path, &content_extensions) {
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
                            // process_batch reaches here only on success with
                            // a now-drained non-empty input — by definition
                            // the in-RAM index has changes that haven't hit
                            // disk yet. (MCP-WCH-007, PR-B)
                            have_unsaved = true;
                            batch_start = None;
                            // MCP-WCH-004 + MCP-WCH-007: also check autosave
                            // after a forced flush. Under sustained event
                            // load the `Timeout` branch (where autosave
                            // normally fires) may not run for many minutes,
                            // so we'd otherwise lose every incremental update
                            // on crash. With the quiet-interval gate the
                            // first save now lands ~5min into a sustained
                            // burst instead of after the legacy 10 min.
                            if autosave_due(have_unsaved || autosave_dirty.load(std::sync::atomic::Ordering::Relaxed), last_autosave.elapsed()) {
                                // MCP-WCH-007: bump `last_autosave` even on
                                // failure so a transient write error doesn't
                                // busy-retry every debounce tick — the next
                                // attempt is throttled to AUTOSAVE_QUIET_INTERVAL.
                                // Only clear `have_unsaved` on success so the
                                // retry actually happens.
                                let had_autosave_dirty = begin_autosave_attempt(&autosave_dirty);
                                let saved_ok = periodic_autosave(&index, &def_index, &index_base);
                                last_autosave = std::time::Instant::now();
                                finish_autosave_attempt(saved_ok, had_autosave_dirty, &mut have_unsaved);
                            }
                        }
                }
                Ok(Err(e)) => {
                    record_watcher_event_error(&stats, &e);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Check generation on each timeout (watcher restart)
                    if watcher_generation.load(Ordering::Acquire) != my_generation {
                        info!(dir = %dir_str, my_generation, "Watcher generation changed, exiting");
                        // MCP-WCH-005: flush pending events before exit so a
                        // workspace switch doesn't drop incremental updates
                        // already received from the FS.
                        if !dirty_files.is_empty() || !removed_files.is_empty() {
                            let _ = process_batch(&index, &def_index, &mut dirty_files, &mut removed_files);
                            have_unsaved = true;
                        }
                        // MCP-WCH-007: thread is about to exit — there is no
                        // "next quiet-interval tick". Persist any unsaved work
                        // (from this final flush or a prior batch that hadn't
                        // hit the 5min mark yet) before we drop the receiver.
                        if have_unsaved || autosave_dirty.load(std::sync::atomic::Ordering::Relaxed) {
                            let _ = periodic_autosave(&index, &def_index, &index_base);
                        }
                        break;
                    }
                    // Debounce window expired — process batch
                    if dirty_files.is_empty() && removed_files.is_empty() {
                        // Check periodic autosave (idle-tick path: quiet
                        // interval flushes the tail of a recent burst,
                        // max interval is the unconditional ceiling).
                        if autosave_due(have_unsaved || autosave_dirty.load(std::sync::atomic::Ordering::Relaxed), last_autosave.elapsed()) {
                            let had_autosave_dirty = begin_autosave_attempt(&autosave_dirty);
                            let saved_ok = periodic_autosave(&index, &def_index, &index_base);
                            last_autosave = std::time::Instant::now();
                            finish_autosave_attempt(saved_ok, had_autosave_dirty, &mut have_unsaved);
                        }
                        continue;
                    }
                    if !process_batch(&index, &def_index, &mut dirty_files, &mut removed_files) {
                        error!("RwLock poisoned, watcher thread exiting to avoid infinite error loop");
                        break;
                    }
                    // Successful flush of a non-empty batch — see the
                    // matching note on the force-flush path. (MCP-WCH-007)
                    have_unsaved = true;
                    batch_start = None;
                    // MCP-WCH-004 + MCP-WCH-007: opportunistic autosave on
                    // every successful batch flush, not only when the
                    // pending sets are empty. Quiet-interval gate makes the
                    // first save land ~5min after the burst started.
                    if autosave_due(have_unsaved || autosave_dirty.load(std::sync::atomic::Ordering::Relaxed), last_autosave.elapsed()) {
                        let had_autosave_dirty = begin_autosave_attempt(&autosave_dirty);
                        let saved_ok = periodic_autosave(&index, &def_index, &index_base);
                        last_autosave = std::time::Instant::now();
                        finish_autosave_attempt(saved_ok, had_autosave_dirty, &mut have_unsaved);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    info!("Watcher channel disconnected, stopping");
                    // MCP-WCH-005: flush any pending events before exit so we
                    // don't drop incremental updates that the FS already
                    // reported. `process_batch` is best-effort; if the lock
                    // is poisoned we simply give up (same as the live loop).
                    if !dirty_files.is_empty() || !removed_files.is_empty() {
                        let _ = process_batch(&index, &def_index, &mut dirty_files, &mut removed_files);
                        have_unsaved = true;
                    }
                    // MCP-WCH-007: thread is about to exit — see matching
                    // note on the generation-change path. Persist any
                    // unsaved work before the receiver is dropped.
                    if have_unsaved || autosave_dirty.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = periodic_autosave(&index, &def_index, &index_base);
                    }
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
    // Sub-timings for lock-contention diagnosis:
    /// Residual time: tokenize + parse + filtering + read-lock purge IDs.
    pub tokenize_ms: f64,
    /// Time waiting to acquire content index write lock.
    pub content_lock_wait_ms: f64,
    /// Time holding content index write lock (actual update work).
    pub content_update_ms: f64,
    /// Time waiting to acquire definition index write lock.
    pub def_lock_wait_ms: f64,
    /// Time holding definition index write lock (actual update work).
    pub def_update_ms: f64,
}

/// Result of a single `update_content_index` call with lock-wait timing.
struct ContentUpdateResult {
    ok: bool,
    lock_wait_ms: f64,
    update_ms: f64,
    applied_dirty: usize,
}

/// Result of a single `update_definition_index` call with lock-wait timing.
struct DefUpdateResult {
    ok: bool,
    lock_wait_ms: f64,
    update_ms: f64,
    applied_dirty: usize,
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
    let content_result = update_content_index(index, &removed_clean, &dirty_clean);
    if !content_result.ok {
        stats.content_lock_poisoned = true;
    } else {
        stats.content_updated = content_result.applied_dirty;
    }
    stats.content_lock_wait_ms = content_result.lock_wait_ms;
    stats.content_update_ms = content_result.update_ms;

    let def_result = update_definition_index(def_index, &removed_clean, &dirty_clean);
    if !def_result.ok {
        stats.def_lock_poisoned = true;
    } else if def_index.is_some() {
        stats.def_updated = def_result.applied_dirty;
    }
    stats.def_lock_wait_ms = def_result.lock_wait_ms;
    stats.def_update_ms = def_result.update_ms;

    stats.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    stats.tokenize_ms = stats.elapsed_ms
        - stats.content_lock_wait_ms - stats.content_update_ms
        - stats.def_lock_wait_ms - stats.def_update_ms;

    info!(
        content_updated = stats.content_updated,
        def_updated = stats.def_updated,
        elapsed_ms = format_args!("{:.1}", stats.elapsed_ms),
        tokenize_ms = format_args!("{:.1}", stats.tokenize_ms),
        content_lock_wait_ms = format_args!("{:.1}", stats.content_lock_wait_ms),
        content_update_ms = format_args!("{:.1}", stats.content_update_ms),
        def_lock_wait_ms = format_args!("{:.1}", stats.def_lock_wait_ms),
        def_update_ms = format_args!("{:.1}", stats.def_update_ms),
        "[reindex-trace] reindex_paths_sync complete"
    );

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
    if !update_content_index(index, &removed_clean, &dirty_clean).ok {
        return false;
    }

    // Update definition index (if available)
    if !update_definition_index(def_index, &removed_clean, &dirty_clean).ok {
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
) -> ContentUpdateResult {
    // ── Phase 1: Tokenize all dirty files OUTSIDE the lock (~5ms × N) ──
    // During this phase, MCP requests work normally on the current index data.
    let tokenized: Vec<TokenizedFileResult> = dirty_clean.iter()
        .filter_map(|path| tokenize_file_standalone(path))
        .collect();
    // Only successfully tokenized dirty files are safe to purge. If a dirty
    // file becomes temporarily unreadable between the fs event and tokenization,
    // dropping its old postings would make search lose a still-known file and
    // break total_tokens == sum(file_token_counts). The watcher/rescan can retry
    // on a later event; until then the old index entry is safer than deletion.
    let tokenized_paths: HashSet<PathBuf> = tokenized.iter()
        .map(|result| result.path.clone())
        .collect();
    // This drives sync-reindex response booleans and autosave scheduling. It is
    // deliberately the number of dirty files actually tokenized, not the number
    // of dirty paths requested.
    let applied_dirty_count = tokenized.len();

    // ── Phase 2: Determine purge IDs (READ LOCK — instant when path lookup exists) ──
    let mut path_lookup_missing = false;
    let mut purge_ids: HashSet<u32> = match index.read() {
        Ok(idx) => {
            let mut ids = HashSet::new();
            if let Some(ref p2id) = idx.path_to_id {
                for path in removed_clean {
                    if let Some(&fid) = p2id.get(path) {
                        ids.insert(fid);
                    }
                }
                // Dirty paths that failed tokenization are deliberately absent
                // here; see `tokenized_paths` above for the stale-but-consistent
                // fallback contract.
                for path in &tokenized_paths {
                    if let Some(&fid) = p2id.get(path) {
                        ids.insert(fid);
                    }
                }
            } else {
                path_lookup_missing = true;
            }
            ids
        }
        Err(e) => {
            error!(error = %e, "Failed to acquire content index read lock (poisoned)");
            return ContentUpdateResult { ok: false, lock_wait_ms: 0.0, update_ms: 0.0, applied_dirty: 0 };
        }
    };
    // READ lock released here

    // ── Phase 3: Apply under WRITE LOCK (targeted purge + ~0.1ms × N insert) ──
    let write_wait_start = std::time::Instant::now();
    match index.write() {
        Ok(mut idx) => {
            let lock_wait_ms = write_wait_start.elapsed().as_secs_f64() * 1000.0;
            let update_start = std::time::Instant::now();
            let idx = &mut *idx;
            if path_lookup_missing {
                ensure_path_to_id(idx);
                if let Some(ref p2id) = idx.path_to_id {
                    for path in removed_clean {
                        if let Some(&fid) = p2id.get(path) {
                            purge_ids.insert(fid);
                        }
                    }
                    for path in &tokenized_paths {
                        if let Some(&fid) = p2id.get(path) {
                            purge_ids.insert(fid);
                        }
                    }
                }
            }
            if !idx.file_tokens_authoritative {
                idx.file_tokens.clear();
            }
            if idx.file_tokens_authoritative && idx.file_tokens.is_empty() {
                // Disk-loaded indexes do not persist `file_tokens`. Rebuild at
                // the first mutable watch update instead of on every read-only
                // CLI load, so `xray grep` never pays reverse-map memory/cpu.
                idx.rebuild_file_tokens();
            }
            let mut touched_tokens = Vec::new();

            // Batch purge: touch only stale files' token posting lists when
            // `file_tokens` is available. This is the hot path this change is
            // protecting: a one-file edit should scale with tokens in that file,
            // not with every posting in a monorepo-sized inverted index.
            let purge_start = std::time::Instant::now();
            if !purge_ids.is_empty() {
                // Counts are adjusted only for files we are actually purging:
                // deleted files and dirty files with a successful replacement
                // tokenization. Failed dirty tokenizations keep their old count
                // and old postings until a later successful update.
                for &fid in &purge_ids {
                    let old_count = if (fid as usize) < idx.file_token_counts.len() {
                        idx.file_token_counts[fid as usize] as u64
                    } else {
                        0u64
                    };
                    idx.total_tokens = idx.total_tokens.saturating_sub(old_count);
                }
                touched_tokens = batch_purge_files(
                    &mut idx.index,
                    &mut idx.file_tokens,
                    &idx.file_token_counts,
                    &purge_ids,
                );
            }
            let purge_ms = purge_start.elapsed().as_secs_f64() * 1000.0;

            // Process removed files: update path_to_id, zero token counts,
            // tombstone the files[] slot. We never reuse file_id, so the slot
            // stays in the Vec as an empty string — it’s no longer counted as
            // a live file (see ContentIndex::live_file_count) but file_id
            // assignments remain stable.
            for path in removed_clean {
                let fid = idx.path_to_id.as_ref()
                    .and_then(|p2id| p2id.get(path).copied());
                if let Some(fid) = fid {
                    if (fid as usize) < idx.file_token_counts.len() {
                        idx.file_token_counts[fid as usize] = 0;
                    }
                    if (fid as usize) < idx.files.len() {
                        idx.files[fid as usize].clear();
                    }
                    if let Some(ref mut p2id) = idx.path_to_id {
                        p2id.remove(path);
                    }
                }
            }

            let applied_changes = !purge_ids.is_empty() || !tokenized.is_empty();

            // Apply pre-tokenized results after purge. New files get a fresh
            // file_id; existing dirty files reuse the same file_id whose old
            // postings were removed above.
            let insert_start = std::time::Instant::now();
            let maintain_file_tokens = idx.file_tokens_authoritative;
            for result in tokenized {
                touched_tokens.extend(apply_tokenized_file(idx, result, maintain_file_tokens));
            }
            let insert_ms = insert_start.elapsed().as_secs_f64() * 1000.0;

            if applied_changes {
                // Advance the content watermark only after the in-memory index
                // actually changed. If every dirty file failed tokenization, the
                // old postings are still present and the old mtime must remain
                // eligible for a later watcher/rescan retry.
                idx.trigram_dirty = true;
                idx.created_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or(std::time::Duration::ZERO)
                    .as_secs();
            }

            // Conditionally shrink collections after retain() to release excess capacity.
            shrink_if_oversized_targeted(idx, &touched_tokens);

            let update_ms = update_start.elapsed().as_secs_f64() * 1000.0;
            if update_ms > 100.0 {
                info!(
                    purge_ms = format_args!("{:.1}", purge_ms),
                    insert_ms = format_args!("{:.1}", insert_ms),
                    update_ms = format_args!("{:.1}", update_ms),
                    purge_ids = purge_ids.len(),
                    dirty_files = dirty_clean.len(),
                    "[reindex-trace] update_content_index write lock breakdown"
                );
            }
            ContentUpdateResult { ok: true, lock_wait_ms, update_ms, applied_dirty: applied_dirty_count }
        }
        Err(e) => {
            error!(error = %e, "Failed to acquire content index write lock (poisoned)");
            ContentUpdateResult { ok: false, lock_wait_ms: 0.0, update_ms: 0.0, applied_dirty: 0 }
        }
    }
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
) -> DefUpdateResult {
    let Some(def_idx) = def_index else {
        return DefUpdateResult { ok: true, lock_wait_ms: 0.0, update_ms: 0.0, applied_dirty: 0 };
    };

    // ── Phase 1: Parse all dirty files OUTSIDE the lock (~30ms × N) ──
    // During this phase, MCP requests work normally on the current index data.
    let parsed: Vec<definitions::ParsedFileResult> = dirty_clean.iter()
        .enumerate()
        .filter_map(|(i, path)| definitions::parse_file_standalone(path, i as u32))
        .collect();

    // Track which dirty paths produced a ParsedFileResult. Sync response fields
    // should report applied parser work, not just paths that were requested.
    let parsed_paths: HashSet<PathBuf> = parsed.iter().map(|r| r.path.clone()).collect();
    let applied_dirty = parsed_paths.len();

    // ── Phase 2: Apply under brief WRITE LOCK (~0.1ms × N + removals) ──
    let write_wait_start = std::time::Instant::now();
    match def_idx.write() {
        Ok(mut idx) => {
            let lock_wait_ms = write_wait_start.elapsed().as_secs_f64() * 1000.0;
            let update_start = std::time::Instant::now();

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

            let update_ms = update_start.elapsed().as_secs_f64() * 1000.0;
            DefUpdateResult { ok: true, lock_wait_ms, update_ms, applied_dirty }
        }
        Err(e) => {
            error!(error = %e, "Failed to acquire definition index write lock (poisoned)");
            DefUpdateResult { ok: false, lock_wait_ms: 0.0, update_ms: 0.0, applied_dirty: 0 }
        }
    }
}

/// Conditionally shrink collections after retain() to release excess capacity.
/// Only shrinks when capacity > 2 × len to avoid unnecessary realloc storms.
fn shrink_if_oversized_targeted(idx: &mut ContentIndex, touched_tokens: &[String]) {
    // The old shrink path walked every posting list after every purge. That was
    // small in test repos and expensive in monorepos. We already know exactly
    // which posting lists were retained/inserted, so only those can need Vec
    // capacity cleanup.
    for token in touched_tokens {
        if let Some(postings) = idx.index.get_mut(token)
            && postings.capacity() > postings.len() * 2 {
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

#[cfg(test)]
fn shrink_if_oversized(idx: &mut ContentIndex) {
    let touched_tokens: Vec<String> = idx.index.keys().cloned().collect();
    shrink_if_oversized_targeted(idx, &touched_tokens);
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
fn apply_tokenized_file(
    index: &mut ContentIndex,
    result: TokenizedFileResult,
    maintain_file_tokens: bool,
) -> Vec<String> {
    // In-place mutation requires a stable path lookup. If this helper is
    // accidentally called on a read-only index, do nothing rather than inventing
    // unstable file_ids.
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
        return Vec::new();
    };

    let mut token_names = Vec::with_capacity(result.tokens.len());

    // Collect the token name while consuming the token map. The same token list
    // becomes the reverse-map entry used by the next targeted purge.
    for (token, lines) in result.tokens {
        index.total_tokens += lines.len() as u64;
        token_names.push(token.clone());
        index.index.entry(token)
            .or_default()
            .push(Posting { file_id, lines });
    }
    token_names.sort_unstable();

    if maintain_file_tokens {
        let file_slot = file_id as usize;
        // New files append dense file_ids; resizing keeps file_tokens indexed by the
        // same file_id namespace as files[] and file_token_counts[].
        if index.file_tokens.len() <= file_slot {
            index.file_tokens.resize_with(file_slot + 1, Vec::new);
        }
        index.file_tokens[file_slot] = token_names.clone();
    }

    // Update file token count
    if (file_id as usize) < index.file_token_counts.len() {
        index.file_token_counts[file_id as usize] = result.total_tokens;
    }

    token_names
}

/// Decide whether the post-reconcile checkpoint must run.
///
/// Returns `true` iff reconcile actually changed in-memory state in a way
/// that diverges from the on-disk `.meta` + binary index snapshot:
/// - `content_added > 0`: file_id allocator grew → divergent `idx.files[]`.
/// - `content_removed > 0`: slot tombstoned → divergent `idx.files[]` and
///   stale `live_file_count` on disk (the original ratchet scenario).
/// - `content_modified > 0`: postings purged and rewritten in-place,
///   `created_at` and `trigram_dirty` updated → on-disk inverted index
///   serves stale results for the modified file. Reviewer-caught MEDIUM
///   (commit-reviewer 2026-04-25, second pass): omitting modify left a
///   force-kill window where session C would load stale postings even
///   though file_id allocation was unchanged.
/// - `def_changed`: any add/modify/remove in the definition index — the
///   def-index `.meta` and binary payload encode per-file definitions.
///
/// Returns `false` only when reconcile was a true no-op (the steady-state
/// startup case). This keeps cold-start cost zero for in-sync workspaces.
pub(crate) fn post_reconcile_checkpoint_needed(
    content_added: usize,
    content_modified: usize,
    content_removed: usize,
    def_changed: bool,
) -> bool {
    content_added > 0
        || content_modified > 0
        || content_removed > 0
        || def_changed
}

// MCP-WCH-007 (PR-B, Hole #2 from `user-stories/meta-checkpoint-durability.md`):
// two-tier autosave timing in `start_watcher`'s event loop. The quiet
// interval bounds data loss to ~5min in the bursty-edit-then-idle case
// (delete 50 files → quiet for ~9 min → force-kill loses all 50 pre-PR-B;
// post-PR-B at most ~5min of unflushed activity is lost). The max
// interval preserves the legacy 10-minute upper bound so steady-state
// idle workspaces still write at most once every 10 min.
pub(crate) const AUTOSAVE_QUIET_INTERVAL: Duration = Duration::from_secs(300);
pub(crate) const AUTOSAVE_MAX_INTERVAL: Duration = Duration::from_secs(600);

/// Two-tier autosave gate (PR-B, Hole #2). Returns `true` when the
/// `start_watcher` event loop should run `periodic_autosave`:
///
/// - `have_unsaved && since_last_save >= AUTOSAVE_QUIET_INTERVAL`: bursty
///   activity recently completed; flush within ~5min of the last batch
///   so a force-kill loses at most ~5min of changes.
/// - `since_last_save >= AUTOSAVE_MAX_INTERVAL`: hard ceiling, fires
///   even with `have_unsaved == false` (matches legacy behavior; on a
///   clean index the actual `periodic_autosave` call short-circuits via
///   its existing `!idx.files.is_empty()` allocator-capacity gate).
///
/// Pure function for unit testability (no `Instant`, no I/O).
fn begin_autosave_attempt(autosave_dirty: &AtomicBool) -> bool {
    // Clear the external dirty channel before taking snapshots. Any mutation that
    // set this bit before the swap is about to be included in this save attempt;
    // any mutation that sets it after the swap happened during serialization and
    // must remain visible for the next attempt.
    autosave_dirty.swap(false, Ordering::Relaxed)
}

fn finish_autosave_attempt(saved_ok: bool, had_autosave_dirty: bool, have_unsaved: &mut bool) {
    if saved_ok {
        // A successful save covers all local watcher batches flushed before the
        // attempt plus any external dirty bit consumed by begin_autosave_attempt.
        // Do not copy `had_autosave_dirty` back into have_unsaved: that was the
        // pre-save bit and would keep the watcher re-armed forever after a
        // reconcile-only save.
        *have_unsaved = false;
    } else if had_autosave_dirty {
        // The attempt consumed an external dirty bit but failed before it became
        // durable. Preserve retry pressure through the local channel; concurrent
        // external mutations after the swap remain in the atomic channel.
        *have_unsaved = true;
    }
}


pub(crate) fn autosave_due(have_unsaved: bool, since_last_save: Duration) -> bool {
    (have_unsaved && since_last_save >= AUTOSAVE_QUIET_INTERVAL)
        || since_last_save >= AUTOSAVE_MAX_INTERVAL
}

/// Periodically save in-memory indexes to disk to protect against data loss
/// from forced process termination (e.g., VS Code killing the MCP server).
///
/// Takes READ locks only — MCP queries are NOT blocked during save.
/// Watcher incremental updates (which need write locks) will be briefly delayed.
/// Returns `true` when the in-memory state is durably reflected on disk
/// after this call:
///   - both indexes were skipped because they have never held any file
///     (allocator capacity == 0),
///   - or every attempted save succeeded.
///
/// Returns `false` when at least one attempted save failed (lock poisoned
/// or write error). Callers in `start_watcher` use the return value to
/// decide whether the unsaved-changes flag may be cleared — on failure,
/// the next quiet-interval tick will re-attempt the save while still
/// throttling at ~5min instead of busy-retrying every debounce tick.
fn periodic_autosave(
    index: &Arc<RwLock<ContentIndex>>,
    def_index: &Option<Arc<RwLock<DefinitionIndex>>>,
    index_base: &std::path::Path,
) -> bool {
    let start = std::time::Instant::now();
    let mut saved = Vec::new();
    let mut all_ok = true;

    // ── Content index: snapshot under brief read lock, serialize without lock ──
    // Gate: `idx.files.is_empty()` — allocator-capacity check, NOT live count.
    // Rationale: if all files were removed during this session, `live_file_count() == 0`
    // but the index is still dirty (the on-disk copy holds stale entries). We MUST
    // checkpoint that "now empty" state so a forced kill doesn't resurrect them.
    // We only skip when the index has never held any file (allocator never grew).
    let content_snapshot = match index.read() {
        Ok(idx) => {
            if idx.files.is_empty() { None }
            else {
                let clone_start = std::time::Instant::now();
                let snapshot = idx.clone();
                let clone_ms = clone_start.elapsed().as_secs_f64() * 1000.0;
                Some((snapshot, clone_ms))
            }
        }
        Err(e) => {
            warn!(error = %e, "Periodic autosave: failed to read content index");
            all_ok = false;
            None
        }
    };
    // READ lock released — clone took ~1-2s for large indexes, not ~79s serialization.

    if let Some((snapshot, clone_ms)) = content_snapshot {
        let serialize_start = std::time::Instant::now();
        if let Err(e) = crate::save_content_index(&snapshot, index_base) {
            warn!(error = %e, "Periodic autosave: failed to save content index");
            all_ok = false;
        } else {
            let serialize_ms = serialize_start.elapsed().as_secs_f64() * 1000.0;
            saved.push(format!("content({} files, cloneMs={:.0}, serializeMs={:.0})",
                snapshot.live_file_count(), clone_ms, serialize_ms));
        }
    }

    // ── Definition index: same snapshot-then-serialize pattern ──
    if let Some(def_idx) = def_index {
        let def_snapshot = match def_idx.read() {
            Ok(idx) => {
                if idx.files.is_empty() { None }
                else {
                    let clone_start = std::time::Instant::now();
                    let snapshot = idx.clone();
                    let clone_ms = clone_start.elapsed().as_secs_f64() * 1000.0;
                    Some((snapshot, clone_ms))
                }
            }
            Err(e) => {
                warn!(error = %e, "Periodic autosave: failed to read definition index");
                all_ok = false;
                None
            }
        };

        if let Some((snapshot, clone_ms)) = def_snapshot {
            let serialize_start = std::time::Instant::now();
            if let Err(e) = crate::definitions::save_definition_index(&snapshot, index_base) {
                warn!(error = %e, "Periodic autosave: failed to save definition index");
                all_ok = false;
            } else {
                let serialize_ms = serialize_start.elapsed().as_secs_f64() * 1000.0;
                saved.push(format!("def({} defs, cloneMs={:.0}, serializeMs={:.0})",
                    snapshot.definitions.len(), clone_ms, serialize_ms));
            }
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

    all_ok
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

fn ensure_path_to_id(index: &mut ContentIndex) {
    if index.path_to_id.is_some() {
        return;
    }

    let mut path_to_id = HashMap::with_capacity(index.files.len());
    // Skip empty tombstone slots from files removed in previous sessions.
    for (file_id, path) in index.files.iter().enumerate() {
        if path.is_empty() {
            continue;
        }
        path_to_id.insert(PathBuf::from(path), file_id as u32);
    }
    index.path_to_id = Some(path_to_id);
}

/// Build a ContentIndex with mutation lookups populated (for watch mode).
pub fn build_watch_index_from(mut index: ContentIndex) -> ContentIndex {
    ensure_path_to_id(&mut index);
    index.file_tokens_authoritative = true;
    index.rebuild_file_tokens();
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

/// Batch purge multiple files from the inverted index.
///
/// With a populated `file_tokens` reverse map, this is targeted:
/// O(tokens_in_purged_files * affected_posting_len) instead of walking every
/// token/posting list in the index. That is the difference between a one-file
/// edit blocking readers for milliseconds and blocking them for seconds on a
/// monorepo-sized content index.
///
/// If `file_tokens` is empty, the index has not entered mutable watch mode yet
/// (or a test is intentionally exercising the legacy path), so we fall back to
/// the old full scan. If `file_tokens` is non-empty but inconsistent, panic: a
/// partial reverse map is corruption and silently degrading would leave stale
/// postings in search results.
fn batch_purge_files(
    inverted: &mut std::collections::HashMap<String, Vec<Posting>>,
    file_tokens: &mut [Vec<String>],
    file_token_counts: &[u32],
    file_ids: &HashSet<u32>,
) -> Vec<String> {
    if file_ids.is_empty() {
        return Vec::new();
    }

    if file_tokens.is_empty() {
        // Compatibility path for cold/deserialized indexes before
        // `rebuild_file_tokens()` has run. It is slower, but correct because it
        // inspects every posting instead of trusting absent reverse data.
        let mut touched_tokens = Vec::with_capacity(inverted.len());
        inverted.retain(|token, postings| {
            touched_tokens.push(token.clone());
            postings.retain(|p| !file_ids.contains(&p.file_id));
            !postings.is_empty()
        });
        touched_tokens.sort_unstable();
        return touched_tokens;
    }

    // A non-empty reverse map is an all-or-bug contract: every live file_id in
    // the mutable index must have a slot. Truncation would otherwise make the
    // targeted path skip stale postings without noticing.
    let max_file_id = file_ids.iter().copied().max().unwrap_or(0) as usize;
    assert!(
        max_file_id < file_tokens.len(),
        "file_tokens missing entry for file_id {}",
        max_file_id
    );

    let mut touched_tokens = Vec::new();
    for &file_id in file_ids {
        let file_slot = file_id as usize;
        let token_count = file_token_counts.get(file_slot).copied().unwrap_or(0);
        // An empty slot is valid only for tombstoned/zero-token files. For a live
        // file with tokens, an empty reverse entry means the targeted purge would
        // touch nothing while old postings remained searchable.
        assert!(
            token_count == 0 || !file_tokens[file_slot].is_empty(),
            "file_tokens empty for file_id {} with {} indexed tokens",
            file_id,
            token_count
        );
        touched_tokens.extend(file_tokens[file_slot].iter().cloned());
    }
    // Multiple files commonly share hot tokens such as `class`; dedup before
    // retaining so each posting list is scanned once per batch.
    touched_tokens.sort_unstable();
    touched_tokens.dedup();

    for token in &touched_tokens {
        let remove_token = if let Some(postings) = inverted.get_mut(token) {
            postings.retain(|p| !file_ids.contains(&p.file_id));
            postings.is_empty()
        } else {
            false
        };
        if remove_token {
            inverted.remove(token);
        }
    }

    for &file_id in file_ids {
        // Keep the reverse map aligned with the forward index: after purge, this
        // file_id has no postings until apply_tokenized_file inserts new ones.
        file_tokens[file_id as usize].clear();
    }

    touched_tokens
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
pub(crate) fn scan_dir_state(dir: &str, extensions: &[String], respect_git_exclude: bool) -> DirState {
    let dir_path = canonicalize_or_warn(dir);

    let mut walker = WalkBuilder::new(&dir_path);
    walker
        .follow_links(true)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(respect_git_exclude);

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
#[allow(dead_code)] // individual fields are read by tests; aggregate not yet used by the binary
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
/// `FileIndex.entries[].path` and `all_files` keys are both absolute,
/// forward-slash `clean_path`-normalised strings (see `build_index`
/// in `src/index.rs` and `scan_dir_state` in this file), so we compare
/// them via direct set-difference in O(n) — no `ends_with` heuristic.
///
/// On Windows both sides are lowercased before comparison so that a
/// drive-letter case mismatch (`C:/Repos/Xray` vs `c:/repos/xray`
/// between `canonicalize_or_warn` and `build_index`) does not report
/// every file as both added and removed on every tick.
///
/// If the `FileIndex` slot is `None` (lazy: not yet built — happens
/// before the first `xray_fast` call), drift is reported as
/// `(0, 0)` because `file_index_dirty` is already set to `true` at
/// watcher startup; counting "lazy init" as drift would inflate the
/// `periodic_rescan_drift_events` metric whose semantics are
/// "notify missed an event".
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
        // Not yet built — do NOT count as drift. Lazy build is handled
        // by the existing `file_index_dirty` flag set at watcher init.
        return (0, 0);
    };

    // Normalise once. On Windows lowercase to tolerate drive-letter
    // case drift between `canonicalize_or_warn` output and the path
    // stored by `build_index` (both are `clean_path`-normalised but
    // Windows canonicalise sometimes yields uppercase `C:` while the
    // user-supplied `--dir` may be `c:`).
    #[cfg(windows)]
    fn norm(s: &str) -> String { s.to_lowercase() }
    #[cfg(not(windows))]
    fn norm(s: &str) -> String { s.to_string() }

    // `FileIndex.entries[].path` may be either absolute (production,
    // built from an absolute `--dir`) or relative to `fi.root` (tests,
    // and historical indexes). Normalise to absolute by joining with
    // `fi.root` whenever the entry does not already contain the root
    // prefix. `all_files` keys are always absolute and already
    // `clean_path`-normalised.
    let root_norm = norm(&clean_path(&fi.root));
    let resolve = |p: &str| -> String {
        let p_norm = norm(&clean_path(p));
        if p_norm.starts_with(&root_norm) || p_norm.starts_with('/') || p_norm.chars().nth(1) == Some(':') {
            p_norm
        } else {
            let sep = if root_norm.ends_with('/') { "" } else { "/" };
            format!("{}{}{}", root_norm, sep, p_norm)
        }
    };

    let in_index: HashSet<String> = fi.entries.iter()
        .filter(|e| !e.is_dir)
        .map(|e| resolve(&e.path))
        .collect();
    let on_disk_set: HashSet<String> = all_files.keys()
        .map(|p| norm(&p.to_string_lossy()))
        .collect();

    let added = on_disk_set.difference(&in_index).count();
    let removed = in_index.difference(&on_disk_set).count();
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
pub(crate) fn periodic_rescan_once(
    index: &Arc<RwLock<ContentIndex>>,
    def_index: &Option<Arc<RwLock<DefinitionIndex>>>,
    file_index: &Arc<RwLock<Option<FileIndex>>>,
    file_index_dirty: &Arc<AtomicBool>,
    dir: &str,
    content_extensions: &[String],
    definition_extensions: &[String],
    stats: &Arc<WatcherStats>,
    respect_git_exclude: bool,
    autosave_dirty: &Arc<AtomicBool>,
) -> RescanOutcome {
    let start = Instant::now();
    stats.periodic_rescan_total.fetch_add(1, Ordering::Relaxed);

    let state = scan_dir_state(dir, content_extensions, respect_git_exclude);
    let (content_added, content_removed, content_modified) =
        compute_content_drift(index, &state.ext_matched);
    let (file_index_added, file_index_removed) =
        compute_file_index_drift(file_index, &state.all_files);

    let content_drift = content_added + content_removed + content_modified > 0;
    let file_drift = file_index_added + file_index_removed > 0;
    let drift_detected = content_drift || file_drift;

    // Any drift — content or file-list — means the file-list index
    // is stale relative to disk. Setting the dirty flag is cheap and
    // defensive: xray_fast's next call will rebuild. In particular,
    // when `file_index` is `None` at rescan time (lazy init), a later
    // build would miss files that were added without a notify event
    // unless we force a rebuild here.
    if drift_detected {
        file_index_dirty.store(true, Ordering::Relaxed);
    }
    if content_drift {
        // Delegate to the existing reconcilers. They each perform
        // their own walk today (acceptable on a 5-min cadence;
        // collapsing to a single walk is a follow-up). Bailing out
        // when nothing changed is their internal fast path.
        let (applied_added, applied_modified, applied_removed) =
            reconcile_content_index(index, dir, content_extensions, respect_git_exclude);
        let mut definition_changed = false;
        if let Some(di) = def_index {
            let (def_added, def_modified, def_removed) =
                definitions::reconcile_definition_index_nonblocking(di, dir, definition_extensions, respect_git_exclude);
            definition_changed = def_added + def_modified + def_removed > 0;
        }
        // Schedule autosave from applied content changes, not merely detected
        // content drift. If content tokenization failed, the old content snapshot
        // is intentionally kept in memory and should not be checkpointed as the
        // new baseline. Definition reconcile is different: a dirty file that
        // fails to parse can still remove stale definitions and advance the def
        // watermark, so any reported def delta must also be persisted.
        if applied_added + applied_modified + applied_removed > 0 || definition_changed {
            autosave_dirty.store(true, Ordering::Relaxed);
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

/// Minimum allowed value of `--rescan-interval-sec`. Values below are
/// silently raised in [`start_periodic_rescan`] so a typo cannot
/// schedule a self-DoS on a large workspace (a single tick walks the
/// whole tree).
pub(crate) const MIN_RESCAN_INTERVAL_SEC: u64 = 10;

/// Granularity of the shutdown-poll inside the rescan sleep loop.
/// Small enough to keep workspace-switch latency low, large enough to
/// keep idle CPU at zero. The thread checks `watcher_generation`
/// every tick.
const RESCAN_SHUTDOWN_POLL: Duration = Duration::from_millis(500);

/// Spawn the periodic-rescan fail-safe thread (Phase 3 of the rollout
/// in `docs/todo_approved_2026-04-21_watcher-periodic-rescan.md`).
///
/// Ticks [`periodic_rescan_once`] on `interval_sec` (clamped to
/// [`MIN_RESCAN_INTERVAL_SEC`]). Sleeps in [`RESCAN_SHUTDOWN_POLL`]
/// slices so workspace switches (`watcher_generation` bump) cause the
/// thread to exit within ~500 ms instead of waiting for a full
/// interval. Idempotent: safe to spawn alongside the live notify
/// event loop — drift is rare on the happy path and produces no
/// extra index work (internal fast paths bail out).
///
/// Returns immediately after spawning; the join handle is dropped on
/// purpose (the thread self-terminates on generation change, mirroring
/// the watcher event loop's shutdown contract).
#[allow(clippy::too_many_arguments)]
pub fn start_periodic_rescan(
    index: Arc<RwLock<ContentIndex>>,
    def_index: Option<Arc<RwLock<DefinitionIndex>>>,
    file_index: Arc<RwLock<Option<FileIndex>>>,
    file_index_dirty: Arc<AtomicBool>,
    dir: PathBuf,
    content_extensions: Vec<String>,
    definition_extensions: Vec<String>,
    interval_sec: u64,
    watcher_generation: Arc<AtomicU64>,
    my_generation: u64,
    stats: Arc<WatcherStats>,
    respect_git_exclude: bool,
    autosave_dirty: Arc<AtomicBool>,
) {
    let effective = interval_sec.max(MIN_RESCAN_INTERVAL_SEC);
    if effective != interval_sec {
        warn!(
            requested = interval_sec,
            effective,
            min = MIN_RESCAN_INTERVAL_SEC,
            "periodic rescan interval clamped to minimum"
        );
    }
    let dir_str = clean_path(&dir.to_string_lossy());
    info!(dir = %dir_str, interval_sec = effective, "periodic rescan thread starting");

    std::thread::spawn(move || {
        let interval = Duration::from_secs(effective);
        // Sleep first so the thread does not race the initial index
        // build / startup reconciliation already triggered by the
        // watcher event loop.
        loop {
            // Sleep in slices for prompt shutdown.
            let sleep_start = Instant::now();
            while sleep_start.elapsed() < interval {
                if watcher_generation.load(Ordering::Acquire) != my_generation {
                    info!(my_generation, "periodic rescan generation changed, exiting");
                    return;
                }
                std::thread::sleep(RESCAN_SHUTDOWN_POLL);
            }

            // Re-check after the sleep before doing real work.
            if watcher_generation.load(Ordering::Acquire) != my_generation {
                info!(my_generation, "periodic rescan generation changed, exiting");
                return;
            }

            let _ = periodic_rescan_once(
                &index,
                &def_index,
                &file_index,
                &file_index_dirty,
                &dir_str,
                &content_extensions,
                &definition_extensions,
                &stats,
                respect_git_exclude,
                &autosave_dirty,
            );
        }
    });
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
    respect_git_exclude: bool,
) -> (usize, usize, usize) {
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
    let dir_state = scan_dir_state(dir, extensions, respect_git_exclude);
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
                    if p2id.contains_key(path) {
                        // Existing file: decide whether it should be retokenized.
                        // Do not add its file_id to purge_ids here; purge is safe
                        // only after Phase 3 successfully produces replacement
                        // tokens for this path.
                        if *mtime > threshold {
                            to_tokenize.push(path.clone());
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
            return (0, 0, 0);
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
        return (0, 0, 0);
    }

    // ── Phase 3: Tokenize all new/modified files (NO LOCK) ──
    // During this phase, MCP requests work normally on the old index data.
    let tokenized: Vec<TokenizedFileResult> = to_tokenize.iter()
        .filter_map(|path| tokenize_file_standalone(path))
        .collect();

    // ── Phase 4: Apply under WRITE LOCK (targeted purge + ~0.1ms × N insert) ──
    let mut applied_added = 0usize;
    let mut applied_modified = 0usize;
    match index.write() {
        Ok(mut idx) => {
            let idx = &mut *idx;
            if !idx.file_tokens_authoritative {
                idx.file_tokens.clear();
            }
            if idx.file_tokens_authoritative && idx.file_tokens.is_empty() {
                // Reconciliation can run immediately after startup on an index
                // loaded from disk. Build the derived reverse map at this mutable
                // gateway instead of in generic read-only load paths.
                idx.rebuild_file_tokens();
            }
            let mut purge_ids = purge_ids;
            if let Some(ref p2id) = idx.path_to_id {
                // Modified files are added to purge_ids only after tokenization
                // succeeds. If read/tokenize fails, keep old postings/counts so
                // the index remains internally consistent and future rescans can
                // retry without having hidden a known file from search.
                for result in &tokenized {
                    if let Some(&fid) = p2id.get(&result.path) {
                        purge_ids.insert(fid);
                        applied_modified += 1;
                    } else {
                        applied_added += 1;
                    }
                }
            }
            let mut touched_tokens = Vec::new();

            // Batch purge stale postings through file_tokens when available.
            // The set contains deleted files plus modified files that have a
            // successful replacement tokenization ready to insert.
            if !purge_ids.is_empty() {
                for &fid in &purge_ids {
                    let old_count = if (fid as usize) < idx.file_token_counts.len() {
                        idx.file_token_counts[fid as usize] as u64
                    } else {
                        0u64
                    };
                    idx.total_tokens = idx.total_tokens.saturating_sub(old_count);
                }
                touched_tokens = batch_purge_files(
                    &mut idx.index,
                    &mut idx.file_tokens,
                    &idx.file_token_counts,
                    &purge_ids,
                );
            }

            // Process removed files: update path_to_id, zero token counts,
            // tombstone the files[] slot (see comment in update_content_index).
            for path in &to_remove {
                let fid = idx.path_to_id.as_ref()
                    .and_then(|p2id| p2id.get(path).copied());
                if let Some(fid) = fid {
                    if (fid as usize) < idx.file_token_counts.len() {
                        idx.file_token_counts[fid as usize] = 0;
                    }
                    if (fid as usize) < idx.files.len() {
                        idx.files[fid as usize].clear();
                    }
                    if let Some(ref mut p2id) = idx.path_to_id {
                        p2id.remove(path);
                    }
                }
            }

            // Apply pre-tokenized results after purge. New files append file_ids;
            // modified files reuse the file_id whose stale postings were purged.
            let maintain_file_tokens = idx.file_tokens_authoritative;
            for result in tokenized {
                touched_tokens.extend(apply_tokenized_file(idx, result, maintain_file_tokens));
            }

            shrink_if_oversized_targeted(idx, &touched_tokens);

            // Advance the reconciliation watermark only for changes that were
            // actually applied. Detected-but-failed tokenizations must remain
            // visible to the next periodic rescan, and startup reconcile must
            // not checkpoint that stale snapshot as the new baseline.
            if applied_added > 0 || applied_modified > 0 || removed > 0 {
                idx.created_at = walk_start;
                idx.trigram_dirty = true;
            }

            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

            if applied_added > 0 || applied_modified > 0 || removed > 0 {
                info!(
                    scanned,
                    added = applied_added,
                    modified = applied_modified,
                    removed,
                    detected_added = added,
                    detected_modified = modified,
                    elapsed_ms = format_args!("{:.1}", elapsed_ms),
                    "Content index reconciliation complete (non-blocking)"
                );
            } else if added > 0 || modified > 0 {
                info!(
                    scanned,
                    detected_added = added,
                    detected_modified = modified,
                    elapsed_ms = format_args!("{:.1}", elapsed_ms),
                    "Content index reconciliation: changes detected but no files tokenized"
                );
            } else {
                info!(
                    scanned,
                    elapsed_ms = format_args!("{:.1}", elapsed_ms),
                    "Content index reconciliation: all files up to date"
                );
            }

            crate::index::log_memory(&format!(
                "watcher: content reconciliation non-blocking (scanned={}, added={}, modified={}, removed={}, detected_added={}, detected_modified={}, {:.0}ms)",
                scanned, applied_added, applied_modified, removed, added, modified, elapsed_ms
            ));
        }
        Err(e) => {
            error!(error = %e, "Failed to acquire content index write lock for reconciliation");
            return (0, 0, 0);
        }
    }

    (applied_added, applied_modified, removed)
}

#[cfg(test)]
#[path = "watcher_tests.rs"]
mod tests;
