//! MCP server startup and configuration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use tracing::{info, warn};

use crate::{
    build_content_index, save_content_index, load_content_index, find_content_index_for_dir,
    index_dir, ContentIndex, TrigramIndex, DEFAULT_MIN_TOKEN_LEN,
};
use crate::definitions;
use crate::git::cache::GitHistoryCache;
use crate::mcp;

use super::args::{ServeArgs, ContentIndexArgs};

pub fn cmd_serve(args: ServeArgs) {
    let dir_str = args.dir.clone();
    // Flatten multi-value --ext: supports both ["rs", "md"] and ["rs,md"]
    let extensions: Vec<String> = args.ext.iter()
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let exts_for_load = extensions.join(",");

    let idx_base = index_dir();
    init_logging(&args, &dir_str, &exts_for_load, &idx_base);

    // ─── Workspace binding: determine BEFORE index building ───
    let extensions_vec: Vec<String> = exts_for_load.split(',').map(|s| s.to_string()).collect();
    let ws_binding = mcp::handlers::determine_initial_binding(&dir_str, &extensions_vec, args.respect_git_exclude);
    let is_unresolved = ws_binding.status == mcp::handlers::WorkspaceStatus::Unresolved;
    if is_unresolved {
        warn!(target: "xray::startup", dir = %ws_binding.dir, "workspace UNRESOLVED (no source files), skipping index build");
    }

    // ─── Async startup flags ───
    let content_ready = Arc::new(AtomicBool::new(false));
    let def_ready = Arc::new(AtomicBool::new(false));
    let file_index_dirty = Arc::new(AtomicBool::new(true));
    let content_building = Arc::new(AtomicBool::new(false));
    let def_building = Arc::new(AtomicBool::new(false));

    // ─── Content index: load from disk or build in background ───
    // Returns the effective `respect_git_exclude` resolved against any persisted
    // value on disk — see `resolve_respect_git_exclude` in `super`. This must be
    // used downstream (watcher, periodic rescan, HandlerContext) instead of the
    // raw CLI flag, otherwise long-running paths silently drop the user's
    // original opt-in (Bug 4 follow-up, 2026-04-24).
    let (index, effective_respect_content) = load_or_build_content_index(
        &dir_str, &exts_for_load, &extensions, &idx_base,
        is_unresolved, args.watch, args.respect_git_exclude,
        &content_ready, &content_building,
    );

    // ─── Definition index: same async pattern ───
    let (def_index, def_extensions_vec, effective_respect_def) = load_or_build_definition_index(
        &dir_str, &extensions, &idx_base, &exts_for_load,
        is_unresolved, args.definitions,
        &def_ready, &def_building,
        args.respect_git_exclude,
    );

    // Pick the canonical effective value for watcher / periodic rescan /
    // HandlerContext. Content and definition indexes are normally built with
    // the same flag value; if they differ (exotic mixed-state on disk),
    // prefer the stricter `true` so excluded files do not leak back via the
    // walker. Both `resolve_respect_git_exclude` calls have already warned
    // about any persisted-vs-CLI mismatch.
    let effective_respect_git_exclude = effective_respect_content || effective_respect_def;
    if effective_respect_content != effective_respect_def {
        warn!(
            target: "xray::startup",
            content = effective_respect_content,
            definitions = effective_respect_def,
            chosen = effective_respect_git_exclude,
            "Content and definition indexes have different persisted respect_git_exclude values; using the stricter (true) for watcher / reindex paths. Run `xray index-content` and `xray index-definitions` to align them explicitly.",
        );
    }

    // ─── File watcher ───
    let watcher_generation = Arc::new(AtomicU64::new(0));
    let watcher_stats = Arc::new(mcp::watcher::WatcherStats::new());
    let file_index = Arc::new(RwLock::new(None));
    if args.watch && !is_unresolved {
        let watch_dir = std::fs::canonicalize(&dir_str)
            .unwrap_or_else(|_| PathBuf::from(&dir_str));
        crate::index::log_memory("serve: starting watcher");
        if let Err(e) = mcp::watcher::start_watcher(
            Arc::clone(&index),
            def_index.as_ref().map(Arc::clone),
            watch_dir.clone(),
            extensions.clone(),
            args.debounce_ms,
            idx_base.clone(),
            Arc::clone(&content_ready),
            Arc::clone(&def_ready),
            Arc::clone(&file_index_dirty),
            Arc::clone(&watcher_generation),
            0, // initial generation
            Arc::clone(&watcher_stats),
            effective_respect_git_exclude,
        ) {
            warn!(error = %e, "Failed to start file watcher");
        }

        // ─── Periodic rescan fail-safe (Phase 3) ───
        // Catches filesystem events that the OS-level `notify` watcher
        // dropped (best-effort on every platform — see
        // `docs/bug-reports/bug-2026-04-21-watcher-misses-new-files-both-indexes.md`).
        if !args.no_periodic_rescan {
            mcp::watcher::start_periodic_rescan(
                Arc::clone(&index),
                def_index.as_ref().map(Arc::clone),
                Arc::clone(&file_index),
                Arc::clone(&file_index_dirty),
                watch_dir,
                extensions,
                args.rescan_interval_sec,
                Arc::clone(&watcher_generation),
                0,
                Arc::clone(&watcher_stats),
                effective_respect_git_exclude,
            );
        }
    }

    // ─── Git history cache: background build ───
    let (git_cache, git_cache_ready) = build_git_cache_background(&dir_str, &idx_base, is_unresolved);

    // ─── Detect current branch ───
    let current_branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&dir_str)
        .output()
        .ok()
        .and_then(|o| if o.status.success() {
            String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
        } else { None });

    if let Some(ref branch) = current_branch {
        info!(branch = %branch, "Detected current branch");
    }

    let max_response_bytes = if args.max_response_kb == 0 { 0 } else { args.max_response_kb * 1024 };

    let ctx = mcp::handlers::HandlerContext {
        index,
        def_index,
        workspace: Arc::new(RwLock::new(ws_binding)),
        server_ext: exts_for_load,
        metrics: args.metrics,
        index_base: idx_base,
        max_response_bytes,
        content_ready,
        def_ready,
        git_cache,
        git_cache_ready,
        current_branch,
        def_extensions: def_extensions_vec,
        file_index,
        file_index_dirty: Arc::clone(&file_index_dirty),
        content_building,
        def_building,
        watcher_generation,
        watch_enabled: args.watch,
        watch_debounce_ms: args.debounce_ms,
        respect_git_exclude: effective_respect_git_exclude,
        watcher_stats,
        periodic_rescan_enabled: !args.no_periodic_rescan,
        rescan_interval_sec: args.rescan_interval_sec,
        branch_name_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
    };
    mcp::server::run_server(ctx);
    crate::index::log_memory("serve: event loop exited");
}

fn init_logging(args: &ServeArgs, dir_str: &str, exts_for_load: &str, idx_base: &Path) {
    let log_level = match args.log_level.as_str() {
        "error" => tracing::Level::ERROR,
        "warn" => tracing::Level::WARN,
        "debug" => tracing::Level::DEBUG,
        "trace" => tracing::Level::TRACE,
        _ => tracing::Level::INFO,
    };
    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .with_target(true)
        .with_writer(std::io::stderr)
        .init();

    info!(dir = %dir_str, ext = %exts_for_load, "Starting MCP server");

    // Enable debug logging if --debug-log was passed
    if args.debug_log {
        crate::index::enable_debug_log(idx_base, dir_str);
    }
    crate::index::log_memory("serve: startup");

    // Log CWD and canonical dir for debugging dir-agnostic mode
    let cwd = std::env::current_dir().map(|p| crate::clean_path(&p.to_string_lossy())).unwrap_or_else(|_| "<unknown>".to_string());
    let canonical_dir = std::fs::canonicalize(dir_str).map(|p| crate::clean_path(&p.to_string_lossy())).unwrap_or_else(|_| dir_str.to_string());
    info!(target: "xray::startup", dir = ?dir_str, cwd = %cwd, canonical = %canonical_dir, "server starting");
}

#[allow(clippy::too_many_arguments)]
fn load_or_build_content_index(
    dir_str: &str,
    exts_for_load: &str,
    extensions: &[String],
    idx_base: &Path,
    is_unresolved: bool,
    watch: bool,
    respect_git_exclude: bool,
    content_ready: &Arc<AtomicBool>,
    content_building: &Arc<AtomicBool>,
) -> (Arc<RwLock<ContentIndex>>, bool) {
    // ─── Async startup: create empty indexes, start event loop immediately ───
    let empty_index = ContentIndex {
        root: dir_str.to_string(),
        format_version: 0,
        created_at: 0,
        max_age_secs: 86400,
        files: Vec::new(),
        index: HashMap::new(),
        total_tokens: 0,
        extensions: extensions.to_vec(),
        file_token_counts: Vec::new(),
        trigram: TrigramIndex::default(),
        trigram_dirty: false,
        path_to_id: if watch { Some(HashMap::new()) } else { None },
        read_errors: 0,
        lossy_file_count: 0,
        worker_panics: 0,
        respect_git_exclude,
    };
    let index = Arc::new(RwLock::new(empty_index));

    // Default to the CLI flag; if a persisted index is loaded below, this is
    // upgraded to the persisted value via `resolve_respect_git_exclude` so the
    // caller (cmd_serve) threads the user's original opt-in into the watcher,
    // periodic rescan, and HandlerContext rather than silently dropping it.
    let mut effective_respect_git_exclude = respect_git_exclude;

    // Try fast load from disk (typically < 3s)
    // Skip if workspace is Unresolved — no point loading/building indexes for wrong directory.
    let start = Instant::now();
    let direct_load = if is_unresolved { None } else {
        load_content_index(dir_str, exts_for_load, idx_base).ok()
    };
    let load_method;
    let loaded = if direct_load.is_some() {
        load_method = "direct";
        direct_load
    } else {
        load_method = "fallback";
        if is_unresolved { None } else { find_content_index_for_dir(dir_str, idx_base, extensions) }
    };

    if let Some(idx) = loaded {
        let load_elapsed = start.elapsed();
        let cache_age = format_cache_age(idx.created_at);
        // Resolve effective `respect_git_exclude` against persisted value:
        // prefer persisted (warn on mismatch) so long-running paths (watcher,
        // periodic rescan, MCP reindex) cannot silently flip the policy.
        effective_respect_git_exclude = super::resolve_respect_git_exclude(
            "serve/content",
            Some(idx.respect_git_exclude),
            respect_git_exclude,
        );
        info!(
            elapsed_ms = format_args!("{:.1}", load_elapsed.as_secs_f64() * 1000.0),
            files = idx.files.len(),
            tokens = idx.index.len(),
            cache_age = %cache_age,
            "Content index loaded from disk"
        );
        crate::index::log_memory(&format!(
            "serve: content loaded [{}] (files={}, tokens={}, trigrams={}, age={})",
            load_method, idx.files.len(), idx.index.len(),
            idx.trigram.trigram_map.len(), cache_age
        ));
        let mut idx = if watch {
            mcp::watcher::build_watch_index_from(idx)
        } else {
            idx
        };
        idx.shrink_maps();
        *index.write().unwrap_or_else(|e| e.into_inner()) = idx;
        content_ready.store(true, Ordering::Release);
        crate::index::log_memory("serve: content ready");

        // Pre-warm trigram index in background to eliminate cold-start penalty
        let warmup_index = Arc::clone(&index);
        std::thread::spawn(move || {
            eprintln!("[warmup] Starting trigram pre-warm...");
            let start = Instant::now();
            let (trigrams, tokens) = warmup_index.read()
                .unwrap_or_else(|e| e.into_inner())
                .warm_up();
            eprintln!("[warmup] Trigram pre-warm completed in {:.1}ms ({} trigrams, {} tokens)",
                start.elapsed().as_secs_f64() * 1000.0, trigrams, tokens);
            crate::index::log_memory("serve: trigram warm-up done");
        });
    } else if !is_unresolved {
        // Build in background — don't block the event loop
        let bg_index = Arc::clone(&index);
        let bg_ready = Arc::clone(content_ready);
        let bg_building = Arc::clone(content_building);
        let bg_dir = dir_str.to_string();
        let bg_ext = exts_for_load.to_string();
        let bg_idx_base = idx_base.to_path_buf();

        std::thread::spawn(move || {
            bg_building.store(true, Ordering::Release);
            info!("Building content index in background...");
            crate::index::log_memory("content-build: starting");
            let build_start = Instant::now();
            let new_idx = match build_content_index(&ContentIndexArgs {
                dir: bg_dir.clone(),
                ext: bg_ext.clone(),
                max_age_hours: 24,
                hidden: false,
                no_ignore: false, respect_git_exclude,
                threads: 0,
                min_token_len: DEFAULT_MIN_TOKEN_LEN,
            }) {
                Ok(idx) => idx,
                Err(e) => {
                    warn!(error = %e, "Failed to build content index");
                    bg_building.store(false, Ordering::Release);
                    bg_ready.store(true, Ordering::Release);
                    return;
                }
            };
            crate::index::log_memory("content-build: finished");
            if let Err(e) = save_content_index(&new_idx, &bg_idx_base) {
                warn!(error = %e, "Failed to save content index to disk");
            } else {
                // Clean up old content indexes for the same root with different extensions
                let exts_str = new_idx.extensions.join(",");
                let saved_path = crate::content_index_path_for(&new_idx.root, &exts_str, &bg_idx_base);
                crate::index::cleanup_stale_same_root_indexes(&bg_idx_base, &saved_path, &new_idx.root, "word-search");
            }

            // Drop build-time index and reload from disk to eliminate allocator
            // fragmentation (~1.5 GB savings). Build creates many temporary allocs
            // that fragment the heap; reloading gives compact contiguous memory.
            let file_count = new_idx.files.len();
            let token_count = new_idx.index.len();
            drop(new_idx);
            crate::index::log_memory("serve: after drop(content build)");
            crate::index::force_mimalloc_collect();
            crate::index::log_memory("serve: after mi_collect (content)");
            let new_idx = match load_content_index(&bg_dir, &bg_ext, &bg_idx_base) {
                Ok(idx) => idx,
                Err(e) => {
                    warn!(error = %e, "Failed to reload content index from disk, rebuilding");
                    match build_content_index(&ContentIndexArgs {
                        dir: bg_dir, ext: bg_ext,
                        max_age_hours: 24, hidden: false, no_ignore: false, respect_git_exclude,
                        threads: 0, min_token_len: DEFAULT_MIN_TOKEN_LEN,
                    }) {
                        Ok(idx) => idx,
                        Err(e2) => {
                            warn!(error = %e2, "Failed to rebuild content index from scratch");
                            bg_building.store(false, Ordering::Release);
                            bg_ready.store(true, Ordering::Release);
                            return;
                        }
                    }
                }
            };

            crate::index::log_memory("serve: after reload content from disk");
            let mut new_idx = if watch {
                mcp::watcher::build_watch_index_from(new_idx)
            } else {
                new_idx
            };
            new_idx.shrink_maps();
            let elapsed = build_start.elapsed();
            info!(
                elapsed_ms = format_args!("{:.1}", elapsed.as_secs_f64() * 1000.0),
                files = file_count,
                tokens = token_count,
                "Content index ready (background build complete)"
            );
            *bg_index.write().unwrap_or_else(|e| e.into_inner()) = new_idx;
            bg_building.store(false, Ordering::Release);
            bg_ready.store(true, Ordering::Release);
            crate::index::log_memory("serve: content ready");

            // Pre-warm trigram index after background build
            eprintln!("[warmup] Starting trigram pre-warm...");
            let warmup_start = Instant::now();
            let (trigrams, tokens) = bg_index.read()
                .unwrap_or_else(|e| e.into_inner())
                .warm_up();
            eprintln!("[warmup] Trigram pre-warm completed in {:.1}ms ({} trigrams, {} tokens)",
                warmup_start.elapsed().as_secs_f64() * 1000.0, trigrams, tokens);
        });
    }

    (index, effective_respect_git_exclude)
}

#[allow(clippy::too_many_arguments)]
fn load_or_build_definition_index(
    dir_str: &str,
    extensions: &[String],
    idx_base: &Path,
    exts_for_load: &str,
    is_unresolved: bool,
    definitions_enabled: bool,
    def_ready: &Arc<AtomicBool>,
    def_building: &Arc<AtomicBool>,
    respect_git_exclude: bool,
) -> (Option<Arc<RwLock<definitions::DefinitionIndex>>>, Vec<String>, bool) {
    // Use compile-time definition extensions based on enabled Cargo features
    let supported_def_langs = definitions::definition_extensions();
    let def_exts = supported_def_langs.iter()
        .filter(|lang| extensions.iter().any(|e| e.eq_ignore_ascii_case(lang)))
        .copied()
        .collect::<Vec<&str>>()
        .join(",");

    // Warn about unsupported extensions requested via --ext
    for ext in extensions {
        let has_parser = supported_def_langs.iter().any(|lang| lang.eq_ignore_ascii_case(ext));
        if !has_parser && ["cs", "ts", "tsx", "sql"].iter().any(|known| known.eq_ignore_ascii_case(ext)) {
            warn!(
                ext = %ext,
                compiled_parsers = ?supported_def_langs,
                "Extension '{}' requested via --ext but its parser is not compiled in this build. \
                 Rebuild with the appropriate feature flag (e.g., --features lang-csharp) to enable it.",
                ext
            );
        }
    }

    let def_exts = if def_exts.is_empty() {
        // No overlap between --ext and compiled parsers — fall back to first compiled parser
        // or "cs" if none are compiled (will produce empty index)
        if let Some(first) = supported_def_langs.first() {
            first.to_string()
        } else {
            "cs".to_string()
        }
    } else {
        def_exts
    };

    // Compute def_extensions for dynamic tool descriptions.
    // IMPORTANT: use the RAW intersection of --ext and definition_extensions(),
    // NOT the post-fallback def_exts. The fallback ("cs" when no overlap) is
    // for index building, but tool descriptions must reflect actual languages.
    // This matches what server.rs does for render_instructions in initialize.
    let server_exts_for_desc: Vec<&str> = exts_for_load.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let def_extensions_vec: Vec<String> = if definitions_enabled {
        supported_def_langs.iter()
            .filter(|lang| server_exts_for_desc.iter().any(|e| e.eq_ignore_ascii_case(lang)))
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    // Default to the CLI flag; persisted value (when an index loads) wins via
    // `resolve_respect_git_exclude` so reindex / watcher reconcile do not lose
    // the user's original opt-in.
    let mut effective_respect_git_exclude = respect_git_exclude;

    let def_index = if definitions_enabled {
        // Create an empty DefinitionIndex placeholder
        let empty_def = definitions::DefinitionIndex {
            root: dir_str.to_string(),
            format_version: 0,
            created_at: 0,
            extensions: def_exts.split(',').map(|s| s.to_string()).collect(),
            files: Vec::new(),
            definitions: Vec::new(),
            name_index: HashMap::new(),
            kind_index: HashMap::new(),
            attribute_index: HashMap::new(),
            base_type_index: HashMap::new(),
            file_index: HashMap::new(),
            path_to_id: HashMap::new(),
            method_calls: HashMap::new(),
            code_stats: HashMap::new(),
            parse_errors: 0,
            lossy_file_count: 0,
            worker_panics: 0,
            empty_file_ids: Vec::new(),
            extension_methods: HashMap::new(),
            selector_index: HashMap::new(),
            template_children: HashMap::new(),
            respect_git_exclude,
        };
        let def_arc = Arc::new(RwLock::new(empty_def));

        // Try fast load from disk
        let def_start = Instant::now();
        let def_ext_vec: Vec<String> = def_exts.split(',').map(|s| s.to_string()).collect();
        let def_direct = if is_unresolved { None } else {
            definitions::load_definition_index(dir_str, &def_exts, idx_base).ok()
        };
        let def_load_method;
        let def_loaded = if def_direct.is_some() {
            def_load_method = "direct";
            def_direct
        } else {
            def_load_method = "fallback";
            if is_unresolved { None } else { definitions::find_definition_index_for_dir(dir_str, idx_base, &def_ext_vec) }
        };

        if let Some(idx) = def_loaded {
            let def_elapsed = def_start.elapsed();
            let cache_age = format_cache_age(idx.created_at);
            let def_file_count = idx.files.len();
            let def_count = idx.definitions.len();
            effective_respect_git_exclude = super::resolve_respect_git_exclude(
                "serve/definitions",
                Some(idx.respect_git_exclude),
                respect_git_exclude,
            );
            info!(
                elapsed_ms = format_args!("{:.1}", def_elapsed.as_secs_f64() * 1000.0),
                definitions = def_count,
                files = def_file_count,
                cache_age = %cache_age,
                "Definition index loaded from disk"
            );
            crate::index::log_memory(&format!(
                "serve: def loaded [{}] (files={}, defs={}, calls={}, age={})",
                def_load_method, def_file_count, def_count,
                idx.method_calls.len(), cache_age
            ));
            let mut idx = idx;
            idx.shrink_maps();
            *def_arc.write().unwrap_or_else(|e| e.into_inner()) = idx;
            def_ready.store(true, Ordering::Release);
        } else if !is_unresolved {
            // Build in background
            let bg_def = Arc::clone(&def_arc);
            let bg_def_ready = Arc::clone(def_ready);
            let bg_def_building = Arc::clone(def_building);
            let bg_dir = dir_str.to_string();
            let bg_def_exts = def_exts.clone();
            let bg_idx_base = idx_base.to_path_buf();

            std::thread::spawn(move || {
                bg_def_building.store(true, Ordering::Release);
                info!("Building definition index in background...");
                crate::index::log_memory("def-build: starting");
                let build_start = Instant::now();
                let new_idx = definitions::build_definition_index(&definitions::DefIndexArgs {
                    dir: bg_dir.clone(),
                    ext: bg_def_exts.clone(),
                    threads: 0,
                    respect_git_exclude,
                });
                crate::index::log_memory("def-build: finished");
                if let Err(e) = definitions::save_definition_index(&new_idx, &bg_idx_base) {
                    warn!(error = %e, "Failed to save definition index to disk");
                } else {
                    // Clean up old definition indexes for the same root with different extensions
                    let exts_str = new_idx.extensions.join(",");
                    let saved_path = definitions::definition_index_path_for(&new_idx.root, &exts_str, &bg_idx_base);
                    crate::index::cleanup_stale_same_root_indexes(&bg_idx_base, &saved_path, &new_idx.root, "code-structure");
                }

                // Drop + reload to eliminate allocator fragmentation (same pattern)
                let def_count = new_idx.definitions.len();
                let file_count = new_idx.files.len();
                drop(new_idx);
                crate::index::log_memory("serve: after drop(def build)");
                crate::index::force_mimalloc_collect();
                crate::index::log_memory("serve: after mi_collect (def)");
                let new_idx = definitions::load_definition_index(&bg_dir, &bg_def_exts, &bg_idx_base)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "Failed to reload definition index from disk, rebuilding");
                        definitions::build_definition_index(&definitions::DefIndexArgs {
                            dir: bg_dir, ext: bg_def_exts, threads: 0,
                            respect_git_exclude,
                        })
                    });

                crate::index::log_memory("serve: after reload def from disk");
                let elapsed = build_start.elapsed();
                info!(
                    elapsed_ms = format_args!("{:.1}", elapsed.as_secs_f64() * 1000.0),
                    definitions = def_count,
                    files = file_count,
                    "Definition index ready (background build complete)"
                );
                let mut new_idx = new_idx;
                new_idx.shrink_maps();
                *bg_def.write().unwrap_or_else(|e| e.into_inner()) = new_idx;
                bg_def_building.store(false, Ordering::Release);
                bg_def_ready.store(true, Ordering::Release);
                crate::index::log_memory("serve: def ready");
            });
        }

        Some(def_arc)
    } else {
        // No --definitions flag: mark as ready (N/A)
        def_ready.store(true, Ordering::Release);
        None
    };

    (def_index, def_extensions_vec, effective_respect_git_exclude)
}

fn build_git_cache_background(
    dir_str: &str,
    idx_base: &Path,
    is_unresolved: bool,
) -> (Arc<RwLock<Option<GitHistoryCache>>>, Arc<AtomicBool>) {
    let git_cache: Arc<RwLock<Option<GitHistoryCache>>> = Arc::new(RwLock::new(None));
    let git_cache_ready = Arc::new(AtomicBool::new(false));

    if !is_unresolved {
        let bg_git_cache = Arc::clone(&git_cache);
        let bg_git_ready = Arc::clone(&git_cache_ready);
        let bg_dir = dir_str.to_string();
        let bg_idx_base = idx_base.to_path_buf();

        std::thread::spawn(move || {
            let start = Instant::now();
            crate::index::log_memory("git-cache: starting");
            eprintln!("[git-cache] Initializing for {}...", bg_dir);

            // Determine repo path — check if it's a git repository
            let repo_path = PathBuf::from(&bg_dir);
            let git_dir = repo_path.join(".git");
            if !git_dir.exists() {
                eprintln!("[git-cache] No .git directory found, skipping");
                bg_git_ready.store(true, Ordering::Release);
                return;
            }

            // Detect default branch
            let branch = match GitHistoryCache::detect_default_branch(&repo_path) {
                Ok(b) => {
                    eprintln!("[git-cache] Detected branch: {}", b);
                    b
                }
                Err(e) => {
                    warn!(error = %e, "Failed to detect default branch, skipping git cache");
                    bg_git_ready.store(true, Ordering::Release);
                    return;
                }
            };

            let cache_path = GitHistoryCache::cache_path_for(&bg_dir, &bg_idx_base);

            // Try to load cache from disk
            let cache = if cache_path.exists() {
                match GitHistoryCache::load_from_disk(&cache_path) {
                    Ok(disk_cache) => {
                        // Check if cached HEAD object still exists (re-clone detection)
                        if !GitHistoryCache::object_exists(&repo_path, &disk_cache.head_hash) {
                            info!("Cached HEAD object not found (repo re-cloned?), full rebuild");
                            None
                        } else {
                            // Check current HEAD
                            match std::process::Command::new("git")
                                .args(["rev-parse", &branch])
                                .current_dir(&repo_path)
                                .output()
                            {
                                Ok(output) if output.status.success() => {
                                    let current_head = String::from_utf8_lossy(&output.stdout).trim().to_string();
                                    if disk_cache.is_valid_for(&current_head) {
                                        // Cache is up to date
                                        let elapsed = start.elapsed();
                                        info!(
                                            elapsed_ms = format_args!("{:.1}", elapsed.as_secs_f64() * 1000.0),
                                            commits = disk_cache.commits.len(),
                                            files = disk_cache.file_commits.len(),
                                            "Git cache loaded from disk (HEAD matches)"
                                        );
                                        Some(disk_cache)
                                    } else if GitHistoryCache::is_ancestor(&repo_path, &disk_cache.head_hash, &current_head) {
                                        // Fast-forward: full rebuild for MVP simplicity
                                        // (incremental update would be faster but adds complexity)
                                        info!(
                                            old_head = %&disk_cache.head_hash[..8],
                                            new_head = %&current_head[..current_head.len().min(8)],
                                            "HEAD changed (fast-forward), rebuilding git cache"
                                        );
                                        None
                                    } else {
                                        // Force push / rebase — full rebuild
                                        info!(
                                            old_head = %&disk_cache.head_hash[..8],
                                            new_head = %&current_head[..current_head.len().min(8)],
                                            "HEAD changed (not ancestor), full rebuild"
                                        );
                                        None
                                    }
                                }
                                _ => {
                                    warn!("Failed to get current HEAD, full rebuild");
                                    None
                                }
                            }
                        }
                    }
                    Err(e) => {
                        info!(error = %e, "Failed to load git cache from disk, full rebuild");
                        None
                    }
                }
            } else {
                None
            };

            // If we got a valid cache from disk, publish it; otherwise build from scratch
            let cache = match cache {
                Some(c) => c,
                None => {
                    eprintln!("[git-cache] Building cache for branch '{}' (this may take a few minutes for large repos)...", branch);
                    match GitHistoryCache::build(&repo_path, &branch) {
                        Ok(new_cache) => {
                            // Save to disk
                            if let Err(e) = new_cache.save_to_disk(&cache_path) {
                                warn!(error = %e, "Failed to save git cache to disk");
                            }
                            new_cache
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to build git cache");
                            bg_git_ready.store(true, Ordering::Release);
                            return;
                        }
                    }
                }
            };

            let commit_count = cache.commits.len();
            let file_count = cache.file_commits.len();

            // Publish: write to Arc<RwLock<Option<GitHistoryCache>>>
            if let Ok(mut guard) = bg_git_cache.write() {
                *guard = Some(cache);
            }
            bg_git_ready.store(true, Ordering::Release);

            let elapsed = start.elapsed();
            crate::index::log_memory("git-cache: ready");
            eprintln!(
                "[git-cache] Ready: {} commits, {} files in {:.1}s",
                commit_count, file_count, elapsed.as_secs_f64()
            );
        });
    }

    (git_cache, git_cache_ready)
}

/// Format a cache age as a human-readable string (e.g., "2h 15m", "5m", "3d 1h").
/// `created_at` is seconds since UNIX epoch.
fn format_cache_age(created_at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age_secs = now.saturating_sub(created_at);

    if age_secs < 60 {
        format!("{}s", age_secs)
    } else if age_secs < 3600 {
        format!("{}m", age_secs / 60)
    } else if age_secs < 86400 {
        let hours = age_secs / 3600;
        let mins = (age_secs % 3600) / 60;
        if mins > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}h", hours)
        }
    } else {
        let days = age_secs / 86400;
        let hours = (age_secs % 86400) / 3600;
        if hours > 0 {
            format!("{}d {}h", days, hours)
        } else {
            format!("{}d", days)
        }
    }
}


#[cfg(test)]
mod serve_format_cache_age_tests {
    use super::format_cache_age;

    /// Helper: convert a desired age (seconds) into a fake `created_at`
    /// timestamp such that `format_cache_age(ts)` will report exactly that age.
    fn ts_for_age(age_secs: u64) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now.saturating_sub(age_secs)
    }

    /// E1 — `format_cache_age` must produce the expected human-readable string
    /// at every boundary of the seconds → minutes → hours → days transitions.
    /// Regression: refactoring the if/else ladder into separate fns risks
    /// off-by-one at boundaries (e.g., `<=` vs `<`, or returning "60s" instead
    /// of "1m" at exactly 60 seconds).
    #[test]
    fn test_format_cache_age_boundaries() {
        // Sub-minute: returns Ns
        assert_eq!(format_cache_age(ts_for_age(0)),  "0s",  "0 seconds → '0s'");
        assert_eq!(format_cache_age(ts_for_age(1)),  "1s",  "1 second → '1s'");
        assert_eq!(format_cache_age(ts_for_age(59)), "59s", "59 seconds → '59s' (just under minute boundary)");

        // Minute boundary: 60s → '1m', NOT '60s'
        assert_eq!(format_cache_age(ts_for_age(60)),   "1m",  "60 seconds → '1m' (minute boundary)");
        assert_eq!(format_cache_age(ts_for_age(61)),   "1m",  "61 seconds → '1m' (truncates seconds)");
        assert_eq!(format_cache_age(ts_for_age(120)),  "2m",  "120 seconds → '2m'");
        assert_eq!(format_cache_age(ts_for_age(3599)), "59m", "3599 seconds → '59m' (just under hour boundary)");

        // Hour boundary: 3600s → '1h', NOT '60m'
        assert_eq!(format_cache_age(ts_for_age(3600)), "1h",      "3600 seconds → '1h' (hour boundary, no minutes)");
        assert_eq!(format_cache_age(ts_for_age(3660)), "1h 1m",   "3660 seconds → '1h 1m' (hour + remainder minutes)");
        assert_eq!(format_cache_age(ts_for_age(7320)), "2h 2m",   "7320 seconds → '2h 2m'");
        assert_eq!(format_cache_age(ts_for_age(86_399)), "23h 59m", "86399 seconds → '23h 59m' (just under day boundary)");

        // Day boundary: 86400s → '1d', NOT '24h'
        assert_eq!(format_cache_age(ts_for_age(86_400)),  "1d",     "86400 seconds → '1d' (day boundary, no hours)");
        assert_eq!(format_cache_age(ts_for_age(90_000)),  "1d 1h",  "90000 seconds → '1d 1h' (day + remainder hours)");
        assert_eq!(format_cache_age(ts_for_age(176_400)), "2d 1h",  "176400 seconds → '2d 1h'");

        // Far future: created_at in the future yields age=0 via saturating_sub
        let future_ts = ts_for_age(0).saturating_add(10_000);
        assert_eq!(format_cache_age(future_ts), "0s",
            "Future timestamps must clamp to '0s' (regression: integer underflow on now - future)");
    }
}


#[cfg(test)]
mod serve_respect_git_exclude_tests {
    //! Regression tests for the serve-path Bug 4 follow-up
    //! (`docs/user-stories/todo_approved_2026-04-24_respect-git-exclude-serve-persisted-semantics.md`).
    //!
    //! Pin down that `load_or_build_content_index` and
    //! `load_or_build_definition_index` resolve the effective
    //! `respect_git_exclude` against the **persisted** index value, not the
    //! raw CLI flag. Without this, a `xray serve` invocation without the
    //! flag would silently drop the user's original opt-in once the
    //! watcher / periodic rescan / MCP reindex paths re-walked the tree.

    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use crate::{build_content_index, save_content_index};
    use super::ContentIndexArgs;

    fn init_repo_with_exclude(dir: &std::path::Path, pattern: &str) {
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir)
            .output()
            .expect("git init");
        let info = dir.join(".git").join("info");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("exclude"), format!("{}\n", pattern)).unwrap();
    }

    /// Regression: `xray serve` without `--respect-git-exclude` must NOT
    /// silently downgrade a persisted content index that was originally
    /// built with the flag. The effective value returned from
    /// `load_or_build_content_index` is what cmd_serve threads into the
    /// watcher, periodic rescan, and HandlerContext — if it dropped to
    /// `false` here, every reconcile path would re-add `.git/info/exclude`
    /// files to the index.
    #[test]
    fn serve_load_preserves_persisted_respect_git_exclude_for_content_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo_with_exclude(dir, "secret.cs");
        std::fs::write(dir.join("public.cs"), "fn marker_public() {}").unwrap();
        std::fs::write(dir.join("secret.cs"), "fn marker_secret() {}").unwrap();

        let idx_base = dir.join("idx");
        std::fs::create_dir_all(&idx_base).unwrap();

        // One-time CLI build with the flag honoured.
        let original = build_content_index(&ContentIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            ext: "cs".to_string(),
            respect_git_exclude: true,
            threads: 1,
            ..Default::default()
        }).unwrap();
        save_content_index(&original, &idx_base).unwrap();
        assert!(original.respect_git_exclude, "sanity: built index has flag=true");

        // Now simulate `xray serve` WITHOUT --respect-git-exclude (CLI = false).
        let extensions = vec!["cs".to_string()];
        let content_ready = Arc::new(AtomicBool::new(false));
        let content_building = Arc::new(AtomicBool::new(false));

        let (_index, effective) = load_or_build_content_index(
            &dir.to_string_lossy(),
            "cs",
            &extensions,
            &idx_base,
            false, // is_unresolved
            false, // watch
            false, // CLI flag — was true at build time, now false
            &content_ready,
            &content_building,
        );

        assert!(
            effective,
            "effective respect_git_exclude must be true (persisted wins over CLI false)"
        );
        assert!(
            content_ready.load(Ordering::Acquire),
            "content_ready must be set after a synchronous load"
        );
    }

    /// Symmetric test for the definition index. Gated on `lang-csharp` because
    /// the existing test fixture relies on a C# parser; the persistence /
    /// resolve logic is identical for any language.
    #[test]
    #[cfg(feature = "lang-csharp")]
    fn serve_load_preserves_persisted_respect_git_exclude_for_definition_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo_with_exclude(dir, "secret.cs");
        std::fs::write(dir.join("public.cs"), "public class PublicService {}").unwrap();
        std::fs::write(dir.join("secret.cs"), "public class SecretService {}").unwrap();

        let idx_base = dir.join("idx");
        std::fs::create_dir_all(&idx_base).unwrap();

        let original = crate::definitions::build_definition_index(&crate::definitions::DefIndexArgs {
            dir: dir.to_string_lossy().to_string(),
            ext: "cs".to_string(),
            threads: 1,
            respect_git_exclude: true,
        });
        crate::definitions::save_definition_index(&original, &idx_base).unwrap();
        assert!(original.respect_git_exclude, "sanity: built def index has flag=true");

        let extensions = vec!["cs".to_string()];
        let def_ready = Arc::new(AtomicBool::new(false));
        let def_building = Arc::new(AtomicBool::new(false));

        let (_def_index, _def_extensions, effective) = load_or_build_definition_index(
            &dir.to_string_lossy(),
            &extensions,
            &idx_base,
            "cs",
            false, // is_unresolved
            true,  // definitions_enabled
            &def_ready,
            &def_building,
            false, // CLI flag — was true at build time, now false
        );

        assert!(
            effective,
            "effective respect_git_exclude must be true for definition index (persisted wins)"
        );
        assert!(
            def_ready.load(Ordering::Acquire),
            "def_ready must be set after a synchronous load"
        );
    }

    /// Sanity check: when no persisted index exists yet (cold start without
    /// `--respect-git-exclude`), the effective value falls through to the CLI
    /// flag. This guards against regressions where `resolve_respect_git_exclude`
    /// is mistakenly called with `Some(false)` instead of `None` on cache-miss.
    #[test]
    fn serve_cold_start_uses_cli_flag_when_no_persisted_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // No git repo, no pre-existing index.
        std::fs::write(dir.join("a.cs"), "public class A {}").unwrap();

        let idx_base = dir.join("idx");
        std::fs::create_dir_all(&idx_base).unwrap();

        let extensions = vec!["cs".to_string()];
        let content_ready = Arc::new(AtomicBool::new(false));
        let content_building = Arc::new(AtomicBool::new(false));

        let (_index, effective) = load_or_build_content_index(
            &dir.to_string_lossy(),
            "cs",
            &extensions,
            &idx_base,
            false, // is_unresolved
            false, // watch
            true,  // CLI flag
            &content_ready,
            &content_building,
        );

        assert!(effective, "cold start: CLI flag passes through unchanged");
    }
}

