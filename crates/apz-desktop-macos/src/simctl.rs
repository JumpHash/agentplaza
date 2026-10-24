//! The iOS simulator's command lines (`xcrun simctl`, PLAN 19.3, W47): boot, install, launch, terminate, screenshots,
//! video and shutdown, built as argument lists so that nothing goes through a shell here.
//!
//! The lines are built and checked on every host. [`Xcrun`] runs the real tool where Xcode's command line tools are
//! installed, and is the only part that needs them. The hub's user tests run the same lines on a Mac node through the
//! node's shell, which takes them as text: [`shell_line`] quotes an argument list for that.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The program every line starts with. It is found on the PATH of the process that runs it.
pub const XCRUN: &str = "xcrun";

/// One `simctl` command. A device is a simulator's UDID, its name, or `booted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Simctl {
    /// Boots the device when it is not booted, and waits until it is (`bootstatus -b`).
    Boot { device: String },
    /// Installs a built `.app` bundle, at an absolute path.
    Install { device: String, app: PathBuf },
    /// Launches a bundle, ending a copy that is already running, so the app starts fresh.
    Launch {
        device: String,
        bundle: String,
        args: Vec<String>,
    },
    /// Ends a running bundle.
    Terminate { device: String, bundle: String },
    /// A PNG of the screen, written to an absolute path.
    Screenshot { device: String, path: PathBuf },
    /// Records the screen to a `.mov` at an absolute path. The file is finished when the process is interrupted
    /// (SIGINT), so it is recorded in the background and stopped with [`Runner::stop`].
    Record { device: String, path: PathBuf },
    /// Shuts the device down.
    Shutdown { device: String },
}

impl Simctl {
    /// The arguments after `xcrun`, starting with `simctl`. Refused when a device or bundle is not a name the tool can
    /// take (empty, starting with `-`, or with a control character), or a path is not absolute.
    pub fn args(&self) -> Result<Vec<String>, String> {
        let words = |items: &[&str]| -> Vec<String> {
            std::iter::once("simctl")
                .chain(items.iter().copied())
                .map(str::to_string)
                .collect()
        };
        match self {
            Simctl::Boot { device } => {
                let device = name("device", device)?;
                Ok(words(&["bootstatus", &device, "-b"]))
            }
            Simctl::Install { device, app } => {
                let device = name("device", device)?;
                let app = absolute("the app", app)?;
                Ok(words(&["install", &device, &app]))
            }
            Simctl::Launch {
                device,
                bundle,
                args,
            } => {
                let device = name("device", device)?;
                let bundle = name("bundle", bundle)?;
                let mut out = words(&["launch", "--terminate-running-process", &device, &bundle]);
                out.extend(args.iter().cloned());
                Ok(out)
            }
            Simctl::Terminate { device, bundle } => {
                let device = name("device", device)?;
                let bundle = name("bundle", bundle)?;
                Ok(words(&["terminate", &device, &bundle]))
            }
            Simctl::Screenshot { device, path } => {
                let device = name("device", device)?;
                let path = absolute("the screenshot", path)?;
                Ok(words(&["io", &device, "screenshot", "--type=png", &path]))
            }
            Simctl::Record { device, path } => {
                let device = name("device", device)?;
                let path = absolute("the recording", path)?;
                Ok(words(&[
                    "io",
                    &device,
                    "recordVideo",
                    "--codec=h264",
                    "--force",
                    &path,
                ]))
            }
            Simctl::Shutdown { device } => {
                let device = name("device", device)?;
                Ok(words(&["shutdown", &device]))
            }
        }
    }
}

/// A name the tool is given as an argument: not empty, not an option, no control characters.
fn name(what: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_control) {
        return Err(format!("{what} is not a simulator name: {value:?}"));
    }
    Ok(value.to_string())
}

/// A path the tool is given: absolute, so that it is never read as an option, with no control characters.
fn absolute(what: &str, path: &Path) -> Result<String, String> {
    let text = path.to_string_lossy();
    if !path.is_absolute() || text.chars().any(char::is_control) {
        return Err(format!("{what} needs an absolute path, not {text:?}"));
    }
    Ok(text.into_owned())
}

/// The command lines of a physical iPhone through Xcode's `devicectl` (PLAN 19.3, 8.5): an iPhone on a Mac node is leased as a
/// device by its UDID, and its app is installed and launched with these lines. ASSUMED from Xcode 15's `xcrun devicectl`
/// (`device install app --device <id> <path>`, `device process launch --device <id> <bundle>`): built and checked here, not
/// run (there is no Xcode on the machine that builds this). Its tree and its pictures are not `devicectl`'s: a physical
/// iPhone needs the accessibility bridge (`simulator::Bridge`) and a capture the bridge does not give, so a physical iPhone's
/// scenarios are not run yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Devicectl {
    /// Lists the devices Xcode can see.
    List,
    /// Installs a built `.app` bundle, at an absolute path, on the device.
    Install { device: String, app: PathBuf },
    /// Launches a bundle on the device.
    Launch { device: String, bundle: String },
}

impl Devicectl {
    /// The arguments after `xcrun`, starting with `devicectl`. The same checks as [`Simctl::args`] apply to a device and a path.
    pub fn args(&self) -> Result<Vec<String>, String> {
        let words = |items: &[&str]| -> Vec<String> {
            std::iter::once("devicectl")
                .chain(items.iter().copied())
                .map(str::to_string)
                .collect()
        };
        match self {
            Devicectl::List => Ok(words(&["list", "devices"])),
            Devicectl::Install { device, app } => {
                let device = name("device", device)?;
                let app = absolute("the app", app)?;
                Ok(words(&[
                    "device", "install", "app", "--device", &device, &app,
                ]))
            }
            Devicectl::Launch { device, bundle } => {
                let device = name("device", device)?;
                let bundle = name("bundle", bundle)?;
                Ok(words(&[
                    "device", "process", "launch", "--device", &device, &bundle,
                ]))
            }
        }
    }
}

/// Runs the tool: a command to its end, a background process, a file a process wrote. The one seam between the command
/// lines and the machine that runs them, so a test can answer from a script.
pub trait Runner {
    /// Runs `xcrun` with `args` to its end. Its standard output, or the failure with its error output.
    fn run(&self, args: &[String]) -> Result<String, String>;
    /// Starts `xcrun` with `args` in the background, and answers its process id.
    fn start(&self, args: &[String]) -> Result<u32, String>;
    /// Interrupts a background process started by [`Runner::start`] and waits for it to end.
    fn stop(&self, pid: u32) -> Result<(), String>;
    /// The bytes of a file on the machine (a screenshot the tool wrote).
    fn fetch(&self, path: &Path) -> Result<Vec<u8>, String>;
}

/// Runs one command to its end: its output. The arguments are checked first ([`Simctl::args`]).
pub fn exec(runner: &dyn Runner, command: &Simctl) -> Result<String, String> {
    runner.run(&command.args()?)
}

/// The real tool, found on a list of directories (the PATH of the process, or a fake in a test).
pub struct Xcrun {
    program: PathBuf,
    background: RefCell<Vec<Child>>,
}

impl Xcrun {
    /// The `xcrun` in the first of `dirs` that has one. Without Xcode's command line tools there is none, and the
    /// error says so: nothing is installed by this.
    pub fn in_dirs(dirs: &[PathBuf]) -> Result<Self, String> {
        dirs.iter()
            .map(|dir| dir.join(XCRUN))
            .find(|candidate| candidate.is_file())
            .map(Self::at)
            .ok_or_else(|| {
                "xcrun is not installed here (Xcode's command line tools): no simulator command can run".to_string()
            })
    }

    /// The `xcrun` on this process's PATH.
    pub fn from_env() -> Result<Self, String> {
        let dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        Self::in_dirs(&dirs)
    }

    /// The tool at `program`.
    pub fn at(program: PathBuf) -> Self {
        Self {
            program,
            background: RefCell::new(Vec::new()),
        }
    }

    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new(&self.program);
        command.args(args).stdin(Stdio::null());
        command
    }
}

impl Runner for Xcrun {
    fn run(&self, args: &[String]) -> Result<String, String> {
        let output = self
            .command(args)
            .output()
            .map_err(|e| format!("cannot run xcrun: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "xcrun {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn start(&self, args: &[String]) -> Result<u32, String> {
        let mut command = self.command(args);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let child = command
            .spawn()
            .map_err(|e| format!("cannot start xcrun: {e}"))?;
        let pid = child.id();
        self.background.borrow_mut().push(child);
        Ok(pid)
    }

    fn stop(&self, pid: u32) -> Result<(), String> {
        let mut background = self.background.borrow_mut();
        let at = background
            .iter()
            .position(|child| child.id() == pid)
            .ok_or_else(|| format!("no background process {pid} to stop"))?;
        let mut child = background.remove(at);
        // The child is waited for before any error is returned, so an interrupt that fails leaves no zombie behind.
        let interrupted = Command::new("/bin/kill")
            .arg("-INT")
            .arg(pid.to_string())
            .status();
        let waited = child.wait();
        interrupted.map_err(|e| format!("cannot interrupt {pid}: {e}"))?;
        waited
            .map(|_| ())
            .map_err(|e| format!("waiting for {pid}: {e}"))
    }

    fn fetch(&self, path: &Path) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    }
}

/// An argument list as one shell line: every argument single-quoted, so that no word is read as more than it is.
pub fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| format!("'{}'", arg.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(command: &Simctl) -> Vec<String> {
        command.args().expect("the line is valid")
    }

    #[test]
    fn each_command_is_the_simctl_line_the_tool_takes() {
        let app = PathBuf::from("/Users/me/build/Shop.app");
        let png = PathBuf::from("/tmp/shot-1.png");
        assert_eq!(
            lines(&Simctl::Boot {
                device: "booted".into()
            }),
            ["simctl", "bootstatus", "booted", "-b"]
        );
        assert_eq!(
            lines(&Simctl::Install {
                device: "ABC-123".into(),
                app: app.clone()
            }),
            ["simctl", "install", "ABC-123", "/Users/me/build/Shop.app"]
        );
        assert_eq!(
            lines(&Simctl::Launch {
                device: "iPhone 17 Pro".into(),
                bundle: "dev.example.shop".into(),
                args: vec!["--fixture".into(), "cart".into()],
            }),
            [
                "simctl",
                "launch",
                "--terminate-running-process",
                "iPhone 17 Pro",
                "dev.example.shop",
                "--fixture",
                "cart"
            ]
        );
        assert_eq!(
            lines(&Simctl::Terminate {
                device: "booted".into(),
                bundle: "dev.example.shop".into()
            }),
            ["simctl", "terminate", "booted", "dev.example.shop"]
        );
        assert_eq!(
            lines(&Simctl::Screenshot {
                device: "booted".into(),
                path: png.clone()
            }),
            [
                "simctl",
                "io",
                "booted",
                "screenshot",
                "--type=png",
                "/tmp/shot-1.png"
            ]
        );
        assert_eq!(
            lines(&Simctl::Record {
                device: "booted".into(),
                path: PathBuf::from("/tmp/run.mov")
            }),
            [
                "simctl",
                "io",
                "booted",
                "recordVideo",
                "--codec=h264",
                "--force",
                "/tmp/run.mov"
            ]
        );
        assert_eq!(
            lines(&Simctl::Shutdown {
                device: "booted".into()
            }),
            ["simctl", "shutdown", "booted"]
        );
    }

    #[test]
    fn a_name_that_is_an_option_or_a_relative_path_is_refused() {
        assert!(
            Simctl::Shutdown {
                device: "--all".into()
            }
            .args()
            .is_err()
        );
        assert!(
            Simctl::Shutdown {
                device: "  ".into()
            }
            .args()
            .is_err()
        );
        assert!(
            Simctl::Install {
                device: "booted".into(),
                app: PathBuf::from("Shop.app")
            }
            .args()
            .is_err()
        );
        assert!(
            Simctl::Terminate {
                device: "booted".into(),
                bundle: "a\nb".into()
            }
            .args()
            .is_err()
        );
    }

    #[test]
    fn a_shell_line_quotes_every_word() {
        let argv = vec![
            "xcrun".to_string(),
            "launch".to_string(),
            "iPhone 17 Pro".to_string(),
            "it's".to_string(),
        ];
        assert_eq!(
            shell_line(&argv),
            "'xcrun' 'launch' 'iPhone 17 Pro' 'it'\\''s'"
        );
    }

    /// A fake `xcrun` on a directory of its own: it records each call's arguments (one per line, a `--` after each
    /// call) in `log`, writes a PNG where a screenshot goes, and runs until it is interrupted when it records.
    #[cfg(unix)]
    fn fake_xcrun(dir: &Path, log: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = format!(
            r#"#!/bin/sh
{{ printf '%s\n' "$@"; echo '--'; }} >> '{log}'
last=""
for a do last="$a"; done
case "$1" in
  simctl)
    case "$2" in
      io)
        case "$4" in
          screenshot) printf '\211PNG\r\n\032\n' > "$last" ;;
          recordVideo)
            printf 'mov' > "$last"
            trap 'exit 0' INT
            while :; do sleep 0.05; done ;;
        esac ;;
      bogus) echo "no such command" >&2; exit 1 ;;
    esac ;;
esac
exit 0
"#,
            log = log.display()
        );
        let program = dir.join(XCRUN);
        std::fs::write(&program, script).expect("the fake xcrun is written");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("the fake xcrun is executable");
        program
    }

    /// The calls a fake recorded, one argument list per call.
    #[cfg(unix)]
    fn recorded(log: &Path) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        text.split("--\n")
            .map(|call| call.lines().map(str::to_string).collect::<Vec<_>>())
            .filter(|call| !call.is_empty())
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn the_lines_run_through_the_xcrun_found_on_the_path_and_record_their_argv() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let tool = fake_xcrun(dir.path(), &log);
        let xcrun = Xcrun::in_dirs(&[dir.path().to_path_buf()]).expect("the fake is found");
        assert_eq!(xcrun.program, tool);

        let booted = exec(
            &xcrun,
            &Simctl::Boot {
                device: "booted".into(),
            },
        )
        .expect("boot runs");
        assert_eq!(booted, "");
        let shot = dir.path().join("shot.png");
        exec(
            &xcrun,
            &Simctl::Screenshot {
                device: "booted".into(),
                path: shot.clone(),
            },
        )
        .expect("the screenshot runs");
        let png = xcrun.fetch(&shot).expect("the screenshot is there");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));

        exec(
            &xcrun,
            &Simctl::Shutdown {
                device: "booted".into(),
            },
        )
        .expect("shutdown on the fake exits 0");

        let calls = recorded(&log);
        assert_eq!(
            calls,
            vec![
                vec!["simctl", "bootstatus", "booted", "-b"],
                vec![
                    "simctl",
                    "io",
                    "booted",
                    "screenshot",
                    "--type=png",
                    shot.to_str().unwrap()
                ],
                vec!["simctl", "shutdown", "booted"],
            ]
            .into_iter()
            .map(|call| call.into_iter().map(str::to_string).collect::<Vec<_>>())
            .collect::<Vec<_>>()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_line_says_what_the_tool_answered() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        fake_xcrun(dir.path(), &log);
        let xcrun = Xcrun::in_dirs(&[dir.path().to_path_buf()]).unwrap();
        let why = xcrun
            .run(&["simctl".to_string(), "bogus".to_string(), "x".to_string()])
            .unwrap_err();
        assert!(why.contains("no such command"), "{why}");
    }

    #[cfg(unix)]
    #[test]
    fn a_recording_runs_in_the_background_until_it_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        fake_xcrun(dir.path(), &log);
        let xcrun = Xcrun::in_dirs(&[dir.path().to_path_buf()]).unwrap();
        let movie = dir.path().join("run.mov");
        let args = Simctl::Record {
            device: "booted".into(),
            path: movie.clone(),
        }
        .args()
        .unwrap();
        let pid = xcrun.start(&args).expect("the recording starts");
        // The fake writes the file as it starts; wait for it, then interrupt the process.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !movie.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(movie.exists(), "the recording wrote its file");
        xcrun.stop(pid).expect("the recording stops");
        assert!(xcrun.stop(pid).is_err(), "a stopped process is gone");
    }

    #[test]
    fn the_devicectl_lines_for_a_physical_iphone_are_built_and_checked() {
        let install = Devicectl::Install {
            device: "00008110-001C2D3E4F5A6B7C".into(),
            app: PathBuf::from("/Users/ci/Shop.app"),
        }
        .args()
        .unwrap();
        assert_eq!(
            install,
            vec![
                "devicectl",
                "device",
                "install",
                "app",
                "--device",
                "00008110-001C2D3E4F5A6B7C",
                "/Users/ci/Shop.app"
            ]
        );
        let launch = Devicectl::Launch {
            device: "00008110-001C2D3E4F5A6B7C".into(),
            bundle: "com.acme.shop".into(),
        }
        .args()
        .unwrap();
        assert_eq!(launch[..4], ["devicectl", "device", "process", "launch"]);
        assert_eq!(
            Devicectl::List.args().unwrap(),
            vec!["devicectl", "list", "devices"]
        );
        assert!(
            Devicectl::Install {
                device: "-x".into(),
                app: PathBuf::from("/a.app"),
            }
            .args()
            .is_err()
        );
        assert!(
            Devicectl::Install {
                device: "d".into(),
                app: PathBuf::from("a.app"),
            }
            .args()
            .is_err(),
            "the app is an absolute path"
        );
    }

    #[test]
    fn no_xcrun_in_the_directories_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let why = Xcrun::in_dirs(&[dir.path().to_path_buf()])
            .err()
            .expect("there is no xcrun");
        assert!(why.contains("Xcode"), "{why}");
    }
}
