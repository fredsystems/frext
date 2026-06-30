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

/// Errors that can occur while performing a file-tree filesystem operation
/// (rename, create, or move-to-trash) triggered from the sidebar.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// The proposed name was empty or contained a path separator, so it could
    /// not be used as a single file or directory name.
    #[error("invalid name: {0:?}")]
    InvalidName(String),

    /// The destination of a create or rename already exists, so the operation
    /// was refused rather than clobbering it.
    #[error("destination already exists: {0}")]
    AlreadyExists(PathBuf),

    /// A path that should have had a parent directory did not (e.g. a
    /// filesystem root), so a sibling could not be created or renamed.
    #[error("path has no parent directory: {0}")]
    NoParent(PathBuf),

    /// An I/O error occurred while creating or renaming.
    #[error("i/o error for {path}: {source}")]
    Io {
        /// The path that was being operated on.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// Moving the entry to the OS trash failed.
    #[error("could not move {path} to trash: {message}")]
    Trash {
        /// The path that could not be trashed.
        path: PathBuf,
        /// A human-readable description of the underlying failure.
        message: String,
    },
}
