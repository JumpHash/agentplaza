//! Running a scanner, audit or fuzzer that is on the `PATH`, under a time limit (PLAN 22.8). Nothing is installed: a tool
//! is looked up on the `PATH` and run where it is, and a tool that is not there is a skip with its reason.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// What a process did: its exit code (None when it was killed at its limit, or ended by a signal), and its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

/// The executable named `name` on the `PATH` list `path_var`, if any: a tool is found, never installed.
pub fn tool(path_var: &str, name: &str) -> Option<PathBuf> {
    crate::load::find_on(path_var, name)
}

/// Runs `program` with `args` in `dir`, killing it when `limit` has passed. Its output is kept as text (lossy UTF-8).
/// Where the process is killed, its output is not read: a grandchild may still hold the pipes open.
pub fn run(program: &Path, args: &[&str], dir: &Path, limit: Duration) -> Result<Ran, String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let started = Instant::now();
    let (code, timed_out) = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break (status.code(), false),
            None if started.elapsed() >= limit => {
                let _ = child.kill();
                let status = child.wait().map_err(|e| e.to_string())?;
                break (status.code(), true);
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let (stdout, stderr) = if timed_out {
        (String::new(), String::new())
    } else {
        (join(out), join(err))
    };
    Ok(Ran {
        code,
        timed_out,
        stdout,
        stderr,
    })
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Option<JoinHandle<String>> {
    pipe.map(|mut pipe| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    })
}

fn join(handle: Option<JoinHandle<String>>) -> String {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// The last `lines` lines of `text`, for evidence.
pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tests run `/bin/sh`, which Windows does not have.
    #[cfg(unix)]
    #[test]
    fn a_process_is_run_in_its_directory_and_its_output_kept() {
        let dir = tempfile::tempdir().unwrap();
        let ran = run(
            Path::new("/bin/sh"),
            &["-c", "pwd; echo out; echo err 1>&2; exit 3"],
            dir.path(),
            Duration::from_secs(20),
        )
        .unwrap();
        assert_eq!(ran.code, Some(3));
        assert!(!ran.timed_out);
        assert!(ran.stdout.contains("out"), "{ran:?}");
        assert!(ran.stderr.contains("err"), "{ran:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_process_past_its_limit_is_killed_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let ran = run(
            Path::new("/bin/sh"),
            &["-c", "sleep 30"],
            dir.path(),
            Duration::from_millis(300),
        )
        .unwrap();
        assert!(ran.timed_out, "{ran:?}");
        assert_eq!(ran.code, None);
    }

    #[test]
    fn a_missing_program_is_an_error_not_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let error = run(
            Path::new("/nonexistent/apz-tool"),
            &[],
            dir.path(),
            Duration::from_secs(1),
        );
        assert!(error.is_err());
    }
}
