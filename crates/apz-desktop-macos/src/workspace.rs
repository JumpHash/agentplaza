//! Applications: the installed bundles, read from their folders and `Info.plist`, and the running ones, from
//! `NSWorkspace`.

use std::path::{Path, PathBuf};

use apz_desktop::AppInfo;
use core_foundation::base::CFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::bundle::CFBundle;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use objc2_app_kit::NSWorkspace;

use crate::apps;

/// An app that is running now.
pub struct RunningApp {
    pub name: Option<String>,
    pub pid: i32,
    pub bundle_path: Option<String>,
}

/// The apps that are running, as the workspace reports them (background helpers included).
pub fn running_apps() -> Vec<RunningApp> {
    let workspace = NSWorkspace::sharedWorkspace();
    let running = workspace.runningApplications();
    running
        .iter()
        .map(|app| RunningApp {
            name: app.localizedName().map(|name| name.to_string()),
            pid: app.processIdentifier(),
            bundle_path: app
                .bundleURL()
                .and_then(|url| url.path())
                .map(|path| path.to_string()),
        })
        .collect()
}

/// The process id of the running app called `name`. An app matches by its name, or by its bundle's file name.
pub fn pid_of(name: &str) -> Option<i32> {
    running_apps()
        .into_iter()
        .find(|app| {
            app.name.as_deref() == Some(name)
                || app
                    .bundle_path
                    .as_deref()
                    .and_then(file_stem)
                    .is_some_and(|stem| stem == name)
        })
        .map(|app| app.pid)
}

fn file_stem(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

/// The installed apps, from the usual application folders, sorted by name.
pub fn installed_apps() -> Vec<AppInfo> {
    let found = app_dirs()
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "app") && path.is_dir())
        .map(|path| bundle_info(&path))
        .collect();
    apps::sort_and_dedupe(found)
}

fn app_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
        "/System/Applications/Utilities",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications"));
    }
    dirs
}

/// The app a bundle describes. A folder without a readable `Info.plist` still lists, under its file name.
fn bundle_info(path: &Path) -> AppInfo {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let info = CFURL::from_path(path, true)
        .and_then(CFBundle::new)
        .map(|bundle| bundle.info_dictionary());
    let text = |key: &str| info.as_ref().and_then(|info| info_text(info, key));
    let applescript = info
        .as_ref()
        .and_then(|info| info_flag(info, "NSAppleScriptEnabled"))
        .unwrap_or(false);
    AppInfo {
        id: apps::app_id(text("CFBundleIdentifier").as_deref(), &stem),
        name: apps::app_name(
            text("CFBundleDisplayName").as_deref(),
            text("CFBundleName").as_deref(),
            &stem,
        ),
        path: path.display().to_string(),
        scripting: apps::scripting_bridges(applescript, has_scripting_dictionary(path)),
    }
}

fn info_text(info: &CFDictionary<CFString, CFType>, key: &str) -> Option<String> {
    info.find(CFString::new(key))?
        .downcast::<CFString>()
        .map(|text| text.to_string())
}

fn info_flag(info: &CFDictionary<CFString, CFType>, key: &str) -> Option<bool> {
    info.find(CFString::new(key))?
        .downcast::<CFBoolean>()
        .map(bool::from)
}

/// Whether the bundle publishes a scripting dictionary (an `.sdef` file in its resources).
fn has_scripting_dictionary(bundle: &Path) -> bool {
    match std::fs::read_dir(bundle.join("Contents/Resources")) {
        Ok(entries) => entries
            .flatten()
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "sdef")),
        Err(_) => false,
    }
}
