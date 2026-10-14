//! The Linux Wayland desktop backend of a workstation: the owner's own screen and input, through the XDG desktop
//! portals.
//!
//! It implements [`apz_desktop::Desktop`] for shared sessions (`SessionKind::Shared`), which are the owner's own
//! session. The screen comes from the ScreenCast portal's PipeWire stream, and the pointer and keyboard from the
//! RemoteDesktop portal. Both are one portal session, so the owner approves them in one dialog. The approval is
//! remembered as a restore token in the backend's state directory, so the owner is asked once, not every time.
//!
//! What this backend does not do:
//!
//! - Pixel sessions. A Wayland compositor gives no private screen through the portals, so `open_session(Pixel)`
//!   is refused. A private screen comes from a virtual display (the X11 or Xvfb backend).
//! - The UI tree, UI actions and app listing. Those belong to the AT-SPI backend and the desktop-entry listing;
//!   the methods here refuse with `Unsupported` so the daemon can route them there.
//! - Owner activity. The portals report none, so [`Desktop::owner_active`] is always `false`. The daemon uses the
//!   session's idle time from logind when it has it.
//!
//! The crate compiles to nothing off Linux.
//!
//! The screen's frames come from PipeWire, which is built only with the `pipewire` feature (it needs the PipeWire
//! headers and libclang). Without it a session still opens and takes input, but `capture` is refused as
//! unsupported.

#![cfg(target_os = "linux")]

#[cfg(feature = "pipewire")]
mod capture;
#[cfg(not(feature = "pipewire"))]
mod capture {
    use std::os::fd::OwnedFd;

    use apz_desktop::{DesktopError, Frame, Result};

    /// The reason a frame is refused. It names the feature that builds the capture.
    const NO_CAPTURE: &str =
        "this build has no PipeWire capture (build with the `pipewire` feature)";

    /// Stands in for the PipeWire capture. The portal's PipeWire remote is not read, so it is closed when dropped.
    pub struct Capture;

    impl Capture {
        pub fn start(_fd: OwnedFd, _node: u32) -> Result<Self> {
            Ok(Self)
        }

        pub fn latest(&self) -> Result<Frame> {
            Err(DesktopError::Unsupported(NO_CAPTURE.into()))
        }
    }
}

mod keysym;
// The pixel converters are used by the PipeWire capture only. They need no PipeWire types, so they compile in every
// build and their tests run everywhere.
#[cfg_attr(not(feature = "pipewire"), allow(dead_code))]
mod pixels;
mod portal;
mod restore;

use std::collections::BTreeSet;
use std::path::PathBuf;

use apz_desktop::{
    AppInfo, Desktop, DesktopError, Frame, Result, SessionSpec, UiNode, parse_combo, scale,
};
use apz_station::{Button, InputAction, Selector, SessionId, SessionKind, UiAction};
use ashpd::desktop::remote_desktop::Axis;

use keysym::{button_code, key_keysym, modifier_keysyms, text_keysyms};
use portal::Portal;
use restore::RestoreToken;

/// The reason a pixel session is refused. It names the backend that serves private sessions.
const PIXEL_REFUSED: &str = "use a virtual display (X11/Xvfb backend) for private sessions";

/// The Wayland backend. Shared sessions share one portal session, which starts with the first of them and ends
/// with the last.
pub struct WaylandDesktop {
    tokens: RestoreToken,
    sessions: BTreeSet<SessionId>,
    next: SessionId,
    portal: Option<Portal>,
}

impl WaylandDesktop {
    /// A backend whose restore token lives in `state_dir`, the state directory the daemon gives it. Nothing is
    /// touched until a session opens.
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        let state_dir: PathBuf = state_dir.into();
        Self {
            tokens: RestoreToken::in_dir(&state_dir),
            sessions: BTreeSet::new(),
            next: 1,
            portal: None,
        }
    }

    /// The live portal of an open session.
    fn portal_for(&self, id: SessionId) -> Result<&Portal> {
        if !self.sessions.contains(&id) {
            return Err(DesktopError::UnknownSession(id));
        }
        self.portal.as_ref().ok_or(DesktopError::UnknownSession(id))
    }

    fn close_portal(&mut self) {
        if let Some(portal) = self.portal.take() {
            portal.close();
        }
    }
}

impl Drop for WaylandDesktop {
    fn drop(&mut self) {
        self.close_portal();
    }
}

impl Desktop for WaylandDesktop {
    /// Opens a shared session. The width and height in the spec are ignored: a shared session is the owner's
    /// whole screen, at the size the compositor gives it.
    fn open_session(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        match spec.kind {
            SessionKind::Pixel => return Err(DesktopError::Unsupported(PIXEL_REFUSED.into())),
            SessionKind::Shared => {}
        }
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return Err(DesktopError::Unsupported(
                "this is not a Wayland session; the X11 backend serves it".into(),
            ));
        }
        if self.portal.is_none() {
            let opened = Portal::open(self.tokens.load().as_deref())?;
            if let Some(token) = &opened.restore_token {
                // A failed save only means the owner is asked again next time.
                let _ = self.tokens.save(token);
            }
            self.portal = Some(opened.portal);
        }
        let id = self.next;
        self.next += 1;
        self.sessions.insert(id);
        Ok(id)
    }

    fn close_session(&mut self, id: SessionId) -> Result<()> {
        if !self.sessions.remove(&id) {
            return Err(DesktopError::UnknownSession(id));
        }
        if self.sessions.is_empty() {
            self.close_portal();
        }
        Ok(())
    }

    /// The screen's latest frame. The compositor sends it in device pixels; on a scaled display it is reduced to
    /// logical pixels so that the coordinates of the input calls match the frame.
    fn capture(&mut self, id: SessionId) -> Result<Frame> {
        let portal = self.portal_for(id)?;
        let frame = portal.latest_frame()?;
        match portal.logical_size() {
            Some((w, h)) if w > 0 && h > 0 && (w, h) != (frame.w, frame.h) => {
                scale(&frame, w as f32 / frame.w as f32)
            }
            _ => Ok(frame),
        }
    }

    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()> {
        if let InputAction::Click { count: 0, .. } = action {
            return Err(DesktopError::Invalid(
                "a click presses at least once".into(),
            ));
        }
        let portal = self.portal_for(id)?;
        match action {
            InputAction::Move { x, y } => portal.motion(*x, *y),
            InputAction::Click {
                x,
                y,
                button,
                count,
            } => {
                portal.motion(*x, *y)?;
                let code = button_code(*button);
                for _ in 0..*count {
                    portal.button(code, true)?;
                    portal.button(code, false)?;
                }
                Ok(())
            }
            InputAction::Down { button } => portal.button(button_code(*button), true),
            InputAction::Up { button } => portal.button(button_code(*button), false),
            InputAction::Scroll { x, y, dx, dy } => {
                portal.motion(*x, *y)?;
                if *dy != 0 {
                    portal.scroll(Axis::Vertical, *dy)?;
                }
                if *dx != 0 {
                    portal.scroll(Axis::Horizontal, *dx)?;
                }
                Ok(())
            }
            InputAction::Type { text } => {
                for sym in text_keysyms(text)? {
                    portal.key(sym, true)?;
                    portal.key(sym, false)?;
                }
                Ok(())
            }
            InputAction::Key { combo } => {
                let combo = parse_combo(combo)?;
                press_combo(portal, &modifier_keysyms(combo.mods), key_keysym(combo.key))
            }
            InputAction::Drag { from, to } => {
                portal.motion(from.0, from.1)?;
                portal.button(button_code(Button::Left), true)?;
                portal.motion(to.0, to.1)?;
                portal.button(button_code(Button::Left), false)
            }
        }
    }

    fn ui_tree(&mut self, _id: SessionId, _app: Option<&str>, _depth: u8) -> Result<UiNode> {
        Err(DesktopError::Unsupported(
            "the UI tree is read by the AT-SPI backend, not through the portals".into(),
        ))
    }

    fn ui_act(&mut self, _id: SessionId, _selector: &Selector, _action: &UiAction) -> Result<()> {
        Err(DesktopError::Unsupported(
            "UI actions go through the AT-SPI backend, not the portals".into(),
        ))
    }

    fn apps(&mut self) -> Result<Vec<AppInfo>> {
        Err(DesktopError::Unsupported(
            "the app list comes from the desktop-entry listing, not the portals".into(),
        ))
    }

    fn launch(&mut self, _id: SessionId, _app: &str, _args: &[String]) -> Result<()> {
        Err(DesktopError::Unsupported(
            "apps are launched by the desktop-entry backend, not through the portals".into(),
        ))
    }

    /// Always `false`: the portals report no owner input. See the crate documentation.
    fn owner_active(&self, _id: SessionId) -> bool {
        false
    }
}

/// Presses the modifiers, presses and releases the key, then releases the modifiers. The modifiers are released
/// even when the key fails, so a failed chord does not leave a key held down.
fn press_combo(portal: &Portal, modifiers: &[u32], key: u32) -> Result<()> {
    let pressed = modifiers
        .iter()
        .try_for_each(|modifier| portal.key(*modifier, true));
    let chord = pressed.and_then(|()| {
        portal.key(key, true)?;
        portal.key(key, false)
    });
    let released = modifiers
        .iter()
        .rev()
        .try_for_each(|modifier| portal.key(*modifier, false));
    chord.and(released)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: SessionKind) -> SessionSpec {
        SessionSpec {
            agent: 1,
            kind,
            width: 1280,
            height: 720,
        }
    }

    #[test]
    fn a_pixel_session_is_refused_with_the_virtual_display_reason() {
        let mut desktop = WaylandDesktop::new(std::env::temp_dir());
        match desktop.open_session(&spec(SessionKind::Pixel)) {
            Err(DesktopError::Unsupported(reason)) => assert_eq!(reason, PIXEL_REFUSED),
            other => panic!("a pixel session must be unsupported, got {other:?}"),
        }
    }

    #[cfg(not(feature = "pipewire"))]
    #[test]
    fn without_the_pipewire_feature_a_frame_is_unsupported_and_names_the_feature() {
        let fd = std::os::fd::OwnedFd::from(std::fs::File::open("/dev/null").expect("opens"));
        let capture = capture::Capture::start(fd, 1).expect("the session starts without a stream");
        match capture.latest() {
            Err(DesktopError::Unsupported(reason)) => {
                assert!(reason.contains("`pipewire` feature"))
            }
            other => panic!("a frame must be unsupported without PipeWire, got {other:?}"),
        }
    }

    #[test]
    fn the_tree_actions_and_apps_are_unsupported_here() {
        let mut desktop = WaylandDesktop::new(std::env::temp_dir());
        assert!(matches!(
            desktop.ui_tree(1, None, 3),
            Err(DesktopError::Unsupported(_))
        ));
        assert!(matches!(
            desktop.ui_act(1, &Selector::default(), &UiAction::Invoke),
            Err(DesktopError::Unsupported(_))
        ));
        assert!(matches!(desktop.apps(), Err(DesktopError::Unsupported(_))));
        assert!(matches!(
            desktop.launch(1, "calc", &[]),
            Err(DesktopError::Unsupported(_))
        ));
    }

    #[test]
    fn a_session_nobody_opened_is_unknown_and_nothing_is_owner_active() {
        let mut desktop = WaylandDesktop::new(std::env::temp_dir());
        assert!(matches!(
            desktop.capture(7),
            Err(DesktopError::UnknownSession(7))
        ));
        assert!(matches!(
            desktop.input(7, &InputAction::Move { x: 1, y: 1 }),
            Err(DesktopError::UnknownSession(7))
        ));
        assert!(matches!(
            desktop.close_session(7),
            Err(DesktopError::UnknownSession(7))
        ));
        assert!(!desktop.owner_active(7));
    }

    #[test]
    fn a_click_needs_at_least_one_press_and_that_is_checked_before_the_session() {
        let mut desktop = WaylandDesktop::new(std::env::temp_dir());
        assert!(matches!(
            desktop.input(
                1,
                &InputAction::Click {
                    x: 0,
                    y: 0,
                    button: Button::Left,
                    count: 0,
                }
            ),
            Err(DesktopError::Invalid(_))
        ));
    }

    /// Manual check on a real Wayland desktop. It asks the owner, through the portal's own dialog, to share the
    /// screen and the keyboard and pointer, captures one frame, and closes the session. It never runs in CI.
    ///
    /// Run it by hand, with a scratch directory that is not the owner's configuration:
    ///
    /// ```text
    /// APZ_WAYLAND_STATE_DIR=$HOME/scratch/wayland-test \
    ///     cargo test -p apz-desktop-wayland --features pipewire -- --ignored --nocapture manual_session_captures_one_frame
    /// ```
    #[test]
    #[ignore = "asks the owner through the desktop portal dialog: run by hand"]
    fn manual_session_captures_one_frame() {
        let state = std::env::var_os("APZ_WAYLAND_STATE_DIR")
            .expect("set APZ_WAYLAND_STATE_DIR to a scratch directory");
        let mut desktop = WaylandDesktop::new(state);
        let id = desktop
            .open_session(&spec(SessionKind::Shared))
            .expect("the owner approves the portal dialog");
        let frame = desktop.capture(id).expect("a frame arrives");
        println!("captured {}x{}", frame.w, frame.h);
        desktop.close_session(id).expect("closes");
    }
}
