//! The error every operation returns. Its variants are the taxonomy callers report, so a caller maps
//! each one to its own error type without changing the reason text.

use std::fmt;
use std::io;

/// Why an operation did not produce its reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    /// The arguments are not what the operation accepts: the reason says what.
    InvalidInput(String),
    /// The operation was refused: a path that leaves the root, or a filesystem permission.
    Denied(String),
    /// What was asked for is not there.
    NotFound(String),
    /// A cap was exceeded, by the request or by the reply.
    LimitExceeded(String),
    /// The file changed since the caller read it: its current hash, or `missing`.
    Conflict(String),
    /// The operation needs something this platform or host does not have.
    Unavailable(String),
    /// Something failed on this side.
    Internal(String),
}

impl OpError {
    pub(crate) fn invalid(reason: impl Into<String>) -> Self {
        Self::InvalidInput(reason.into())
    }

    pub(crate) fn denied(reason: impl Into<String>) -> Self {
        Self::Denied(reason.into())
    }

    pub(crate) fn not_found(reason: impl Into<String>) -> Self {
        Self::NotFound(reason.into())
    }

    pub(crate) fn limit(reason: impl Into<String>) -> Self {
        Self::LimitExceeded(reason.into())
    }

    pub(crate) fn internal(reason: impl Into<String>) -> Self {
        Self::Internal(reason.into())
    }

    /// An I/O failure, with `what` naming the operation and its path. A missing file is not found,
    /// a refused one is denied, anything else is internal.
    pub(crate) fn io(err: io::Error, what: impl fmt::Display) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Self::not_found(format!("{what}: {err}")),
            io::ErrorKind::PermissionDenied => Self::denied(format!("{what}: {err}")),
            _ => Self::internal(format!("{what}: {err}")),
        }
    }

    /// The refusal for an operation this platform has no backend for.
    #[cfg(not(unix))]
    pub(crate) fn unsupported(what: &str) -> Self {
        Self::Unavailable(format!("{what}: unsupported on this platform"))
    }
}

impl fmt::Display for OpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(reason) => write!(formatter, "invalid input: {reason}"),
            Self::Denied(reason) => write!(formatter, "denied: {reason}"),
            Self::NotFound(reason) => write!(formatter, "not found: {reason}"),
            Self::LimitExceeded(reason) => write!(formatter, "limit exceeded: {reason}"),
            Self::Conflict(reason) => write!(formatter, "conflict: {reason}"),
            Self::Unavailable(reason) => write!(formatter, "unavailable: {reason}"),
            Self::Internal(reason) => write!(formatter, "internal: {reason}"),
        }
    }
}

impl std::error::Error for OpError {}
