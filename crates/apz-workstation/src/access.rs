//! The owner's accessibility tree on Linux: AT-SPI, read from the owner's session bus.
//!
//! The X11 backend takes it as its [`UiHook`], and the Wayland backend (whose portals give no tree) is wrapped in
//! [`WaylandOwner`]. Only the owner's own display or screen is answered: a pixel session is a different desktop,
//! and the owner's tree would show the owner's windows, not the agent's.

use apz_desktop::{Desktop, DesktopError, Frame, Result, SessionSpec, UiNode};
use apz_desktop_atspi::AtspiTree;
use apz_desktop_wayland::WaylandDesktop;
use apz_desktop_x11::UiHook;
use apz_station::{InputAction, Selector, SessionId, UiAction};

/// The owner's AT-SPI tree, connected on first use. A failed connection is tried again on the next call.
pub struct Access {
    bus: Option<String>,
    tree: Option<AtspiTree>,
}

impl Access {
    /// The tree on the session bus this process was started with (`DBUS_SESSION_BUS_ADDRESS`).
    pub fn from_env() -> Self {
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        Self::with_bus(bus)
    }

    /// The tree on the session bus at `bus`, or none when there is no bus.
    pub fn with_bus(bus: Option<String>) -> Self {
        Self {
            bus: bus.filter(|v| !v.is_empty()),
            tree: None,
        }
    }

    fn connected(&mut self) -> Result<&AtspiTree> {
        if self.tree.is_none() {
            let bus = self.bus.as_deref().ok_or_else(|| {
                DesktopError::Unsupported(
                    "the UI tree needs a session bus: DBUS_SESSION_BUS_ADDRESS is unset".into(),
                )
            })?;
            self.tree = Some(AtspiTree::connect(bus)?);
        }
        self.tree
            .as_ref()
            .ok_or_else(|| DesktopError::Platform("no accessibility connection".into()))
    }

    pub fn tree(&mut self, app: Option<&str>, depth: u8) -> Result<UiNode> {
        self.connected()?.tree(app, depth)
    }

    pub fn act(&mut self, selector: &Selector, action: &UiAction) -> Result<()> {
        self.connected()?.act(selector, action)
    }
}

/// The AT-SPI hook of an X11 backend. Answers only for `owner_display`, the owner's `DISPLAY`.
pub struct OwnerUi {
    owner_display: String,
    access: Access,
}

impl OwnerUi {
    pub fn new(owner_display: String, access: Access) -> Self {
        Self {
            owner_display,
            access,
        }
    }

    fn check(&self, display: &str) -> Result<()> {
        if !self.owner_display.is_empty() && display == self.owner_display {
            Ok(())
        } else {
            Err(DesktopError::Unsupported(
                "a pixel session's UI tree is not built: its display has no accessibility bus here"
                    .into(),
            ))
        }
    }
}

impl UiHook for OwnerUi {
    fn ui_tree(&mut self, display: &str, app: Option<&str>, depth: u8) -> Result<UiNode> {
        self.check(display)?;
        self.access.tree(app, depth)
    }

    fn ui_act(&mut self, display: &str, selector: &Selector, action: &UiAction) -> Result<()> {
        self.check(display)?;
        self.access.act(selector, action)
    }
}

/// The Wayland backend with the owner's UI tree from AT-SPI. Everything but the tree is the portal backend's.
pub struct WaylandOwner {
    inner: WaylandDesktop,
    access: Access,
}

impl WaylandOwner {
    pub fn new(inner: WaylandDesktop, access: Access) -> Self {
        Self { inner, access }
    }
}

impl Desktop for WaylandOwner {
    fn open_session(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        self.inner.open_session(spec)
    }

    fn close_session(&mut self, id: SessionId) -> Result<()> {
        self.inner.close_session(id)
    }

    fn capture(&mut self, id: SessionId) -> Result<Frame> {
        self.inner.capture(id)
    }

    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()> {
        self.inner.input(id, action)
    }

    fn ui_tree(&mut self, _id: SessionId, app: Option<&str>, depth: u8) -> Result<UiNode> {
        self.access.tree(app, depth)
    }

    fn ui_act(&mut self, _id: SessionId, selector: &Selector, action: &UiAction) -> Result<()> {
        self.access.act(selector, action)
    }

    fn apps(&mut self) -> Result<Vec<apz_desktop::AppInfo>> {
        self.inner.apps()
    }

    fn launch(&mut self, id: SessionId, app: &str, args: &[String]) -> Result<()> {
        self.inner.launch(id, app, args)
    }

    fn owner_active(&self, id: SessionId) -> bool {
        self.inner.owner_active(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_ui_check_accepts_only_the_owners_display() {
        // only_the_owners_display_has_a_tree: the owner's display gets a tree,
        // a pixel display is refused.
        {
            let hook = OwnerUi::new(":1".into(), Access::with_bus(None));
            assert!(hook.check(":1").is_ok());
            assert!(hook.check(":7").is_err(), "a pixel display is refused");
        }
        // with_no_owner_display_nothing_has_a_tree: with no owner display,
        // nothing has a tree.
        {
            let hook = OwnerUi::new(String::new(), Access::with_bus(None));
            assert!(hook.check("").is_err());
        }
    }

    #[test]
    fn without_a_session_bus_the_tree_is_unsupported() {
        let mut access = Access::with_bus(None);
        assert!(matches!(
            access.tree(None, 1),
            Err(DesktopError::Unsupported(_))
        ));
    }
}
