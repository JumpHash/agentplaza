//! The directory a caller is confined to, and the path rules that keep operations inside it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use apz_ide::path::PathError;

use crate::error::OpError;

/// The confined directory. Every operation names its paths relative to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    dir: PathBuf,
}

impl Root {
    /// The root at `dir`, as given. It is canonicalized per operation, so it may be a symlink.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory as given.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The canonical root. A root that is not there is unavailable.
    pub fn canonical(&self) -> Result<PathBuf, OpError> {
        fs::canonicalize(&self.dir).map_err(|err| {
            OpError::Unavailable(format!(
                "the workspace {} is not mounted: {err}",
                self.dir.display()
            ))
        })
    }
}

/// A path an operation may touch: its cleaned name, and where it is.
pub(crate) struct Target {
    pub(crate) rel: String,
    pub(crate) abs: PathBuf,
}

/// Checks `raw` with `lexical`, then the real filesystem: the deepest existing ancestor is
/// canonicalized and must land inside the canonical root, and the place it lands is checked with
/// `landed`.
pub(crate) fn locate(
    root: &Root,
    raw: &str,
    lexical: fn(&str) -> Result<String, PathError>,
    landed: fn(&str) -> Result<String, PathError>,
) -> Result<Target, OpError> {
    let rel = lexical(raw).map_err(|err| path_refusal(raw, err))?;
    let canonical = root.canonical()?;
    let mut ancestor = canonical.join(&rel);
    let resolved = loop {
        match fs::canonicalize(&ancestor) {
            Ok(resolved) => break resolved,
            Err(err) if err.kind() == io::ErrorKind::NotFound => match ancestor.parent() {
                Some(parent) => ancestor = parent.to_path_buf(),
                None => return Err(OpError::denied(format!("{raw:?} cannot be resolved"))),
            },
            Err(err) => return Err(OpError::io(err, format!("cannot resolve {raw:?}"))),
        }
    };
    let Ok(inside) = resolved.strip_prefix(&canonical) else {
        return Err(OpError::denied(format!(
            "{raw:?} resolves to {} outside the workspace",
            resolved.display()
        )));
    };
    let inside = inside.to_string_lossy();
    if !inside.is_empty() {
        landed(&inside).map_err(|err| path_refusal(raw, err))?;
    }
    Ok(Target {
        abs: canonical.join(&rel),
        rel,
    })
}

/// The refusal for a path the rules name. Malformed input is invalid; the rest are denied.
fn path_refusal(raw: &str, err: PathError) -> OpError {
    match err {
        PathError::Empty | PathError::Nul | PathError::Control => {
            OpError::invalid(format!("{raw:?}: {err}"))
        }
        _ => OpError::denied(format!("{raw:?} refused: {err}")),
    }
}

/// How a path is shown in a reply: `.` for the root itself.
pub(crate) fn shown(rel: &str) -> &str {
    if rel.is_empty() { "." } else { rel }
}
