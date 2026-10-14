//! The Windows backend. COM is initialised in the multithreaded apartment, where the interfaces used here are
//! free-threaded, so the daemon may build the backend on one thread and run its request loop on another. The
//! thread that calls into the backend must have initialised COM itself, which the daemon's loop thread does.

mod apps;
mod capture;
mod input;
mod registry;
mod tree;
mod window;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use apz_desktop::{AppInfo, Desktop, DesktopError, Frame, Result, SessionSpec, UiNode};
use apz_station::{InputAction, Selector, SessionId, SessionKind, UiAction};
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationElement};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

use crate::{catalog, owner, policy};

/// How long an app listing is reused before the Start menu and the registry are read again.
const CATALOGUE_TTL: Duration = Duration::from_secs(60);

/// The Windows desktop: what [`apz_desktop::Desktop`] asks of Windows. Create it on the thread that will call it.
pub struct WindowsDesktop {
    uia: IUIAutomation,
    sessions: BTreeMap<SessionId, Session>,
    next: SessionId,
    /// The tick after our newest injected input, for [`owner::owner_is_active`].
    last_injected: Option<u32>,
    pointer: (i32, i32),
    catalogue: Option<(Instant, Vec<AppInfo>)>,
    /// Windows Server runs several sessions at once; the client editions run one.
    server: bool,
    /// Declared last, so COM stays initialised until every interface above is released.
    _com: ComApartment,
}

struct Session {
    capture: capture::Capturer,
}

/// This thread's COM apartment. It is uninitialised on drop when this value initialised it.
struct ComApartment(bool);

impl ComApartment {
    fn enter() -> Self {
        // SAFETY: no reserved pointer is passed.
        let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // A success status (including "already initialised") owns one reference; a failure owns none.
        Self(status.is_ok())
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the CoInitializeEx that succeeded in `enter`.
            unsafe { CoUninitialize() };
        }
    }
}

// SAFETY: the interfaces this value holds (the UI Automation client, the Direct3D device and its duplication, the
// staging textures) are free-threaded, since COM was initialised in the multithreaded apartment. The daemon's one
// request loop calls the backend from one thread at a time, so no two threads use a context at once.
unsafe impl Send for WindowsDesktop {}

impl WindowsDesktop {
    /// Starts the backend on this thread: COM, per-monitor DPI awareness (so screen points are the pixels the
    /// capture returns), and UI Automation.
    pub fn new() -> Result<Self> {
        let com = ComApartment::enter();
        // SAFETY: the awareness context is a constant. A process that set its own awareness keeps it, and the
        // call's failure is harmless.
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        // SAFETY: the class id is a constant; no outer object is passed.
        let uia: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
                .map_err(platform_error)?;
        let product = registry::string_value(
            HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\ProductOptions",
            "ProductType",
        );
        Ok(Self {
            uia,
            sessions: BTreeMap::new(),
            next: 1,
            last_injected: None,
            pointer: (0, 0),
            catalogue: None,
            server: product.is_some_and(|kind| policy::is_server_product_type(&kind)),
            _com: com,
        })
    }

    fn require(&self, id: SessionId) -> Result<()> {
        if self.sessions.contains_key(&id) {
            Ok(())
        } else {
            Err(DesktopError::UnknownSession(id))
        }
    }

    /// The installed apps, from a listing no older than [`CATALOGUE_TTL`].
    fn catalogue(&mut self) -> &[AppInfo] {
        let fresh = self
            .catalogue
            .as_ref()
            .is_some_and(|(read, _)| read.elapsed() < CATALOGUE_TTL);
        if !fresh {
            self.catalogue = Some((Instant::now(), apps::list()));
        }
        self.catalogue
            .as_ref()
            .map_or(&[][..], |(_, list)| list.as_slice())
    }

    fn find_app(&mut self, app: &str) -> Result<AppInfo> {
        self.catalogue()
            .iter()
            .find(|info| catalog::names_app(app, &info.name, &info.id))
            .cloned()
            .ok_or_else(|| DesktopError::NotFound(format!("no installed app named {app}")))
    }

    /// The root element for a call: the main window of `app` when given, else the foreground window.
    fn root(&mut self, app: Option<&str>) -> Result<IUIAutomationElement> {
        let hwnd = match app {
            None => window::foreground()?,
            Some(name) => {
                let executable = self
                    .find_app(name)
                    .map(|info| info.id)
                    .unwrap_or_else(|_| name.to_string());
                window::main_window_of(&executable)?
            }
        };
        // SAFETY: the window handle came from this thread's window queries.
        unsafe { self.uia.ElementFromHandle(hwnd) }.map_err(platform_error)
    }
}

impl Desktop for WindowsDesktop {
    fn open_session(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        match spec.kind {
            // TODO(workstations): on Windows Server, one pixel session per agent comes from the multi-session
            // path (an RDS session per agent). That is not built; until it is, the refusal below holds on every
            // edition.
            SessionKind::Pixel => Err(DesktopError::Unsupported(
                if self.server {
                    policy::SERVER_PIXEL_UNSUPPORTED
                } else {
                    policy::CLIENT_PIXEL_UNSUPPORTED
                }
                .into(),
            )),
            // The shared session is the interactive session this process runs in: the owner's screen. The size in
            // the spec does not apply to it.
            SessionKind::Shared => {
                let id = self.next;
                self.next += 1;
                self.sessions.insert(
                    id,
                    Session {
                        capture: capture::Capturer::default(),
                    },
                );
                Ok(id)
            }
        }
    }

    fn close_session(&mut self, id: SessionId) -> Result<()> {
        self.sessions
            .remove(&id)
            .map(|_| ())
            .ok_or(DesktopError::UnknownSession(id))
    }

    fn capture(&mut self, id: SessionId) -> Result<Frame> {
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or(DesktopError::UnknownSession(id))?;
        session.capture.grab()
    }

    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()> {
        self.require(id)?;
        self.send_action(action)
    }

    fn ui_tree(&mut self, id: SessionId, app: Option<&str>, depth: u8) -> Result<UiNode> {
        self.require(id)?;
        let root = self.root(app)?;
        tree::snapshot(&self.uia, &root, depth)
    }

    fn ui_act(&mut self, id: SessionId, selector: &Selector, action: &UiAction) -> Result<()> {
        self.require(id)?;
        let root = self.root(selector.app.as_deref())?;
        let element = tree::locate(&self.uia, &root, selector)?;
        tree::act(&element, action)
    }

    fn apps(&mut self) -> Result<Vec<AppInfo>> {
        Ok(self.catalogue().to_vec())
    }

    /// Brings the app's main window to the front (SetForegroundWindow). The app is named by its catalogue id or
    /// name. Windows lets a process take the foreground only from the foreground process, so this can be
    /// refused; the refusal comes back as the platform's error.
    fn focus(&mut self, id: &str) -> Result<()> {
        let info = self.find_app(id)?;
        let hwnd = window::main_window_of(&info.id)?;
        window::bring_to_front(hwnd)
    }

    fn launch(&mut self, id: SessionId, app: &str, args: &[String]) -> Result<()> {
        self.require(id)?;
        let info = self.find_app(app)?;
        apps::launch(&info.path, &catalog::command_line(args))
    }

    fn owner_active(&self, id: SessionId) -> bool {
        if !self.sessions.contains_key(&id) {
            return false;
        }
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is a valid LASTINPUTINFO with its size set.
        if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
            return false;
        }
        // SAFETY: GetTickCount takes no pointers.
        let now = unsafe { GetTickCount() };
        owner::owner_is_active(info.dwTime, self.last_injected, now)
    }
}

/// An error from a Windows call as the daemon reports it: access denied is a permission, anything else is the
/// platform's own failure.
pub(crate) fn platform_error(error: windows::core::Error) -> DesktopError {
    if error.code() == E_ACCESSDENIED {
        DesktopError::NotPermitted(error.to_string())
    } else {
        DesktopError::Platform(error.to_string())
    }
}
