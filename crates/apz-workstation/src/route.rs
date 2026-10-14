//! The desktop a daemon serves, built from its lanes.
//!
//! A station has two kinds of screen session, and each is served by its own backend:
//! - a **shared** session is the owner's own screen, served by the owner's backend (X11 or Wayland on Linux,
//!   macOS, Windows);
//! - a **pixel** session is a private virtual display, served by a second backend (X11 on Xvfb on Linux), or
//!   refused when the machine has none.
//!
//! [`Route`] presents the two as one [`Desktop`]. Each backend numbers its sessions from 1, so [`Route`] hands the
//! daemon its own ids and keeps the map to the backend session behind each one. [`Unavailable`] is the desktop of a
//! machine with no usable backend: every call is refused with the reason, so nothing is faked.

use std::collections::BTreeMap;

use apz_desktop::{AppInfo, Desktop, DesktopError, Frame, Result, SessionSpec, UiNode};
use apz_station::{InputAction, Selector, SessionId, SessionKind, UiAction};

/// Two lanes, one desktop. See the module docs.
pub struct Route {
    shared: Box<dyn Desktop>,
    pixel: Option<Box<dyn Desktop>>,
    /// Why a pixel session is refused when there is no pixel lane.
    no_pixel: String,
    /// The daemon's session id to the session's kind and its id on that lane's backend.
    sessions: BTreeMap<SessionId, (SessionKind, SessionId)>,
    next: SessionId,
}

impl Route {
    /// A route whose shared sessions go to `shared` and pixel sessions to `pixel`. Without a pixel lane, a pixel
    /// session is refused with `no_pixel`.
    pub fn new(
        shared: Box<dyn Desktop>,
        pixel: Option<Box<dyn Desktop>>,
        no_pixel: impl Into<String>,
    ) -> Self {
        Self {
            shared,
            pixel,
            no_pixel: no_pixel.into(),
            sessions: BTreeMap::new(),
            next: 1,
        }
    }

    fn lane(&mut self, kind: SessionKind) -> Result<&mut dyn Desktop> {
        match kind {
            SessionKind::Shared => Ok(self.shared.as_mut()),
            SessionKind::Pixel => match self.pixel.as_mut() {
                Some(desktop) => Ok(desktop.as_mut()),
                None => Err(DesktopError::Unsupported(self.no_pixel.clone())),
            },
        }
    }

    /// The lane and the backend's id for a daemon session.
    fn route(&mut self, id: SessionId) -> Result<(&mut dyn Desktop, SessionId)> {
        let &(kind, inner) = self
            .sessions
            .get(&id)
            .ok_or(DesktopError::UnknownSession(id))?;
        Ok((self.lane(kind)?, inner))
    }
}

impl Desktop for Route {
    fn open_session(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        let inner = self.lane(spec.kind)?.open_session(spec)?;
        let id = self.next;
        self.next += 1;
        self.sessions.insert(id, (spec.kind, inner));
        Ok(id)
    }

    fn close_session(&mut self, id: SessionId) -> Result<()> {
        let &(kind, inner) = self
            .sessions
            .get(&id)
            .ok_or(DesktopError::UnknownSession(id))?;
        let result = self.lane(kind)?.close_session(inner);
        self.sessions.remove(&id);
        result
    }

    fn capture(&mut self, id: SessionId) -> Result<Frame> {
        let (desktop, inner) = self.route(id)?;
        desktop.capture(inner)
    }

    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()> {
        let (desktop, inner) = self.route(id)?;
        desktop.input(inner, action)
    }

    fn ui_tree(&mut self, id: SessionId, app: Option<&str>, depth: u8) -> Result<UiNode> {
        let (desktop, inner) = self.route(id)?;
        desktop.ui_tree(inner, app, depth)
    }

    fn ui_act(&mut self, id: SessionId, selector: &Selector, action: &UiAction) -> Result<()> {
        let (desktop, inner) = self.route(id)?;
        desktop.ui_act(inner, selector, action)
    }

    /// The owner's apps when the owner's backend lists them, else the pixel backend's list (the desktop-entry
    /// listing of the machine, which a Wayland backend does not give).
    fn apps(&mut self) -> Result<Vec<AppInfo>> {
        match self.shared.apps() {
            Err(DesktopError::Unsupported(_)) => match self.pixel.as_mut() {
                Some(pixel) => pixel.apps(),
                None => Err(DesktopError::Unsupported(
                    "this machine lists no applications".into(),
                )),
            },
            other => other,
        }
    }

    fn launch(&mut self, id: SessionId, app: &str, args: &[String]) -> Result<()> {
        let (desktop, inner) = self.route(id)?;
        desktop.launch(inner, app, args)
    }

    fn focus(&mut self, id: &str) -> Result<()> {
        self.shared.focus(id)
    }

    fn quit(&mut self, id: &str, force: bool) -> Result<()> {
        self.shared.quit(id, force)
    }

    fn owner_active(&self, id: SessionId) -> bool {
        match self.sessions.get(&id) {
            Some(&(SessionKind::Shared, inner)) => self.shared.owner_active(inner),
            Some(&(SessionKind::Pixel, inner)) => self
                .pixel
                .as_ref()
                .is_some_and(|pixel| pixel.owner_active(inner)),
            None => false,
        }
    }
}

/// The desktop of a machine with no usable backend. Every call is refused with the reason it has none.
pub struct Unavailable {
    reason: String,
}

impl Unavailable {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    fn refusal(&self) -> DesktopError {
        DesktopError::Unsupported(self.reason.clone())
    }
}

impl Desktop for Unavailable {
    fn open_session(&mut self, _spec: &SessionSpec) -> Result<SessionId> {
        Err(self.refusal())
    }

    fn close_session(&mut self, id: SessionId) -> Result<()> {
        Err(DesktopError::UnknownSession(id))
    }

    fn capture(&mut self, _id: SessionId) -> Result<Frame> {
        Err(self.refusal())
    }

    fn input(&mut self, _id: SessionId, _action: &InputAction) -> Result<()> {
        Err(self.refusal())
    }

    fn ui_tree(&mut self, _id: SessionId, _app: Option<&str>, _depth: u8) -> Result<UiNode> {
        Err(self.refusal())
    }

    fn ui_act(&mut self, _id: SessionId, _selector: &Selector, _action: &UiAction) -> Result<()> {
        Err(self.refusal())
    }

    fn apps(&mut self) -> Result<Vec<AppInfo>> {
        Err(self.refusal())
    }

    fn launch(&mut self, _id: SessionId, _app: &str, _args: &[String]) -> Result<()> {
        Err(self.refusal())
    }

    fn focus(&mut self, _id: &str) -> Result<()> {
        Err(self.refusal())
    }

    fn quit(&mut self, _id: &str, _force: bool) -> Result<()> {
        Err(self.refusal())
    }

    fn owner_active(&self, _id: SessionId) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_desktop::FakeDesktop;

    fn spec(kind: SessionKind) -> SessionSpec {
        SessionSpec {
            agent: 1,
            kind,
            width: 640,
            height: 480,
        }
    }

    #[test]
    fn each_lane_numbers_its_own_sessions_and_the_route_keeps_them_apart() {
        let mut route = Route::new(
            Box::new(FakeDesktop::new()),
            Some(Box::new(FakeDesktop::new())),
            "no pixel",
        );
        let shared = route.open_session(&spec(SessionKind::Shared)).unwrap();
        let pixel = route.open_session(&spec(SessionKind::Pixel)).unwrap();
        assert_ne!(shared, pixel, "the route ids are distinct");
        assert!(route.capture(shared).is_ok());
        assert!(route.capture(pixel).is_ok());
        route.close_session(pixel).unwrap();
        assert!(matches!(
            route.capture(pixel),
            Err(DesktopError::UnknownSession(_))
        ));
        assert!(
            route.capture(shared).is_ok(),
            "closing one leaves the other"
        );
    }

    #[test]
    fn a_pixel_session_without_a_pixel_lane_is_refused_with_the_reason() {
        let mut route = Route::new(Box::new(FakeDesktop::new()), None, "install Xvfb");
        match route.open_session(&spec(SessionKind::Pixel)) {
            Err(DesktopError::Unsupported(reason)) => assert_eq!(reason, "install Xvfb"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_session_is_refused() {
        let mut route = Route::new(Box::new(FakeDesktop::new()), None, "none");
        assert!(matches!(
            route.capture(99),
            Err(DesktopError::UnknownSession(99))
        ));
        assert!(!route.owner_active(99));
    }

    #[test]
    fn unavailable_refuses_everything_with_its_reason() {
        let mut desktop = Unavailable::new("no display");
        assert!(matches!(
            desktop.apps(),
            Err(DesktopError::Unsupported(ref r)) if r == "no display"
        ));
        assert!(matches!(
            desktop.open_session(&spec(SessionKind::Shared)),
            Err(DesktopError::Unsupported(_))
        ));
        assert!(!desktop.owner_active(1));
    }
}
