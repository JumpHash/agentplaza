//! The desktop control trait and the plain types it trades in.

use apz_station::{Bridge, InputAction, Selector, SessionId, SessionKind, UiAction};
use serde::{Deserialize, Serialize};

/// What the daemon's platform backends return and the daemon maps to `ok: false`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DesktopError {
    #[error("no session {0}")]
    UnknownSession(SessionId),
    /// The platform cannot do it (no UI tree on this machine, no scripting bridge for the app).
    #[error("not supported here: {0}")]
    Unsupported(String),
    /// The OS or the owner has not granted it (macOS Screen Recording, the Wayland portal).
    #[error("not permitted: {0}")]
    NotPermitted(String),
    /// No such app, window, or control.
    #[error("not found: {0}")]
    NotFound(String),
    /// The arguments are wrong (a zero-sized session, an ambiguous selector, a bad key name).
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// Encoding an image failed.
    #[error("encode failed: {0}")]
    Encode(String),
    /// Anything else the platform reports.
    #[error("{0}")]
    Platform(String),
}

/// The result type of every fallible call on [`Desktop`].
pub type Result<T> = std::result::Result<T, DesktopError>;

/// What a new session is: whose it is, what kind, and its size in screen points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    pub agent: u64,
    pub kind: SessionKind,
    pub width: u32,
    pub height: u32,
}

/// A screen capture: `rgba` is `w * h * 4` bytes, row-major, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// A frame of `w` by `h` pixels, or an error when the size and the buffer disagree.
    pub fn new(w: u32, h: u32, rgba: Vec<u8>) -> Result<Self> {
        let frame = Self { w, h, rgba };
        frame.check()?;
        Ok(frame)
    }

    /// Checks that the buffer holds exactly `w * h` RGBA pixels and that the frame is not empty.
    pub fn check(&self) -> Result<()> {
        if self.w == 0 || self.h == 0 {
            return Err(DesktopError::Invalid("a frame has no pixels".into()));
        }
        let expected = (self.w as usize)
            .checked_mul(self.h as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        if expected != Some(self.rgba.len()) {
            return Err(DesktopError::Invalid(format!(
                "a {}x{} frame needs {} bytes, not {}",
                self.w,
                self.h,
                (self.w as u64)
                    .saturating_mul(self.h as u64)
                    .saturating_mul(4),
                self.rgba.len()
            )));
        }
        Ok(())
    }
}

/// A node of the accessibility tree. `role` is the normalised lower-snake name (`button`, `text_field`,
/// `check_box`, `window`); `bounds` is `(x, y, w, h)` in screen points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiNode {
    pub id: u64,
    pub role: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub bounds: (i32, i32, u32, u32),
    /// The accessibility actions the control takes, by name: `invoke`, `set_value`, `focus`, `select`,
    /// `toggle`, `expand`, `collapse`.
    pub actions: Vec<String>,
    pub children: Vec<UiNode>,
}

/// An installed application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    /// The bundle id or the executable name; stable across launches.
    pub id: String,
    /// The name people see; `Selector.app` and `Op::Apps` take this.
    pub name: String,
    pub path: String,
    /// The scripting bridges this app offers, when it has any.
    pub scripting: Vec<Bridge>,
}

/// The desktop control interface. A platform backend (macOS, Windows, Linux) implements it, and the
/// daemon serves the station's operations through it. Every call is made from the daemon's one request
/// loop, so a backend never runs two calls at once.
pub trait Desktop: Send {
    /// Opens a session for an agent: a pixel session takes a virtual or the shared screen, a shared one
    /// is the owner's own session.
    fn open_session(&mut self, spec: &SessionSpec) -> Result<SessionId>;
    fn close_session(&mut self, id: SessionId) -> Result<()>;
    /// The session's screen now, at full size.
    fn capture(&mut self, id: SessionId) -> Result<Frame>;
    /// One pointer or keyboard action, in screen points.
    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()>;
    /// The UI tree of the session, or of `app`'s windows, down to `depth` levels below the root.
    fn ui_tree(&mut self, id: SessionId, app: Option<&str>, depth: u8) -> Result<UiNode>;
    /// An accessibility action on the control `selector` picks.
    fn ui_act(&mut self, id: SessionId, selector: &Selector, action: &UiAction) -> Result<()>;
    fn apps(&mut self) -> Result<Vec<AppInfo>>;
    fn launch(&mut self, id: SessionId, app: &str, args: &[String]) -> Result<()>;
    /// Brings the app `id` (its [`AppInfo::id`], or its name) to the front. Unsupported unless the platform
    /// can do it without a window the daemon cannot name.
    fn focus(&mut self, id: &str) -> Result<()> {
        let _ = id;
        Err(DesktopError::Unsupported(
            "this backend cannot focus an app".into(),
        ))
    }
    /// Asks the app `id` to quit. `force` ends it without giving it a chance to save. Unsupported unless the
    /// platform can do it.
    fn quit(&mut self, id: &str, force: bool) -> Result<()> {
        let _ = (id, force);
        Err(DesktopError::Unsupported(
            "this backend cannot quit an app".into(),
        ))
    }
    /// Whether the owner is using this session right now (their input moved it recently).
    fn owner_active(&self, id: SessionId) -> bool;
}

/// The wire name of a [`UiAction`], as the actions list of a [`UiNode`] spells it.
pub fn action_name(action: &UiAction) -> &'static str {
    match action {
        UiAction::Invoke => "invoke",
        UiAction::SetValue { .. } => "set_value",
        UiAction::Focus => "focus",
        UiAction::Select => "select",
        UiAction::Toggle => "toggle",
        UiAction::Expand => "expand",
        UiAction::Collapse => "collapse",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_needs_exactly_four_bytes_per_pixel() {
        assert!(Frame::new(2, 2, vec![0; 16]).is_ok());
        assert!(Frame::new(2, 2, vec![0; 15]).is_err());
        assert!(Frame::new(0, 2, vec![]).is_err());
        assert!(Frame::new(u32::MAX, u32::MAX, vec![]).is_err());
    }

    #[test]
    fn every_action_has_its_wire_name() {
        assert_eq!(action_name(&UiAction::Invoke), "invoke");
        assert_eq!(
            action_name(&UiAction::SetValue {
                value: String::new()
            }),
            "set_value"
        );
        assert_eq!(action_name(&UiAction::Focus), "focus");
        assert_eq!(action_name(&UiAction::Select), "select");
        assert_eq!(action_name(&UiAction::Toggle), "toggle");
        assert_eq!(action_name(&UiAction::Expand), "expand");
        assert_eq!(action_name(&UiAction::Collapse), "collapse");
    }
}
