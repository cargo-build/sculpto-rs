//! Structured parse errors.
//!
//! Every failure names the file, the offending element and a stable kind so a
//! host adapter can surface the problem without re-parsing. No fallbacks: a
//! malformed required field fails the whole document.

use std::fmt;
use std::path::PathBuf;

/// Stable failure classification for untrusted input documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The file could not be read at all.
    Io,
    /// The file is not valid XML.
    Malformed,
    /// The root element is not a supported document.
    UnsupportedRoot,
    /// A supported document declares a version this crate cannot read.
    UnsupportedVersion,
    /// A required element or attribute is missing.
    MissingRequired,
    /// A value has the wrong shape (number, enum token, range).
    InvalidValue,
}

/// A parse failure with its source location and owning element.
#[derive(Debug, Clone)]
pub struct Error {
    kind: ErrorKind,
    /// File the error came from, when known.
    pub path: Option<PathBuf>,
    /// Element path such as `SliderSetInfo/SliderSet[2]/Slider[@name=Lean]`.
    pub element: String,
    /// Human-readable detail; never used for control flow.
    pub detail: String,
}

impl Error {
    /// Build an error for an element path.
    pub fn new(kind: ErrorKind, element: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            path: None,
            element: element.into(),
            detail: detail.into(),
        }
    }

    /// Classify the failure.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Attach the originating file path.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(f, "{}: ", path.display())?;
        }
        write!(
            f,
            "{}: {}: {}",
            self.element,
            self.kind_label(),
            self.detail
        )
    }
}

impl Error {
    fn kind_label(&self) -> &'static str {
        match self.kind {
            ErrorKind::Io => "I/O error",
            ErrorKind::Malformed => "malformed XML",
            ErrorKind::UnsupportedRoot => "unsupported root element",
            ErrorKind::UnsupportedVersion => "unsupported version",
            ErrorKind::MissingRequired => "missing required value",
            ErrorKind::InvalidValue => "invalid value",
        }
    }
}

impl std::error::Error for Error {}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;
