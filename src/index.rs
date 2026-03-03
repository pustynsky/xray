//! Index storage: save/load/build for FileIndex and ContentIndex.

use std::collections::HashMap;
use std::fs;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;

use crate::error::SearchError;
use search_index::{clean_path, extract_semantic_prefix, generate_trigrams, read_file_lossy, stable_hash, tokenize, ContentIndex, FileEntry, FileIndex, Posting, TrigramIndex};

use crate::{ContentIndexArgs, IndexArgs};

// ─── Windows FFI bindings (shared by log_memory + get_process_memory_info) ───

#[cfg(target_os = "windows")]
mod win_ffi {
    /// Windows process memory counters, matching PROCESS_MEMORY_COUNTERS from psapi.h.
    #[repr(C)]
    #[allow(non_snake_case)]
    pub(super) struct ProcessMemoryCounters {
        pub cb: u32,
        pub PageFaultCount: u32,
        pub PeakWorkingSetSize: usize,
        pub WorkingSetSize: usize,
        pub QuotaPeakPagedPoolUsage: usize,
        pub QuotaPagedPoolUsage: usize,
        pub QuotaPeakNonPagedPoolUsage: usize,
        pub QuotaNonPagedPoolUsage: usize,
        pub PagefileUsage: usize,
        pub PeakPagefileUsage: usize,
    }

    unsafe extern "system" {
        pub(super) fn GetCurrentProcess() -> isize;
        pub(super) fn K32GetProcessMemoryInfo(
            process: isize,
            ppsmemCounters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }
}

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
            "{:>8} | {:>8} | {:>8} | {:>8} | label",
            "elapsed", "WS_MB", "Peak_MB", "Commit_MB"
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
    if let Some(path) = DEBUG_LOG_PATH.get()
        && let Ok(mut f) = fs::OpenOptions::new().append(true).open(path) {
            let _ = writeln!(f, "{}", line);
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

/// Query Windows process memory counters via FFI.
///
/// Returns `None` if the FFI call fails. Shared by [`log_memory()`] and
/// [`get_process_memory_info()`] to avoid duplicating the 15-line init + call block.
#[cfg(target_os = "windows")]
fn get_pmc() -> Option<win_ffi::ProcessMemoryCounters> {
    let mut pmc = win_ffi::ProcessMemoryCounters {
        cb: std::mem::size_of::<win_ffi::ProcessMemoryCounters>() as u32,
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

    // SAFETY: GetCurrentProcess() returns a pseudo-handle (-1) that is always valid
    // and does not need to be closed. K32GetProcessMemoryInfo is safe to call with
    // a correctly-laid-out #[repr(C)] ProcessMemoryCounters struct initialized to zero,
    // with cb set to the struct size. The function only writes within the struct bounds.
    let ok = unsafe {
        win_ffi::K32GetProcessMemoryInfo(win_ffi::GetCurrentProcess(), &mut pmc, pmc.cb)
    };

    if ok == 0 { None } else { Some(pmc) }
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

    let Some(pmc) = get_pmc() else { return };

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
    let Some(pmc) = get_pmc() else {
        return serde_json::json!({});
    };

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
    // SAFETY: mi_collect(true) is an idempotent operation that triggers garbage collection
    // in the mimalloc allocator. It is safe to call at any time — mimalloc guarantees
    // thread-safety for mi_collect. Our global allocator is mimalloc (#[global_allocator]),
    // so the allocator is initialized before any code runs.
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
    // Use active count (excludes tombstones from incremental updates)
    let active_defs: usize = idx.file_index.values().map(|v| v.len()).sum();
    let defs_mb = active_defs as f64 * 200.0 / 1_048_576.0;

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
        "definitionCount": active_defs,
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
    /// File extensions indexed (content + definition only)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    /// Type-specific metadata — discriminated by "type" field in JSON
    #[serde(flatten)]
    pub details: IndexDetails,
}

/// Type-specific index metadata, serialized with `"type"` as the JSON discriminator.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum IndexDetails {
    /// Content (word-search) index metadata
    #[serde(rename = "content")]
    Content {
        unique_tokens: usize,
        total_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parse_errors: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lossy_file_count: Option<usize>,
    },
    /// Definition (code-structure) index metadata
    #[serde(rename = "definition")]
    Definition {
        definitions: usize,
        call_sites: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parse_errors: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lossy_file_count: Option<usize>,
    },
    /// File-list index metadata
    #[serde(rename = "file-list")]
    FileList {
        entries: usize,
    },
    /// Git history cache metadata
    #[serde(rename = "git-history")]
    GitHistory {
        commits: usize,
        authors: usize,
        branch: String,
        head_hash: String,
    },
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
        root: idx.root.clone(),
        created_at: idx.created_at,
        max_age_secs: idx.max_age_secs,
        files: idx.files.len(),
        extensions: idx.extensions.clone(),
        details: IndexDetails::Content {
            unique_tokens: idx.index.len(),
            total_tokens: idx.total_tokens,
            parse_errors: if idx.read_errors > 0 { Some(idx.read_errors) } else { None },
            lossy_file_count: if idx.lossy_file_count > 0 { Some(idx.lossy_file_count) } else { None },
        },
    }
}

/// Build IndexMeta for a FileIndex.
pub fn file_index_meta(idx: &crate::FileIndex) -> IndexMeta {
    IndexMeta {
        root: idx.root.clone(),
        created_at: idx.created_at,
        max_age_secs: idx.max_age_secs,
        files: 0,
        extensions: Vec::new(),
        details: IndexDetails::FileList {
            entries: idx.entries.len(),
        },
    }
}

/// Build IndexMeta for a DefinitionIndex.
pub fn definition_index_meta(idx: &crate::definitions::DefinitionIndex) -> IndexMeta {
    let call_sites: usize = idx.method_calls.values().map(|v| v.len()).sum();
    let active_defs: usize = idx.file_index.values().map(|v| v.len()).sum();
    IndexMeta {
        root: idx.root.clone(),
        created_at: idx.created_at,
        max_age_secs: 0,
        files: idx.files.len(),
        extensions: idx.extensions.clone(),
        details: IndexDetails::Definition {
            definitions: active_defs,
            call_sites,
            parse_errors: if idx.parse_errors > 0 { Some(idx.parse_errors) } else { None },
            lossy_file_count: if idx.lossy_file_count > 0 { Some(idx.lossy_file_count) } else { None },
        },
    }
}

/// Build IndexMeta for a GitHistoryCache.
pub fn git_cache_meta(cache: &crate::git::cache::GitHistoryCache) -> IndexMeta {
    IndexMeta {
        root: String::new(),
        created_at: cache.built_at,
        max_age_secs: 0,
        files: cache.file_commits.len(),
        extensions: Vec::new(),
        details: IndexDetails::GitHistory {
            commits: cache.commits.len(),
            authors: cache.authors.len(),
            branch: cache.branch.clone(),
            head_hash: cache.head_hash.clone(),
        },
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
    let mut writer = encoder.finish().map_err(std::io::Error::other)?;
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

            if let Some(root) = read_root_from_index_file(&path)
                && !std::path::Path::new(&root).exists()
                    && std::fs::remove_file(&path).is_ok() {
                        removed += 1;
                        eprintln!("  Removed orphaned index: {} (root: {})", path.display(), root);
                        // Also remove sidecar .meta file
                        let _ = std::fs::remove_file(meta_path_for(&path));
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
                if root_canonical.eq_ignore_ascii_case(&target)
                    && std::fs::remove_file(&path).is_ok() {
                        removed += 1;
                        eprintln!("  Removed index for dir '{}': {} ({})",
                            dir, path.display(), ext.unwrap_or("?"));
                        // Also remove sidecar .meta file
                        let _ = std::fs::remove_file(meta_path_for(&path));
                    }
            }
        }
    }

    removed
}

// ─── Index building ──────────────────────────────────────────────────

pub fn build_index(args: &IndexArgs) -> Result<FileIndex, SearchError> {
    let root = fs::canonicalize(&args.dir)
        .map_err(|_| SearchError::DirNotFound(args.dir.clone()))?;
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

    Ok(index)
}

// ─── Content index building ──────────────────────────────────────────

pub fn build_content_index(args: &ContentIndexArgs) -> Result<ContentIndex, SearchError> {
    let root = fs::canonicalize(&args.dir)
        .map_err(|_| SearchError::DirNotFound(args.dir.clone()))?;
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

    let extensions: Arc<[String]> = extensions.into();
    let file_data: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    let read_errors = std::sync::atomic::AtomicUsize::new(0);
    let lossy_file_count = std::sync::atomic::AtomicUsize::new(0);

    builder.build_parallel().run(|| {
        let extensions = Arc::clone(&extensions);
        let file_data = &file_data;
        let read_errors = &read_errors;
        let lossy_file_count = &lossy_file_count;
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
                    Ok((content, was_lossy)) => {
                        if was_lossy {
                            lossy_file_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            eprintln!("[content-index] WARNING: lossy UTF-8 conversion: {}", path);
                        }
                        file_data.lock().unwrap_or_else(|e| e.into_inner()).push((path, content));
                    }
                    Err(e) => {
                        read_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        eprintln!("[content-index] WARNING: failed to read file: {} — {}", path, e);
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    let file_data = recover_mutex(file_data, "content-index");
    let file_count = file_data.len();
    let read_errors = read_errors.load(std::sync::atomic::Ordering::Relaxed);
    let lossy_file_count = lossy_file_count.load(std::sync::atomic::Ordering::Relaxed);
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
        "Indexed {} files, {} unique tokens ({} total) in {:.3}s ({} read errors, {} lossy-utf8)",
        file_count, unique_tokens, total_tokens, elapsed.as_secs_f64(),
        read_errors, lossy_file_count
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    Ok(ContentIndex {
        root: root_str,
        created_at: now,
        max_age_secs: args.max_age_hours * 3600,
        files,
        index,
        total_tokens,
        extensions: extensions.to_vec(),
        file_token_counts,
        trigram,
        trigram_dirty: false,
        path_to_id: None,
        read_errors,
        lossy_file_count,
    })
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
#[path = "index_tests.rs"]
mod index_tests;
