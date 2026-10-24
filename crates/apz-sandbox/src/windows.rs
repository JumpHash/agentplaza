//! Windows backend: an AppContainer with no capabilities, inside a job object.
//!
//! - The container's SID is derived from the workdir path, so each workdir has its
//!   own container. The profile is created once and kept.
//! - The container reaches only what is granted: the workdir and the read-write
//!   paths (modify), the read-only paths and the program (read and execute). Those
//!   ACEs stay on the files after the child exits.
//! - A container with no capabilities has no network. `LoopbackPort` needs the container
//!   on the loopback exemption list, which only an administrator can change (see
//!   [`loopback`]): until it is there, that spec is refused.
//! - The process starts suspended, joins the job (kill on close, memory, active
//!   processes, per-process CPU time), and only then resumes.
//! - stdin, stdout and stderr are always pipes: std cannot tell what a `Stdio` is.

use std::ffi::{OsStr, c_void};
use std::fs::File;
use std::io::{self, Read, Write};
use std::iter::repeat_n;
use std::mem::{size_of, size_of_val, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    FALSE, HANDLE, HANDLE_FLAG_INHERIT, HLOCAL, LocalFree, SetHandleInformation, TRUE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT,
    SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, FreeSid, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    SECURITY_CAPABILITIES, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::loopback;
use crate::{Network, SandboxError, Spec, Support};

const CONTAINER_PREFIX: &str = "apz-sbx-";

/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`: the profile was created earlier.
const PROFILE_EXISTS: i32 = 0x8007_00B7_u32 as i32;

/// Access for the workdir and read-write paths.
const MODIFY: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

/// Access for read-only paths and the program.
const READ_EXECUTE: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;

pub(crate) fn support() -> Support {
    Support {
        kind: "appcontainer",
        detail: "AppContainer with no capabilities: no network. LoopbackPort needs the container \
                 on the loopback exemption list, which an administrator grants once (the hub \
                 asks the owner), and a filter that keeps it to the proxy's port; until then \
                 LoopbackPort is refused. Reads and writes only where granted: workdir and \
                 read_write (modify), read_only and the program file (read, execute); those \
                 ACEs stay on the files after the child exits, and the program's directory is \
                 not granted (put its DLLs in read_only). Job object: kill on close, memory, \
                 active processes, per-process CPU time. stdin, stdout and stderr are always \
                 piped."
            .to_owned(),
    }
}

pub(crate) fn command(_spec: &Spec) -> Result<Command, SandboxError> {
    Err(SandboxError::Unsupported(
        "on Windows use spawn: the AppContainer is created with CreateProcessW, which \
         std::process::Command cannot express"
            .to_owned(),
    ))
}

pub(crate) fn spawn(
    spec: &Spec,
    _stdin: Stdio,
    _stdout: Stdio,
    _stderr: Stdio,
) -> Result<Process, SandboxError> {
    // The three Stdio values cannot be read on Windows, so they are dropped here and
    // all three streams are piped instead (see the module docs).
    let p = crate::prepare(spec)?;
    let memory = match p.limits.memory_bytes {
        Some(bytes) => Some(
            usize::try_from(bytes)
                .map_err(|_| SandboxError::Invalid("memory_bytes is too large".to_owned()))?,
        ),
        None => None,
    };
    let cpu_100ns = match p.limits.cpu_seconds {
        Some(seconds) => Some(
            i64::try_from(seconds)
                .ok()
                .and_then(|s| s.checked_mul(10_000_000))
                .ok_or_else(|| SandboxError::Invalid("cpu_seconds is too large".to_owned()))?,
        ),
        None => None,
    };

    let sid = sid_of(&p.workdir)?;
    if let Network::LoopbackPort(port) = p.network
        && !loopback::exempt(sid.0)?
    {
        return Err(SandboxError::Unsupported(format!(
            "LoopbackPort({port}): the container is not exempt from loopback, and only an \
             administrator can exempt it: the hub asks the owner once, or run `apid windows-loopback \
             grant` from an elevated prompt"
        )));
    }
    grant(&p.workdir, sid.0, MODIFY, Need::Yes)?;
    for path in &p.read_write {
        grant(path, sid.0, MODIFY, Need::Yes)?;
    }
    for path in &p.read_only {
        grant(path, sid.0, READ_EXECUTE, Need::IfPossible)?;
    }
    grant(&p.program_real, sid.0, READ_EXECUTE, Need::IfPossible)?;

    // Each pipe is (read end, write end). The child gets the end it uses; the parent
    // keeps the other, which must not be inherited.
    let (stdin_read, stdin_write) = pipe()?;
    let (stdout_read, stdout_write) = pipe()?;
    let (stderr_read, stderr_write) = pipe()?;
    for parent_end in [&stdin_write, &stdout_read, &stderr_read] {
        keep_private(parent_end)?;
    }
    let child_handles: [HANDLE; 3] = [
        stdin_read.as_raw_handle(),
        stdout_write.as_raw_handle(),
        stderr_write.as_raw_handle(),
    ];

    let job = job(memory, p.limits.processes, cpu_100ns)?;

    let mut attributes = AttributeList::new(2)?;
    let containment = SECURITY_CAPABILITIES {
        AppContainerSid: sid.0,
        Capabilities: null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    attributes.set(
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        (&containment as *const SECURITY_CAPABILITIES).cast(),
        size_of::<SECURITY_CAPABILITIES>(),
    )?;
    attributes.set(
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        child_handles.as_ptr().cast(),
        size_of_val(&child_handles),
    )?;

    let app = wide(p.program_real.as_os_str());
    let mut line = command_line(p.program.as_os_str(), &p.args);
    let env = env_block(&p.env);
    let cwd = wide(p.workdir.as_os_str());

    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = child_handles[0];
    startup.StartupInfo.hStdOutput = child_handles[1];
    startup.StartupInfo.hStdError = child_handles[2];
    startup.lpAttributeList = attributes.list;
    let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
    let flags = EXTENDED_STARTUPINFO_PRESENT
        | CREATE_UNICODE_ENVIRONMENT
        | CREATE_SUSPENDED
        | CREATE_NO_WINDOW;
    // SAFETY: every pointer is to a live, NUL-terminated buffer or an initialised struct
    // that outlives the call. The inheritable handles are the child ends listed in the
    // handle-list attribute, so nothing else is inherited.
    let created = unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            TRUE,
            flags,
            env.as_ptr().cast(),
            cwd.as_ptr(),
            (&startup as *const STARTUPINFOEXW).cast(),
            &mut info,
        )
    };
    if created == FALSE {
        // The names of the environment variables, never their values, so a failure says what it was given.
        let err = io::Error::last_os_error();
        let names: Vec<&str> = p.env.iter().map(|(key, _)| key.as_str()).collect();
        return Err(SandboxError::Io(io::Error::new(
            err.kind(),
            format!(
                "CreateProcessW of {} in {} with environment variables [{}]: {err}",
                p.program_real.display(),
                p.workdir.display(),
                names.join(", ")
            ),
        )));
    }
    // SAFETY: CreateProcessW succeeded, so these two handles are valid and ours to own.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
    // The child has its copies now; the parent must close its own, or reads never see EOF.
    drop((stdin_read, stdout_write, stderr_write));

    // The process is still suspended, so nothing it does can escape the job: join the
    // job first, then resume. ResumeThread returns (DWORD)-1 on failure.
    let assigned =
        unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) };
    if assigned == FALSE {
        let err = os_error("AssignProcessToJobObject");
        terminate(&process);
        return Err(err);
    }
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        let err = os_error("ResumeThread");
        terminate(&process);
        return Err(err);
    }
    drop(thread);

    Ok(Process {
        process,
        job,
        pid: info.dwProcessId,
        stdin: Some(File::from(stdin_write)),
        stdout: Some(File::from(stdout_read)),
        stderr: Some(File::from(stderr_read)),
    })
}

/// A confined child on Windows: the process, its job, and the parent's pipe ends.
pub(crate) struct Process {
    process: OwnedHandle,
    job: OwnedHandle,
    pid: u32,
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

impl Process {
    pub(crate) fn id(&self) -> u32 {
        self.pid
    }

    pub(crate) fn stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.stdin
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Write + Send>)
    }

    pub(crate) fn stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>)
    }

    pub(crate) fn stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>)
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.wait_for(INFINITE)? {
                return Ok(status);
            }
        }
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_for(0)
    }

    /// Terminates every process in the job, so the child's own children go too.
    pub(crate) fn kill(&mut self) -> io::Result<()> {
        // SAFETY: the job handle is valid for the life of `self`.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wait_for(&self, millis: u32) -> io::Result<Option<ExitStatus>> {
        // SAFETY: the process handle is valid for the life of `self`.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), millis) } {
            WAIT_OBJECT_0 => {
                let mut code: u32 = 0;
                // SAFETY: `code` is a valid out pointer for the call.
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == FALSE {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

/// The AppContainer's SID, freed on drop.
pub(crate) struct OwnedSid(pub(crate) PSID);

impl Drop for OwnedSid {
    fn drop(&mut self) {
        // SAFETY: the SID came from CreateAppContainerProfile or Derive..., which
        // allocate it for the caller to free with FreeSid.
        unsafe { FreeSid(self.0) };
    }
}

/// A `LocalAlloc` block from the Win32 security API (a descriptor, an ACL, a SID or a
/// list), freed on drop.
pub(crate) struct Local(pub(crate) HLOCAL);

impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: the block was allocated by the security API and is freed exactly once.
        unsafe { LocalFree(self.0) };
    }
}

/// A PROC_THREAD_ATTRIBUTE_LIST, deleted on drop. `storage` keeps the list's memory
/// alive, and it is usize-aligned because the list needs pointer alignment.
struct AttributeList {
    _storage: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl AttributeList {
    fn new(slots: u32) -> Result<Self, SandboxError> {
        let mut bytes: usize = 0;
        // SAFETY: a null list asks only for the size. The call fails by design.
        unsafe { InitializeProcThreadAttributeList(null_mut(), slots, 0, &mut bytes) };
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list: LPPROC_THREAD_ATTRIBUTE_LIST = storage.as_mut_ptr().cast();
        // SAFETY: `list` points at `bytes` bytes of pointer-aligned storage that
        // `storage` keeps alive for as long as the list exists.
        if unsafe { InitializeProcThreadAttributeList(list, slots, 0, &mut bytes) } == FALSE {
            return Err(os_error("InitializeProcThreadAttributeList"));
        }
        Ok(Self {
            _storage: storage,
            list,
        })
    }

    /// Sets one attribute. The value is not copied: it must stay valid until
    /// CreateProcessW returns.
    fn set(
        &mut self,
        attribute: u32,
        value: *const c_void,
        size: usize,
    ) -> Result<(), SandboxError> {
        // SAFETY: `value` points at `size` readable bytes that the caller keeps alive
        // through CreateProcessW. No previous-value buffer is requested.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.list,
                0,
                attribute as usize,
                value,
                size,
                null_mut(),
                null(),
            )
        };
        if ok == FALSE {
            return Err(os_error("UpdateProcThreadAttribute"));
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: the list was initialised in `new` and is deleted once, while `_storage`
        // is still alive (fields drop after this method returns).
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

fn container_name(workdir: &Path) -> String {
    // FNV-1a over the path's UTF-16 bytes: one workdir always maps to one container.
    // With the prefix the name is 24 characters, well under the 64 allowed.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for unit in workdir.as_os_str().encode_wide() {
        for byte in unit.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{CONTAINER_PREFIX}{hash:016x}")
}

/// The SID of the container of `workdir` (a canonical path), created on first use.
pub(crate) fn sid_of(workdir: &Path) -> Result<OwnedSid, SandboxError> {
    container_sid(&container_name(workdir))
}

fn container_sid(name: &str) -> Result<OwnedSid, SandboxError> {
    let wide_name = wide(name);
    let mut sid: PSID = null_mut();
    // SAFETY: the name strings are NUL-terminated and live through the call. A null
    // capability list with count 0 requests no capabilities.
    let hr = unsafe {
        CreateAppContainerProfile(
            wide_name.as_ptr(),
            wide_name.as_ptr(),
            wide_name.as_ptr(),
            null(),
            0,
            &mut sid,
        )
    };
    if hr >= 0 {
        return Ok(OwnedSid(sid));
    }
    if hr != PROFILE_EXISTS {
        return Err(hresult_error("CreateAppContainerProfile", hr));
    }
    // SAFETY: as above. The profile exists, so its SID is derived from the name.
    let hr = unsafe { DeriveAppContainerSidFromAppContainerName(wide_name.as_ptr(), &mut sid) };
    if hr < 0 {
        return Err(hresult_error(
            "DeriveAppContainerSidFromAppContainerName",
            hr,
        ));
    }
    Ok(OwnedSid(sid))
}

/// Whether a grant must succeed. System locations (Program Files, Windows) are already
/// readable by every AppContainer and their DACLs cannot be changed by a standard user,
/// so a read-only grant there may be refused: the container then has no more access
/// than it already had.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    Yes,
    IfPossible,
}

const ERROR_ACCESS_DENIED: u32 = 5;

/// Adds an ACE for `sid` to `path`'s DACL, keeping the existing entries.
fn grant(path: &Path, sid: PSID, access: u32, need: Need) -> Result<(), SandboxError> {
    match grant_ace(path, sid, access) {
        Err(code) if code == ERROR_ACCESS_DENIED && need == Need::IfPossible => Ok(()),
        Err(code) => Err(win32_error("granting the AppContainer access", code)),
        Ok(()) => Ok(()),
    }
}

/// The ACE itself. Errors are Win32 codes, so the caller can decide what they mean.
fn grant_ace(path: &Path, sid: PSID, access: u32) -> Result<(), u32> {
    let object = wide(path.as_os_str());
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `object` is NUL-terminated. The out pointers are valid for writes. The
    // descriptor that `dacl` points into is freed below.
    let rc = unsafe {
        GetNamedSecurityInfoW(
            object.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if rc != 0 {
        return Err(rc);
    }
    let _descriptor = Local(descriptor);

    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.cast(),
        },
    };
    let mut merged: *mut ACL = null_mut();
    // SAFETY: `entry` and the old DACL are valid for the call. `merged` receives a new
    // block that `Local` frees.
    let rc = unsafe { SetEntriesInAclW(1, &entry, dacl, &mut merged) };
    if rc != 0 {
        return Err(rc);
    }
    let _merged = Local(merged.cast());

    // SAFETY: `object` is NUL-terminated and `merged` is a valid DACL for the call.
    let rc = unsafe {
        SetNamedSecurityInfoW(
            object.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            merged,
            null_mut(),
        )
    };
    if rc != 0 {
        return Err(rc);
    }
    Ok(())
}

/// An anonymous pipe as (read end, write end). Both ends start inheritable.
fn pipe() -> Result<(OwnedHandle, OwnedHandle), SandboxError> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: TRUE,
    };
    let mut read: HANDLE = null_mut();
    let mut write: HANDLE = null_mut();
    // SAFETY: both out pointers are valid, and `attributes` lives through the call.
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == FALSE {
        return Err(os_error("CreatePipe"));
    }
    // SAFETY: CreatePipe succeeded, so both handles are valid and ours to own.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    })
}

/// Stops a parent-side pipe end from being inherited by the child.
fn keep_private(handle: &OwnedHandle) -> Result<(), SandboxError> {
    // SAFETY: the handle is valid for the duration of the call.
    let ok = unsafe { SetHandleInformation(handle.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) };
    if ok == FALSE {
        return Err(os_error("SetHandleInformation"));
    }
    Ok(())
}

/// A job that kills its processes when the last handle closes, with the caller's caps.
fn job(
    memory: Option<usize>,
    processes: Option<u32>,
    cpu_100ns: Option<i64>,
) -> Result<OwnedHandle, SandboxError> {
    // SAFETY: null attributes and a null name are both allowed.
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    if raw.is_null() {
        return Err(os_error("CreateJobObjectW"));
    }
    // SAFETY: CreateJobObjectW returned a fresh handle that nothing else owns.
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };

    // SAFETY: the struct is plain data, and all-zero is a valid value for it.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    let mut flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if let Some(bytes) = memory {
        info.ProcessMemoryLimit = bytes;
        flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    }
    if let Some(count) = processes {
        info.BasicLimitInformation.ActiveProcessLimit = count;
        flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    }
    if let Some(time) = cpu_100ns {
        info.BasicLimitInformation.PerProcessUserTimeLimit = time;
        flags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
    }
    info.BasicLimitInformation.LimitFlags = flags;
    // SAFETY: `info` is initialised and lives through the call; its size is passed.
    let ok = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == FALSE {
        return Err(os_error("SetInformationJobObject"));
    }
    Ok(job)
}

/// Terminates a process that is already created but cannot be used, ignoring errors.
fn terminate(process: &OwnedHandle) {
    // SAFETY: the handle is valid. Failure here cannot be reported any further.
    unsafe { TerminateProcess(process.as_raw_handle(), 1) };
}

/// The command line: the program as argv[0], then each argument, quoted the way the
/// MSVC runtime parses them.
fn command_line(program: &OsStr, args: &[String]) -> Vec<u16> {
    let mut line = Vec::new();
    let program: Vec<u16> = program.encode_wide().collect();
    push_arg(&mut line, &program);
    for arg in args {
        line.push(SPACE);
        let units: Vec<u16> = arg.encode_utf16().collect();
        push_arg(&mut line, &units);
    }
    line.push(0);
    line
}

fn push_arg(line: &mut Vec<u16>, arg: &[u16]) {
    let quote = arg.is_empty() || arg.iter().any(|&c| c == SPACE || c == TAB || c == QUOTE);
    if !quote {
        line.extend_from_slice(arg);
        return;
    }
    line.push(QUOTE);
    let mut backslashes = 0usize;
    for &c in arg {
        if c == BACKSLASH {
            backslashes += 1;
            continue;
        }
        if c == QUOTE {
            // Backslashes before a quote are doubled, and the quote itself escaped.
            line.extend(repeat_n(BACKSLASH, backslashes * 2 + 1));
        } else {
            line.extend(repeat_n(BACKSLASH, backslashes));
        }
        backslashes = 0;
        line.push(c);
    }
    // Backslashes just before the closing quote are doubled.
    line.extend(repeat_n(BACKSLASH, backslashes * 2));
    line.push(QUOTE);
}

/// The environment block: `NAME=value` entries sorted by name, each NUL-terminated,
/// then an extra NUL. An empty environment is just two NULs.
fn env_block(env: &[(String, String)]) -> Vec<u16> {
    let mut sorted: Vec<&(String, String)> = env.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut block = Vec::new();
    for (key, value) in sorted {
        block.extend(format!("{key}={value}").encode_utf16());
        block.push(0);
    }
    // The block ends with an extra NUL, so an empty block needs one more before it.
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// A NUL-terminated UTF-16 copy of `text`.
pub(crate) fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// The error for the last Win32 call on this thread, with the call's name.
pub(crate) fn os_error(call: &str) -> SandboxError {
    let err = io::Error::last_os_error();
    SandboxError::Io(io::Error::new(err.kind(), format!("{call}: {err}")))
}

/// The error for a call that returns a Win32 error code directly.
pub(crate) fn win32_error(call: &str, code: u32) -> SandboxError {
    let err = io::Error::from_raw_os_error(code as i32);
    SandboxError::Io(io::Error::new(err.kind(), format!("{call}: {err}")))
}

/// The error for a call that returns an HRESULT.
fn hresult_error(call: &str, hr: i32) -> SandboxError {
    SandboxError::Io(io::Error::other(format!(
        "{call} failed: HRESULT 0x{:08x}",
        hr as u32
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_block_ends_with_two_nuls_even_when_empty() {
        assert_eq!(env_block(&[]), [0, 0]);
        let env = [
            ("B".to_owned(), "2".to_owned()),
            ("A".to_owned(), "1".to_owned()),
        ];
        let expected: Vec<u16> = "A=1\0B=2\0\0".encode_utf16().collect();
        assert_eq!(env_block(&env), expected);
    }
}
