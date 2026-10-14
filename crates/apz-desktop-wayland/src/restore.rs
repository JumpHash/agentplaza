//! The RemoteDesktop restore token. The portal gives one when the owner approves a session, and the next session
//! presents it, so the owner approves once rather than every time. It is stored in the backend's state directory
//! with owner-only permissions, since it lets the session start without the dialog.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The file in the state directory that holds the token.
const FILE: &str = "wayland-restore-token";

/// The stored restore token of the Wayland backend.
pub struct RestoreToken {
    path: PathBuf,
}

impl RestoreToken {
    /// The token store in `state_dir`, the directory the daemon gives this backend. Nothing is read or created
    /// until the token is loaded or saved.
    pub fn in_dir(state_dir: &Path) -> Self {
        Self {
            path: state_dir.join(FILE),
        }
    }

    /// The stored token, or `None` when there is none or the file cannot be read.
    pub fn load(&self) -> Option<String> {
        let text = fs::read_to_string(&self.path).ok()?;
        let token = text.trim();
        (!token.is_empty()).then(|| token.to_owned())
    }

    /// Stores `token` in place of any earlier one. The file is written under a temporary name and renamed into
    /// place, so an interrupted save leaves the old token or the new one, never a mix.
    pub fn save(&self, token: &str) -> io::Result<()> {
        if token.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an empty restore token",
            ));
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temporary = self.path.with_extension("tmp");
        let _ = fs::remove_file(&temporary);
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(token.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A fresh directory under the system temporary directory, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir()
                .join(format!("apz-wayland-{name}-{}-{nanos}", std::process::id()));
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_token_loads_as_none() {
        let scratch = Scratch::new("missing");
        assert_eq!(RestoreToken::in_dir(&scratch.0).load(), None);
    }

    #[test]
    fn a_saved_token_loads_back_and_a_second_save_replaces_it() {
        let scratch = Scratch::new("round-trip");
        let store = RestoreToken::in_dir(&scratch.0);
        store.save("first-token").expect("saves");
        assert_eq!(store.load().as_deref(), Some("first-token"));
        store.save("second-token").expect("saves again");
        assert_eq!(store.load().as_deref(), Some("second-token"));
    }

    #[test]
    fn the_token_file_is_readable_by_its_owner_only() {
        let scratch = Scratch::new("mode");
        let store = RestoreToken::in_dir(&scratch.0);
        store.save("secret").expect("saves");
        let mode = fs::metadata(scratch.0.join(FILE))
            .expect("the file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_empty_or_blank_token_is_refused_and_loads_as_none() {
        let scratch = Scratch::new("empty");
        let store = RestoreToken::in_dir(&scratch.0);
        assert!(store.save("").is_err());
        assert!(store.save("   \n").is_err());
        fs::create_dir_all(&scratch.0).expect("makes the directory");
        fs::write(scratch.0.join(FILE), "  \n").expect("writes a blank file");
        assert_eq!(store.load(), None);
    }

    #[test]
    fn a_token_is_read_without_the_newline_an_editor_left() {
        let scratch = Scratch::new("trim");
        fs::create_dir_all(&scratch.0).expect("makes the directory");
        fs::write(scratch.0.join(FILE), "abc123\n").expect("writes");
        assert_eq!(
            RestoreToken::in_dir(&scratch.0).load().as_deref(),
            Some("abc123")
        );
    }
}
