//! The Win32 helpers the Windows backends of runs and terminals share: wide strings, the environment block,
//! a job object that ends a process tree as a whole, the attribute list a process is started with, and the
//! start of a process suspended, so it can be put in its job before it runs.
//!
//! This module calls the Win32 API. Each `unsafe` block says what its calls rely on.

#![allow(unsafe_code)]

use std::ffi::{OsString, c_void};
use std::io;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{
    FALSE, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, TRUE, WAIT_FAILED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateEventW, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, ResumeThread, STARTUPINFOEXW, STARTUPINFOW,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows_sys::core::BOOL;

use crate::env::Env;

/// A NUL-terminated UTF-16 copy of `text`, as a Win32 string argument takes it.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A NUL-terminated UTF-16 copy of `path`.
pub(crate) fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// `path` without the `\\?\` verbatim prefix that canonicalization adds. A shell's working directory takes the
/// plain form, which `cmd.exe` and PowerShell print without complaint. A UNC path keeps its prefix.
pub(crate) fn plain_path(path: &Path) -> PathBuf {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// The environment block a process is created with: each `name=value` as UTF-16 and a NUL, then one more NUL.
/// `extra` is set after the environment's own variables, so it wins for a name both give. A name or value with a
/// NUL in it cannot be a block entry, so it is refused.
pub(crate) fn environment_block(env: &Env, extra: &[(&str, &str)]) -> io::Result<Vec<u16>> {
    let mut pairs = env.pairs();
    for &(key, value) in extra {
        pairs.insert(OsString::from(key), OsString::from(value));
    }
    let mut block = Vec::new();
    for (key, value) in &pairs {
        if key.encode_wide().any(|unit| unit == 0) || value.encode_wide().any(|unit| unit == 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an environment name or value holds a NUL",
            ));
        }
        block.extend(key.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// A job object with kill-on-close: the processes in it end when its last handle closes, and [`Job::kill`] ends
/// them at once. A process is put in it with [`Job::assign`] before it runs, so its children are in it too.
pub(crate) struct Job(OwnedHandle);

impl Job {
    /// An unnamed job object that ends its processes when it closes.
    pub(crate) fn new() -> io::Result<Job> {
        // SAFETY: null security attributes and a null name make an unnamed job object. A non-null return is a new
        // handle that nothing else owns, so OwnedHandle takes it and closes it.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is the valid, owned job handle created above.
        let job = Job(unsafe { OwnedHandle::from_raw_handle(raw) });
        // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a valid value of that struct. Only the
        // kill-on-close flag is set on it.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: SetInformationJobObject reads one JOBOBJECT_EXTENDED_LIMIT_INFORMATION of the size passed, which
        // lives on the stack for the call.
        let ok = unsafe {
            SetInformationJobObject(
                job.handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn handle(&self) -> HANDLE {
        self.0.as_raw_handle()
    }

    /// Puts `process` in the job. It must not have run yet, so nothing it starts escapes the job.
    pub(crate) fn assign(&self, process: HANDLE) -> io::Result<()> {
        // SAFETY: both handles are open: the job is owned by `self`, and the caller keeps `process` open.
        let ok = unsafe { AssignProcessToJobObject(self.handle(), process) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Ends every process in the job. A job with no processes left is not an error, so the result is ignored.
    pub(crate) fn kill(&self) {
        // SAFETY: TerminateJobObject takes the open job handle and an exit code, and touches no memory.
        unsafe { TerminateJobObject(self.handle(), 1) };
    }
}

/// The attribute list a process is created with. It holds `count` attributes, set with [`AttrList::set`].
pub(crate) struct AttrList {
    buf: Vec<usize>,
}

impl AttrList {
    /// An attribute list with room for `count` attributes.
    pub(crate) fn new(count: u32) -> io::Result<AttrList> {
        let mut size = 0usize;
        // SAFETY: a null list asks only for the size the list needs. This call is expected to fail with
        // "insufficient buffer", and `size` is the value it writes.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut size) };
        let mut list = AttrList {
            buf: vec![0usize; size.div_ceil(mem::size_of::<usize>())],
        };
        // SAFETY: `buf` holds `size` bytes of usize-aligned memory, which this list owns until it is dropped.
        let ok =
            unsafe { InitializeProcThreadAttributeList(list.as_mut_ptr(), count, 0, &mut size) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(list)
    }

    /// The list as the startup info takes it. It stays valid while this value lives.
    pub(crate) fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buf.as_mut_ptr().cast()
    }

    /// Sets one attribute. `value` is read when the process is created, so it must stay valid until then.
    pub(crate) fn set(
        &mut self,
        attribute: usize,
        value: *const c_void,
        len: usize,
    ) -> io::Result<()> {
        let list = self.as_mut_ptr();
        // SAFETY: the list was initialized with room for `count` attributes, and the caller keeps `value` valid for
        // `len` bytes until the process is created.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                attribute,
                value,
                len,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for AttrList {
    fn drop(&mut self) {
        // SAFETY: the list was initialized by `new` and is deleted once, here.
        unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
    }
}

/// A process created suspended: its process handle, and its primary thread, which [`resume`] starts.
pub(crate) struct Started {
    pub(crate) process: OwnedHandle,
    pub(crate) thread: OwnedHandle,
}

/// Creates a process suspended, with `command_line` (a writable copy is made), in `dir`, with the environment
/// `environment` (see [`environment_block`]) and the startup info `startup`, whose attribute list the caller
/// keeps alive. `inherit` says whether the process inherits the handles its attribute list allows, and `flags`
/// are extra creation flags.
pub(crate) fn create_suspended(
    command_line: &str,
    dir: &Path,
    environment: &[u16],
    startup: &mut STARTUPINFOEXW,
    inherit: bool,
    flags: u32,
) -> io::Result<Started> {
    let mut command = wide(command_line);
    let dir = wide_path(dir);
    startup.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
    let inherit: BOOL = if inherit { TRUE } else { FALSE };
    // SAFETY: `command` is a writable NUL-terminated buffer, which CreateProcessW may write to. `environment` is a
    // UTF-16 block of NUL-terminated entries ended by an empty one, and CREATE_UNICODE_ENVIRONMENT says so. `dir`
    // is NUL-terminated. `startup` is a STARTUPINFOEXW, passed as its STARTUPINFOW prefix with the extended flag
    // set; its `cb` and attribute list are set above and the caller keeps the list alive. `info` receives two
    // handles, which OwnedHandle takes below.
    let mut info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            ptr::null(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            inherit,
            flags | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            dir.as_ptr(),
            (startup as *const STARTUPINFOEXW).cast::<STARTUPINFOW>(),
            &mut info,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateProcessW succeeded, so both handles are open and owned by no one else.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    // SAFETY: as above.
    let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
    Ok(Started { process, thread })
}

/// Starts the suspended primary thread of a process.
pub(crate) fn resume(thread: &OwnedHandle) -> io::Result<()> {
    // SAFETY: the thread handle is open and owned by `thread`. ResumeThread touches no memory.
    let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
    if previous == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// How a wait on a handle ended.
pub(crate) enum Waited {
    /// The handle is signaled: a process has exited, or an event or overlapped operation has completed.
    Signaled,
    /// The wait's time ran out first.
    TimedOut,
}

/// Waits up to `millis` for the handle to be signaled. `u32::MAX` waits without a limit.
pub(crate) fn wait_for(handle: HANDLE, millis: u32) -> io::Result<Waited> {
    // SAFETY: the handle is an open handle that a process or event object holds, and the caller keeps it open for
    // the call.
    match unsafe { WaitForSingleObject(handle, millis) } {
        WAIT_OBJECT_0 => Ok(Waited::Signaled),
        WAIT_TIMEOUT => Ok(Waited::TimedOut),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        other => Err(io::Error::other(format!(
            "WaitForSingleObject returned {other}"
        ))),
    }
}

/// The exit code of a process that has exited, or `None` when it is still running or the code cannot be read.
pub(crate) fn exit_code_of(process: HANDLE) -> Option<i32> {
    let mut code = 0u32;
    // SAFETY: the handle is an open process handle, and `code` is the one u32 GetExitCodeProcess writes.
    let ok = unsafe { GetExitCodeProcess(process, &mut code) };
    if ok == 0 || code == 259 {
        // 259 is STILL_ACTIVE: the process has not exited.
        return None;
    }
    Some(code as i32)
}

/// Ends one process at once, for one that was never put in a job. A process that already exited is left alone.
pub(crate) fn terminate(process: HANDLE) {
    // SAFETY: the handle is an open process handle held by the caller. TerminateProcess touches no memory.
    unsafe { TerminateProcess(process, 1) };
}

/// Sets whether a handle is inherited by a process created with inheritance (the handle list still applies).
pub(crate) fn set_inheritable(handle: HANDLE, inheritable: bool) -> io::Result<()> {
    let flag = if inheritable { HANDLE_FLAG_INHERIT } else { 0 };
    // SAFETY: the handle is an open handle held by the caller; SetHandleInformation changes only its inherit flag.
    let ok = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, flag) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// An unnamed, manual-reset event that starts unset. Overlapped I/O signals it when an operation completes.
pub(crate) fn event() -> io::Result<OwnedHandle> {
    // SAFETY: null attributes and a null name make an unnamed event, not inherited. A non-null return is a new handle
    // that nothing else owns.
    let raw = unsafe { CreateEventW(ptr::null(), TRUE, FALSE, ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is the valid, owned event handle created above.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}
