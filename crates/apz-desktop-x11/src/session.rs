//! A desktop session: a private Xvfb (pixel sessions) or the owner's display (shared sessions), with its X
//! connection, the pixel format of its root window, and what owner activity needs.
//!
//! A pixel session's server, window manager and launched apps share one process group, led by the server. A
//! stop sends SIGTERM to the group, waits briefly, then SIGKILL, so nothing started on the display survives.
//! Shared sessions own no processes; stopping one only drops the connection.

use std::cell::Cell;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use apz_desktop::{DesktopError, Frame, Result, SessionSpec};
use apz_station::{InputAction, SessionKind};
use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageOrder, Window};
use x11rb::rust_connection::RustConnection;

use crate::backend::X11Config;
use crate::capture::{ByteOrder, ChannelMasks, Grabber, PixelFormat, platform_error};
use crate::display::{DisplayPaths, END_DISPLAY, FIRST_DISPLAY, display_name, pick_display};
use crate::input;

/// The largest side of a session's screen, in points.
pub(crate) const MAX_SIDE: u32 = 16384;
/// How long a server gets to start (its socket to appear and accept a connection).
const STARTUP: Duration = Duration::from_secs(10);
/// How long a stopped server's processes get to exit after SIGTERM.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// How long after the owner last moved the pointer or typed the session counts as owner-active.
pub(crate) const OWNER_WINDOW: Duration = Duration::from_secs(3);
/// Slack for clock granularity when comparing the server's idle time with our last injected event.
const IDLE_MARGIN: Duration = Duration::from_millis(300);
/// How many numbers a pixel session tries when a server exits before its socket appears (a race for a number).
const START_ATTEMPTS: u32 = 8;
const POLL: Duration = Duration::from_millis(20);

/// The processes of a private display: its server, the optional window manager, and the files it owns.
pub(crate) struct Server {
    number: u32,
    /// The process group: the server's pid, while the server is running. Zero once stopped.
    group: i32,
    /// The server first, then the window manager.
    children: Vec<Child>,
    paths: DisplayPaths,
    /// Whether the server came up, so its lock file and socket are ours to remove.
    owns_files: bool,
}

impl Server {
    /// Stops the processes: SIGTERM to the group, a short grace, then SIGKILL, and removes the files the
    /// server left behind. Safe to call twice.
    fn stop(&mut self) {
        if self.group == 0 && self.children.is_empty() {
            return;
        }
        signal_group(self.group, libc::SIGTERM);
        let deadline = Instant::now() + STOP_GRACE;
        for child in &mut self.children {
            while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                thread::sleep(POLL);
            }
        }
        signal_group(self.group, libc::SIGKILL);
        for child in &mut self.children {
            let _ = child.wait();
        }
        self.children.clear();
        self.group = 0;
        if self.owns_files {
            let _ = fs::remove_file(self.paths.lock_file(self.number));
            let _ = fs::remove_file(self.paths.socket(self.number));
            self.owns_files = false;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Sends `signal` to a process group. Never signals group 0 or 1, which would be our own group or init's.
fn signal_group(group: i32, signal: libc::c_int) {
    if group > 1 {
        // SAFETY: kill with a negative pid signals a process group; errors (ESRCH once the group is gone) are
        // expected and ignored.
        unsafe { libc::kill(-group, signal) };
    }
}

/// One open session.
pub(crate) struct Session {
    pub(crate) kind: SessionKind,
    /// The display name clients use: `:N` for a pixel session, the owner's `DISPLAY` for a shared one.
    pub(crate) display: String,
    conn: RustConnection,
    root: Window,
    width: u32,
    height: u32,
    format: PixelFormat,
    server: Option<Server>,
    grabber: Grabber,
    /// Where the pointer was after our last action, or where we first saw it.
    pointer: Cell<Option<(i16, i16)>>,
    /// When our last injected event was sent.
    last_injected: Cell<Option<Instant>>,
    /// When owner activity was last seen.
    owner_seen: Cell<Option<Instant>>,
}

impl Session {
    /// Starts a private display for a pixel session: `Xvfb :N` under `config.run_as`, then the window manager
    /// when one is configured, then a connection to the server.
    pub(crate) fn start_pixel(config: &X11Config, spec: &SessionSpec) -> Result<Self> {
        let paths = &config.display_paths;
        let geometry = format!("{}x{}x24", spec.width, spec.height);
        let mut last_exit = None;
        for _ in 0..START_ATTEMPTS {
            let number = pick_display(FIRST_DISPLAY, END_DISPLAY, |n| paths.is_taken(n))
                .ok_or_else(|| {
                    DesktopError::Platform(format!(
                        "no free display number from {FIRST_DISPLAY} to {END_DISPLAY}"
                    ))
                })?;
            let display = display_name(number);
            let mut command = wrapped(
                config.run_as.as_deref(),
                config.xvfb.as_os_str(),
                &[
                    display.clone(),
                    "-screen".into(),
                    "0".into(),
                    geometry.clone(),
                    "-nolisten".into(),
                    "tcp".into(),
                ],
            );
            command
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_DISPLAY")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            command.process_group(0);
            let child = command.spawn().map_err(|error| {
                DesktopError::Platform(format!("cannot start {}: {error}", config.xvfb.display()))
            })?;
            let group = i32::try_from(child.id()).unwrap_or(0);
            let mut server = Server {
                number,
                group,
                children: vec![child],
                paths: paths.clone(),
                owns_files: false,
            };
            let deadline = Instant::now() + STARTUP;
            loop {
                if paths.socket(number).exists() {
                    server.owns_files = true;
                    break;
                }
                if let Some(status) = server.children[0].try_wait().map_err(platform_error)? {
                    last_exit = Some(format!(
                        "{display} exited ({status}) before its socket appeared"
                    ));
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(DesktopError::Platform(format!(
                        "{display} did not come up within {}s",
                        STARTUP.as_secs()
                    )));
                }
                thread::sleep(POLL);
            }
            if !server.owns_files {
                // Lost a race for this number: the server is gone, so nothing is ours to remove. Try the next.
                server.owns_files = false;
                continue;
            }
            let (conn, screen) = connect_when_ready(&display, &mut server.children[0])?;
            if let Some(argv) = config
                .window_manager
                .as_deref()
                .filter(|argv| !argv.is_empty())
            {
                let mut manager =
                    wrapped(config.run_as.as_deref(), OsStr::new(&argv[0]), &argv[1..]);
                manager
                    .env("DISPLAY", &display)
                    .env_remove("WAYLAND_DISPLAY")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                manager.process_group(group);
                let child = manager.spawn().map_err(|error| {
                    DesktopError::Platform(format!(
                        "cannot start the window manager {}: {error}",
                        argv[0]
                    ))
                })?;
                server.children.push(child);
            }
            return Self::from_connection(conn, screen, SessionKind::Pixel, display, Some(server));
        }
        Err(DesktopError::Platform(
            last_exit.unwrap_or_else(|| "no display came up".to_string()),
        ))
    }

    /// Connects to the owner's display for a shared session. The session takes the display's size.
    pub(crate) fn connect_shared(display: String) -> Result<Self> {
        let (conn, screen) = RustConnection::connect(Some(&display)).map_err(|error| {
            DesktopError::Platform(format!(
                "cannot connect to the owner's display {display}: {error}"
            ))
        })?;
        Self::from_connection(conn, screen, SessionKind::Shared, display, None)
    }

    fn from_connection(
        conn: RustConnection,
        screen: usize,
        kind: SessionKind,
        display: String,
        server: Option<Server>,
    ) -> Result<Self> {
        let setup = conn.setup();
        let root_screen = setup.roots.get(screen).ok_or_else(|| {
            DesktopError::Platform(format!("the display {display} has no screen {screen}"))
        })?;
        let root = root_screen.root;
        let width = u32::from(root_screen.width_in_pixels);
        let height = u32::from(root_screen.height_in_pixels);
        let bits_per_pixel = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == root_screen.root_depth)
            .map(|format| u32::from(format.bits_per_pixel))
            .ok_or_else(|| DesktopError::Platform("the root depth has no pixmap format".into()))?;
        let visual = root_screen
            .allowed_depths
            .iter()
            .flat_map(|depth| depth.visuals.iter())
            .find(|visual| visual.visual_id == root_screen.root_visual)
            .ok_or_else(|| DesktopError::Platform("the root visual is not listed".into()))?;
        let format = PixelFormat {
            bits_per_pixel,
            order: match setup.image_byte_order {
                ImageOrder::LSB_FIRST => ByteOrder::Lsb,
                _ => ByteOrder::Msb,
            },
            masks: ChannelMasks {
                red: visual.red_mask,
                green: visual.green_mask,
                blue: visual.blue_mask,
            },
        };
        let session = Self {
            kind,
            display,
            conn,
            root,
            width,
            height,
            format,
            server,
            grabber: Grabber::default(),
            pointer: Cell::new(None),
            last_injected: Cell::new(None),
            owner_seen: Cell::new(None),
        };
        session.pointer.set(session.pointer_position());
        Ok(session)
    }

    /// The server's pid, for a pixel session.
    pub(crate) fn server_pid(&self) -> Option<u32> {
        self.server
            .as_ref()
            .and_then(|server| server.children.first())
            .map(Child::id)
    }

    /// The process group of a pixel session's processes.
    pub(crate) fn group(&self) -> Option<i32> {
        self.server
            .as_ref()
            .map(|server| server.group)
            .filter(|&group| group > 0)
    }

    pub(crate) fn capture(&mut self) -> Result<Frame> {
        self.grabber
            .grab(&self.conn, self.root, self.width, self.height, self.format)
    }

    pub(crate) fn input(&self, action: &InputAction) -> Result<()> {
        self.last_injected.set(Some(Instant::now()));
        let result = input::perform(&self.conn, self.root, (self.width, self.height), action);
        self.last_injected.set(Some(Instant::now()));
        self.pointer.set(self.pointer_position());
        result
    }

    /// Whether the owner is using this session now. Always false for a pixel session.
    pub(crate) fn owner_active(&self) -> bool {
        if self.kind != SessionKind::Shared {
            return false;
        }
        let now = Instant::now();
        if let Some(here) = self.pointer_position() {
            if pointer_moved_by_owner(self.pointer.get(), here) {
                self.owner_seen.set(Some(now));
            }
            self.pointer.set(Some(here));
        }
        if let Some(idle_ms) = self.idle_ms() {
            let since = self
                .last_injected
                .get()
                .map(|at| now.saturating_duration_since(at));
            if idle_shows_owner(idle_ms, since)
                && let Some(at) = now.checked_sub(Duration::from_millis(u64::from(idle_ms)))
            {
                self.owner_seen.set(Some(at));
            }
        }
        matches!(self.owner_seen.get(), Some(seen) if now.saturating_duration_since(seen) < OWNER_WINDOW)
    }

    /// Starts an app on this session's display. Pixel sessions run it under `run_as`, in the session's group.
    pub(crate) fn spawn(&self, config: &X11Config, argv: &[String]) -> Result<()> {
        let mut command = match (self.kind, config.run_as.as_deref()) {
            (SessionKind::Pixel, Some(user)) => {
                let mut command = Command::new("sudo");
                command
                    .args(["-n", "-u", user, "--", "env", "-u", "WAYLAND_DISPLAY"])
                    .arg(format!("DISPLAY={}", self.display))
                    .args(argv);
                command
            }
            _ => {
                let mut command = Command::new(&argv[0]);
                command
                    .args(&argv[1..])
                    .env("DISPLAY", &self.display)
                    .env_remove("WAYLAND_DISPLAY");
                command
            }
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if self.kind == SessionKind::Pixel
            && let Some(group) = self.group()
        {
            command.process_group(group);
        }
        let mut child = command.spawn().map_err(|error| {
            DesktopError::Platform(format!("cannot start {}: {error}", argv[0]))
        })?;
        // The app outlives this call; a thread reaps it so no zombie is left behind.
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    /// The pointer's position on the root window, or `None` if the server does not say.
    fn pointer_position(&self) -> Option<(i16, i16)> {
        let reply = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((reply.root_x, reply.root_y))
    }

    /// The server's idle time in milliseconds (XScreenSaver), or `None` without the extension.
    fn idle_ms(&self) -> Option<u32> {
        let reply = self
            .conn
            .screensaver_query_info(self.root)
            .ok()?
            .reply()
            .ok()?;
        Some(reply.ms_since_user_input)
    }
}

/// Whether the pointer is somewhere other than where we left it, so the owner moved it.
pub(crate) fn pointer_moved_by_owner(expected: Option<(i16, i16)>, now: (i16, i16)) -> bool {
    matches!(expected, Some(expected) if expected != now)
}

/// Whether the server's idle time shows input from the owner: input newer than our last injected event, by
/// more than the margin. Without any injection yet, any input is the owner's.
pub(crate) fn idle_shows_owner(idle_ms: u32, since_injected: Option<Duration>) -> bool {
    let idle = Duration::from_millis(u64::from(idle_ms));
    match since_injected {
        None => true,
        Some(elapsed) => idle + IDLE_MARGIN < elapsed,
    }
}

/// Waits for the server's socket to accept connections, then connects.
fn connect_when_ready(display: &str, server: &mut Child) -> Result<(RustConnection, usize)> {
    let deadline = Instant::now() + STARTUP;
    loop {
        match RustConnection::connect(Some(display)) {
            Ok(pair) => return Ok(pair),
            Err(error) => {
                if server.try_wait().map_err(platform_error)?.is_some() {
                    return Err(DesktopError::Platform(format!(
                        "{display} exited before accepting a connection"
                    )));
                }
                if Instant::now() >= deadline {
                    return Err(DesktopError::Platform(format!(
                        "cannot connect to {display}: {error}"
                    )));
                }
                thread::sleep(POLL);
            }
        }
    }
}

/// A command for a pixel session's process: plain, or through `sudo -n -u <user>` when `run_as` is set.
/// `sudo -n` never prompts, so a missing sudoers rule fails the start instead of hanging it.
fn wrapped(run_as: Option<&str>, program: &OsStr, args: &[String]) -> Command {
    match run_as {
        None => {
            let mut command = Command::new(program);
            command.args(args);
            command
        }
        Some(user) => {
            let mut command = Command::new("sudo");
            command
                .args(["-n", "-u", user, "--"])
                .arg(program)
                .args(args);
            command
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_that_moved_off_our_last_point_was_moved_by_the_owner() {
        assert!(
            !pointer_moved_by_owner(None, (5, 5)),
            "nothing to compare with"
        );
        assert!(!pointer_moved_by_owner(Some((5, 5)), (5, 5)));
        assert!(pointer_moved_by_owner(Some((5, 5)), (6, 5)));
    }

    #[test]
    fn idle_time_newer_than_our_last_event_is_the_owner() {
        // Idle for 100 ms; our last event was 2 s ago: the owner typed or moved after us.
        assert!(idle_shows_owner(100, Some(Duration::from_secs(2))));
        // Idle for 2 s, our last event 2 s ago: the idle clock is ours.
        assert!(!idle_shows_owner(2000, Some(Duration::from_secs(2))));
        // Within the margin of our own event: not counted.
        assert!(!idle_shows_owner(100, Some(Duration::from_millis(250))));
        // Never injected: any recent input is the owner's.
        assert!(idle_shows_owner(100, None));
    }

    #[test]
    fn a_run_as_user_wraps_the_program_in_sudo_without_a_prompt() {
        let plain = wrapped(None, OsStr::new("Xvfb"), &[":90".into()]);
        assert_eq!(plain.get_program(), "Xvfb");
        let as_user = wrapped(Some("agent"), OsStr::new("Xvfb"), &[":90".into()]);
        assert_eq!(as_user.get_program(), "sudo");
        let args: Vec<_> = as_user.get_args().collect();
        assert_eq!(args, vec!["-n", "-u", "agent", "--", "Xvfb", ":90"]);
    }
}
