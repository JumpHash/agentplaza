//! Top-level windows: the foreground one, the main window of a process by its executable, and bringing a window
//! to the front.

use apz_desktop::{DesktopError, Result};
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, BringWindowToTop, EnumWindows, GWL_EXSTYLE, GetForegroundWindow,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SW_RESTORE, SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
};
use windows::core::PWSTR;

use crate::catalog;

/// The foreground window. Fails when there is none (the desktop is locked, or no window has focus).
pub fn foreground() -> Result<HWND> {
    // SAFETY: GetForegroundWindow takes no pointers.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        Err(DesktopError::NotFound("no foreground window".into()))
    } else {
        Ok(hwnd)
    }
}

/// The main window of an executable: a visible, titled, top-level window of a process whose executable is
/// `executable`. The foreground window wins among them, then the topmost one.
pub fn main_window_of(executable: &str) -> Result<HWND> {
    // SAFETY: GetForegroundWindow takes no pointers.
    let foreground = unsafe { GetForegroundWindow() };
    let mut first = None;
    for hwnd in top_windows() {
        if !is_main(hwnd) {
            continue;
        }
        let Some(pid) = pid_of(hwnd) else {
            continue;
        };
        let runs_exe = process_exe(pid).is_some_and(|file| catalog::same_exe(executable, &file));
        if !runs_exe {
            continue;
        }
        if hwnd == foreground {
            return Ok(hwnd);
        }
        first.get_or_insert(hwnd);
    }
    first.ok_or_else(|| DesktopError::NotFound(format!("no open window of {executable}")))
}

/// Brings a window to the front and gives it focus. Windows only lets the foreground process hand out
/// foreground rights, so this allows the window's process to take them and attaches to the current foreground
/// thread's input while it raises the window.
pub fn bring_to_front(hwnd: HWND) -> Result<()> {
    let pid = pid_of(hwnd);
    // SAFETY: each call takes handles and integers this function holds, on this thread.
    unsafe {
        if let Some(pid) = pid {
            let _ = AllowSetForegroundWindow(pid);
        }
        let me = GetCurrentThreadId();
        let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let attached = foreground_thread != 0
            && foreground_thread != me
            && AttachThreadInput(me, foreground_thread, true).as_bool();
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let raised = SetForegroundWindow(hwnd).as_bool();
        let _ = BringWindowToTop(hwnd);
        if attached {
            let _ = AttachThreadInput(me, foreground_thread, false);
        }
        if raised {
            Ok(())
        } else {
            Err(DesktopError::Platform(
                "Windows did not bring the window to the front".into(),
            ))
        }
    }
}

/// Every top-level window, topmost first.
fn top_windows() -> Vec<HWND> {
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: the callback only runs during this call, while `windows` is alive; it gets its address through
    // `lparam`.
    let _ = unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut windows as *mut Vec<HWND> as isize),
        )
    };
    windows
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the address of the Vec that `top_windows` passed in, and it outlives the enumeration.
    let windows = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    windows.push(hwnd);
    BOOL(1)
}

fn is_main(hwnd: HWND) -> bool {
    // SAFETY: these read window state for a handle EnumWindows gave us.
    unsafe {
        IsWindowVisible(hwnd).as_bool()
            && GetWindowTextLengthW(hwnd) > 0
            && (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW.0 == 0
    }
}

fn pid_of(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

/// The file name of a process's executable, or `None` when the process cannot be opened (a process of another
/// user, an elevated one).
fn process_exe(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed below in every path.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: `buffer` has `len` units.
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: `process` came from OpenProcess and is closed once.
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    let path = String::from_utf16_lossy(&buffer[..len as usize]);
    Some(catalog::file_name(&path).to_string())
}
