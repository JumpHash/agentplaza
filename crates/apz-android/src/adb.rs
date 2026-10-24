//! The adb command lines (PLAN 19.3). Each [`Adb`] is one command for one device, built as an argument list so that
//! nothing goes through a shell here. The lines assume the commands of adb and of Android 12's device shell:
//!
//! - `shell uiautomator dump /dev/tty` writes the window's XML to the terminal (the reader skips any line before it);
//! - `shell screencap -p <path>` writes a PNG on the device, which `pull` copies to the node;
//! - `shell screenrecord --time-limit <s> <path>` records until it is interrupted, and `shell pkill -2 screenrecord`
//!   interrupts it on the device (the local adb process that carries it ends with it);
//! - `shell input text` takes its text with `%s` for a space and a backslash before a shell character (adb joins the
//!   arguments with spaces for the device's shell);
//! - `shell am start -n <package>/<activity>` and `shell am force-stop <package>`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The program every line starts with. The node's shell or [`Local`] finds it on its PATH (or in the directories given).
pub const ADB: &str = "adb";
/// The longest a device's screenrecord records in one file (its own limit).
pub const RECORD_MAX_S: u32 = 180;

/// One adb command for one device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Adb {
    /// The window's accessibility tree, as XML, written to the terminal.
    UiDump,
    /// A tap at a point of the screen, in pixels.
    Tap { x: i32, y: i32 },
    /// A swipe from one point to another over `ms` milliseconds.
    Swipe {
        from: (i32, i32),
        to: (i32, i32),
        ms: u32,
    },
    /// Text typed into the focused field (escaped by [`input_text`]).
    Text { text: String },
    /// A key event, by its Android keycode name (`KEYCODE_BACK`).
    Key { code: String },
    /// A PNG of the screen, written to an absolute path on the device.
    Screencap { path: String },
    /// Copies a file from the device to an absolute path on the node.
    Pull { device_path: String, local: PathBuf },
    /// Deletes a file on the device.
    Remove { path: String },
    /// Records the screen to an absolute path on the device for `seconds` (at most [`RECORD_MAX_S`]). Runs in the
    /// background; [`Adb::StopRecord`] ends it.
    Record { path: String, seconds: u32 },
    /// Interrupts the device's screenrecord, so that its file is finished.
    StopRecord,
    /// Installs an APK, by an absolute path on the node. Replaces an app already installed.
    Install { apk: PathBuf },
    /// Starts an activity: `package/.Activity` (the full component).
    Start { component: String },
    /// Ends a package's processes.
    ForceStop { package: String },
}

impl Adb {
    /// The arguments after `adb` for the device `serial`: `-s <serial>` and then the command. Refused when the serial is not
    /// a serial, a path is not absolute, a key is not a keycode, a package or component is not a name, or a time is out of
    /// range.
    pub fn args(&self, serial: &str) -> Result<Vec<String>, String> {
        if !serial_ok(serial) {
            return Err(format!("{serial:?} is not an adb serial"));
        }
        let mut out = vec!["-s".to_string(), serial.to_string()];
        let words: Vec<String> = match self {
            Adb::UiDump => shell(&["uiautomator", "dump", "/dev/tty"]),
            Adb::Tap { x, y } => shell(&["input", "tap", &x.to_string(), &y.to_string()]),
            Adb::Swipe { from, to, ms } => {
                if !(1..=10_000).contains(ms) {
                    return Err("a swipe takes between 1 and 10000 milliseconds".into());
                }
                shell(&[
                    "input",
                    "swipe",
                    &from.0.to_string(),
                    &from.1.to_string(),
                    &to.0.to_string(),
                    &to.1.to_string(),
                    &ms.to_string(),
                ])
            }
            Adb::Text { text } => shell(&["input", "text", &input_text(text)?]),
            Adb::Key { code } => {
                let keycode = code.len() > "KEYCODE_".len()
                    && code.starts_with("KEYCODE_")
                    && code
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                if !keycode {
                    return Err(format!("{code:?} is not a keycode name (KEYCODE_BACK)"));
                }
                shell(&["input", "keyevent", code])
            }
            Adb::Screencap { path } => {
                device_path("the screenshot", path)?;
                shell(&["screencap", "-p", path])
            }
            Adb::Pull {
                device_path: from,
                local,
            } => {
                device_path("the pulled file", from)?;
                let local = absolute_local("the pulled file", local)?;
                vec!["pull".into(), from.clone(), local]
            }
            Adb::Remove { path } => {
                device_path("the removed file", path)?;
                shell(&["rm", "-f", path])
            }
            Adb::Record { path, seconds } => {
                device_path("the recording", path)?;
                if !(1..=RECORD_MAX_S).contains(seconds) {
                    return Err(format!(
                        "a recording lasts between 1 and {RECORD_MAX_S} seconds"
                    ));
                }
                shell(&["screenrecord", "--time-limit", &seconds.to_string(), path])
            }
            Adb::StopRecord => shell(&["pkill", "-2", "screenrecord"]),
            Adb::Install { apk } => {
                let apk = absolute_local("the app", apk)?;
                vec!["install".into(), "-r".into(), apk]
            }
            Adb::Start { component } => {
                let ok = component
                    .split_once('/')
                    .is_some_and(|(package, activity)| {
                        package_ok(package)
                            && !activity.is_empty()
                            && !activity.contains(char::is_whitespace)
                    });
                if !ok {
                    return Err(format!("{component:?} is not a package/activity component"));
                }
                shell(&["am", "start", "-n", component])
            }
            Adb::ForceStop { package } => {
                if !package_ok(package) {
                    return Err(format!("{package:?} is not a package name"));
                }
                shell(&["am", "force-stop", package])
            }
        };
        out.extend(words);
        Ok(out)
    }
}

/// `shell` and the device's words.
fn shell(words: &[&str]) -> Vec<String> {
    std::iter::once("shell")
        .chain(words.iter().copied())
        .map(str::to_string)
        .collect()
}

/// A serial: not empty, not an option, no whitespace or control characters.
fn serial_ok(serial: &str) -> bool {
    !serial.is_empty()
        && !serial.starts_with('-')
        && !serial.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// A package name: letters, digits, dots and underscores, not empty.
fn package_ok(package: &str) -> bool {
    !package.is_empty()
        && package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

fn device_path(what: &str, path: &str) -> Result<(), String> {
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return Err(format!(
            "{what} needs an absolute path on the device, not {path:?}"
        ));
    }
    Ok(())
}

fn absolute_local(what: &str, path: &Path) -> Result<String, String> {
    let text = path.to_string_lossy();
    if !path.is_absolute() || text.chars().any(char::is_control) {
        return Err(format!(
            "{what} needs an absolute path on the node, not {text:?}"
        ));
    }
    Ok(text.into_owned())
}

/// The text an `input text` line takes: a space is `%s`, a shell character is backslashed (adb hands the device's shell one
/// line), and anything `input` cannot take is refused: non-ASCII, control characters, and `%` (which `input` reads as an
/// escape).
pub fn input_text(text: &str) -> Result<String, String> {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            ' ' => out.push_str("%s"),
            c if c.is_ascii_alphanumeric()
                || matches!(c, '.' | ',' | '_' | '-' | '@' | ':' | '/' | '+' | '=') =>
            {
                out.push(c)
            }
            c if c.is_ascii_graphic() && c != '%' => {
                out.push('\\');
                out.push(c);
            }
            c => return Err(format!("{c:?} cannot be typed with input text")),
        }
    }
    Ok(out)
}

/// Runs adb: the argument lists of [`Adb::args`] (the program is added by the runner), a background process, and a file the
/// command wrote. The one seam between the lines and the machine that runs them, so a test can answer from a script.
pub trait Runner {
    /// Runs `adb` with `args` to its end: its standard output, or the failure with its error output.
    fn run(&self, args: &[String]) -> Result<String, String>;
    /// Starts `adb` with `args` in the background, and answers its process id.
    fn start(&self, args: &[String]) -> Result<u32, String>;
    /// Ends a background process started by [`Runner::start`] and waits for it. A process that does not end is killed,
    /// and that is the error.
    fn stop(&self, pid: u32) -> Result<(), String>;
    /// The bytes of a file on the node.
    fn fetch(&self, path: &Path) -> Result<Vec<u8>, String>;
}

/// Runs one command for `serial` to its end: its output. The arguments are checked first ([`Adb::args`]).
pub fn exec(runner: &dyn Runner, serial: &str, command: &Adb) -> Result<String, String> {
    runner.run(&command.args(serial)?)
}

/// An argument list as one shell line: every argument single-quoted, so that no word is read as more than it is.
pub fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| format!("'{}'", arg.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The port the adb server listens on, on this machine: the one loopback port an adb client may reach when confined. The server
/// holds the USB connections to the devices, so a client needs no device access of its own, and none is granted.
pub const SERVER_PORT: u16 = 5037;

/// The `adb` found in a list of directories, run on this machine. Production runs adb on a node through its shell instead
/// (`user_test::android`); this is the runner of the tests (a fake `adb` on a directory of its own) and of a node that runs
/// adb itself. A confined runner ([`Local::confined`]) runs each line under the operating system's sandbox (`apz_sandbox`:
/// the Seatbelt profile on macOS, Landlock on Linux), the confinement noded gives its sessions.
pub struct Local {
    program: PathBuf,
    background: RefCell<Vec<Child>>,
    /// The work directory a confined line may write (its cwd and HOME). None when the runner does not confine.
    workdir: Option<PathBuf>,
}

impl Local {
    /// The `adb` in the first of `dirs` that has one. The error says so: nothing is installed by this.
    pub fn in_dirs(dirs: &[PathBuf]) -> Result<Self, String> {
        dirs.iter()
            .map(|dir| dir.join(ADB))
            .find(|candidate| candidate.is_file())
            .map(Self::at)
            .ok_or_else(|| {
                "adb is not found in the directories given (Android platform tools)".to_string()
            })
    }

    /// The `adb` in `dirs`, run confined to `workdir`.
    pub fn confined_in_dirs(dirs: &[PathBuf], workdir: PathBuf) -> Result<Self, String> {
        let mut local = Self::in_dirs(dirs)?;
        local.workdir = Some(workdir);
        Ok(local)
    }

    /// The tool at `program`.
    pub fn at(program: PathBuf) -> Self {
        Self {
            program,
            background: RefCell::new(Vec::new()),
            workdir: None,
        }
    }

    /// The tool at `program`, run confined: its lines may read the program, write `workdir` (their cwd and HOME), and reach
    /// the adb server on this machine ([`SERVER_PORT`]). Nothing else.
    pub fn confined(program: PathBuf, workdir: PathBuf) -> Self {
        Self {
            workdir: Some(workdir),
            ..Self::at(program)
        }
    }

    /// The command a line runs as. Unconfined, the program with its arguments. Confined, the sandbox's command: on macOS
    /// `sandbox-exec` with the profile, then the program. The profile is built by `apz_sandbox` from the work directory and
    /// the server's port.
    pub fn command_for(&self, args: &[String]) -> Result<Command, String> {
        let mut command = match &self.workdir {
            None => {
                let mut command = Command::new(&self.program);
                command.args(args);
                command
            }
            Some(workdir) => {
                let home = workdir.display().to_string();
                let spec = apz_sandbox::Spec {
                    program: self.program.clone(),
                    args: args.to_vec(),
                    env: vec![
                        (
                            "PATH".to_string(),
                            "/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
                        ),
                        ("HOME".to_string(), home.clone()),
                        ("TMPDIR".to_string(), home),
                    ],
                    workdir: workdir.clone(),
                    read_only: Vec::new(),
                    read_write: Vec::new(),
                    network: apz_sandbox::Network::LoopbackPort(SERVER_PORT),
                    limits: apz_sandbox::Limits::default(),
                };
                apz_sandbox::command(&spec).map_err(|e| format!("adb cannot be confined: {e}"))?
            }
        };
        command.stdin(Stdio::null());
        Ok(command)
    }
}

/// How long [`Local::stop`] waits for a background process before it kills it.
const STOP_WAIT: Duration = Duration::from_secs(15);

impl Runner for Local {
    fn run(&self, args: &[String]) -> Result<String, String> {
        let output = self
            .command_for(args)?
            .output()
            .map_err(|e| format!("cannot run adb: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "adb {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn start(&self, args: &[String]) -> Result<u32, String> {
        let mut command = self.command_for(args)?;
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let child = command
            .spawn()
            .map_err(|e| format!("cannot start adb: {e}"))?;
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
        let deadline = std::time::Instant::now() + STOP_WAIT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("process {pid} did not end, and was killed"));
                }
                Err(e) => return Err(format!("waiting for {pid}: {e}")),
            }
        }
    }

    fn fetch(&self, path: &Path) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(command: &Adb) -> Vec<String> {
        command.args("emulator-5554").unwrap()
    }

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn each_command_is_the_adb_line_for_its_serial() {
        assert_eq!(
            words(&Adb::UiDump),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "uiautomator",
                "dump",
                "/dev/tty"
            ])
        );
        assert_eq!(
            words(&Adb::Tap { x: 540, y: 1680 }),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "input",
                "tap",
                "540",
                "1680"
            ])
        );
        assert_eq!(
            words(&Adb::Swipe {
                from: (500, 1500),
                to: (500, 500),
                ms: 300
            }),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "input",
                "swipe",
                "500",
                "1500",
                "500",
                "500",
                "300"
            ])
        );
        assert_eq!(
            words(&Adb::Key {
                code: "KEYCODE_BACK".into()
            }),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "input",
                "keyevent",
                "KEYCODE_BACK"
            ])
        );
        assert_eq!(
            words(&Adb::Screencap {
                path: "/sdcard/shot.png".into()
            }),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "screencap",
                "-p",
                "/sdcard/shot.png"
            ])
        );
        assert_eq!(
            words(&Adb::Pull {
                device_path: "/sdcard/shot.png".into(),
                local: PathBuf::from("/tmp/shot.png"),
            }),
            list(&[
                "-s",
                "emulator-5554",
                "pull",
                "/sdcard/shot.png",
                "/tmp/shot.png"
            ])
        );
        assert_eq!(
            words(&Adb::Record {
                path: "/sdcard/run.mp4".into(),
                seconds: 60
            }),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "screenrecord",
                "--time-limit",
                "60",
                "/sdcard/run.mp4"
            ])
        );
        assert_eq!(
            words(&Adb::StopRecord),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "pkill",
                "-2",
                "screenrecord"
            ])
        );
        assert_eq!(
            words(&Adb::Install {
                apk: PathBuf::from("/Users/ci/app.apk")
            }),
            list(&["-s", "emulator-5554", "install", "-r", "/Users/ci/app.apk"])
        );
        assert_eq!(
            words(&Adb::Start {
                component: "com.shop/.MainActivity".into()
            }),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "am",
                "start",
                "-n",
                "com.shop/.MainActivity"
            ])
        );
        assert_eq!(
            words(&Adb::ForceStop {
                package: "com.shop".into()
            }),
            list(&[
                "-s",
                "emulator-5554",
                "shell",
                "am",
                "force-stop",
                "com.shop"
            ])
        );
    }

    #[test]
    fn typed_text_is_escaped_for_the_device_shell_and_refused_when_input_cannot_take_it() {
        assert_eq!(input_text("no ice").unwrap(), "no%sice");
        assert_eq!(input_text("a&b (c)").unwrap(), r"a\&b%s\(c\)");
        assert_eq!(input_text("it's").unwrap(), r"it\'s");
        assert!(
            input_text("100%").is_err(),
            "a percent sign is an escape to input"
        );
        assert!(input_text("café").is_err(), "non-ASCII is refused");
        assert!(input_text("line\nbreak").is_err());
    }

    #[test]
    fn a_serial_or_path_or_name_that_is_not_one_is_refused() {
        assert!(Adb::UiDump.args("-x").is_err(), "an option is not a serial");
        assert!(Adb::UiDump.args("").is_err());
        assert!(Adb::UiDump.args("a b").is_err());
        assert!(
            Adb::Screencap {
                path: "shot.png".into()
            }
            .args("s")
            .is_err(),
            "a device path is absolute"
        );
        assert!(
            Adb::Install {
                apk: PathBuf::from("app.apk")
            }
            .args("s")
            .is_err()
        );
        assert!(
            Adb::Key {
                code: "BACK".into()
            }
            .args("s")
            .is_err()
        );
        assert!(
            Adb::Key {
                code: "KEYCODE_".into()
            }
            .args("s")
            .is_err()
        );
        assert!(
            Adb::Start {
                component: "com.shop".into()
            }
            .args("s")
            .is_err(),
            "a component names its activity"
        );
        assert!(
            Adb::ForceStop {
                package: "com.shop;rm".into()
            }
            .args("s")
            .is_err()
        );
        assert!(
            Adb::Record {
                path: "/sdcard/x.mp4".into(),
                seconds: 181
            }
            .args("s")
            .is_err()
        );
        assert!(
            Adb::Swipe {
                from: (0, 0),
                to: (1, 1),
                ms: 0
            }
            .args("s")
            .is_err()
        );
    }

    #[test]
    fn a_shell_line_quotes_every_word() {
        let line = shell_line(&list(&["adb", "input", "text", "it's"]));
        assert_eq!(line, r"'adb' 'input' 'text' 'it'\''s'");
    }

    /// A fake adb: a script on a directory of its own, which records its argv and answers `uiautomator dump` from a fixture.
    fn fake_adb(dir: &Path, log: &Path) -> PathBuf {
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\n\
             case \"$*\" in\n\
               *uiautomator*) echo 'UI hierchary dumped to: /dev/tty'; echo '<hierarchy rotation=\"0\"><node text=\"Hi\" bounds=\"[0,0][10,10]\"/></hierarchy>';;\n\
               *screencap*) printf 'PNG' > \"{dir}/shot.png\";;\n\
               *\" pull \"*) cp \"{dir}/shot.png\" \"$5\";;\n\
             esac\n",
            log = log.display(),
            dir = dir.display(),
        );
        let program = dir.join(ADB);
        std::fs::write(&program, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    #[test]
    fn the_lines_run_through_the_adb_found_on_the_directories_and_answer_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        fake_adb(dir.path(), &log);
        let runner = Local::in_dirs(&[dir.path().to_path_buf()]).unwrap();
        let dump = exec(&runner, "emulator-5554", &Adb::UiDump).unwrap();
        assert!(dump.contains("<hierarchy"), "{dump}");
        exec(
            &runner,
            "emulator-5554",
            &Adb::Screencap {
                path: "/sdcard/shot.png".into(),
            },
        )
        .unwrap();
        let local = dir.path().join("pulled.png");
        exec(
            &runner,
            "emulator-5554",
            &Adb::Pull {
                device_path: "/sdcard/shot.png".into(),
                local: local.clone(),
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), b"PNG");
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(
            recorded.contains("-s emulator-5554 shell uiautomator dump /dev/tty"),
            "{recorded}"
        );
        assert!(
            recorded.contains("-s emulator-5554 pull /sdcard/shot.png"),
            "{recorded}"
        );
    }

    #[test]
    fn a_failing_line_says_what_adb_answered_and_no_adb_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(ADB);
        std::fs::write(&program, "#!/bin/sh\necho 'device offline' >&2\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = Local::in_dirs(&[dir.path().to_path_buf()]).unwrap();
        let why = exec(&runner, "x1", &Adb::UiDump).unwrap_err();
        assert!(why.contains("device offline"), "{why}");
        let empty = tempfile::tempdir().unwrap();
        assert!(Local::in_dirs(&[empty.path().to_path_buf()]).is_err());
    }

    #[test]
    fn a_confined_line_is_the_sandbox_command_for_the_work_dir_and_the_server_port() {
        let dir = tempfile::tempdir().unwrap();
        let program = fake_adb(dir.path(), &dir.path().join("argv.log"));
        let runner = Local::confined(program.clone(), dir.path().to_path_buf());
        let args = Adb::UiDump.args("emulator-5554").unwrap();
        let command = runner.command_for(&args).unwrap();
        let argv: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let real = std::fs::canonicalize(&program).unwrap();
        if cfg!(target_os = "macos") {
            // Seatbelt: sandbox-exec applies the profile, then runs the program.
            assert_eq!(command.get_program(), "/usr/bin/sandbox-exec");
            assert_eq!(argv[0], "-p");
            let profile = &argv[1];
            let work = std::fs::canonicalize(dir.path()).unwrap();
            assert!(profile.contains("(deny default)"), "{profile}");
            assert!(
                profile.contains(&format!("(subpath \"{}\")", work.display())),
                "{profile}"
            );
            assert!(
                profile.contains("localhost:5037"),
                "the adb server's port, and no other network: {profile}"
            );
            assert_eq!(argv[2], "--");
            assert_eq!(argv[3], program.display().to_string());
            assert_eq!(&argv[4..], args.as_slice());
        } else {
            // Landlock is applied in the child before exec: the program is the command, and the rules are the same spec.
            assert_eq!(command.get_program(), real.as_os_str());
            assert_eq!(argv, args);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_confined_adb_answers_inside_its_work_dir_and_cannot_write_outside_it() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let escape = outside.path().join("escaped");
        let log = dir.path().join("argv.log");
        std::fs::write(
            dir.path().join("screen.xml"),
            "<hierarchy><node bounds=\"[0,0][1,1]\"/></hierarchy>",
        )
        .unwrap();
        let program = dir.path().join(ADB);
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncat '{screen}'\nif touch '{escape}' 2>/dev/null; then echo wrote; else echo denied; fi\nexit 0\n",
            log = log.display(),
            screen = dir.path().join("screen.xml").display(),
            escape = escape.display(),
        );
        std::fs::write(&program, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = Local::confined(program, dir.path().to_path_buf());
        let answer = exec(&runner, "emulator-5554", &Adb::UiDump).unwrap();
        assert!(
            answer.contains("<hierarchy>"),
            "the work dir is readable: {answer}"
        );
        assert!(
            answer.contains("denied"),
            "a write outside the work dir is refused: {answer}"
        );
        assert!(!escape.exists(), "nothing was written outside");
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("uiautomator dump"),
            "the work dir is writable"
        );
    }
}
