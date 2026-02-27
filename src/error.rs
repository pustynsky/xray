//! Unified error type for the search engine.

use thiserror::Error;

/// All errors that can occur in search operations.
#[derive(Error, Debug)]
pub enum SearchError {
    /// I/O error (file read/write, directory access)
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization/deserialization error (bincode)
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),

    /// Invalid regex pattern
    #[error("Invalid regex pattern '{pattern}': {source}")]
    InvalidRegex {
        pattern: String,
        #[source]
        source: regex::Error,
    },

    /// Directory does not exist
    #[error("Directory does not exist: {0}")]
    DirNotFound(String),

    /// No index found for the given directory
    #[error("No content index found for '{dir}'. Build one first:\n  search-index content-index -d {dir} -e cs")]
    IndexNotFound { dir: String },

    /// Index is stale and auto-reindex is disabled
    #[error("Index is stale (age: {age_secs}s, max: {max_secs}s)")]
    StaleIndex { age_secs: u64, max_secs: u64 },

    /// Lock poisoned (thread panicked while holding a lock)
    #[error("Lock poisoned: {0}")]
    LockPoisoned(String),

    /// Failed to save index to disk
    #[error("Failed to save index: {0}")]
    SaveFailed(String),

    /// Phrase has no indexable tokens
    #[error("Phrase '{phrase}' has no indexable tokens (min length 2)")]
    EmptyPhrase { phrase: String },

    /// Mutually exclusive flags or other argument validation error
    #[error("{0}")]
    InvalidArgs(String),

    /// Failed to load an index from disk
    #[error("Failed to load index from {path}: {message}")]
    IndexLoad {
        path: String,
        message: String,
    },
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
