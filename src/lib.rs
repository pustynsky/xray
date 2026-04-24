//! # Xray — High-Performance Code Intelligence Engine
//!
//! Inverted index + AST-based code intelligence engine for large-scale codebases.
//! Sub-microsecond content search, structural code navigation, and native MCP server.
//!
//! ## Library usage
//!
//! This crate is primarily a CLI tool / MCP server, but core types and functions
//! are exposed as a library for benchmarking and integration testing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Default minimum token length used for indexing and search.
/// Tokens shorter than this are discarded during tokenization.
pub const DEFAULT_MIN_TOKEN_LEN: usize = 2;

// ─── Clock ──────────────────────────────────────────────────────────

/// Return the current Unix timestamp in seconds, or `None` if the system
/// clock is set before the Unix epoch.
///
/// **Why `Option` and not `unwrap_or(0)`:** the previous pattern across
/// `cli/info.rs`, `build.rs`, and the watcher silently substituted `0` on
/// clock failure. Downstream `now.saturating_sub(created_at)` then evaluated
/// to `0` for *every* cache, so `xray info` reported every index as "0.0h
/// ago" and never stale — a misleading "all-fresh" view while the watcher
/// silently missed changes. Returning `None` lets callers either surface
/// "unknown age" honestly or treat the failure as "ancient" (force-stale).
///
/// On any reasonably-configured host this returns `Some` always.
#[must_use]
pub fn current_unix_secs() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

// ─── Stable hashing ─────────────────────────────────────────────────

/// Stable FNV-1a hash (deterministic across Rust versions, unlike `DefaultHasher`).
///
/// Accepts multiple byte slices that are fed into the hash sequentially,
/// allowing callers to combine directory path + extension list, etc.
#[must_use]
pub fn stable_hash(parts: &[&[u8]]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
    let mut hash = FNV_OFFSET;
    for part in parts {
        for &byte in *part {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

// ─── Core public types ───────────────────────────────────────────────

/// Strip the `\\?\` extended-length path prefix that Windows canonicalize adds,
/// and normalize path separators to forward slashes for cross-platform consistency.
#[must_use]
pub fn clean_path(p: &str) -> String {
    p.strip_prefix(r"\\?\").unwrap_or(p).replace('\\', "/")
}

/// Compare two path strings using platform-appropriate case sensitivity.
///
/// On Windows the comparison is case-insensitive (ASCII), matching the
/// behaviour of NTFS / ReFS for the common code-path identifiers we deal
/// with (drive letters, ASCII directory names). On Unix the comparison is
/// strictly case-sensitive, matching the underlying filesystem semantics.
///
/// Use this helper for any equality check between two cleaned/canonicalized
/// path strings (e.g. cache `root` lookups). Mixing raw `==` and
/// `eq_ignore_ascii_case` across the codebase has produced cross-platform
/// bugs where indexes saved under one casing could not be found again
/// (orphan caches under `%LOCALAPPDATA%\xray`).
#[must_use]
#[inline]
pub fn path_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Canonicalize a path, falling back to the raw input on failure.
///
/// Wraps `std::fs::canonicalize` with a `tracing::warn!` on error so the
/// silent fallback (broken symlink, permission denied, drive disconnected)
/// becomes visible in the structured log. The fallback itself is preserved
/// — many call sites use this for cache-key derivation where any reasonable
/// path is better than `panic!`, but the resulting orphan-index risk should
/// at least be observable to operators.
///
/// Use this everywhere instead of the raw
/// `fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p))` pattern.
#[must_use]
pub fn canonicalize_or_warn(dir: &str) -> std::path::PathBuf {
    match std::fs::canonicalize(dir) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                target: "xray::path",
                dir = %dir,
                error = %e,
                "canonicalize failed; falling back to raw input \
                 (cache key may diverge from canonical path)",
            );
            std::path::PathBuf::from(dir)
        }
    }
}

/// Check if `path` is logically inside `root` (workspace / server --dir),
/// correctly handling **symlinked subdirectories** (e.g., `docs/personal` →
/// `D:\Personal\…`) that the indexer reaches via `WalkBuilder::follow_links`.
///
/// Logic:
/// 1. **Logical-path comparison first** (no `canonicalize`). This matches what
///    the indexer sees: a file under `<root>/<symlink_dir>/foo.md` is indexed
///    by its logical path, not by the symlink target.
/// 2. **Canonical-path fallback** for two cases where logical comparison fails:
///    - 8.3 short names on Windows (`PROGRA~1\App` vs `Program Files\App`).
///    - Mixed-separator inputs that don't share a textual prefix with `root`.
/// 3. **Path-traversal protection**: if `path` contains a `..` segment, the
///    canonical form is **required** even if the logical comparison succeeded
///    — otherwise `<root>/sub/../../../etc/passwd` would be accepted as
///    "inside root" by string prefix.
///
/// Both `path` and `root` should be absolute. Comparison is case-insensitive
/// (Windows-safe). Returns `true` if `path == root` or `path` lives below `root`.
///
/// # Examples
/// - `path="C:/Repos/X/src/main.rs"`, `root="C:/Repos/X"` → `true` (logical match)
/// - `path="C:/Repos/X/docs/personal/foo.md"` (where `personal` is a symlink to
///   `D:/Personal/...`), `root="C:/Repos/X"` → `true` (logical match — file
///   is reached via the workspace path tree, exactly as the indexer sees it).
/// - `path="D:/Personal/foo.md"` (the symlink target itself), `root="C:/Repos/X"`
///   → `false` (the input is outside the workspace tree; pass the logical
///   `<root>/<symlink>/...` form if you want it accepted).
/// - `path="C:/Repos/X/src/../../../etc/passwd"`, `root="C:/Repos/X"` → `false`
///   (path traversal escapes via canonical fallback).
#[must_use]
pub fn is_path_within(path: &str, root: &str) -> bool {
    let root_norm = clean_path(root).to_lowercase();
    if root_norm.is_empty() {
        // LIB-013: refuse to accept anything when no boundary is provided.
        // Pre-fix this returned `true` ("no boundary, accept everything"),
        // which made forgetting to pass a real root a silent scope-bypass.
        // Callers that legitimately want "no boundary" must pass an explicit
        // sentinel (e.g. "/" on POSIX) so the intent is visible at the call site.
        return false;
    }
    let root_trimmed = root_norm.trim_end_matches('/');
    let root_with_sep = format!("{}/", root_trimmed);

    // `inside` checks an already-normalized lowercase candidate against root.
    let inside = |candidate: &str| -> bool {
        let c = candidate.trim_end_matches('/');
        c == root_trimmed || c.starts_with(&root_with_sep)
    };

    // Detect path traversal segments (`..`) so we can force canonical validation
    // even when the textual prefix happens to match.
    let has_traversal = path
        .split(['/', '\\'])
        .any(|seg| seg == "..");

    // Logical-path comparison first (matches `WalkBuilder::follow_links`).
    if !has_traversal {
        let logical = clean_path(path).to_lowercase();
        if inside(&logical) {
            return true;
        }
    }

    // Logical-with-traversal: collapse `..` purely textually against root, then
    // re-check inside. Without this, equivalent in-workspace paths produce
    // different verdicts only because of how they're written:
    //
    //   `nonexistent/dir`           → accepted via the no-traversal logical branch
    //   `src/../nonexistent/dir`    → previously rejected (canonical fallback fails
    //                                 on the missing leaf), now accepted as long as
    //                                 the resolved logical form stays inside root.
    //
    // Genuine escape (`../outside`, `../../etc/passwd`, ...) is still rejected:
    // the resolver returns None when `..` pops past the root, and any in-root
    // resolution that lands outside the workspace fails the `inside` check.
    // Symlink-escape risk is no worse than the pre-existing no-traversal logical
    // branch, which already trusts textual containment for `WalkBuilder` parity.
    //
    // We also stash the resolved (`..`-free) form in `safe_for_walkup` so the
    // walk-up canonical fallback below can run safely on `..`-bearing inputs
    // where the original path would silently lose its `..` markers via
    // `PathBuf::pop`/`file_name` and re-accept genuine escapes.
    let safe_for_walkup: Option<String> = if has_traversal {
        let resolved = resolve_dotdot_logical(path, root);
        if let Some(ref r) = resolved {
            let logical = r.to_lowercase();
            if inside(&logical) {
                return true;
            }
        }
        // `None` here means the resolver classified the input as a genuine
        // escape (`..` popped past root). We deliberately leave
        // `safe_for_walkup` as `None` so the walk-up branch below skips it
        // and the function returns `false` at the end.
        resolved
    } else {
        Some(path.to_string())
    };

    // Canonical fallback: handles 8.3 short names, traversal validation,
    // and arbitrary input shapes that don't share a textual prefix with root.
    if let Ok(canonical_path) = std::fs::canonicalize(path) {
        let canon = clean_path(&canonical_path.to_string_lossy()).to_lowercase();
        if let Ok(canonical_root) = std::fs::canonicalize(root) {
            let croot = clean_path(&canonical_root.to_string_lossy()).to_lowercase();
            let croot_trimmed = croot.trim_end_matches('/');
            let croot_with_sep = format!("{}/", croot_trimmed);
            let c = canon.trim_end_matches('/');
            return c == croot_trimmed || c.starts_with(&croot_with_sep);
        }
        // Root failed to canonicalize — compare canonical path against logical root.
        return inside(&canon);
    }

    // Walk-up canonical fallback for non-existent leaves (Windows 8.3 short/long
    // form mismatch). When the path itself doesn't exist (e.g. a brand-new
    // subdir the caller wants to enumerate), `canonicalize(path)` fails and
    // the textual `inside(logical)` check above can miss legitimate in-workspace
    // paths whose ancestor exists in a different short/long form than `root`.
    // Walk up from the leaf to the longest existing ancestor, canonicalize
    // THAT, then re-attach the unresolved tail and re-check containment.
    //
    // Concrete repro (windows-latest CI):
    //   path = `C:/Users/RUNNER~1/AppData/Local/Temp/xray_fast_test_.../nonexistent-but-inside`
    //   root = `C:/Users/runneradmin/AppData/Local/Temp/xray_fast_test_...`  (canonical)
    // Without this branch the textual `inside` rejects (`runner~1` ≠ `runneradmin`)
    // and the canonicalize(path) above fails on the non-existent leaf.
    //
    // SAFETY: this branch operates on `safe_for_walkup`, NOT on the raw
    // `path` argument. For `..`-bearing inputs we use the resolved form from
    // `resolve_dotdot_logical`, which has already classified genuine escapes
    // as `None` (the resolver returns `None` when `..` pops past root). For
    // confirmed escapes `safe_for_walkup` is `None` and we skip the walk-up
    // entirely, so a payload like `<root>/sub/../../outside` cannot get a
    // second chance via `PathBuf::pop`/`file_name` (which silently drops
    // `..` segments and would re-accept the escape — the bug that broke
    // `test_is_path_within_relative_dotdot_escape_still_rejected` on Linux
    // CI when this branch ran unconditionally on the raw `path`).
    if let Some(walkup_input) = safe_for_walkup.as_deref()
        && let Ok(canonical_root) = std::fs::canonicalize(root)
    {
        let croot = clean_path(&canonical_root.to_string_lossy()).to_lowercase();
        let croot_trimmed = croot.trim_end_matches('/');
        let croot_with_sep = format!("{}/", croot_trimmed);
        let mut p = std::path::PathBuf::from(walkup_input);
        let mut tail = std::path::PathBuf::new();
        // Bound the walk so a pathological input cannot stat the entire
        // ancestor chain. 64 segments far exceeds any realistic in-workspace path.
        for _ in 0..64 {
            if p.as_os_str().is_empty() || p.exists() { break; }
            if let Some(name) = p.file_name() {
                tail = std::path::PathBuf::from(name).join(&tail);
            }
            if !p.pop() { break; }
        }
        if !p.as_os_str().is_empty()
            && let Ok(canonical_anc) = std::fs::canonicalize(&p)
        {
            let with_tail = canonical_anc.join(&tail);
            let canon = clean_path(&with_tail.to_string_lossy()).to_lowercase();
            let c = canon.trim_end_matches('/');
            if c == croot_trimmed || c.starts_with(&croot_with_sep) {
                return true;
            }
        }
    }

    false
}

/// Resolve `..` segments in `path` purely textually, joining with `root` if
/// `path` is relative. Returns `None` when `..` pops past the root prefix
/// (genuine escape attempt). Used by [`is_path_within`] to give consistent
/// verdicts to equivalent in-workspace paths regardless of how `..` is
/// written, even when the leaf does not exist on disk yet.
fn resolve_dotdot_logical(path: &str, root: &str) -> Option<String> {
    use std::path::{Component, Path, PathBuf};

    let p = Path::new(path);
    let combined: PathBuf = if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(root).join(p)
    };

    let mut prefix: Option<std::ffi::OsString> = None;
    let mut has_root = false;
    let mut stack: Vec<std::ffi::OsString> = Vec::new();
    for comp in combined.components() {
        match comp {
            Component::Prefix(p) => prefix = Some(p.as_os_str().to_os_string()),
            Component::RootDir => has_root = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if stack.is_empty() {
                    // `..` would pop past the root prefix — escape attempt.
                    return None;
                }
                stack.pop();
            }
            Component::Normal(n) => stack.push(n.to_os_string()),
        }
    }

    let mut out = PathBuf::new();
    if let Some(p) = prefix {
        out.push(p);
    }
    if has_root {
        out.push(std::path::MAIN_SEPARATOR.to_string());
    }
    for s in stack {
        out.push(s);
    }
    Some(clean_path(&out.to_string_lossy()))
}


// ─── Index file naming ───────────────────────────────────────────────

/// Windows reserved device names that cannot be used as filenames.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL",
    "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Maximum length for the semantic prefix portion of index filenames.
const MAX_PREFIX_LEN: usize = 50;

/// Sanitize a string for safe use in a filename on Windows.
///
/// Rules:
/// 1. Characters not in `[a-zA-Z0-9_-]` are replaced with `_`
/// 2. All characters are lowercased for cross-platform consistency
/// 3. Windows reserved names (CON, NUL, etc.) get a `_` prefix
/// 4. Empty result becomes `_`
/// 5. Truncated to [`MAX_PREFIX_LEN`] characters
#[must_use]
pub fn sanitize_for_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();

    let sanitized = if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    };

    // Check Windows reserved names (case-insensitive)
    let upper = sanitized.to_uppercase();
    let sanitized = if WINDOWS_RESERVED.iter().any(|r| upper == *r) {
        format!("_{}", sanitized)
    } else {
        sanitized
    };

    // Truncate to max length (char-safe to avoid panicking on multi-byte chars)
    if sanitized.len() > MAX_PREFIX_LEN {
        sanitized.chars().take(MAX_PREFIX_LEN).collect::<String>()
    } else {
        sanitized
    }
}

/// Extract a human-readable semantic prefix from a canonical path for use in index filenames.
///
/// Rules based on the number of "Normal" path components (excluding drive prefix and root):
/// - 0 components (drive root like `C:\`) → drive letter (e.g., `c`)
/// - 1 component (e.g., `C:\test`) → `{drive_letter}_{name}` (e.g., `c_test`)
/// - 2+ components (e.g., `C:\Repos\PBI`) → `{second_to_last}_{last}` (e.g., `repos_pbi`)
///
/// Each component is sanitized via [`sanitize_for_filename`] before joining.
#[must_use]
pub fn extract_semantic_prefix(canonical: &std::path::Path) -> String {
    use std::path::Component;

    // Extract drive letter (Windows: first char of Prefix component)
    let drive_letter = canonical
        .components()
        .find_map(|c| {
            if let Component::Prefix(p) = c {
                let s = p.as_os_str().to_string_lossy();
                s.chars().next().filter(|ch| ch.is_ascii_alphabetic())
            } else {
                None
            }
        });

    // Collect Normal components (the meaningful directory names)
    let normals: Vec<String> = canonical
        .components()
        .filter_map(|c| {
            if let Component::Normal(s) = c {
                Some(s.to_string_lossy().to_string())
            } else {
                None
            }
        })
        .collect();

    match normals.len() {
        0 => {
            // Drive root: C:\ → "C"
            match drive_letter {
                Some(letter) => letter.to_lowercase().to_string(),
                None => "_".to_string(),
            }
        }
        1 => {
            // Single component: C:\test → "C_test"
            let name = sanitize_for_filename(&normals[0]);
            match drive_letter {
                Some(letter) => format!("{}_{}", letter.to_lowercase(), name),
                None => name,
            }
        }
        _ => {
            // Two+ components: take last two
            let parent = sanitize_for_filename(&normals[normals.len() - 2]);
            let name = sanitize_for_filename(&normals[normals.len() - 1]);
            let combined = format!("{}_{}", parent, name);
            // Truncate the combined result
            if combined.len() > MAX_PREFIX_LEN {
                combined.chars().take(MAX_PREFIX_LEN).collect::<String>()
            } else {
                combined
            }
        }
    }
}

/// Maximum file size accepted by [`read_file_lossy`] (LIB-007).
///
/// Files larger than this are rejected with `io::ErrorKind::Other` rather than
/// loaded into memory. The 50 MB ceiling covers the largest realistic source
/// files in the supported indexed languages while preventing OOM on giant
/// build artefacts (minified JS bundles, generated SQL dumps) that occasionally
/// land in indexed directories. UTF-16 inputs at this size still fit in roughly
/// ~150 MB of working memory after expansion (~3× worst case for ASCII-heavy
/// content), which is bounded and survivable.
pub const MAX_INDEX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Read a file as a String, handling BOM-detected encodings and lossy UTF-8 fallback.
///
/// Encoding detection order:
/// 1. UTF-16LE BOM (`FF FE`) → decode as UTF-16LE (strips BOM)
/// 2. UTF-16BE BOM (`FE FF`) → decode as UTF-16BE (strips BOM)
/// 3. UTF-8 BOM (`EF BB BF`) → strip BOM, decode as UTF-8
/// 4. No BOM → decode as UTF-8, with lossy fallback for invalid bytes
///
/// Returns `(content, was_lossy)` where `was_lossy` is true if replacement characters
/// were inserted during lossy UTF-8 conversion. Files successfully decoded via BOM
/// (UTF-16LE/BE/UTF-8 BOM) return `was_lossy = false`.
///
/// Files larger than [`MAX_INDEX_FILE_BYTES`] return `io::ErrorKind::Other`
/// (LIB-007); existing callers that use `.ok()?` simply skip the file, which is
/// the desired "oversized files are not indexed" behaviour.
pub fn read_file_lossy(path: &std::path::Path) -> std::io::Result<(String, bool)> {
    // LIB-007: bound peak memory before allocation. `std::fs::read` allocates
    // a Vec sized from the file metadata in one shot, so a 1 GB file produces
    // a 1 GB allocation regardless of what we do afterwards. Check size first.
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() > MAX_INDEX_FILE_BYTES
    {
        return Err(std::io::Error::other(
            format!(
                "file '{}' is {} bytes, exceeds MAX_INDEX_FILE_BYTES ({} bytes); skipped to avoid OOM",
                path.display(),
                meta.len(),
                MAX_INDEX_FILE_BYTES
            ),
        ));
    }
    let raw = std::fs::read(path)?;

    // UTF-16LE BOM: FF FE
    if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xFE {
        return Ok((decode_utf16le(&raw[2..]), false));
    }

    // UTF-16BE BOM: FE FF
    if raw.len() >= 2 && raw[0] == 0xFE && raw[1] == 0xFF {
        return Ok((decode_utf16be(&raw[2..]), false));
    }

    // UTF-8 BOM: EF BB BF — strip BOM, then decode as UTF-8
    let utf8_bytes = if raw.len() >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF {
        &raw[3..]
    } else {
        &raw
    };

    match std::str::from_utf8(utf8_bytes) {
        Ok(s) => Ok((s.to_string(), false)),
        Err(_) => Ok((String::from_utf8_lossy(utf8_bytes).into_owned(), true)),
    }
}

/// Decode UTF-16LE bytes (after BOM) into a String.
/// Uses `char::decode_utf16` for proper surrogate pair handling.
/// Invalid surrogate pairs are replaced with U+FFFD.
///
/// LIB-009: an odd-length input means the file was truncated or corrupted.
/// `chunks_exact(2)` would silently drop the trailing byte, losing the last
/// code unit. Append a replacement character so downstream tokenisation sees
/// the truncation rather than masking it.
fn decode_utf16le(bytes: &[u8]) -> String {
    let u16_iter = bytes.chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let mut s: String = char::decode_utf16(u16_iter)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect();
    if !bytes.len().is_multiple_of(2) {
        s.push('\u{FFFD}');
    }
    s
}

/// Decode UTF-16BE bytes (after BOM) into a String.
/// Uses `char::decode_utf16` for proper surrogate pair handling.
/// Invalid surrogate pairs are replaced with U+FFFD.
///
/// LIB-009: same trailing-byte handling as [`decode_utf16le`].
fn decode_utf16be(bytes: &[u8]) -> String {
    let u16_iter = bytes.chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
    let mut s: String = char::decode_utf16(u16_iter)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect();
    if !bytes.len().is_multiple_of(2) {
        s.push('\u{FFFD}');
    }
    s
}

// ─── File index types ────────────────────────────────────────────────

/// An entry in the file index — represents a single file or directory.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub modified: u64, // seconds since epoch
    pub is_dir: bool,
}

/// Format version for FileIndex. Bump when changing the on-disk struct layout.
/// Loading an index with a different version triggers a full rebuild via the
/// fast `read_format_version_from_index_file` header check.
pub const FILE_INDEX_VERSION: u32 = 2;

/// File index: a flat list of all files/directories under a root.
///
/// Used for fast file-name search without filesystem walk.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileIndex {
    pub root: String,
    /// Format version — used to detect stale indexes after schema changes.
    /// Placed after `root` so that `read_root_from_index_file()` can still
    /// read root as the first bincode field, and
    /// `read_format_version_from_index_file()` can read the version as the
    /// second field. Reordering these two fields breaks the header readers
    /// (see `test_file_index_field_order_guard` in `main_tests.rs`).
    #[serde(default)]
    pub format_version: u32,
    pub created_at: u64,
    pub max_age_secs: u64,
    pub entries: Vec<FileEntry>,
    /// Whether this index was built with `--respect-git-exclude` honoured.
    /// Persisted so that auto-rebuild paths (in `xray fast` / `xray grep`)
    /// preserve the user's original choice instead of silently flipping to
    /// the CLI default. See `docs/bug-reports/2026-04-23_417f315_cli-auto-rebuild-loses-flag.md`.
    #[serde(default)]
    pub respect_git_exclude: bool,
}

impl FileIndex {
    /// Check if the index is older than its configured max age.
    pub fn is_stale(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_secs();
        now.saturating_sub(self.created_at) > self.max_age_secs
    }
}

// ─── Content index types ─────────────────────────────────────────────

/// A posting: file_id + line numbers where the token appears.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Posting {
    pub file_id: u32,
    pub lines: Vec<u32>,
}

/// Trigram index for substring search.
/// Maps 3-character sequences to tokens containing them.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TrigramIndex {
    /// All unique tokens from the inverted index, sorted alphabetically.
    pub tokens: Vec<String>,
    /// Trigram → sorted vec of token indices (into `tokens` vec).
    pub trigram_map: HashMap<String, Vec<u32>>,
}

/// Generate trigrams (3-char sliding windows) from a token.
/// Uses char-based windows for Unicode correctness.
/// Returns empty vec for tokens shorter than 3 chars.
#[must_use]
pub fn generate_trigrams(token: &str) -> Vec<String> {
    // PERF-05: ASCII fast-path. The vast majority of tokens fed into the
    // trigram index are ASCII identifiers (code, log lines, English text).
    // The general path below pays for `chars().collect::<Vec<char>>()` (a
    // heap Vec of 4-byte `char` per code unit) plus a per-window
    // `iter().collect::<String>()` (another heap String per trigram). For
    // pure ASCII none of that is needed: each byte is one char so we can
    // slide directly over `as_bytes().windows(3)` and the 3-byte slice is
    // already a valid UTF-8 sequence (each byte < 0x80). Allocates exactly
    // `len-2` Strings of capacity 3 — same as the general path but skips
    // the intermediate `Vec<char>` and per-window iterator collection.
    // Non-ASCII tokens (Cyrillic, CJK, emoji) keep the original char-based
    // path verbatim — code-unit windows would split multi-byte sequences.
    if token.is_ascii() {
        if token.len() < 3 {
            return vec![];
        }
        return token
            .as_bytes()
            .windows(3)
            .map(|w| {
                // SAFETY: token.is_ascii() means every byte < 0x80, so any
                // 3-byte window is valid UTF-8 (3 single-byte codepoints).
                // We use the checked conversion + unwrap rather than
                // from_utf8_unchecked: the validation walk on 3 bytes is
                // negligible vs. the String heap allocation that follows,
                // and this keeps the function 100% safe code.
                std::str::from_utf8(w)
                    .expect("ASCII bytes are always valid UTF-8")
                    .to_string()
            })
            .collect();
    }
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 3 {
        return vec![];
    }
    chars.windows(3)
        .map(|w| w.iter().collect::<String>())
        .collect()
}

/// Inverted index: token → list of postings.
///
/// The core data structure for content search. Maps every token
/// to the files and line numbers where it appears.
/// Format version for ContentIndex. Bump when changing the struct layout.
/// Loading an index with a different version triggers a rebuild.
pub const CONTENT_INDEX_VERSION: u32 = 3;

#[derive(Serialize, Deserialize, Debug)]
pub struct ContentIndex {
    pub root: String,
    /// Format version — used to detect stale indexes after schema changes.
    /// Placed after `root` so that `read_root_from_index_file()` can still
    /// read root as the first bincode field.
    #[serde(default)]
    pub format_version: u32,
    pub created_at: u64,
    pub max_age_secs: u64,
    /// file_id → file path
    pub files: Vec<String>,
    /// token (lowercased) → postings
    pub index: HashMap<String, Vec<Posting>>,
    /// total tokens indexed
    pub total_tokens: u64,
    /// extensions that were indexed
    pub extensions: Vec<String>,
    /// file_id → total token count in that file (for TF-IDF)
    pub file_token_counts: Vec<u32>,
    /// Trigram index for substring search
    #[serde(default)]
    pub trigram: TrigramIndex,
    /// Whether the trigram index needs rebuilding before next substring search
    #[serde(default)]
    pub trigram_dirty: bool,
    /// Path → file_id lookup (populated with --watch)
    #[serde(default)]
    pub path_to_id: Option<HashMap<PathBuf, u32>>,
    /// Number of files that failed to read during indexing (IO errors)
    #[serde(default)]
    pub read_errors: usize,
    /// Number of files that required lossy UTF-8 conversion (contained non-UTF8 bytes)
    #[serde(default)]
    pub lossy_file_count: usize,
    /// Number of worker threads that panicked during index building.
    /// Zero for a clean build. Non-zero indicates partial/degraded index.
    #[serde(default)]
    pub worker_panics: usize,
    /// Whether this index was built with `--respect-git-exclude` honoured.
    /// Persisted so that auto-rebuild paths in `xray grep` preserve the
    /// user's original choice instead of silently flipping to the CLI
    /// default. See `docs/bug-reports/2026-04-23_417f315_cli-auto-rebuild-loses-flag.md`.
    #[serde(default)]
    pub respect_git_exclude: bool,
}

impl ContentIndex {
    /// Check if the index is older than its configured max age.
    pub fn is_stale(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_secs();
        now.saturating_sub(self.created_at) > self.max_age_secs
    }

    /// Number of live files currently indexed.
    ///
    /// `self.files.len()` is the file_id allocator capacity (append-only); on
    /// removal the slot is tombstoned (string cleared) but the Vec itself is
    /// never shrunk so file_id remains a stable index. Use this method for any
    /// user-visible count reporting (`xray_info`, `IndexMeta`, logs, memory
    /// estimates) so deletions are reflected.
    ///
    /// When `path_to_id` is populated (server with `--watch`) it is the
    /// authoritative live set. Otherwise (cold CLI build, no watcher → no
    /// removals possible) the raw `files.len()` is correct, with a defensive
    /// filter for empty tombstone slots inherited from older index files.
    pub fn live_file_count(&self) -> usize {
        if let Some(ref p2id) = self.path_to_id {
            p2id.len()
        } else {
            self.files.iter().filter(|s| !s.is_empty()).count()
        }
    }

    /// Pre-warm the trigram index by touching all data structures.
    ///
    /// After deserialization, the OS may not have paged in all the memory
    /// for the trigram index. The first real substring query would pay
    /// ~3 seconds of page-fault overhead. This method forces all pages
    /// into resident memory by iterating through every trigram posting
    /// list and every token string, eliminating the cold-start penalty.
    ///
    /// This method is idempotent — calling it multiple times is harmless
    /// (subsequent calls complete in microseconds since pages are already resident).
    ///
    /// Returns the number of trigrams and tokens touched (for logging).
    pub fn warm_up(&self) -> (usize, usize) {
        use std::hint::black_box;

        let trigram = &self.trigram;

        // Touch every posting list in the trigram map to fault in pages
        let mut trigram_count = 0usize;
        for posting_list in trigram.trigram_map.values() {
            // Read first and last element to touch the page range
            if let Some(first) = posting_list.first() {
                black_box(*first);
            }
            if let Some(last) = posting_list.last() {
                black_box(*last);
            }
            trigram_count += 1;
        }

        // Touch every token string to fault in the tokens vec pages
        let mut token_count = 0usize;
        for token in &trigram.tokens {
            // Read first byte of each token to force page-in
            if let Some(first_byte) = token.as_bytes().first() {
                black_box(*first_byte);
            }
            token_count += 1;
        }

        // Also touch the inverted index keys (HashMap bucket pages)
        for postings in self.index.values() {
            if let Some(first) = postings.first() {
                black_box(first.file_id);
            }
        }

        (trigram_count, token_count)
    }

    /// Shrink all internal HashMaps to fit their current size.
    /// Call after loading from disk to reclaim excess capacity.
    /// Saves ~20-50 MB for large indexes by eliminating HashMap over-allocation.
    pub fn shrink_maps(&mut self) {
        self.index.shrink_to_fit();
        for postings in self.index.values_mut() {
            postings.shrink_to_fit();
        }
        self.trigram.trigram_map.shrink_to_fit();
        for list in self.trigram.trigram_map.values_mut() {
            list.shrink_to_fit();
        }
        if let Some(ref mut p2id) = self.path_to_id {
            p2id.shrink_to_fit();
        }
    }
}

impl Default for ContentIndex {
    fn default() -> Self {
        ContentIndex {
            root: String::new(),
            format_version: 0,
            created_at: 0,
            max_age_secs: 3600,
            files: Vec::new(),
            index: HashMap::new(),
            total_tokens: 0,
            extensions: Vec::new(),
            file_token_counts: Vec::new(),
            trigram: TrigramIndex::default(),
            trigram_dirty: false,
            path_to_id: None,
            read_errors: 0,
            lossy_file_count: 0,
            worker_panics: 0,
            respect_git_exclude: false,
        }
    }
}

/// Tokenize a line of text into lowercase tokens.
///
/// Splits on non-alphanumeric characters (except `_`),
/// filters by minimum length, and lowercases all tokens.
///
/// # Examples
///
/// ```
/// use code_xray::tokenize;
///
/// let tokens = tokenize("private readonly HttpClient _client;", 2);
/// assert!(tokens.contains(&"private".to_string()));
/// assert!(tokens.contains(&"httpclient".to_string()));
/// assert!(tokens.contains(&"_client".to_string()));
/// ```
#[must_use]
pub fn tokenize(line: &str, min_len: usize) -> Vec<String> {
    line.split(|c: char| !c.is_alphanumeric() && c != '_')
        // MINOR-24: compare by Unicode scalar count, not byte length. `len()`
        // would under-count Greek / Cyrillic / CJK tokens (multi-byte UTF-8)
        // and let them slip below `min_len`, breaking the CJK/Cyrillic index.
        .filter(|s| s.chars().count() >= min_len)
        .map(fast_lowercase)
        .collect()
}

/// IDX-010: lowercase the token without an extra `to_lowercase()` allocation
/// when the input is already pure ASCII lowercase (the common case for source
/// code identifiers, JSON keys, and lowercase config tokens). Falls back to
/// the Unicode-aware `str::to_lowercase` for any input containing ASCII
/// uppercase or non-ASCII characters — preserving the Greek / Cyrillic / CJK
/// case-folding semantics established by MINOR-24.
#[inline]
fn fast_lowercase(s: &str) -> String {
    if s.bytes().all(|b| b.is_ascii() && !b.is_ascii_uppercase()) {
        s.to_string()
    } else {
        s.to_lowercase()
    }
}

/// Canonicalize a path and return it with `\\?\` prefix stripped + separators
/// normalised to `/`. Intended for tests that need to compare paths against
/// indexer/walker output, which always passes through `fs::canonicalize` +
/// [`clean_path`]. On CI runners with short user names (`runneradmin` ->
/// `RUNNER~1`), `tempfile::tempdir()` and `std::env::temp_dir()` return the
/// 8.3 short form; the walker returns the long form. Without this round-trip,
/// set comparisons spuriously diverge.
///
/// Exposed as `pub` (not `#[cfg(test)]`) so the binary crate's tests can
/// reach it via the lib crate, but it has no production callers.
#[doc(hidden)]
#[must_use]
pub fn canonicalize_test_root(p: &std::path::Path) -> std::path::PathBuf {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    std::path::PathBuf::from(clean_path(&canon.to_string_lossy()))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod lib_tests;

// ─── Property-based tests (proptest) ─────────────────────────────────

#[cfg(test)]
#[path = "lib_property_tests.rs"]
mod property_tests;
