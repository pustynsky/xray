//! High-performance code search engine with inverted indexing and AST-based code intelligence.
//!
//! Binary crate entry point. All CLI logic is in the `cli` module.

// Use mimalloc as global allocator — aggressively returns freed pages to the OS,
// reducing memory fragmentation by ~70-80% compared to Windows HeapAlloc.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Re-export core types from library crate
pub use code_xray::{canonicalize_or_warn, clean_path, current_unix_secs, is_path_within, path_eq, read_file_lossy, tokenize, ContentIndex, FileEntry, FileIndex, Posting, TrigramIndex, DEFAULT_MIN_TOKEN_LEN, FILE_INDEX_VERSION};

mod cli;
mod definitions;
mod error;
mod git;
mod index;
mod mcp;
mod tips;

pub use error::SearchError;
pub use index::{
    build_content_index, build_index, cleanup_indexes_for_dir, cleanup_orphaned_indexes,
    cleanup_stale_tmp_files,
    content_index_path_for, find_content_index_for_dir, index_dir, index_path_for,
    load_content_index, load_index, save_content_index, save_index,
};

// Re-export CLI types used by other modules
pub use cli::args::{IndexArgs, ContentIndexArgs, ServeArgs};

fn main() {
    cli::run();
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
