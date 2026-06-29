//! Typed error definitions for frext.

use std::path::PathBuf;

/// Errors that can occur while persisting or restoring editor session state.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// The platform-specific project directories could not be resolved.
    #[error("could not determine a state directory for frext")]
    NoStateDir,

    /// An I/O error occurred while reading or writing a file.
    #[error("i/o error for {path}: {source}")]
    Io {
        /// The path that was being operated on.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// (De)serialization of the session index failed.
    #[error("failed to (de)serialize session state: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Errors that can occur while compiling a search query into a matcher.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// The user-supplied regular expression failed to compile.
    #[error("invalid regular expression: {0}")]
    InvalidRegex(String),
}
