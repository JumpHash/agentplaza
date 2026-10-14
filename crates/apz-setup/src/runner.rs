//! The host's commands and files, behind [`Runner`] so the setup can run against a mock in tests.

use std::io::{self, BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// What a command printed, and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    /// The exit code, when the command had one.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    /// A successful run that printed `stdout`.
    pub fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            code: Some(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    /// A failed run that printed `stderr`.
    pub fn failed(stderr: &str) -> Output {
        Output {
            success: false,
            code: Some(1),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }
}

/// The host: runs commands, finds programs on PATH, reads files.
pub trait Runner {
    /// Runs `program args` with stdin closed and waits for it. `Err` only when it cannot start.
    fn output(&self, program: &str, args: &[&str]) -> io::Result<Output>;

    /// Like [`Runner::output`], but each line of its stdout and stderr goes to `log` as it arrives.
    fn stream(
        &self,
        program: &str,
        args: &[&str],
        log: &mut dyn FnMut(String),
    ) -> io::Result<Output>;

    /// Whether `program` is on PATH (nothing is run).
    fn on_path(&self, program: &str) -> bool;

    /// The text of the file at `path`; `None` when it cannot be read.
    fn read_file(&self, path: &str) -> Option<String>;
}

/// The real host.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn output(&self, program: &str, args: &[&str]) -> io::Result<Output> {
        let out = command(program).args(args).stdin(Stdio::null()).output()?;
        Ok(Output {
            success: out.status.success(),
            code: out.status.code(),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }

    fn stream(
        &self,
        program: &str,
        args: &[&str],
        log: &mut dyn FnMut(String),
    ) -> io::Result<Output> {
        let mut child = command(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Both pipes are drained on their own threads (so neither can fill and block the child); the
        // lines reach `log` in the order they arrive.
        let (tx, rx) = mpsc::channel();
        let out = pump(child.stdout.take(), tx.clone());
        let err = pump(child.stderr.take(), tx);
        for text in rx {
            log(text);
        }
        let stdout = out.join().unwrap_or_default();
        let stderr = err.join().unwrap_or_default();
        let status = child.wait()?;
        Ok(Output {
            success: status.success(),
            code: status.code(),
            stdout: stdout.join("\n").into_bytes(),
            stderr: stderr.join("\n").into_bytes(),
        })
    }

    fn on_path(&self, program: &str) -> bool {
        let Some(paths) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&paths)
            .any(|dir| dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file())
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

/// A command for a probe or an installer step: on Windows it opens no console window (the app
/// has none, so each console program would flash its own).
fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let cmd = Command::new(program);
    #[cfg(windows)]
    let cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    };
    cmd
}

/// Reads `reader` line by line, sending each line to `tx` as it comes; the thread returns all
/// the lines when the stream ends.
fn pump<R: Read + Send + 'static>(
    reader: Option<R>,
    tx: mpsc::Sender<String>,
) -> thread::JoinHandle<Vec<String>> {
    thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(reader) = reader {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                let _ = tx.send(line.clone());
                lines.push(line);
            }
        }
        lines
    })
}

/// A mock host for the tests: it answers each command from `respond` and records it as
/// `program arg arg`.
#[cfg(test)]
pub(crate) mod mock {
    use super::{Output, Runner};
    use std::cell::RefCell;
    use std::io;
    use std::path::PathBuf;

    pub(crate) struct Mock {
        respond: Box<dyn Fn(&str, &[&str]) -> Output>,
        /// Programs "on PATH".
        pub(crate) paths: Vec<&'static str>,
        /// Files that can be read, by path.
        pub(crate) files: Vec<(&'static str, String)>,
        calls: RefCell<Vec<String>>,
    }

    impl Mock {
        pub(crate) fn new(respond: impl Fn(&str, &[&str]) -> Output + 'static) -> Mock {
            Mock {
                respond: Box::new(respond),
                paths: Vec::new(),
                files: Vec::new(),
                calls: RefCell::new(Vec::new()),
            }
        }

        /// Every command run so far, as `program arg arg`.
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }

        fn record(&self, program: &str, args: &[&str]) -> Output {
            let line = std::iter::once(program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.calls.borrow_mut().push(line);
            (self.respond)(program, args)
        }
    }

    impl Runner for Mock {
        fn output(&self, program: &str, args: &[&str]) -> io::Result<Output> {
            Ok(self.record(program, args))
        }

        fn stream(
            &self,
            program: &str,
            args: &[&str],
            log: &mut dyn FnMut(String),
        ) -> io::Result<Output> {
            let out = self.record(program, args);
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            for line in text.lines() {
                log(line.to_string());
            }
            Ok(out)
        }

        fn on_path(&self, program: &str) -> bool {
            self.paths.contains(&program)
        }

        fn read_file(&self, path: &str) -> Option<String> {
            self.files
                .iter()
                .find(|(name, _)| *name == path)
                .map(|(_, text)| text.clone())
        }
    }

    /// An empty directory for a test's files: `apz-setup-<tag>` under the system's temp dir,
    /// emptied first (each test has its own tag).
    pub(crate) fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-setup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
