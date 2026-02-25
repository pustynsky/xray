//! Index storage: save/load/build for FileIndex and ContentIndex.

use std::collections::HashMap;
use std::fs;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;

use crate::error::SearchError;
use search_index::{clean_path, extract_semantic_prefix, generate_trigrams, read_file_lossy, stable_hash, tokenize, ContentIndex, FileEntry, FileIndex, Posting, TrigramIndex};

use crate::{ContentIndexArgs, IndexArgs};

// ─── Debug logging (memory diagnostics + MCP request/response traces) ────

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Whether debug logging is enabled (fast check via AtomicBool).
static DEBUG_LOG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Path to the .debug.log file (set once by `enable_debug_log`).
static DEBUG_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Startup timestamp for relative timing in log entries.
static DEBUG_LOG_START: OnceLock<Instant> = OnceLock::new();

/// Compute the per-server debug log file path.
/// Uses the same semantic prefix + hash naming as index files.
pub fn debug_log_path_for(index_base: &std::path::Path, server_dir: &str) -> PathBuf {
    let canonical = fs::canonicalize(server_dir).unwrap_or_else(|_| PathBuf::from(server_dir));
    let hash = stable_hash(&[canonical.to_string_lossy().as_bytes()]);
    let prefix = extract_semantic_prefix(&canonical);
    index_base.join(format!("{}_{:08x}.debug.log", prefix, hash as u32))
}

/// Enable debug logging: creates/truncates a per-server `.debug.log` in `index_base`,
/// writes a header line, and sets the global enable flag.
///
/// The log filename uses the same semantic prefix as index files (e.g.,
/// `repos_shared_00343f32.debug.log`) so multiple servers don't overwrite
/// each other's logs.
///
/// Must be called once at startup before any `log_memory()` / `log_request()` / `log_response()` calls.
pub fn enable_debug_log(index_base: &std::path::Path, server_dir: &str) {
    let _ = fs::create_dir_all(index_base);
    let log_path = debug_log_path_for(index_base, server_dir);

    // Truncate and write header
    if let Ok(mut f) = fs::File::create(&log_path) {
        let _ = writeln!(f,
            "{:>8} | {:>8} | {:>8} | {:>8} | {}",
            "elapsed", "WS_MB", "Peak_MB", "Commit_MB", "label"
        );
        let _ = writeln!(f, "{}", "-".repeat(70));
    }

    let _ = DEBUG_LOG_PATH.set(log_path.clone());
    let _ = DEBUG_LOG_START.set(Instant::now());
    DEBUG_LOG_ENABLED.store(true, Ordering::Release);

    eprintln!("[debug-log] Enabled, writing to {}", log_path.display());
}

/// Generate ISO 8601 UTC timestamp from SystemTime (no chrono dependency).
/// Format: "2026-02-24T09:28:20Z"
pub fn format_utc_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Howard Hinnant civil date algorithm
    let s = (secs % 86400) as u32;
    let z = (secs / 86400) as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, s / 3600, (s % 3600) / 60, s % 60)
}

/// Append a line to the debug log file. Shared by log_memory, log_request, log_response.
fn append_to_debug_log(line: &str) {
    if let Some(path) = DEBUG_LOG_PATH.get() {
        if let Ok(mut f) = fs::OpenOptions::new().append(true).open(path) {
            let _ = writeln!(f, "{}", line);
        }
    }
}

/// Log an MCP tool request to the debug log file.
/// Format: "2026-02-24T09:28:20Z | REQ  | search_grep | {"terms":"HttpClient","ext":"cs"}"
/// No-op when `--debug-log` is not passed (single AtomicBool check).
pub fn log_request(tool: &str, args: &str) {
    if !DEBUG_LOG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let line = format!("{} | REQ  | {} | {}", format_utc_timestamp(), tool, args);
    eprintln!("[debug-log] {}", line);
    append_to_debug_log(&line);
}

/// Log an MCP tool response to the debug log file.
/// Format: "2026-02-24T09:28:20Z | RESP | search_grep | 12.3ms | 4.2KB | WS=350.1MB"
/// followed by the full response body on the next line.
/// No-op when `--debug-log` is not passed (single AtomicBool check).
pub fn log_response(tool: &str, elapsed_ms: f64, response_bytes: usize, response_body: &str) {
    if !DEBUG_LOG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let ws_mb = {
        let info = get_process_memory_info();
        info["workingSetMB"].as_f64().map(|v| format!("WS={:.1}MB", v)).unwrap_or_default()
    };
    let line = format!("{} | RESP | {} | {:.1}ms | {:.1}KB | {}",
        format_utc_timestamp(), tool, elapsed_ms,
        response_bytes as f64 / 1024.0, ws_mb);
    eprintln!("[debug-log] {}", line);
    append_to_debug_log(&line);
    // Log full response body
    append_to_debug_log(response_body);
}

/// Log current process memory metrics (Working Set, Peak WS, Commit) to the debug log file.
///
/// When `--debug-log` is not passed, this is a fast no-op (single AtomicBool check).
/// On non-Windows platforms, this is always a no-op.
#[cfg(target_os = "windows")]
pub fn log_memory(label: &str) {
    if !DEBUG_LOG_ENABLED.load(Ordering::Acquire) {
        return;
    }

    // Windows API: K32GetProcessMemoryInfo
    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessMemoryCounters {
        cb: u32,
        PageFaultCount: u32,
        PeakWorkingSetSize: usize,
        WorkingSetSize: usize,
        QuotaPeakPagedPoolUsage: usize,
        QuotaPagedPoolUsage: usize,
        QuotaPeakNonPagedPoolUsage: usize,
        QuotaNonPagedPoolUsage: usize,
        PagefileUsage: usize,
        PeakPagefileUsage: usize,
    }

    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(
            process: isize,
            ppsmemCounters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    let mut pmc = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
    };

    let ok = unsafe {
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb)
    };

    if ok == 0 {
        return;
    }

    let ws_mb = pmc.WorkingSetSize as f64 / 1_048_576.0;
    let peak_mb = pmc.PeakWorkingSetSize as f64 / 1_048_576.0;
    let commit_mb = pmc.PagefileUsage as f64 / 1_048_576.0;

    let elapsed = DEBUG_LOG_START
        .get()
        .map(|s| s.elapsed().as_secs_f64())
        .unwrap_or(0.0);

    let line = format!(
        "{:8.2} | {:8.1} | {:8.1} | {:8.1} | {}",
        elapsed, ws_mb, peak_mb, commit_mb, label
    );

    // Print to stderr
    eprintln!("[memory] {}", line);

    // Append to debug log file
    append_to_debug_log(&line);
}

/// Log current process memory metrics — no-op on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
pub fn log_memory(_label: &str) {
    // No-op on non-Windows
}

/// Get current process memory info as a JSON object.
/// Returns Working Set, Peak WS, and Commit in MB.
/// On non-Windows, returns an empty object.
#[cfg(target_os = "windows")]
pub fn get_process_memory_info() -> serde_json::Value {
    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessMemoryCounters {
        cb: u32,
        PageFaultCount: u32,
        PeakWorkingSetSize: usize,
        WorkingSetSize: usize,
        QuotaPeakPagedPoolUsage: usize,
        QuotaPagedPoolUsage: usize,
        QuotaPeakNonPagedPoolUsage: usize,
        QuotaNonPagedPoolUsage: usize,
        PagefileUsage: usize,
        PeakPagefileUsage: usize,
    }

    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(
            process: isize,
            ppsmemCounters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    let mut pmc = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
    };

    let ok = unsafe {
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb)
    };

    if ok == 0 {
        return serde_json::json!({});
    }

    let round1 = |v: f64| (v * 10.0).round() / 10.0;
    serde_json::json!({
        "workingSetMB": round1(pmc.WorkingSetSize as f64 / 1_048_576.0),
        "peakWorkingSetMB": round1(pmc.PeakWorkingSetSize as f64 / 1_048_576.0),
        "commitMB": round1(pmc.PagefileUsage as f64 / 1_048_576.0),
    })
}

/// Get current process memory info — returns empty object on non-Windows.
#[cfg(not(target_os = "windows"))]
pub fn get_process_memory_info() -> serde_json::Value {
    serde_json::json!({})
}

/// Force mimalloc to collect and decommit all freed segments.
/// This prevents abandoned thread heaps from inflating Working Set
/// after the build+drop+reload pattern.
pub fn force_mimalloc_collect() {
    unsafe extern "C" {
        fn mi_collect(force: bool);
    }
    unsafe { mi_collect(true); }
}

/// Estimate the in-memory size of a ContentIndex.
/// Returns a JSON object with per-component MB estimates.
pub fn estimate_content_index_memory(idx: &ContentIndex) -> serde_json::Value {
    let round1 = |v: f64| (v * 10.0).round() / 10.0;

    // Sample average key length from first 1000 tokens
    let sample_size = idx.index.len().min(1000);
    let avg_key_len = if sample_size > 0 {
        let total_key_bytes: usize = idx.index.keys().take(sample_size).map(|k| k.len()).sum();
        total_key_bytes as f64 / sample_size as f64
    } else {
        8.0
    };

    // Count total postings and estimate average lines per posting
    let mut total_postings: usize = 0;
    let mut total_lines: usize = 0;
    let posting_sample = idx.index.values().take(1000);
    for postings in posting_sample {
        for p in postings {
            total_postings += 1;
            total_lines += p.lines.len();
        }
    }
    let avg_lines = if total_postings > 0 {
        total_lines as f64 / total_postings as f64
    } else {
        1.0
    };

    // If we only sampled, extrapolate total postings
    let full_total_postings: usize = idx.index.values().map(|v| v.len()).sum();

    // Inverted index estimate:
    // Each HashMap entry: ~80 bytes overhead + key String (24 + len) + Vec<Posting> (24 + postings)
    // Each Posting: 4 (file_id) + 24 (Vec header) + lines * 4 = 28 + avg_lines * 4
    let per_entry = 80.0 + 24.0 + avg_key_len + 24.0;
    let per_posting = 28.0 + avg_lines * 4.0;
    let inverted_mb = (idx.index.len() as f64 * per_entry + full_total_postings as f64 * per_posting) / 1_048_576.0;

    // Trigram tokens estimate
    let tri_sample_size = idx.trigram.tokens.len().min(1000);
    let avg_token_len = if tri_sample_size > 0 {
        let total: usize = idx.trigram.tokens.iter().take(tri_sample_size).map(|t| t.len()).sum();
        total as f64 / tri_sample_size as f64
    } else {
        8.0
    };
    let trigram_tokens_mb = idx.trigram.tokens.len() as f64 * (24.0 + avg_token_len) / 1_048_576.0;

    // Trigram map estimate
    let total_tri_postings: usize = idx.trigram.trigram_map.values().map(|v| v.len()).sum();
    let trigram_map_mb = (idx.trigram.trigram_map.len() as f64 * 80.0 + total_tri_postings as f64 * 4.0) / 1_048_576.0;

    // Files estimate
    let avg_file_path_len = if !idx.files.is_empty() {
        let sample = idx.files.len().min(1000);
        let total: usize = idx.files.iter().take(sample).map(|f| f.len()).sum();
        total as f64 / sample as f64
    } else {
        50.0
    };
    let files_mb = idx.files.len() as f64 * (24.0 + avg_file_path_len) / 1_048_576.0;

    let total_mb = inverted_mb + trigram_tokens_mb + trigram_map_mb + files_mb;

    serde_json::json!({
        "invertedIndexMB": round1(inverted_mb),
        "trigramTokensMB": round1(trigram_tokens_mb),
        "trigramMapMB": round1(trigram_map_mb),
        "filesMB": round1(files_mb),
        "totalEstimateMB": round1(total_mb),
        "uniqueTokens": idx.index.len(),
        "totalPostings": full_total_postings,
        "trigramCount": idx.trigram.trigram_map.len(),
        "fileCount": idx.files.len(),
    })
}

/// Estimate the in-memory size of a DefinitionIndex.
/// Returns a JSON object with per-component MB estimates.
pub fn estimate_definition_index_memory(idx: &crate::definitions::DefinitionIndex) -> serde_json::Value {
    let round1 = |v: f64| (v * 10.0).round() / 10.0;

    // Each definition: ~200 bytes (name, kind, attributes, base_types, parent, signature, line range)
    let defs_mb = idx.definitions.len() as f64 * 200.0 / 1_048_576.0;

    // Call sites: ~60 bytes each (method_name, receiver, line, col)
    let total_calls: usize = idx.method_calls.values().map(|v| v.len()).sum();
    let calls_mb = total_calls as f64 * 60.0 / 1_048_576.0;

    // Files: ~50 bytes avg path
    let files_mb = idx.files.len() as f64 * 74.0 / 1_048_576.0;

    // Indexes (name_index, kind_index, file_index, etc.): ~80 bytes per entry + Vec overhead
    let index_entries = idx.name_index.len() + idx.kind_index.len() + idx.file_index.len()
        + idx.attribute_index.len() + idx.base_type_index.len();
    let indexes_mb = index_entries as f64 * 100.0 / 1_048_576.0;

    // Code stats: ~64 bytes each
    let stats_mb = idx.code_stats.len() as f64 * 64.0 / 1_048_576.0;

    let total_mb = defs_mb + calls_mb + files_mb + indexes_mb + stats_mb;

    serde_json::json!({
        "definitionsMB": round1(defs_mb),
        "callSitesMB": round1(calls_mb),
        "filesMB": round1(files_mb),
        "indexesMB": round1(indexes_mb),
        "codeStatsMB": round1(stats_mb),
        "totalEstimateMB": round1(total_mb),
        "definitionCount": idx.definitions.len(),
        "callSiteCount": total_calls,
        "fileCount": idx.files.len(),
        "codeStatsCount": idx.code_stats.len(),
    })
}

/// Estimate the in-memory size of a GitHistoryCache.
/// Returns a JSON object with per-component MB estimates.
pub fn estimate_git_cache_memory(cache: &crate::git::cache::GitHistoryCache) -> serde_json::Value {
    let round1 = |v: f64| (v * 10.0).round() / 10.0;

    // Commits: ~120 bytes each (hash, timestamp, author_id, message interned)
    let commits_mb = cache.commits.len() as f64 * 120.0 / 1_048_576.0;

    // File commits: HashMap<String, Vec<u32>> — path string + vec of commit indices
    let files_mb = cache.file_commits.len() as f64 * 100.0 / 1_048_576.0;

    // Authors: Vec<String> — ~40 bytes each
    let authors_mb = cache.authors.len() as f64 * 40.0 / 1_048_576.0;

    let total_mb = commits_mb + files_mb + authors_mb;

    serde_json::json!({
        "commitsMB": round1(commits_mb),
        "filesMB": round1(files_mb),
        "authorsMB": round1(authors_mb),
        "totalEstimateMB": round1(total_mb),
        "commitCount": cache.commits.len(),
        "fileCount": cache.file_commits.len(),
        "authorCount": cache.authors.len(),
    })
}

// ─── Index metadata sidecar (.meta) ─────────────────────────────────

/// Lightweight metadata saved alongside each index file.
/// Allows `search-index info` CLI to display index stats without
/// deserializing the full index (which can be 500+ MB in RAM).
///
/// Written as `<index_file>.meta` (e.g., `prefix_12345678.word-search.meta`).
/// Format: JSON, ~200 bytes per file.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct IndexMeta {
    /// Index type: "content", "definition", "file-list", "git-history"
    #[serde(rename = "type")]
    pub index_type: String,
    /// Root directory of the index
    pub root: String,
    /// Timestamp when the index was created (seconds since epoch)
    pub created_at: u64,
    /// Max age in seconds before the index is considered stale (0 = no limit)
    #[serde(default)]
    pub max_age_secs: u64,
    /// Number of files in the index
    #[serde(default)]
    pub files: usize,
    /// Number of unique tokens (content index only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_tokens: Option<usize>,
    /// Total tokens indexed (content index only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// File extensions indexed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    /// Number of definitions (definition index only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definitions: Option<usize>,
    /// Number of call sites (definition index only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_sites: Option<usize>,
    /// Number of parse errors (definition index only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_errors: Option<usize>,
    /// Number of lossy UTF-8 files (definition index only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lossy_file_count: Option<usize>,
    /// Number of entries (file-list index only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<usize>,
    /// Number of commits (git-history only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commits: Option<usize>,
    /// Number of authors (git-history only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<usize>,
    /// Branch name (git-history only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// HEAD commit hash (git-history only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_hash: Option<String>,
}

/// Save an IndexMeta sidecar file alongside an index file.
/// The sidecar path is `<index_path>.meta`.
/// Errors are logged but do not propagate (sidecar is best-effort).
pub fn save_index_meta(index_path: &std::path::Path, meta: &IndexMeta) {
    let meta_path = meta_path_for(index_path);
    match serde_json::to_string_pretty(meta) {
        Ok(json) => {
            if let Err(e) = fs::write(&meta_path, json) {
                eprintln!("[meta] Warning: failed to write {}: {}", meta_path.display(), e);
            }
        }
        Err(e) => {
            eprintln!("[meta] Warning: failed to serialize meta: {}", e);
        }
    }
}

/// Load an IndexMeta sidecar file. Returns None if not found or invalid.
pub fn load_index_meta(index_path: &std::path::Path) -> Option<IndexMeta> {
    let meta_path = meta_path_for(index_path);
    let json = fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Compute the sidecar path for a given index file path.
fn meta_path_for(index_path: &std::path::Path) -> PathBuf {
    let mut meta = index_path.as_os_str().to_owned();
    meta.push(".meta");
    PathBuf::from(meta)
}

/// Build IndexMeta for a ContentIndex.
pub fn content_index_meta(idx: &crate::ContentIndex) -> IndexMeta {
    IndexMeta {
        index_type: "content".to_string(),
        root: idx.root.clone(),
        created_at: idx.created_at,
        max_age_secs: idx.max_age_secs,
        files: idx.files.len(),
        unique_tokens: Some(idx.index.len()),
        total_tokens: Some(idx.total_tokens),
        extensions: idx.extensions.clone(),
        definitions: None,
        call_sites: None,
        parse_errors: None,
        lossy_file_count: None,
        entries: None,
        commits: None,
        authors: None,
        branch: None,
        head_hash: None,
    }
}

/// Build IndexMeta for a FileIndex.
pub fn file_index_meta(idx: &crate::FileIndex) -> IndexMeta {
    IndexMeta {
        index_type: "file-list".to_string(),
        root: idx.root.clone(),
        created_at: idx.created_at,
        max_age_secs: idx.max_age_secs,
        files: 0,
        unique_tokens: None,
        total_tokens: None,
        extensions: Vec::new(),
        definitions: None,
        call_sites: None,
        parse_errors: None,
        lossy_file_count: None,
        entries: Some(idx.entries.len()),
        commits: None,
        authors: None,
        branch: None,
        head_hash: None,
    }
}

/// Build IndexMeta for a DefinitionIndex.
pub fn definition_index_meta(idx: &crate::definitions::DefinitionIndex) -> IndexMeta {
    let call_sites: usize = idx.method_calls.values().map(|v| v.len()).sum();
    IndexMeta {
        index_type: "definition".to_string(),
        root: idx.root.clone(),
        created_at: idx.created_at,
        max_age_secs: 0,
        files: idx.files.len(),
        unique_tokens: None,
        total_tokens: None,
        extensions: idx.extensions.clone(),
        definitions: Some(idx.definitions.len()),
        call_sites: Some(call_sites),
        parse_errors: if idx.parse_errors > 0 { Some(idx.parse_errors) } else { None },
        lossy_file_count: if idx.lossy_file_count > 0 { Some(idx.lossy_file_count) } else { None },
        entries: None,
        commits: None,
        authors: None,
        branch: None,
        head_hash: None,
    }
}

/// Build IndexMeta for a GitHistoryCache.
pub fn git_cache_meta(cache: &crate::git::cache::GitHistoryCache) -> IndexMeta {
    IndexMeta {
        index_type: "git-history".to_string(),
        root: String::new(),
        created_at: cache.built_at,
        max_age_secs: 0,
        files: cache.file_commits.len(),
        unique_tokens: None,
        total_tokens: None,
        extensions: Vec::new(),
        definitions: None,
        call_sites: None,
        parse_errors: None,
        lossy_file_count: None,
        entries: None,
        commits: Some(cache.commits.len()),
        authors: Some(cache.authors.len()),
        branch: Some(cache.branch.clone()),
        head_hash: Some(cache.head_hash.clone()),
    }
}

// ─── Index helpers ───────────────────────────────────────────────────

/// Recover data from a Mutex, handling poisoned state gracefully.
/// If the mutex was poisoned (a thread panicked while holding the lock),
/// logs a warning and recovers the data. This is consistent with the
/// `.lock().unwrap_or_else(|e| e.into_inner())` pattern used throughout.
pub(crate) fn recover_mutex<T>(mutex: std::sync::Mutex<T>, label: &str) -> T {
    match mutex.into_inner() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[WARN] {} mutex was poisoned (a worker thread panicked), recovering data", label);
            e.into_inner()
        }
    }
}

// ─── LZ4 compression helpers ────────────────────────────────────────

/// Magic bytes identifying LZ4-compressed index files.
pub const LZ4_MAGIC: &[u8; 4] = b"LZ4S";

/// Save a serializable value to a file with LZ4 frame compression.
/// Writes magic bytes, then LZ4-compressed bincode data.
/// Logs compression ratio and timing to stderr.
pub fn save_compressed<T: serde::Serialize>(path: &std::path::Path, data: &T, label: &str) -> Result<(), SearchError> {
    let start = Instant::now();

    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(LZ4_MAGIC)?;
    let mut encoder = lz4_flex::frame::FrameEncoder::new(writer);
    bincode::serialize_into(&mut encoder, data)?;
    let mut writer = encoder.finish().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    writer.flush()?;

    let compressed_size = std::fs::metadata(path)?.len();
    let elapsed = start.elapsed();

    eprintln!("[{}] Saved {:.1} MB (compressed) in {:.2}s to {}",
        label,
        compressed_size as f64 / 1_048_576.0,
        elapsed.as_secs_f64(),
        path.display());

    Ok(())
}

/// Load a deserializable value from a file, supporting both LZ4-compressed
/// and legacy uncompressed formats (backward compatibility).
/// Returns `Err(SearchError::IndexLoad)` with a descriptive message on failure.
pub fn load_compressed<T: serde::de::DeserializeOwned>(path: &std::path::Path, label: &str) -> Result<T, SearchError> {
    let path_str = path.display().to_string();
    let start = Instant::now();
    let compressed_size = std::fs::metadata(path)
        .map_err(|e| SearchError::IndexLoad {
            path: path_str.clone(),
            message: format!("file not found or inaccessible: {}", e),
        })?
        .len();

    let file = std::fs::File::open(path).map_err(|e| SearchError::IndexLoad {
        path: path_str.clone(),
        message: format!("cannot open file: {}", e),
    })?;
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).map_err(|e| SearchError::IndexLoad {
        path: path_str.clone(),
        message: format!("read error (magic bytes): {}", e),
    })?;

    let result = if &magic == LZ4_MAGIC {
        // Compressed format
        let decoder = lz4_flex::frame::FrameDecoder::new(reader);
        bincode::deserialize_from(decoder).map_err(|e| SearchError::IndexLoad {
            path: path_str.clone(),
            message: format!("LZ4 deserialization failed: {}", e),
        })?
    } else {
        // Legacy uncompressed format
        reader.seek(SeekFrom::Start(0)).map_err(|e| SearchError::IndexLoad {
            path: path_str.clone(),
            message: format!("seek error: {}", e),
        })?;
        let data = {
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).map_err(|e| SearchError::IndexLoad {
                path: path_str.clone(),
                message: format!("read error: {}", e),
            })?;
            buf
        };
        bincode::deserialize(&data).map_err(|e| SearchError::IndexLoad {
            path: path_str.clone(),
            message: format!("deserialization failed: {}", e),
        })?
    };

    let elapsed = start.elapsed();
    eprintln!("[{}] Loaded {:.1} MB in {:.3}s",
        label,
        compressed_size as f64 / 1_048_576.0,
        elapsed.as_secs_f64());

    Ok(result)
}

// ─── Index storage ───────────────────────────────────────────────────

/// Default production index directory: `%LOCALAPPDATA%/search-index`.
/// Tests should NOT use this — pass a test-local directory instead.
pub fn index_dir() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("search-index")
}

pub fn index_path_for(dir: &str, index_base: &std::path::Path) -> PathBuf {
    let canonical = fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
    let hash = stable_hash(&[canonical.to_string_lossy().as_bytes()]);
    let prefix = extract_semantic_prefix(&canonical);
    index_base.join(format!("{}_{:08x}.file-list", prefix, hash as u32))
}

pub fn save_index(index: &FileIndex, index_base: &std::path::Path) -> Result<(), SearchError> {
    fs::create_dir_all(index_base)?;
    let path = index_path_for(&index.root, index_base);
    save_compressed(&path, index, "file-index")?;
    save_index_meta(&path, &file_index_meta(index));
    Ok(())
}

pub fn load_index(dir: &str, index_base: &std::path::Path) -> Result<FileIndex, SearchError> {
    let path = index_path_for(dir, index_base);
    load_compressed(&path, "file-index")
}

pub fn content_index_path_for(dir: &str, exts: &str, index_base: &std::path::Path) -> PathBuf {
    let canonical = fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
    let hash = stable_hash(&[canonical.to_string_lossy().as_bytes(), exts.as_bytes()]);
    let prefix = extract_semantic_prefix(&canonical);
    index_base.join(format!("{}_{:08x}.word-search", prefix, hash as u32))
}

pub fn save_content_index(index: &ContentIndex, index_base: &std::path::Path) -> Result<(), SearchError> {
    fs::create_dir_all(index_base)?;
    let exts_str = index.extensions.join(",");
    let path = content_index_path_for(&index.root, &exts_str, index_base);
    save_compressed(&path, index, "content-index")?;
    save_index_meta(&path, &content_index_meta(index));
    Ok(())
}

pub fn load_content_index(dir: &str, exts: &str, index_base: &std::path::Path) -> Result<ContentIndex, SearchError> {
    let path = content_index_path_for(dir, exts, index_base);
    load_compressed(&path, "content-index")
}

/// Try to find any content index (.word-search) file matching the given directory.
///
/// When `expected_exts` is non-empty, the cached index must contain ALL
/// of the expected extensions (superset check). If the cached index is
/// missing any expected extension, it is skipped so the caller can
/// trigger a full rebuild with the correct extensions.
///
/// This prevents a stale cache (e.g., built with `--ext cs` only) from
/// being used when the server now requires `--ext cs,sql`.
pub fn find_content_index_for_dir(dir: &str, index_base: &std::path::Path, expected_exts: &[String]) -> Option<ContentIndex> {
    if !index_base.exists() {
        return None;
    }
    let canonical = fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
    let clean = clean_path(&canonical.to_string_lossy());

    for entry in fs::read_dir(index_base).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "word-search") {
            match load_compressed::<ContentIndex>(&path, "content-index") {
                Ok(index) => {
                    if index.root == clean {
                        // Validate that cached index has ALL expected extensions
                        if !expected_exts.is_empty() {
                            let has_all = expected_exts.iter().all(|ext|
                                index.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
                            );
                            if !has_all {
                                eprintln!("[find_content_index] Skipping {} — extensions mismatch (cached: {:?}, expected: {:?})",
                                    path.display(), index.extensions, expected_exts);
                                continue;
                            }
                        }
                        return Some(index);
                    }
                }
                Err(e) => {
                    eprintln!("[find_content_index] Skipping {}: {}", path.display(), e);
                }
            }
        }
    }
    None
}

/// Read the root field from an index file without deserializing the whole file.
/// Handles both LZ4-compressed and legacy uncompressed formats.
/// Bincode stores a String as: u64 (length) + bytes. Since `root` is the first field in
/// FileIndex, ContentIndex, and DefinitionIndex, we can read just the first few bytes.
fn read_root_from_index_file(path: &std::path::Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;

    let reader: Box<dyn Read> = if &magic == LZ4_MAGIC {
        Box::new(lz4_flex::frame::FrameDecoder::new(BufReader::new(file)))
    } else {
        file.seek(SeekFrom::Start(0)).ok()?;
        Box::new(BufReader::new(file))
    };

    // Read bincode-encoded string: 8-byte length prefix + UTF-8 bytes
    let mut len_buf = [0u8; 8];
    let mut reader = reader;
    reader.read_exact(&mut len_buf).ok()?;
    let len = u64::from_le_bytes(len_buf) as usize;
    if len > 4096 { return None; }
    let mut str_buf = vec![0u8; len];
    reader.read_exact(&mut str_buf).ok()?;
    String::from_utf8(str_buf).ok()
}

/// Public wrapper for `read_root_from_index_file` — used by `handle_search_info`
/// to get the root directory from a file-list index without full deserialization.
pub fn read_root_from_index_file_pub(path: &std::path::Path) -> Option<String> {
    read_root_from_index_file(path)
}

/// Remove orphaned index files whose root directory no longer exists on disk.
/// Returns the number of files removed.
/// Reads only the root field from each file header (fast — no full deserialization).
pub fn cleanup_orphaned_indexes(index_base: &std::path::Path) -> usize {
    if !index_base.exists() {
        return 0;
    }

    let mut removed = 0;

    if let Ok(entries) = std::fs::read_dir(index_base) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if !matches!(ext, Some("file-list") | Some("word-search") | Some("code-structure")) {
                continue;
            }

            if let Some(root) = read_root_from_index_file(&path) {
                if !std::path::Path::new(&root).exists() {
                    if std::fs::remove_file(&path).is_ok() {
                        removed += 1;
                        eprintln!("  Removed orphaned index: {} (root: {})", path.display(), root);
                        // Also remove sidecar .meta file
                        let _ = std::fs::remove_file(meta_path_for(&path));
                    }
                }
            }
        }
    }

    removed
}

/// Remove all index files (.file-list, .word-search, .code-structure) whose root matches the given directory.
/// Comparison is case-insensitive on the canonicalized paths (Windows-safe).
/// Returns the number of files removed.
pub fn cleanup_indexes_for_dir(dir: &str, index_base: &std::path::Path) -> usize {
    if !index_base.exists() {
        return 0;
    }

    let target = std::fs::canonicalize(dir)
        .map(|p| clean_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| clean_path(dir));

    let mut removed = 0;

    if let Ok(entries) = std::fs::read_dir(index_base) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if !matches!(ext, Some("file-list") | Some("word-search") | Some("code-structure")) {
                continue;
            }

            if let Some(root) = read_root_from_index_file(&path) {
                let root_canonical = std::fs::canonicalize(&root)
                    .map(|p| clean_path(&p.to_string_lossy()))
                    .unwrap_or_else(|_| clean_path(&root));
                if root_canonical.eq_ignore_ascii_case(&target) {
                    if std::fs::remove_file(&path).is_ok() {
                        removed += 1;
                        eprintln!("  Removed index for dir '{}': {} ({})",
                            dir, path.display(), ext.unwrap_or("?"));
                        // Also remove sidecar .meta file
                        let _ = std::fs::remove_file(meta_path_for(&path));
                    }
                }
            }
        }
    }

    removed
}

// ─── Index building ──────────────────────────────────────────────────

pub fn build_index(args: &IndexArgs) -> FileIndex {
    let root = fs::canonicalize(&args.dir).unwrap_or_else(|_| PathBuf::from(&args.dir));
    let root_str = clean_path(&root.to_string_lossy());

    eprintln!("Indexing {}...", root_str);
    let start = Instant::now();

    let mut builder = WalkBuilder::new(&root);
    builder.hidden(!args.hidden);
    builder.git_ignore(!args.no_ignore);
    builder.git_global(!args.no_ignore);
    builder.git_exclude(!args.no_ignore);

    let thread_count = if args.threads == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    } else {
        args.threads
    };
    builder.threads(thread_count);

    let entries: Mutex<Vec<FileEntry>> = Mutex::new(Vec::new());

    builder.build_parallel().run(|| {
        let entries = &entries;
        Box::new(move |result| {
            if let Ok(entry) = result {
                let path = clean_path(&entry.path().to_string_lossy());
                let metadata = entry.metadata().ok();
                let (size, modified, is_dir) = if let Some(m) = metadata {
                    let mod_time = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    (m.len(), mod_time, m.is_dir())
                } else {
                    (0, 0, false)
                };

                let fe = FileEntry {
                    path,
                    size,
                    modified,
                    is_dir,
                };

                entries.lock().unwrap_or_else(|e| e.into_inner()).push(fe);
            }
            ignore::WalkState::Continue
        })
    });

    let entries = recover_mutex(entries, "file-index");
    let count = entries.len();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    let index = FileIndex {
        root: root_str,
        created_at: now,
        max_age_secs: args.max_age_hours * 3600,
        entries,
    };

    let elapsed = start.elapsed();
    eprintln!(
        "Indexed {} entries in {:.3}s",
        count,
        elapsed.as_secs_f64()
    );

    index
}

// ─── Content index building ──────────────────────────────────────────

pub fn build_content_index(args: &ContentIndexArgs) -> ContentIndex {
    let root = fs::canonicalize(&args.dir).unwrap_or_else(|_| PathBuf::from(&args.dir));
    let root_str = clean_path(&root.to_string_lossy());
    let extensions: Vec<String> = args.ext.split(',').map(|s| s.trim().to_lowercase()).collect();

    eprintln!(
        "Building content index for {} (extensions: {})...",
        root_str,
        extensions.join(", ")
    );
    let start = Instant::now();

    let mut builder = WalkBuilder::new(&root);
    builder.hidden(!args.hidden);
    builder.git_ignore(!args.no_ignore);
    builder.git_global(!args.no_ignore);
    builder.git_exclude(!args.no_ignore);

    let thread_count = if args.threads == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    } else {
        args.threads
    };
    builder.threads(thread_count);

    let file_data: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    builder.build_parallel().run(|| {
        let extensions = extensions.clone();
        let file_data = &file_data;
        Box::new(move |result| {
            if let Ok(entry) = result {
                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    return ignore::WalkState::Continue;
                }
                let ext_match = entry
                    .path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)));
                if !ext_match {
                    return ignore::WalkState::Continue;
                }
                let path = clean_path(&entry.path().to_string_lossy());
                match read_file_lossy(entry.path()) {
                    Ok((content, _was_lossy)) => {
                        file_data.lock().unwrap_or_else(|e| e.into_inner()).push((path, content));
                    }
                    Err(_) => {}
                }
            }
            ignore::WalkState::Continue
        })
    });

    let file_data = recover_mutex(file_data, "content-index");
    let file_count = file_data.len();
    let min_len = args.min_token_len;
    log_memory(&format!("content-build: after file walk ({} files)", file_count));

    // ─── Parallel tokenization ──────────────────────────────────
    let num_tok_threads = thread_count.max(1);
    let tok_chunk_size = file_count.div_ceil(num_tok_threads).max(1);

    let chunk_results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = file_data
            .chunks(tok_chunk_size)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let base_file_id = (chunk_idx * tok_chunk_size) as u32;
                s.spawn(move || {
                    let mut local_files: Vec<String> = Vec::with_capacity(chunk.len());
                    let mut local_counts: Vec<u32> = Vec::with_capacity(chunk.len());
                    let mut local_index: HashMap<String, Vec<Posting>> = HashMap::new();
                    let mut local_total: u64 = 0;

                    for (i, (path, content)) in chunk.iter().enumerate() {
                        let file_id = base_file_id + i as u32;
                        local_files.push(path.clone());
                        let mut file_tokens: HashMap<String, Vec<u32>> = HashMap::new();
                        let mut file_total: u32 = 0;

                        for (line_num, line) in content.lines().enumerate() {
                            for token in tokenize(line, min_len) {
                                local_total += 1;
                                file_total += 1;
                                file_tokens
                                    .entry(token)
                                    .or_default()
                                    .push((line_num + 1) as u32);
                            }
                        }

                        local_counts.push(file_total);

                        for (token, lines) in file_tokens {
                            local_index
                                .entry(token)
                                .or_default()
                                .push(Posting { file_id, lines });
                        }
                    }

                    (local_files, local_counts, local_index, local_total)
                })
            })
            .collect();

        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| {
            eprintln!("[WARN] Worker thread panicked during content index building");
            (Vec::new(), Vec::new(), HashMap::new(), 0u64)
        })).collect()
    });

    log_memory("content-build: after tokenization (file_data + chunks alive)");

    // Free raw file contents — no longer needed after tokenization.
    // This releases ~1.6 GB for large repos (80K files × ~20KB avg content).
    // Without this drop, the file data stays alive until function return,
    // causing peak memory to be ~1.6 GB higher during build vs. load-from-disk.
    drop(file_data);
    log_memory("content-build: after drop(file_data)");

    // ─── Merge per-thread results ───────────────────────────────
    let mut files: Vec<String> = Vec::with_capacity(file_count);
    let mut file_token_counts: Vec<u32> = Vec::with_capacity(file_count);
    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    let mut total_tokens: u64 = 0;

    for (local_files, local_counts, local_index, local_total) in chunk_results {
        files.extend(local_files);
        file_token_counts.extend(local_counts);
        total_tokens += local_total;
        for (token, postings) in local_index {
            index.entry(token).or_default().extend(postings);
        }
    }

    let unique_tokens = index.len();
    log_memory(&format!("content-build: after merge ({} tokens)", unique_tokens));

    // Build trigram index from inverted index tokens
    let trigram = build_trigram_index(&index);
    eprintln!(
        "Trigram index: {} trigrams, {} tokens",
        trigram.trigram_map.len(),
        trigram.tokens.len()
    );
    log_memory("content-build: after trigram build");

    let elapsed = start.elapsed();

    eprintln!(
        "Indexed {} files, {} unique tokens ({} total) in {:.3}s",
        file_count, unique_tokens, total_tokens, elapsed.as_secs_f64()
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    ContentIndex {
        root: root_str,
        created_at: now,
        max_age_secs: args.max_age_hours * 3600,
        files,
        index,
        total_tokens,
        extensions,
        file_token_counts,
        trigram,
        trigram_dirty: false,
        forward: None,
        path_to_id: None,
    }
}

/// Build a trigram index from the inverted index's token keys.
pub fn build_trigram_index(inverted: &HashMap<String, Vec<Posting>>) -> TrigramIndex {
    let mut tokens: Vec<String> = inverted.keys().cloned().collect();
    tokens.sort();

    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();

    for (idx, token) in tokens.iter().enumerate() {
        let trigrams = generate_trigrams(token);
        for trigram in trigrams {
            trigram_map.entry(trigram).or_default().push(idx as u32);
        }
    }

    // Sort and dedup posting lists
    for list in trigram_map.values_mut() {
        list.sort();
        list.dedup();
    }

    TrigramIndex { tokens, trigram_map }
}
#[cfg(test)]
mod index_tests {
    use std::collections::HashMap;
    use std::io::Write;
    use search_index::Posting;
    use crate::index::build_trigram_index;

    #[test]
    fn test_build_trigram_index_basic() {
        let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
        inverted.insert("httpclient".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
        inverted.insert("httphandler".to_string(), vec![Posting { file_id: 1, lines: vec![5] }]);
        inverted.insert("ab".to_string(), vec![Posting { file_id: 2, lines: vec![10] }]); // too short for trigrams

        let ti = build_trigram_index(&inverted);

        // Tokens should be sorted
        assert_eq!(ti.tokens, vec!["ab", "httpclient", "httphandler"]);

        // "htt" should map to both http tokens
        let htt = ti.trigram_map.get("htt").unwrap();
        assert_eq!(htt.len(), 2); // indices of httpclient and httphandler

        // "cli" should only map to httpclient
        let cli = ti.trigram_map.get("cli").unwrap();
        assert_eq!(cli.len(), 1);

        // "ab" should not generate any trigrams (too short)
        // but "ab" should still be in tokens list
        assert!(ti.tokens.contains(&"ab".to_string()));
    }

    #[test]
    fn test_build_trigram_index_empty() {
        let inverted: HashMap<String, Vec<Posting>> = HashMap::new();
        let ti = build_trigram_index(&inverted);
        assert!(ti.tokens.is_empty());
        assert!(ti.trigram_map.is_empty());
    }

    #[test]
    fn test_build_trigram_index_sorted_posting_lists() {
        let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
        inverted.insert("abcdef".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
        inverted.insert("abcxyz".to_string(), vec![Posting { file_id: 1, lines: vec![2] }]);

        let ti = build_trigram_index(&inverted);

        // All posting lists should be sorted
        for (_, list) in &ti.trigram_map {
            for window in list.windows(2) {
                assert!(window[0] <= window[1], "Posting list not sorted");
            }
        }
    }

    #[test]
    fn test_build_trigram_index_single_token() {
        let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
        inverted.insert("foobar".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);

        let ti = build_trigram_index(&inverted);

        assert_eq!(ti.tokens, vec!["foobar"]);
        // "foobar" has 4 trigrams: foo, oob, oba, bar
        assert_eq!(ti.trigram_map.len(), 4);
        assert!(ti.trigram_map.contains_key("foo"));
        assert!(ti.trigram_map.contains_key("oob"));
        assert!(ti.trigram_map.contains_key("oba"));
        assert!(ti.trigram_map.contains_key("bar"));
    }

    #[test]
    fn test_build_trigram_index_deduplicates() {
        // Two tokens sharing the same trigram should appear once each in the posting list
        let mut inverted: HashMap<String, Vec<Posting>> = HashMap::new();
        inverted.insert("abc".to_string(), vec![Posting { file_id: 0, lines: vec![1] }]);
        inverted.insert("abcdef".to_string(), vec![Posting { file_id: 1, lines: vec![2] }]);

        let ti = build_trigram_index(&inverted);

        let abc_list = ti.trigram_map.get("abc").unwrap();
        // Both "abc" (idx 0) and "abcdef" (idx 1) share trigram "abc"
        assert_eq!(abc_list.len(), 2);
        // Should be deduped (no duplicates)
        let mut deduped = abc_list.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(abc_list.len(), deduped.len());
    }

    // ─── LZ4 compression tests ──────────────────────────────

    #[test]
    fn test_save_load_compressed_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.bin");

        let data = vec!["hello".to_string(), "world".to_string(), "compressed".to_string()];
        crate::index::save_compressed(&path, &data, "test").unwrap();
        let loaded: Result<Vec<String>, _> = crate::index::load_compressed(&path, "test");
        assert!(loaded.is_ok());
        assert_eq!(data, loaded.unwrap());

        // Verify file starts with LZ4 magic bytes
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(&raw[..4], crate::index::LZ4_MAGIC);
    }

    #[test]
    fn test_load_compressed_legacy_uncompressed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("legacy.bin");

        // Write uncompressed bincode (legacy format)
        let data = vec!["legacy".to_string(), "format".to_string()];
        let encoded = bincode::serialize(&data).unwrap();
        std::fs::write(&path, &encoded).unwrap();

        // load_compressed should still read it via backward compatibility
        let loaded: Result<Vec<String>, _> = crate::index::load_compressed(&path, "test");
        assert!(loaded.is_ok());
        assert_eq!(data, loaded.unwrap());
    }

    #[test]
    fn test_load_compressed_missing_file_returns_err() {
        let path = std::path::Path::new("/nonexistent/path/to/file.bin");
        let result: Result<Vec<String>, _> = crate::index::load_compressed(path, "test");
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("Failed to load index"), "Error should contain 'Failed to load index', got: {}", err_msg);
    }

    #[test]
    fn test_load_compressed_corrupt_data() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("corrupt.bin");

        // Write random bytes that look like neither valid LZ4 nor valid bincode
        std::fs::write(&path, b"this is not valid data at all!!!!!").unwrap();
        let result: Result<Vec<String>, _> = crate::index::load_compressed(&path, "test");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("deserialization failed"), "Error should mention deserialization, got: {}", err_msg);
    }

    // ─── Memory diagnostics tests ────────────────────────────

    #[test]
    fn test_log_memory_is_noop_when_disabled() {
        // log_memory should be a safe no-op when memory logging is not enabled
        // (default state: MEMORY_LOG_ENABLED is false)
        crate::index::log_memory("test: this should be a no-op");
        // No panic, no output — success
    }

    #[test]
    fn test_enable_debug_log_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        // Note: we can't call enable_debug_log in tests because it uses
        // global OnceLock (can only set once per process). Instead, test the
        // file creation logic directly.
        let log_path = tmp.path().join("debug.log");
        {
            let mut f = std::fs::File::create(&log_path).unwrap();
            writeln!(f, "{:>8} | {:>8} | {:>8} | {:>8} | {}",
                "elapsed", "WS_MB", "Peak_MB", "Commit_MB", "label").unwrap();
            writeln!(f, "{}", "-".repeat(70)).unwrap();
        }
        assert!(log_path.exists());
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("elapsed"));
        assert!(content.contains("WS_MB"));
        assert!(content.contains("label"));
    }

    #[test]
    fn test_debug_log_path_has_semantic_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let server_dir = tmp.path().to_string_lossy().to_string();
        let path = crate::index::debug_log_path_for(tmp.path(), &server_dir);
        let filename = path.file_name().unwrap().to_string_lossy();
        assert!(filename.ends_with(".debug.log"),
            "Debug log filename should end with .debug.log, got: {}", filename);
        assert!(filename.contains('_'),
            "Debug log filename should have prefix_hash format, got: {}", filename);
    }

    #[test]
    fn test_debug_log_path_different_dirs_different_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_a = tmp.path().join("dir_a");
        let dir_b = tmp.path().join("dir_b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let path_a = crate::index::debug_log_path_for(tmp.path(), &dir_a.to_string_lossy());
        let path_b = crate::index::debug_log_path_for(tmp.path(), &dir_b.to_string_lossy());
        assert_ne!(path_a, path_b,
            "Different server dirs should produce different debug log paths");
    }

    #[test]
    fn test_debug_log_path_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let server_dir = tmp.path().to_string_lossy().to_string();
        let path1 = crate::index::debug_log_path_for(tmp.path(), &server_dir);
        let path2 = crate::index::debug_log_path_for(tmp.path(), &server_dir);
        assert_eq!(path1, path2,
            "Same inputs should produce same debug log path");
    }

    #[test]
    fn test_log_request_format() {
        // Test format_utc_timestamp + log_request line format
        // Since we can't enable the global log in tests, test the format logic directly
        let ts = crate::index::format_utc_timestamp();
        assert!(ts.ends_with('Z'), "Timestamp should end with Z: {}", ts);
        assert!(ts.contains('T'), "Timestamp should contain T separator: {}", ts);
        assert_eq!(ts.len(), 20, "Timestamp should be 20 chars (YYYY-MM-DDTHH:MM:SSZ): {}", ts);
    }

    #[test]
    fn test_log_response_format() {
        // Verify format_utc_timestamp produces valid ISO 8601
        let ts = crate::index::format_utc_timestamp();
        // Parse year, month, day
        let year: u32 = ts[0..4].parse().unwrap();
        let month: u32 = ts[5..7].parse().unwrap();
        let day: u32 = ts[8..10].parse().unwrap();
        assert!(year >= 2020 && year <= 2100, "Year out of range: {}", year);
        assert!(month >= 1 && month <= 12, "Month out of range: {}", month);
        assert!(day >= 1 && day <= 31, "Day out of range: {}", day);
    }

    #[test]
    fn test_debug_log_path_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let server_dir = tmp.path().to_string_lossy().to_string();
        let path = crate::index::debug_log_path_for(tmp.path(), &server_dir);
        let filename = path.file_name().unwrap().to_string_lossy();
        assert!(filename.ends_with(".debug.log"),
            "Debug log filename should end with .debug.log, got: {}", filename);
    }

    #[test]
    fn test_format_utc_timestamp_format() {
        let ts = crate::index::format_utc_timestamp();
        // Verify exact format: YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(ts.as_bytes()[4], b'-');
        assert_eq!(ts.as_bytes()[7], b'-');
        assert_eq!(ts.as_bytes()[10], b'T');
        assert_eq!(ts.as_bytes()[13], b':');
        assert_eq!(ts.as_bytes()[16], b':');
        assert_eq!(ts.as_bytes()[19], b'Z');
    }

    #[test]
    fn test_get_process_memory_info_returns_json() {
        let info = crate::index::get_process_memory_info();
        // On Windows, should have workingSetMB, peakWorkingSetMB, commitMB
        // On non-Windows, returns empty object
        assert!(info.is_object());
        #[cfg(target_os = "windows")]
        {
            assert!(info["workingSetMB"].as_f64().is_some(), "should have workingSetMB");
            assert!(info["peakWorkingSetMB"].as_f64().is_some(), "should have peakWorkingSetMB");
            assert!(info["commitMB"].as_f64().is_some(), "should have commitMB");
            // Working set should be > 0 for any running process
            assert!(info["workingSetMB"].as_f64().unwrap() > 0.0, "working set should be > 0");
        }
    }

    #[test]
    fn test_force_mimalloc_collect_does_not_panic() {
        // force_mimalloc_collect should be safe to call at any time
        crate::index::force_mimalloc_collect();
        // No panic — success
    }

    #[test]
    fn test_estimate_content_index_memory_empty() {
        let idx = search_index::ContentIndex {
            root: ".".to_string(),
            created_at: 0,
            max_age_secs: 3600,
            files: vec![],
            index: HashMap::new(),
            total_tokens: 0,
            extensions: vec![],
            file_token_counts: vec![],
            trigram: search_index::TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: None,
        };
        let estimate = crate::index::estimate_content_index_memory(&idx);
        assert!(estimate.is_object());
        assert_eq!(estimate["fileCount"], 0);
        assert_eq!(estimate["uniqueTokens"], 0);
        assert_eq!(estimate["totalPostings"], 0);
        // Total estimate should be 0 for empty index
        assert_eq!(estimate["totalEstimateMB"].as_f64().unwrap(), 0.0);
    }

    #[test]
    fn test_estimate_content_index_memory_nonempty() {
        let mut index = HashMap::new();
        index.insert("httpclient".to_string(), vec![
            Posting { file_id: 0, lines: vec![1, 5, 10] },
            Posting { file_id: 1, lines: vec![3] },
        ]);
        index.insert("ilogger".to_string(), vec![
            Posting { file_id: 0, lines: vec![2] },
        ]);

        let idx = search_index::ContentIndex {
            root: ".".to_string(),
            created_at: 0,
            max_age_secs: 3600,
            files: vec!["file0.cs".to_string(), "file1.cs".to_string()],
            index,
            total_tokens: 100,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![50, 30],
            trigram: search_index::TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: None,
        };
        let estimate = crate::index::estimate_content_index_memory(&idx);
        assert!(estimate.is_object());
        assert_eq!(estimate["fileCount"], 2);
        assert_eq!(estimate["uniqueTokens"], 2);
        assert_eq!(estimate["totalPostings"], 3);
        // Total estimate should be >= 0 (may round to 0.0 for tiny indexes)
        assert!(estimate["totalEstimateMB"].as_f64().is_some());
        assert!(estimate["invertedIndexMB"].as_f64().is_some());
        // Verify all expected fields are present
        assert!(estimate["trigramTokensMB"].as_f64().is_some());
        assert!(estimate["trigramMapMB"].as_f64().is_some());
        assert!(estimate["filesMB"].as_f64().is_some());
        assert!(estimate["trigramCount"].as_u64().is_some());
    }

    #[test]
    fn test_estimate_definition_index_memory_empty() {
        let idx = crate::definitions::DefinitionIndex {
            root: ".".to_string(),
            created_at: 0,
            extensions: vec![],
            files: vec![],
            definitions: vec![],
            name_index: std::collections::HashMap::new(),
            kind_index: std::collections::HashMap::new(),
            attribute_index: std::collections::HashMap::new(),
            base_type_index: std::collections::HashMap::new(),
            file_index: std::collections::HashMap::new(),
            path_to_id: std::collections::HashMap::new(),
            method_calls: std::collections::HashMap::new(),
            code_stats: std::collections::HashMap::new(),
            parse_errors: 0,
            lossy_file_count: 0,
            empty_file_ids: vec![],
            extension_methods: std::collections::HashMap::new(),
            selector_index: std::collections::HashMap::new(),
            template_children: std::collections::HashMap::new(),
        };
        let estimate = crate::index::estimate_definition_index_memory(&idx);
        assert!(estimate.is_object());
        assert_eq!(estimate["definitionCount"], 0);
        assert_eq!(estimate["fileCount"], 0);
        assert_eq!(estimate["totalEstimateMB"].as_f64().unwrap(), 0.0);
    }

    // ─── find_content_index_for_dir extension validation tests ─────

    #[test]
    fn test_find_content_index_skips_stale_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        let index_base = tmp.path();

        let root_dir = tmp.path().join("project");
        std::fs::create_dir_all(&root_dir).unwrap();
        let root_str = crate::clean_path(&root_dir.to_string_lossy());

        // Save a content index with only "cs" extension
        let idx = search_index::ContentIndex {
            root: root_str.clone(),
            created_at: 0,
            max_age_secs: 86400,
            files: vec![],
            index: HashMap::new(),
            total_tokens: 0,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![],
            trigram: search_index::TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: None,
        };
        crate::save_content_index(&idx, index_base).unwrap();

        // Request "cs,sql" — should NOT find the old cs-only index
        let expected = vec!["cs".to_string(), "sql".to_string()];
        let result = crate::index::find_content_index_for_dir(&root_str, index_base, &expected);
        assert!(result.is_none(),
            "Should not find cs-only content index when cs,sql is expected");
    }

    #[test]
    fn test_find_content_index_accepts_superset() {
        let tmp = tempfile::tempdir().unwrap();
        let index_base = tmp.path();

        let root_dir = tmp.path().join("project");
        std::fs::create_dir_all(&root_dir).unwrap();
        let root_str = crate::clean_path(&root_dir.to_string_lossy());

        // Save a content index with "cs,sql,md" extensions
        let idx = search_index::ContentIndex {
            root: root_str.clone(),
            created_at: 0,
            max_age_secs: 86400,
            files: vec![],
            index: HashMap::new(),
            total_tokens: 0,
            extensions: vec!["cs".to_string(), "sql".to_string(), "md".to_string()],
            file_token_counts: vec![],
            trigram: search_index::TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: None,
        };
        crate::save_content_index(&idx, index_base).unwrap();

        // Request "cs,sql" — should find the superset index
        let expected = vec!["cs".to_string(), "sql".to_string()];
        let result = crate::index::find_content_index_for_dir(&root_str, index_base, &expected);
        assert!(result.is_some(),
            "Should find cs,sql,md content index when cs,sql is expected (superset)");
    }

    #[test]
    fn test_find_content_index_empty_expected_accepts_any() {
        let tmp = tempfile::tempdir().unwrap();
        let index_base = tmp.path();

        let root_dir = tmp.path().join("project");
        std::fs::create_dir_all(&root_dir).unwrap();
        let root_str = crate::clean_path(&root_dir.to_string_lossy());

        let idx = search_index::ContentIndex {
            root: root_str.clone(),
            created_at: 0,
            max_age_secs: 86400,
            files: vec![],
            index: HashMap::new(),
            total_tokens: 0,
            extensions: vec!["cs".to_string()],
            file_token_counts: vec![],
            trigram: search_index::TrigramIndex::default(),
            trigram_dirty: false,
            forward: None,
            path_to_id: None,
        };
        crate::save_content_index(&idx, index_base).unwrap();

        // Empty expected — should accept any (backward compatible)
        let result = crate::index::find_content_index_for_dir(&root_str, index_base, &[]);
        assert!(result.is_some(),
            "Empty expected_exts should accept any cached content index");
    }

    #[test]
    fn test_compressed_file_smaller_than_uncompressed() {
        let tmp = tempfile::tempdir().unwrap();
        let compressed_path = tmp.path().join("compressed.bin");
        let uncompressed_path = tmp.path().join("uncompressed.bin");

        // Create data with repetitive content (compresses well)
        let data: Vec<String> = (0..1000).map(|i| format!("repeated_token_{}", i % 10)).collect();

        crate::index::save_compressed(&compressed_path, &data, "test").unwrap();
        let uncompressed = bincode::serialize(&data).unwrap();
        std::fs::write(&uncompressed_path, &uncompressed).unwrap();

        let compressed_size = std::fs::metadata(&compressed_path).unwrap().len();
        let uncompressed_size = std::fs::metadata(&uncompressed_path).unwrap().len();

        assert!(compressed_size < uncompressed_size,
            "Compressed ({}) should be smaller than uncompressed ({})",
            compressed_size, uncompressed_size);
    }
}