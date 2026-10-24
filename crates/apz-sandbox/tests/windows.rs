//! End-to-end checks of the AppContainer backend: a confined child starts, runs and reports its exit code.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use apz_sandbox::{Limits, Network, Spec, spawn};

/// A fresh directory under the temp folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("apz-sbx-win-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn system_root() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
}

/// `cmd.exe /C exit 7` in the container, with `env`.
fn exit_seven(workdir: &Path, env: Vec<(String, String)>) -> Result<i32, String> {
    let spec = Spec {
        program: Path::new(&system_root()).join("System32").join("cmd.exe"),
        args: vec!["/C".into(), "exit 7".into()],
        env,
        workdir: workdir.to_path_buf(),
        read_only: vec![],
        read_write: vec![],
        network: Network::None,
        limits: Limits::default(),
    };
    let mut child =
        spawn(&spec, Stdio::null(), Stdio::null(), Stdio::null()).map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    status.code().ok_or_else(|| "no exit code".to_string())
}

#[test]
fn a_child_with_a_users_environment_starts_and_reports_its_exit_code() {
    let dir = TempDir::new("profile");
    let home = dir.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let root = system_root();
    let env = vec![
        ("PATH".to_owned(), format!(r"{root}\System32;{root}")),
        ("SystemRoot".to_owned(), root.clone()),
        ("windir".to_owned(), root),
        ("SystemDrive".to_owned(), "C:".to_owned()),
        ("USERPROFILE".to_owned(), home.display().to_string()),
        (
            "APPDATA".to_owned(),
            home.join(r"AppData\Roaming").display().to_string(),
        ),
        (
            "LOCALAPPDATA".to_owned(),
            home.join(r"AppData\Local").display().to_string(),
        ),
        ("TEMP".to_owned(), dir.0.display().to_string()),
        ("TMP".to_owned(), dir.0.display().to_string()),
    ];
    assert_eq!(exit_seven(&dir.0, env), Ok(7));
}
