//! The daemon's log: one file in the state directory, rotated when it grows past a limit.
//!
//! Lines carry a Unix time, a level and a message. Callers never log file contents, commands, tokens or
//! anything else from the hub: messages name what happened (an op kind, a station, an error), not the data.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::state::LOG_FILE;

/// The size at which the log is rotated: the current file moves to `<name>.1` (replacing the last one).
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// The longest message kept on one line.
const MAX_LINE: usize = 512;

/// The log file, safe to share across threads.
pub struct Log {
    path: PathBuf,
    max_bytes: u64,
    guard: Mutex<()>,
}

impl Log {
    /// The log in `dir`, rotated at [`MAX_BYTES`].
    pub fn open(dir: &Path) -> Log {
        Self::with_limit(dir, MAX_BYTES)
    }

    /// The log in `dir`, rotated at `max_bytes`.
    pub fn with_limit(dir: &Path, max_bytes: u64) -> Log {
        Log {
            path: dir.join(LOG_FILE),
            max_bytes,
            guard: Mutex::new(()),
        }
    }

    /// Where the log is written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.write("info", message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.write("warn", message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.write("error", message.as_ref());
    }

    fn write(&self, level: &str, message: &str) {
        let _lock = self.guard.lock().unwrap_or_else(|e| e.into_inner());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = format!("{stamp} {level} {}\n", sanitize(message));
        // A log that cannot be written must not stop the daemon, so failures are dropped.
        let _ = self.rotate_if_needed();
        if let Ok(mut file) = apz_os::private::append(&self.path) {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn rotate_if_needed(&self) -> std::io::Result<()> {
        let size = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if size < self.max_bytes {
            return Ok(());
        }
        let previous = self.path.with_extension("log.1");
        let _ = fs::remove_file(&previous);
        fs::rename(&self.path, previous)
    }
}

/// One line: control characters become spaces, and the message is cut to [`MAX_LINE`] characters.
fn sanitize(message: &str) -> String {
    message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_LINE)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-ws-log-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch dir");
        dir
    }

    #[test]
    fn lines_carry_a_level_and_one_line_each() {
        let dir = scratch("lines");
        let log = Log::open(&dir);
        log.info("connected as ws-7");
        log.warn("line one\nline two");
        let text = fs::read_to_string(log.path()).expect("the log exists");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" info connected as ws-7"));
        assert!(lines[1].ends_with(" warn line one line two"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_log_moves_to_the_previous_file() {
        let dir = scratch("rotate");
        let log = Log::with_limit(&dir, 64);
        for n in 0..10 {
            log.info(format!("entry {n} with some padding to fill the file"));
        }
        let previous = dir.join("apz-workstation.log.1");
        assert!(previous.exists(), "the old log was rotated");
        let current = fs::metadata(log.path()).expect("a current log").len();
        assert!(
            current < 200,
            "the current log started again: {current} bytes"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
