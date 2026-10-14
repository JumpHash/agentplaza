//! The X11 backend: the [`Desktop`] the workstation daemon serves, over its sessions.

use std::collections::BTreeMap;
use std::path::PathBuf;

use apz_desktop::{AppInfo, Desktop, DesktopError, Frame, Result, SessionSpec, UiNode};
use apz_station::{InputAction, Selector, SessionId, SessionKind, UiAction};

use crate::apps::{DesktopEntry, application_dirs, expand_exec, find, scan};
use crate::display::DisplayPaths;
use crate::session::{MAX_SIDE, Session};

/// The configuration of the backend. [`X11Config::default`] is what a plaza machine uses unless the owner
/// set something: `Xvfb` from PATH, no window manager, the daemon's own user, the owner's `DISPLAY`.
#[derive(Debug, Clone)]
pub struct X11Config {
    /// The X server binary. Default `Xvfb`, found on PATH.
    pub xvfb: PathBuf,
    /// The window manager a pixel session starts, as argv. Default none.
    pub window_manager: Option<Vec<String>>,
    /// The OS user that pixel sessions and their apps run as, through `sudo -n -u <user>`. Default none: the
    /// daemon's own user. The daemon's user needs a sudoers rule that runs commands as this user without a
    /// password; `sudo -n` fails the start rather than prompt. Shared sessions never change user.
    pub run_as: Option<String>,
    /// The owner's display for shared sessions. Default none: `DISPLAY` when the session opens.
    pub owner_display: Option<String>,
    /// Where X keeps its lock files and sockets. Default `/tmp` and `/tmp/.X11-unix`.
    pub display_paths: DisplayPaths,
    /// The directories `.desktop` entries are read from, in precedence order. Default none: the XDG
    /// application directories.
    pub application_dirs: Option<Vec<PathBuf>>,
}

impl Default for X11Config {
    fn default() -> Self {
        Self {
            xvfb: PathBuf::from("Xvfb"),
            window_manager: None,
            run_as: None,
            owner_display: None,
            display_paths: DisplayPaths::default(),
            application_dirs: None,
        }
    }
}

/// The accessibility side of the desktop: the UI tree and UI actions of a session's display. The AT-SPI crate
/// implements it; the daemon gives it to [`X11Desktop::with_ui`]. Without one, the UI calls return
/// `Unsupported`.
pub trait UiHook: Send {
    /// The UI tree of the apps on `display`, or of `app`'s windows, down to `depth` levels.
    fn ui_tree(&mut self, display: &str, app: Option<&str>, depth: u8) -> Result<UiNode>;
    /// An accessibility action on the control `selector` picks on `display`.
    fn ui_act(&mut self, display: &str, selector: &Selector, action: &UiAction) -> Result<()>;
}

/// The X11 desktop backend. See the crate documentation.
pub struct X11Desktop {
    config: X11Config,
    sessions: BTreeMap<SessionId, Session>,
    next: SessionId,
    ui: Option<Box<dyn UiHook>>,
}

impl X11Desktop {
    pub fn new(config: X11Config) -> Self {
        Self {
            config,
            sessions: BTreeMap::new(),
            next: 1,
            ui: None,
        }
    }

    /// Adds the accessibility side: the UI tree and UI actions go to `hook`.
    pub fn with_ui(mut self, hook: Box<dyn UiHook>) -> Self {
        self.ui = Some(hook);
        self
    }

    /// The display name a session's clients use (`:N`, or the owner's `DISPLAY`).
    pub fn display_of(&self, id: SessionId) -> Option<&str> {
        self.sessions
            .get(&id)
            .map(|session| session.display.as_str())
    }

    /// The pid of a pixel session's X server.
    pub fn server_pid(&self, id: SessionId) -> Option<u32> {
        self.sessions.get(&id).and_then(Session::server_pid)
    }

    fn session(&self, id: SessionId) -> Result<&Session> {
        self.sessions
            .get(&id)
            .ok_or(DesktopError::UnknownSession(id))
    }

    fn session_mut(&mut self, id: SessionId) -> Result<&mut Session> {
        self.sessions
            .get_mut(&id)
            .ok_or(DesktopError::UnknownSession(id))
    }

    fn entries(&self) -> Vec<DesktopEntry> {
        let dirs = self.config.application_dirs.clone().unwrap_or_else(|| {
            application_dirs(
                std::env::var_os("HOME").map(PathBuf::from).as_deref(),
                std::env::var("XDG_DATA_HOME").ok().as_deref(),
                std::env::var("XDG_DATA_DIRS").ok().as_deref(),
            )
        });
        scan(&dirs)
    }
}

impl Desktop for X11Desktop {
    fn open_session(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        let session = match spec.kind {
            SessionKind::Pixel => {
                let sized = |side: u32| (1..=MAX_SIDE).contains(&side);
                if !sized(spec.width) || !sized(spec.height) {
                    return Err(DesktopError::Invalid(format!(
                        "a session is 1 to {MAX_SIDE} points a side, not {}x{}",
                        spec.width, spec.height
                    )));
                }
                Session::start_pixel(&self.config, spec)?
            }
            SessionKind::Shared => {
                // A shared session is the owner's display as it is: its size is the display's, not the spec's.
                let display = self
                    .config
                    .owner_display
                    .clone()
                    .or_else(|| std::env::var("DISPLAY").ok())
                    .filter(|display| !display.is_empty())
                    .ok_or_else(|| {
                        DesktopError::Unsupported(
                            "there is no X display to share: DISPLAY is unset".into(),
                        )
                    })?;
                Session::connect_shared(display)?
            }
        };
        let id = self.next;
        self.next += 1;
        self.sessions.insert(id, session);
        Ok(id)
    }

    fn close_session(&mut self, id: SessionId) -> Result<()> {
        // Dropping the session stops its processes (a pixel session's server, window manager and apps).
        self.sessions
            .remove(&id)
            .map(drop)
            .ok_or(DesktopError::UnknownSession(id))
    }

    fn capture(&mut self, id: SessionId) -> Result<Frame> {
        self.session_mut(id)?.capture()
    }

    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()> {
        self.session(id)?.input(action)
    }

    fn ui_tree(&mut self, id: SessionId, app: Option<&str>, depth: u8) -> Result<UiNode> {
        let display = self.session(id)?.display.clone();
        match self.ui.as_mut() {
            Some(hook) => hook.ui_tree(&display, app, depth),
            None => Err(DesktopError::Unsupported(
                "the UI tree needs the accessibility backend".into(),
            )),
        }
    }

    fn ui_act(&mut self, id: SessionId, selector: &Selector, action: &UiAction) -> Result<()> {
        let display = self.session(id)?.display.clone();
        match self.ui.as_mut() {
            Some(hook) => hook.ui_act(&display, selector, action),
            None => Err(DesktopError::Unsupported(
                "the UI actions need the accessibility backend".into(),
            )),
        }
    }

    fn apps(&mut self) -> Result<Vec<AppInfo>> {
        Ok(self
            .entries()
            .into_iter()
            .map(|entry| AppInfo {
                id: entry.id,
                name: entry.name,
                path: entry.path.display().to_string(),
                scripting: Vec::new(),
            })
            .collect())
    }

    fn launch(&mut self, id: SessionId, app: &str, args: &[String]) -> Result<()> {
        let entries = self.entries();
        let entry = find(&entries, app)
            .ok_or_else(|| DesktopError::NotFound(format!("no application \"{app}\"")))?;
        let argv = expand_exec(&entry.exec, args)?;
        self.session(id)?.spawn(&self.config, &argv)
    }

    fn owner_active(&self, id: SessionId) -> bool {
        self.sessions.get(&id).is_some_and(Session::owner_active)
    }
}
