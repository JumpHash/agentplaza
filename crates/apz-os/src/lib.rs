//! What differs between the platforms AgentPlaza runs on (Linux on x86_64 and arm64 — Ubuntu,
//! Fedora, Arch, Raspberry Pi OS —, macOS, Windows on x64 and arm64), in one place, so the rest of
//! the code base has no `cfg(unix)` of its own:
//!
//! - [`dirs`]: where the plaza keeps its state, data and configuration.
//! - [`private`]: files and directories only the current user may read (keys, tokens, secrets).
//! - [`random`]: bytes from the operating system's CSPRNG.
//! - [`process`]: starting a service that outlives the app that started it.
//! - [`runner`]: starting a workspace binary from a test, under the target's runner when it has one.

/// The version of this installation: the `VERSION` file the packages ship in the resources
/// directory (`share/agentplaza/VERSION`, written by packaging/scripts/stage.sh), else the crate's
/// own version (a development build). Read at run time, not compiled in, so a release changes no
/// compiled code and CI's caches stay warm across releases.
pub fn version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        dirs::resources()
            .and_then(|share| std::fs::read_to_string(share.join("VERSION")).ok())
            .map(|text| text.trim().to_string())
            .filter(|v| {
                !v.is_empty()
                    && v.len() <= 64
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
            })
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
    })
}

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
        std::env::var_os(var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
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

    /// Where the node's per-VM vsock sockets live (`{dir}/vm{cid}` and `{dir}/vm{cid}_{port}`): `/tmp/plaza-vsock`
    /// on Linux and macOS (short, so socket paths stay under the 108-byte limit), the state directory's `vsock`
    /// on Windows (`%LOCALAPPDATA%\AgentPlaza\state\vsock`: the user profile's ACL keeps it private).
    pub fn vsock_sockets() -> PathBuf {
        if cfg!(windows) {
            state().join("vsock")
        } else {
            PathBuf::from("/tmp/plaza-vsock")
        }
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

    /// The installed resources (the web console, the roles, the prompts, the guest images), found
    /// beside the binary: `$APZ_SHARE` if set, else `<bin>/../share/agentplaza`, and on macOS also
    /// `<bin>/../Resources/share/agentplaza` (inside an app bundle). `None` when none is a directory.
    pub fn resources() -> Option<PathBuf> {
        let exe = std::env::current_exe().unwrap_or_default();
        resources_from(std::env::var_os("APZ_SHARE").as_deref(), &exe)
    }

    /// [`resources`] for a given `APZ_SHARE` value and binary path.
    pub fn resources_from(
        share_env: Option<&std::ffi::OsStr>,
        exe: &std::path::Path,
    ) -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = share_env
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .into_iter()
            .collect();
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("..").join("share").join("agentplaza"));
            if cfg!(target_os = "macos") {
                candidates.push(
                    dir.join("..")
                        .join("Resources")
                        .join("share")
                        .join("agentplaza"),
                );
            }
        }
        candidates.into_iter().find(|p| p.is_dir())
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
            let mode = fs::metadata(dir)
                .map_err(|e| format!("{}: {e}", dir.display()))?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                return Err(format!(
                    "{} is readable by other users (mode {:o}); it must be 0700",
                    dir.display(),
                    mode & 0o777
                ));
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
                Err(format!(
                    "{} is outside your profile ({}); secrets must live under it",
                    dir.display(),
                    profile.display()
                ))
            }
        }
    }

    fn tmp_beside(path: &Path) -> PathBuf {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        path.with_file_name(format!(".{name}.tmp.{}", std::process::id()))
    }

    /// `rename` that replaces an existing target on every platform (Windows' rename does too
    /// through std, which uses `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`).
    fn replace(from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
}

pub mod machine {
    //! The machine's own identifier: the same on every run of the machine, set by the OS and not by a user. The hub
    //! and a workstation daemon on the same machine read the same value, which is how the hub refuses a workstation
    //! that is its own machine.

    use std::sync::OnceLock;

    /// The machine's identifier, read once: `/etc/machine-id` on Linux, `IOPlatformUUID` on macOS, and the
    /// `MachineGuid` of the Cryptography key on Windows. `None` when the OS gives none.
    pub fn id() -> Option<String> {
        static ID: OnceLock<Option<String>> = OnceLock::new();
        ID.get_or_init(read).clone()
    }

    /// The `IOPlatformUUID` in `ioreg -rd1 -c IOPlatformExpertDevice` output: the line `"IOPlatformUUID" = "<id>"`.
    pub fn parse_ioreg(text: &str) -> Option<String> {
        text.lines()
            .find(|line| line.contains("\"IOPlatformUUID\""))
            .and_then(|line| line.split_once('='))
            .map(|(_, value)| value.trim().trim_matches('"').trim().to_string())
            .filter(|value| !value.is_empty())
    }

    #[cfg(target_os = "linux")]
    fn read() -> Option<String> {
        std::fs::read_to_string("/etc/machine-id")
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    }

    #[cfg(target_os = "macos")]
    fn read() -> Option<String> {
        let output = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .ok()?;
        parse_ioreg(&String::from_utf8_lossy(&output.stdout))
    }

    #[cfg(windows)]
    fn read() -> Option<String> {
        use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
        use windows::core::w;
        let key = w!("SOFTWARE\\Microsoft\\Cryptography");
        let value = w!("MachineGuid");
        let mut bytes = 0u32;
        // SAFETY: the key and value names are static, NUL-terminated strings; the first call only asks for the size.
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key,
                value,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut bytes),
            )
            .ok()
            .ok()?;
        }
        let mut units = vec![0u16; (bytes as usize).div_ceil(2).max(1)];
        let mut len = (units.len() * 2) as u32;
        // SAFETY: `units` holds `len` bytes, the size the registry is asked to fill.
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key,
                value,
                RRF_RT_REG_SZ,
                None,
                Some(units.as_mut_ptr().cast()),
                Some(&mut len),
            )
            .ok()
            .ok()?;
        }
        let text = String::from_utf16_lossy(&units[..(len as usize / 2).min(units.len())]);
        let text = text.trim_end_matches('\0').trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    fn read() -> Option<String> {
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_ioreg_uuid_is_read_from_its_line() {
            let text = "+-o Root  <class IORegistryEntry>\n    |   \"IOPlatformSerialNumber\" = \"C02X\"\n    |   \"IOPlatformUUID\" = \"4C2F1E0A-1234-5678-9ABC-DEF012345678\"\n";
            assert_eq!(
                parse_ioreg(text).as_deref(),
                Some("4C2F1E0A-1234-5678-9ABC-DEF012345678")
            );
            assert_eq!(parse_ioreg("nothing here"), None);
        }

        #[test]
        fn the_id_is_the_same_on_every_read() {
            assert_eq!(id(), id());
        }
    }
}

pub mod random {
    //! Bytes from the operating system's CSPRNG (`getrandom(2)`, `getentropy`, `ProcessPrng`).

    /// Fills `buf` with random bytes.
    pub fn fill(buf: &mut [u8]) -> std::io::Result<()> {
        getrandom::fill(buf)
            .map_err(|e| std::io::Error::other(format!("the OS random source failed: {e}")))
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
        if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        }
    }
}

pub mod runner {
    //! Starting a workspace binary from a test. Cargo starts a test binary through
    //! `CARGO_TARGET_<TRIPLE>_RUNNER` when its target needs one (the arm64 CI gate runs its tests
    //! under `qemu-aarch64 -L <sysroot>`), but a binary a test spawns is started by the kernel, so an
    //! arm64 child on an x86_64 host dies in the dynamic loader before it runs a line. Nothing here
    //! is set on a native run, and no product code calls it.

    use std::path::Path;
    use std::process::Command;

    /// The runner cargo configured for this build's architecture (`CARGO_TARGET_<TRIPLE>_RUNNER`,
    /// split on whitespace the way cargo splits it), or empty on a native run.
    pub fn runner() -> Vec<String> {
        runner_in(std::env::consts::ARCH, std::env::vars())
    }

    /// The runner among `vars` whose triple begins with `arch`. Matching the architecture keeps a
    /// runner for a different target, which cargo lets sit in the same environment, from applying.
    fn runner_in(arch: &str, mut vars: impl Iterator<Item = (String, String)>) -> Vec<String> {
        let arch = arch.to_ascii_uppercase();
        vars.find_map(|(name, value)| {
            let triple = name
                .strip_prefix("CARGO_TARGET_")?
                .strip_suffix("_RUNNER")?;
            triple.starts_with(&arch).then_some(value)
        })
        .map(|value| value.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
    }

    /// A command that runs `program` the way cargo ran this test binary: under [`runner`] when one
    /// is configured, directly otherwise.
    pub fn command(program: &Path) -> Command {
        let mut launch = runner();
        if launch.is_empty() {
            return Command::new(program);
        }
        let launcher = launch.remove(0);
        let mut command = Command::new(launcher);
        command.args(launch).arg(program);
        command
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_runner_applies_only_to_its_own_architecture() {
            let vars = || {
                [
                    (
                        "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER".to_string(),
                        "qemu-aarch64 -L /usr/aarch64-linux-gnu".to_string(),
                    ),
                    ("PATH".to_string(), "/usr/bin".to_string()),
                ]
                .into_iter()
            };
            assert_eq!(
                runner_in("aarch64", vars()),
                vec!["qemu-aarch64", "-L", "/usr/aarch64-linux-gnu"]
            );
            assert!(runner_in("x86_64", vars()).is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vsock_sockets_default() {
        let dir = dirs::vsock_sockets();
        if cfg!(windows) {
            assert!(dir.ends_with("vsock"), "{}", dir.display());
        } else {
            assert_eq!(dir, std::path::PathBuf::from("/tmp/plaza-vsock"));
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apz-os-{tag}-{}-{}",
            std::process::id(),
            random::hex(4).unwrap()
        ));
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
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name() != "key")
            .collect();
        assert!(leftovers.is_empty(), "no temporary file is left behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
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
        assert!(
            private::check_dir(&dir).is_ok(),
            "create_dir narrows an existing directory"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resources_come_from_apz_share_then_beside_the_binary() {
        let root = scratch("resources");
        let share = root.join("share").join("agentplaza");
        let elsewhere = root.join("elsewhere");
        let bin = root.join("bin");
        for dir in [&share, &elsewhere, &bin] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let exe = bin.join("apid");
        assert_eq!(
            dirs::resources_from(Some(elsewhere.as_os_str()), &exe),
            Some(elsewhere.clone())
        );
        assert_eq!(
            dirs::resources_from(None, &exe),
            Some(bin.join("..").join("share").join("agentplaza"))
        );
        assert_eq!(
            dirs::resources_from(None, std::path::Path::new("/nonexistent/bin/apid")),
            None
        );
        std::fs::remove_dir_all(&root).unwrap();
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
            assert!(
                p.to_string_lossy().to_lowercase().contains("agentplaza"),
                "{}",
                p.display()
            );
        }
    }
}
