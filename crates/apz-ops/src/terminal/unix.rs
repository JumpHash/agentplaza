//! The unix terminals: a PTY per terminal, a login shell on its slave, a reader thread on its master.
//!
//! Exit is read with `waitid(WNOWAIT)` on Linux, which leaves the shell a zombie: its code is known, its
//! pid is not reused, and [`retire`] reaps it. Elsewhere `waitid` with `WNOWAIT` is not available (Apple
//! has none), so the shell is reaped through its `Child` when it exits; the process group is then signalled
//! only while it still exists.
//!
//! This module is the crate's second syscall boundary (with `runs::unix`): the PTY calls are not in `std`.
//! Each `unsafe` block says what it relies on.

#![allow(unsafe_code)]

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apz_ide::path::check_dir;

use super::table::{
    Idle, MAX_TERMS, Ring, Table, decode_write, encode_read, locked, now_ms, remove_term, size,
    spawn_janitor, sweep, term_name, with_term,
};
use super::{
    TermClosed, TermInfo, TermList, TermOpened, TermRead, TermResized, TermWritten,
};
use crate::env::Env;
use crate::error::OpError;
use crate::root::{Root, locate};

const BASH: &str = "/bin/bash";
const SHELL: &str = "/bin/sh";
/// How long a write waits for the shell to read before it returns what went in.
const WRITE_WAIT: Duration = Duration::from_secs(2);
/// How long a closing shell has to exit on its own before its group is SIGKILLed.
const CLOSE_GRACE: Duration = Duration::from_secs(2);
/// How often the reader checks for a stop and for the shell's exit when the master is quiet.
const TICK: Duration = Duration::from_millis(50);
const TICK_MS: libc::c_int = 50;
const POLL: Duration = Duration::from_millis(20);

/// The terminals one table holds. A table starts a janitor thread that closes idle terminals, and
/// closes every terminal it holds when it is dropped.
pub struct Terminals {
    table: Arc<Mutex<Table<Term>>>,
}

/// One terminal: its master, its shell's pid, and the state the reader shares with it.
struct Term {
    /// The master. The reader holds a duplicate; the one here is dropped when the terminal closes.
    master: Arc<File>,
    /// The shell's pid, which is also its session's process group id.
    pid: i32,
    cols: u16,
    rows: u16,
    started_ms: u64,
    last_active: Instant,
    shared: Arc<Shared>,
}

/// What the reader thread fills and the calls read, and the shell itself.
struct Shared {
    ring: Mutex<Ring>,
    /// The shell's exit code, once it has exited.
    exit: Mutex<Option<i32>>,
    /// Set by [`retire`]: the reader stops and drops its duplicate of the master.
    stop: AtomicBool,
    /// The shell. Reaped by [`retire`], or by the exit check on platforms without `WNOWAIT`.
    child: Mutex<Child>,
}

impl Shared {
    fn exit_code(&self) -> Option<i32> {
        *locked(&self.exit)
    }
}

impl Idle for Term {
    fn last_active(&self) -> Instant {
        self.last_active
    }

    fn touch(&mut self) {
        self.last_active = Instant::now();
    }
}

impl Default for Terminals {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminals {
    /// A terminal table with no terminals. Its janitor starts now.
    pub fn new() -> Self {
        let table = Arc::new(Mutex::new(Table::default()));
        spawn_janitor(&table, retire);
        Self { table }
    }

    /// Opens a login shell in a new terminal, in `cwd` (a directory of the root; `""` is the root).
    ///
    /// `cols` and `rows` must be from 1 to 1000. The shell gets `env`, plus `TERM`. At most [`MAX_TERMS`]
    /// terminals are open.
    pub fn open(
        &self,
        root: &Root,
        env: &Env,
        cols: u64,
        rows: u64,
        cwd: &str,
    ) -> Result<TermOpened, OpError> {
        let cols = size(cols, "cols")?;
        let rows = size(rows, "rows")?;
        let target = locate(root, cwd, check_dir, check_dir)?;
        let metadata = fs::metadata(&target.abs)
            .map_err(|err| OpError::io(err, format!("cannot open a terminal in {cwd:?}")))?;
        if !metadata.is_dir() {
            return Err(OpError::invalid(format!("{cwd:?} is not a directory")));
        }

        sweep(&self.table, retire);
        let mut table = locked(&self.table);
        if table.terms.len() >= MAX_TERMS {
            return Err(OpError::limit(format!(
                "{MAX_TERMS} terminals are open; close one first"
            )));
        }
        let term = spawn(env, &target.abs, Pty::open(cols, rows)?, cols, rows)?;
        table.next += 1;
        let id = table.next;
        table.terms.insert(id, term);
        Ok(TermOpened {
            term: term_name(id),
        })
    }

    /// Writes base64 `data_b64` to the terminal, at most [`WRITE_MAX`] bytes of it.
    pub fn write(&self, term: &str, data_b64: &str) -> Result<TermWritten, OpError> {
        sweep(&self.table, retire);
        let data = decode_write(data_b64)?;
        let master = with_term(&self.table, term, |term| Arc::clone(&term.master))?;
        let written = write_master(&master, &data)?;
        Ok(TermWritten {
            ok: written == data.len(),
            written,
        })
    }

    /// The terminal's output from `offset` (the start when `None`), at most [`READ_MAX`] bytes.
    pub fn read(&self, term: &str, offset: Option<u64>) -> Result<TermRead, OpError> {
        sweep(&self.table, retire);
        let offset = offset.unwrap_or(0);
        let shared = with_term(&self.table, term, |term| Arc::clone(&term.shared))?;
        let slice = locked(&shared.ring).read(offset)?;
        let exit_code = shared.exit_code();
        Ok(TermRead {
            data_b64: encode_read(&slice.data),
            exit_code,
            exited: exit_code.is_some(),
            lost: slice.lost,
            next: slice.next,
            offset: slice.offset,
            term: term.to_string(),
        })
    }

    /// Sets the terminal's size. The kernel sends SIGWINCH to the terminal's foreground group.
    pub fn resize(&self, term: &str, cols: u64, rows: u64) -> Result<TermResized, OpError> {
        sweep(&self.table, retire);
        let cols = size(cols, "cols")?;
        let rows = size(rows, "rows")?;
        let master = with_term(&self.table, term, |term| {
            term.cols = cols;
            term.rows = rows;
            Arc::clone(&term.master)
        })?;
        let window = winsize(cols, rows);
        // SAFETY: TIOCSWINSZ reads one winsize from the pointer, which lives for the call. The master is
        // open (the Arc is held here).
        let rc = unsafe {
            libc::ioctl(
                master.as_raw_fd(),
                libc::TIOCSWINSZ as _,
                &window as *const libc::winsize,
            )
        };
        if rc != 0 {
            return Err(OpError::internal(format!(
                "cannot resize the terminal: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(TermResized {
            cols,
            rows,
            term: term.to_string(),
        })
    }

    /// Closes the terminal and ends its shell and its process group. Returns once the shell is gone.
    pub fn close(&self, term: &str) -> Result<TermClosed, OpError> {
        sweep(&self.table, retire);
        retire(remove_term(&self.table, term)?);
        Ok(TermClosed {
            closed: true,
            term: term.to_string(),
        })
    }

    /// The open terminals.
    pub fn list(&self) -> Result<TermList, OpError> {
        sweep(&self.table, retire);
        let table = locked(&self.table);
        let terminals = table
            .terms
            .iter()
            .map(|(id, term)| TermInfo {
                cols: term.cols,
                exited: term.shared.exit_code().is_some(),
                rows: term.rows,
                started_ms: term.started_ms,
                term: term_name(*id),
            })
            .collect();
        Ok(TermList { terminals })
    }
}

impl Drop for Terminals {
    fn drop(&mut self) {
        let terms: Vec<Term> = std::mem::take(&mut locked(&self.table).terms)
            .into_values()
            .collect();
        for term in terms {
            retire(term);
        }
    }
}

/// Ends a terminal. The master is dropped (the reader drops its own duplicate within [`TICK`]), which
/// hangs the session up. The shell gets [`CLOSE_GRACE`] to exit. Then the session's process group is
/// SIGKILLed, and the shell reaped.
fn retire(term: Term) {
    let Term {
        master,
        pid,
        shared,
        ..
    } = term;
    shared.stop.store(true, Ordering::SeqCst);
    drop(master);
    let deadline = Instant::now() + CLOSE_GRACE;
    while exit_of(pid, &shared).is_none() && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    // SAFETY: a plain kill. The shell is alive or a zombie, and either way holds `pid`, so `-pid` names
    // its own group and no other. Where the shell was already reaped, the group exists only while it has
    // members, and a pid is not reused while it names a live group.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let mut child = locked(&shared.child);
    let _ = child.kill();
    let _ = child.wait();
}

/// Starts the shell on `pty`'s slave in `dir`, with `env`, and the reader on its master.
fn spawn(env: &Env, dir: &Path, pty: Pty, cols: u16, rows: u16) -> Result<Term, OpError> {
    let (program, args): (&str, &[&str]) = if Path::new(BASH).exists() {
        (BASH, &["-l"])
    } else {
        (SHELL, &[])
    };
    let Pty { master, slave } = pty;
    let stdout = slave
        .try_clone()
        .map_err(|err| OpError::internal(format!("cannot dup the terminal: {err}")))?;
    let stderr = slave
        .try_clone()
        .map_err(|err| OpError::internal(format!("cannot dup the terminal: {err}")))?;
    let mut builder = Command::new(program);
    builder.args(args);
    env.apply(&mut builder);
    builder
        .env("TERM", "xterm-256color")
        .current_dir(dir)
        .stdin(Stdio::from(slave))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    // SAFETY: runs in the forked child between fork and exec. setsid and ioctl are async-signal-safe, and
    // the closure allocates nothing. Stdio is already on the slave, so fd 0 is the terminal.
    unsafe {
        builder.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = builder
        .spawn()
        .map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;
    // Dropping the builder closes the parent's copies of the slave, so the master sees end of file when
    // the shell's side is gone.
    drop(builder);
    let pid = child.id() as i32;
    let master = File::from(master);
    let reader = master
        .try_clone()
        .map_err(|err| OpError::internal(format!("cannot dup the master: {err}")))?;
    let shared = Arc::new(Shared {
        ring: Mutex::new(Ring::default()),
        exit: Mutex::new(None),
        stop: AtomicBool::new(false),
        child: Mutex::new(child),
    });
    let watched = Arc::clone(&shared);
    thread::spawn(move || pump(reader, watched, pid));
    Ok(Term {
        master: Arc::new(master),
        pid,
        cols,
        rows,
        started_ms: now_ms(),
        last_active: Instant::now(),
        shared,
    })
}

/// Reads the master into the ring until the terminal is stopped, and notes the shell's exit when it comes.
fn pump(mut master: File, shared: Arc<Shared>, pid: i32) {
    let mut chunk = [0u8; 8192];
    let mut hung_up = false;
    while !shared.stop.load(Ordering::SeqCst) {
        if hung_up {
            thread::sleep(TICK);
        } else if ready(master.as_raw_fd(), libc::POLLIN, TICK_MS) {
            match master.read(&mut chunk) {
                Ok(0) => hung_up = true,
                Ok(read) => locked(&shared.ring).append(&chunk[..read]),
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                // EIO: every descriptor of the slave is closed, so nothing more will come.
                Err(_) => hung_up = true,
            }
        }
        if let Some(code) = exit_of(pid, &shared) {
            *locked(&shared.exit) = Some(code);
        }
    }
}

/// Writes `data` to the master without waiting past [`WRITE_WAIT`]. The descriptor is non-blocking; a full
/// input queue waits for the shell to read. Returns how many bytes went in.
fn write_master(master: &File, data: &[u8]) -> Result<usize, OpError> {
    let deadline = Instant::now() + WRITE_WAIT;
    let mut writer = master;
    let mut written = 0;
    while written < data.len() {
        match writer.write(&data[written..]) {
            Ok(0) => break,
            Ok(sent) => written += sent,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let timeout = left.as_millis().min(i32::MAX as u128) as libc::c_int;
                if !ready(master.as_raw_fd(), libc::POLLOUT, timeout) {
                    break;
                }
            }
            // EIO: the shell's side is closed, so the shell has gone.
            Err(err) if err.raw_os_error() == Some(libc::EIO) => {
                return Err(OpError::Unavailable(
                    "the terminal's shell has exited".to_string(),
                ));
            }
            Err(err) => {
                return Err(OpError::internal(format!(
                    "cannot write to the terminal: {err}"
                )));
            }
        }
    }
    Ok(written)
}

/// A PTY pair, both descriptors close-on-exec, the master non-blocking.
struct Pty {
    master: OwnedFd,
    slave: OwnedFd,
}

impl Pty {
    fn open(cols: u16, rows: u16) -> Result<Pty, OpError> {
        let window = winsize(cols, rows);
        let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
        // SAFETY: on success openpty stores two new descriptors, which are owned from the next line. The
        // name and termios pointers may be null, which means the defaults; the winsize is read only.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                ptr::null_mut(),
                ptr::null_mut(),
                &window as *const libc::winsize as *mut libc::winsize,
            )
        };
        if rc != 0 {
            return Err(OpError::internal(format!(
                "cannot open a pseudo-terminal: {}",
                io::Error::last_os_error()
            )));
        }
        // SAFETY: openpty succeeded, so both are open descriptors that nothing else owns.
        let pty = unsafe {
            Pty {
                master: OwnedFd::from_raw_fd(master),
                slave: OwnedFd::from_raw_fd(slave),
            }
        };
        cloexec(pty.master.as_raw_fd())
            .map_err(|err| OpError::internal(format!("cannot set up the terminal: {err}")))?;
        cloexec(pty.slave.as_raw_fd())
            .map_err(|err| OpError::internal(format!("cannot set up the terminal: {err}")))?;
        nonblocking(pty.master.as_raw_fd())
            .map_err(|err| OpError::internal(format!("cannot set up the terminal: {err}")))?;
        Ok(pty)
    }
}

fn cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor this module owns, with no pointer argument.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor this module owns, with no pointer argument.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Whether `fd` has one of `events` ready within `timeout_ms`. A hang-up counts as ready, so the caller
/// finds out with its next read or write.
fn ready(fd: RawFd, events: libc::c_short, timeout_ms: libc::c_int) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: one pollfd, which poll reads and writes for the call.
    let rc = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
    rc > 0 && poll.revents != 0
}

/// The shell's exit code, if it has exited, read without reaping (`WNOWAIT`), so the pid stays reserved
/// for the session's group until [`retire`] reaps it. A signal's end is 128 plus the signal, which is what
/// a shell reports for one.
#[cfg(target_os = "linux")]
fn exit_of(pid: i32, _shared: &Shared) -> Option<i32> {
    // SAFETY: waitid fills the siginfo_t it is given. The struct is zeroed first, so a call that finds no
    // exit leaves si_pid zero.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let options = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
    let rc = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, options) };
    // SAFETY: the siginfo_t was filled by waitid above.
    if rc != 0 || unsafe { info.si_pid() } == 0 {
        return None;
    }
    // SAFETY: as above.
    let status = unsafe { info.si_status() };
    Some(if info.si_code == libc::CLD_EXITED {
        status
    } else {
        128 + status
    })
}

/// The exit code on platforms without `waitid` with `WNOWAIT`. The shell is reaped here, through its
/// `Child`, so its pid may be reused once its group is empty; [`retire`] signals the group only while it
/// still exists, and the `Child` never signals a pid it has reaped.
#[cfg(not(target_os = "linux"))]
fn exit_of(_pid: i32, shared: &Shared) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    match locked(&shared.child).try_wait() {
        Ok(Some(status)) => Some(
            status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
        ),
        _ => None,
    }
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::super::table::{IDLE, WRITE_MAX, take_idle};
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use std::os::unix::fs::symlink;

    const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

    fn scratch(name: &str) -> (Root, Env) {
        let dir =
            std::env::temp_dir().join(format!("apz-ops-term-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch root");
        (Root::new(&dir), Env::new(&dir, PATH))
    }

    fn open_at(terms: &Terminals, root: &Root, env: &Env, cwd: &str) -> String {
        terms
            .open(root, env, 80, 24, cwd)
            .expect("the terminal opens")
            .term
    }

    fn type_text(terms: &Terminals, term: &str, text: &str) {
        terms
            .write(term, &STANDARD.encode(text))
            .expect("the write goes in");
    }

    fn decoded(reply: &TermRead) -> Vec<u8> {
        STANDARD.decode(&reply.data_b64).expect("base64")
    }

    /// Reads `term` from its start, following `next`, until `done` returns a value for the output so far.
    fn read_with<T>(terms: &Terminals, term: &str, done: impl Fn(&str) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut offset = 0;
        let mut output = Vec::new();
        loop {
            let got = terms.read(term, Some(offset)).expect("the read");
            output.extend(decoded(&got));
            offset = got.next;
            let text = String::from_utf8_lossy(&output);
            if let Some(found) = done(&text) {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "not done within 10 s; output so far: {text:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Reads until `marker` is in the output.
    fn read_until(terms: &Terminals, term: &str, marker: &str) {
        read_with(terms, term, |text| text.contains(marker).then_some(()));
    }

    #[test]
    fn a_shell_runs_and_its_output_comes_back_by_offset() {
        let (root, env) = scratch("echo");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        // The typed line echoes as written, so the marker is the output: "hi2" never appears in "hi$((1+1))".
        let typed = terms
            .write(&term, &STANDARD.encode("echo hi$((1+1))\n"))
            .expect("the write goes in");
        assert!(typed.ok);
        read_until(&terms, &term, "hi2\r\n");
        let closed = terms.close(&term).expect("the close");
        assert_eq!(closed, TermClosed { closed: true, term });
    }

    #[test]
    fn a_shell_starts_in_the_directory_it_is_given() {
        let (root, env) = scratch("cwd");
        fs::create_dir_all(root.dir().join("sub")).expect("a subdirectory");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "sub");
        type_text(&terms, &term, "pwd\n");
        let expected = fs::canonicalize(root.dir().join("sub")).expect("canonical");
        read_until(&terms, &term, &format!("{}\r\n", expected.display()));
        terms.close(&term).expect("the close");
    }

    #[test]
    fn the_shell_sees_the_environment_it_is_given() {
        let (root, env) = scratch("env");
        let env = env.with_var("APZ_OPS_TEST", "seen");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        type_text(&terms, &term, "echo v=$APZ_OPS_TEST$((1+1))\n");
        read_until(&terms, &term, "v=seen2\r\n");
        terms.close(&term).expect("the close");
    }

    #[test]
    fn resize_changes_what_stty_reports() {
        let (root, env) = scratch("resize");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        let resized = terms.resize(&term, 100, 30).expect("the resize");
        assert_eq!((resized.cols, resized.rows), (100, 30));
        type_text(&terms, &term, "stty size\n");
        read_until(&terms, &term, "30 100\r\n");
        terms.close(&term).expect("the close");
    }

    #[test]
    fn resize_signals_the_foreground_group() {
        let (root, env) = scratch("winch");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        type_text(&terms, &term, "trap 'echo got-winch' WINCH; echo arm$((1+1))\n");
        read_until(&terms, &term, "arm2\r\n");
        terms.resize(&term, 100, 30).expect("the resize");
        // The shell is the foreground group, and its trap runs on SIGWINCH.
        read_until(&terms, &term, "got-winch\r\n");
        terms.close(&term).expect("the close");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn close_ends_the_shell_and_its_process() {
        let (root, env) = scratch("close");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        type_text(&terms, &term, "echo pid:$$\n");
        // The typed line has "pid:$$"; the output has "pid:<digits>\r\n", which only the output has.
        let pid: u32 = read_with(&terms, &term, |text| {
            text.split("pid:").skip(1).find_map(|rest| {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                (!digits.is_empty() && rest[digits.len()..].starts_with("\r\n"))
                    .then(|| digits.parse().expect("a pid"))
            })
        });
        assert!(Path::new(&format!("/proc/{pid}")).exists());
        terms.close(&term).expect("the close");
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "the shell outlived its close"
        );
        assert!(terms.list().expect("the list").terminals.is_empty());
    }

    #[test]
    fn a_shell_that_exits_is_reported_with_its_code() {
        let (root, env) = scratch("exit");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        type_text(&terms, &term, "exit 3\n");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let got = terms.read(&term, None).expect("the read");
            if got.exited {
                assert_eq!(got.exit_code, Some(3));
                break;
            }
            assert!(Instant::now() < deadline, "the shell did not exit");
            thread::sleep(Duration::from_millis(20));
        }
        let listed = terms.list().expect("the list");
        assert!(listed.terminals[0].exited);
        terms.close(&term).expect("the close");
    }

    #[test]
    fn ring_overflow_reports_the_bytes_it_dropped() {
        let (root, env) = scratch("ring");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        // More than the 1 MiB ring holds, then a marker at the end.
        type_text(
            &terms,
            &term,
            "head -c 1200000 /dev/zero | tr '\\0' a; echo END$((1+2))\n",
        );
        // Reads from 0 do not advance until the output has overflowed the ring; the first such read reports
        // the bytes it dropped, and the reads after it go on from where it stopped.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut offset = 0;
        let mut lost_seen = None;
        let mut output = Vec::new();
        loop {
            let got = terms.read(&term, Some(offset)).expect("the read");
            let lost = got.lost;
            if lost_seen.is_none() && lost > 0 {
                assert_eq!(got.offset, lost, "the read starts where the lost bytes end");
                lost_seen = Some(lost);
            }
            if lost_seen.is_some() {
                output.extend(decoded(&got));
                offset = got.next;
                if String::from_utf8_lossy(&output).contains("END3") {
                    break;
                }
            } else {
                offset = 0;
            }
            assert!(
                Instant::now() < deadline,
                "no overflow and END3 within 20 s"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert!(lost_seen.expect("the ring overflowed") > 0);
        terms.close(&term).expect("the close");
    }

    #[test]
    fn at_most_four_terminals_are_open() {
        let (root, env) = scratch("limit");
        let terms = Terminals::new();
        let names: Vec<String> = (0..MAX_TERMS)
            .map(|_| open_at(&terms, &root, &env, ""))
            .collect();
        let refused = terms.open(&root, &env, 80, 24, "");
        assert!(
            matches!(refused, Err(OpError::LimitExceeded(_))),
            "{refused:?}"
        );
        terms.close(&names[0]).expect("the close");
        let again = open_at(&terms, &root, &env, "");
        for name in names[1..].iter().chain(std::iter::once(&again)) {
            terms.close(name).expect("the close");
        }
    }

    #[test]
    fn a_cwd_outside_the_root_is_refused() {
        let (root, env) = scratch("escape");
        symlink("/", root.dir().join("out")).expect("a link out");
        let terms = Terminals::new();
        for cwd in ["..", "../..", "/etc", "out"] {
            let refused = terms.open(&root, &env, 80, 24, cwd);
            assert!(
                matches!(refused, Err(OpError::Denied(_))),
                "{cwd}: {refused:?}"
            );
        }
        assert!(terms.list().expect("the list").terminals.is_empty());
    }

    #[test]
    fn a_terminal_idle_past_the_limit_is_taken_and_the_fresh_one_is_kept() {
        let (root, env) = scratch("idle");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        assert!(
            take_idle(&terms.table, Instant::now()).is_empty(),
            "a fresh terminal is not idle"
        );
        let later = Instant::now() + IDLE + Duration::from_secs(1);
        let idle = take_idle(&terms.table, later);
        assert_eq!(idle.len(), 1);
        assert!(locked(&terms.table).terms.is_empty());
        for term in idle {
            retire(term);
        }
        let _ = term;
    }

    #[test]
    fn bad_arguments_are_refused_before_anything_starts() {
        let (root, env) = scratch("args");
        let terms = Terminals::new();
        let zero = terms.open(&root, &env, 0, 24, "");
        assert!(matches!(zero, Err(OpError::InvalidInput(_))), "{zero:?}");
        let huge = terms.open(&root, &env, 80, 1001, "");
        assert!(matches!(huge, Err(OpError::InvalidInput(_))), "{huge:?}");
        let name = open_at(&terms, &root, &env, "");
        let not_a_name = terms.write("shell-1", "");
        assert!(
            matches!(not_a_name, Err(OpError::InvalidInput(_))),
            "{not_a_name:?}"
        );
        let unknown = terms.read("term-99", None);
        assert!(matches!(unknown, Err(OpError::NotFound(_))), "{unknown:?}");
        let big = STANDARD.encode(vec![b'a'; WRITE_MAX + 1]);
        let too_big = terms.write(&name, &big);
        assert!(
            matches!(too_big, Err(OpError::LimitExceeded(_))),
            "{too_big:?}"
        );
        let not_base64 = terms.write(&name, "@@");
        assert!(
            matches!(not_base64, Err(OpError::InvalidInput(_))),
            "{not_base64:?}"
        );
        terms.close(&name).expect("the close");
    }
}
