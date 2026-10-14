//! Which desktop backend this daemon uses, and what the machine can offer because of it.
//!
//! The platform backends are separate crates, written in parallel (`apz-desktop-x11`, `apz-desktop-wayland`,
//! `apz-desktop-macos`, `apz-desktop-windows`, `apz-desktop-atspi`). Each one is behind a cargo feature of this
//! crate, off by default. [`choose`] is pure: given the probed environment and the features built in, it names
//! the backend. [`open`] constructs it. With no feature on, [`open`] builds the in-memory [`FakeDesktop`].

use apz_desktop::{Desktop, FakeDesktop};
use apz_station::Os;

/// The backend a daemon runs its screen operations on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    X11,
    Wayland,
    MacOs,
    Windows,
    /// The in-memory desktop: no real screen. Used when no backend is built in.
    Fake,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::X11 => "x11",
            Backend::Wayland => "wayland",
            Backend::MacOs => "macos",
            Backend::Windows => "windows",
            Backend::Fake => "fake",
        }
    }
}

/// The backends and accessibility support this build includes (the cargo features).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Built {
    pub x11: bool,
    pub wayland: bool,
    pub macos: bool,
    pub windows: bool,
    pub atspi: bool,
}

impl Built {
    /// The features this build was compiled with.
    pub const fn here() -> Built {
        Built {
            x11: cfg!(feature = "x11"),
            wayland: cfg!(feature = "wayland"),
            macos: cfg!(feature = "macos"),
            windows: cfg!(feature = "windows"),
            atspi: cfg!(feature = "atspi"),
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

/// The backend for this machine. Linux: Wayland for a Wayland session, else X11 when there is a display or
/// Xvfb. macOS and Windows: their own backend. Anything without a built backend is the fake desktop.
pub fn choose(probe: &Probe, built: Built) -> Backend {
    match probe.os {
        Os::Linux => {
            if probe.wayland && built.wayland {
                Backend::Wayland
            } else if (probe.display || probe.xvfb) && built.x11 {
                Backend::X11
            } else {
                Backend::Fake
            }
        }
        Os::Macos if built.macos => Backend::MacOs,
        Os::Windows if built.windows => Backend::Windows,
        _ => Backend::Fake,
    }
}

/// Whether the machine can make virtual displays, so pixel sessions do not take the owner's screen. Linux
/// needs Xvfb, macOS its backend, Windows client editions never (one interactive session).
pub fn virtual_displays(probe: &Probe, backend: Backend) -> bool {
    match probe.os {
        Os::Linux => probe.xvfb && backend != Backend::Fake,
        Os::Macos => backend == Backend::MacOs,
        Os::Windows => false,
    }
}

/// Whether the UI tree works: AT-SPI on Linux (with a session bus), the accessibility APIs on macOS and Windows.
pub fn ui_tree(probe: &Probe, backend: Backend, built: Built) -> bool {
    match probe.os {
        Os::Linux => built.atspi && probe.dbus && backend != Backend::Fake,
        Os::Macos => backend == Backend::MacOs,
        Os::Windows => backend == Backend::Windows,
    }
}

/// Constructs the backend. A backend whose feature is off is never chosen, so only the fake desktop is built
/// here until the platform crates are wired in.
pub fn open(backend: Backend) -> Box<dyn Desktop> {
    let _ = backend;
    Box::new(FakeDesktop::new())
}

/// How many applications the desktop reports. The fake desktop's list is invented, so it counts as none.
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

    fn probe(os: Os) -> Probe {
        Probe {
            os,
            display: false,
            wayland: false,
            dbus: false,
            xvfb: false,
        }
    }

    #[test]
    fn with_no_backend_built_everything_is_the_fake_desktop() {
        for os in [Os::Linux, Os::Macos, Os::Windows] {
            let mut p = probe(os);
            p.display = true;
            p.wayland = true;
            assert_eq!(choose(&p, NONE), Backend::Fake);
        }
    }

    #[test]
    fn linux_picks_wayland_for_a_wayland_session_and_x11_for_a_display() {
        let built = Built {
            x11: true,
            wayland: true,
            ..NONE
        };
        let mut wayland = probe(Os::Linux);
        wayland.wayland = true;
        wayland.display = true;
        assert_eq!(choose(&wayland, built), Backend::Wayland);

        let mut x = probe(Os::Linux);
        x.display = true;
        assert_eq!(choose(&x, built), Backend::X11);

        let mut xvfb = probe(Os::Linux);
        xvfb.xvfb = true;
        assert_eq!(choose(&xvfb, built), Backend::X11, "Xvfb alone is enough");

        assert_eq!(choose(&probe(Os::Linux), built), Backend::Fake);
    }

    #[test]
    fn macos_and_windows_pick_their_own_backend() {
        let built = Built {
            macos: true,
            windows: true,
            ..NONE
        };
        assert_eq!(choose(&probe(Os::Macos), built), Backend::MacOs);
        assert_eq!(choose(&probe(Os::Windows), built), Backend::Windows);
    }

    #[test]
    fn virtual_displays_follow_the_platform_rules() {
        let mut xvfb = probe(Os::Linux);
        xvfb.xvfb = true;
        assert!(virtual_displays(&xvfb, Backend::X11));
        assert!(!virtual_displays(&xvfb, Backend::Fake));
        assert!(!virtual_displays(&probe(Os::Linux), Backend::X11));
        assert!(virtual_displays(&probe(Os::Macos), Backend::MacOs));
        assert!(!virtual_displays(&probe(Os::Macos), Backend::Fake));
        assert!(!virtual_displays(&probe(Os::Windows), Backend::Windows));
    }

    #[test]
    fn the_ui_tree_needs_a_real_backend_and_the_platform_support() {
        let atspi = Built {
            atspi: true,
            ..NONE
        };
        let mut linux = probe(Os::Linux);
        linux.dbus = true;
        assert!(ui_tree(&linux, Backend::X11, atspi));
        assert!(!ui_tree(&linux, Backend::X11, NONE), "no AT-SPI feature");
        assert!(!ui_tree(&linux, Backend::Fake, atspi));
        assert!(ui_tree(&probe(Os::Macos), Backend::MacOs, NONE));
        assert!(!ui_tree(&probe(Os::Windows), Backend::Fake, NONE));
    }

    #[test]
    fn the_fake_desktop_counts_no_applications() {
        let mut desktop = open(Backend::Fake);
        assert_eq!(apps_count(Backend::Fake, desktop.as_mut()), 0);
    }
}
