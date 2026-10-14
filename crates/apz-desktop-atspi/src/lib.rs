//! The Linux accessibility layer of a workstation: a session's AT-SPI UI tree and accessibility actions, and the
//! owner's idle time from logind.
//!
//! The X11 and Wayland backends call [`AtspiTree`] for `ui_tree` and `ui_act`. [`AtspiTree::connect`] takes the
//! session bus address to start from, so the accessibility bus it reaches is always the one that session
//! advertises. [`idle_ms`] reads logind's idle hint, which tells the daemon when the owner last used the machine.
//!
//! Other targets compile this crate empty: AT-SPI and logind are Linux D-Bus services.

#![cfg(target_os = "linux")]

mod act;
mod actions;
mod logind;
mod role;
mod select;
mod tree;
mod walk;

pub use logind::idle_ms;
pub use tree::AtspiTree;
