//! The app catalogue: the targets of Start-menu shortcuts and the App Paths registrations, and launching through
//! ShellExecuteW.

use std::path::{Path, PathBuf};

use apz_desktop::{AppInfo, DesktopError, Result};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
};
use windows::Win32::System::Registry::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use windows::Win32::UI::Shell::{IShellLinkW, ShellExecuteW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, Interface, PCWSTR, w};

use super::registry;
use crate::catalog;

const START_MENU: &str = r"Microsoft\Windows\Start Menu\Programs";
const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";
/// How deep the Start menu's folders are followed.
const MAX_FOLDER_DEPTH: u32 = 4;

/// Every app the Start menu and the App Paths registrations name, by name, without duplicates.
pub fn list() -> Vec<AppInfo> {
    let mut apps: Vec<AppInfo> = Vec::new();
    for dir in start_menu_dirs() {
        for shortcut in shortcuts_in(&dir, 0) {
            let Some(target) = shortcut_target(&shortcut) else {
                continue;
            };
            let Some(name) = shortcut.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            add(&mut apps, name, target);
        }
    }
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for exe in registry::subkeys(root, APP_PATHS) {
            let key = format!(r"{APP_PATHS}\{exe}");
            if let Some(target) = registry::string_value(root, &key, "") {
                let name = exe.strip_suffix(".exe").unwrap_or(&exe).to_string();
                add(&mut apps, &name, target);
            }
        }
    }
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

/// Adds an app whose target is an executable, unless one with the same executable is listed already.
fn add(apps: &mut Vec<AppInfo>, name: &str, target: String) {
    let id = catalog::file_name(&target).to_string();
    if !id.to_ascii_lowercase().ends_with(".exe") || name.to_ascii_lowercase().contains("uninstall")
    {
        return;
    }
    if apps.iter().any(|app| app.id.eq_ignore_ascii_case(&id)) {
        return;
    }
    let com = catalog::progid_for_exe(&id)
        .is_some_and(|progid| registry::key_exists(HKEY_CLASSES_ROOT, progid));
    apps.push(AppInfo {
        id,
        name: name.to_string(),
        path: target,
        scripting: catalog::bridges(com),
    });
}

/// Starts an executable, with its arguments already quoted into one command line. `path` is an executable the
/// catalogue listed.
pub fn launch(path: &str, params: &str) -> Result<()> {
    // SAFETY: the strings are copied for the call and the window handle is null (no owner window).
    let code = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(path),
            &HSTRING::from(params),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value above 32 on success and an error code at or below it.
    let code = code.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(DesktopError::Platform(format!(
            "ShellExecuteW could not start {path} (code {code})"
        )))
    }
}

fn start_menu_dirs() -> Vec<PathBuf> {
    ["ProgramData", "APPDATA"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|base| PathBuf::from(base).join(START_MENU))
        .collect()
}

/// The shortcut files under `dir`, following folders to `MAX_FOLDER_DEPTH`.
fn shortcuts_in(dir: &Path, depth: u32) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_FOLDER_DEPTH {
                found.extend(shortcuts_in(&path, depth + 1));
            }
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
        {
            found.push(path);
        }
    }
    found
}

/// The executable a shortcut starts, from the shell link; `None` for a shortcut that cannot be read.
fn shortcut_target(shortcut: &Path) -> Option<String> {
    let path = shortcut.to_str()?;
    // SAFETY: CoCreateInstance builds the shell link object on this thread, whose COM is initialised by the
    // backend; the file is loaded and the path read into a buffer of known size.
    let link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let file: IPersistFile = link.cast().ok()?;
    unsafe { file.Load(&HSTRING::from(path), STGM_READ) }.ok()?;
    let mut buffer = [0u16; 512];
    unsafe { link.GetPath(&mut buffer, std::ptr::null_mut(), 0) }.ok()?;
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    let target = String::from_utf16_lossy(&buffer[..end]);
    (!target.is_empty()).then_some(target)
}
