//! # search-index — High-Performance Code Search Engine
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
pub fn read_file_lossy(path: &std::path::Path) -> std::io::Result<(String, bool)> {
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
fn decode_utf16le(bytes: &[u8]) -> String {
    let u16_iter = bytes.chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    char::decode_utf16(u16_iter)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

/// Decode UTF-16BE bytes (after BOM) into a String.
/// Uses `char::decode_utf16` for proper surrogate pair handling.
/// Invalid surrogate pairs are replaced with U+FFFD.
fn decode_utf16be(bytes: &[u8]) -> String {
    let u16_iter = bytes.chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
    char::decode_utf16(u16_iter)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

// ─── File index types ────────────────────────────────────────────────

/// An entry in the file index — represents a single file or directory.
#[derive(Serialize, Deserialize, Debug)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub modified: u64, // seconds since epoch
    pub is_dir: bool,
}

/// File index: a flat list of all files/directories under a root.
///
/// Used for fast file-name search without filesystem walk.
#[derive(Serialize, Deserialize, Debug)]
pub struct FileIndex {
    pub root: String,
    pub created_at: u64,
    pub max_age_secs: u64,
    pub entries: Vec<FileEntry>,
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
pub const CONTENT_INDEX_VERSION: u32 = 1;

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
/// use search_index::tokenize;
///
/// let tokens = tokenize("private readonly HttpClient _client;", 2);
/// assert!(tokens.contains(&"private".to_string()));
/// assert!(tokens.contains(&"httpclient".to_string()));
/// assert!(tokens.contains(&"_client".to_string()));
/// ```
#[must_use]
pub fn tokenize(line: &str, min_len: usize) -> Vec<String> {
    line.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| s.len() >= min_len)
        .map(|s| s.to_lowercase())
        .collect()
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod lib_tests;

// ─── Property-based tests (proptest) ─────────────────────────────────

#[cfg(test)]
#[path = "lib_property_tests.rs"]
mod property_tests;
