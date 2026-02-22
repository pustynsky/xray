//! Git history cache — compact in-memory representation for sub-millisecond queries.
//!
//! Replaces per-file `git log` CLI calls (2-6 sec each) with HashMap lookups (<1 ms).
//! Design: [`git-history-cache-design.md`](../../user-stories/git-history-cache-design.md)
//!
//! ## Module isolation
//!
//! This module depends ONLY on `std`, `serde`, and serialization crates (bincode, lz4_flex).
//! It does NOT import from `src/index.rs`, `src/definitions/`, or `src/mcp/`.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

// ─── Constants ──────────────────────────────────────────────────────

/// Cache format version. Bump when struct layout changes incompatibly.
pub const FORMAT_VERSION: u32 = 1;

/// Field separator in git log format — U+241E (SYMBOL FOR RECORD SEPARATOR).
/// Same as used in [`mod.rs`](super) for consistency. Never appears in commit data.
const FIELD_SEP: &str = "␞";

/// Commit line prefix in git log output.
const COMMIT_PREFIX: &str = "COMMIT:";

// ─── Core types ─────────────────────────────────────────────────────

/// Compact commit metadata — optimized for minimal RAM usage.
///
/// Field order is chosen to minimize padding with default Rust repr.
/// See design doc §2.1 for the 38-byte target (actual size may differ
/// due to alignment — verified in unit tests).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitMeta {
    /// Unix timestamp (seconds since epoch). 8 bytes, 8-align.
    pub timestamp: i64,
    /// SHA-1 hash as raw bytes (not hex string). 20 bytes.
    pub hash: [u8; 20],
    /// Offset into [`GitHistoryCache::subjects`] string pool.
    pub subject_offset: u32,
    /// Length of subject in subjects pool (u32 per debate-08).
    pub subject_len: u32,
    /// Index into [`GitHistoryCache::authors`]. Max 65,535 unique authors.
    pub author_idx: u16,
}

/// Author name + email (deduplicated in the author pool).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AuthorEntry {
    pub name: String,
    pub email: String,
}

/// Git history cache — compact in-memory representation.
///
/// 50K commits × ~65K files ≈ 5-10 MB RAM.
/// Storage: `Arc<RwLock<Option<GitHistoryCache>>>` in the MCP server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHistoryCache {
    /// Cache format version. Mismatch → full rebuild.
    pub format_version: u32,
    /// SHA-1 hex of HEAD when cache was built. Used for invalidation.
    pub head_hash: String,
    /// Branch name the cache was built from (main/master/develop/trunk).
    pub branch: String,
    /// Timestamp when cache was built (seconds since epoch).
    pub built_at: u64,
    /// All commits. Index into this vec = "commit ID" used in file_commits.
    pub commits: Vec<CommitMeta>,
    /// Author pool — deduplicated (name, email) pairs.
    pub authors: Vec<AuthorEntry>,
    /// Subject pool — all commit subjects concatenated.
    pub subjects: String,
    /// Main index: normalized file path → vec of commit indices.
    /// Keys are as-is from git output (forward slashes, repo-relative).
    pub file_commits: HashMap<String, Vec<u32>>,
}

// ─── Query return types ─────────────────────────────────────────────

/// Information about a single commit (returned from cache queries).
#[derive(Clone, Debug)]
pub struct CommitInfo {
    /// SHA-1 hash as 40-char hex string.
    pub hash: String,
    /// Unix timestamp (seconds since epoch).
    pub timestamp: i64,
    /// Author display name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Commit subject line.
    pub subject: String,
}

/// Aggregated author statistics for a file or directory.
#[derive(Clone, Debug)]
pub struct AuthorSummary {
    pub name: String,
    pub email: String,
    pub commit_count: usize,
    pub first_commit_timestamp: i64,
    pub last_commit_timestamp: i64,
}

/// File activity summary within a directory.
#[derive(Clone, Debug)]
pub struct FileActivity {
    pub file_path: String,
    pub commit_count: usize,
    pub last_modified: i64,
    pub authors: Vec<String>,
}

// ─── Builder (used during streaming parse) ──────────────────────────

/// Accumulates data during streaming parse of git log output.
/// Converted to [`GitHistoryCache`] after parsing is complete.
pub struct GitHistoryCacheBuilder {
    commits: Vec<CommitMeta>,
    authors: Vec<AuthorEntry>,
    author_map: HashMap<(String, String), u16>,
    subjects: String,
    file_commits: HashMap<String, Vec<u32>>,
    current_commit_idx: Option<u32>,
}

impl GitHistoryCacheBuilder {
    pub fn new() -> Self {
        Self {
            commits: Vec::new(),
            authors: Vec::new(),
            author_map: HashMap::new(),
            subjects: String::new(),
            file_commits: HashMap::new(),
            current_commit_idx: None,
        }
    }

    /// Get or insert an author, returning the index. Errors if >65535 unique authors.
    fn intern_author(&mut self, name: &str, email: &str) -> Result<u16, String> {
        let key = (name.to_string(), email.to_string());
        if let Some(&idx) = self.author_map.get(&key) {
            return Ok(idx);
        }
        if self.authors.len() >= 65535 {
            return Err(format!(
                "Too many unique authors (>65535). Cache cannot represent this repository. \
                 Last author: {} <{}>",
                name, email
            ));
        }
        let idx = self.authors.len() as u16;
        self.authors.push(AuthorEntry {
            name: name.to_string(),
            email: email.to_string(),
        });
        self.author_map.insert(key, idx);
        Ok(idx)
    }

    /// Append a commit subject to the subject pool, returning (offset, len).
    fn intern_subject(&mut self, subject: &str) -> (u32, u32) {
        let offset = self.subjects.len() as u32;
        self.subjects.push_str(subject);
        let len = subject.len() as u32;
        (offset, len)
    }

    /// Add a commit to the builder.
    fn add_commit(
        &mut self,
        hash: [u8; 20],
        timestamp: i64,
        author_name: &str,
        author_email: &str,
        subject: &str,
    ) -> Result<(), String> {
        let author_idx = self.intern_author(author_name, author_email)?;
        let (subject_offset, subject_len) = self.intern_subject(subject);

        let commit_idx = self.commits.len() as u32;
        self.commits.push(CommitMeta {
            timestamp,
            hash,
            subject_offset,
            subject_len,
            author_idx,
        });
        self.current_commit_idx = Some(commit_idx);
        Ok(())
    }

    /// Add a file path associated with the current commit.
    fn add_file(&mut self, file_path: &str) {
        if let Some(commit_idx) = self.current_commit_idx {
            self.file_commits
                .entry(file_path.to_string())
                .or_default()
                .push(commit_idx);
        }
    }

    /// Convert builder into a finalized cache.
    pub(crate) fn build(mut self, head_hash: String, branch: String) -> GitHistoryCache {
        // Shrink allocations to fit
        self.commits.shrink_to_fit();
        self.authors.shrink_to_fit();
        self.subjects.shrink_to_fit();
        for list in self.file_commits.values_mut() {
            list.shrink_to_fit();
        }
        self.file_commits.shrink_to_fit();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_secs();

        GitHistoryCache {
            format_version: FORMAT_VERSION,
            head_hash,
            branch,
            built_at: now,
            commits: self.commits,
            authors: self.authors,
            subjects: self.subjects,
            file_commits: self.file_commits,
        }
    }
}

// ─── Hex utilities ──────────────────────────────────────────────────

/// Parse a 40-char hex SHA-1 string into [u8; 20].
pub fn hex_to_bytes(hex: &str) -> Result<[u8; 20], String> {
    if hex.len() != 40 {
        return Err(format!("Invalid SHA-1 hex length: {} (expected 40)", hex.len()));
    }
    let mut bytes = [0u8; 20];
    for i in 0..20 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("Invalid hex at position {}: {}", i * 2, e))?;
    }
    Ok(bytes)
}

/// Convert [u8; 20] to a 40-char lowercase hex string.
pub fn bytes_to_hex(bytes: &[u8; 20]) -> String {
    let mut hex = String::with_capacity(40);
    for b in bytes {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

// ─── Path normalization ─────────────────────────────────────────────

impl GitHistoryCache {
    /// Normalize a query path for cache lookup.
    ///
    /// Rules (from design doc §3.1, debate-08):
    /// - `\` → `/` (Windows backslashes)
    /// - Strip leading `./`
    /// - `"."` → `""` (root)
    /// - Strip trailing `/`
    /// - Collapse `//` → `/`
    /// - `.trim()` whitespace
    pub fn normalize_path(path: &str) -> String {
        let path = path.trim();
        if path.is_empty() {
            return String::new();
        }

        // Replace backslashes with forward slashes
        let mut result = path.replace('\\', "/");

        // Collapse double slashes
        while result.contains("//") {
            result = result.replace("//", "/");
        }

        // Strip leading "./"
        while result.starts_with("./") {
            result = result[2..].to_string();
        }

        // "." alone means root
        if result == "." {
            return String::new();
        }

        // Strip trailing "/"
        while result.ends_with('/') {
            result.pop();
        }

        result
    }
}

// ─── Streaming parser ───────────────────────────────────────────────

/// Parse git log output line by line (streaming).
///
/// Expected format: `--format=COMMIT:%H␞%at␞%aE␞%aN␞%s` with `--name-only`.
///
/// Parsing rules:
/// - Lines starting with `COMMIT:` are commit headers
/// - Split header by `␞` (U+241E) — NOT by `|`
/// - Subject is the last field — use `fields[4..].join(sep)` as defense
/// - Non-empty lines after a commit header are file paths
/// - Empty lines separate commits
pub fn parse_git_log_stream(
    reader: impl BufRead,
    builder: &mut GitHistoryCacheBuilder,
) -> Result<(), String> {
    let mut commit_count: u64 = 0;
    let progress_start = std::time::Instant::now();
    let mut last_progress = std::time::Instant::now();

    for line_result in reader.lines() {
        let line = line_result.map_err(|e| format!("IO error reading git log: {}", e))?;

        if line.starts_with(COMMIT_PREFIX) {
            // Parse commit header: COMMIT:<hash>␞<timestamp>␞<email>␞<name>␞<subject...>
            let header = &line[COMMIT_PREFIX.len()..];
            let fields: Vec<&str> = header.split(FIELD_SEP).collect();

            if fields.len() < 5 {
                // Malformed line — skip silently (robustness)
                eprintln!(
                    "[git-cache] Warning: malformed commit line ({} fields, expected >=5): {}",
                    fields.len(),
                    &line[..line.len().min(100)]
                );
                builder.current_commit_idx = None;
                continue;
            }

            let hash_hex = fields[0].trim();
            let timestamp_str = fields[1].trim();
            let email = fields[2].trim();
            let name = fields[3].trim();
            // Subject is the last field — rejoin in case ␞ appeared in subject
            let subject = fields[4..].join(FIELD_SEP);
            let subject = subject.trim();

            let hash = match hex_to_bytes(hash_hex) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("[git-cache] Warning: skipping commit with bad hash: {}", e);
                    builder.current_commit_idx = None;
                    continue;
                }
            };

            let timestamp: i64 = match timestamp_str.parse() {
                Ok(t) => t,
                Err(e) => {
                    eprintln!(
                        "[git-cache] Warning: skipping commit with bad timestamp '{}': {}",
                        timestamp_str, e
                    );
                    builder.current_commit_idx = None;
                    continue;
                }
            };

            builder.add_commit(hash, timestamp, name, email, subject)?;
            commit_count += 1;

            // Progress logging every 10K commits or every 10 seconds
            if commit_count % 10_000 == 0 || last_progress.elapsed().as_secs() >= 10 {
                let elapsed = progress_start.elapsed();
                eprintln!(
                    "[git-cache] Progress: {} commits parsed ({:.1}s elapsed)...",
                    commit_count,
                    elapsed.as_secs_f64()
                );
                last_progress = std::time::Instant::now();
            }
        } else if !line.is_empty() {
            // Non-empty line after a commit = file path
            let file_path = line.trim();
            if !file_path.is_empty() {
                builder.add_file(file_path);
            }
        }
        // Empty lines are commit separators — nothing to do
    }

    Ok(())
}

// ─── Path prefix matching ───────────────────────────────────────────

/// Check if a file path matches a query path prefix.
///
/// Design doc §4: `== path || starts_with(path + "/")`
/// Ensures `src` doesn't match `src2`.
fn matches_path_prefix(file_path: &str, query_path: &str) -> bool {
    if query_path.is_empty() {
        return true; // match all (entire repo)
    }
    file_path == query_path || file_path.starts_with(&format!("{}/", query_path))
}

// ─── Public API ─────────────────────────────────────────────────────

impl GitHistoryCache {
    /// Save cache to disk using bincode + LZ4 compression (via save_compressed).
    /// Uses atomic write: write to temp file, then rename.
    pub fn save_to_disk(&self, path: &std::path::Path) -> Result<(), String> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create cache directory: {}", e))?;
        }

        // Atomic write: write to temp file first, then rename
        let tmp_path_str = format!("{}.tmp", path.display());
        let tmp_path = std::path::PathBuf::from(&tmp_path_str);

        crate::index::save_compressed(&tmp_path, self, "git-history")
            .map_err(|e| format!("Failed to save git cache: {}", e))?;

        std::fs::rename(&tmp_path, path)
            .map_err(|e| format!("Failed to rename temp cache file: {}", e))?;

        // Write sidecar .meta file (best-effort)
        crate::index::save_index_meta(path, &crate::index::git_cache_meta(self));

        Ok(())
    }

    /// Load cache from disk using bincode + LZ4 decompression.
    /// Returns Err on any error (corrupt file, wrong version) — caller does full rebuild.
    pub fn load_from_disk(path: &std::path::Path) -> Result<Self, String> {
        let cache: Self = crate::index::load_compressed(path, "git-history")
            .map_err(|e| format!("Failed to load git cache: {}", e))?;

        // Validate format version
        if cache.format_version != FORMAT_VERSION {
            return Err(format!(
                "Git cache format version mismatch: file has {}, expected {}",
                cache.format_version, FORMAT_VERSION
            ));
        }

        Ok(cache)
    }

    /// Construct the cache file path for a given directory.
    /// Follows the same naming convention as `.word-search` and `.code-structure` files:
    /// `<semantic_prefix>_<hash>.git-history`
    pub fn cache_path_for(dir: &str, index_base: &std::path::Path) -> std::path::PathBuf {
        let canonical = std::fs::canonicalize(dir)
            .unwrap_or_else(|_| std::path::PathBuf::from(dir));
        let hash = search::stable_hash(&[
            canonical.to_string_lossy().as_bytes(),
            b"git-history", // distinguish from content/def indexes
        ]);
        let prefix = search::extract_semantic_prefix(&canonical);
        index_base.join(format!("{}_{:08x}.git-history", prefix, hash as u32))
    }

    /// Check if the cached HEAD hash is an ancestor of the current HEAD.
    /// Used to decide between incremental update and full rebuild.
    pub fn is_ancestor(repo_path: &Path, old_head: &str, new_head: &str) -> bool {
        Command::new("git")
            .args(["merge-base", "--is-ancestor", old_head, new_head])
            .current_dir(repo_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Check if a git object exists in the repository.
    /// Used to detect re-cloned repos where the cached HEAD is gone.
    pub fn object_exists(repo_path: &Path, hash: &str) -> bool {
        Command::new("git")
            .args(["cat-file", "-t", hash])
            .current_dir(repo_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Build cache by running `git log` and parsing output.
    ///
    /// Spawns `git log --name-only --no-renames` as a child process,
    /// parses output line-by-line (streaming — no 163 MB in RAM).
    pub fn build(repo_path: &Path, branch: &str) -> Result<Self, String> {
        // Check for commit-graph and emit hint if missing
        let commit_graph_path = repo_path.join(".git/objects/info/commit-graph");
        if !commit_graph_path.exists() {
            eprintln!(
                "[git-cache] Hint: run 'git commit-graph write --reachable' to speed up git history indexing by 2-5x"
            );
        }

        // Get HEAD hash for the branch
        let head_hash = Self::get_branch_head(repo_path, branch)?;

        // Spawn git log with streaming output
        let mut child = Command::new("git")
            .args([
                "-c",
                "core.quotePath=false", // raw UTF-8 paths
                "log",
                "--name-only",
                "--no-renames",
                &format!("--format={}%H{}%at{}%aE{}%aN{}%s",
                    COMMIT_PREFIX, FIELD_SEP, FIELD_SEP, FIELD_SEP, FIELD_SEP),
                branch,
            ])
            .current_dir(repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn git log: {}", e))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture git log stdout".to_string())?;

        let reader = std::io::BufReader::new(stdout);
        let mut builder = GitHistoryCacheBuilder::new();

        parse_git_log_stream(reader, &mut builder)?;

        // Wait for git to finish
        let status = child
            .wait()
            .map_err(|e| format!("Failed to wait for git log: {}", e))?;

        if !status.success() {
            return Err(format!("git log exited with status: {}", status));
        }

        let cache = builder.build(head_hash, branch.to_string());

        eprintln!(
            "[git-cache] Built cache: {} commits, {} authors, {} files, subjects={} bytes",
            cache.commits.len(),
            cache.authors.len(),
            cache.file_commits.len(),
            cache.subjects.len()
        );

        Ok(cache)
    }

    /// Query file history — returns commits touching this file.
    ///
    /// Order: filter by date/author/message → sort by timestamp descending → truncate to maxResults.
    ///
    /// Returns `(commits, total_count)` where `total_count` is the number of matching
    /// commits BEFORE applying `max_results` truncation. This allows callers to know
    /// the true total even when results are limited.
    pub fn query_file_history(
        &self,
        file: &str,
        max_results: Option<usize>,
        from: Option<i64>,
        to: Option<i64>,
        author_filter: Option<&str>,
        message_filter: Option<&str>,
    ) -> (Vec<CommitInfo>, usize) {
        let normalized = Self::normalize_path(file);

        let commit_ids = match self.file_commits.get(&normalized) {
            Some(ids) => ids,
            None => return (Vec::new(), 0),
        };

        // Pre-compute matching author indices for O(1) lookup
        let matching_author_idxs: Option<std::collections::HashSet<u16>> = author_filter.map(|pattern| {
            let pattern_lower = pattern.to_lowercase();
            self.authors.iter().enumerate()
                .filter(|(_, a)| a.name.to_lowercase().contains(&pattern_lower) || a.email.to_lowercase().contains(&pattern_lower))
                .map(|(i, _)| i as u16)
                .collect()
        });

        let message_filter_lower = message_filter.map(|m| m.to_lowercase());

        let mut commits: Vec<CommitInfo> = commit_ids
            .iter()
            .filter_map(|&idx| {
                let meta = self.commits.get(idx as usize)?;

                // Filter by date range
                if let Some(from_ts) = from {
                    if meta.timestamp < from_ts {
                        return None;
                    }
                }
                if let Some(to_ts) = to {
                    if meta.timestamp > to_ts {
                        return None;
                    }
                }

                // Filter by author
                if let Some(ref idxs) = matching_author_idxs {
                    if !idxs.contains(&meta.author_idx) {
                        return None;
                    }
                }

                // Filter by message
                if let Some(ref msg_pattern) = message_filter_lower {
                    let subject = self.get_subject(meta);
                    if !subject.to_lowercase().contains(msg_pattern.as_str()) {
                        return None;
                    }
                }

                Some(self.commit_meta_to_info(meta))
            })
            .collect();

        // Sort by timestamp descending (newest first)
        commits.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        // Count total BEFORE truncation
        let total_count = commits.len();

        // Truncate to maxResults
        if let Some(max) = max_results {
            if max > 0 {
                commits.truncate(max);
            }
        }

        (commits, total_count)
    }

    /// Query authors — aggregate authors for a file or directory.
    ///
    /// For a directory path, aggregates across all files with matching prefix.
    /// Supports optional filtering by author, message, and date range.
    pub fn query_authors(
        &self,
        path: &str,
        author_filter: Option<&str>,
        message_filter: Option<&str>,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Vec<AuthorSummary> {
        let normalized = Self::normalize_path(path);

        // Pre-compute matching author indices for O(1) lookup
        let matching_author_idxs: Option<std::collections::HashSet<u16>> = author_filter.map(|pattern| {
            let pattern_lower = pattern.to_lowercase();
            self.authors.iter().enumerate()
                .filter(|(_, a)| a.name.to_lowercase().contains(&pattern_lower) || a.email.to_lowercase().contains(&pattern_lower))
                .map(|(i, _)| i as u16)
                .collect()
        });

        let message_filter_lower = message_filter.map(|m| m.to_lowercase());

        // Collect all commit indices matching the path
        let mut all_commit_ids: Vec<u32> = Vec::new();

        for (file_path, commit_ids) in &self.file_commits {
            if matches_path_prefix(file_path, &normalized) {
                all_commit_ids.extend(commit_ids);
            }
        }

        // Deduplicate commit IDs (a commit may touch multiple files in a directory)
        all_commit_ids.sort();
        all_commit_ids.dedup();

        // Aggregate by author: idx → (count, first_ts, last_ts)
        let mut author_stats: HashMap<u16, (usize, i64, i64)> = HashMap::new();

        for &commit_idx in &all_commit_ids {
            if let Some(meta) = self.commits.get(commit_idx as usize) {
                // Filter by date range
                if let Some(from_ts) = from {
                    if meta.timestamp < from_ts {
                        continue;
                    }
                }
                if let Some(to_ts) = to {
                    if meta.timestamp > to_ts {
                        continue;
                    }
                }

                // Filter by author
                if let Some(ref idxs) = matching_author_idxs {
                    if !idxs.contains(&meta.author_idx) {
                        continue;
                    }
                }

                // Filter by message
                if let Some(ref msg_pattern) = message_filter_lower {
                    let subject = self.get_subject(meta);
                    if !subject.to_lowercase().contains(msg_pattern.as_str()) {
                        continue;
                    }
                }

                let entry = author_stats.entry(meta.author_idx).or_insert((0, i64::MAX, i64::MIN));
                entry.0 += 1;
                if meta.timestamp < entry.1 {
                    entry.1 = meta.timestamp; // earliest
                }
                if meta.timestamp > entry.2 {
                    entry.2 = meta.timestamp; // latest
                }
            }
        }

        let mut summaries: Vec<AuthorSummary> = author_stats
            .into_iter()
            .filter_map(|(author_idx, (count, first_ts, last_ts))| {
                let author = self.authors.get(author_idx as usize)?;
                Some(AuthorSummary {
                    name: author.name.clone(),
                    email: author.email.clone(),
                    commit_count: count,
                    first_commit_timestamp: first_ts,
                    last_commit_timestamp: last_ts,
                })
            })
            .collect();

        // Sort by commit count descending
        summaries.sort_by(|a, b| b.commit_count.cmp(&a.commit_count));

        summaries
    }

    /// Query activity — files changed in a directory within a time range.
    ///
    /// Uses path prefix matching: `== path || starts_with(path + "/")`.
    /// Supports optional filtering by author and message.
    pub fn query_activity(
        &self,
        path: &str,
        from: Option<i64>,
        to: Option<i64>,
        author_filter: Option<&str>,
        message_filter: Option<&str>,
    ) -> Vec<FileActivity> {
        let normalized = Self::normalize_path(path);

        // Pre-compute matching author indices for O(1) lookup
        let matching_author_idxs: Option<std::collections::HashSet<u16>> = author_filter.map(|pattern| {
            let pattern_lower = pattern.to_lowercase();
            self.authors.iter().enumerate()
                .filter(|(_, a)| a.name.to_lowercase().contains(&pattern_lower) || a.email.to_lowercase().contains(&pattern_lower))
                .map(|(i, _)| i as u16)
                .collect()
        });

        let message_filter_lower = message_filter.map(|m| m.to_lowercase());

        let mut activities: Vec<FileActivity> = Vec::new();

        for (file_path, commit_ids) in &self.file_commits {
            if !matches_path_prefix(file_path, &normalized) {
                continue;
            }

            let matching_commits: Vec<&CommitMeta> = commit_ids
                .iter()
                .filter_map(|&idx| {
                    let meta = self.commits.get(idx as usize)?;
                    if let Some(from_ts) = from {
                        if meta.timestamp < from_ts {
                            return None;
                        }
                    }
                    if let Some(to_ts) = to {
                        if meta.timestamp > to_ts {
                            return None;
                        }
                    }

                    // Filter by author
                    if let Some(ref idxs) = matching_author_idxs {
                        if !idxs.contains(&meta.author_idx) {
                            return None;
                        }
                    }

                    // Filter by message
                    if let Some(ref msg_pattern) = message_filter_lower {
                        let subject = self.get_subject(meta);
                        if !subject.to_lowercase().contains(msg_pattern.as_str()) {
                            return None;
                        }
                    }

                    Some(meta)
                })
                .collect();

            if matching_commits.is_empty() {
                continue;
            }

            // Find last modified timestamp
            let last_modified = matching_commits
                .iter()
                .map(|m| m.timestamp)
                .max()
                .unwrap_or(0);

            // Collect unique authors
            let mut author_set: Vec<u16> = matching_commits.iter().map(|m| m.author_idx).collect();
            author_set.sort();
            author_set.dedup();

            let authors: Vec<String> = author_set
                .into_iter()
                .filter_map(|idx| {
                    let author = self.authors.get(idx as usize)?;
                    Some(format!("{} <{}>", author.name, author.email))
                })
                .collect();

            activities.push(FileActivity {
                file_path: file_path.clone(),
                commit_count: matching_commits.len(),
                last_modified,
                authors,
            });
        }

        // Sort by last_modified descending
        activities.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));

        activities
    }

    /// Check if cache is still valid for the given HEAD hash.
    pub fn is_valid_for(&self, head_hash: &str) -> bool {
        self.head_hash == head_hash && self.format_version == FORMAT_VERSION
    }

    /// Detect the default branch name by trying main, master, develop, trunk.
    pub fn detect_default_branch(repo_path: &Path) -> Result<String, String> {
        for branch in &["main", "master", "develop", "trunk"] {
            let output = Command::new("git")
                .args(["rev-parse", "--verify", branch])
                .current_dir(repo_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            if output.map(|s| s.success()).unwrap_or(false) {
                return Ok(branch.to_string());
            }
        }
        // Fallback to HEAD
        Ok("HEAD".to_string())
    }

    // ─── Internal helpers ───────────────────────────────────────────

    /// Get subject string for a commit meta.
    fn get_subject(&self, meta: &CommitMeta) -> &str {
        let start = meta.subject_offset as usize;
        let end = start + meta.subject_len as usize;
        if end <= self.subjects.len() {
            &self.subjects[start..end]
        } else {
            "<invalid>"
        }
    }

    /// Get the HEAD hash of a branch.
    fn get_branch_head(repo_path: &Path, branch: &str) -> Result<String, String> {
        let output = Command::new("git")
            .args(["rev-parse", branch])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git rev-parse: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git rev-parse {} failed: {}", branch, stderr.trim()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Convert a [`CommitMeta`] to a [`CommitInfo`] (resolving indices).
    fn commit_meta_to_info(&self, meta: &CommitMeta) -> CommitInfo {
        let author = self
            .authors
            .get(meta.author_idx as usize)
            .cloned()
            .unwrap_or_else(|| AuthorEntry {
                name: "<unknown>".to_string(),
                email: "<unknown>".to_string(),
            });

        let subject_start = meta.subject_offset as usize;
        let subject_end = subject_start + meta.subject_len as usize;
        let subject = if subject_end <= self.subjects.len() {
            self.subjects[subject_start..subject_end].to_string()
        } else {
            "<invalid subject>".to_string()
        };

        CommitInfo {
            hash: bytes_to_hex(&meta.hash),
            timestamp: meta.timestamp,
            author_name: author.name,
            author_email: author.email,
            subject,
        }
    }
}

// ─── Public builder access for testing ──────────────────────────────

/// Create a cache from pre-parsed data (for unit testing without git).
///
/// This is the builder pattern exposed for tests — not used in production
/// where [`GitHistoryCache::build()`] is the entry point.
#[cfg(test)]
impl GitHistoryCache {
    /// Create a builder for constructing a cache without git CLI.
    pub fn builder() -> GitHistoryCacheBuilder {
        GitHistoryCacheBuilder::new()
    }

    /// Finalize a builder into a cache (for testing).
    pub fn from_builder(
        builder: GitHistoryCacheBuilder,
        head_hash: String,
        branch: String,
    ) -> Self {
        builder.build(head_hash, branch)
    }
}

// ─── Hex conversion public for tests ────────────────────────────────

#[cfg(test)]
pub use self::hex_to_bytes as parse_hex_hash;
#[cfg(test)]
pub use self::bytes_to_hex as format_hex_hash;