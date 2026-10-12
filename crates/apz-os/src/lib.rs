//! What differs between the platforms AgentPlaza runs on (Linux on x86_64 and arm64 — Ubuntu,
//! Fedora, Arch, Raspberry Pi OS —, macOS, Windows on x64 and arm64), in one place, so the rest of
//! the code base has no `cfg(unix)` of its own:
//!
//! - [`dirs`]: where the plaza keeps its state, data and configuration.
//! - [`private`]: files and directories only the current user may read (keys, tokens, secrets).
//! - [`random`]: bytes from the operating system's CSPRNG.
//! - [`process`]: starting a service that outlives the app that started it.

pub mod dirs {
    //! The plaza's directories:
    //!
    //! | | Linux | macOS | Windows |
    //! |---|---|---|---|
    //! | state | `$XDG_STATE_HOME/agentplaza` (`~/.local/state/…`) | `~/Library/Application Support/AgentPlaza/state` | `%LOCALAPPDATA%\AgentPlaza\state` |
    //! | data | `$XDG_DATA_HOME/agentplaza` (`~/.local/share/…`) | `~/Library/Application Support/AgentPlaza/data` | `%LOCALAPPDATA%\AgentPlaza\data` |
    //! | config | `$XDG_CONFIG_HOME/agentplaza` (`~/.config/…`) | `~/Library/Application Support/AgentPlaza/config` | `%LOCALAPPDATA%\AgentPlaza\config` |
    //!
    //! Everything sits under the user's own profile: on Windows that profile's inherited ACL is what
    //! keeps the plaza's secrets private (see [`crate::private`]).

    use std::path::PathBuf;

    /// The user's home directory (`HOME`, or `USERPROFILE` on Windows).
    pub fn home() -> Option<PathBuf> {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
    }

    /// Where the plaza keeps its state (the hub's records, node identities, tokens).
    pub fn state() -> PathBuf {
        place("XDG_STATE_HOME", ".local/state", "state")
    }

    /// Where the plaza keeps bulky data (guest images, the library's files, logs).
    pub fn data() -> PathBuf {
        place("XDG_DATA_HOME", ".local/share", "data")
    }

    /// Where the apps keep their settings (the desktop app's connections).
    pub fn config() -> PathBuf {
        place("XDG_CONFIG_HOME", ".config", "config")
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn place(xdg: &str, fallback: &str, _kind: &str) -> PathBuf {
        let base = std::env::var_os(xdg)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home().unwrap_or_else(|| PathBuf::from("/")).join(fallback));
        base.join("agentplaza")
    }

    #[cfg(target_os = "macos")]
    fn place(_xdg: &str, _fallback: &str, kind: &str) -> PathBuf {
        home()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join("Library/Application Support/AgentPlaza")
            .join(kind)
    }

    #[cfg(windows)]
    fn place(_xdg: &str, _fallback: &str, kind: &str) -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join("AppData").join("Local")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("AgentPlaza").join(kind)
    }
}

pub mod private {
    //! Files only the current user may read.
    //!
    //! On Unix a private file is mode 0600 and a private directory 0700, set when it is created
    //! (never a window where it is readable by others) and again after, in case it existed.
    //!
    //! On Windows there are no mode bits: a file created under the user's profile
    //! (`%LOCALAPPDATA%`, see [`crate::dirs`]) inherits the profile's ACL, which grants the user,
    //! SYSTEM and Administrators only. [`check_dir`] refuses a secrets directory outside the
    //! profile there, so a key is never written somewhere other users can read.

    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    /// Creates `dir` and its parents; on Unix the directory itself is 0700. An existing directory
    /// is accepted (and narrowed to 0700 on Unix).
    pub fn create_dir(dir: &Path) -> io::Result<()> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Writes `bytes` to `path` atomically and privately: a fresh `<name>.tmp.<pid>` beside it
    /// (0600 on Unix, created exclusively), synced, then renamed over `path`.
    pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let tmp = tmp_beside(path);
        let _ = fs::remove_file(&tmp);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let result = (|| {
            let mut file = options.open(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            replace(&tmp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Opens `path` for appending, creating it private (0600 on Unix) if it does not exist.
    pub fn append(path: &Path) -> io::Result<fs::File> {
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(path)
    }

    /// Whether others could read files in `dir`: on Unix, any group/other permission bit on the
    /// directory; on Windows, `dir` lying outside the user's profile. `Err` names the problem.
    pub fn check_dir(dir: &Path) -> Result<(), String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?.permissions().mode();
            if mode & 0o077 != 0 {
                return Err(format!("{} is readable by other users (mode {:o}); it must be 0700", dir.display(), mode & 0o777));
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let profile = crate::dirs::home().ok_or("USERPROFILE is not set")?;
            let dir = fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let profile = fs::canonicalize(&profile).unwrap_or(profile);
            if dir.starts_with(&profile) {
                Ok(())
            } else {
                Err(format!("{} is outside your profile ({}); secrets must live under it", dir.display(), profile.display()))
            }
        }
    }

    fn tmp_beside(path: &Path) -> PathBuf {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        path.with_file_name(format!(".{name}.tmp.{}", std::process::id()))
    }

    /// `rename` that replaces an existing target on every platform (Windows' rename does too
    /// through std, which uses `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`).
    fn replace(from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
}

pub mod random {
    //! Bytes from the operating system's CSPRNG (`getrandom(2)`, `getentropy`, `ProcessPrng`).

    /// Fills `buf` with random bytes.
    pub fn fill(buf: &mut [u8]) -> std::io::Result<()> {
        getrandom::fill(buf).map_err(|e| std::io::Error::other(format!("the OS random source failed: {e}")))
    }

    /// `n` random bytes as lowercase hex (`2n` characters): tokens and nonces.
    pub fn hex(n: usize) -> std::io::Result<String> {
        let mut bytes = vec![0u8; n];
        fill(&mut bytes)?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
}

pub mod process {
    //! Services the desktop app starts (the hub, a node) and that keep running after it exits.

    use std::process::Command;

    /// Detaches `cmd` from the starting process's terminal and signals: its own process group on
    /// Unix; a new process group without a console window on Windows.
    pub fn detach(cmd: &mut Command) -> &mut Command {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        cmd
    }

    /// The executable name for `name` on this platform (`name.exe` on Windows).
    pub fn exe(name: &str) -> String {
        if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-os-{tag}-{}-{}", std::process::id(), random::hex(4).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn private_write_replaces_atomically() {
        let dir = scratch("write");
        private::create_dir(&dir).unwrap();
        let file = dir.join("key");
        private::write(&file, b"one").unwrap();
        private::write(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name() != "key").collect();
        assert!(leftovers.is_empty(), "no temporary file is left behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            assert!(private::check_dir(&dir).is_ok());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_open_directory_is_not_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("open");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private::check_dir(&dir).is_err());
        private::create_dir(&dir).unwrap();
        assert!(private::check_dir(&dir).is_ok(), "create_dir narrows an existing directory");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn random_hex_is_fresh() {
        let (a, b) = (random::hex(16).unwrap(), random::hex(16).unwrap());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn dirs_are_distinct_and_named_for_the_plaza() {
        let (s, d, c) = (dirs::state(), dirs::data(), dirs::config());
        assert!(s != d && d != c && s != c);
        for p in [&s, &d, &c] {
            assert!(p.to_string_lossy().to_lowercase().contains("agentplaza"), "{}", p.display());
        }
    }
}
