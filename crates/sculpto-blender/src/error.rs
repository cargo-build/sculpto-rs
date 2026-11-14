//! Classified host-adapter failures.
//!
//! The caller decides from [`BlenderErrorKind`] and `code`, never from the
//! message. Every code is a stable identifier; adding one is a contract change.

use std::fmt;

/// The class of a host-adapter failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlenderErrorKind {
    /// The request is not valid JSON for this schema.
    InvalidRequest,
    /// The request is well formed but violates a parameter or path rule.
    InvalidParams,
    /// A required executable, input or artifact does not exist.
    NotFound,
    /// The host refused the job for a stated reason.
    Refused,
    /// The host process crashed or could not start.
    Environment,
    /// The host process exceeded the request timeout.
    Timeout,
    /// Reading or writing a local artifact failed.
    Io,
}

impl BlenderErrorKind {
    /// Stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidParams => "invalid_params",
            Self::NotFound => "not_found",
            Self::Refused => "refused",
            Self::Environment => "environment",
            Self::Timeout => "timeout",
            Self::Io => "io",
        }
    }
}

/// One classified failure from the Blender boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlenderError {
    kind: BlenderErrorKind,
    code: &'static str,
    message: String,
}

impl BlenderError {
    /// Build a classified error. `code` is a stable identifier, not a sentence.
    pub fn new(kind: BlenderErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }

    /// The caller's decision input.
    pub fn kind(&self) -> BlenderErrorKind {
        self.kind
    }

    /// The stable source code.
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// Human-readable detail.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for BlenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BlenderError {}

/// Shorthand for the crate's result type.
pub type Result<T, E = BlenderError> = std::result::Result<T, E>;
