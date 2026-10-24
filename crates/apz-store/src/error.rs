//! What can go wrong in the store.

use std::fmt;
use std::path::PathBuf;

/// A store error. Every variant says what failed in words an owner can act on.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite refused a statement or a connection.
    Sqlite(rusqlite::Error),
    /// A file or folder could not be read or written.
    Io(std::io::Error),
    /// Another hub holds the folder's lock.
    Locked { dir: PathBuf, holder: String },
    /// The file was written by a newer hub: its schema is past what this hub understands.
    Newer {
        file: PathBuf,
        found: u32,
        known: u32,
    },
    /// A migration that was applied to the file has different text now: the embedded migrations were edited.
    Changed { file: PathBuf, version: u32 },
    /// The writer has stopped (the store is closing or its thread died).
    Closed,
    /// A value that does not fit its column, a row that does not decode, or a schema that is not well formed.
    Invalid(String),
}

impl StoreError {
    /// An [`StoreError::Invalid`] from anything printable.
    pub fn invalid(what: impl fmt::Display) -> Self {
        StoreError::Invalid(what.to_string())
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "sqlite: {e}"),
            StoreError::Io(e) => write!(f, "{e}"),
            StoreError::Locked { dir, holder } => write!(
                f,
                "another process is using {} ({holder}); stop it first, or give this one another folder",
                dir.display()
            ),
            StoreError::Newer { file, found, known } => write!(
                f,
                "{} has schema version {found}, newer than this hub understands ({known}): run the hub that wrote it",
                file.display()
            ),
            StoreError::Changed { file, version } => write!(
                f,
                "{}: migration {version} was changed after it was applied; migrations are append-only",
                file.display()
            ),
            StoreError::Closed => write!(f, "the store is closed"),
            StoreError::Invalid(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Sqlite(e) => Some(e),
            StoreError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}
