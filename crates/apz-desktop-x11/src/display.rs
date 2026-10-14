//! Picking the display number of a private Xvfb: the first number from [`FIRST_DISPLAY`] whose lock file and
//! socket are both absent.

use std::path::PathBuf;

/// The first display number a private session may take. Lower numbers are left to the system and the owner.
pub const FIRST_DISPLAY: u32 = 90;

/// One past the last display number tried.
pub const END_DISPLAY: u32 = 1000;

/// Where X keeps its lock files and sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayPaths {
    pub lock_dir: PathBuf,
    pub socket_dir: PathBuf,
}

impl Default for DisplayPaths {
    fn default() -> Self {
        Self {
            lock_dir: PathBuf::from("/tmp"),
            socket_dir: PathBuf::from("/tmp/.X11-unix"),
        }
    }
}

impl DisplayPaths {
    /// The lock file X writes for display `n` (`/tmp/.X<n>-lock`).
    pub fn lock_file(&self, n: u32) -> PathBuf {
        self.lock_dir.join(format!(".X{n}-lock"))
    }

    /// The socket X listens on for display `n` (`/tmp/.X11-unix/X<n>`).
    pub fn socket(&self, n: u32) -> PathBuf {
        self.socket_dir.join(format!("X{n}"))
    }

    /// Whether an X server holds display `n` or is starting on it.
    pub fn is_taken(&self, n: u32) -> bool {
        self.lock_file(n).exists() || self.socket(n).exists()
    }
}

/// The first number in `first..end` that `taken` does not report.
pub fn pick_display(first: u32, end: u32, taken: impl Fn(u32) -> bool) -> Option<u32> {
    (first..end).find(|&n| !taken(n))
}

/// The name X clients use for display `n`.
pub fn display_name(n: u32) -> String {
    format!(":{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir, unique to this test run.
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apz-x11-display-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn the_first_free_number_from_the_start_is_picked() {
        assert_eq!(pick_display(90, 100, |_| false), Some(90));
        assert_eq!(pick_display(90, 100, |n| n == 90), Some(91));
        assert_eq!(pick_display(90, 93, |n| n < 92), Some(92));
        assert_eq!(pick_display(90, 92, |_| true), None, "no free number");
    }

    #[test]
    fn a_lock_file_or_a_socket_takes_a_number() {
        let dir = scratch("taken");
        let paths = DisplayPaths {
            lock_dir: dir.clone(),
            socket_dir: dir.clone(),
        };
        assert!(!paths.is_taken(90));
        std::fs::write(paths.lock_file(90), b"123\n").expect("a lock file");
        assert!(paths.is_taken(90), "a lock file takes its number");
        std::fs::write(paths.socket(91), b"").expect("a socket stand-in");
        assert!(paths.is_taken(91), "a socket takes its number");
        let picked = pick_display(FIRST_DISPLAY, END_DISPLAY, |n| paths.is_taken(n));
        assert_eq!(picked, Some(92));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_system_paths_are_the_tmp_lock_and_socket_places() {
        let paths = DisplayPaths::default();
        assert_eq!(paths.lock_file(95), PathBuf::from("/tmp/.X95-lock"));
        assert_eq!(paths.socket(95), PathBuf::from("/tmp/.X11-unix/X95"));
        assert_eq!(display_name(95), ":95");
    }
}
