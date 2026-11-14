//! Classified storage failures.

use std::fmt;

/// Stable failure classification for store operations.
#[derive(Debug)]
pub enum StoreError {
    /// The database file could not be opened or created.
    Open(redb::DatabaseError),
    /// A transaction could not be started.
    Begin(redb::TransactionError),
    /// A table could not be opened.
    Table(redb::TableError),
    /// A read or write inside a transaction failed.
    Storage(redb::StorageError),
    /// The write transaction could not be committed.
    Commit(redb::CommitError),
    /// A record could not be serialized or decoded.
    Serde(serde_json::Error),
    /// A file operation failed.
    Io(std::io::Error),
    /// A stored value is not valid UTF-8.
    Utf8(std::str::Utf8Error),
    /// A kind or record identity is empty or contains the reserved separator.
    InvalidKey {
        /// The offending field.
        field: &'static str,
        /// The rejected value.
        value: String,
    },
    /// A write was attempted through a read-only handle.
    ReadOnly,
    /// The stored schema version differs from the caller's.
    VersionMismatch {
        /// Family name.
        kind: &'static str,
        /// Version found on disk.
        found: u32,
        /// Version the caller expects.
        expected: u32,
    },
    /// The consumer rejected the record before storage.
    Validation {
        /// Family name.
        kind: &'static str,
        /// Reviewer-facing detail.
        detail: String,
    },
}

/// Result alias for store operations.
pub type StoreResult<T> = Result<T, StoreError>;

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(formatter, "store open failed: {error}"),
            Self::Begin(error) => write!(formatter, "transaction begin failed: {error}"),
            Self::Table(error) => write!(formatter, "table operation failed: {error}"),
            Self::Storage(error) => write!(formatter, "storage operation failed: {error}"),
            Self::Commit(error) => write!(formatter, "commit failed: {error}"),
            Self::Serde(error) => write!(formatter, "record encoding failed: {error}"),
            Self::Io(error) => write!(formatter, "file operation failed: {error}"),
            Self::Utf8(error) => write!(formatter, "stored value is not UTF-8: {error}"),
            Self::InvalidKey { field, value } => {
                write!(
                    formatter,
                    "invalid {field} {value:?}: empty or reserved separator"
                )
            }
            Self::ReadOnly => write!(formatter, "store is read-only"),
            Self::VersionMismatch {
                kind,
                found,
                expected,
            } => write!(
                formatter,
                "record kind {kind} has version {found}, expected {expected}"
            ),
            Self::Validation { kind, detail } => {
                write!(formatter, "record kind {kind} rejected: {detail}")
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open(error) => Some(error),
            Self::Begin(error) => Some(error),
            Self::Table(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Commit(error) => Some(error),
            Self::Serde(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Utf8(error) => Some(error),
            Self::ReadOnly
            | Self::InvalidKey { .. }
            | Self::VersionMismatch { .. }
            | Self::Validation { .. } => None,
        }
    }
}

impl From<redb::DatabaseError> for StoreError {
    fn from(error: redb::DatabaseError) -> Self {
        Self::Open(error)
    }
}

impl From<redb::TransactionError> for StoreError {
    fn from(error: redb::TransactionError) -> Self {
        Self::Begin(error)
    }
}

impl From<redb::TableError> for StoreError {
    fn from(error: redb::TableError) -> Self {
        Self::Table(error)
    }
}

impl From<redb::StorageError> for StoreError {
    fn from(error: redb::StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<redb::CommitError> for StoreError {
    fn from(error: redb::CommitError) -> Self {
        Self::Commit(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serde(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<std::str::Utf8Error> for StoreError {
    fn from(error: std::str::Utf8Error) -> Self {
        Self::Utf8(error)
    }
}
