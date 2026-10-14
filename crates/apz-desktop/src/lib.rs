//! The desktop control interface of a workstation.
//!
//! The workstation daemon serves the station's screen, input, UI-tree and app operations through the
//! [`Desktop`] trait. Each platform backend (macOS, Windows, Linux) implements the trait; this crate has no
//! platform code. It holds the shared parts every backend and the daemon use:
//!
//! - [`Desktop`] and its types ([`SessionSpec`], [`Frame`], [`UiNode`], [`AppInfo`], [`DesktopError`]);
//! - the frame helpers: [`scale`], [`grid_overlay`], [`encode`] (PNG or JPEG), and [`map_point`] for
//!   screenshot pixels to screen points;
//! - [`parse_combo`], which turns `"ctrl+shift+s"` into modifiers and a key, with names every platform uses;
//! - [`FakeDesktop`], an in-memory desktop for tests of the daemon and the hub.
//!
//! The wire types (inputs, selectors, UI actions, bridges, image formats) come from `apz-station`, so the
//! daemon can pass a request straight through to a backend.

mod desktop;
mod fake;
mod frame;
mod keys;

pub use desktop::{
    AppInfo, Desktop, DesktopError, Frame, Result, SessionSpec, UiNode, action_name,
};
pub use fake::{FakeDesktop, Launch, UiActLog};
pub use frame::{encode, grid_overlay, map_point, scale};
pub use keys::{Key, KeyCombo, Mods, parse_combo};
