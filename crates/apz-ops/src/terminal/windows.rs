//! The windows terminals: a ConPTY per terminal, `powershell.exe -NoLogo` (else `cmd.exe`) started on it in a job
//! object, a reader thread on its output pipe, and a waiter thread on the shell's exit.
//!
//! Input goes through an overlapped named pipe, so a write can wait for the shell the way the unix write does: it
//! waits at most [`WRITE_WAIT`], cancels what has not gone in, and reports how much did. Closing closes the pseudo
//! console, which hangs the shell up. The shell gets [`CLOSE_GRACE`] to exit on its own, and then the job is killed,
//! which ends whatever is left of its tree.
//!
//! The table, the ring, the idle janitor and the argument rules are shared in [`super::table`]. This module makes
//! the console and the shell and ends them.
//!
//! This module calls the Win32 console, pipe, I/O and process API through [`crate::win`]. Each `unsafe` block says
//! what its calls rely on.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io::{self, Read};
use std::mem;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA,
    ERROR_OPERATION_ABORTED, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_OUTBOUND, WriteFile,
};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use super::table::{
    Idle, MAX_TERMS, Ring, Table, decode_write, encode_read, locked, now_ms, remove_term, size,
    spawn_janitor, sweep, term_name, with_term,
};
use super::{TermClosed, TermInfo, TermList, TermOpened, TermRead, TermResized, TermWritten};
use crate::env::Env;
use crate::error::OpError;
use crate::root::{Root, locate};
use apz_ide::path::check_dir;

use crate::win::{
    AttrList, Job, Waited, create_suspended, environment_block, event, exit_code_of, plain_path,
    resume, terminate, wait_for, wide,
};

/// How long a write waits for the shell to take what it sends before it returns what went in.
const WRITE_WAIT: Duration = Duration::from_secs(2);
/// How long a closing shell has to exit on its own before its job is killed.
const CLOSE_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);
/// The shells tried in order: PowerShell, then `cmd.exe` where PowerShell is missing.
const SHELLS: [&str; 2] = ["powershell.exe -NoLogo", "cmd.exe"];
/// Names for the input pipes, which must be unique for the life of the process.
static PIPE_SEQ: AtomicU64 = AtomicU64::new(0);

/// The terminals one table holds. A table starts a janitor thread that closes idle terminals, and closes every
/// terminal it holds when it is dropped.
pub struct Terminals {
    table: Arc<Mutex<Table<Term>>>,
}

/// One terminal: its input, its console, its job, and the state the reader and waiter share with the calls.
struct Term {
    /// The server end of the input pipe, which the console reads from its client end.
    input: Arc<OwnedHandle>,
    /// The pseudo console. Dropping it closes it, which hangs the shell up.
    console: Conpty,
    /// The job that holds the shell and what it starts.
    job: Job,
    shared: Arc<Shared>,
    cols: u16,
    rows: u16,
    started_ms: u64,
    last_active: Instant,
}

/// What the reader and the waiter fill, and the calls read.
struct Shared {
    ring: Mutex<Ring>,
    /// The shell's exit code, once it has exited.
    exit: Mutex<Option<i32>>,
}

/// A pseudo console. Dropping it closes it.
struct Conpty(HPCON);

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

impl Drop for Conpty {
    fn drop(&mut self) {
        // SAFETY: the console was created by CreatePseudoConsole and is closed once, here.
        unsafe { ClosePseudoConsole(self.0) };
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

    /// Opens a shell in a new terminal, in `cwd` (a directory of the root; `""` is the root).
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
        let metadata = std::fs::metadata(&target.abs)
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
        let term = spawn(env, &target.abs, cols, rows)?;
        table.next += 1;
        let id = table.next;
        table.terms.insert(id, term);
        Ok(TermOpened {
            term: term_name(id),
        })
    }

    /// Writes base64 `data_b64` to the terminal, at most [`WRITE_MAX`](super::table::WRITE_MAX) bytes of it.
    pub fn write(&self, term: &str, data_b64: &str) -> Result<TermWritten, OpError> {
        sweep(&self.table, retire);
        let data = decode_write(data_b64)?;
        let input = with_term(&self.table, term, |term| Arc::clone(&term.input))?;
        let written = write_input(input.as_raw_handle(), &data)?;
        Ok(TermWritten {
            ok: written == data.len(),
            written,
        })
    }

    /// The terminal's output from `offset` (the start when `None`), at most [`READ_MAX`](super::table::READ_MAX)
    /// bytes.
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

    /// Sets the terminal's size. The console tells the shell, as a unix terminal's resize signals its group.
    pub fn resize(&self, term: &str, cols: u64, rows: u64) -> Result<TermResized, OpError> {
        sweep(&self.table, retire);
        let cols = size(cols, "cols")?;
        let rows = size(rows, "rows")?;
        with_term(&self.table, term, |term| -> Result<(), OpError> {
            let coord = COORD {
                X: cols as i16,
                Y: rows as i16,
            };
            // SAFETY: the console is open while its term is in the table, which this call holds locked. The
            // size is passed by value.
            let hr = unsafe { ResizePseudoConsole(term.console.0, coord) };
            if hr < 0 {
                return Err(OpError::internal(format!(
                    "cannot resize the terminal: HRESULT {hr:#x}"
                )));
            }
            term.cols = cols;
            term.rows = rows;
            Ok(())
        })??;
        Ok(TermResized {
            cols,
            rows,
            term: term.to_string(),
        })
    }

    /// Closes the terminal and ends its shell and its job. Returns once the shell is gone or its grace is over.
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

/// Creates the input pipe: an overlapped named pipe whose server end is ours and whose client end the console
/// reads. The name is unique to this process and this call.
fn input_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let name = format!(
        r"\\.\pipe\apz-term-{}-{}",
        std::process::id(),
        PIPE_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let name = wide(&name);
    // SAFETY: the name is a NUL-terminated UTF-16 string that lives for the call; null security attributes give
    // the default. The one instance is created overlapped, so writes can be timed.
    let server = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_OUTBOUND | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            64 * 1024,
            0,
            ptr::null(),
        )
    };
    if server == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateNamedPipeW returned a valid handle that nothing else owns.
    let server = unsafe { OwnedHandle::from_raw_handle(server) };
    // SAFETY: the name is valid for the call. The client opens the instance the server just created, for reading
    // only, as the console does.
    let client = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ,
            0,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if client == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned a valid handle that nothing else owns.
    let client = unsafe { OwnedHandle::from_raw_handle(client) };
    Ok((server, client))
}

/// Writes `data` to the input pipe, waiting at most [`WRITE_WAIT`] for the shell to take it. A write still pending
/// then is cancelled, and the bytes it moved are counted. Returns how many bytes went in.
fn write_input(pipe: HANDLE, data: &[u8]) -> Result<usize, OpError> {
    let deadline = Instant::now() + WRITE_WAIT;
    let mut written = 0usize;
    while written < data.len() {
        let chunk = &data[written..];
        // Each write gets its own event: a manual-reset event left signaled by one write would make the next look
        // complete before it is.
        let done = event()
            .map_err(|err| OpError::internal(format!("cannot write to the terminal: {err}")))?;
        // SAFETY: OVERLAPPED is plain data, and all-zero is a valid one. Its event is ours and lives for the call.
        let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
        overlapped.hEvent = done.as_raw_handle();
        let mut sent: u32 = 0;
        let len = chunk.len().min(u32::MAX as usize) as u32;
        // SAFETY: `chunk` is valid for `len` bytes, and `sent` and `overlapped` live until the operation completes
        // or is cancelled below, before they go out of scope.
        let ok = unsafe { WriteFile(pipe, chunk.as_ptr(), len, &mut sent, &mut overlapped) };
        if ok == 0 {
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(code) if code == ERROR_IO_PENDING as i32 => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    let millis = left.as_millis().min(u32::MAX as u128) as u32;
                    if let Waited::TimedOut =
                        wait_for(done.as_raw_handle(), millis).map_err(|err| {
                            OpError::internal(format!("cannot write to the terminal: {err}"))
                        })?
                    {
                        // SAFETY: CancelIoEx cancels the one operation `overlapped` names on `pipe`.
                        unsafe { CancelIoEx(pipe, &overlapped) };
                        // SAFETY: waiting for the cancelled operation to finish means `sent` is no longer written
                        // to. The result is FALSE with ERROR_OPERATION_ABORTED, and `sent` holds what it moved.
                        unsafe { GetOverlappedResult(pipe, &overlapped, &mut sent, 1) };
                        return Ok(written + sent as usize);
                    }
                }
                Some(code) if code == ERROR_NO_DATA as i32 || code == ERROR_BROKEN_PIPE as i32 => {
                    // The console has closed its end: the shell has gone.
                    return Err(OpError::Unavailable(
                        "the terminal's shell has exited".to_string(),
                    ));
                }
                _ => {
                    return Err(OpError::internal(format!(
                        "cannot write to the terminal: {err}"
                    )));
                }
            }
        }
        // The write completed, or its wait did, so this reads how much it moved.
        // SAFETY: the operation has completed, so this returns at once and writes `sent`.
        let read = unsafe { GetOverlappedResult(pipe, &overlapped, &mut sent, 0) };
        if read == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_OPERATION_ABORTED as i32) {
                return Err(OpError::internal(format!(
                    "cannot write to the terminal: {err}"
                )));
            }
        }
        if sent == 0 {
            break;
        }
        written += sent as usize;
    }
    Ok(written)
}

/// Starts the shell on a new console in `dir`, with `env`, and the reader and the waiter on it.
fn spawn(env: &Env, dir: &Path, cols: u16, rows: u16) -> Result<Term, OpError> {
    let (input_server, input_client) = input_pipe()
        .map_err(|err| OpError::internal(format!("cannot open the terminal's input: {err}")))?;
    let (output_reader, output_writer) = io::pipe()
        .map_err(|err| OpError::internal(format!("cannot open the terminal's output: {err}")))?;
    let coord = COORD {
        X: cols as i16,
        Y: rows as i16,
    };
    let mut hpc: HPCON = 0;
    // SAFETY: CreatePseudoConsole reads the two pipe handles, which stay open for the call, and writes the new
    // console into `hpc`. The size is passed by value.
    let hr = unsafe {
        CreatePseudoConsole(
            coord,
            input_client.as_raw_handle(),
            output_writer.as_raw_handle(),
            0,
            &mut hpc,
        )
    };
    if hr < 0 {
        return Err(OpError::internal(format!(
            "cannot open a pseudo console: HRESULT {hr:#x}"
        )));
    }
    // From here the console is owned, and dropping it closes it.
    let console = Conpty(hpc);
    // The console has its own copies of the pipe ends it reads and writes.
    drop(input_client);
    drop(output_writer);

    let mut attributes = AttrList::new(1)
        .map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;
    // ConPTY passes the handle itself as the attribute's value (an input attribute: the list keeps `lpValue` as it
    // is, not what it points to). A pointer to the handle is not a console, and the shell then dies at startup
    // (0xC0000142 on the runner).
    attributes
        .set(
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            console.0 as usize as *const c_void,
            mem::size_of::<HPCON>(),
        )
        .map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;
    // SAFETY: an all-zero STARTUPINFOEXW is a valid value of that struct. Its attribute list is set below.
    let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup.lpAttributeList = attributes.as_mut_ptr();
    // Without STARTF_USESTDHANDLES a console child takes this process's standard handles, even with inheritance
    // off: under a service or a test runner those are pipes, so the shell writes its prompt there, reads end of
    // file and exits 0, and the console sees nothing. Invalid handles make it fall back to its console, the ConPTY.
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
    let environment = environment_block(env, &[("TERM", "xterm-256color")])
        .map_err(|err| OpError::invalid(format!("the environment is not usable: {err}")))?;
    let job =
        Job::new().map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;

    let mut started = None;
    for shell in SHELLS {
        match create_suspended(
            shell,
            &plain_path(dir),
            &environment,
            &mut startup,
            false,
            CREATE_SUSPENDED,
        ) {
            Ok(child) => {
                started = Some(child);
                break;
            }
            Err(err) if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {}
            Err(err) => {
                return Err(OpError::internal(format!("cannot start the shell: {err}")));
            }
        }
    }
    let Some(started) = started else {
        return Err(OpError::internal(
            "cannot start the shell: neither powershell.exe nor cmd.exe was found",
        ));
    };
    let process = started.process;
    let thread = started.thread;
    // The shell is in its job before it runs, so nothing it starts escapes the job.
    if let Err(err) = job.assign(process.as_raw_handle()) {
        terminate(process.as_raw_handle());
        return Err(OpError::internal(format!(
            "cannot put the shell in its job: {err}"
        )));
    }
    if let Err(err) = resume(&thread) {
        job.kill();
        return Err(OpError::internal(format!("cannot start the shell: {err}")));
    }
    drop(thread);

    let shared = Arc::new(Shared {
        ring: Mutex::new(Ring::default()),
        exit: Mutex::new(None),
    });
    let pumped = Arc::clone(&shared);
    thread::spawn(move || pump(output_reader, pumped));
    let watched = Arc::clone(&shared);
    thread::spawn(move || watch(process, watched));
    Ok(Term {
        input: Arc::new(input_server),
        console,
        job,
        shared,
        cols,
        rows,
        started_ms: now_ms(),
        last_active: Instant::now(),
    })
}

/// Reads the console's output into the ring until the console closes its end.
fn pump(mut output: io::PipeReader, shared: Arc<Shared>) {
    let mut chunk = [0u8; 8192];
    loop {
        match output.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => locked(&shared.ring).append(&chunk[..read]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            // A broken pipe: the console has closed its output, so nothing more will come.
            Err(_) => break,
        }
    }
}

/// Notes the shell's exit when it comes.
fn watch(process: OwnedHandle, shared: Arc<Shared>) {
    let handle = process.as_raw_handle();
    if wait_for(handle, u32::MAX).is_ok() {
        *locked(&shared.exit) = exit_code_of(handle);
    }
}

/// Ends a terminal. Closing the console hangs the shell up, and the shell gets [`CLOSE_GRACE`] to exit. Then the job
/// is killed, which ends whatever is left of the tree.
fn retire(term: Term) {
    let Term {
        input,
        console,
        job,
        shared,
        ..
    } = term;
    drop(input);
    drop(console);
    let deadline = Instant::now() + CLOSE_GRACE;
    while shared.exit_code().is_none() && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    job.kill();
    drop(job);
}

#[cfg(test)]
mod tests {
    use super::super::table::{IDLE, WRITE_MAX, take_idle};
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use std::fs;
    use std::os::windows::fs::symlink_dir;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    const PATH: &str = r"C:\Windows\System32;C:\Windows";
    /// Wide enough that a line the shell prints is not wrapped.
    const COLS: u64 = 200;
    const ROWS: u64 = 40;
    /// How long output may still come after the shell has exited, before a wait gives up on it.
    const EXIT_GRACE: Duration = Duration::from_secs(1);
    /// `ERROR_PRIVILEGE_NOT_HELD`: a symlink needs SeCreateSymbolicLinkPrivilege, which the runner lacks.
    const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;

    fn scratch(name: &str) -> (Root, Env) {
        let dir = std::env::temp_dir().join(format!("apz-ops-term-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch root");
        // A process's environment starts empty, and PowerShell cannot load without SYSTEMROOT (error 8009001d): the
        // test gives the shell what the workstation daemon gives it (apz_workstation::daemon::WINDOWS_VARS).
        let mut env = Env::new(&dir, PATH);
        for name in ["SYSTEMROOT", "COMSPEC", "WINDIR", "PATHEXT"] {
            if let Some(value) = std::env::var(name).ok().filter(|v| !v.is_empty()) {
                env = env.with_var(name, value);
            }
        }
        (Root::new(&dir), env)
    }

    fn open_at(terms: &Terminals, root: &Root, env: &Env, cwd: &str) -> String {
        terms
            .open(root, env, COLS, ROWS, cwd)
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

    /// Fails the wait when the shell has exited and [`EXIT_GRACE`] has passed without what the wait wants. A shell
    /// that exited will not print it, so the test ends at once with the exit code rather than at its timeout.
    fn fail_if_exited(got: &TermRead, exited_at: &mut Option<Instant>, output: &str) {
        if !got.exited {
            return;
        }
        let since = *exited_at.get_or_insert_with(Instant::now);
        assert!(
            since.elapsed() < EXIT_GRACE,
            "the shell exited with code {:?} before it printed what the test waits for; output so far: {output:?}",
            got.exit_code
        );
    }

    /// Reads `term` from its start, following `next`, until `done` returns a value for the output so far.
    fn read_with<T>(terms: &Terminals, term: &str, done: impl Fn(&str) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut offset = 0;
        let mut output = Vec::new();
        let mut exited_at = None;
        loop {
            let got = terms.read(term, Some(offset)).expect("the read");
            output.extend(decoded(&got));
            offset = got.next;
            let text = String::from_utf8_lossy(&output);
            if let Some(found) = done(&text) {
                return found;
            }
            fail_if_exited(&got, &mut exited_at, &text);
            assert!(
                Instant::now() < deadline,
                "not done within 20 s; output so far: {text:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Reads until `marker` is in the output.
    fn read_until(terms: &Terminals, term: &str, marker: &str) {
        read_with(terms, term, |text| text.contains(marker).then_some(()));
    }

    /// Whether a process with `pid` is still running.
    fn process_alive(pid: u32) -> bool {
        // SAFETY: OpenProcess takes no pointers. A null return means no such process, and a returned handle is
        // closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let alive = crate::win::exit_code_of(handle).is_none();
        // SAFETY: the handle was returned by OpenProcess above and is closed once.
        unsafe { CloseHandle(handle) };
        alive
    }

    #[test]
    fn a_shell_runs_and_its_output_comes_back_by_offset() {
        let (root, env) = scratch("echo");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        // The typed line echoes as written, so the marker is the output: "hi2" never appears in "(1+1)".
        let typed = terms
            .write(&term, &STANDARD.encode("Write-Output (\"hi\" + (1+1))\r\n"))
            .expect("the write goes in");
        assert!(typed.ok);
        read_until(&terms, &term, "hi2");
        let closed = terms.close(&term).expect("the close");
        assert_eq!(closed, TermClosed { closed: true, term });
    }

    #[test]
    fn a_shell_starts_in_the_directory_it_is_given() {
        let (root, env) = scratch("cwd");
        fs::create_dir_all(root.dir().join("sub")).expect("a subdirectory");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "sub");
        type_text(
            &terms,
            &term,
            "Write-Output (\"CWD=\" + (Get-Location).Path)\r\n",
        );
        let expected = plain_path(&fs::canonicalize(root.dir().join("sub")).expect("canonical"));
        read_until(&terms, &term, &format!("CWD={}", expected.display()));
        terms.close(&term).expect("the close");
    }

    #[test]
    fn the_shell_sees_the_environment_it_is_given() {
        let (root, env) = scratch("env");
        let env = env.with_var("APZ_OPS_TEST", "seen");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        type_text(
            &terms,
            &term,
            "Write-Output (\"v=\" + $env:APZ_OPS_TEST + (1+1))\r\n",
        );
        read_until(&terms, &term, "v=seen2");
        terms.close(&term).expect("the close");
    }

    #[test]
    fn resize_changes_what_the_console_reports() {
        let (root, env) = scratch("resize");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        let resized = terms.resize(&term, 100, 30).expect("the resize");
        assert_eq!((resized.cols, resized.rows), (100, 30));
        type_text(
            &terms,
            &term,
            "Write-Output (\"size=\" + [Console]::WindowWidth + \"x\" + [Console]::WindowHeight)\r\n",
        );
        read_until(&terms, &term, "size=100x30");
        terms.close(&term).expect("the close");
    }

    #[test]
    fn close_ends_the_shell_and_its_process() {
        let (root, env) = scratch("close");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        type_text(&terms, &term, "Write-Output (\"pid:\" + $PID)\r\n");
        // The typed line has "pid:" then "+ $PID"; the output has "pid:" then digits and a line end.
        let pid: u32 = read_with(&terms, &term, |text| {
            text.split("pid:").skip(1).find_map(|rest| {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                (!digits.is_empty() && rest[digits.len()..].starts_with(['\r', '\n']))
                    .then(|| digits.parse().expect("a pid"))
            })
        });
        assert!(process_alive(pid), "the shell runs before the close");
        terms.close(&term).expect("the close");
        let deadline = Instant::now() + Duration::from_secs(10);
        while process_alive(pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!process_alive(pid), "the shell outlived its close");
        assert!(terms.list().expect("the list").terminals.is_empty());
    }

    #[test]
    fn a_shell_that_exits_is_reported_with_its_code() {
        let (root, env) = scratch("exit");
        let terms = Terminals::new();
        let term = open_at(&terms, &root, &env, "");
        type_text(&terms, &term, "exit 3\r\n");
        let deadline = Instant::now() + Duration::from_secs(20);
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
            "Write-Output ('a' * 1200000); Write-Output (\"END\" + (1+2))\r\n",
        );
        // Reads from 0 do not advance until the output has overflowed the ring; the first such read reports
        // the bytes it dropped, and the reads after it go on from where it stopped.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut offset = 0;
        let mut lost_seen = None;
        let mut output = Vec::new();
        let mut exited_at = None;
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
            fail_if_exited(&got, &mut exited_at, &String::from_utf8_lossy(&output));
            assert!(
                Instant::now() < deadline,
                "no overflow and END3 within 60 s"
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
        let refused = terms.open(&root, &env, COLS, ROWS, "");
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

    /// A cwd that climbs out of the root, and one that is a link out of it. A junction needs no privilege, so the
    /// junction case always runs. A symlink needs SeCreateSymbolicLinkPrivilege, which the runner's service account
    /// lacks, so that case runs only where the privilege is held and prints why it did not run otherwise.
    #[test]
    fn a_cwd_outside_the_root_is_refused() {
        let (root, env) = scratch("escape");
        let junction = root.dir().join("junction");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(r"C:\")
            .status()
            .expect("cmd runs");
        assert!(made.success(), "mklink /J: {made}");
        let mut cwds = vec!["..", "../..", "junction"];
        match symlink_dir(r"C:\", root.dir().join("symlink")) {
            Ok(()) => cwds.push("symlink"),
            Err(err) if err.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
                println!("skipping the symlink case: {err} (no SeCreateSymbolicLinkPrivilege)");
            }
            Err(err) => panic!("a symlink out of the root: {err}"),
        }
        let terms = Terminals::new();
        for cwd in cwds {
            let refused = terms.open(&root, &env, COLS, ROWS, cwd);
            assert!(
                matches!(refused, Err(OpError::Denied(_))),
                "{cwd}: {refused:?}"
            );
        }
        assert!(terms.list().expect("the list").terminals.is_empty());
        // Removing a link removes only the link: its target, the drive root, stays.
        fs::remove_dir(&junction).expect("the junction is removed");
        let _ = fs::remove_dir(root.dir().join("symlink"));
        assert!(Path::new(r"C:\").exists(), "the link's target survives");
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
