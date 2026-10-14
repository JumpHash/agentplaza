//! The owner's terminals: a shell on a pseudo-terminal in this VM, served as operator tools.
//!
//! `ide_term_open` opens a PTY pair and starts a login shell (`/bin/bash -l`, else `/bin/sh`) on
//! its slave, in a new session, so the slave is the shell's controlling terminal. The shell gets the
//! environment every guest command gets ([`crate::bash::environment`]) plus `TERM`, and a directory
//! of the workspace as its working directory ([`crate::ide::locate`] confines it).
//!
//! A reader thread per terminal drains the master into a ring of the last [`RING_CAP`] bytes. Every
//! byte has an offset counted from the terminal's start, so a reader that polls from its last `next`
//! sees each byte once. An offset the ring has dropped moves on to the oldest byte kept, and the reply
//! says how many bytes were lost in `lost`.
//!
//! A terminal lives until it is closed or the harness exits. One with no read, write or resize for
//! [`IDLE`] is closed in the background. Closing hangs the master up (the shell gets SIGHUP), gives
//! the shell [`CLOSE_GRACE`] to exit, then SIGKILLs the session's process group. The group id is the
//! shell's pid, and the shell is not reaped until the close, so nothing else can hold that id by then.
//!
//! Exit is read with `waitid(WNOWAIT)`, which leaves the shell a zombie: its code is known, its pid is
//! not reused, and [`retire`] reaps it.
//!
//! This module is the crate's second syscall boundary (with [`crate::vsock`]): the PTY calls are not in
//! `std`. Each `unsafe` block says what it relies on.

#![allow(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_ide::path::check_dir;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::bash::{self, Privilege, SHELL};
use crate::files::{self, Workspace};
use crate::ide;
use crate::tools::ToolError;

const BASH: &str = "/bin/bash";
/// Terminals open at once, the exited ones included until they are closed.
const MAX_TERMS: usize = 4;
/// How long a terminal may go without a read, write or resize before it is closed.
const IDLE: Duration = Duration::from_secs(30 * 60);
/// How often the janitor looks for idle terminals when no call does.
const JANITOR_EVERY: Duration = Duration::from_secs(60);
/// The most output a terminal keeps; older bytes are dropped.
const RING_CAP: usize = 1 << 20;
/// The most a read returns, in raw bytes. Base64 makes it 4/3 of that, so 48 KiB would not fit the
/// operator's output cap with the reply around it; 47 KiB does.
const READ_MAX: usize = 47 * 1024;
/// The most a write takes, in raw bytes.
const WRITE_MAX: usize = 64 * 1024;
/// How long a write waits for the shell to read before it returns what went in. The operator's calls
/// are served one at a time and expire after five seconds queued, so this stays well under that.
const WRITE_WAIT: Duration = Duration::from_secs(2);
/// How long a closing shell has to exit on its own before its group is SIGKILLed.
const CLOSE_GRACE: Duration = Duration::from_secs(2);
/// How often the reader checks for a stop and for the shell's exit when the master is quiet.
const TICK: Duration = Duration::from_millis(50);
const TICK_MS: libc::c_int = 50;
const POLL: Duration = Duration::from_millis(20);
/// The largest number of columns or rows.
const MAX_SIZE: u64 = 1000;

/// `ide_term_open {cols, rows, cwd?}`: opens a shell in a new terminal.
///
/// Output: `{"term": "term-<n>"}`. `cwd` is a directory of the workspace, relative to its root, and
/// defaults to the root. At most [`MAX_TERMS`] terminals are open.
pub fn open(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    START_JANITOR.call_once(|| {
        thread::spawn(|| {
            loop {
                thread::sleep(JANITOR_EVERY);
                sweep(&TERMS);
            }
        });
    });
    open_in(&TERMS, workspace, args)
}

/// Starts, once, the thread that closes idle terminals when no call arrives to do it.
static START_JANITOR: Once = Once::new();

/// `ide_term_write {term, data_b64}`: writes bytes to the terminal, at most [`WRITE_MAX`] of them.
///
/// Output: `{"ok", "written"}`. `ok` is false when not everything went in before [`WRITE_WAIT`]; the
/// caller sends the rest from `written` on, in a later call.
pub fn write(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    write_in(&TERMS, args)
}

/// `ide_term_read {term, offset?}`: the terminal's output from `offset`, at most [`READ_MAX`] bytes.
///
/// Output: `{"term", "offset", "next", "data_b64", "exited", "exit_code", "lost"}`. `offset` is where
/// `data_b64` starts, which is later than asked by `lost` bytes when the ring dropped them. `exit_code`
/// is null while the shell runs, and 128 plus the signal when a signal ended it.
pub fn read(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    read_in(&TERMS, args)
}

/// `ide_term_resize {term, cols, rows}`: sets the terminal's size. The kernel sends SIGWINCH to the
/// terminal's foreground process group when the size changes.
pub fn resize(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    resize_in(&TERMS, args)
}

/// `ide_term_close {term}`: closes the terminal and ends its shell and its process group.
///
/// Output: `{"term", "closed": true}`. It returns after the shell is gone, which takes up to
/// [`CLOSE_GRACE`] when the shell ignores the hang-up.
pub fn close(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    close_in(&TERMS, args)
}

/// `ide_term_list {}`: the open terminals.
///
/// Output: `{"terminals": [{"term", "cols", "rows", "started_ms", "exited"}]}`, with `started_ms` the
/// Unix time the shell started.
pub fn list(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    list_in(&TERMS, args)
}

/// The terminals this harness holds. One process, one workspace: a static is enough.
static TERMS: Mutex<Table> = Mutex::new(Table {
    next: 0,
    terms: BTreeMap::new(),
});

#[derive(Default)]
struct Table {
    next: u64,
    terms: BTreeMap<u64, Term>,
}

/// One terminal: its master, its shell, and the state the reader shares with it.
struct Term {
    /// The master. The reader holds a duplicate; the one here is dropped when the terminal closes.
    master: Arc<File>,
    /// The shell. Reaped by [`retire`] only.
    child: Child,
    /// The shell's pid, which is also its session's process group id.
    pid: i32,
    cols: u16,
    rows: u16,
    started_ms: u64,
    last_active: Instant,
    shared: Arc<Shared>,
}

/// What the reader thread fills and the calls read.
#[derive(Default)]
struct Shared {
    ring: Mutex<Ring>,
    /// The shell's exit code, once it has exited.
    exit: Mutex<Option<i32>>,
    /// Set by [`retire`]: the reader stops and drops its duplicate of the master.
    stop: AtomicBool,
}

/// The last [`RING_CAP`] bytes of output, and how many bytes have ever been written.
#[derive(Default)]
struct Ring {
    bytes: VecDeque<u8>,
    total: u64,
}

impl Ring {
    fn append(&mut self, data: &[u8]) {
        self.total += data.len() as u64;
        self.bytes.extend(data);
        if self.bytes.len() > RING_CAP {
            let excess = self.bytes.len() - RING_CAP;
            self.bytes.drain(..excess);
        }
    }

    /// The offset of the oldest byte kept.
    fn oldest(&self) -> u64 {
        self.total - self.bytes.len() as u64
    }
}

fn open_in(
    registry: &Mutex<Table>,
    workspace: &Workspace,
    args: &Value,
) -> Result<String, ToolError> {
    let cols = size_arg(args, "cols")?;
    let rows = size_arg(args, "rows")?;
    let cwd = match args.get("cwd") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(cwd)) => cwd.clone(),
        Some(_) => return Err(files::invalid("\"cwd\" must be a string")),
    };
    let target = ide::locate(workspace, &cwd, check_dir, check_dir)?;
    let metadata = fs::metadata(&target.abs)
        .map_err(|err| files::io_tool_error(err, format!("cannot open a terminal in {cwd:?}")))?;
    if !metadata.is_dir() {
        return Err(files::invalid(format!("{cwd:?} is not a directory")));
    }

    sweep(registry);
    let mut table = locked(registry);
    if table.terms.len() >= MAX_TERMS {
        return Err(files::limit(format!(
            "{MAX_TERMS} terminals are open; close one first"
        )));
    }
    let term = spawn(workspace, &target.abs, Pty::open(cols, rows)?, cols, rows)?;
    table.next += 1;
    let id = table.next;
    table.terms.insert(id, term);
    Ok(json!({ "term": term_name(id) }).to_string())
}

fn write_in(registry: &Mutex<Table>, args: &Value) -> Result<String, ToolError> {
    sweep(registry);
    let encoded = files::string_arg(args, "data_b64")?;
    let data = STANDARD
        .decode(encoded)
        .map_err(|err| files::invalid(format!("\"data_b64\" is not base64: {err}")))?;
    if data.len() > WRITE_MAX {
        return Err(files::limit(format!(
            "{} bytes is over the {WRITE_MAX}-byte write cap",
            data.len()
        )));
    }
    let master = with_term(registry, args, |term| Arc::clone(&term.master))?;
    let written = write_master(&master, &data)?;
    Ok(json!({ "ok": written == data.len(), "written": written }).to_string())
}

fn read_in(registry: &Mutex<Table>, args: &Value) -> Result<String, ToolError> {
    sweep(registry);
    let offset = match args.get("offset") {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| files::invalid("\"offset\" must be a whole number of bytes"))?,
    };
    let shared = with_term(registry, args, |term| Arc::clone(&term.shared))?;
    let ring = locked(&shared.ring);
    let total = ring.total;
    if offset > total {
        return Err(files::invalid(format!(
            "offset {offset} is past the end of the output ({total} bytes)"
        )));
    }
    let oldest = ring.oldest();
    let lost = oldest.saturating_sub(offset);
    let start = offset.max(oldest);
    let len = (total - start).min(READ_MAX as u64) as usize;
    let from = (start - oldest) as usize;
    let data: Vec<u8> = ring.bytes.range(from..from + len).copied().collect();
    drop(ring);
    let exit_code = shared.exit_code();
    Ok(json!({
        "term": files::string_arg(args, "term")?,
        "offset": start,
        "next": start + len as u64,
        "data_b64": STANDARD.encode(&data),
        "exited": exit_code.is_some(),
        "exit_code": exit_code,
        "lost": lost,
    })
    .to_string())
}

fn resize_in(registry: &Mutex<Table>, args: &Value) -> Result<String, ToolError> {
    sweep(registry);
    let cols = size_arg(args, "cols")?;
    let rows = size_arg(args, "rows")?;
    let master = with_term(registry, args, |term| {
        term.cols = cols;
        term.rows = rows;
        Arc::clone(&term.master)
    })?;
    let size = winsize(cols, rows);
    // SAFETY: TIOCSWINSZ reads one winsize from the pointer, which lives for the call. The master is open
    // (the Arc is held here).
    let rc = unsafe {
        libc::ioctl(
            master.as_raw_fd(),
            libc::TIOCSWINSZ as _,
            &size as *const libc::winsize,
        )
    };
    if rc != 0 {
        return Err(internal(format!(
            "cannot resize the terminal: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(json!({ "term": files::string_arg(args, "term")?, "cols": cols, "rows": rows }).to_string())
}

fn close_in(registry: &Mutex<Table>, args: &Value) -> Result<String, ToolError> {
    sweep(registry);
    let name = files::string_arg(args, "term")?;
    let id = parse_term(&name)?;
    let term = locked(registry)
        .terms
        .remove(&id)
        .ok_or_else(|| files::not_found(format!("no terminal {name:?}")))?;
    retire(term);
    Ok(json!({ "term": name, "closed": true }).to_string())
}

fn list_in(registry: &Mutex<Table>, _args: &Value) -> Result<String, ToolError> {
    sweep(registry);
    let table = locked(registry);
    let terminals: Vec<Value> = table
        .terms
        .iter()
        .map(|(id, term)| {
            json!({
                "term": term_name(*id),
                "cols": term.cols,
                "rows": term.rows,
                "started_ms": term.started_ms,
                "exited": term.shared.exit_code().is_some(),
            })
        })
        .collect();
    Ok(json!({ "terminals": terminals }).to_string())
}

/// Runs `act` on the terminal `args.term` names, and counts the call as activity on it.
fn with_term<T>(
    registry: &Mutex<Table>,
    args: &Value,
    act: impl FnOnce(&mut Term) -> T,
) -> Result<T, ToolError> {
    let name = files::string_arg(args, "term")?;
    let id = parse_term(&name)?;
    let mut table = locked(registry);
    let term = table
        .terms
        .get_mut(&id)
        .ok_or_else(|| files::not_found(format!("no terminal {name:?}")))?;
    term.last_active = Instant::now();
    Ok(act(term))
}

/// Closes, in the background, every terminal idle for [`IDLE`].
fn sweep(registry: &Mutex<Table>) {
    for term in take_idle(registry, Instant::now()) {
        thread::spawn(move || retire(term));
    }
}

/// Removes and returns the terminals idle at `now`.
fn take_idle(registry: &Mutex<Table>, now: Instant) -> Vec<Term> {
    let mut table = locked(registry);
    let idle: Vec<u64> = table
        .terms
        .iter()
        .filter(|(_, term)| now.duration_since(term.last_active) >= IDLE)
        .map(|(id, _)| *id)
        .collect();
    idle.into_iter()
        .filter_map(|id| table.terms.remove(&id))
        .collect()
}

/// Ends a terminal. The master is dropped (the reader drops its own duplicate within [`TICK`]), which
/// hangs the session up. The shell gets [`CLOSE_GRACE`] to exit. Then the session's process group is
/// SIGKILLed, and the shell reaped.
fn retire(term: Term) {
    let Term {
        master,
        mut child,
        pid,
        shared,
        ..
    } = term;
    shared.stop.store(true, Ordering::SeqCst);
    drop(master);
    let deadline = Instant::now() + CLOSE_GRACE;
    while exit_of(pid).is_none() && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    // SAFETY: a plain kill. The shell is alive or a zombie, and either way holds `pid`, so `-pid`
    // names its own group and no other.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

/// Starts the shell on `pty`'s slave in `dir` and the reader on its master.
fn spawn(
    workspace: &Workspace,
    dir: &Path,
    pty: Pty,
    cols: u16,
    rows: u16,
) -> Result<Term, ToolError> {
    let (program, args): (&str, &[&str]) = if Path::new(BASH).exists() {
        (BASH, &["-l"])
    } else {
        (SHELL, &[])
    };
    let Pty { master, slave } = pty;
    let stdout = slave
        .try_clone()
        .map_err(|err| internal(format!("cannot dup the terminal: {err}")))?;
    let stderr = slave
        .try_clone()
        .map_err(|err| internal(format!("cannot dup the terminal: {err}")))?;
    let mut builder = Command::new(program);
    builder.args(args);
    // The environment every guest command has; this builder makes its own session, so it cannot take the
    // process group `bash::command` gives.
    bash::environment(&mut builder, workspace.dir(), Privilege::Agent).map_err(internal)?;
    builder
        .current_dir(dir)
        .env("TERM", "xterm-256color")
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
        .map_err(|err| internal(format!("cannot start the shell: {err}")))?;
    // Dropping the builder closes the parent's copies of the slave, so the master sees end of file when
    // the shell's side is gone.
    drop(builder);
    let pid = child.id() as i32;
    let master = File::from(master);
    let reader = master
        .try_clone()
        .map_err(|err| internal(format!("cannot dup the master: {err}")))?;
    let shared = Arc::new(Shared::default());
    let reader_shared = Arc::clone(&shared);
    thread::spawn(move || pump(reader, reader_shared, pid));
    Ok(Term {
        master: Arc::new(master),
        child,
        pid,
        cols,
        rows,
        started_ms: now_ms(),
        last_active: Instant::now(),
        shared,
    })
}

/// Reads the master into the ring until the terminal is stopped, and notes the shell's exit when it
/// comes. The shell is not reaped here; [`retire`] does that.
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
        if let Some(code) = exit_of(pid) {
            *locked(&shared.exit) = Some(code);
        }
    }
}

/// Writes `data` to the master without waiting past [`WRITE_WAIT`]. The descriptor is non-blocking; a
/// full input queue waits for the shell to read. Returns how many bytes went in.
fn write_master(master: &File, data: &[u8]) -> Result<usize, ToolError> {
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
                return Err(ToolError::Unavailable {
                    reason: "the terminal's shell has exited".to_string(),
                });
            }
            Err(err) => return Err(internal(format!("cannot write to the terminal: {err}"))),
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
    fn open(cols: u16, rows: u16) -> Result<Pty, ToolError> {
        let size = winsize(cols, rows);
        let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
        // SAFETY: on success openpty stores two new descriptors, which are owned from the next line. The
        // name and termios pointers may be null, which means the defaults; the winsize is read only.
        let rc =
            unsafe { libc::openpty(&mut master, &mut slave, ptr::null_mut(), ptr::null(), &size) };
        if rc != 0 {
            return Err(internal(format!(
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
            .map_err(|err| internal(format!("cannot set up the terminal: {err}")))?;
        cloexec(pty.slave.as_raw_fd())
            .map_err(|err| internal(format!("cannot set up the terminal: {err}")))?;
        nonblocking(pty.master.as_raw_fd())
            .map_err(|err| internal(format!("cannot set up the terminal: {err}")))?;
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

/// Whether `fd` has one of `events` ready within `timeout_ms`. A hang-up counts as ready, so the
/// caller finds out with its next read or write.
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

/// The shell's exit code, if it has exited. Read without reaping (WNOWAIT), so the pid stays reserved
/// for the session's group until [`retire`] reaps it. A signal's end is 128 plus the signal, which is
/// what a shell reports for one.
fn exit_of(pid: i32) -> Option<i32> {
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

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

impl Shared {
    fn exit_code(&self) -> Option<i32> {
        *locked(&self.exit)
    }
}

fn size_arg(args: &Value, key: &str) -> Result<u16, ToolError> {
    args.get(key)
        .and_then(Value::as_u64)
        .filter(|size| (1..=MAX_SIZE).contains(size))
        .map(|size| size as u16)
        .ok_or_else(|| {
            files::invalid(format!(
                "{key:?} must be a whole number from 1 to {MAX_SIZE}"
            ))
        })
}

fn term_name(id: u64) -> String {
    format!("term-{id}")
}

/// The id a terminal name stands for. Only the canonical spelling is accepted, so one terminal has
/// one name.
fn parse_term(name: &str) -> Result<u64, ToolError> {
    name.strip_prefix("term-")
        .and_then(|digits| digits.parse::<u64>().ok())
        .filter(|id| term_name(*id) == name)
        .ok_or_else(|| files::invalid(format!("{name:?} is not a terminal name")))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn internal(reason: impl Into<String>) -> ToolError {
    ToolError::Internal {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_proto::wire::MAX_TOOL_OUTPUT;
    use serde_json::json;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-term-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    fn registry() -> Mutex<Table> {
        Mutex::new(Table {
            next: 0,
            terms: BTreeMap::new(),
        })
    }

    fn open_at(registry: &Mutex<Table>, workspace: &Workspace, cwd: Option<&str>) -> String {
        let args = match cwd {
            Some(cwd) => json!({ "cols": 80, "rows": 24, "cwd": cwd }),
            None => json!({ "cols": 80, "rows": 24 }),
        };
        let reply = reply(open_in(registry, workspace, &args));
        reply["term"].as_str().expect("a term name").to_string()
    }

    fn reply(result: Result<String, ToolError>) -> Value {
        let text = result.expect("the tool succeeds");
        assert!(
            text.len() <= MAX_TOOL_OUTPUT,
            "a reply of {} bytes is over the cap",
            text.len()
        );
        serde_json::from_str(&text).expect("the reply is JSON")
    }

    fn type_text(registry: &Mutex<Table>, term: &str, text: &str) -> Value {
        let data_b64 = STANDARD.encode(text);
        reply(write_in(
            registry,
            &json!({ "term": term, "data_b64": data_b64 }),
        ))
    }

    fn decoded(reply: &Value) -> Vec<u8> {
        STANDARD
            .decode(reply["data_b64"].as_str().expect("data"))
            .expect("base64")
    }

    /// Reads `term` from its start, following `next`, until `done` returns a value for the output so far.
    fn read_with<T>(registry: &Mutex<Table>, term: &str, done: impl Fn(&str) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut offset = 0;
        let mut output = Vec::new();
        loop {
            let got = reply(read_in(
                registry,
                &json!({ "term": term, "offset": offset }),
            ));
            output.extend(decoded(&got));
            offset = got["next"].as_u64().expect("next");
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
    fn read_until(registry: &Mutex<Table>, term: &str, marker: &str) {
        read_with(registry, term, |text| text.contains(marker).then_some(()));
    }

    #[test]
    fn a_shell_runs_and_its_output_comes_back_by_offset() {
        let workspace = scratch("echo");
        let registry = registry();
        let term = open_at(&registry, &workspace, None);
        // The typed line echoes as written, so the marker is the output: "hi2" never appears in "hi$((1+1))".
        assert_eq!(type_text(&registry, &term, "echo hi$((1+1))\n")["ok"], true);
        read_until(&registry, &term, "hi2\r\n");
        let closed = reply(close_in(&registry, &json!({ "term": term })));
        assert_eq!(closed, json!({ "term": term, "closed": true }));
    }

    #[test]
    fn a_shell_starts_in_the_directory_it_is_given() {
        let workspace = scratch("cwd");
        fs::create_dir_all(workspace.dir().join("sub")).expect("a subdirectory");
        let registry = registry();
        let term = open_at(&registry, &workspace, Some("sub"));
        type_text(&registry, &term, "pwd\n");
        let expected = fs::canonicalize(workspace.dir().join("sub")).expect("canonical");
        read_until(&registry, &term, &format!("{}\r\n", expected.display()));
        reply(close_in(&registry, &json!({ "term": term })));
    }

    #[test]
    fn resize_changes_what_stty_reports() {
        let workspace = scratch("resize");
        let registry = registry();
        let term = open_at(&registry, &workspace, None);
        let resized = reply(resize_in(
            &registry,
            &json!({ "term": term, "cols": 100, "rows": 30 }),
        ));
        assert_eq!(
            (resized["cols"].clone(), resized["rows"].clone()),
            (json!(100), json!(30))
        );
        type_text(&registry, &term, "stty size\n");
        read_until(&registry, &term, "30 100\r\n");
        reply(close_in(&registry, &json!({ "term": term })));
    }

    #[test]
    fn resize_signals_the_foreground_group() {
        let workspace = scratch("winch");
        let registry = registry();
        let term = open_at(&registry, &workspace, None);
        type_text(
            &registry,
            &term,
            "trap 'echo got-winch' WINCH; echo arm$((1+1))\n",
        );
        read_until(&registry, &term, "arm2\r\n");
        reply(resize_in(
            &registry,
            &json!({ "term": term, "cols": 100, "rows": 30 }),
        ));
        // The shell is the foreground group, and its trap runs on SIGWINCH.
        read_until(&registry, &term, "got-winch\r\n");
        reply(close_in(&registry, &json!({ "term": term })));
    }

    #[test]
    fn close_ends_the_shell_and_its_process() {
        let workspace = scratch("close");
        let registry = registry();
        let term = open_at(&registry, &workspace, None);
        type_text(&registry, &term, "echo pid:$$\n");
        // The typed line has "pid:$$"; the output has "pid:<digits>\r\n", which only the output has.
        let pid: u32 = read_with(&registry, &term, |text| {
            text.split("pid:").skip(1).find_map(|rest| {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                (!digits.is_empty() && rest[digits.len()..].starts_with("\r\n"))
                    .then(|| digits.parse().expect("a pid"))
            })
        });
        assert!(Path::new(&format!("/proc/{pid}")).exists());
        reply(close_in(&registry, &json!({ "term": term })));
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "the shell outlived its close"
        );
        let listed = reply(list_in(&registry, &json!({})));
        assert_eq!(listed["terminals"], json!([]));
    }

    #[test]
    fn a_shell_that_exits_is_reported_with_its_code() {
        let workspace = scratch("exit");
        let registry = registry();
        let term = open_at(&registry, &workspace, None);
        type_text(&registry, &term, "exit 3\n");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let got = reply(read_in(&registry, &json!({ "term": term })));
            if got["exited"] == true {
                assert_eq!(got["exit_code"], json!(3));
                break;
            }
            assert!(Instant::now() < deadline, "the shell did not exit");
            thread::sleep(Duration::from_millis(20));
        }
        let listed = reply(list_in(&registry, &json!({})));
        assert_eq!(listed["terminals"][0]["exited"], true);
        reply(close_in(&registry, &json!({ "term": term })));
    }

    #[test]
    fn ring_overflow_reports_the_bytes_it_dropped() {
        let workspace = scratch("ring");
        let registry = registry();
        let term = open_at(&registry, &workspace, None);
        // More than the 1 MiB ring holds, then a marker at the end.
        type_text(
            &registry,
            &term,
            "head -c 1200000 /dev/zero | tr '\\0' a; echo END$((1+2))\n",
        );
        // Reads from 0 do not advance until the output has overflowed the ring; the first such read
        // reports the bytes it dropped, and the reads after it go on from where it stopped.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut offset = 0;
        let mut lost_seen = None;
        let mut output = Vec::new();
        loop {
            let got = reply(read_in(
                &registry,
                &json!({ "term": term, "offset": offset }),
            ));
            let lost = got["lost"].as_u64().expect("lost");
            if lost_seen.is_none() && lost > 0 {
                assert_eq!(
                    got["offset"],
                    json!(lost),
                    "the read starts where the lost bytes end"
                );
                lost_seen = Some(lost);
            }
            if lost_seen.is_some() {
                output.extend(decoded(&got));
                offset = got["next"].as_u64().expect("next");
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
        reply(close_in(&registry, &json!({ "term": term })));
    }

    #[test]
    fn at_most_four_terminals_are_open() {
        let workspace = scratch("limit");
        let registry = registry();
        let names: Vec<String> = (0..MAX_TERMS)
            .map(|_| open_at(&registry, &workspace, None))
            .collect();
        let refused = open_in(&registry, &workspace, &json!({ "cols": 80, "rows": 24 }));
        assert!(
            matches!(refused, Err(ToolError::LimitExceeded { .. })),
            "{refused:?}"
        );
        reply(close_in(&registry, &json!({ "term": names[0] })));
        let again = open_at(&registry, &workspace, None);
        for name in names[1..].iter().chain(std::iter::once(&again)) {
            reply(close_in(&registry, &json!({ "term": name })));
        }
    }

    #[test]
    fn a_cwd_outside_the_workspace_is_refused() {
        let workspace = scratch("escape");
        symlink("/", workspace.dir().join("out")).expect("a link out");
        let registry = registry();
        for cwd in ["..", "../..", "/etc", "out"] {
            let refused = open_in(
                &registry,
                &workspace,
                &json!({ "cols": 80, "rows": 24, "cwd": cwd }),
            );
            assert!(
                matches!(refused, Err(ToolError::Denied { .. })),
                "{cwd}: {refused:?}"
            );
        }
        assert_eq!(
            reply(list_in(&registry, &json!({})))["terminals"],
            json!([])
        );
    }

    #[test]
    fn a_terminal_idle_past_the_limit_is_taken_and_the_fresh_one_is_kept() {
        let workspace = scratch("idle");
        let registry = registry();
        let term = open_at(&registry, &workspace, None);
        assert!(
            take_idle(&registry, Instant::now()).is_empty(),
            "a fresh terminal is not idle"
        );
        let later = Instant::now() + IDLE + Duration::from_secs(1);
        let idle = take_idle(&registry, later);
        assert_eq!(idle.len(), 1);
        assert!(locked(&registry).terms.is_empty());
        for term in idle {
            retire(term);
        }
        let _ = term;
    }

    #[test]
    fn bad_arguments_are_refused_before_anything_starts() {
        let workspace = scratch("args");
        let registry = registry();
        let zero = open_in(&registry, &workspace, &json!({ "cols": 0, "rows": 24 }));
        assert!(
            matches!(zero, Err(ToolError::InvalidInput { .. })),
            "{zero:?}"
        );
        let name = open_in(&registry, &workspace, &json!({ "cols": 80, "rows": 24 })).map(|_| ());
        assert!(name.is_ok());
        let not_a_name = write_in(&registry, &json!({ "term": "shell-1", "data_b64": "" }));
        assert!(
            matches!(not_a_name, Err(ToolError::InvalidInput { .. })),
            "{not_a_name:?}"
        );
        let unknown = read_in(&registry, &json!({ "term": "term-99" }));
        assert!(
            matches!(unknown, Err(ToolError::NotFound { .. })),
            "{unknown:?}"
        );
        let big = STANDARD.encode(vec![b'a'; WRITE_MAX + 1]);
        let too_big = write_in(&registry, &json!({ "term": "term-1", "data_b64": big }));
        assert!(
            matches!(too_big, Err(ToolError::LimitExceeded { .. })),
            "{too_big:?}"
        );
        let not_base64 = write_in(&registry, &json!({ "term": "term-1", "data_b64": "@@" }));
        assert!(
            matches!(not_base64, Err(ToolError::InvalidInput { .. })),
            "{not_base64:?}"
        );
        for (_, term) in std::mem::take(&mut locked(&registry).terms) {
            retire(term);
        }
    }
}
