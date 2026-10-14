//! The environment of the processes this crate starts.
//!
//! A started process gets exactly what an [`Env`] names: the environment is cleared first, so nothing
//! of the caller's leaks in. The caller chooses `HOME`, `PATH`, any extra variables (a proxy, say)
//! and, on unix, a uid and gid to drop to.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The environment for the processes this crate starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    home: PathBuf,
    path: String,
    vars: Vec<(String, String)>,
    uid: Option<u32>,
}

impl Env {
    /// An environment with `HOME` set to `home` and `PATH` to `path`, and nothing else.
    pub fn new(home: impl Into<PathBuf>, path: impl Into<String>) -> Self {
        Self {
            home: home.into(),
            path: path.into(),
            vars: Vec::new(),
            uid: None,
        }
    }

    /// Adds one variable.
    pub fn with_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.push((key.into(), value.into()));
        self
    }

    /// Drops the processes to this uid and gid before they run (unix). `None` keeps the caller's.
    pub fn with_uid(mut self, uid: Option<u32>) -> Self {
        self.uid = uid;
        self
    }

    /// The home directory the processes see.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The variables a started Windows process gets, each name once, sorted by name: `HOME` and `PATH`, then
    /// the rest, the last value given for a name winning. The Windows backends build their block from it.
    #[cfg(windows)]
    pub(crate) fn pairs(
        &self,
    ) -> std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> {
        let mut pairs = std::collections::BTreeMap::new();
        pairs.insert(
            std::ffi::OsString::from("HOME"),
            self.home.clone().into_os_string(),
        );
        pairs.insert(
            std::ffi::OsString::from("PATH"),
            std::ffi::OsString::from(self.path.as_str()),
        );
        for (key, value) in &self.vars {
            pairs.insert(
                std::ffi::OsString::from(key.as_str()),
                std::ffi::OsString::from(value.as_str()),
            );
        }
        pairs
    }

    /// Clears `builder`'s environment and sets this one on it. The working directory is the caller's
    /// to set.
    pub fn apply(&self, builder: &mut Command) {
        builder
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.path);
        for (key, value) in &self.vars {
            builder.env(key, value);
        }
        #[cfg(unix)]
        if let Some(uid) = self.uid {
            use std::os::unix::process::CommandExt;
            builder.uid(uid).gid(uid);
        }
    }
}
