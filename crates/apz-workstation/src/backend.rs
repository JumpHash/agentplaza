//! Which desktop backend this daemon uses, and what the machine can offer because of it.
//!
//! The platform backends are always built for their own platform (see the manifest). [`choose`] is pure: given the
//! probed environment and what this build has, it names the backend for the owner's screen. [`open`] constructs the
//! desktop the daemon serves: the owner's backend for shared sessions, the Linux virtual-display backend for pixel
//! sessions, and the owner's UI tree from AT-SPI.
//!
//! The in-memory [`FakeDesktop`] is used only when it is asked for (`run --fake-desktop` or
//! `APZ_WS_FAKE_DESKTOP=1`), never as a silent fallback. A machine with no usable backend gets [`Unavailable`], which
//! refuses every call with the reason.

use std::path::Path;

use apz_desktop::{Desktop, FakeDesktop};
use apz_station::Os;

use crate::route::{Route, Unavailable};

#[cfg(target_os = "linux")]
use crate::access::{Access, OwnerUi, WaylandOwner};
#[cfg(target_os = "linux")]
use apz_desktop_wayland::WaylandDesktop;
#[cfg(target_os = "linux")]
use apz_desktop_x11::{X11Config, X11Desktop};

/// Why a pixel session is refused when the machine has no virtual display.
const PIXEL_NOT_BUILT: &str =
    "pixel sessions need a virtual display: on Linux, install Xvfb; this platform has none yet";

/// The backend a daemon runs its screen operations on, for its owner's screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    X11,
    Wayland,
    MacOs,
    Windows,
    /// No backend on this machine. Every screen operation is refused with the reason.
    Unavailable,
    /// The in-memory desktop, only when asked for.
    Fake,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::X11 => "x11",
            Backend::Wayland => "wayland",
            Backend::MacOs => "macos",
            Backend::Windows => "windows",
            Backend::Unavailable => "unavailable",
            Backend::Fake => "fake",
        }
    }
}

/// The backends and accessibility support this build includes, by platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Built {
    pub x11: bool,
    pub wayland: bool,
    pub macos: bool,
    pub windows: bool,
    pub atspi: bool,
}

impl Built {
    /// The backends this build was compiled with: each platform's own.
    pub const fn here() -> Built {
        Built {
            x11: cfg!(target_os = "linux"),
            wayland: cfg!(target_os = "linux"),
            macos: cfg!(target_os = "macos"),
            windows: cfg!(windows),
            atspi: cfg!(target_os = "linux"),
        }
    }
}

/// What the environment says about the machine's display setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub os: Os,
    /// `DISPLAY` is set (an X server is reachable).
    pub display: bool,
    /// The session is Wayland.
    pub wayland: bool,
    /// The desktop portal answers on the session bus (or the bus would start it).
    pub portal: bool,
    /// A D-Bus session bus is reachable (AT-SPI needs one).
    pub dbus: bool,
    /// `Xvfb` is installed, so virtual displays are possible on Linux.
    pub xvfb: bool,
}

impl Probe {
    /// The probe of this machine.
    pub fn here() -> Probe {
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        let wayland = set("WAYLAND_DISPLAY")
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"));
        Probe {
            os: crate::facts::host_os(),
            display: set("DISPLAY"),
            wayland,
            portal: portal_reachable(),
            dbus: set("DBUS_SESSION_BUS_ADDRESS"),
            xvfb: on_path("Xvfb"),
        }
    }
}

/// Whether `name` is an executable file on `PATH`.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Whether the desktop portal is on the session bus: its service has an owner, or D-Bus would start it. Only the
/// bus names are asked about; no portal is called.
#[cfg(target_os = "linux")]
fn portal_reachable() -> bool {
    use zbus::blocking::{Connection, fdo::DBusProxy};
    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    let Ok(bus) = Connection::session() else {
        return false;
    };
    let Ok(proxy) = DBusProxy::new(&bus) else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(PORTAL) else {
        return false;
    };
    if proxy.name_has_owner(name).unwrap_or(false) {
        return true;
    }
    proxy
        .list_activatable_names()
        .map(|names| names.iter().any(|n| n.as_str() == PORTAL))
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn portal_reachable() -> bool {
    false
}

/// The backend for this machine's owner's screen. Linux: Wayland for a Wayland session with the portal reachable,
/// else X11 when there is a display. macOS and Windows: their own backend. Anything else is [`Backend::Unavailable`].
pub fn choose(probe: &Probe, built: Built) -> Backend {
    match probe.os {
        Os::Linux => {
            if probe.wayland && probe.portal && built.wayland {
                Backend::Wayland
            } else if probe.display && built.x11 {
                Backend::X11
            } else {
                Backend::Unavailable
            }
        }
        Os::Macos if built.macos => Backend::MacOs,
        Os::Windows if built.windows => Backend::Windows,
        _ => Backend::Unavailable,
    }
}

/// The backend a command uses: the fake desktop when the owner asked for it, else the machine's.
pub fn select(fake: bool, probe: &Probe, built: Built) -> Backend {
    if fake {
        Backend::Fake
    } else {
        choose(probe, built)
    }
}

/// Whether the fake desktop was asked for by the environment (`APZ_WS_FAKE_DESKTOP=1`).
pub fn fake_requested() -> bool {
    std::env::var("APZ_WS_FAKE_DESKTOP").is_ok_and(|v| v == "1")
}

/// Why this machine has no backend for its owner's screen, in words the hub can show.
pub fn unavailable_reason(probe: &Probe, built: Built) -> String {
    match probe.os {
        Os::Linux if probe.wayland && !probe.portal && !probe.display => {
            "a Wayland session with no desktop portal on the session bus and no X display: \
             start xdg-desktop-portal, or set DISPLAY"
                .to_string()
        }
        Os::Linux if !probe.display && !probe.wayland => {
            "no screen: DISPLAY and WAYLAND_DISPLAY are both unset".to_string()
        }
        Os::Linux if !built.x11 => "no X11 backend in this build".to_string(),
        Os::Linux => "no usable display backend".to_string(),
        Os::Macos => "the macOS backend is not built into this daemon".to_string(),
        Os::Windows => "the Windows backend is not built into this daemon".to_string(),
    }
}

/// Constructs the desktop for `backend`. Never fails: a backend that cannot start becomes [`Unavailable`] with the
/// reason, so the daemon still serves what it can (STOP ALL, files, runs).
pub fn open(backend: Backend, probe: &Probe, built: Built, state_dir: &Path) -> Box<dyn Desktop> {
    match backend {
        Backend::Fake => Box::new(FakeDesktop::new()),
        Backend::MacOs => macos_desktop(),
        Backend::Windows => windows_desktop(),
        Backend::X11 | Backend::Wayland | Backend::Unavailable => {
            open_linux(backend, probe, built, state_dir)
        }
    }
}

#[cfg(target_os = "linux")]
fn open_linux(backend: Backend, probe: &Probe, built: Built, state_dir: &Path) -> Box<dyn Desktop> {
    let owner_display = std::env::var("DISPLAY").unwrap_or_default();
    let shared: Box<dyn Desktop> = match backend {
        Backend::Wayland => Box::new(WaylandOwner::new(
            WaylandDesktop::new(state_dir),
            Access::from_env(),
        )),
        Backend::X11 => Box::new(x11(owner_display.clone())),
        _ => Box::new(Unavailable::new(unavailable_reason(probe, built))),
    };
    // Pixel sessions are always X11 on a virtual display, whatever the owner's backend is. Without Xvfb there is
    // none, and the route refuses them with the reason.
    let pixel: Option<Box<dyn Desktop>> =
        (probe.xvfb && built.x11).then(|| Box::new(x11(owner_display)) as Box<dyn Desktop>);
    Box::new(Route::new(shared, pixel, PIXEL_NOT_BUILT))
}

/// An X11 backend whose UI tree is AT-SPI on the owner's display.
#[cfg(target_os = "linux")]
fn x11(owner_display: String) -> X11Desktop {
    X11Desktop::new(X11Config::default())
        .with_ui(Box::new(OwnerUi::new(owner_display, Access::from_env())))
}

#[cfg(not(target_os = "linux"))]
fn open_linux(
    backend: Backend,
    probe: &Probe,
    built: Built,
    _state_dir: &Path,
) -> Box<dyn Desktop> {
    let _ = backend;
    Box::new(Unavailable::new(unavailable_reason(probe, built)))
}

#[cfg(target_os = "macos")]
fn macos_desktop() -> Box<dyn Desktop> {
    Box::new(Route::new(
        Box::new(apz_desktop_macos::MacDesktop::new()),
        None,
        PIXEL_NOT_BUILT,
    ))
}

#[cfg(not(target_os = "macos"))]
fn macos_desktop() -> Box<dyn Desktop> {
    Box::new(Unavailable::new("the macOS backend runs only on macOS"))
}

#[cfg(windows)]
fn windows_desktop() -> Box<dyn Desktop> {
    match apz_desktop_windows::WindowsDesktop::new() {
        Ok(desktop) => Box::new(Route::new(Box::new(desktop), None, PIXEL_NOT_BUILT)),
        Err(err) => Box::new(Unavailable::new(err.to_string())),
    }
}

#[cfg(not(windows))]
fn windows_desktop() -> Box<dyn Desktop> {
    Box::new(Unavailable::new("the Windows backend runs only on Windows"))
}

/// Whether pixel sessions have a backend: a Linux machine with Xvfb and the X11 backend. macOS and Windows have
/// none yet (their sessions are the owner's screen only).
pub fn virtual_displays(probe: &Probe, built: Built) -> bool {
    match probe.os {
        Os::Linux => probe.xvfb && built.x11,
        Os::Macos | Os::Windows => false,
    }
}

/// Whether the UI tree works: AT-SPI on Linux (with a session bus) for the X11 and Wayland backends, the
/// accessibility APIs on macOS and Windows.
pub fn ui_tree(probe: &Probe, backend: Backend, built: Built) -> bool {
    match probe.os {
        Os::Linux => {
            built.atspi && probe.dbus && matches!(backend, Backend::X11 | Backend::Wayland)
        }
        Os::Macos => backend == Backend::MacOs,
        Os::Windows => backend == Backend::Windows,
    }
}

/// How many applications the desktop reports. The fake desktop's list is invented, so it counts as none, and a
/// desktop that refuses the listing counts as none too.
pub fn apps_count(backend: Backend, desktop: &mut dyn Desktop) -> u32 {
    if backend == Backend::Fake {
        return 0;
    }
    desktop.apps().map(|apps| apps.len() as u32).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Built = Built {
        x11: false,
        wayland: false,
        macos: false,
        windows: false,
        atspi: false,
    };

    const BOTH: Built = Built {
        x11: true,
        wayland: true,
        macos: false,
        windows: false,
        atspi: true,
    };

    fn probe(os: Os) -> Probe {
        Probe {
            os,
            display: false,
            wayland: false,
            portal: false,
            dbus: false,
            xvfb: false,
        }
    }

    #[test]
    fn backend_choice_follows_the_built_backends_and_the_display() {
        // with_no_backend_built_nothing_is_chosen: with no backend built,
        // nothing is chosen on any OS.
        {
            for os in [Os::Linux, Os::Macos, Os::Windows] {
                let mut p = probe(os);
                p.display = true;
                p.wayland = true;
                p.portal = true;
                assert_eq!(choose(&p, NONE), Backend::Unavailable);
            }
        }
        // linux_picks_wayland_only_with_the_portal_and_x11_for_a_display:
        // Wayland needs the portal, X11 serves a display.
        {
            let mut wayland = probe(Os::Linux);
            wayland.wayland = true;
            wayland.display = true;
            wayland.portal = true;
            assert_eq!(choose(&wayland, BOTH), Backend::Wayland);

            wayland.portal = false;
            assert_eq!(
                choose(&wayland, BOTH),
                Backend::X11,
                "XWayland's DISPLAY serves when the portal is not reachable"
            );

            let mut x = probe(Os::Linux);
            x.display = true;
            assert_eq!(choose(&x, BOTH), Backend::X11);
        }
        // xvfb_alone_is_not_a_shared_screen: Xvfb alone is not a shared screen.
        {
            let mut xvfb = probe(Os::Linux);
            xvfb.xvfb = true;
            assert_eq!(
                choose(&xvfb, BOTH),
                Backend::Unavailable,
                "pixel sessions use Xvfb; the owner's screen needs DISPLAY or Wayland"
            );
            assert!(unavailable_reason(&xvfb, BOTH).contains("DISPLAY"));
        }
        // macos_and_windows_pick_their_own_backend: each OS picks its backend.
        {
            let built = Built {
                macos: true,
                windows: true,
                ..NONE
            };
            assert_eq!(choose(&probe(Os::Macos), built), Backend::MacOs);
            assert_eq!(choose(&probe(Os::Windows), built), Backend::Windows);
        }
    }

    #[test]
    fn fake_is_chosen_only_when_asked_for() {
        let mut p = probe(Os::Linux);
        p.display = true;
        assert_eq!(select(true, &p, BOTH), Backend::Fake);
        assert_eq!(select(false, &p, BOTH), Backend::X11);
    }

    #[test]
    fn virtual_displays_are_linux_with_xvfb_and_x11() {
        let mut xvfb = probe(Os::Linux);
        xvfb.xvfb = true;
        assert!(virtual_displays(&xvfb, BOTH));
        assert!(!virtual_displays(&xvfb, NONE));
        assert!(!virtual_displays(&probe(Os::Linux), BOTH));
        let macos = Built {
            macos: true,
            ..NONE
        };
        assert!(!virtual_displays(&probe(Os::Macos), macos));
        assert!(!virtual_displays(&probe(Os::Windows), macos));
    }

    #[test]
    fn the_ui_tree_needs_a_real_backend_and_the_platform_support() {
        let mut linux = probe(Os::Linux);
        linux.dbus = true;
        assert!(ui_tree(&linux, Backend::X11, BOTH));
        assert!(ui_tree(&linux, Backend::Wayland, BOTH));
        assert!(!ui_tree(&linux, Backend::X11, NONE), "no AT-SPI feature");
        assert!(!ui_tree(&linux, Backend::Unavailable, BOTH));
        assert!(!ui_tree(&linux, Backend::Fake, BOTH));
        assert!(ui_tree(&probe(Os::Macos), Backend::MacOs, NONE));
        assert!(!ui_tree(&probe(Os::Windows), Backend::Unavailable, NONE));
    }
}
