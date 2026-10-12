//! The `agentplaza` command on PATH, for the two installs that do not put it there.
//!
//! The Linux packages and the Windows MSI already put `agentplaza` on PATH. Two installs do not:
//! - **macOS app** (dragged from the DMG): the CLI sits next to the desktop executable, inside the
//!   bundle. It is linked into `/usr/local/bin`, which needs an administrator password.
//! - **Linux AppImage**: the AppImage runs the CLI when it is invoked through a link named
//!   `agentplaza`, so the link goes into `$HOME/.local/bin` and needs no privileges.
//!
//! [`status`] says which case applies to this run, and [`link`] makes the link.

use std::path::{Path, PathBuf};

/// Where the `agentplaza` command stands for this install.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CliLink {
    /// The command already resolves (a package put it there, or the link exists).
    OnPath,
    /// `link` should become a link to `target`; the user can ask for it.
    Linkable { target: PathBuf, link: PathBuf },
    /// There is no CLI next to this app to link, or no home to link it into.
    #[default]
    Unavailable,
}

/// The state of the `agentplaza` command for this install.
pub fn status() -> CliLink {
    status_os()
}

/// Makes `link` a link to `target`. On macOS this asks for an administrator password; a dismissed
/// prompt is the error "cancelled".
pub fn link(target: &Path, link: &Path) -> Result<(), String> {
    link_os(target, link)
}

#[cfg(target_os = "macos")]
fn status_os() -> CliLink {
    /// Where macOS users get the command.
    const LINK: &str = "/usr/local/bin/agentplaza";
    let Ok(exe) = std::env::current_exe() else {
        return CliLink::Unavailable;
    };
    let target = exe.with_file_name("agentplaza");
    if !target.is_file() {
        return CliLink::Unavailable;
    }
    let link = PathBuf::from(LINK);
    let same = matches!(
        (std::fs::canonicalize(&link), std::fs::canonicalize(&target)),
        (Ok(found), Ok(wanted)) if found == wanted
    );
    if same {
        CliLink::OnPath
    } else {
        CliLink::Linkable { target, link }
    }
}

#[cfg(target_os = "linux")]
fn status_os() -> CliLink {
    // Without an AppImage, a package put the command on PATH.
    let Some(appimage) = std::env::var_os("APPIMAGE") else {
        return CliLink::OnPath;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return CliLink::Unavailable;
    };
    let target = PathBuf::from(appimage);
    let link = PathBuf::from(home).join(".local/bin/agentplaza");
    if std::fs::read_link(&link).is_ok_and(|found| found == target) {
        CliLink::OnPath
    } else {
        CliLink::Linkable { target, link }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn status_os() -> CliLink {
    CliLink::OnPath
}

#[cfg(target_os = "macos")]
fn link_os(target: &Path, link: &Path) -> Result<(), String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(applescript_command(target, link))
        .output()
        .map_err(|e| format!("could not run osascript: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // osascript reports a dismissed password prompt as error -128.
    if stderr.contains("-128") {
        Err("cancelled".into())
    } else if stderr.is_empty() {
        Err("the command could not be linked".into())
    } else {
        Err(stderr)
    }
}

#[cfg(target_os = "linux")]
fn link_os(target: &Path, link: &Path) -> Result<(), String> {
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if std::fs::symlink_metadata(link).is_ok() {
        std::fs::remove_file(link).map_err(|e| e.to_string())?;
    }
    std::os::unix::fs::symlink(target, link).map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn link_os(_target: &Path, _link: &Path) -> Result<(), String> {
    Ok(())
}

/// The `osascript -e` program that links `target` at `link` as root. The paths are quoted for the
/// shell, and the shell command is escaped for AppleScript's double-quoted string.
#[cfg(any(target_os = "macos", test))]
fn applescript_command(target: &Path, link: &Path) -> String {
    let shell = format!("mkdir -p /usr/local/bin && ln -sf {} {}", shell_quote(target), shell_quote(link));
    let escaped = shell.replace('\\', "\\\\").replace('"', "\\\"");
    format!("do shell script \"{escaped}\" with administrator privileges")
}

/// One path as a single shell word: single quotes, with each `'` closed, escaped and reopened.
#[cfg(any(target_os = "macos", test))]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_quotes_a_path_with_a_space_and_a_quote() {
        let command = applescript_command(Path::new("/Applications/Agent Plaza/it's/agentplaza"), Path::new("/usr/local/bin/agentplaza"));
        assert_eq!(
            command,
            r#"do shell script "mkdir -p /usr/local/bin && ln -sf '/Applications/Agent Plaza/it'\\''s/agentplaza' '/usr/local/bin/agentplaza'" with administrator privileges"#
        );
    }

    #[test]
    fn applescript_escapes_double_quotes_and_backslashes() {
        let command = applescript_command(Path::new(r#"/x/"a"\b"#), Path::new("/usr/local/bin/agentplaza"));
        assert!(command.contains(r#"'/x/\"a\"\\b'"#), "{command}");
    }
}
