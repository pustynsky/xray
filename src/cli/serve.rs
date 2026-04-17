//! MCP server startup and configuration.

use std::path::PathBuf;
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

    let idx_base = index_dir();

    // Enable debug logging if --debug-log was passed
    if args.debug_log {
        crate::index::enable_debug_log(&idx_base, &dir_str);
    }
    crate::index::log_memory("serve: startup");

    // Log CWD and canonical dir for debugging dir-agnostic mode
    let cwd = std::env::current_dir().map(|p| crate::clean_path(&p.to_string_lossy())).unwrap_or_else(|_| "<unknown>".to_string());
    let canonical_dir = std::fs::canonicalize(&dir_str).map(|p| crate::clean_path(&p.to_string_lossy())).unwrap_or_else(|_| dir_str.clone());
    eprintln!("[startup] dir={:?}, cwd={}, canonical={}", dir_str, cwd, canonical_dir);

    // ─── Async startup: create empty indexes, start event loop immediately ───
    use std::collections::HashMap;

    let content_ready = Arc::new(AtomicBool::new(false));
    let def_ready = Arc::new(AtomicBool::new(false));
    let file_index_dirty = Arc::new(AtomicBool::new(true));
    let content_building = Arc::new(AtomicBool::new(false));
    let def_building = Arc::new(AtomicBool::new(false));

    // ─── Workspace binding: determine BEFORE index building ───
    // This prevents building indexes for wrong directory (e.g., VS Code install dir)
    // when server starts with --dir . and CWD has no source files.
    let extensions_vec: Vec<String> = exts_for_load.split(',').map(|s| s.to_string()).collect();
    let ws_binding = mcp::handlers::determine_initial_binding(&dir_str, &extensions_vec);
    let is_unresolved = ws_binding.status == mcp::handlers::WorkspaceStatus::Unresolved;
    if is_unresolved {
        eprintln!("[startup] Workspace UNRESOLVED (no source files in '{}'), skipping index build", ws_binding.dir);
    }

    // Create an empty ContentIndex so the event loop can start immediately
    let empty_index = ContentIndex {
        root: dir_str.clone(),
        format_version: 0,
        created_at: 0,
        max_age_secs: 86400,
        files: Vec::new(),
        index: HashMap::new(),
        total_tokens: 0,
        extensions: extensions.clone(),
        file_token_counts: Vec::new(),
        trigram: TrigramIndex::default(),
        trigram_dirty: false,
        path_to_id: if args.watch { Some(HashMap::new()) } else { None },
        read_errors: 0,
        lossy_file_count: 0,
    };
    let index = Arc::new(RwLock::new(empty_index));

    // Try fast load from disk (typically < 3s)
    // Skip if workspace is Unresolved — no point loading/building indexes for wrong directory.
    let start = Instant::now();
    let direct_load = if is_unresolved { None } else {
        load_content_index(&dir_str, &exts_for_load, &idx_base).ok()
    };
    let load_method;
    let loaded = if direct_load.is_some() {
        load_method = "direct";
        direct_load
    } else {
        load_method = "fallback";
        if is_unresolved { None } else { find_content_index_for_dir(&dir_str, &idx_base, &extensions) }
    };

    if let Some(idx) = loaded {
        let load_elapsed = start.elapsed();
        let cache_age = format_cache_age(idx.created_at);
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
        let mut idx = if args.watch {
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
        let bg_index: Arc<RwLock<ContentIndex>> = Arc::clone(&index);
        let bg_ready = Arc::clone(&content_ready);
        let bg_building = Arc::clone(&content_building);
        let bg_dir = dir_str.clone();
        let bg_ext = exts_for_load.clone();
        let bg_idx_base = idx_base.clone();
        let bg_watch = args.watch;

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
                no_ignore: false, respect_git_exclude: args.respect_git_exclude,
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
                        max_age_hours: 24, hidden: false, no_ignore: false, respect_git_exclude: args.respect_git_exclude,
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
            let mut new_idx = if bg_watch {
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

    // ─── Definition index: same async pattern ───
    // Use compile-time definition extensions based on enabled Cargo features
    let supported_def_langs = definitions::definition_extensions();
    let def_exts = supported_def_langs.iter()
        .filter(|lang| extensions.iter().any(|e| e.eq_ignore_ascii_case(lang)))
        .copied()
        .collect::<Vec<&str>>()
        .join(",");

    // Warn about unsupported extensions requested via --ext
    for ext in &extensions {
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

    let def_index = if args.definitions {
        // Create an empty DefinitionIndex placeholder
        let empty_def = definitions::DefinitionIndex {
            root: dir_str.clone(),
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
            empty_file_ids: Vec::new(),
            extension_methods: HashMap::new(),
            selector_index: HashMap::new(),
            template_children: HashMap::new(),
        };
        let def_arc = Arc::new(RwLock::new(empty_def));

        // Try fast load from disk
        let def_start = Instant::now();
        let def_ext_vec: Vec<String> = def_exts.split(',').map(|s| s.to_string()).collect();
        let def_direct = if is_unresolved { None } else {
            definitions::load_definition_index(&dir_str, &def_exts, &idx_base).ok()
        };
        let def_load_method;
        let def_loaded = if def_direct.is_some() {
            def_load_method = "direct";
            def_direct
        } else {
            def_load_method = "fallback";
            if is_unresolved { None } else { definitions::find_definition_index_for_dir(&dir_str, &idx_base, &def_ext_vec) }
        };

        if let Some(idx) = def_loaded {
            let def_elapsed = def_start.elapsed();
            let cache_age = format_cache_age(idx.created_at);
            let def_file_count = idx.files.len();
            let def_count = idx.definitions.len();
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
            let bg_def_ready = Arc::clone(&def_ready);
            let bg_def_building = Arc::clone(&def_building);
            let bg_dir = dir_str.clone();
            let bg_def_exts = def_exts.clone();
            let bg_idx_base = idx_base.clone();

            std::thread::spawn(move || {
                bg_def_building.store(true, Ordering::Release);
                info!("Building definition index in background...");
                crate::index::log_memory("def-build: starting");
                let build_start = Instant::now();
                let new_idx = definitions::build_definition_index(&definitions::DefIndexArgs {
                    dir: bg_dir.clone(),
                    ext: bg_def_exts.clone(),
                    threads: 0,
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

    // Start file watcher if --watch (only after content index is available)
    // Watcher works fine with an empty index — it will update it as files change.
    let watcher_generation = Arc::new(AtomicU64::new(0));
    if args.watch && !is_unresolved {
        let watch_dir = std::fs::canonicalize(&dir_str)
            .unwrap_or_else(|_| PathBuf::from(&dir_str));
        crate::index::log_memory("serve: starting watcher");
        if let Err(e) = mcp::watcher::start_watcher(
            Arc::clone(&index),
            def_index.as_ref().map(Arc::clone),
            watch_dir,
            extensions,
            args.debounce_ms,
            idx_base.clone(),
            Arc::clone(&content_ready),
            Arc::clone(&def_ready),
            Arc::clone(&file_index_dirty),
            Arc::clone(&watcher_generation),
            0, // initial generation
        ) {
            warn!(error = %e, "Failed to start file watcher");
        }
    }

    // ─── Git history cache: background build ───
    let git_cache: Arc<RwLock<Option<GitHistoryCache>>> = Arc::new(RwLock::new(None));
    let git_cache_ready = Arc::new(AtomicBool::new(false));

    if !is_unresolved {
        let bg_git_cache = Arc::clone(&git_cache);
        let bg_git_ready = Arc::clone(&git_cache_ready);
        let bg_dir = dir_str.clone();
        let bg_idx_base = idx_base.clone();

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
    // Compute def_extensions for dynamic tool descriptions.
    // IMPORTANT: use the RAW intersection of --ext and definition_extensions(),
    // NOT the post-fallback def_exts. The fallback ("cs" when no overlap) is
    // for index building, but tool descriptions must reflect actual languages.
    // This matches what server.rs does for render_instructions in initialize.
    // Parse server ext from exts_for_load (String) since extensions (Vec<String>)
    // was moved into start_watcher above.
    let server_exts_for_desc: Vec<&str> = exts_for_load.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let def_extensions_vec: Vec<String> = if args.definitions {
        supported_def_langs.iter()
            .filter(|lang| server_exts_for_desc.iter().any(|e| e.eq_ignore_ascii_case(lang)))
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let file_index = Arc::new(RwLock::new(None));

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
        respect_git_exclude: args.respect_git_exclude,
    };
    mcp::server::run_server(ctx);
    crate::index::log_memory("serve: event loop exited");
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